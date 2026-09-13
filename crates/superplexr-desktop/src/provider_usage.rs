//! Usage-limit probes for the agent CLIs a person may run inside superplexr:
//! Claude Code, Codex, and OpenCode.
//!
//! Each probe reuses the login the CLI already keeps on this machine, holds the
//! bearer token in memory only for the duration of one request, and calls the
//! provider's own usage endpoint through the system `curl` binary (the same
//! transport the CLIs' `/usage` views rely on; Anthropic's edge rejects most
//! non-browser TLS stacks). Nothing secret is logged, persisted, rendered, or
//! placed on a command line: curl reads its configuration from stdin.

use std::{
    collections::BTreeMap,
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub(crate) const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const REQUEST_TIMEOUT_SECS: u64 = 20;
const MAX_BODY_BYTES: usize = 256 * 1024;
const MAX_CREDENTIAL_BYTES: u64 = 1024 * 1024;
const MIN_REFRESH_SECS: u64 = 30;
const MAX_REFRESH_SECS: u64 = 3_600;

/// The agent CLIs whose plan limits the sidebar tracks.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProviderKind {
    Claude,
    Codex,
    OpenCode,
}

impl ProviderKind {
    pub(crate) const ALL: [ProviderKind; 3] = [Self::Claude, Self::Codex, Self::OpenCode];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
        }
    }

    /// Embedded brand mark, served through [`crate::assets::DesktopAssets`].
    pub(crate) fn icon_path(self) -> &'static str {
        match self {
            Self::Claude => "icons/claude.svg",
            Self::Codex => "icons/openai.svg",
            Self::OpenCode => "icons/opencode.svg",
        }
    }

    /// Classify a foreground executable without inspecting its arguments or
    /// terminal contents. Release binaries commonly carry platform suffixes.
    pub(crate) fn from_process_name(process: &str) -> Option<Self> {
        let name = Path::new(process)
            .file_name()
            .and_then(|name| name.to_str())?
            .trim_end_matches(".exe")
            .to_ascii_lowercase();
        if name == "codex" || name.starts_with("codex-") {
            Some(Self::Codex)
        } else if name == "opencode" || name.starts_with("opencode-") {
            Some(Self::OpenCode)
        } else if name == "claude" || name.starts_with("claude-") {
            Some(Self::Claude)
        } else {
            None
        }
    }

    pub(crate) fn index(self) -> usize {
        match self {
            Self::Claude => 0,
            Self::Codex => 1,
            Self::OpenCode => 2,
        }
    }

    /// The command that links a subscription, shown when no login exists.
    pub(crate) fn sign_in_command(self) -> &'static str {
        match self {
            Self::Claude => "claude /login",
            Self::Codex => "codex login",
            Self::OpenCode => "opencode auth login",
        }
    }
}

