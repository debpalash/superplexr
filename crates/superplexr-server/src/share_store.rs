use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use superplexr_core::{MissionId, SessionId};
use superplexr_protocol::{ShareRole, ShareSummary};
use thiserror::Error;
use uuid::Uuid;

const FILE_VERSION: u16 = 1;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SHARES: usize = 256;
const MAX_SCOPE_IDS: usize = 64;
const MIN_TTL_SECONDS: u64 = 60;
const MAX_TTL_SECONDS: u64 = 30 * 24 * 60 * 60;
const TOKEN_PREFIX: &str = "t9s_";

#[derive(Debug, Error)]
pub(crate) enum ShareError {
    #[error("share store is not a regular owner-controlled file: {0}")]
    Insecure(PathBuf),
    #[error("share store exceeds 1 MiB")]
    TooLarge,
    #[error("share store could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("share store is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported share store version {0}")]
    Version(u16),
    #[error("share limit of {MAX_SHARES} reached")]
    TooMany,
    #[error("share label must contain 1 to 80 bytes and no NUL")]
    InvalidLabel,
    #[error("share must scope at least one and at most {MAX_SCOPE_IDS} Missions/Sessions")]
    InvalidScope,
    #[error("share lifetime must be between 60 seconds and 30 days")]
    InvalidLifetime,
    #[error("share token is invalid, expired, or revoked")]
    Unauthorized,
    #[error("share {0} does not exist")]
    NotFound(Uuid),
    #[error("system time is before the Unix epoch")]
    Clock,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredShare {
    summary: ShareSummary,
    token_digest: [u8; 32],
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ShareFile {
    version: u16,
    shares: BTreeMap<Uuid, StoredShare>,
}

pub(crate) struct ShareStore {
    path: PathBuf,
    shares: BTreeMap<Uuid, StoredShare>,
}

impl ShareStore {
    pub(crate) fn open(path: PathBuf) -> Result<Self, ShareError> {
        let shares = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                // SAFETY: geteuid reads immutable process credentials.
                let current_uid = unsafe { libc::geteuid() };
                if metadata.file_type().is_symlink()
                    || !metadata.is_file()
                    || metadata.uid() != current_uid
                {
                    return Err(ShareError::Insecure(path));
                }
                if metadata.len() > MAX_FILE_BYTES {
                    return Err(ShareError::TooLarge);
                }
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                let file: ShareFile = serde_json::from_slice(&fs::read(&path)?)?;
                if file.version != FILE_VERSION {
                    return Err(ShareError::Version(file.version));
                }
                if file.shares.len() > MAX_SHARES {
                    return Err(ShareError::TooMany);
                }
                file.shares
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self { path, shares })
    }

    pub(crate) fn create(
        &mut self,
        label: String,
        role: ShareRole,
        mut mission_ids: Vec<MissionId>,
        mut session_ids: Vec<SessionId>,
        expires_in_seconds: u64,
    ) -> Result<(ShareSummary, String), ShareError> {
        let label = label.trim().to_owned();
        if label.is_empty() || label.len() > 80 || label.contains('\0') {
            return Err(ShareError::InvalidLabel);
        }
        mission_ids.sort();
        mission_ids.dedup();
        session_ids.sort();
        session_ids.dedup();
        let scope_len = mission_ids.len().saturating_add(session_ids.len());
        if scope_len == 0 || scope_len > MAX_SCOPE_IDS {
            return Err(ShareError::InvalidScope);
        }
        if !(MIN_TTL_SECONDS..=MAX_TTL_SECONDS).contains(&expires_in_seconds) {
            return Err(ShareError::InvalidLifetime);
        }
        if self.shares.len() == MAX_SHARES {
            return Err(ShareError::TooMany);
        }

        let now = now_micros()?;
        let expires_at_micros = now
            .checked_add(expires_in_seconds.saturating_mul(1_000_000))
            .ok_or(ShareError::InvalidLifetime)?;
        let share_id = Uuid::new_v4();
        let mut secret = [0_u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut secret)?;
        let token = format!("{TOKEN_PREFIX}{share_id}_{}", hex_encode(&secret));
        let token_digest: [u8; 32] = Sha256::digest(secret).into();
        let summary = ShareSummary {
            share_id,
            label,
            role,
            mission_ids,
            session_ids,
            created_at_micros: now,
            expires_at_micros,
            revoked_at_micros: None,
        };
        self.shares.insert(
            share_id,
            StoredShare {
                summary: summary.clone(),
                token_digest,
            },
        );
        if let Err(error) = self.persist() {
            self.shares.remove(&share_id);
            return Err(error);
        }
        Ok((summary, token))
    }

    pub(crate) fn list(&self) -> Vec<ShareSummary> {
        self.shares
            .values()
            .map(|share| share.summary.clone())
            .collect()
    }

    pub(crate) fn authenticate(&self, token: &str) -> Result<ShareSummary, ShareError> {
        let (share_id, secret) = parse_token(token).ok_or(ShareError::Unauthorized)?;
        let share = self.shares.get(&share_id).ok_or(ShareError::Unauthorized)?;
        let candidate: [u8; 32] = Sha256::digest(secret).into();
        if !constant_time_equal(&candidate, &share.token_digest)
            || share.summary.revoked_at_micros.is_some()
            || now_micros()? >= share.summary.expires_at_micros
        {
            return Err(ShareError::Unauthorized);
        }
        Ok(share.summary.clone())
    }

    pub(crate) fn revoke(&mut self, share_id: Uuid) -> Result<ShareSummary, ShareError> {
        let previous = self
            .shares
            .get(&share_id)
            .cloned()
            .ok_or(ShareError::NotFound(share_id))?;
        let share = self
            .shares
            .get_mut(&share_id)
            .expect("share existence checked above");
        if share.summary.revoked_at_micros.is_none() {
            share.summary.revoked_at_micros = Some(now_micros()?);
        }
        let summary = share.summary.clone();
        if let Err(error) = self.persist() {
            self.shares.insert(share_id, previous);
            return Err(error);
        }
        Ok(summary)
    }

    pub(crate) fn expire_due(&mut self) -> Result<Vec<Uuid>, ShareError> {
        let now = now_micros()?;
        let previous = self.shares.clone();
        let mut expired = Vec::new();
        for (share_id, share) in &mut self.shares {
            if share.summary.revoked_at_micros.is_none() && share.summary.expires_at_micros <= now {
                share.summary.revoked_at_micros = Some(now);
                expired.push(*share_id);
            }
        }
        if expired.is_empty() {
            return Ok(expired);
        }
        if let Err(error) = self.persist() {
            self.shares = previous;
            return Err(error);
        }
        Ok(expired)
    }

    fn persist(&self) -> Result<(), ShareError> {
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "share path has no parent")
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
                &ShareFile {
                    version: FILE_VERSION,
                    shares: self.shares.clone(),
                },
            )?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path)?;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
            File::open(parent)?.sync_all()?;
            Ok::<(), ShareError>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn parse_token(token: &str) -> Option<(Uuid, [u8; 32])> {
    let (share_id, secret) = token.strip_prefix(TOKEN_PREFIX)?.split_once('_')?;
    Some((Uuid::parse_str(share_id).ok()?, hex_decode(secret)?))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn hex_decode(encoded: &str) -> Option<[u8; 32]> {
    if encoded.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (index, pair) in encoded.as_bytes().chunks_exact(2).enumerate() {
        bytes[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(bytes)
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn now_micros() -> Result<u64, ShareError> {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ShareError::Clock)?
        .as_micros();
    u64::try_from(micros).map_err(|_| ShareError::Clock)
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    name.push_str(&format!(".{}.tmp", Uuid::new_v4()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_one_time_outputs_and_revocation_is_durable() {
        let root = std::env::temp_dir().join(format!("superplexr-share-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("share fixture should exist");
        let path = root.join("shares.json");
        let mission_id = MissionId::new();
        let mut store = ShareStore::open(path.clone()).expect("share store should open");
        let (share, token) = store
            .create(
                "Review partner".to_owned(),
                ShareRole::Observer,
                vec![mission_id, mission_id],
                Vec::new(),
                3_600,
            )
            .expect("share should be created");
        assert_eq!(share.mission_ids, [mission_id]);
        assert_eq!(
            store.authenticate(&token).expect("token should work"),
            share
        );
        assert!(
            !String::from_utf8_lossy(&fs::read(&path).expect("store should read")).contains(&token)
        );
        assert_eq!(
            fs::metadata(&path)
                .expect("share store should exist")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let revoked = store.revoke(share.share_id).expect("share should revoke");
        assert!(revoked.revoked_at_micros.is_some());
        assert!(matches!(
            store.authenticate(&token),
            Err(ShareError::Unauthorized)
        ));
        let reopened = ShareStore::open(path).expect("share store should reopen");
        assert!(reopened.list()[0].revoked_at_micros.is_some());
        fs::remove_dir_all(root).expect("isolated fixture should be removable");
    }

    #[test]
    fn malformed_tokens_and_unbounded_specs_fail_closed() {
        let root = std::env::temp_dir().join(format!("superplexr-share-{}", Uuid::new_v4()));
        let mut store = ShareStore::open(root.join("shares.json")).expect("store should open");
        assert!(matches!(
            store.authenticate("not-a-token"),
            Err(ShareError::Unauthorized)
        ));
        assert!(matches!(
            store.create(
                " ".to_owned(),
                ShareRole::Observer,
                vec![MissionId::new()],
                Vec::new(),
                3_600,
            ),
            Err(ShareError::InvalidLabel)
        ));
        assert!(matches!(
            store.create(
                "guest".to_owned(),
                ShareRole::Observer,
                Vec::new(),
                Vec::new(),
                3_600,
            ),
            Err(ShareError::InvalidScope)
        ));
        assert!(matches!(
            store.create(
                "guest".to_owned(),
                ShareRole::Observer,
                vec![MissionId::new()],
                Vec::new(),
                59,
            ),
            Err(ShareError::InvalidLifetime)
        ));
    }

    #[test]
    fn expiry_is_durably_promoted_to_active_revocation() {
        let root = std::env::temp_dir().join(format!("superplexr-share-{}", Uuid::new_v4()));
        let path = root.join("shares.json");
        let mut store = ShareStore::open(path.clone()).expect("store should open");
        let (share, token) = store
            .create(
                "temporary controller".to_owned(),
                ShareRole::Controller,
                Vec::new(),
                vec![SessionId::new()],
                60,
            )
            .expect("share should be created");
        store
            .shares
            .get_mut(&share.share_id)
            .expect("share should remain present")
            .summary
            .expires_at_micros = 0;

        assert_eq!(
            store.expire_due().expect("expiry should persist"),
            [share.share_id]
        );
        assert!(matches!(
            store.authenticate(&token),
            Err(ShareError::Unauthorized)
        ));
        let reopened = ShareStore::open(path).expect("store should reopen");
        assert!(reopened.list()[0].revoked_at_micros.is_some());
        fs::remove_dir_all(root).expect("isolated fixture should be removable");
    }
}
