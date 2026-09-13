use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use superplexr_core::{MissionId, SessionId};
use superplexr_protocol::{
    SessionGroupChange, SessionGroupId, SessionGroupSummary,
};
use thiserror::Error;

const FILE_VERSION: u16 = 1;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_GROUPS: usize = 4096;
const MAX_SESSIONS_PER_GROUP: usize = 256;

#[derive(Debug, Error)]
pub(crate) enum SessionGroupError {
    #[error("Session group store is not a regular owner-controlled file: {0}")]
    Insecure(PathBuf),
    #[error("Session group store exceeds 4 MiB")]
    TooLarge,
    #[error("Session group store could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("Session group store is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported Session group store version {0}")]
    FileVersion(u16),
    #[error("Session group limit of {MAX_GROUPS} is reached")]
    TooManyGroups,
    #[error("Session group {0} already exists")]
    AlreadyExists(SessionGroupId),
    #[error("Session group {0} does not exist")]
    NotFound(SessionGroupId),
    #[error("Session group name must contain 1 to 128 bytes and no NUL")]
    InvalidName,
    #[error("Session group may contain at most {MAX_SESSIONS_PER_GROUP} Sessions")]
    TooManySessions,
    #[error("Session group cannot contain a Session more than once")]
    DuplicateSession,
    #[error("Session group {group_id} is version {actual}; request expected {expected}")]
    Conflict {
        group_id: SessionGroupId,
        expected: u64,
        actual: u64,
    },
    #[error("Session {0} is already a member of this group")]
    SessionAlreadyPresent(SessionId),
    #[error("Session {session_id} is already a member of Session group {group_id}")]
    SessionInAnotherGroup {
        session_id: SessionId,
        group_id: SessionGroupId,
    },
    #[error("Session {0} is not a member of this group")]
    SessionNotPresent(SessionId),
    #[error("Session group version is exhausted")]
    VersionExhausted,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionGroupFile {
    version: u16,
    groups: BTreeMap<SessionGroupId, SessionGroupSummary>,
}

pub(crate) struct SessionGroupStore {
    path: PathBuf,
    groups: BTreeMap<SessionGroupId, SessionGroupSummary>,
}

impl SessionGroupStore {
    pub(crate) fn open(path: PathBuf) -> Result<Self, SessionGroupError> {
        let groups = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                // SAFETY: geteuid has no preconditions and only reads process metadata.
                let current_uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.uid() != current_uid
                {
                    return Err(SessionGroupError::Insecure(path));
                }
                if metadata.len() > MAX_FILE_BYTES {
                    return Err(SessionGroupError::TooLarge);
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                let file: SessionGroupFile = serde_json::from_slice(&fs::read(&path)?)?;
                if file.version != FILE_VERSION {
                    return Err(SessionGroupError::FileVersion(file.version));
                }
                if file.groups.len() > MAX_GROUPS {
                    return Err(SessionGroupError::TooManyGroups);
                }
                for group in file.groups.values() {
                    validate(group)?;
                }
                file.groups
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self { path, groups })
    }

    pub(crate) fn list(&self, mission_id: Option<MissionId>) -> Vec<SessionGroupSummary> {
        let mut groups = self
            .groups
            .values()
            .filter(|group| mission_id.is_none_or(|id| group.mission_id == Some(id)))
            .cloned()
            .collect::<Vec<_>>();
        groups.sort_by_key(|group| (group.position, group.group_id));
        groups
    }

    pub(crate) fn create(
        &mut self,
        mut group: SessionGroupSummary,
    ) -> Result<SessionGroupSummary, SessionGroupError> {
        if self.groups.contains_key(&group.group_id) {
            return Err(SessionGroupError::AlreadyExists(group.group_id));
        }
        if self.groups.len() == MAX_GROUPS {
            return Err(SessionGroupError::TooManyGroups);
        }
        group.version = 1;
        validate(&group)?;
        self.ensure_members_available(group.group_id, &group.session_ids)?;
        self.groups.insert(group.group_id, group.clone());
        if let Err(error) = self.persist() {
            self.groups.remove(&group.group_id);
            return Err(error);
        }
        Ok(group)
    }

    pub(crate) fn update(
        &mut self,
        group_id: SessionGroupId,
        expected_version: u64,
        change: SessionGroupChange,
    ) -> Result<SessionGroupSummary, SessionGroupError> {
        let previous = self
            .groups
            .get(&group_id)
            .cloned()
            .ok_or(SessionGroupError::NotFound(group_id))?;
        if previous.version != expected_version {
            return Err(SessionGroupError::Conflict {
                group_id,
                expected: expected_version,
                actual: previous.version,
            });
        }
        let mut updated = previous.clone();
        match change {
            SessionGroupChange::Rename { name } => updated.name = name,
            SessionGroupChange::SetPosition { position } => updated.position = position,
            SessionGroupChange::SetPinned { pinned } => updated.pinned = pinned,
            SessionGroupChange::SetDetached { detached } => updated.detached = detached,
            SessionGroupChange::AddSession { session_id } => {
                if updated.session_ids.contains(&session_id) {
                    return Err(SessionGroupError::SessionAlreadyPresent(session_id));
                }
                self.ensure_members_available(group_id, &[session_id])?;
                updated.session_ids.push(session_id);
            }
            SessionGroupChange::RemoveSession { session_id } => {
                let Some(index) = updated.session_ids.iter().position(|id| *id == session_id)
                else {
                    return Err(SessionGroupError::SessionNotPresent(session_id));
                };
                updated.session_ids.remove(index);
            }
        }
        updated.version = updated
            .version
            .checked_add(1)
            .ok_or(SessionGroupError::VersionExhausted)?;
        validate(&updated)?;
        self.groups.insert(group_id, updated.clone());
        if let Err(error) = self.persist() {
            self.groups.insert(group_id, previous);
            return Err(error);
        }
        Ok(updated)
    }

