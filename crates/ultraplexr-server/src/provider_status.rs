use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use ultraplexr_core::{FinishOutcome, Mission, MissionId, RunId, RunPhase, SignalKind};
use ultraplexr_protocol::{
    ProviderActivityState, ProviderFactInput, ProviderFactSource, ProviderFactSummary,
    RunActivitySource, RunActivityState, RunActivitySummary,
};
use thiserror::Error;

const FILE_VERSION: u16 = 1;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FACTS: usize = 4_096;
const MAX_PROVIDER_BYTES: usize = 128;
const MAX_ADAPTER_VERSION_BYTES: usize = 64;
const MAX_SUMMARY_BYTES: usize = 1_024;
const MIN_VALID_SECONDS: u16 = 1;
const MAX_VALID_SECONDS: u16 = 3_600;

#[derive(Debug, Error)]
pub(crate) enum ProviderStatusError {
    #[error("provider fact store is not a regular owner-controlled file: {0}")]
    Insecure(PathBuf),
    #[error("provider fact store exceeds 4 MiB")]
    TooLarge,
    #[error("provider fact store could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("provider fact store is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported provider fact store version {0}")]
    Version(u16),
    #[error("provider fact store exceeds {MAX_FACTS} Runs")]
    TooManyFacts,
    #[error("provider ID must contain 1 to {MAX_PROVIDER_BYTES} bytes and no NUL")]
    InvalidProvider,
    #[error(
        "adapter version must contain 1 to {MAX_ADAPTER_VERSION_BYTES} bytes and no NUL"
    )]
    InvalidAdapterVersion,
    #[error("provider summary must contain 1 to {MAX_SUMMARY_BYTES} bytes and no NUL")]
    InvalidSummary,
    #[error("provider fact validity must be between 1 second and 1 hour")]
    InvalidValidity,
    #[error("provider fact store key does not match its Run")]
    InvalidKey,
    #[error("system clock is before the Unix epoch")]
    Clock,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProviderStatusFile {
    version: u16,
    facts: BTreeMap<RunId, ProviderFactSummary>,
}

pub(crate) struct ProviderStatusStore {
    path: PathBuf,
    facts: BTreeMap<RunId, ProviderFactSummary>,
    announced_expired: BTreeSet<RunId>,
}

