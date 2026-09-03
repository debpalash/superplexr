//! Reaching a runtime over its network gateway.
//!
//! The runtime's certificate is self-signed; a device trusts it by the
//! SHA-256 fingerprint it was shown at pairing, and nothing else. The TLS
//! session is driven by a plain blocking pump so the rest of the client — the
//! multiplexed wire, subscriptions, reconnects — is unchanged: it sees a
//! reader and a writer, as it does for a Unix socket.

use std::{
    io::{self, Read, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
};

use rustls::{
    ClientConfig, ClientConnection,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};

/// How a client reaches a runtime.
#[derive(Clone, Debug)]
pub enum Endpoint {
    /// The local Unix socket. The peer's uid is what authenticates it.
    Unix(std::path::PathBuf),
    /// The TLS gateway of a runtime this device has paired with, or is
    /// pairing with now.
    Gateway(GatewayEndpoint),
}

#[derive(Clone, Debug)]
pub struct GatewayEndpoint {
    /// `host:port`.
    pub address: String,
    /// This device's stable identity, chosen at first pairing and kept.
    pub device_id: uuid::Uuid,
    /// `sha256:<hex>` of the runtime's certificate, as shown at pairing.
    pub fingerprint: String,
    /// The device token from a completed pairing.
    pub device_token: Option<String>,
    /// A pairing code to exchange for a token on this connection.
    pub pairing_code: Option<String>,
}

/// Trusts exactly one certificate: the one whose digest was pinned.
#[derive(Debug)]
struct PinnedFingerprint {
    digest: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedFingerprint {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let digest: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if digest == self.digest {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "the runtime's certificate does not match the pinned fingerprint".to_owned(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Parse `sha256:<64 hex>` into its digest.
pub fn parse_fingerprint(text: &str) -> Option<[u8; 32]> {
    let hex = text.trim().strip_prefix("sha256:")?;
    if hex.len() != 64 {
        return None;
    }
    let mut digest = [0_u8; 32];
    for (index, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(chunk).ok()?;
        digest[index] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(digest)
}

/// A blocking TLS stream that can be read and written from two threads.
///
/// rustls does not split a connection, so both halves share it under a
/// mutex — but nothing blocks while holding it. The reader waits for
/// ciphertext on its own clone of the socket, outside the lock, and takes
/// the lock only to feed rustls and drain plaintext; the writer takes it only
/// to encrypt and send. An earlier version blocked inside the lock with a
/// short timeout and starved the writer: requests sometimes never left.
pub struct TlsHalf {
    inner: Arc<Mutex<TlsShared>>,
    /// The reader's own handle on the socket; `None` on the writer half.
    read_socket: Option<TcpStream>,
}

struct TlsShared {
    connection: ClientConnection,
    /// Used for the handshake and by the writer.
    socket: TcpStream,
}

pub fn connect(endpoint: &GatewayEndpoint) -> io::Result<(TlsHalf, TlsHalf)> {
    let digest = parse_fingerprint(&endpoint.fingerprint).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "fingerprint must be sha256: followed by 64 hex characters",
        )
    })?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .map_err(|e| io::Error::other(format!("tls versions: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedFingerprint { digest, provider }))
        .with_no_client_auth();
    let socket = TcpStream::connect(&endpoint.address)?;
    socket.set_nodelay(true)?;
    let read_socket = socket.try_clone()?;
    // The name is irrelevant under pinning but rustls needs one.
    let name = ServerName::try_from("superplexr")
        .map_err(|e| io::Error::other(format!("server name: {e}")))?;
    let mut connection = ClientConnection::new(Arc::new(config), name)
        .map_err(|e| io::Error::other(format!("tls client: {e}")))?;
    // Drive the handshake to completion before handing the halves out, so a
    // wrong fingerprint fails here, with a clear error, not mid-frame.
    let mut handshake_socket = socket.try_clone()?;
    while connection.is_handshaking() {
        connection.complete_io(&mut handshake_socket).map_err(|e| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("tls handshake: {e}"),
            )
        })?;
    }
    let shared = Arc::new(Mutex::new(TlsShared { connection, socket }));
    Ok((
        TlsHalf {
            inner: Arc::clone(&shared),
            read_socket: Some(read_socket),
        },
        TlsHalf {
            inner: shared,
            read_socket: None,
        },
    ))
}

fn poisoned() -> io::Error {
    io::Error::other("tls connection lock poisoned")
}

impl Read for TlsHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let socket = self
            .read_socket
            .as_mut()
            .ok_or_else(|| io::Error::other("the writer half of a TLS stream cannot read"))?;
        let mut ciphertext = [0_u8; 16 * 1024];
        loop {
            // Plaintext already decrypted goes first; hold the lock only for it.
            {
                let mut guard = self.inner.lock().map_err(|_| poisoned())?;
                match guard.connection.reader().read(buf) {
                    Ok(n) if n > 0 => return Ok(n),
                    Ok(_) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }
            }
            // Wait for ciphertext with no lock held, so the writer never waits
            // on a blocked reader.
            let read = socket.read(&mut ciphertext)?;
            if read == 0 {
                return Ok(0);
            }
            let mut guard = self.inner.lock().map_err(|_| poisoned())?;
            let TlsShared {
                connection,
                socket: write_socket,
            } = &mut *guard;
            let mut pending = &ciphertext[..read];
            while !pending.is_empty() {
                let consumed = connection.read_tls(&mut pending)?;
                if consumed == 0 {
                    break;
                }
                connection
                    .process_new_packets()
                    .map_err(|e| io::Error::other(format!("tls: {e}")))?;
            }
            // Processing may queue records of ours (alerts, key updates).
            while connection.wants_write() {
                connection.write_tls(write_socket)?;
            }
        }
    }
}

impl Write for TlsHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self.inner.lock().map_err(|_| poisoned())?;
        let TlsShared { connection, socket } = &mut *guard;
        let written = connection.writer().write(buf)?;
        while connection.wants_write() {
            connection.write_tls(socket)?;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self.inner.lock().map_err(|_| poisoned())?;
        let TlsShared { connection, socket } = &mut *guard;
        connection.writer().flush()?;
        while connection.wants_write() {
            connection.write_tls(socket)?;
        }
        socket.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_parse_only_in_the_shown_form() {
        let shown = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(parse_fingerprint(&shown).map(|d| d[0]), Some(0xab));
        assert!(
            parse_fingerprint("ab".repeat(32).as_str()).is_none(),
            "prefix required"
        );
        assert!(parse_fingerprint("sha256:abc").is_none(), "wrong length");
        assert!(
            parse_fingerprint(&format!("sha256:{}", "zz".repeat(32))).is_none(),
            "not hex"
        );
    }
}