/// How long a usage window lasts. The sidebar shows one short and one long
/// window per provider; the settings page lists all of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowSpan {
    Short,
    Long,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UsageWindow {
    pub(crate) label: String,
    pub(crate) span: WindowSpan,
    /// Utilisation in percent, clamped to `0.0..=100.0`.
    pub(crate) used_percent: f32,
    pub(crate) resets_at_unix: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ProviderUsage {
    pub(crate) plan: Option<String>,
    /// Human description of where the login came from, never the login itself.
    pub(crate) source: String,
    pub(crate) windows: Vec<UsageWindow>,
    pub(crate) notes: Vec<String>,
}

impl ProviderUsage {
    pub(crate) fn short_window(&self) -> Option<&UsageWindow> {
        self.windows
            .iter()
            .find(|window| window.span == WindowSpan::Short)
    }

    pub(crate) fn long_window(&self) -> Option<&UsageWindow> {
        self.windows
            .iter()
            .find(|window| window.span == WindowSpan::Long)
    }

    pub(crate) fn peak_percent(&self) -> Option<f32> {
        self.windows
            .iter()
            .map(|window| window.used_percent)
            .fold(None, |peak, value| {
                Some(peak.map_or(value, |peak: f32| peak.max(value)))
            })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ProviderUsageStatus {
    /// Hidden by the person in settings; no probe runs.
    Disabled,
    /// No usable login on this machine. Carries a short explanation.
    NotSignedIn(String),
    /// First probe still running; nothing to show yet.
    Loading,
    Ready(ProviderUsage),
    /// The probe failed. The last good reading, if any, stays visible.
    Failed {
        detail: String,
        previous: Option<ProviderUsage>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProviderUsageSnapshot {
    pub(crate) kind: ProviderKind,
    pub(crate) status: ProviderUsageStatus,
    pub(crate) refreshing: bool,
    pub(crate) fetched_at_unix: Option<i64>,
}

impl ProviderUsageSnapshot {
    pub(crate) fn initial(kind: ProviderKind, enabled: bool) -> Self {
        Self {
            kind,
            status: if enabled {
                ProviderUsageStatus::Loading
            } else {
                ProviderUsageStatus::Disabled
            },
            refreshing: false,
            fetched_at_unix: None,
        }
    }

    pub(crate) fn usage(&self) -> Option<&ProviderUsage> {
        match &self.status {
            ProviderUsageStatus::Ready(usage) => Some(usage),
            ProviderUsageStatus::Failed {
                previous: Some(usage),
                ..
            } => Some(usage),
            _ => None,
        }
    }

    /// Replace the status with a fresh probe result, keeping the last good
    /// reading visible when the new probe failed.
    pub(crate) fn apply(&mut self, status: ProviderUsageStatus, now_unix: i64) {
        let previous = self.usage().cloned();
        self.status = match status {
            ProviderUsageStatus::Failed { detail, .. } => {
                ProviderUsageStatus::Failed { detail, previous }
            }
            other => other,
        };
        self.refreshing = false;
        self.fetched_at_unix = Some(now_unix);
    }
}

fn default_true() -> bool {
    true
}

fn default_refresh_secs() -> u64 {
    300
}

/// Owner-controlled, persisted preferences for the provider usage strip.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProviderUsageSettings {
    #[serde(default = "default_true")]
    pub(crate) claude: bool,
    #[serde(default = "default_true")]
    pub(crate) codex: bool,
    #[serde(default = "default_true")]
    pub(crate) opencode: bool,
    #[serde(default = "default_refresh_secs")]
    pub(crate) refresh_interval_secs: u64,
}

impl Default for ProviderUsageSettings {
    fn default() -> Self {
        Self {
            claude: true,
            codex: true,
            opencode: true,
            refresh_interval_secs: default_refresh_secs(),
        }
    }
}

impl ProviderUsageSettings {
    pub(crate) const REFRESH_CHOICES: [(u64, &'static str); 4] =
        [(60, "1m"), (300, "5m"), (900, "15m"), (1_800, "30m")];

    pub(crate) fn enabled(&self, kind: ProviderKind) -> bool {
        match kind {
            ProviderKind::Claude => self.claude,
            ProviderKind::Codex => self.codex,
            ProviderKind::OpenCode => self.opencode,
        }
    }

    pub(crate) fn set_enabled(&mut self, kind: ProviderKind, enabled: bool) {
        match kind {
            ProviderKind::Claude => self.claude = enabled,
            ProviderKind::Codex => self.codex = enabled,
            ProviderKind::OpenCode => self.opencode = enabled,
        }
    }

    pub(crate) fn refresh_interval(&self) -> Duration {
        Duration::from_secs(
            self.refresh_interval_secs
                .clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS),
        )
    }
}

/// Where each CLI keeps its login on this machine. Resolved once per probe
/// from the process environment so tests can point it at fixtures.
#[derive(Clone, Debug)]
pub(crate) struct ProbeEnvironment {
    pub(crate) claude_config_dir: PathBuf,
    pub(crate) codex_home: PathBuf,
    pub(crate) opencode_data_dir: PathBuf,
    pub(crate) use_keychain: bool,
    pub(crate) openai_api_key_present: bool,
}

impl ProbeEnvironment {
    pub(crate) fn from_process() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let claude_config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"));
        let opencode_data_dir = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
            .join("opencode");
        Self {
            claude_config_dir,
            codex_home,
            opencode_data_dir,
            use_keychain: cfg!(target_os = "macos"),
            openai_api_key_present: std::env::var_os("OPENAI_API_KEY")
                .is_some_and(|value| !value.is_empty()),
        }
    }
}

/// Bearer token held in memory for one request. Debug output never shows it.
struct Secret(String);

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

#[derive(Debug)]
enum ProbeError {
    NotSignedIn(String),
    Failed(String),
}

impl From<ProbeError> for ProviderUsageStatus {
    fn from(error: ProbeError) -> Self {
        match error {
            ProbeError::NotSignedIn(detail) => Self::NotSignedIn(detail),
            ProbeError::Failed(detail) => Self::Failed {
                detail,
                previous: None,
            },
        }
    }
}

/// Run one blocking probe. Call from a background executor.
pub(crate) fn probe(kind: ProviderKind, env: &ProbeEnvironment) -> ProviderUsageStatus {
    let result = match kind {
        ProviderKind::Claude => probe_claude(env),
        ProviderKind::Codex => probe_codex(env),
        ProviderKind::OpenCode => probe_opencode(env),
    };
    match result {
        Ok(usage) => ProviderUsageStatus::Ready(usage),
        Err(error) => error.into(),
    }
}

fn probe_claude(env: &ProbeEnvironment) -> Result<ProviderUsage, ProbeError> {
    let login = claude_login(env)?;
    let body = fetch_claude_usage(&login.token)?;
    parse_claude_usage(&body, &login.source, login.plan)
        .map_err(|detail| ProbeError::Failed(format!("Unexpected usage response: {detail}")))
}

fn probe_codex(env: &ProbeEnvironment) -> Result<ProviderUsage, ProbeError> {
    let login = codex_login(env)?;
    let body = fetch_codex_usage(&login.token, login.account_id.as_deref())?;
    parse_codex_usage(&body, &login.source)
        .map_err(|detail| ProbeError::Failed(format!("Unexpected usage response: {detail}")))
}

fn probe_opencode(env: &ProbeEnvironment) -> Result<ProviderUsage, ProbeError> {
    let path = env.opencode_data_dir.join("auth.json");
    let text = read_credential_file(&path)?
        .ok_or_else(|| ProbeError::NotSignedIn("No OpenCode logins".to_owned()))?;
    let auth = parse_opencode_auth(&text)
        .map_err(|detail| ProbeError::Failed(format!("Unreadable OpenCode auth.json: {detail}")))?;
    if auth.is_empty() {
        return Err(ProbeError::NotSignedIn("No OpenCode logins".to_owned()));
    }

    let mut usage = ProviderUsage {
        plan: None,
        source: "OpenCode".to_owned(),
        windows: Vec::new(),
        notes: Vec::new(),
    };
    let mut api_key_providers = Vec::new();
    let mut failures = Vec::new();
    let mut linked = Vec::new();

    for (provider, entry) in &auth {
        match entry {
            OpenCodeAuth::OAuth { access, account_id } => match provider.as_str() {
                "anthropic" => match fetch_claude_usage(access).and_then(|body| {
                    parse_claude_usage(&body, "OpenCode · Anthropic", None)
                        .map_err(ProbeError::Failed)
                }) {
                    Ok(fetched) => {
                        linked.push("Anthropic");
                        usage
                            .windows
                            .extend(prefixed_windows("Anthropic", fetched.windows));
                        usage.notes.extend(fetched.notes);
                    }
                    Err(error) => failures.push(format!("Anthropic: {}", error_detail(error))),
                },
                "openai" => {
                    match fetch_codex_usage(access, account_id.as_deref()).and_then(|body| {
                        parse_codex_usage(&body, "OpenCode · OpenAI").map_err(ProbeError::Failed)
                    }) {
                        Ok(fetched) => {
                            linked.push("OpenAI");
                            if usage.plan.is_none() {
                                usage.plan = fetched.plan;
                            }
                            usage
                                .windows
                                .extend(prefixed_windows("OpenAI", fetched.windows));
                            usage.notes.extend(fetched.notes);
                        }
                        Err(error) => failures.push(format!("OpenAI: {}", error_detail(error))),
                    }
                }
                _ => api_key_providers.push(format!("{provider} (OAuth)")),
            },
            OpenCodeAuth::ApiKey | OpenCodeAuth::WellKnown => {
                api_key_providers.push(provider.clone());
            }
        }
    }

    if !api_key_providers.is_empty() {
        usage
            .notes
            .push(format!("API key · {}", api_key_providers.join(", ")));
    }
    if linked.is_empty() {
        if let Some(first) = failures.first() {
            return Err(ProbeError::Failed(first.clone()));
        }
        return Err(ProbeError::NotSignedIn(format!(
            "API key only · {}",
            api_key_providers.join(", ")
        )));
    }
    usage.source = format!("OpenCode · {}", linked.join(" + "));
    for failure in failures {
        usage.notes.push(failure);
    }
    Ok(usage)
}

fn error_detail(error: ProbeError) -> String {
    match error {
        ProbeError::NotSignedIn(detail) | ProbeError::Failed(detail) => detail,
    }
}

fn prefixed_windows(prefix: &str, windows: Vec<UsageWindow>) -> Vec<UsageWindow> {
    windows
        .into_iter()
        .map(|window| UsageWindow {
            label: format!("{prefix} {}", window.label),
            ..window
        })
        .collect()
}

// --- Credential discovery -------------------------------------------------

#[derive(Debug)]
struct ClaudeLogin {
    token: Secret,
    plan: Option<String>,
    source: String,
}

fn claude_login(env: &ProbeEnvironment) -> Result<ClaudeLogin, ProbeError> {
    if env.use_keychain
        && let Some(text) = read_keychain_secret(CLAUDE_KEYCHAIN_SERVICE)
    {
        return parse_claude_credentials(&text, "macOS Keychain");
    }
    let path = env.claude_config_dir.join(".credentials.json");
    match read_credential_file(&path)? {
        Some(text) => parse_claude_credentials(&text, "~/.claude"),
        None => Err(ProbeError::NotSignedIn("No Claude login".to_owned())),
    }
}

fn parse_claude_credentials(text: &str, source: &str) -> Result<ClaudeLogin, ProbeError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| ProbeError::Failed(format!("Unreadable Claude credentials: {error}")))?;
    let oauth = value
        .get("claudeAiOauth")
        .filter(|oauth| oauth.is_object())
        .ok_or_else(|| ProbeError::NotSignedIn("No Claude subscription login".to_owned()))?;
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| ProbeError::NotSignedIn("Claude login has no token".to_owned()))?;
    let plan = oauth
        .get("subscriptionType")
        .and_then(Value::as_str)
        .map(pretty_plan);
    Ok(ClaudeLogin {
        token: Secret(token.to_owned()),
        plan,
        source: source.to_owned(),
    })
}

#[derive(Debug)]
struct CodexLogin {
    token: Secret,
    account_id: Option<String>,
    source: String,
}

fn codex_login(env: &ProbeEnvironment) -> Result<CodexLogin, ProbeError> {
    let path = env.codex_home.join("auth.json");
    match read_credential_file(&path)? {
        Some(text) => parse_codex_credentials(&text, env.openai_api_key_present),
        None if env.openai_api_key_present => Err(ProbeError::NotSignedIn(
            "API key · no plan windows".to_owned(),
        )),
        None => Err(ProbeError::NotSignedIn("No Codex login".to_owned())),
    }
}

fn parse_codex_credentials(text: &str, api_key_env: bool) -> Result<CodexLogin, ProbeError> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| ProbeError::Failed(format!("Unreadable Codex auth.json: {error}")))?;
    let tokens = value.get("tokens").filter(|tokens| tokens.is_object());
    let token = tokens
        .and_then(|tokens| tokens.get("access_token"))
        .and_then(Value::as_str)
        .filter(|token| !token.trim().is_empty());
    let Some(token) = token else {
        let api_key = value
            .get("OPENAI_API_KEY")
            .and_then(Value::as_str)
            .is_some_and(|key| !key.is_empty())
            || api_key_env;
        return Err(ProbeError::NotSignedIn(if api_key {
            "API key · no plan windows".to_owned()
        } else {
            "No Codex login".to_owned()
        }));
    };
    let account_id = tokens
        .and_then(|tokens| tokens.get("account_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    Ok(CodexLogin {
        token: Secret(token.to_owned()),
        account_id,
        source: "~/.codex".to_owned(),
    })
}

