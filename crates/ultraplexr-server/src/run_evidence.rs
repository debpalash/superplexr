use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use ultraplexr_core::{MissionId, RunId};
use ultraplexr_protocol::{
    RunEvidenceInput, RunEvidenceKind, RunEvidenceSource, RunEvidenceSummary,
};
use thiserror::Error;

const FILE_VERSION: u16 = 1;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FACTS: usize = 4_096;
const MAX_PROVIDER_BYTES: usize = 128;
const MAX_ADAPTER_VERSION_BYTES: usize = 64;
const MAX_KEY_BYTES: usize = 256;
const MAX_REVISION_BYTES: usize = 256;
const MAX_SUMMARY_BYTES: usize = 2_048;
const MAX_NAME_BYTES: usize = 512;
const MAX_URL_BYTES: usize = 2_048;

#[derive(Debug, Error)]
pub(crate) enum RunEvidenceError {
    #[error("Run evidence store is not a regular owner-controlled file: {0}")]
    Insecure(PathBuf),
    #[error("Run evidence store exceeds 8 MiB")]
    TooLarge,
    #[error("Run evidence store could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("Run evidence store is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported Run evidence store version {0}")]
    Version(u16),
    #[error("Run evidence store exceeds {MAX_FACTS} records")]
    TooManyFacts,
    #[error("Run evidence contains an invalid provider ID")]
    InvalidProvider,
    #[error("Run evidence contains an invalid adapter version")]
    InvalidAdapterVersion,
    #[error("Run evidence contains an invalid evidence key")]
    InvalidKey,
    #[error("Run evidence contains an invalid revision")]
    InvalidRevision,
    #[error("Run evidence contains an invalid summary")]
    InvalidSummary,
    #[error("Run evidence contains an invalid name or title")]
    InvalidName,
    #[error("Run evidence contains an invalid URL")]
    InvalidUrl,
    #[error("Run evidence store key does not match its record")]
    MismatchedKey,
    #[error("system clock is before the Unix epoch")]
    Clock,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunEvidenceFile {
    version: u16,
    facts: BTreeMap<String, RunEvidenceSummary>,
}

pub(crate) struct RunEvidenceStore {
    path: PathBuf,
    facts: BTreeMap<String, RunEvidenceSummary>,
}

impl RunEvidenceStore {
    pub(crate) fn open(path: PathBuf) -> Result<Self, RunEvidenceError> {
        let facts = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                // SAFETY: geteuid has no preconditions and reads process metadata.
                let current_uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.uid() != current_uid
                {
                    return Err(RunEvidenceError::Insecure(path));
                }
                if metadata.len() > MAX_FILE_BYTES {
                    return Err(RunEvidenceError::TooLarge);
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                let file: RunEvidenceFile = serde_json::from_slice(&fs::read(&path)?)?;
                if file.version != FILE_VERSION {
                    return Err(RunEvidenceError::Version(file.version));
                }
                if file.facts.len() > MAX_FACTS {
                    return Err(RunEvidenceError::TooManyFacts);
                }
                for (key, fact) in &file.facts {
                    validate_summary(fact)?;
                    if key != &store_key(fact.run_id, &fact.provider_id, &fact.evidence_key) {
                        return Err(RunEvidenceError::MismatchedKey);
                    }
                }
                file.facts
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self { path, facts })
    }

    #[cfg(test)]
    pub(crate) fn transient() -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "ultraplexr-evidence-transient-{}.json",
                uuid::Uuid::new_v4()
            )),
            facts: BTreeMap::new(),
        }
    }

    pub(crate) fn record(
        &mut self,
        mission_id: MissionId,
        run_id: RunId,
        input: RunEvidenceInput,
        source: RunEvidenceSource,
    ) -> Result<RunEvidenceSummary, RunEvidenceError> {
        validate_input(&input)?;
        let key = store_key(run_id, &input.provider_id, &input.evidence_key);
        if !self.facts.contains_key(&key) && self.facts.len() == MAX_FACTS {
            return Err(RunEvidenceError::TooManyFacts);
        }
        let fact = RunEvidenceSummary {
            mission_id,
            run_id,
            provider_id: input.provider_id,
            adapter_version: input.adapter_version,
            evidence_key: input.evidence_key,
            revision: input.revision,
            summary: input.summary,
            url: input.url,
            kind: input.kind,
            source,
            observed_at_unix_micros: now_unix_micros()?,
        };
        let previous = self.facts.insert(key.clone(), fact.clone());
        if let Err(error) = self.persist() {
            match previous {
                Some(previous) => {
                    self.facts.insert(key, previous);
                }
                None => {
                    self.facts.remove(&key);
                }
            }
            return Err(error);
        }
        Ok(fact)
    }

    pub(crate) fn list(&self, mission_id: MissionId, run_id: RunId) -> Vec<RunEvidenceSummary> {
        let mut facts = self
            .facts
            .values()
            .filter(|fact| fact.mission_id == mission_id && fact.run_id == run_id)
            .cloned()
            .collect::<Vec<_>>();
        facts.sort_by(|left, right| {
            right
                .observed_at_unix_micros
                .cmp(&left.observed_at_unix_micros)
                .then_with(|| left.provider_id.cmp(&right.provider_id))
                .then_with(|| left.evidence_key.cmp(&right.evidence_key))
        });
        facts
    }

    fn persist(&self) -> Result<(), RunEvidenceError> {
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Run evidence path has no parent",
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
                &RunEvidenceFile {
                    version: FILE_VERSION,
                    facts: self.facts.clone(),
                },
            )?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
            fs::File::open(parent)?.sync_all()?;
            Ok::<(), RunEvidenceError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn validate_summary(fact: &RunEvidenceSummary) -> Result<(), RunEvidenceError> {
    validate_input(&RunEvidenceInput {
        provider_id: fact.provider_id.clone(),
        adapter_version: fact.adapter_version.clone(),
        evidence_key: fact.evidence_key.clone(),
        revision: fact.revision.clone(),
        summary: fact.summary.clone(),
        url: fact.url.clone(),
        kind: fact.kind.clone(),
    })
}

