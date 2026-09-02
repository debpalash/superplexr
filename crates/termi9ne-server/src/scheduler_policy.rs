use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use termi9ne_core::MissionId;
use termi9ne_protocol::{SchedulerPolicy, SchedulerSettings};
use thiserror::Error;

const FILE_VERSION: u16 = 1;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_POLICIES: usize = 256;
pub(crate) const DEFAULT_GLOBAL_AGENT_CONCURRENCY: u16 = 12;

#[derive(Debug, Error)]
pub(crate) enum PolicyError {
    #[error("scheduler policy store is not a regular owner-controlled file: {0}")]
    Insecure(PathBuf),
    #[error("scheduler policy store exceeds 1 MiB")]
    TooLarge,
    #[error("scheduler policy store could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("scheduler policy store is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported scheduler policy store version {0}")]
    Version(u16),
    #[error("scheduler policy store exceeds {MAX_POLICIES} Missions")]
    TooManyPolicies,
    #[error("scheduler max concurrency must be between 1 and 256")]
    InvalidConcurrency,
    #[error("global scheduler max concurrency must be between 1 and 256")]
    InvalidGlobalConcurrency,
    #[error("scheduler Session prefix must contain 1 to 128 bytes and no NUL")]
    InvalidPrefix,
    #[error("scheduler working directory must be an absolute path without NUL")]
    InvalidWorkingDirectory,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    version: u16,
    #[serde(default = "default_settings")]
    settings: SchedulerSettings,
    policies: BTreeMap<MissionId, SchedulerPolicy>,
}

pub(crate) struct SchedulerPolicyStore {
    path: PathBuf,
    settings: SchedulerSettings,
    policies: BTreeMap<MissionId, SchedulerPolicy>,
}