#[derive(Debug)]
enum OpenCodeAuth {
    OAuth {
        access: Secret,
        account_id: Option<String>,
    },
    ApiKey,
    WellKnown,
}

fn parse_opencode_auth(text: &str) -> Result<BTreeMap<String, OpenCodeAuth>, String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let Some(entries) = value.as_object() else {
        return Err("expected a JSON object keyed by provider".to_owned());
    };
    let mut auth = BTreeMap::new();
    for (provider, entry) in entries {
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let parsed = match kind {
            "oauth" => {
                let Some(access) = entry
                    .get("access")
                    .and_then(Value::as_str)
                    .filter(|token| !token.is_empty())
                else {
                    continue;
                };
                OpenCodeAuth::OAuth {
                    access: Secret(access.to_owned()),
                    account_id: entry
                        .get("accountId")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                }
            }
            "api" => OpenCodeAuth::ApiKey,
            "wellknown" => OpenCodeAuth::WellKnown,
            _ => continue,
        };
        auth.insert(provider.trim_end_matches('/').to_owned(), parsed);
    }
    Ok(auth)
}

/// Read an owner-only credential file. Missing files are `Ok(None)`; anything
/// suspicious (symlink, oversized) is a failure rather than a silent skip.
fn read_credential_file(path: &Path) -> Result<Option<String>, ProbeError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ProbeError::Failed(format!(
                "Cannot read {}: {error}",
                path.display()
            )));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ProbeError::Failed(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        return Err(ProbeError::Failed(format!(
            "{} is unexpectedly large",
            path.display()
        )));
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|error| ProbeError::Failed(format!("Cannot read {}: {error}", path.display())))
}