impl ProviderStatusStore {
    pub(crate) fn open(path: PathBuf) -> Result<Self, ProviderStatusError> {
        let facts = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                // SAFETY: geteuid has no preconditions and reads process metadata.
                let current_uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.uid() != current_uid
                {
                    return Err(ProviderStatusError::Insecure(path));
                }
                if metadata.len() > MAX_FILE_BYTES {
                    return Err(ProviderStatusError::TooLarge);
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                let file: ProviderStatusFile = serde_json::from_slice(&fs::read(&path)?)?;
                if file.version != FILE_VERSION {
                    return Err(ProviderStatusError::Version(file.version));
                }
                if file.facts.len() > MAX_FACTS {
                    return Err(ProviderStatusError::TooManyFacts);
                }
                for (run_id, fact) in &file.facts {
                    if *run_id != fact.run_id {
                        return Err(ProviderStatusError::InvalidKey);
                    }
                    validate_summary(fact)?;
                }
                file.facts
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            facts,
            announced_expired: BTreeSet::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn transient() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "ultraplexr-provider-transient-{}.json",
                uuid::Uuid::new_v4()
            )),
            facts: BTreeMap::new(),
            announced_expired: BTreeSet::new(),
        }
    }

    pub(crate) fn get(&self, run_id: RunId) -> Option<&ProviderFactSummary> {
        self.facts.get(&run_id)
    }

    pub(crate) fn record(
        &mut self,
        mission_id: MissionId,
        run_id: RunId,
        input: ProviderFactInput,
        source: ProviderFactSource,
    ) -> Result<ProviderFactSummary, ProviderStatusError> {
        validate_input(&input)?;
        if !self.facts.contains_key(&run_id) && self.facts.len() == MAX_FACTS {
            return Err(ProviderStatusError::TooManyFacts);
        }
        let observed_at_unix_micros = now_unix_micros()?;
        let expires_at_unix_micros = observed_at_unix_micros.saturating_add(
            u64::from(input.valid_for_seconds).saturating_mul(1_000_000),
        );
        let fact = ProviderFactSummary {
            mission_id,
            run_id,
            provider_id: input.provider_id,
            adapter_version: input.adapter_version,
            state: input.state,
            summary: input.summary,
            source,
            observed_at_unix_micros,
            expires_at_unix_micros,
        };
        let previous = self.facts.insert(run_id, fact.clone());
        self.announced_expired.remove(&run_id);
        if let Err(error) = self.persist() {
            match previous {
                Some(previous) => {
                    self.facts.insert(run_id, previous);
                }
                None => {
                    self.facts.remove(&run_id);
                }
            }
            return Err(error);
        }
        Ok(fact)
    }

    pub(crate) fn take_newly_expired(&mut self, now_unix_micros: u64) -> Vec<RunId> {
        let expired = self
            .facts
            .iter()
            .filter_map(|(run_id, fact)| {
                (now_unix_micros > fact.expires_at_unix_micros
                    && !self.announced_expired.contains(run_id))
                .then_some(*run_id)
            })
            .collect::<Vec<_>>();
        self.announced_expired.extend(expired.iter().copied());
        expired
    }

    fn persist(&self) -> Result<(), ProviderStatusError> {
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "provider fact path has no parent",
            )
        })?;
        fs::create_dir_all(parent)?;
        let temporary = temporary_path(&self.path);
        let result = (|| {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            serde_json::to_writer_pretty(
                &mut file,
                &ProviderStatusFile {
                    version: FILE_VERSION,
                    facts: self.facts.clone(),
                },
            )?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
            fs::File::open(parent)?.sync_all()?;
            Ok::<(), ProviderStatusError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

pub(crate) fn derive_activity(
    mission: &Mission,
    run_id: RunId,
    fact: Option<&ProviderFactSummary>,
    now_unix_micros: u64,
) -> Option<RunActivitySummary> {
    let run = mission.runs.get(&run_id)?;
    let lifecycle = |state, explanation: &str| RunActivitySummary {
        mission_id: mission.id,
        run_id,
        state,
        source: RunActivitySource::RunLifecycle,
        explanation: explanation.to_owned(),
        provider_id: None,
        adapter_version: None,
        observed_at_unix_micros: None,
        expires_at_unix_micros: None,
    };
    if run.phase == RunPhase::Pending {
        return Some(lifecycle(
            RunActivityState::Pending,
            "Run lifecycle is pending",
        ));
    }
    if run.phase == RunPhase::Finished {
        let (state, explanation) = match run.outcome {
            Some(FinishOutcome::Succeeded) => {
                (RunActivityState::Completed, "Run lifecycle succeeded")
            }
            Some(FinishOutcome::Failed) => (RunActivityState::Failed, "Run lifecycle failed"),
            Some(FinishOutcome::Cancelled) | None => {
                (RunActivityState::Cancelled, "Run lifecycle was cancelled")
            }
        };
        return Some(lifecycle(state, explanation));
    }

    let mut signals = mission
        .signals
        .values()
        .filter(|signal| signal.run_id == run_id && signal.resolution.is_none())
        .filter_map(|signal| {
            let (rank, state, explanation) = match &signal.kind {
                SignalKind::Escalation { conflict, .. } => (
                    0_u8,
                    RunActivityState::Blocked,
                    format!("Unresolved escalation Signal: {conflict}"),
                ),
                SignalKind::ApprovalNeeded { operation, .. } => (
                    1,
                    RunActivityState::WaitingApproval,
                    format!("Unresolved approval Signal: {operation}"),
                ),
                SignalKind::InputNeeded { question } => (
                    2,
                    RunActivityState::WaitingInput,
                    format!("Unresolved input Signal: {question}"),
                ),
                SignalKind::Blocked { reason } => (
                    3,
                    RunActivityState::Blocked,
                    format!("Unresolved blocked Signal: {reason}"),
                ),
                SignalKind::Progress { .. } | SignalKind::ArtifactReady { .. } => return None,
            };
            Some((rank, signal.id.to_string(), state, explanation))
        })
        .collect::<Vec<_>>();
    signals.sort_by(|left, right| (left.0, &left.1).cmp(&(right.0, &right.1)));
    if let Some((_, _, state, explanation)) = signals.into_iter().next() {
        return Some(RunActivitySummary {
            mission_id: mission.id,
            run_id,
            state,
            source: RunActivitySource::Signal,
            explanation,
            provider_id: None,
            adapter_version: None,
            observed_at_unix_micros: None,
            expires_at_unix_micros: None,
        });
    }
    if run.phase == RunPhase::Paused {
        return Some(lifecycle(
            RunActivityState::Paused,
            "Run lifecycle is paused without unresolved attention",
        ));
    }
    if let Some(fact) = fact
        && fact.mission_id == mission.id
        && now_unix_micros <= fact.expires_at_unix_micros
    {
        return Some(RunActivitySummary {
            mission_id: mission.id,
            run_id,
            state: match fact.state {
                ProviderActivityState::Working => RunActivityState::Working,
                ProviderActivityState::Idle => RunActivityState::Idle,
                ProviderActivityState::WaitingInput => RunActivityState::WaitingInput,
                ProviderActivityState::WaitingApproval => RunActivityState::WaitingApproval,
                ProviderActivityState::Blocked => RunActivityState::Blocked,
            },
            source: match fact.source {
                ProviderFactSource::AuthenticatedAgent => RunActivitySource::AuthenticatedAgent,
                ProviderFactSource::OwnerHook => RunActivitySource::OwnerHook,
            },
            explanation: format!(
                "{} adapter {} reported: {}",
                fact.provider_id, fact.adapter_version, fact.summary
            ),
            provider_id: Some(fact.provider_id.clone()),
            adapter_version: Some(fact.adapter_version.clone()),
            observed_at_unix_micros: Some(fact.observed_at_unix_micros),
            expires_at_unix_micros: Some(fact.expires_at_unix_micros),
        });
    }
    Some(RunActivitySummary {
        mission_id: mission.id,
        run_id,
        state: RunActivityState::Unknown,
        source: RunActivitySource::None,
        explanation: fact.map_or_else(
            || "No authoritative Signal or fresh provider fact exists".to_owned(),
            |fact| {
                format!(
                    "Latest provider fact expired at {}",
                    fact.expires_at_unix_micros
                )
            },
        ),
        provider_id: fact.map(|fact| fact.provider_id.clone()),
        adapter_version: fact.map(|fact| fact.adapter_version.clone()),
        observed_at_unix_micros: fact.map(|fact| fact.observed_at_unix_micros),
        expires_at_unix_micros: fact.map(|fact| fact.expires_at_unix_micros),
    })
}

pub(crate) fn now_unix_micros() -> Result<u64, ProviderStatusError> {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ProviderStatusError::Clock)?
        .as_micros();
    u64::try_from(micros).map_err(|_| ProviderStatusError::Clock)
}

fn validate_input(input: &ProviderFactInput) -> Result<(), ProviderStatusError> {
    validate_text(
        &input.provider_id,
        MAX_PROVIDER_BYTES,
        ProviderStatusError::InvalidProvider,
    )?;
    validate_text(
        &input.adapter_version,
        MAX_ADAPTER_VERSION_BYTES,
        ProviderStatusError::InvalidAdapterVersion,
    )?;
    validate_text(
        &input.summary,
        MAX_SUMMARY_BYTES,
        ProviderStatusError::InvalidSummary,
    )?;
    if !(MIN_VALID_SECONDS..=MAX_VALID_SECONDS).contains(&input.valid_for_seconds) {
        return Err(ProviderStatusError::InvalidValidity);
    }
    Ok(())
}

fn validate_summary(fact: &ProviderFactSummary) -> Result<(), ProviderStatusError> {
    validate_input(&ProviderFactInput {
        provider_id: fact.provider_id.clone(),
        adapter_version: fact.adapter_version.clone(),
        state: fact.state,
        summary: fact.summary.clone(),
        valid_for_seconds: MIN_VALID_SECONDS,
    })?;
    let validity_micros = fact
        .expires_at_unix_micros
        .checked_sub(fact.observed_at_unix_micros)
        .ok_or(ProviderStatusError::InvalidValidity)?;
    let min_validity_micros = u64::from(MIN_VALID_SECONDS) * 1_000_000;
    let max_validity_micros = u64::from(MAX_VALID_SECONDS) * 1_000_000;
    if !(min_validity_micros..=max_validity_micros).contains(&validity_micros) {
        return Err(ProviderStatusError::InvalidValidity);
    }
    Ok(())
}

fn validate_text(
    value: &str,
    max_bytes: usize,
    error: ProviderStatusError,
) -> Result<(), ProviderStatusError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        Err(error)
    } else {
        Ok(())
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    name.push_str(&format!(".{}.tmp", uuid::Uuid::new_v4()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultraplexr_core::{Actor, Command, Risk, RunPriority, SignalId};

    fn running_mission() -> (Mission, RunId) {
        let (mut mission, _) = Mission::create(
            MissionId::new(),
            "Truthful activity",
            Actor::human("owner").expect("valid owner"),
        )
        .expect("valid Mission");
        let run_id = RunId::new();
        for event in mission
            .decide(Command::PlanRun {
                run_id,
                parent: None,
                dependencies: Vec::new(),
                retry_of: None,
                actor: Actor::agent("agent", "codex").expect("valid agent"),
                objective: "Implement activity".to_owned(),
                priority: RunPriority::Normal,
            })
            .expect("Run should plan")
        {
            mission.apply(&event).expect("plan should apply");
        }
        for event in mission
            .decide(Command::StartReadyRun { run_id })
            .expect("Run should start")
        {
            mission.apply(&event).expect("start should apply");
        }
        (mission, run_id)
    }

    fn input(state: ProviderActivityState) -> ProviderFactInput {
        ProviderFactInput {
            provider_id: "openai-codex".to_owned(),
            adapter_version: "1.0".to_owned(),
            state,
            summary: "provider is executing a tool".to_owned(),
            valid_for_seconds: 30,
        }
    }

    #[test]
    fn facts_persist_atomically_and_expire_to_unknown() {
        let root = std::env::temp_dir().join(format!("ultraplexr-provider-{}", RunId::new()));
        let path = root.join("provider-facts.json");
        let (mission, run_id) = running_mission();
        let mut store = ProviderStatusStore::open(path.clone()).expect("store should open");
        let fact = store
            .record(
                mission.id,
                run_id,
                input(ProviderActivityState::Working),
                ProviderFactSource::AuthenticatedAgent,
            )
            .expect("fact should persist");
        assert_eq!(
            derive_activity(&mission, run_id, Some(&fact), fact.observed_at_unix_micros)
                .expect("activity should derive")
                .state,
            RunActivityState::Working
        );
        assert_eq!(
            derive_activity(
                &mission,
                run_id,
                Some(&fact),
                fact.expires_at_unix_micros + 1
            )
            .expect("activity should derive")
            .state,
            RunActivityState::Unknown
        );
        assert_eq!(
            store.take_newly_expired(fact.expires_at_unix_micros + 1),
            [run_id]
        );
        assert!(
            store
                .take_newly_expired(fact.expires_at_unix_micros + 2)
                .is_empty()
        );
        let reopened = ProviderStatusStore::open(path.clone()).expect("store should reopen");
        assert_eq!(reopened.get(run_id), Some(&fact));
        assert_eq!(
            fs::metadata(path)
                .expect("fact metadata should exist")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).expect("test state should be removable");
    }

    #[test]
    fn unresolved_signal_precedes_fresh_provider_fact() {
        let (mut mission, run_id) = running_mission();
        let signal_id = SignalId::new();
        for event in mission
            .decide(Command::RaiseSignal {
                signal_id,
                run_id,
                kind: SignalKind::ApprovalNeeded {
                    operation: "publish release".to_owned(),
                    risk: Risk::High,
                },
            })
            .expect("Signal should raise")
        {
            mission.apply(&event).expect("Signal should apply");
        }
        let now = 10_000_000;
        let fact = ProviderFactSummary {
            mission_id: mission.id,
            run_id,
            provider_id: "codex".to_owned(),
            adapter_version: "1".to_owned(),
            state: ProviderActivityState::Working,
            summary: "still working".to_owned(),
            source: ProviderFactSource::AuthenticatedAgent,
            observed_at_unix_micros: now,
            expires_at_unix_micros: now + 1_000_000,
        };
        let activity = derive_activity(&mission, run_id, Some(&fact), now)
            .expect("activity should derive");
        assert_eq!(activity.state, RunActivityState::WaitingApproval);
        assert_eq!(activity.source, RunActivitySource::Signal);
    }

    #[test]
    fn persisted_facts_cannot_claim_unbounded_validity() {
        let (mission, run_id) = running_mission();
        let fact = ProviderFactSummary {
            mission_id: mission.id,
            run_id,
            provider_id: "codex".to_owned(),
            adapter_version: "1".to_owned(),
            state: ProviderActivityState::Working,
            summary: "working".to_owned(),
            source: ProviderFactSource::AuthenticatedAgent,
            observed_at_unix_micros: 1_000_000,
            expires_at_unix_micros: 1_000_000
                + (u64::from(MAX_VALID_SECONDS) + 1) * 1_000_000,
        };

        assert!(matches!(
            validate_summary(&fact),
            Err(ProviderStatusError::InvalidValidity)
        ));
    }
}
