//! Durable Fault records: what failed, where, and whether it still fails.
//!
//! A Fault is the debugging counterpart to a Candidate. It is created from
//! observed evidence (an exit status plus a bounded output slice), it can be
//! replayed on demand, and it leaves `Open` only when a replay passes or the
//! owner dismisses it with a note. Nothing here interprets output with a
//! model; classification comes from the reporter and is recorded as stated.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use termi9ne_core::{FaultId, MissionId, RunId, SessionId};
use termi9ne_protocol::{
    FaultInput, FaultSource, FaultState, FaultSummary, ReproReceipt,
};
use thiserror::Error;

const FILE_VERSION: u16 = 1;
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FAULTS: usize = 2_048;
const MAX_COMMAND_BYTES: usize = 4_096;
const MAX_SUMMARY_BYTES: usize = 1_024;
const MAX_NOTE_BYTES: usize = 2_048;
const MAX_REVISION_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 4_096;
/// Output slices stay small enough to read, forward to an agent, and store
/// without turning the record into a log file.
pub(crate) const MAX_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Debug, Error)]
pub(crate) enum FaultError {
    #[error("Fault store is not a regular owner-controlled file: {0}")]
    Insecure(PathBuf),
    #[error("Fault store exceeds 16 MiB")]
    TooLarge,
    #[error("Fault store could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("Fault store is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported Fault store version {0}")]
    Version(u16),
    #[error("Fault store exceeds {MAX_FAULTS} records")]
    TooManyFaults,
    #[error("Fault command is empty or too long")]
    InvalidCommand,
    #[error("Fault working directory must be an absolute path")]
    InvalidDirectory,
    #[error("Fault summary is empty or too long")]
    InvalidSummary,
    #[error("Fault note is empty or too long")]
    InvalidNote,
    #[error("Fault revision is too long")]
    InvalidRevision,
    #[error("Fault store key does not match its record")]
    MismatchedKey,
    #[error("no Fault {0}")]
    Unknown(FaultId),
    #[error("Fault {0} is already closed")]
    AlreadyClosed(FaultId),
    #[error(
        "Fault {0} cannot be resolved until a replay passes; reproduce it first or dismiss it with a note"
    )]
    Unproven(FaultId),
    #[error("system clock is before the Unix epoch")]
    Clock,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FaultFile {
    version: u16,
    faults: BTreeMap<String, FaultSummary>,
}

pub(crate) struct FaultStore {
    path: PathBuf,
    faults: BTreeMap<String, FaultSummary>,
}

impl FaultStore {
    pub(crate) fn open(path: PathBuf) -> Result<Self, FaultError> {
        let faults = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                // SAFETY: geteuid has no preconditions and reads process metadata.
                let current_uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.uid() != current_uid
                {
                    return Err(FaultError::Insecure(path));
                }
                if metadata.len() > MAX_FILE_BYTES {
                    return Err(FaultError::TooLarge);
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                let file: FaultFile = serde_json::from_slice(&fs::read(&path)?)?;
                if file.version != FILE_VERSION {
                    return Err(FaultError::Version(file.version));
                }
                if file.faults.len() > MAX_FAULTS {
                    return Err(FaultError::TooManyFaults);
                }
                for (key, fault) in &file.faults {
                    validate_summary(fault)?;
                    if key != &fault.fault_id.to_string() {
                        return Err(FaultError::MismatchedKey);
                    }
                }
                file.faults
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self { path, faults })
    }