fn read_keychain_secret(service: &str) -> Option<String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", service, "-w"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

// --- Transport ------------------------------------------------------------

fn fetch_claude_usage(token: &Secret) -> Result<String, ProbeError> {
    let headers = [
        ("Authorization", format!("Bearer {}", token.0)),
        ("anthropic-beta", "oauth-2025-04-20".to_owned()),
        ("Accept", "application/json".to_owned()),
        ("User-Agent", "superplexr-desktop".to_owned()),
    ];
    let (status, body) = curl_get(CLAUDE_USAGE_URL, &headers)?;
    match status {
        200..=299 => Ok(body),
        401 | 403 => Err(ProbeError::NotSignedIn("Login expired".to_owned())),
        _ => Err(ProbeError::Failed(http_failure(status, &body))),
    }
}

fn fetch_codex_usage(token: &Secret, account_id: Option<&str>) -> Result<String, ProbeError> {
    let mut headers = vec![
        ("Authorization", format!("Bearer {}", token.0)),
        ("Accept", "application/json".to_owned()),
        ("User-Agent", "codex-cli".to_owned()),
    ];
    if let Some(account_id) = account_id {
        headers.push(("ChatGPT-Account-Id", account_id.to_owned()));
    }
    let (status, body) = curl_get(CODEX_USAGE_URL, &headers)?;
    match status {
        200..=299 => Ok(body),
        401 | 403 => Err(ProbeError::NotSignedIn("Login expired".to_owned())),
        _ => Err(ProbeError::Failed(http_failure(status, &body))),
    }
}

fn http_failure(status: u16, body: &str) -> String {
    let excerpt: String = body.chars().filter(|c| !c.is_control()).take(120).collect();
    if excerpt.trim().is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {}", excerpt.trim())
    }
}

/// Build the curl configuration for one GET. Secrets travel on stdin only.
fn curl_config(url: &str, headers: &[(&str, String)]) -> String {
    let mut config = String::new();
    config.push_str(&format!("url = \"{}\"\n", curl_quote(url)));
    for (name, value) in headers {
        config.push_str(&format!(
            "header = \"{}: {}\"\n",
            curl_quote(name),
            curl_quote(value)
        ));
    }
    config.push_str("silent\nshow-error\nfail-early\n");
    config.push_str(&format!("max-time = {REQUEST_TIMEOUT_SECS}\n"));
    config.push_str("write-out = \"\\n%{http_code}\"\n");
    config
}

fn curl_quote(value: &str) -> String {
    value
        .chars()
        .filter(|c| !matches!(c, '\n' | '\r'))
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '"' => vec!['\\', '"'],
            other => vec![other],
        })
        .collect()
}

fn curl_get(url: &str, headers: &[(&str, String)]) -> Result<(u16, String), ProbeError> {
    let mut child = Command::new("curl")
        .args(["--config", "-", "--output", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ProbeError::Failed(format!("curl unavailable: {error}")))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| ProbeError::Failed("curl stdin unavailable".to_owned()))?;
        stdin
            .write_all(curl_config(url, headers).as_bytes())
            .map_err(|error| ProbeError::Failed(format!("curl configuration failed: {error}")))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| ProbeError::Failed(format!("curl did not finish: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(ProbeError::Failed(if stderr.is_empty() {
            format!("curl exited with {}", output.status)
        } else {
            stderr.chars().take(160).collect()
        }));
    }
    let mut stdout = output.stdout;
    if stdout.len() > MAX_BODY_BYTES {
        return Err(ProbeError::Failed("usage response too large".to_owned()));
    }
    let split = stdout
        .iter()
        .rposition(|byte| *byte == b'\n')
        .ok_or_else(|| ProbeError::Failed("curl returned no status code".to_owned()))?;
    let code = String::from_utf8_lossy(&stdout[split + 1..])
        .trim()
        .parse::<u16>()
        .map_err(|_| ProbeError::Failed("curl returned an unreadable status".to_owned()))?;
    stdout.truncate(split);
    Ok((code, String::from_utf8_lossy(&stdout).into_owned()))
}

