//! Web Push from the runtime: the three things worth a phone's attention —
//! a Fault opened, an approval waiting, a Run finished — delivered to any
//! browser that subscribed from the web shell, including a phone with the
//! shell installed to its home screen.
//!
//! RFC 8291 (message encryption, `aes128gcm`) and RFC 8292 (VAPID) by hand
//! on ring, because the primitives are all there — ECDH P-256, HKDF-SHA256,
//! AES-128-GCM, ECDSA P-256 — and the two RFCs together are shorter than
//! the dependency they would replace. Delivery is one HTTPS POST per
//! subscription, made with the system's `curl` so the runtime needs no TLS
//! client roots of its own.
//!
//! Subscriptions live in `push/subscriptions.json` under the state dir,
//! owner-only, keyed by endpoint. The VAPID key is made once and kept in
//! `push/vapid.p8`. A push service that answers 404 or 410 has dropped the
//! subscription and it is forgotten.

use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ring::{
    aead, agreement, hkdf,
    rand::SystemRandom,
    signature::{self, KeyPair},
};
use serde::{Deserialize, Serialize};
use ultraplexr_protocol::PushSubscription;

const RECORD_SIZE: u32 = 4096;
/// A push message must fit one record with the delimiter and the tag.
const MAX_PAYLOAD_BYTES: usize = (RECORD_SIZE as usize) - 1 - 16 - 1;
const VAPID_TTL: Duration = Duration::from_secs(12 * 60 * 60);
const CONTACT: &str = "mailto:ultraplexr@localhost";