    #[cfg(test)]
    pub(crate) fn transient() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "termi9ne-fault-transient-{}.json",
                uuid::Uuid::new_v4()
            )),
            faults: BTreeMap::new(),
        }
    }

    /// Record a newly observed failure.
    pub(crate) fn report(
        &mut self,
        input: FaultInput,
        source: FaultSource,
    ) -> Result<FaultSummary, FaultError> {
        let fault = FaultSummary {
            fault_id: FaultId::new(),
            kind: input.kind,
            command: input.command,
            cwd: input.cwd,
            exit_code: input.exit_code,
            revision: input.revision,
            summary: input.summary,
            output: truncate_output(&input.output),
            session_id: input.session_id,
            mission_id: input.mission_id,
            run_id: input.run_id,
            source,
            observed_at_unix_micros: now_micros()?,
            state: FaultState::Open,
            repro: None,
            repro_attempts: 0,
            fix_run_id: None,
        };
        validate_summary(&fault)?;
        if self.faults.len() >= MAX_FAULTS {
            self.evict_oldest_closed()?;
        }
        self.faults.insert(fault.fault_id.to_string(), fault.clone());
        self.persist()?;
        Ok(fault)
    }

    pub(crate) fn get(&self, fault_id: FaultId) -> Result<FaultSummary, FaultError> {
        self.faults
            .get(&fault_id.to_string())
            .cloned()
            .ok_or(FaultError::Unknown(fault_id))
    }

    /// Faults newest first, optionally narrowed to one Mission or Session.
    pub(crate) fn list(
        &self,
        mission_id: Option<MissionId>,
        session_id: Option<SessionId>,
        include_closed: bool,
    ) -> Vec<FaultSummary> {
        let mut faults = self
            .faults
            .values()
            .filter(|fault| mission_id.is_none_or(|id| fault.mission_id == Some(id)))
            .filter(|fault| session_id.is_none_or(|id| fault.session_id == Some(id)))
            .filter(|fault| include_closed || fault.is_open())
            .cloned()
            .collect::<Vec<_>>();
        faults.sort_by(|left, right| {
            right
                .observed_at_unix_micros
                .cmp(&left.observed_at_unix_micros)
                .then_with(|| left.fault_id.to_string().cmp(&right.fault_id.to_string()))
        });
        faults
    }

    /// Attach a replay receipt. A passing replay does not close the Fault by
    /// itself; the owner still decides, but only then may they resolve it.
    pub(crate) fn record_repro(
        &mut self,
        fault_id: FaultId,
        receipt: ReproReceipt,
    ) -> Result<FaultSummary, FaultError> {
        let fault = self
            .faults
            .get_mut(&fault_id.to_string())
            .ok_or(FaultError::Unknown(fault_id))?;
        fault.repro_attempts = fault.repro_attempts.saturating_add(1);
        fault.repro = Some(ReproReceipt {
            output: truncate_output(&receipt.output),
            ..receipt
        });
        let updated = fault.clone();
        self.persist()?;
        Ok(updated)
    }

    /// Record which Run has been asked to fix this Fault. Assigning a Run
    /// makes no claim about the outcome; only a replay can do that.
    pub(crate) fn assign_fix(
        &mut self,
        fault_id: FaultId,
        run_id: RunId,
    ) -> Result<FaultSummary, FaultError> {
        let fault = self
            .faults
            .get_mut(&fault_id.to_string())
            .ok_or(FaultError::Unknown(fault_id))?;
        if !fault.is_open() {
            return Err(FaultError::AlreadyClosed(fault_id));
        }
        fault.fix_run_id = Some(run_id);
        let updated = fault.clone();
        self.persist()?;
        Ok(updated)
    }

    /// Close a Fault as fixed. Refused unless its latest replay passed, so a
    /// Fault can never be closed by assertion alone.
    pub(crate) fn resolve(
        &mut self,
        fault_id: FaultId,
        note: String,
    ) -> Result<FaultSummary, FaultError> {
        validate_note(&note)?;
        let at_unix_micros = now_micros()?;
        let fault = self
            .faults
            .get_mut(&fault_id.to_string())
            .ok_or(FaultError::Unknown(fault_id))?;
        if !fault.is_open() {
            return Err(FaultError::AlreadyClosed(fault_id));
        }
        if !fault.repro_passes() {
            return Err(FaultError::Unproven(fault_id));
        }
        fault.state = FaultState::Resolved {
            note,
            at_unix_micros,
        };
        let updated = fault.clone();
        self.persist()?;
        Ok(updated)
    }

    /// Close a Fault without repro evidence, recording the owner's reason.
    pub(crate) fn dismiss(
        &mut self,
        fault_id: FaultId,
        note: String,
    ) -> Result<FaultSummary, FaultError> {
        validate_note(&note)?;
        let at_unix_micros = now_micros()?;
        let fault = self
            .faults
            .get_mut(&fault_id.to_string())
            .ok_or(FaultError::Unknown(fault_id))?;
        if !fault.is_open() {
            return Err(FaultError::AlreadyClosed(fault_id));
        }
        fault.state = FaultState::Dismissed {
            note,
            at_unix_micros,
        };
        let updated = fault.clone();
        self.persist()?;
        Ok(updated)
    }

    /// Make room by dropping the oldest closed Fault. Open Faults are never
    /// evicted: losing an unresolved failure silently is worse than refusing.
    fn evict_oldest_closed(&mut self) -> Result<(), FaultError> {
        let oldest = self
            .faults
            .values()
            .filter(|fault| !fault.is_open())
            .min_by_key(|fault| fault.observed_at_unix_micros)
            .map(|fault| fault.fault_id.to_string());
        match oldest {
            Some(key) => {
                self.faults.remove(&key);
                Ok(())
            }
            None => Err(FaultError::TooManyFaults),
        }
    }

    fn persist(&self) -> Result<(), FaultError> {
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Fault path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        // The temporary name is unique so concurrent writers cannot collide,
        // and it is removed on any failure rather than left behind.
        let temporary = parent.join(format!(".faults-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut handle = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            serde_json::to_writer_pretty(
                &mut handle,
                &FaultFile {
                    version: FILE_VERSION,
                    faults: self.faults.clone(),
                },
            )?;
            handle.write_all(b"\n")?;
            handle.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

/// Keep the head and tail of long output: the command line and the first error
/// usually sit at the top, the failing assertion at the bottom.
pub(crate) fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_BYTES {
        return output.to_owned();
    }
    let head_budget = MAX_OUTPUT_BYTES / 2;
    let tail_budget = MAX_OUTPUT_BYTES - head_budget;
    let head_end = floor_char_boundary(output, head_budget);
    let tail_start = ceil_char_boundary(output, output.len().saturating_sub(tail_budget));
    let omitted = tail_start.saturating_sub(head_end);
    format!(
        "{}\n… {omitted} bytes omitted …\n{}",
        &output[..head_end],
        &output[tail_start..]
    )
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

fn validate_summary(fault: &FaultSummary) -> Result<(), FaultError> {
    let command = fault.command.trim();
    if command.is_empty() || fault.command.len() > MAX_COMMAND_BYTES {
        return Err(FaultError::InvalidCommand);
    }
    if !fault.cwd.is_absolute() || fault.cwd.as_os_str().len() > MAX_PATH_BYTES {
        return Err(FaultError::InvalidDirectory);
    }
    if fault.summary.trim().is_empty() || fault.summary.len() > MAX_SUMMARY_BYTES {
        return Err(FaultError::InvalidSummary);
    }
    if fault
        .revision
        .as_ref()
        .is_some_and(|revision| revision.len() > MAX_REVISION_BYTES)
    {
        return Err(FaultError::InvalidRevision);
    }
    if fault.output.len() > MAX_OUTPUT_BYTES * 2 {
        return Err(FaultError::InvalidSummary);
    }
    match &fault.state {
        FaultState::Open => {}
        FaultState::Resolved { note, .. } | FaultState::Dismissed { note, .. } => {
            validate_note(note)?;
        }
    }
    Ok(())
}

fn validate_note(note: &str) -> Result<(), FaultError> {
    if note.trim().is_empty() || note.len() > MAX_NOTE_BYTES {
        return Err(FaultError::InvalidNote);
    }
    Ok(())
}

fn now_micros() -> Result<u64, FaultError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros() as u64)
        .map_err(|_| FaultError::Clock)
}