// --- Response parsing -----------------------------------------------------

fn parse_claude_usage(
    body: &str,
    source: &str,
    plan: Option<String>,
) -> Result<ProviderUsage, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("expected a JSON object".to_owned());
    }
    const BUCKETS: [(&str, &str, WindowSpan); 5] = [
        ("five_hour", "5h", WindowSpan::Short),
        ("seven_day", "7d", WindowSpan::Long),
        ("seven_day_opus", "7d Opus", WindowSpan::Other),
        ("seven_day_sonnet", "7d Sonnet", WindowSpan::Other),
        ("seven_day_oauth_apps", "7d Apps", WindowSpan::Other),
    ];
    let mut windows = Vec::new();
    for (key, label, span) in BUCKETS {
        let Some(bucket) = value.get(key).filter(|bucket| bucket.is_object()) else {
            continue;
        };
        let Some(utilization) = bucket.get("utilization").and_then(Value::as_f64) else {
            continue;
        };
        windows.push(UsageWindow {
            label: label.to_owned(),
            span,
            used_percent: clamp_percent(utilization),
            resets_at_unix: bucket
                .get("resets_at")
                .and_then(Value::as_str)
                .and_then(parse_rfc3339_unix),
        });
    }
    if windows.is_empty() {
        return Err("no usage windows present".to_owned());
    }
    let mut notes = Vec::new();
    if let Some(extra) = value.get("extra_usage").filter(|extra| extra.is_object())
        && extra.get("is_enabled").and_then(Value::as_bool) == Some(true)
    {
        match extra.get("utilization").and_then(Value::as_f64) {
            Some(utilization) => notes.push(format!(
                "Extra usage {}%",
                clamp_percent(utilization).round() as u32
            )),
            None => notes.push("Extra usage on".to_owned()),
        }
    }
    Ok(ProviderUsage {
        plan,
        source: source.to_owned(),
        windows,
        notes,
    })
}

fn parse_codex_usage(body: &str, source: &str) -> Result<ProviderUsage, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("expected a JSON object".to_owned());
    }
    let now = now_unix();
    let mut windows = Vec::new();
    let mut notes = Vec::new();
    let plan = value
        .get("plan_type")
        .and_then(Value::as_str)
        .map(pretty_plan);

    if let Some(limit) = value.get("rate_limit").filter(|limit| limit.is_object()) {
        windows.extend(codex_windows(limit, None, now));
        if limit.get("limit_reached").and_then(Value::as_bool) == Some(true) {
            notes.push("Limit reached".to_owned());
        }
    }
    if let Some(extra) = value
        .get("additional_rate_limits")
        .and_then(Value::as_array)
    {
        for entry in extra {
            let name = ["limit_name", "limit_id", "name"]
                .iter()
                .find_map(|key| entry.get(*key).and_then(Value::as_str))
                .unwrap_or("Extra");
            if let Some(limit) = entry.get("rate_limit").filter(|limit| limit.is_object()) {
                windows.extend(codex_windows(limit, Some(name), now));
            }
        }
    }
    if let Some(credits) = value.get("credits").filter(|credits| credits.is_object()) {
        if credits.get("unlimited").and_then(Value::as_bool) == Some(true) {
            notes.push("Unlimited credits".to_owned());
        } else if credits.get("has_credits").and_then(Value::as_bool) == Some(true) {
            match credits.get("balance") {
                Some(Value::Number(balance)) => notes.push(format!("Credits {balance}")),
                Some(Value::String(balance)) => notes.push(format!("Credits {balance}")),
                _ => notes.push("Credits on".to_owned()),
            }
        }
    }
    if windows.is_empty() {
        return Err("no rate-limit windows present".to_owned());
    }
    Ok(ProviderUsage {
        plan,
        source: source.to_owned(),
        windows,
        notes,
    })
}

fn codex_windows(limit: &Value, prefix: Option<&str>, now: i64) -> Vec<UsageWindow> {
    ["primary_window", "secondary_window"]
        .iter()
        .filter_map(|key| limit.get(*key).filter(|window| window.is_object()))
        .filter_map(|window| {
            let used = window.get("used_percent").and_then(Value::as_f64)?;
            let seconds = window
                .get("limit_window_seconds")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let resets_at = window
                .get("reset_at")
                .and_then(Value::as_i64)
                .filter(|value| *value > 0)
                .or_else(|| {
                    window
                        .get("reset_after_seconds")
                        .and_then(Value::as_i64)
                        .map(|after| now + after)
                });
            let (label, span) = span_from_seconds(seconds);
            Some(UsageWindow {
                label: match prefix {
                    Some(prefix) => format!("{label} {prefix}"),
                    None => label,
                },
                span: if prefix.is_some() {
                    WindowSpan::Other
                } else {
                    span
                },
                used_percent: clamp_percent(used),
                resets_at_unix: resets_at,
            })
        })
        .collect()
}