#[derive(Debug, thiserror::Error)]
pub(crate) enum PushError {
    #[error("push storage: {0}")]
    Io(#[from] io::Error),
    #[error("push storage: {0}")]
    Json(#[from] serde_json::Error),
    #[error("push crypto failed")]
    Crypto,
    #[error("subscription keys are not valid: {0}")]
    InvalidSubscription(&'static str),
    #[error("push message is over {MAX_PAYLOAD_BYTES} bytes")]
    TooLarge,
}

impl From<ring::error::Unspecified> for PushError {
    fn from(_: ring::error::Unspecified) -> Self {
        Self::Crypto
    }
}

impl From<ring::error::KeyRejected> for PushError {
    fn from(_: ring::error::KeyRejected) -> Self {
        Self::Crypto
    }
}

/// What a notification says and where a tap should land.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Notice {
    pub title: String,
    pub body: String,
    /// A path on the web shell, e.g. `/#fault=<id>`.
    pub url: String,
    pub tag: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Stored {
    subscriptions: BTreeMap<String, PushSubscription>,
}

pub(crate) struct PushStore {
    directory: PathBuf,
    vapid: signature::EcdsaKeyPair,
    stored: Stored,
    rng: SystemRandom,
    /// Tags announced in this runtime's lifetime, so a standing condition
    /// (an approval still waiting) is not sent on every mission change.
    announced: std::collections::HashSet<String>,
}

impl PushStore {
    pub(crate) fn open(state_dir: &Path) -> Result<Self, PushError> {
        let directory = state_dir.join("push");
        fs::create_dir_all(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let rng = SystemRandom::new();
        let key_path = directory.join("vapid.p8");
        let vapid_pkcs8 = match fs::read(&key_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let document = signature::EcdsaKeyPair::generate_pkcs8(
                    &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
                    &rng,
                )?;
                write_private(&key_path, document.as_ref())?;
                document.as_ref().to_vec()
            }
            Err(error) => return Err(error.into()),
        };
        let vapid = signature::EcdsaKeyPair::from_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &vapid_pkcs8,
            &rng,
        )?;
        let stored = match fs::read(directory.join("subscriptions.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Stored {
                subscriptions: BTreeMap::new(),
            },
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            directory,
            vapid,
            stored,
            rng,
            announced: std::collections::HashSet::new(),
        })
    }

    /// True the first time a tag is seen; a standing condition announces once.
    pub(crate) fn first_time(&mut self, tag: &str) -> bool {
        self.announced.insert(tag.to_owned())
    }

    /// The application server key a browser subscribes with, base64url.
    pub(crate) fn public_key(&self) -> String {
        base64url(self.vapid.public_key().as_ref())
    }

    pub(crate) fn subscriptions(&self) -> Vec<PushSubscription> {
        self.stored.subscriptions.values().cloned().collect()
    }

    pub(crate) fn register(&mut self, subscription: PushSubscription) -> Result<(), PushError> {
        decode_key(&subscription.p256dh)?;
        decode_auth(&subscription.auth)?;
        if !subscription.endpoint.starts_with("https://") {
            return Err(PushError::InvalidSubscription("endpoint is not https"));
        }
        self.stored
            .subscriptions
            .insert(subscription.endpoint.clone(), subscription);
        self.save()
    }

    pub(crate) fn forget(&mut self, endpoint: &str) -> Result<bool, PushError> {
        let removed = self.stored.subscriptions.remove(endpoint).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> Result<(), PushError> {
        let bytes = serde_json::to_vec_pretty(&self.stored)?;
        write_private(&self.directory.join("subscriptions.json"), &bytes)
    }

    /// Encrypt `notice` for one subscription: the body of the POST.
    pub(crate) fn encrypt(
        &self,
        subscription: &PushSubscription,
        notice: &Notice,
    ) -> Result<Vec<u8>, PushError> {
        let payload = serde_json::to_vec(notice)?;
        encrypt_aes128gcm(
            &self.rng,
            &decode_key(&subscription.p256dh)?,
            &decode_auth(&subscription.auth)?,
            &payload,
        )
    }

    /// The `Authorization: vapid …` header value for one endpoint's origin.
    pub(crate) fn vapid_header(&self, endpoint: &str) -> Result<String, PushError> {
        let origin = origin_of(endpoint).ok_or(PushError::InvalidSubscription("endpoint has no origin"))?;
        let expiry = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + VAPID_TTL.as_secs();
        let header = base64url(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = base64url(
            serde_json::json!({ "aud": origin, "exp": expiry, "sub": CONTACT })
                .to_string()
                .as_bytes(),
        );
        let signing_input = format!("{header}.{claims}");
        let signature = self.vapid.sign(&self.rng, signing_input.as_bytes())?;
        Ok(format!(
            "vapid t={signing_input}.{}, k={}",
            base64url(signature.as_ref()),
            self.public_key()
        ))
    }

    /// Send one notice to every subscription. Dropped subscriptions (404,
    /// 410) are forgotten. Blocking: call from a blocking task.
    pub(crate) fn send_all(&mut self, notice: &Notice) -> Vec<(String, u16)> {
        let mut outcomes = Vec::new();
        let subscriptions = self.subscriptions();
        for subscription in subscriptions {
            let status = self.send_one(&subscription, notice).unwrap_or(0);
            if status == 404 || status == 410 {
                let _ = self.forget(&subscription.endpoint);
            }
            outcomes.push((subscription.endpoint, status));
        }
        outcomes
    }

    fn send_one(&self, subscription: &PushSubscription, notice: &Notice) -> Result<u16, PushError> {
        let body = self.encrypt(subscription, notice)?;
        let authorization = self.vapid_header(&subscription.endpoint)?;
        let temporary = self
            .directory
            .join(format!(".push-{}.bin", uuid::Uuid::new_v4()));
        write_private(&temporary, &body)?;
        let mut curl = Command::new("curl");
        curl.args(["-sS", "-o", "/dev/null", "-w", "%{http_code}", "-X", "POST"])
            .args(["--max-time", "15"]);
        // A private push service (or a test's stand-in) can be trusted by
        // dropping its certificate at push/ca.pem; public services use the
        // system roots as usual.
        let ca = self.directory.join("ca.pem");
        if ca.is_file() {
            curl.arg("--cacert").arg(&ca);
        }
        let output = curl
            .args(["-H", "Content-Encoding: aes128gcm"])
            .args(["-H", "Content-Type: application/octet-stream"])
            .args(["-H", "TTL: 86400"])
            .args(["-H", "Urgency: high"])
            .args(["-H", &format!("Authorization: {authorization}")])
            .arg("--data-binary")
            .arg(format!("@{}", temporary.display()))
            .arg(&subscription.endpoint)
            .output();
        let _ = fs::remove_file(&temporary);
        let output = output?;
        let status = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        if status == 0 {
            // curl could not deliver at all; say why, once, in the log.
            eprintln!(
                "push to {} failed before any answer: {}",
                subscription.endpoint,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(status)
    }

}

/// RFC 8291 §3 with RFC 8188 `aes128gcm` framing, one record.
fn encrypt_aes128gcm(
    rng: &SystemRandom,
    receiver_public: &[u8; 65],
    auth: &[u8; 16],
    payload: &[u8],
) -> Result<Vec<u8>, PushError> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(PushError::TooLarge);
    }
    let mut salt = [0_u8; 16];
    ring::rand::SecureRandom::fill(rng, &mut salt)?;
    let sender_private = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, rng)?;
    let sender_public = sender_private.compute_public_key()?;
    let sender_public: [u8; 65] = sender_public
        .as_ref()
        .try_into()
        .map_err(|_| PushError::Crypto)?;
    let receiver = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, receiver_public);
    let shared: [u8; 32] = agreement::agree_ephemeral(sender_private, &receiver, |secret| {
        secret.try_into().map_err(|_| PushError::Crypto)
    })??;
    let ciphertext = seal(&shared, auth, receiver_public, &sender_public, &salt, payload)?;
    let mut body = Vec::with_capacity(21 + 65 + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(65);
    body.extend_from_slice(&sender_public);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// The key schedule and the seal, shared by the sender and the test's receiver.
fn seal(
    shared: &[u8; 32],
    auth: &[u8; 16],
    receiver_public: &[u8; 65],
    sender_public: &[u8; 65],
    salt: &[u8; 16],
    payload: &[u8],
) -> Result<Vec<u8>, PushError> {
    let (key, nonce) = key_and_nonce(shared, auth, receiver_public, sender_public, salt)?;
    let mut record = Vec::with_capacity(payload.len() + 1 + 16);
    record.extend_from_slice(payload);
    record.push(0x02); // last record
    let sealing = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &key)?);
    sealing.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::empty(),
        &mut record,
    )?;
    Ok(record)
}

