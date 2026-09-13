//! Paired devices: what may reach the daemon over the network gateway.
//!
//! The Unix socket trusts the peer's uid. A network listener has no uid to
//! trust, so it trusts nothing by default: a connection must present a
//! device token minted by pairing, or a share token, or it is refused at the
//! handshake before a single request is read. Pairing is a short-lived code
//! shown on the daemon's host and typed on the device; the device keeps the
//! token, the daemon keeps only its digest.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use superplexr_protocol::{DeviceRole, DeviceSummary};
use thiserror::Error;
use uuid::Uuid;

const FILE_VERSION: u16 = 1;
const MAX_DEVICES: usize = 256;
const MAX_PENDING_PAIRINGS: usize = 16;
/// A pairing code is typed by a person; it lives long enough for that and
/// no longer.
pub(crate) const PAIRING_TTL_SECONDS: u64 = 5 * 60;
const PAIRING_TTL_MICROS: u64 = PAIRING_TTL_SECONDS * 1_000_000;
const TOKEN_PREFIX: &str = "spxd_";

#[derive(Debug, Error)]
pub(crate) enum DeviceError {
    #[error("device store is not a regular owner-controlled file: {0}")]
    Insecure(PathBuf),
    #[error("device store could not be read or written: {0}")]
    Io(#[from] std::io::Error),
    #[error("device store is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported device store version {0}")]
    Version(u16),
    #[error("device label is empty or too long")]
    InvalidLabel,
    #[error("too many paired devices")]
    TooMany,
    #[error("too many pairings waiting; let one expire or use it")]
    TooManyPending,
    #[error("pairing code is unknown, used, or expired")]
    UnknownPairing,
    #[error("device token is invalid or revoked")]
    Unauthorized,
    #[error("unknown device {0}")]
    Unknown(Uuid),
    #[error("system clock is before the Unix epoch")]
    Clock,
}

#[derive(Clone, Deserialize, Serialize)]
struct StoredDevice {
    summary: DeviceSummary,
    token_digest: [u8; 32],
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeviceFile {
    version: u16,
    devices: BTreeMap<Uuid, StoredDevice>,
}

/// A pairing waiting for a device to claim it. Never persisted: a restart
/// invalidates codes, which is the safe direction.
struct PendingPairing {
    label: String,
    role: DeviceRole,
    code_digest: [u8; 32],
    expires_at_micros: u64,
}

pub(crate) struct DeviceStore {
    path: Option<PathBuf>,
    devices: BTreeMap<Uuid, StoredDevice>,
    pending: Vec<PendingPairing>,
}

impl DeviceStore {
    pub(crate) fn open(path: PathBuf) -> Result<Self, DeviceError> {
        let devices = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
                    return Err(DeviceError::Insecure(path));
                }
                let file: DeviceFile = serde_json::from_slice(&fs::read(&path)?)?;
                if file.version != FILE_VERSION {
                    return Err(DeviceError::Version(file.version));
                }
                file.devices
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path: Some(path),
            devices,
            pending: Vec::new(),
        })
    }

    /// A store that forgets everything on drop, for tests.
    #[cfg(test)]
    pub(crate) fn transient() -> Self {
        Self {
            path: None,
            devices: BTreeMap::new(),
            pending: Vec::new(),
        }
    }

    /// Begin a pairing: returns the code to type on the device.
    pub(crate) fn begin_pairing(
        &mut self,
        label: String,
        role: DeviceRole,
    ) -> Result<String, DeviceError> {
        let label = label.trim().to_owned();
        if label.is_empty() || label.len() > 80 || label.contains('\0') {
            return Err(DeviceError::InvalidLabel);
        }
        let now = now_micros()?;
        self.pending.retain(|pairing| pairing.expires_at_micros > now);
        if self.pending.len() >= MAX_PENDING_PAIRINGS {
            return Err(DeviceError::TooManyPending);
        }
        // Eight characters from an alphabet without look-alikes: easy to read
        // off one screen and type on another, and 32^8 against a five minute
        // window is plenty.
        let code = pairing_code()?;
        self.pending.push(PendingPairing {
            label,
            role,
            code_digest: Sha256::digest(code.as_bytes()).into(),
            expires_at_micros: now + PAIRING_TTL_MICROS,
        });
        Ok(code)
    }

    /// Claim a pairing with its code: mints the device and its token. The
    /// token is returned once and never stored.
    pub(crate) fn complete_pairing(
        &mut self,
        code: &str,
        device_id: Uuid,
    ) -> Result<(DeviceSummary, String), DeviceError> {
        let now = now_micros()?;
        self.pending.retain(|pairing| pairing.expires_at_micros > now);
        let digest: [u8; 32] = Sha256::digest(code.trim().to_ascii_uppercase().as_bytes()).into();
        let index = self
            .pending
            .iter()
            .position(|pairing| pairing.code_digest == digest)
            .ok_or(DeviceError::UnknownPairing)?;
        if self.devices.len() >= MAX_DEVICES {
            return Err(DeviceError::TooMany);
        }
        let pairing = self.pending.remove(index);
        let mut secret = [0_u8; 32];
        File::open("/dev/urandom")?.read_exact(&mut secret)?;
        let token = format!("{TOKEN_PREFIX}{device_id}_{}", hex_encode(&secret));
        let summary = DeviceSummary {
            device_id,
            label: pairing.label,
            role: pairing.role,
            paired_at_micros: now,
            last_seen_at_micros: Some(now),
            revoked_at_micros: None,
        };
        self.devices.insert(
            device_id,
            StoredDevice {
                summary: summary.clone(),
                token_digest: Sha256::digest(secret).into(),
            },
        );
        if let Err(error) = self.persist() {
            self.devices.remove(&device_id);
            return Err(error);
        }
        Ok((summary, token))
    }

    /// Verify a device token. Records when the device was last seen.
    pub(crate) fn authenticate(&mut self, token: &str) -> Result<DeviceSummary, DeviceError> {
        let (device_id, secret) = parse_token(token).ok_or(DeviceError::Unauthorized)?;
        let candidate: [u8; 32] = Sha256::digest(secret).into();
        let device = self.devices.get_mut(&device_id).ok_or(DeviceError::Unauthorized)?;
        if device.summary.revoked_at_micros.is_some() || !constant_time_eq(&device.token_digest, &candidate) {
            return Err(DeviceError::Unauthorized);
        }
        device.summary.last_seen_at_micros = Some(now_micros()?);
        let summary = device.summary.clone();
        // Last-seen is informational; a failure to record it must not refuse
        // a valid device.
        let _ = self.persist();
        Ok(summary)
    }

    /// True while the device may keep using an open connection.
    pub(crate) fn is_active(&self, device_id: Uuid) -> bool {
        self.devices
            .get(&device_id)
            .is_some_and(|device| device.summary.revoked_at_micros.is_none())
    }

    pub(crate) fn list(&self) -> Vec<DeviceSummary> {
        self.devices.values().map(|device| device.summary.clone()).collect()
    }

    /// Revocation is immediate for new connections; open ones are closed by
    /// the caller.
    pub(crate) fn revoke(&mut self, device_id: Uuid) -> Result<DeviceSummary, DeviceError> {
        let device = self.devices.get_mut(&device_id).ok_or(DeviceError::Unknown(device_id))?;
        if device.summary.revoked_at_micros.is_none() {
            device.summary.revoked_at_micros = Some(now_micros()?);
        }
        let summary = device.summary.clone();
        self.persist()?;
        Ok(summary)
    }

    fn persist(&self) -> Result<(), DeviceError> {
        let Some(path) = &self.path else { return Ok(()) };
        let file = DeviceFile {
            version: FILE_VERSION,
            devices: self.devices.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)?;
        let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
        let mut handle = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        handle.write_all(&bytes)?;
        handle.sync_all()?;
        drop(handle);
        fs::rename(&temporary, path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
}

fn now_micros() -> Result<u64, DeviceError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros() as u64)
        .map_err(|_| DeviceError::Clock)
}