impl SchedulerPolicyStore {
    pub(crate) fn open(path: PathBuf) -> Result<Self, PolicyError> {
        let (settings, policies) = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                // SAFETY: geteuid has no preconditions and reads process metadata.
                let current_uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.uid() != current_uid
                {
                    return Err(PolicyError::Insecure(path));
                }
                if metadata.len() > MAX_FILE_BYTES {
                    return Err(PolicyError::TooLarge);
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                let file: PolicyFile = serde_json::from_slice(&fs::read(&path)?)?;
                if file.version != FILE_VERSION {
                    return Err(PolicyError::Version(file.version));
                }
                if file.policies.len() > MAX_POLICIES {
                    return Err(PolicyError::TooManyPolicies);
                }
                for policy in file.policies.values() {
                    validate(policy)?;
                }
                validate_settings(file.settings)?;
                (file.settings, file.policies)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (default_settings(), BTreeMap::new())
            }
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            settings,
            policies,
        })
    }

    pub(crate) fn list(&self) -> Vec<SchedulerPolicy> {
        self.policies.values().cloned().collect()
    }

    pub(crate) const fn settings(&self) -> SchedulerSettings {
        self.settings
    }

    pub(crate) fn set_settings(&mut self, settings: SchedulerSettings) -> Result<(), PolicyError> {
        validate_settings(settings)?;
        let previous = self.settings;
        self.settings = settings;
        if let Err(error) = self.persist() {
            self.settings = previous;
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn set(&mut self, policy: SchedulerPolicy) -> Result<(), PolicyError> {
        validate(&policy)?;
        if !self.policies.contains_key(&policy.mission_id) && self.policies.len() == MAX_POLICIES {
            return Err(PolicyError::TooManyPolicies);
        }
        let mission_id = policy.mission_id;
        let previous = self.policies.insert(mission_id, policy);
        if let Err(error) = self.persist() {
            match previous {
                Some(previous) => {
                    self.policies.insert(mission_id, previous);
                }
                None => {
                    self.policies.remove(&mission_id);
                }
            }
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), PolicyError> {
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "scheduler policy path has no parent",
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
                &PolicyFile {
                    version: FILE_VERSION,
                    settings: self.settings,
                    policies: self.policies.clone(),
                },
            )?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
            fs::File::open(parent)?.sync_all()?;
            Ok::<(), PolicyError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn validate(policy: &SchedulerPolicy) -> Result<(), PolicyError> {
    if !(1..=256).contains(&policy.max_concurrency) {
        return Err(PolicyError::InvalidConcurrency);
    }
    if policy.session_name_prefix.is_empty()
        || policy.session_name_prefix.len() > 128
        || policy.session_name_prefix.contains('\0')
    {
        return Err(PolicyError::InvalidPrefix);
    }
    if !policy.cwd.is_absolute() || policy.cwd.as_os_str().to_string_lossy().contains('\0') {
        return Err(PolicyError::InvalidWorkingDirectory);
    }
    Ok(())
}

const fn default_settings() -> SchedulerSettings {
    SchedulerSettings {
        global_max_concurrency: DEFAULT_GLOBAL_AGENT_CONCURRENCY,
    }
}

fn validate_settings(settings: SchedulerSettings) -> Result<(), PolicyError> {
    if !(1..=256).contains(&settings.global_max_concurrency) {
        return Err(PolicyError::InvalidGlobalConcurrency);
    }
    Ok(())
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
    use termi9ne_terminal::GridSize;

    fn policy() -> SchedulerPolicy {
        SchedulerPolicy {
            mission_id: MissionId::new(),
            enabled: true,
            max_concurrency: 3,
            session_name_prefix: "autopilot".to_owned(),
            cwd: std::env::current_dir().expect("current directory should resolve"),
            grid: GridSize::new(100, 30).expect("valid grid"),
        }
    }

    #[test]
    fn policies_persist_atomically_with_owner_only_permissions() {
        let root = std::env::temp_dir().join(format!("termi9ne-policy-{}", MissionId::new()));
        let path = root.join("scheduler-policies.json");
        let expected = policy();
        let mut store = SchedulerPolicyStore::open(path.clone()).expect("store should open");
        store.set(expected.clone()).expect("policy should persist");

        let reopened = SchedulerPolicyStore::open(path.clone()).expect("store should reopen");
        assert_eq!(reopened.list(), [expected]);
        assert_eq!(
            reopened.settings().global_max_concurrency,
            DEFAULT_GLOBAL_AGENT_CONCURRENCY
        );
        assert_eq!(
            fs::metadata(&path)
                .expect("policy metadata should exist")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        fs::remove_dir_all(root).expect("isolated policy state should be removable");
    }

    #[test]
    fn invalid_policy_never_replaces_the_durable_state() {
        let root = std::env::temp_dir().join(format!("termi9ne-policy-{}", MissionId::new()));
        let path = root.join("scheduler-policies.json");
        let expected = policy();
        let mut store = SchedulerPolicyStore::open(path.clone()).expect("store should open");
        store.set(expected.clone()).expect("policy should persist");
        store
            .set_settings(SchedulerSettings {
                global_max_concurrency: 7,
            })
            .expect("settings should persist");
        let mut invalid = expected.clone();
        invalid.max_concurrency = 0;
        assert!(matches!(
            store.set(invalid),
            Err(PolicyError::InvalidConcurrency)
        ));
        assert_eq!(
            SchedulerPolicyStore::open(path.clone())
                .expect("store should reopen")
                .list(),
            [expected]
        );
        assert!(matches!(
            store.set_settings(SchedulerSettings {
                global_max_concurrency: 0,
            }),
            Err(PolicyError::InvalidGlobalConcurrency)
        ));
        assert_eq!(store.settings().global_max_concurrency, 7);
        assert_eq!(
            SchedulerPolicyStore::open(path)
                .expect("settings should reopen")
                .settings()
                .global_max_concurrency,
            7
        );
        fs::remove_dir_all(root).expect("isolated policy state should be removable");
    }
}