fn key_and_nonce(
    shared: &[u8; 32],
    auth: &[u8; 16],
    receiver_public: &[u8; 65],
    sender_public: &[u8; 65],
    salt: &[u8; 16],
) -> Result<([u8; 16], [u8; 12]), PushError> {
    // IKM = HKDF(auth, shared, "WebPush: info\0" || ua_public || as_public, 32)
    let mut info = Vec::with_capacity(14 + 65 + 65);
    info.extend_from_slice(b"WebPush: info\0");
    info.extend_from_slice(receiver_public);
    info.extend_from_slice(sender_public);
    let mut ikm = [0_u8; 32];
    hkdf::Salt::new(hkdf::HKDF_SHA256, auth)
        .extract(shared)
        .expand(&[&info], Len(32))?
        .fill(&mut ikm)?;
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(&ikm);
    let mut key = [0_u8; 16];
    prk.expand(&[b"Content-Encoding: aes128gcm\0"], Len(16))?
        .fill(&mut key)?;
    let mut nonce = [0_u8; 12];
    prk.expand(&[b"Content-Encoding: nonce\0"], Len(12))?
        .fill(&mut nonce)?;
    Ok((key, nonce))
}

struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

fn decode_key(p256dh: &str) -> Result<[u8; 65], PushError> {
    let bytes = base64url_decode(p256dh)
        .ok_or(PushError::InvalidSubscription("p256dh is not base64url"))?;
    let key: [u8; 65] = bytes
        .try_into()
        .map_err(|_| PushError::InvalidSubscription("p256dh is not a 65-byte P-256 point"))?;
    if key[0] != 0x04 {
        return Err(PushError::InvalidSubscription("p256dh is not uncompressed"));
    }
    Ok(key)
}

fn decode_auth(auth: &str) -> Result<[u8; 16], PushError> {
    base64url_decode(auth)
        .ok_or(PushError::InvalidSubscription("auth is not base64url"))?
        .try_into()
        .map_err(|_| PushError::InvalidSubscription("auth is not 16 bytes"))
}

fn origin_of(endpoint: &str) -> Option<String> {
    let rest = endpoint.strip_prefix("https://")?;
    let host = rest.split('/').next()?;
    (!host.is_empty()).then(|| format!("https://{host}"))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), PushError> {
    use std::io::Write as _;
    let temporary = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    Ok(())
}

pub(crate) fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[usize::from(b0 >> 2)] as char);
        out.push(TABLE[usize::from((b0 & 0b11) << 4 | b1 >> 4)] as char);
        if chunk.len() > 1 {
            out.push(TABLE[usize::from((b1 & 0b1111) << 2 | b2 >> 6)] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[usize::from(b2 & 0b11_1111)] as char);
        }
    }
    out
}

