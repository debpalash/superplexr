//! Opt-in framing policy, separate from authentication and API-origin policy.
use axum::http::{HeaderValue, Uri};
use std::net::IpAddr;

const CSP_PREFIX: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors ";
pub(crate) const STANDALONE_CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// A bounded list of trusted parent origins, not a CORS or authorization grant.
/// HTTPS origins and HTTP loopback origins are accepted. Wildcards, credentials,
/// paths, queries, fragments, opaque origins, and CSP expressions are rejected.
#[derive(Clone)]
pub struct EmbeddingPolicy {
    origins: Vec<String>,
    pub(crate) csp: HeaderValue,
}

impl EmbeddingPolicy {
    pub fn new(origins: Vec<String>) -> Result<Self, &'static str> {
        if origins.is_empty() || origins.len() > 16 {
            return Err("embedding requires between 1 and 16 explicit parent origins");
        }
        let mut normalized = Vec::with_capacity(origins.len());
        for origin in origins {
            let origin = normalize_origin(&origin)?;
            if !normalized.contains(&origin) {
                normalized.push(origin);
            }
        }
        let csp = HeaderValue::from_str(&format!("{CSP_PREFIX}{}", normalized.join(" ")))
            .map_err(|_| "invalid embedding security policy")?;
        Ok(Self {
            origins: normalized,
            csp,
        })
    }

    pub(crate) fn permits_origin(&self, origin: &HeaderValue) -> bool {
        origin
            .to_str()
            .ok()
            .and_then(|value| normalize_origin(value).ok())
            .is_some_and(|value| self.origins.contains(&value))
    }
}

fn normalize_origin(value: &str) -> Result<String, &'static str> {
    const INVALID: &str = "embed origin must be an HTTPS origin or HTTP loopback origin, without path, credentials, wildcard, query, or fragment";
    if value.len() > 256
        || value.is_empty()
        || !value.is_ascii()
        || value.bytes().any(|byte| {
            byte.is_ascii_whitespace()
                || byte.is_ascii_control()
                || matches!(
                    byte,
                    b'@' | b'*' | b'\'' | b'"' | b';' | b',' | b'\\' | b'%' | b'?' | b'#'
                )
        })
    {
        return Err(INVALID);
    }
    let uri: Uri = value.parse().map_err(|_| INVALID)?;
    let scheme = uri.scheme_str().ok_or(INVALID)?;
    if !matches!(scheme, "https" | "http") {
        return Err(INVALID);
    }
    let authority = uri.authority().ok_or(INVALID)?;
    // Compare the original input as well: Uri gives an absent path the same
    // presentation as '/', but an origin must not contain either a path or '/'.
    if value != format!("{scheme}://{authority}") {
        return Err(INVALID);
    }
    let host = authority.host().to_ascii_lowercase();
    let ip_host = host
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .unwrap_or(&host);
    let ip = ip_host.parse::<IpAddr>().ok();
    if ip.is_none() {
        // ASCII DNS labels (including explicit punycode), never CSP syntax or
        // browser-specific numeric IPv4 spellings such as 127.1 or 0x7f000001.
        let labels_valid = host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            });
        let last = host.rsplit('.').next().ok_or(INVALID)?;
        if !labels_valid || last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x") {
            return Err(INVALID);
        }
    }
    if scheme == "http" && host != "localhost" && !ip.is_some_and(|ip| ip.is_loopback()) {
        return Err("HTTP embed origins must be localhost or a loopback IP address");
    }
    let port = authority.port_u16();
    // An unparsed or empty explicit port must not silently become the default.
    let expected_authority = match port {
        Some(0) => return Err(INVALID),
        Some(port) => format!("{}:{port}", authority.host()),
        None => authority.host().to_owned(),
    };
    if authority.as_str() != expected_authority {
        return Err(INVALID);
    }
    let host = match ip {
        Some(IpAddr::V4(ip)) => ip.to_string(),
        Some(IpAddr::V6(ip)) => format!("[{ip}]"),
        None => host,
    };
    match port {
        Some(port) if !(scheme == "https" && port == 443 || scheme == "http" && port == 80) => {
            Ok(format!("{scheme}://{host}:{port}"))
        }
        _ => Ok(format!("{scheme}://{host}")),
    }
}
