//! Durable, server-authored review material for one frozen Candidate.
//!
//! This module owns the file format and atomic persistence. Callers provide a
//! frozen manifest and receive only the Artifact facts needed by the graph.

use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use termi9ne_core::{MissionId, RealizedChange, RealizedChangeManifest, RunId};
use thiserror::Error;
use uuid::Uuid;

const SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Error)]
pub(crate) enum ReviewArtifactError {
    #[error("Candidate review Artifact I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Candidate review Artifact encoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Candidate review Artifact identity is incomplete")]
    MissingSnapshot,
    #[error("existing Candidate review Artifact does not match its content identity")]
    ContentConflict,
}

#[derive(Clone, Debug)]
pub(crate) struct MaterializedReviewArtifact {
    pub locator: String,
    pub sha256: String,
}

#[derive(Serialize)]
struct CandidateReviewDocument<'a> {
    schema_version: u16,
    mission_id: MissionId,
    run_id: RunId,
    base_revision: &'a str,
    snapshot_revision: &'a str,
    snapshot_tree: &'a str,
    patch_sha256: &'a str,
    changed_files: &'a [RealizedChange],
    affected_roots: Vec<&'a str>,
    required_checks: [ReviewCheck; 6],
}

#[derive(Clone, Copy, Serialize)]
struct ReviewCheck {
    id: &'static str,
    state: &'static str,
    purpose: &'static str,
}

const REQUIRED_CHECKS: [ReviewCheck; 6] = [
    ReviewCheck {
        id: "format",
        state: "not_run",
        purpose: "Source formatting is clean",
    },
    ReviewCheck {
        id: "lint",
        state: "not_run",
        purpose: "Static analysis is clean",
    },
    ReviewCheck {
        id: "tests",
        state: "not_run",
        purpose: "Relevant automated tests pass",
    },
    ReviewCheck {
        id: "delivery",
        state: "not_run",
        purpose: "The built result works in its delivery environment",
    },
    ReviewCheck {
        id: "provenance",
        state: "not_run",
        purpose: "Inputs and generated outputs have known provenance",
    },
    ReviewCheck {
        id: "repeatability",
        state: "not_run",
        purpose: "An independent run can reproduce the result",
    },
];

pub(crate) fn materialize(
    state_dir: &Path,
    mission_id: MissionId,
    run_id: RunId,
    manifest: &RealizedChangeManifest,
) -> Result<MaterializedReviewArtifact, ReviewArtifactError> {
    let snapshot_revision = manifest
        .snapshot_revision
        .as_deref()
        .ok_or(ReviewArtifactError::MissingSnapshot)?;
    let snapshot_tree = manifest
        .snapshot_tree
        .as_deref()
        .ok_or(ReviewArtifactError::MissingSnapshot)?;
    let affected_roots = manifest
        .changes
        .iter()
        .filter_map(|change| change.path.split('/').next())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let document = CandidateReviewDocument {
        schema_version: SCHEMA_VERSION,
        mission_id,
        run_id,
        base_revision: &manifest.base_revision,
        snapshot_revision,
        snapshot_tree,
        patch_sha256: &manifest.patch_sha256,
        changed_files: &manifest.changes,
        affected_roots,
        required_checks: REQUIRED_CHECKS,
    };
    let mut bytes = serde_json::to_vec_pretty(&document)?;
    bytes.push(b'\n');
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let directory = state_dir.join("review-artifacts");
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() && metadata.uid() == effective_uid() => {}
        Ok(_) => return Err(ReviewArtifactError::ContentConflict),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&directory)?;
        }
        Err(error) => return Err(error.into()),
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    let destination = directory.join(format!("{sha256}.json"));
    persist_content_addressed(&destination, &bytes)?;
    Ok(MaterializedReviewArtifact {
        locator: destination.to_string_lossy().into_owned(),
        sha256,
    })
}

fn persist_content_addressed(destination: &Path, bytes: &[u8]) -> Result<(), ReviewArtifactError> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.is_file() && metadata.uid() == effective_uid() => {
            return if fs::read(destination)? == bytes {
                Ok(())
            } else {
                Err(ReviewArtifactError::ContentConflict)
            };
        }
        Ok(_) => return Err(ReviewArtifactError::ContentConflict),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = destination
        .parent()
        .ok_or_else(|| std::io::Error::other("review Artifact has no parent directory"))?;
    let temporary = parent.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    if let Err(error) = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temporary, destination))
    {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid reads immutable process credentials and has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termi9ne_core::{ChangeOperation, RealizedChange};

    #[test]
    fn review_contract_is_content_addressed_and_explicitly_unverified() {
        let root = std::env::temp_dir().join(format!("termi9ne-review-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("fixture directory should create");
        let manifest = RealizedChangeManifest {
            schema_version: 1,
            scanner_version: 2,
            repository_identity: "fixture".to_owned(),
            base_revision: "a".repeat(40),
            head_revision: "a".repeat(40),
            snapshot_revision: Some("b".repeat(40)),
            snapshot_tree: Some("c".repeat(40)),
            snapshot_ref: Some("refs/termi9ne/candidates/m/r/b".to_owned()),
            patch_sha256: "d".repeat(64),
            changes: vec![RealizedChange {
                path: "src/main.rs".to_owned(),
                operation: ChangeOperation::Modify,
                content_sha256: Some("e".repeat(64)),
                git_mode: Some("100644".to_owned()),
                git_object_id: Some("f".repeat(40)),
            }],
        };
        let first = materialize(&root, MissionId::new(), RunId::new(), &manifest)
            .expect("review Artifact should materialize");
        let bytes = fs::read(&first.locator).expect("review Artifact should exist");
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).expect("review Artifact should be JSON");
        assert_eq!(json["required_checks"][0]["state"], "not_run");
        assert_eq!(json["affected_roots"][0], "src");
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), first.sha256);
        fs::remove_dir_all(root).expect("fixture should clean up");
    }
}