pub(crate) fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut buffer = 0_u32;
    let mut bits = 0_u32;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ultraplexr-push-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn base64url_round_trips_without_padding() {
        for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0xff, 0xee, 0x00]] {
            let text = base64url(bytes);
            assert!(!text.contains('='));
            assert!(!text.contains('+') && !text.contains('/'));
            assert_eq!(base64url_decode(&text).expect("decodes"), bytes);
        }
        assert_eq!(base64url(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn a_receiver_with_the_keys_decrypts_what_the_sender_sealed() {
        // The receiver is a browser: it has a P-256 key pair and a 16-byte
        // auth secret. Both sides run the same schedule; only the receiver
        // can, because only it holds the private half.
        let rng = SystemRandom::new();
        let receiver_private =
            agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).expect("key");
        let receiver_public: [u8; 65] = receiver_private
            .compute_public_key()
            .expect("public")
            .as_ref()
            .try_into()
            .expect("65 bytes");
        let mut auth = [0_u8; 16];
        ring::rand::SecureRandom::fill(&rng, &mut auth).expect("auth");
        let payload = br#"{"title":"Fault opened","body":"cargo test exited 101"}"#;
        let body = encrypt_aes128gcm(&rng, &receiver_public, &auth, payload).expect("encrypts");

        // RFC 8188 header: salt(16) | rs(4) | idlen(1) | keyid(idlen)
        assert_eq!(&body[16..20], &RECORD_SIZE.to_be_bytes());
        assert_eq!(body[20], 65);
        let salt: [u8; 16] = body[..16].try_into().expect("salt");
        let sender_public: [u8; 65] = body[21..86].try_into().expect("sender key");
        assert_eq!(sender_public[0], 0x04);
        let ciphertext = &body[86..];
        assert_eq!(ciphertext.len(), payload.len() + 1 + 16);

        let sender = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, sender_public);
        let shared: [u8; 32] = agreement::agree_ephemeral(receiver_private, &sender, |s| {
            s.try_into().expect("32 bytes")
        })
        .expect("agree");
        let (key, nonce) =
            key_and_nonce(&shared, &auth, &receiver_public, &sender_public, &salt).expect("schedule");
        let opening = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &key).expect("key"));
        let mut record = ciphertext.to_vec();
        let plain = opening
            .open_in_place(aead::Nonce::assume_unique_for_key(nonce), aead::Aad::empty(), &mut record)
            .expect("opens");
        assert_eq!(plain.last(), Some(&0x02), "last-record delimiter");
        assert_eq!(&plain[..plain.len() - 1], payload);
    }

    #[test]
    fn the_store_keeps_one_vapid_key_and_validates_subscriptions() {
        let dir = temp_dir("store");
        let mut store = PushStore::open(&dir).expect("opens");
        let public = store.public_key();
        assert_eq!(base64url_decode(&public).expect("decodes").len(), 65);
        let again = PushStore::open(&dir).expect("reopens");
        assert_eq!(again.public_key(), public, "the key is made once");

        let rng = SystemRandom::new();
        let browser = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).expect("key");
        let p256dh = base64url(browser.compute_public_key().expect("public").as_ref());
        let good = PushSubscription {
            endpoint: "https://push.example/send/abc".to_owned(),
            p256dh: p256dh.clone(),
            auth: base64url(&[7; 16]),
            label: "phone".to_owned(),
        };
        store.register(good.clone()).expect("registers");
        assert!(store.register(PushSubscription { auth: "short".to_owned(), ..good.clone() }).is_err());
        assert!(store.register(PushSubscription { endpoint: "http://plain".to_owned(), ..good.clone() }).is_err());
        assert_eq!(store.subscriptions().len(), 1);
        let header = store.vapid_header(&good.endpoint).expect("header");
        assert!(header.starts_with("vapid t=") && header.contains(", k="));
        let token = header["vapid t=".len()..].split(',').next().expect("token");
        let parts = token.split('.').collect::<Vec<_>>();
        assert_eq!(parts.len(), 3);
        let claims = base64url_decode(parts[1]).expect("claims");
        let claims: serde_json::Value = serde_json::from_slice(&claims).expect("json");
        assert_eq!(claims["aud"], "https://push.example");
        assert_eq!(base64url_decode(parts[2]).expect("signature").len(), 64, "ES256 r||s");
        assert!(store.forget(&good.endpoint).expect("forgets"));
        assert!(store.subscriptions().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
