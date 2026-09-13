//! The network gateway's identity and TLS configuration.
//!
//! The runtime's certificate is self-signed and made once; what a device
//! pins is its SHA-256 fingerprint, shown beside the pairing code. There is
//! no certificate authority to trust and nothing to renew: the identity is
//! the runtime, and revoking it is deleting the file.

use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Arc,
};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

/// What the runtime tells owners so a device can be pointed at it.
#[derive(Clone, Debug)]
pub(crate) struct GatewayInfo {
    /// `sha256:` followed by the certificate digest in lowercase hex.
    pub fingerprint: String,
    /// The address the listener was bound to, as given.
    pub advertised: String,
}

pub(crate) struct GatewayIdentity {
    pub certificate: CertificateDer<'static>,
    pub key: PrivateKeyDer<'static>,
    pub fingerprint: String,
}

/// Load the identity from the state directory, creating it on first use.
pub(crate) fn load_or_create_identity(state_dir: &Path) -> Result<GatewayIdentity, String> {
    let dir = state_dir.join("gateway");
    fs::create_dir_all(&dir).map_err(|e| format!("gateway directory: {e}"))?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("gateway directory permissions: {e}"))?;
    let cert_path = dir.join("identity.crt");
    let key_path = dir.join("identity.key");
    let (cert_der, key_der) = match (fs::read(&cert_path), fs::read(&key_path)) {
        (Ok(cert), Ok(key)) => (cert, key),
        _ => {
            let certified = rcgen::generate_simple_self_signed(vec!["superplexr".to_owned()])
                .map_err(|e| format!("could not create the gateway identity: {e}"))?;
            let cert = certified.cert.der().to_vec();
            let key = certified.signing_key.serialize_der();
            write_private(&cert_path, &cert)?;
            write_private(&key_path, &key)?;
            (cert, key)
        }
    };
    let fingerprint = fingerprint_hex(&cert_der);
    Ok(GatewayIdentity {
        certificate: CertificateDer::from(cert_der),
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        fingerprint,
    })
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `sha256:` and the lowercase hex digest, the form a person compares by eye.
pub(crate) fn fingerprint_hex(certificate_der: &[u8]) -> String {
    let digest = Sha256::digest(certificate_der);
    let hex = digest.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("sha256:{hex}")
}

pub(crate) fn server_config(identity: &GatewayIdentity) -> Result<Arc<rustls::ServerConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls versions: {e}"))?
        .with_no_client_auth()
        .with_single_cert(vec![identity.certificate.clone()], identity.key.clone_key())
        .map_err(|e| format!("tls identity: {e}"))?;
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_is_created_once_and_its_fingerprint_is_stable() {
        let root = std::env::temp_dir().join(format!("superplexr-gateway-{}", uuid::Uuid::new_v4()));
        let first = load_or_create_identity(&root).expect("create");
        assert!(first.fingerprint.starts_with("sha256:"));
        assert_eq!(first.fingerprint.len(), "sha256:".len() + 64);
        let second = load_or_create_identity(&root).expect("reload");
        assert_eq!(first.fingerprint, second.fingerprint, "a restart keeps the identity");
        let mode = fs::metadata(root.join("gateway/identity.key"))
            .expect("key file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the private key is owner-only");
        server_config(&second).expect("the identity yields a usable TLS config");
        let _ = fs::remove_dir_all(&root);
    }
}
