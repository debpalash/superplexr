use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use superplexr_core::{MissionId, SessionId};
use superplexr_protocol::SessionGroupId;
use thiserror::Error;

use crate::{
    pane_layout::PaneLayout, provider_usage::ProviderUsageSettings, theme::ThemeSelection,
};

const DOCUMENT_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WorkspaceDocument {
    pub(crate) version: u16,
    pub(crate) active_workspace: u64,
    pub(crate) next_sequence: u64,
    #[serde(default)]
    pub(crate) theme: ThemeSelection,
    #[serde(default)]
    pub(crate) provider_usage: ProviderUsageSettings,
    /// Owner-dragged sidebar width in pixels; `None` keeps the responsive default.
    #[serde(default)]
    pub(crate) sidebar_width: Option<u16>,
    pub(crate) workspaces: Vec<WorkspaceRecord>,
}

impl WorkspaceDocument {
    pub(crate) fn new(
        active_workspace: u64,
        next_sequence: u64,
        theme: ThemeSelection,
        provider_usage: ProviderUsageSettings,
        sidebar_width: Option<u16>,
        workspaces: Vec<WorkspaceRecord>,
    ) -> Self {
        Self {
            version: DOCUMENT_VERSION,
            active_workspace,
            next_sequence,
            theme,
            provider_usage,
            sidebar_width,
            workspaces,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WorkspaceRecord {
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) mission_id: Option<MissionId>,
    pub(crate) title: String,
    pub(crate) pinned: bool,
    pub(crate) selected_session: usize,
    pub(crate) focus_mode: bool,
    pub(crate) sessions: Vec<SessionRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SessionRecord {
    #[serde(default)]
    pub(crate) group_id: Option<SessionGroupId>,
    #[serde(default)]
    pub(crate) group_version: u64,
    #[serde(default)]
    pub(crate) pinned: bool,
    pub(crate) name: String,
    pub(crate) actor: String,
    pub(crate) terminals: Vec<SessionId>,
    #[serde(default)]
    pub(crate) pane_layout: Option<PaneLayout>,
}

#[derive(Debug, Error)]
pub(crate) enum WorkspaceStoreError {
    #[error("workspace state I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("workspace state is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("workspace state version {0} is unsupported")]
    UnsupportedVersion(u16),
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceStore {
    path: PathBuf,
}

impl WorkspaceStore {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(crate) fn load(&self) -> Result<Option<WorkspaceDocument>, WorkspaceStoreError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(WorkspaceStoreError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "workspace state is not a real file",
            )));
        }
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        let bytes = fs::read(&self.path)?;
        let document: WorkspaceDocument = serde_json::from_slice(&bytes)?;
        if document.version != DOCUMENT_VERSION {
            return Err(WorkspaceStoreError::UnsupportedVersion(document.version));
        }
        Ok(Some(document))
    }

    pub(crate) fn save(&self, document: &WorkspaceDocument) -> Result<(), WorkspaceStoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
            let metadata = fs::symlink_metadata(parent)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(WorkspaceStoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "workspace state parent is not a real directory",
                )));
            }
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let temporary = temporary_path(&self.path);
        let bytes = serde_json::to_vec_pretty(document)?;
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_documents_round_trip_atomically() {
        let root = std::env::temp_dir().join(format!("superplexr-workspace-{}", SessionId::new()));
        let store = WorkspaceStore::new(root.join("workspaces.json"));
        let document = WorkspaceDocument::new(
            7,
            9,
            ThemeSelection::BuiltIn("paper".to_owned()),
            ProviderUsageSettings {
                codex: false,
                refresh_interval_secs: 900,
                ..ProviderUsageSettings::default()
            },
            Some(260),
            vec![WorkspaceRecord {
                id: 7,
                mission_id: Some(MissionId::new()),
                title: "Agent work".to_owned(),
                pinned: true,
                selected_session: 0,
                focus_mode: false,
                sessions: vec![SessionRecord {
                    group_id: Some(SessionGroupId::new()),
                    group_version: 3,
                    pinned: true,
                    name: "codex".to_owned(),
                    actor: "agent".to_owned(),
                    terminals: vec![SessionId::new()],
                    pane_layout: Some(PaneLayout::equal(&[1])),
                }],
            }],
        );

        store.save(&document).expect("workspace should persist");
        assert_eq!(
            fs::metadata(root.join("workspaces.json"))
                .expect("workspace metadata should exist")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(store.load().expect("workspace should load"), Some(document));
        fs::remove_dir_all(root).expect("test state should be removable");
    }

    #[test]
    fn legacy_workspace_document_defaults_to_graphite() {
        let document: WorkspaceDocument = serde_json::from_str(
            r#"{
                "version": 1,
                "active_workspace": 1,
                "next_sequence": 2,
                "workspaces": []
            }"#,
        )
        .expect("legacy presentation state should remain readable");
        assert_eq!(document.theme, ThemeSelection::default());
        assert_eq!(document.provider_usage, ProviderUsageSettings::default());
        assert_eq!(document.sidebar_width, None);
    }
}