fn validate_input(input: &RunEvidenceInput) -> Result<(), RunEvidenceError> {
    validate_text(
        &input.provider_id,
        MAX_PROVIDER_BYTES,
        RunEvidenceError::InvalidProvider,
    )?;
    validate_text(
        &input.adapter_version,
        MAX_ADAPTER_VERSION_BYTES,
        RunEvidenceError::InvalidAdapterVersion,
    )?;
    validate_text(
        &input.evidence_key,
        MAX_KEY_BYTES,
        RunEvidenceError::InvalidKey,
    )?;
    validate_text(
        &input.revision,
        MAX_REVISION_BYTES,
        RunEvidenceError::InvalidRevision,
    )?;
    validate_text(
        &input.summary,
        MAX_SUMMARY_BYTES,
        RunEvidenceError::InvalidSummary,
    )?;
    if let Some(url) = &input.url {
        validate_text(url, MAX_URL_BYTES, RunEvidenceError::InvalidUrl)?;
    }
    let name = match &input.kind {
        RunEvidenceKind::Check { name, .. } => name,
        RunEvidenceKind::ChangeRequest { title, .. } => title,
    };
    validate_text(name, MAX_NAME_BYTES, RunEvidenceError::InvalidName)
}

fn validate_text(
    value: &str,
    max_bytes: usize,
    error: RunEvidenceError,
) -> Result<(), RunEvidenceError> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        Err(error)
    } else {
        Ok(())
    }
}

fn store_key(run_id: RunId, provider_id: &str, evidence_key: &str) -> String {
    format!("{run_id}\0{provider_id}\0{evidence_key}")
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

fn now_unix_micros() -> Result<u64, RunEvidenceError> {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RunEvidenceError::Clock)?
        .as_micros();
    u64::try_from(micros).map_err(|_| RunEvidenceError::Clock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use ultraplexr_protocol::{CheckEvidenceState, RunEvidenceKind};

    fn input(state: CheckEvidenceState, revision: &str) -> RunEvidenceInput {
        RunEvidenceInput {
            provider_id: "github".to_owned(),
            adapter_version: "1".to_owned(),
            evidence_key: "check/test".to_owned(),
            revision: revision.to_owned(),
            summary: "test suite completed".to_owned(),
            url: Some("https://example.invalid/check/1".to_owned()),
            kind: RunEvidenceKind::Check {
                name: "test".to_owned(),
                state,
            },
        }
    }

    #[test]
    fn evidence_replaces_by_provider_key_and_survives_reopen() {
        let root = std::env::temp_dir().join(format!("ultraplexr-evidence-{}", RunId::new()));
        let path = root.join("run-evidence.json");
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let mut store = RunEvidenceStore::open(path.clone()).expect("store should open");
        store
            .record(
                mission_id,
                run_id,
                input(CheckEvidenceState::Running, "abc"),
                RunEvidenceSource::OwnerHook,
            )
            .expect("running evidence should persist");
        let passed = store
            .record(
                mission_id,
                run_id,
                input(CheckEvidenceState::Passed, "def"),
                RunEvidenceSource::OwnerHook,
            )
            .expect("passed evidence should replace");
        assert_eq!(store.list(mission_id, run_id), std::slice::from_ref(&passed));

        let reopened = RunEvidenceStore::open(path.clone()).expect("store should reopen");
        assert_eq!(reopened.list(mission_id, run_id), [passed]);
        assert_eq!(
            fs::metadata(path)
                .expect("evidence metadata should exist")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).expect("test state should be removable");
    }

    #[test]
    fn evidence_rejects_empty_or_unbounded_fields() {
        let mut invalid = input(CheckEvidenceState::Failed, "abc");
        invalid.evidence_key = " ".to_owned();
        assert!(matches!(
            validate_input(&invalid),
            Err(RunEvidenceError::InvalidKey)
        ));

        let mut invalid = input(CheckEvidenceState::Failed, "abc");
        invalid.url = Some("x".repeat(MAX_URL_BYTES + 1));
        assert!(matches!(
            validate_input(&invalid),
            Err(RunEvidenceError::InvalidUrl)
        ));
    }
}