fn span_from_seconds(seconds: i64) -> (String, WindowSpan) {
    const HOUR: i64 = 3_600;
    const DAY: i64 = 24 * HOUR;
    match seconds {
        s if s <= 0 => ("?".to_owned(), WindowSpan::Other),
        s if s <= 6 * HOUR => {
            let hours = ((s + HOUR / 2) / HOUR).max(1);
            (format!("{hours}h"), WindowSpan::Short)
        }
        s if s < 5 * DAY => (format!("{}h", (s + HOUR / 2) / HOUR), WindowSpan::Other),
        s if s <= 8 * DAY => ("7d".to_owned(), WindowSpan::Long),
        s if s <= 32 * DAY => ("30d".to_owned(), WindowSpan::Long),
        s => (format!("{}d", s / DAY), WindowSpan::Long),
    }
}

fn clamp_percent(value: f64) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    value.clamp(0.0, 100.0) as f32
}

fn pretty_plan(plan: &str) -> String {
    let plan = plan.trim();
    if plan.is_empty() {
        return String::new();
    }
    plan.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// --- Time helpers ---------------------------------------------------------

pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Parse an RFC 3339 timestamp such as `2026-09-02T12:34:56.789Z` or
/// `2026-09-02T12:34:56+02:00` into Unix seconds without a date crate.
fn parse_rfc3339_unix(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..=60).contains(&second)
    {
        return None;
    }
    let mut rest = &text[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(fraction.len());
        rest = &fraction[digits..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        zone => {
            let sign = match zone.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            if zone.len() != 6 || zone.as_bytes()[3] != b':' {
                return None;
            }
            sign * (hours * 3_600 + minutes * 60)
        }
    };
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second - offset)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = (i64::from(month) + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub(crate) fn format_percent(percent: f32) -> String {
    format!("{}%", percent.round() as u32)
}

/// Time until a window resets: "2h 13m", "3d 4h", or "now".
pub(crate) fn format_reset(resets_at_unix: i64, now_unix: i64) -> String {
    let remaining = resets_at_unix - now_unix;
    if remaining <= 0 {
        return "now".to_owned();
    }
    format_span(remaining)
}

/// Age of a reading: "2m", "1h 5m", or "now".
pub(crate) fn format_age(fetched_at_unix: i64, now_unix: i64) -> String {
    let age = now_unix - fetched_at_unix;
    if age < 45 {
        "now".to_owned()
    } else {
        format_span(age)
    }
}

fn format_span(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{}m", minutes.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_usage_maps_windows_and_extra_usage() {
        let body = r#"{
            "five_hour": {"utilization": 42.5, "resets_at": "2026-09-02T15:00:00.000000+00:00"},
            "seven_day": {"utilization": 17, "resets_at": "2026-09-05T09:30:00Z"},
            "seven_day_opus": {"utilization": 3.2, "resets_at": null},
            "seven_day_sonnet": null,
            "extra_usage": {"is_enabled": true, "utilization": 12.4, "monthly_limit": 100}
        }"#;
        let usage = parse_claude_usage(body, "macOS Keychain", Some("Max".to_owned()))
            .expect("usage should parse");
        assert_eq!(usage.plan.as_deref(), Some("Max"));
        assert_eq!(usage.windows.len(), 3);
        let short = usage.short_window().expect("five-hour window");
        assert_eq!(short.label, "5h");
        assert_eq!(short.used_percent, 42.5);
        assert_eq!(short.resets_at_unix, Some(1_788_361_200));
        let long = usage.long_window().expect("weekly window");
        assert_eq!(long.used_percent, 17.0);
        assert_eq!(long.resets_at_unix, Some(1_788_600_600));
        assert_eq!(usage.windows[2].resets_at_unix, None);
        assert_eq!(usage.peak_percent(), Some(42.5));
        assert_eq!(usage.notes, ["Extra usage 12%"]);
    }

    #[test]
    fn claude_usage_rejects_bodies_without_windows() {
        assert!(parse_claude_usage("{}", "file", None).is_err());
        assert!(parse_claude_usage("[]", "file", None).is_err());
        assert!(parse_claude_usage("nope", "file", None).is_err());
    }

    #[test]
    fn codex_usage_maps_primary_secondary_and_credits() {
        let body = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {"used_percent": 61, "limit_window_seconds": 18000, "reset_after_seconds": 900, "reset_at": 1788361200},
                "secondary_window": {"used_percent": 9, "limit_window_seconds": 604800, "reset_after_seconds": 90000, "reset_at": 1788600600}
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": 12.5},
            "additional_rate_limits": [
                {"limit_name": "codex_reasoning", "rate_limit": {"primary_window": {"used_percent": 5, "limit_window_seconds": 18000, "reset_after_seconds": 10, "reset_at": 1788361200}}}
            ]
        }"#;
        let usage = parse_codex_usage(body, "~/.codex/auth.json").expect("usage should parse");
        assert_eq!(usage.plan.as_deref(), Some("Pro"));
        assert_eq!(usage.windows.len(), 3);
        let short = usage.short_window().expect("primary window");
        assert_eq!(short.label, "5h");
        assert_eq!(short.used_percent, 61.0);
        assert_eq!(short.resets_at_unix, Some(1_788_361_200));
        let long = usage.long_window().expect("secondary window");
        assert_eq!(long.label, "7d");
        assert_eq!(long.used_percent, 9.0);
        assert_eq!(usage.windows[2].label, "5h codex_reasoning");
        assert_eq!(usage.windows[2].span, WindowSpan::Other);
        assert_eq!(usage.notes, ["Credits 12.5"]);
    }

    #[test]
    fn codex_usage_falls_back_to_reset_after_seconds() {
        let body = r#"{"rate_limit": {"primary_window": {"used_percent": 100, "limit_window_seconds": 18000, "reset_after_seconds": 600}}, "credits": {"unlimited": true}}"#;
        let usage = parse_codex_usage(body, "file").expect("usage should parse");
        let window = &usage.windows[0];
        assert_eq!(window.used_percent, 100.0);
        let expected = now_unix() + 600;
        let resets = window.resets_at_unix.expect("derived reset");
        assert!((resets - expected).abs() <= 2, "{resets} vs {expected}");
        assert_eq!(usage.notes, ["Unlimited credits"]);
    }

    #[test]
    fn window_span_labels_follow_duration() {
        assert_eq!(
            span_from_seconds(18_000),
            ("5h".to_owned(), WindowSpan::Short)
        );
        assert_eq!(
            span_from_seconds(3_600),
            ("1h".to_owned(), WindowSpan::Short)
        );
        assert_eq!(
            span_from_seconds(86_400),
            ("24h".to_owned(), WindowSpan::Other)
        );
        assert_eq!(
            span_from_seconds(604_800),
            ("7d".to_owned(), WindowSpan::Long)
        );
        assert_eq!(
            span_from_seconds(2_592_000),
            ("30d".to_owned(), WindowSpan::Long)
        );
        assert_eq!(span_from_seconds(0), ("?".to_owned(), WindowSpan::Other));
    }

    #[test]
    fn claude_credentials_expose_only_what_is_needed() {
        let login = parse_claude_credentials(
            r#"{"claudeAiOauth": {"accessToken": "sk-ant-oat01-abc", "refreshToken": "r", "expiresAt": 1, "subscriptionType": "max"}}"#,
            "macOS Keychain",
        )
        .expect("login should parse");
        assert_eq!(login.plan.as_deref(), Some("Max"));
        assert_eq!(login.source, "macOS Keychain");
        assert_eq!(format!("{:?}", login.token), "Secret(…)");

        assert!(matches!(
            parse_claude_credentials(r#"{"other": 1}"#, "file"),
            Err(ProbeError::NotSignedIn(_))
        ));
        assert!(matches!(
            parse_claude_credentials("{", "file"),
            Err(ProbeError::Failed(_))
        ));
    }

    #[test]
    fn codex_credentials_distinguish_api_keys_from_chatgpt_logins() {
        let login = parse_codex_credentials(
            r#"{"tokens": {"access_token": "t", "account_id": "acct_1", "refresh_token": "r"}}"#,
            false,
        )
        .expect("login should parse");
        assert_eq!(login.account_id.as_deref(), Some("acct_1"));

        let error = parse_codex_credentials(r#"{"OPENAI_API_KEY": "sk-x"}"#, false)
            .expect_err("api keys have no plan windows");
        assert!(matches!(error, ProbeError::NotSignedIn(detail) if detail.contains("API key")));
        let error = parse_codex_credentials(r#"{}"#, false).expect_err("no login");
        assert!(
            matches!(error, ProbeError::NotSignedIn(detail) if detail.contains("No Codex login"))
        );
    }

    #[test]
    fn opencode_auth_classifies_providers() {
        let auth = parse_opencode_auth(
            r#"{
                "anthropic": {"type": "oauth", "access": "a", "refresh": "r", "expires": 1},
                "openai": {"type": "oauth", "access": "b", "refresh": "r", "expires": 1, "accountId": "acct"},
                "opencode/": {"type": "api", "key": "k"},
                "corp": {"type": "wellknown", "key": "k", "token": "t"},
                "broken": {"type": "oauth", "refresh": "r", "expires": 1},
                "weird": {"type": "something"}
            }"#,
        )
        .expect("auth should parse");
        assert_eq!(
            auth.keys().cloned().collect::<Vec<_>>(),
            ["anthropic", "corp", "openai", "opencode"]
        );
        assert!(matches!(
            auth.get("openai"),
            Some(OpenCodeAuth::OAuth { account_id: Some(id), .. }) if id == "acct"
        ));
        assert!(matches!(auth.get("opencode"), Some(OpenCodeAuth::ApiKey)));
        assert!(matches!(auth.get("corp"), Some(OpenCodeAuth::WellKnown)));
        assert!(parse_opencode_auth("[]").is_err());
    }

    #[test]
    fn rfc3339_timestamps_convert_to_unix_seconds() {
        assert_eq!(parse_rfc3339_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_unix("2026-09-02T15:00:00Z"),
            Some(1_788_361_200)
        );
        assert_eq!(
            parse_rfc3339_unix("2026-09-02T17:00:00.123456+02:00"),
            Some(1_788_361_200)
        );
        assert_eq!(
            parse_rfc3339_unix("2026-09-02T10:00:00-05:00"),
            Some(1_788_361_200)
        );
        assert_eq!(parse_rfc3339_unix("2026-13-02T10:00:00Z"), None);
        assert_eq!(parse_rfc3339_unix("garbage"), None);
    }

    #[test]
    fn curl_config_keeps_secrets_off_the_command_line() {
        let config = curl_config(
            "https://example.test/usage",
            &[("Authorization", "Bearer a\"b\\c".to_owned())],
        );
        assert!(config.contains("url = \"https://example.test/usage\""));
        assert!(config.contains("header = \"Authorization: Bearer a\\\"b\\\\c\""));
        assert!(config.contains("max-time = 20"));
        assert!(config.contains("write-out = \"\\n%{http_code}\""));
    }

    #[test]
    fn human_time_helpers_read_naturally() {
        assert_eq!(format_reset(1_000, 1_000), "now");
        assert_eq!(format_reset(1_000 + 8_040, 1_000), "2h 14m");
        assert_eq!(format_reset(1_000 + 3 * 86_400 + 4 * 3_600, 1_000), "3d 4h");
        assert_eq!(format_reset(1_030, 1_000), "1m");
        assert_eq!(format_age(1_000, 1_010), "now");
        assert_eq!(format_age(1_000, 1_000 + 120), "2m");
        assert_eq!(format_percent(42.4), "42%");
        assert_eq!(pretty_plan("self_serve_business"), "Self Serve Business");
    }

    #[test]
    fn settings_default_to_every_provider_with_a_bounded_interval() {
        let settings: ProviderUsageSettings = serde_json::from_str("{}").expect("defaults");
        assert_eq!(settings, ProviderUsageSettings::default());
        assert!(ProviderKind::ALL.iter().all(|kind| settings.enabled(*kind)));
        assert_eq!(settings.refresh_interval(), Duration::from_secs(300));

        let tight: ProviderUsageSettings =
            serde_json::from_str(r#"{"codex": false, "refresh_interval_secs": 5}"#)
                .expect("partial settings");
        assert!(!tight.enabled(ProviderKind::Codex));
        assert_eq!(
            tight.refresh_interval(),
            Duration::from_secs(MIN_REFRESH_SECS)
        );
    }

    #[test]
    fn snapshots_keep_the_last_good_reading_across_failures() {
        let mut snapshot = ProviderUsageSnapshot::initial(ProviderKind::Claude, true);
        assert_eq!(snapshot.status, ProviderUsageStatus::Loading);
        let usage = ProviderUsage {
            plan: Some("Max".to_owned()),
            source: "test".to_owned(),
            windows: vec![UsageWindow {
                label: "5-hour".to_owned(),
                span: WindowSpan::Short,
                used_percent: 40.0,
                resets_at_unix: None,
            }],
            notes: Vec::new(),
        };
        snapshot.apply(ProviderUsageStatus::Ready(usage.clone()), 10);
        assert_eq!(snapshot.fetched_at_unix, Some(10));
        snapshot.apply(
            ProviderUsageStatus::Failed {
                detail: "HTTP 500".to_owned(),
                previous: None,
            },
            20,
        );
        assert_eq!(snapshot.usage(), Some(&usage));
        assert!(matches!(
            &snapshot.status,
            ProviderUsageStatus::Failed { detail, previous: Some(_) } if detail == "HTTP 500"
        ));
        snapshot.apply(ProviderUsageStatus::NotSignedIn("gone".to_owned()), 30);
        assert_eq!(snapshot.usage(), None);
        assert_eq!(
            ProviderUsageSnapshot::initial(ProviderKind::Codex, false).status,
            ProviderUsageStatus::Disabled
        );
    }

    /// Manual diagnostic against the real logins on this machine. Prints only
    /// statuses and percentages, never tokens:
    /// `cargo test -p superplexr-desktop live_probe -- --ignored --nocapture`
    #[test]
    #[ignore = "talks to provider endpoints with the local logins"]
    fn live_probe_reports_each_provider() {
        let env = ProbeEnvironment::from_process();
        for kind in ProviderKind::ALL {
            let started = std::time::Instant::now();
            let status = probe(kind, &env);
            println!(
                "{:<9} {:>5}ms {:?}",
                kind.label(),
                started.elapsed().as_millis(),
                status
            );
        }
    }

    #[test]
    fn missing_credential_files_are_not_errors() {
        let root =
            std::env::temp_dir().join(format!("superplexr-provider-usage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("temp root");
        assert!(matches!(
            read_credential_file(&root.join("absent.json")),
            Ok(None)
        ));
        assert!(matches!(
            read_credential_file(&root),
            Err(ProbeError::Failed(_))
        ));
        let env = ProbeEnvironment {
            claude_config_dir: root.clone(),
            codex_home: root.clone(),
            opencode_data_dir: root.clone(),
            use_keychain: false,
            openai_api_key_present: false,
        };
        assert!(matches!(
            probe(ProviderKind::Claude, &env),
            ProviderUsageStatus::NotSignedIn(_)
        ));
        assert!(matches!(
            probe(ProviderKind::Codex, &env),
            ProviderUsageStatus::NotSignedIn(_)
        ));
        assert!(matches!(
            probe(ProviderKind::OpenCode, &env),
            ProviderUsageStatus::NotSignedIn(_)
        ));
        fs::write(
            root.join("auth.json"),
            r#"{"opencode": {"type": "api", "key": "k"}}"#,
        )
        .expect("fixture");
        assert!(matches!(
            probe(ProviderKind::OpenCode, &env),
            ProviderUsageStatus::NotSignedIn(detail) if detail.contains("opencode")
        ));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn foreground_process_names_detect_supported_terminal_agents() {
        assert_eq!(
            ProviderKind::from_process_name("/usr/local/bin/codex"),
            Some(ProviderKind::Codex)
        );
        assert_eq!(
            ProviderKind::from_process_name("codex-aarch64-apple-darwin"),
            Some(ProviderKind::Codex)
        );
        assert_eq!(
            ProviderKind::from_process_name("/opt/homebrew/bin/opencode"),
            Some(ProviderKind::OpenCode)
        );
        assert_eq!(
            ProviderKind::from_process_name("claude-code"),
            Some(ProviderKind::Claude)
        );
        assert_eq!(ProviderKind::from_process_name("zsh"), None);
    }
}