/// Directory a replay should run in. Symlinks are followed deliberately:
/// common working directories are symlinked (`/tmp` on macOS), and the replay
/// already runs an owner-supplied command, so refusing links would block
/// ordinary use without adding authority.
pub(crate) fn repro_directory(cwd: &Path) -> Result<PathBuf, String> {
    let metadata =
        fs::metadata(cwd).map_err(|error| format!("{} is unavailable: {error}", cwd.display()))?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a directory", cwd.display()));
    }
    Ok(cwd.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use termi9ne_protocol::FaultKind;

    fn input(command: &str) -> FaultInput {
        FaultInput {
            kind: FaultKind::TestFailed,
            command: command.to_owned(),
            cwd: PathBuf::from("/tmp"),
            exit_code: Some(1),
            revision: Some("abc123".to_owned()),
            summary: "2 tests failed".to_owned(),
            output: "assertion failed".to_owned(),
            session_id: None,
            mission_id: None,
            run_id: None,
        }
    }

    fn receipt(reproduced: bool) -> ReproReceipt {
        ReproReceipt {
            attempted_at_unix_micros: 1,
            reproduced,
            exit_code: Some(if reproduced { 1 } else { 0 }),
            output: "replayed".to_owned(),
            duration_ms: 12,
            revision: Some("abc123".to_owned()),
            error: None,
        }
    }

    #[test]
    fn reported_faults_open_and_list_newest_first() {
        let mut store = FaultStore::transient();
        let first = store
            .report(input("cargo test"), FaultSource::OwnerHook)
            .expect("first fault");
        let second = store
            .report(input("cargo build"), FaultSource::TerminalExit)
            .expect("second fault");
        assert!(first.is_open() && second.is_open());
        assert_eq!(first.repro_attempts, 0);
        assert!(first.repro.is_none());

        let listed = store.list(None, None, false);
        assert_eq!(listed.len(), 2);
        assert!(listed[0].observed_at_unix_micros >= listed[1].observed_at_unix_micros);
        assert_eq!(store.get(second.fault_id).expect("get").command, "cargo build");
        assert!(matches!(
            store.get(FaultId::new()),
            Err(FaultError::Unknown(_))
        ));
    }

    #[test]
    fn resolution_requires_a_passing_replay() {
        let mut store = FaultStore::transient();
        let fault = store
            .report(input("cargo test"), FaultSource::OwnerHook)
            .expect("fault");

        // No replay yet: refused.
        assert!(matches!(
            store.resolve(fault.fault_id, "fixed it".to_owned()),
            Err(FaultError::Unproven(_))
        ));

        // A replay that still fails keeps it refused.
        let still_failing = store
            .record_repro(fault.fault_id, receipt(true))
            .expect("repro recorded");
        assert_eq!(still_failing.repro_attempts, 1);
        assert!(!still_failing.repro_passes());
        assert!(matches!(
            store.resolve(fault.fault_id, "fixed it".to_owned()),
            Err(FaultError::Unproven(_))
        ));

        // A passing replay permits resolution exactly once.
        let passing = store
            .record_repro(fault.fault_id, receipt(false))
            .expect("repro recorded");
        assert!(passing.repro_passes());
        assert_eq!(passing.repro_attempts, 2);
        let resolved = store
            .resolve(fault.fault_id, "fixed by run 7".to_owned())
            .expect("resolve");
        assert!(!resolved.is_open());
        assert!(matches!(
            resolved.state,
            FaultState::Resolved { ref note, .. } if note == "fixed by run 7"
        ));
        assert!(matches!(
            store.resolve(fault.fault_id, "again".to_owned()),
            Err(FaultError::AlreadyClosed(_))
        ));
        assert!(store.list(None, None, false).is_empty());
        assert_eq!(store.list(None, None, true).len(), 1);
    }

    #[test]
    fn a_replay_that_could_not_run_is_not_proof() {
        let mut store = FaultStore::transient();
        let fault = store
            .report(input("cargo test"), FaultSource::OwnerHook)
            .expect("fault");
        let broken = ReproReceipt {
            reproduced: false,
            error: Some("directory is unavailable".to_owned()),
            ..receipt(false)
        };
        let updated = store
            .record_repro(fault.fault_id, broken)
            .expect("repro recorded");
        assert!(!updated.repro_passes());
        assert!(matches!(
            store.resolve(fault.fault_id, "trust me".to_owned()),
            Err(FaultError::Unproven(_))
        ));
    }

    #[test]
    fn assigning_a_fix_run_records_it_without_claiming_success() {
        let mut store = FaultStore::transient();
        let fault = store
            .report(input("cargo test"), FaultSource::OwnerHook)
            .expect("fault");
        assert!(fault.fix_run_id.is_none());

        let run_id = RunId::new();
        let assigned = store.assign_fix(fault.fault_id, run_id).expect("assign");
        assert_eq!(assigned.fix_run_id, Some(run_id));
        // Assignment is not evidence: the Fault stays open and unresolvable.
        assert!(assigned.is_open());
        assert!(matches!(
            store.resolve(fault.fault_id, "the agent said so".to_owned()),
            Err(FaultError::Unproven(_))
        ));

        // A second attempt replaces the first.
        let retry = RunId::new();
        assert_eq!(
            store.assign_fix(fault.fault_id, retry).expect("reassign").fix_run_id,
            Some(retry)
        );

        // A closed Fault takes no new assignment.
        store
            .dismiss(fault.fault_id, "not worth fixing".to_owned())
            .expect("dismiss");
        assert!(matches!(
            store.assign_fix(fault.fault_id, RunId::new()),
            Err(FaultError::AlreadyClosed(_))
        ));
        assert!(matches!(
            store.assign_fix(FaultId::new(), RunId::new()),
            Err(FaultError::Unknown(_))
        ));
    }

    #[test]
    fn the_fix_objective_states_the_evidence_and_the_acceptance_test() {
        let mut store = FaultStore::transient();
        let fault = store
            .report(input("cargo test -p thing"), FaultSource::OwnerHook)
            .expect("fault");
        let objective = fault.fix_objective();
        assert!(objective.contains("cargo test -p thing"));
        assert!(objective.contains("/tmp"));
        assert!(objective.contains("exit: 1"));
        assert!(objective.contains("assertion failed"));
        // The acceptance test is the replay, not the agent's own account.
        assert!(objective.contains("verified by replay"));
    }

    #[test]
    fn dismissal_needs_a_note_but_no_replay() {
        let mut store = FaultStore::transient();
        let fault = store
            .report(input("flaky"), FaultSource::OwnerHook)
            .expect("fault");
        assert!(matches!(
            store.dismiss(fault.fault_id, "   ".to_owned()),
            Err(FaultError::InvalidNote)
        ));
        let dismissed = store
            .dismiss(fault.fault_id, "known upstream flake".to_owned())
            .expect("dismiss");
        assert!(!dismissed.is_open());
        assert!(matches!(dismissed.state, FaultState::Dismissed { .. }));
    }

    #[test]
    fn malformed_reports_are_refused() {
        let mut store = FaultStore::transient();
        let mut empty_command = input("   ");
        empty_command.summary = "x".to_owned();
        assert!(matches!(
            store.report(empty_command, FaultSource::OwnerHook),
            Err(FaultError::InvalidCommand)
        ));

        let mut relative = input("cargo test");
        relative.cwd = PathBuf::from("relative/path");
        assert!(matches!(
            store.report(relative, FaultSource::OwnerHook),
            Err(FaultError::InvalidDirectory)
        ));

        let mut blank_summary = input("cargo test");
        blank_summary.summary = "  ".to_owned();
        assert!(matches!(
            store.report(blank_summary, FaultSource::OwnerHook),
            Err(FaultError::InvalidSummary)
        ));

        let mut long_revision = input("cargo test");
        long_revision.revision = Some("a".repeat(MAX_REVISION_BYTES + 1));
        assert!(matches!(
            store.report(long_revision, FaultSource::OwnerHook),
            Err(FaultError::InvalidRevision)
        ));
    }

    #[test]
    fn long_output_keeps_both_ends_and_stays_bounded() {
        let mut store = FaultStore::transient();
        let mut noisy = input("cargo test");
        noisy.output = format!("HEAD{}TAIL", "x".repeat(MAX_OUTPUT_BYTES * 2));
        let fault = store
            .report(noisy, FaultSource::OwnerHook)
            .expect("fault");
        assert!(fault.output.starts_with("HEAD"));
        assert!(fault.output.ends_with("TAIL"));
        assert!(fault.output.contains("bytes omitted"));
        assert!(fault.output.len() <= MAX_OUTPUT_BYTES + 64);
        // Multi-byte characters must not be split.
        let wide = truncate_output(&"é".repeat(MAX_OUTPUT_BYTES));
        assert!(wide.contains("bytes omitted"));
    }

    #[test]
    fn faults_filter_by_mission_and_session() {
        let mut store = FaultStore::transient();
        let mission = MissionId::new();
        let session = SessionId::new();
        let mut scoped = input("cargo test");
        scoped.mission_id = Some(mission);
        scoped.session_id = Some(session);
        store
            .report(scoped, FaultSource::AuthenticatedAgent)
            .expect("scoped fault");
        store
            .report(input("cargo build"), FaultSource::OwnerHook)
            .expect("loose fault");

        assert_eq!(store.list(Some(mission), None, false).len(), 1);
        assert_eq!(store.list(None, Some(session), false).len(), 1);
        assert_eq!(store.list(Some(MissionId::new()), None, false).len(), 0);
        assert_eq!(store.list(None, None, false).len(), 2);
    }

    #[test]
    fn faults_round_trip_through_an_owner_only_file() {
        let root = std::env::temp_dir().join(format!("termi9ne-fault-{}", uuid::Uuid::new_v4()));
        let path = root.join("faults.json");
        let mut store = FaultStore::open(path.clone()).expect("open");
        let fault = store
            .report(input("cargo test"), FaultSource::OwnerHook)
            .expect("fault");
        store
            .record_repro(fault.fault_id, receipt(true))
            .expect("repro");

        assert_eq!(
            fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let reopened = FaultStore::open(path).expect("reopen");
        let restored = reopened.get(fault.fault_id).expect("restored");
        assert_eq!(restored.command, "cargo test");
        assert_eq!(restored.repro_attempts, 1);
        assert!(restored.is_open());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn repro_directory_refuses_anything_but_a_real_directory() {
        assert!(repro_directory(Path::new("/tmp")).is_ok());
        assert!(repro_directory(Path::new("/tmp/termi9ne-does-not-exist-xyz")).is_err());
    }
}