const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn pairing_code() -> Result<String, DeviceError> {
    let mut raw = [0_u8; 8];
    File::open("/dev/urandom")?.read_exact(&mut raw)?;
    Ok(raw
        .iter()
        .map(|byte| CODE_ALPHABET[usize::from(byte % 32)] as char)
        .collect())
}

fn parse_token(token: &str) -> Option<(Uuid, Vec<u8>)> {
    let rest = token.strip_prefix(TOKEN_PREFIX)?;
    let (id, hex) = rest.split_once('_')?;
    let device_id = id.parse().ok()?;
    let secret = hex_decode(hex)?;
    (secret.len() == 32).then_some((device_id, secret))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_mints_a_token_that_authenticates_once_paired_and_not_after_revocation() {
        let mut store = DeviceStore::transient();
        let code = store
            .begin_pairing("phone".to_owned(), DeviceRole::Owner)
            .expect("pairing begins");
        assert_eq!(code.len(), 8);

        // Wrong code: refused, and the pairing is still waiting.
        assert!(matches!(
            store.complete_pairing("NOPE0000", Uuid::new_v4()),
            Err(DeviceError::UnknownPairing)
        ));

        let device_id = Uuid::new_v4();
        let (device, token) = store
            .complete_pairing(&code.to_lowercase(), device_id)
            .expect("case-insensitive code pairs");
        assert_eq!(device.device_id, device_id);
        assert!(token.starts_with(TOKEN_PREFIX));

        // The code is single use.
        assert!(matches!(
            store.complete_pairing(&code, Uuid::new_v4()),
            Err(DeviceError::UnknownPairing)
        ));

        let seen = store.authenticate(&token).expect("token authenticates");
        assert_eq!(seen.device_id, device_id);
        assert!(matches!(
            store.authenticate("spxd_not_a_token"),
            Err(DeviceError::Unauthorized)
        ));
        let mut wrong = token.clone();
        let last = wrong.pop().expect("token has a last character");
        wrong.push(if last == '0' { '1' } else { '0' });
        assert!(matches!(store.authenticate(&wrong), Err(DeviceError::Unauthorized)));

        store.revoke(device_id).expect("revoke");
        assert!(matches!(store.authenticate(&token), Err(DeviceError::Unauthorized)));
        assert!(store.list()[0].revoked_at_micros.is_some());
    }

    #[test]
    fn devices_round_trip_through_an_owner_only_file() {
        let root = std::env::temp_dir().join(format!("superplexr-devices-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("root");
        let path = root.join("devices.json");
        let token = {
            let mut store = DeviceStore::open(path.clone()).expect("open new");
            let code = store.begin_pairing("laptop".to_owned(), DeviceRole::Owner).expect("code");
            store.complete_pairing(&code, Uuid::new_v4()).expect("pair").1
        };
        assert_eq!(fs::metadata(&path).expect("file").permissions().mode() & 0o777, 0o600);
        let mut reopened = DeviceStore::open(path.clone()).expect("reopen");
        assert!(reopened.authenticate(&token).is_ok(), "the digest survives a restart");
        assert!(
            reopened.pending.is_empty(),
            "pairings do not survive a restart; that is the safe direction"
        );
        assert!(!fs::read_to_string(&path).expect("read").contains(&token[token.len() - 16..]),
            "the token itself is never written");
        let _ = fs::remove_dir_all(&root);
    }
}