    pub(crate) fn delete(
        &mut self,
        group_id: SessionGroupId,
        expected_version: u64,
    ) -> Result<(), SessionGroupError> {
        let existing = self
            .groups
            .get(&group_id)
            .ok_or(SessionGroupError::NotFound(group_id))?;
        if existing.version != expected_version {
            return Err(SessionGroupError::Conflict {
                group_id,
                expected: expected_version,
                actual: existing.version,
            });
        }
        let previous = self.groups.remove(&group_id).expect("group was checked");
        if let Err(error) = self.persist() {
            self.groups.insert(group_id, previous);
            return Err(error);
        }
        Ok(())
    }

    fn persist(&self) -> Result<(), SessionGroupError> {
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "group path has no parent")
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
                &SessionGroupFile {
                    version: FILE_VERSION,
                    groups: self.groups.clone(),
                },
            )?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
            fs::File::open(parent)?.sync_all()?;
            Ok::<(), SessionGroupError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn ensure_members_available(
        &self,
        target_group_id: SessionGroupId,
        session_ids: &[SessionId],
    ) -> Result<(), SessionGroupError> {
        for (group_id, group) in &self.groups {
            if *group_id == target_group_id {
                continue;
            }
            if let Some(session_id) = session_ids
                .iter()
                .find(|session_id| group.session_ids.contains(session_id))
            {
                return Err(SessionGroupError::SessionInAnotherGroup {
                    session_id: *session_id,
                    group_id: *group_id,
                });
            }
        }
        Ok(())
    }
}

fn validate(group: &SessionGroupSummary) -> Result<(), SessionGroupError> {
    if group.name.trim().is_empty() || group.name.len() > 128 || group.name.contains('\0') {
        return Err(SessionGroupError::InvalidName);
    }
    if group.session_ids.len() > MAX_SESSIONS_PER_GROUP {
        return Err(SessionGroupError::TooManySessions);
    }
    let unique = group.session_ids.iter().copied().collect::<HashSet<_>>();
    if unique.len() != group.session_ids.len() {
        return Err(SessionGroupError::DuplicateSession);
    }
    if group.version == 0 {
        return Err(SessionGroupError::VersionExhausted);
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", uuid::Uuid::new_v4()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group() -> SessionGroupSummary {
        SessionGroupSummary {
            group_id: SessionGroupId::new(),
            mission_id: Some(MissionId::new()),
            name: "shells".to_owned(),
            session_ids: vec![SessionId::new()],
            position: 4,
            pinned: false,
            detached: false,
            version: 0,
        }
    }

    #[test]
    fn changes_are_versioned_and_persisted() {
        let root = std::env::temp_dir().join(format!("superplexr-groups-{}", SessionId::new()));
        let path = root.join("session-groups.json");
        let mut store = SessionGroupStore::open(path.clone()).expect("store opens");
        let created = store.create(group()).expect("group is created");
        assert_eq!(created.version, 1);
        let renamed = store
            .update(
                created.group_id,
                created.version,
                SessionGroupChange::Rename {
                    name: "agents".to_owned(),
                },
            )
            .expect("group is renamed");
        assert_eq!(renamed.version, 2);
        assert_eq!(renamed.name, "agents");
        let detached = store
            .update(
                renamed.group_id,
                renamed.version,
                SessionGroupChange::SetDetached { detached: true },
            )
            .expect("group detaches without touching member Sessions");
        assert!(detached.detached);

        let reopened = SessionGroupStore::open(path).expect("store reopens");
        assert_eq!(reopened.list(created.mission_id), [detached]);
        fs::remove_dir_all(root).expect("isolated state is removable");
    }

    #[test]
    fn stale_changes_do_not_replace_state() {
        let root = std::env::temp_dir().join(format!("superplexr-groups-{}", SessionId::new()));
        let mut store = SessionGroupStore::open(root.join("groups.json")).expect("store opens");
        let created = store.create(group()).expect("group is created");
        let error = store
            .update(
                created.group_id,
                0,
                SessionGroupChange::SetPinned { pinned: true },
            )
            .expect_err("stale update is rejected");
        assert!(matches!(error, SessionGroupError::Conflict { .. }));
        assert!(!store.list(None)[0].pinned);
        fs::remove_dir_all(root).expect("isolated state is removable");
    }

    #[test]
    fn one_session_cannot_appear_in_two_groups() {
        let root = std::env::temp_dir().join(format!("superplexr-groups-{}", SessionId::new()));
        let mut store = SessionGroupStore::open(root.join("groups.json")).expect("store opens");
        let first = store.create(group()).expect("first group is created");
        let error = store
            .create(SessionGroupSummary {
                group_id: SessionGroupId::new(),
                mission_id: first.mission_id,
                name: "duplicate".to_owned(),
                session_ids: first.session_ids.clone(),
                position: 5,
                pinned: false,
                detached: false,
                version: 0,
            })
            .expect_err("duplicate membership is rejected");
        assert!(matches!(
            error,
            SessionGroupError::SessionInAnotherGroup { .. }
        ));
        fs::remove_dir_all(root).expect("isolated state is removable");
    }
}
