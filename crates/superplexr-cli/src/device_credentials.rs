//! What this device keeps after pairing with a runtime's gateway.
//!
//! One file per gateway address under `~/.superplexr/devices/`, owner-only:
//! the runtime's pinned fingerprint, this device's id, and its token. The
//! token is the credential; the fingerprint is what makes the TLS session
//! trustworthy without a certificate authority.

use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use superplexr_client::GatewayEndpoint;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DeviceCredentials {
    pub address: String,
    pub fingerprint: String,
    pub device_id: Uuid,
    pub token: String,
    #[serde(default)]
    pub label: String,
}

impl DeviceCredentials {
    pub(crate) fn endpoint(&self) -> GatewayEndpoint {
        GatewayEndpoint {
            address: self.address.clone(),
            fingerprint: self.fingerprint.clone(),
            device_id: self.device_id,
            device_token: Some(self.token.clone()),
            pairing_code: None,
        }
    }
}

fn directory() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".superplexr").join("devices"))
}

/// The file for one gateway address. `host:port` is not a safe file name,
/// so it is flattened.
fn path_for(address: &str) -> Result<PathBuf, String> {
    let name = address
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    Ok(directory()?.join(format!("{name}.json")))
}

pub(crate) fn load(address: &str) -> Result<Option<DeviceCredentials>, String> {
    let path = path_for(address)?;
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

pub(crate) fn store(credentials: &DeviceCredentials) -> Result<PathBuf, String> {
    let dir = directory()?;
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = path_for(&credentials.address)?;
    let bytes = serde_json::to_vec_pretty(credentials).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| format!("{}: {e}", temporary.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", temporary.display()))?;
    drop(file);
    fs::rename(&temporary, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

pub(crate) fn forget(address: &str) -> Result<bool, String> {
    let path = path_for(address)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_become_safe_file_names() {
        let path = path_for("host.example:7373").expect("path");
        assert!(path.ends_with("host.example_7373.json"));
        let path = path_for("../evil").expect("path");
        assert!(path.ends_with(".._evil.json"), "{}", path.display());
        assert!(!path.to_string_lossy().contains("/../"));
    }
}
