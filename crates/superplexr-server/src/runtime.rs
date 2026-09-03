mod agent_channel;
mod engine_driver;
mod fault_store;
mod provider_status;
mod realized_change;
mod review_artifact;
mod run_evidence;
mod run_checkout;
mod sandbox;
mod scheduler_policy;
mod session_group_store;
mod share_store;
mod store;

use std::{
    collections::HashMap,
    ffi::OsString,
    io::{Read, Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    os::unix::net::UnixStream as StdUnixStream,
    path::{Path, PathBuf},
    process::{Command as StdCommand, Stdio},
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use fault_store::{FaultStore, repro_directory, truncate_output};
use provider_status::ProviderStatusStore;
use run_checkout::RunCheckoutStore;
use run_evidence::RunEvidenceStore;
use scheduler_policy::SchedulerPolicyStore;
use session_group_store::SessionGroupStore;
use sha2::{Digest, Sha256};
use share_store::ShareStore;
use store::{MissionStore, StoreError};
use superplexr_core::{
    ActorKind, ArtifactId, ChangeClaimKey, ChangeIntentState, Command, FinishOutcome,
    Mission, MissionId, RunDriverSnapshot, RunHarnessSnapshot, RunId, SessionId,
    VerifiedDeliveryCommand,
};
use superplexr_plugin::{
    PluginActivityState, PluginEvent, PluginPublisher, PluginRuntimeState, PluginStatusLevel,
    PluginSupervisor, PluginTerminalState,
};
use superplexr_protocol::{
    ClientRequest, ConfiguredAgentLaunchPreview, FrameDelta, MissionEvent, PROTOCOL_VERSION,
    FaultSource, ProtocolError, ProviderFactSource, Request, ResponseBody, RunActivityEvent,
    PluginRuntimeStateSummary, PluginRuntimeSummary, PluginStatusLevelSummary,
    PluginStatusSummary, RunActivitySummary, RunEvidenceSource, RuntimeDiagnostics,
    ScheduledAgentLaunch, ScheduledAgentLaunchFailure, ServerEvent, ServerResponse, ShareRole,
    ShareSummary,
    SessionGroupEvent, SessionGroupSummary, TerminalCapture, TerminalIndexEvent,
    TerminalForegroundProcess, TerminalSessionSpec, TerminalSessionStatus, TerminalSessionSummary,
    TerminalWaitCondition, default_socket_path,
    write_server_event_v3_on_stream,
    wire_v3::{AsyncWireReader, AsyncWireWriter, FrameKind, WireError, server_handshake},
};
use superplexr_runtime::{RuntimeError, SessionEvent, SessionHandle, SessionSpec};
use superplexr_terminal::{TerminalAction, TerminalError, TerminalModel};
use thiserror::Error;
use tokio::{
    io::BufReader,
    net::{
        UnixListener, UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::{Mutex, broadcast, mpsc},
};

#[derive(Debug, Parser)]
#[command(about = "Durable local runtime for superplexr missions and terminals")]
struct Args {
    #[arg(long, default_value_os_t = default_socket_path())]
    socket: PathBuf,
    #[arg(long, default_value = ".superplexr")]
    state_dir: PathBuf,
}

#[derive(Debug, Error)]
enum ServerError {
    #[error("server I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("server state contains invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error(transparent)]
    Request(#[from] RequestError),
    #[error(transparent)]
    SchedulerPolicy(#[from] scheduler_policy::PolicyError),
    #[error(transparent)]
    Share(#[from] share_store::ShareError),
    #[error(transparent)]
    Checkout(#[from] run_checkout::CheckoutError),
    #[error(transparent)]
    ProviderStatus(#[from] provider_status::ProviderStatusError),
    #[error(transparent)]
    RunEvidence(#[from] run_evidence::RunEvidenceError),
    #[error(transparent)]
    Fault(#[from] fault_store::FaultError),
    #[error(transparent)]
    SessionGroup(#[from] session_group_store::SessionGroupError),
    #[error(transparent)]
    Plugin(#[from] superplexr_plugin::PluginError),
    #[error("socket {0} already exists; another runtime may be active")]
    SocketExists(PathBuf),
}

#[derive(Debug, Error)]
enum RequestError {
    #[error("terminal runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error("terminal metadata is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("terminal session {0} does not exist")]
    TerminalNotFound(SessionId),
    #[error("terminal session {0} already exists")]
    TerminalAlreadyExists(SessionId),
    #[error("terminal session {0} is retained history and is no longer running")]
    TerminalNotRunning(SessionId),
    #[error("terminal registry lock was poisoned")]
    RegistryPoisoned,
    #[error("terminal search query exceeds 1024 bytes")]
    SearchQueryTooLong,
    #[error("terminal history offset exceeds the retained 100000-row window")]
    InvalidTerminalHistoryOffset,
    #[error("mission history limit must be between 1 and 500")]
    InvalidHistoryLimit,
    #[error("terminal history worker failed: {0}")]
    HistoryWorker(String),
    #[error("terminal wait timeout must be between 1 millisecond and 1 hour")]
    InvalidWaitTimeout,
    #[error("terminal quiet wait must be between 50 milliseconds and 1 minute")]
    InvalidQuietWait,
    #[error("terminal text wait query must contain 1 to 1024 bytes and no NUL")]
    InvalidWaitQuery,
    #[error("terminal wait timed out")]
    WaitTimeout,
    #[error("terminal is already closed and cannot satisfy this wait")]
    WaitCannotChange,
    #[error("terminal wait stream closed before satisfying the condition")]
    WaitStreamClosed,
    #[error("runtime terminal wait limit is occupied")]
    TooManyWaits,
    #[error("client {client_id} does not control terminal session {session_id}")]
    NotController {
        session_id: SessionId,
        client_id: uuid::Uuid,
    },
    #[error(
        "terminal session {session_id} control epoch is {actual}; request used stale epoch {provided:?}"
    )]
    StaleControlEpoch {
        session_id: SessionId,
        provided: Option<u64>,
        actual: u64,
    },
    #[error("control connection changed client identity")]
    ClientIdentityChanged,
    #[error("agent terminal binding must match the launch Mission and Run")]
    InvalidAgentBinding,
    #[error("running terminal session {0} must exit before it can be archived")]
    ArchiveRunningTerminal(SessionId),
    #[error(transparent)]
    Driver(#[from] engine_driver::DriverError),
    #[error("configured driver launch requires an agent-owned Run")]
    ConfiguredDriverRequiresAgent,
    #[error("writable run {0} must launch from its ready managed checkout")]
    ManagedCheckoutRequired(RunId),
    #[error("driver snapshot is invalid: {0}")]
    DriverSnapshot(String),
    #[error(transparent)]
    SchedulerPolicy(#[from] scheduler_policy::PolicyError),
    #[error(transparent)]
    Share(#[from] share_store::ShareError),
    #[error(transparent)]
    Checkout(#[from] run_checkout::CheckoutError),
    #[error(transparent)]
    RealizedChange(#[from] realized_change::RealizedChangeError),
    #[error("realized-change inspection worker failed: {0}")]
    RealizedChangeWorker(String),
    #[error(transparent)]
    ReviewArtifact(#[from] review_artifact::ReviewArtifactError),
    #[error(transparent)]
    ProviderStatus(#[from] provider_status::ProviderStatusError),
    #[error(transparent)]
    RunEvidence(#[from] run_evidence::RunEvidenceError),
    #[error(transparent)]
    Fault(#[from] fault_store::FaultError),
    #[error(transparent)]
    SessionGroup(#[from] session_group_store::SessionGroupError),
    #[error("global agent concurrency limit {0} is occupied")]
    GlobalAgentConcurrencyLimit(u16),
    #[error("Share does not authorize this operation or resource")]
    ShareDenied,
    #[error("control connection changed authorization identity")]
    ShareIdentityChanged,
    #[error("share scope references a Mission or Session that does not exist")]
    InvalidShareScope,
}

struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Clone)]
struct TerminalRecord {
    handle: Option<SessionHandle>,
    projection: Arc<RwLock<TerminalProjection>>,
}

impl TerminalRecord {
    fn live_handle(&self, session_id: SessionId) -> Result<&SessionHandle, RequestError> {
        self.handle
            .as_ref()
            .ok_or(RequestError::TerminalNotRunning(session_id))
    }
}

struct TerminalProjection {
    summary: TerminalSessionSummary,
    frame: Option<Arc<superplexr_terminal::FullFrame>>,
    final_event: Option<ServerEvent>,
}

#[derive(Clone, Copy)]
struct TerminalBinding {
    mission_id: MissionId,
    run_id: RunId,
    session_id: SessionId,
}

struct AppState {
    store: Mutex<MissionStore>,
    scheduler_policies: Mutex<SchedulerPolicyStore>,
    shares: Mutex<ShareStore>,
    run_checkouts: Mutex<RunCheckoutStore>,
    provider_status: Mutex<ProviderStatusStore>,
    run_evidence: Mutex<RunEvidenceStore>,
    faults: Mutex<FaultStore>,
    session_groups: Mutex<SessionGroupStore>,
    checkout_gate: Mutex<()>,
    agent_launch_gate: Mutex<()>,
    terminals: RwLock<HashMap<SessionId, TerminalRecord>>,
    terminal_state_dir: PathBuf,
    agent_socket_path: PathBuf,
    mission_events: broadcast::Sender<Mission>,
    activity_events: broadcast::Sender<RunActivitySummary>,
    terminal_events: broadcast::Sender<TerminalSessionSummary>,
    session_group_events: broadcast::Sender<SessionGroupEvent>,
    share_revocations: broadcast::Sender<uuid::Uuid>,
    started_at: Instant,
    open_connections: AtomicUsize,
    agent_connections: AtomicUsize,
    active_waits: AtomicUsize,
    plugins: PluginPublisher,
}

const MAX_ACTIVE_WAITS: usize = 128;

struct ActiveWaitGuard<'a>(&'a AtomicUsize);

impl<'a> ActiveWaitGuard<'a> {
    fn acquire(count: &'a AtomicUsize) -> Result<Self, RequestError> {
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < MAX_ACTIVE_WAITS).then_some(current + 1)
            })
            .map_err(|_| RequestError::TooManyWaits)?;
        Ok(Self(count))
    }
}

impl Drop for ActiveWaitGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ClientAuthority {
    Owner,
    Shared(ShareSummary),
}

impl ClientAuthority {
    fn identity(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Owner => None,
            Self::Shared(share) => Some(share.share_id),
        }
    }
}

#[derive(Clone, Copy)]
struct RequestContext {
    client_id: uuid::Uuid,
    surface_id: Option<uuid::Uuid>,
    control_epoch: Option<u64>,
    share_id: Option<uuid::Uuid>,
    request_id: uuid::Uuid,
    expected_mission_version: Option<u64>,
    agent_identity: Option<agent_channel::AgentIdentity>,
}

struct ConnectionCount<'a>(&'a AtomicUsize);

impl<'a> ConnectionCount<'a> {
    fn enter(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::Relaxed);
        Self(count)
    }
}

impl Drop for ConnectionCount<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[tokio::main]
pub async fn run_from_env() -> Result<(), String> {
    run_server(Args::parse())
        .await
        .map_err(|error| error.to_string())
}

/// Run the daemon on a dedicated Tokio runtime.
///
/// This is used by the desktop's hidden daemon process. Errors are converted
/// to text so private server implementation types do not leak into callers.
pub fn run_blocking(socket: PathBuf, state_dir: PathBuf) -> Result<(), String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to construct daemon runtime: {error}"))?
        .block_on(run_server(Args { socket, state_dir }))
        .map_err(|error| error.to_string())
}

async fn run_server(args: Args) -> Result<(), ServerError> {
    remove_stale_socket(&args.socket)?;
    if let Some(parent) = args.socket.parent() {
        let existed = parent.exists();
        tokio::fs::create_dir_all(parent).await?;
        if !existed || parent == args.state_dir {
            secure_directory(parent)?;
        }
    }
    tokio::fs::create_dir_all(&args.state_dir).await?;
    secure_directory(&args.state_dir)?;
    secure_existing_state(&args.state_dir)?;
    secure_terminal_history(&args.state_dir)?;
    let plugin_supervisor = PluginSupervisor::start(args.state_dir.join("plugins"))?;
    let plugin_publisher = plugin_supervisor.publisher();
    let agent_socket_path = args.state_dir.canonicalize()?.join("agent.sock");
    remove_stale_socket(&agent_socket_path)?;

    let recovered_terminals = recover_terminal_history(&args.state_dir)?;
    let (mission_events, _) = broadcast::channel(256);
    let (activity_events, _) = broadcast::channel(512);
    let (terminal_events, _) = broadcast::channel(256);
    let (session_group_events, _) = broadcast::channel(256);
    let (share_revocations, _) = broadcast::channel(64);
    let run_checkout_root = args.state_dir.join("run-checkouts");
    let state = Arc::new(AppState {
        store: Mutex::new(MissionStore::open(args.state_dir.join("missions")).await?),
        scheduler_policies: Mutex::new(SchedulerPolicyStore::open(
            args.state_dir.join("scheduler-policies.json"),
        )?),
        shares: Mutex::new(ShareStore::open(args.state_dir.join("shares.json"))?),
        run_checkouts: Mutex::new(RunCheckoutStore::open(
            args.state_dir.join("run-checkouts.json"),
            run_checkout_root,
        )?),
        provider_status: Mutex::new(ProviderStatusStore::open(
            args.state_dir.join("provider-facts.json"),
        )?),
        run_evidence: Mutex::new(RunEvidenceStore::open(
            args.state_dir.join("run-evidence.json"),
        )?),
        faults: Mutex::new(FaultStore::open(args.state_dir.join("faults.json"))?),
        session_groups: Mutex::new(SessionGroupStore::open(
            args.state_dir.join("session-groups.json"),
        )?),
        checkout_gate: Mutex::new(()),
        agent_launch_gate: Mutex::new(()),
        terminals: RwLock::new(recovered_terminals),
        terminal_state_dir: args.state_dir,
        agent_socket_path: agent_socket_path.clone(),
        mission_events,
        activity_events,
        terminal_events,
        session_group_events,
        share_revocations,
        started_at: Instant::now(),
        open_connections: AtomicUsize::new(0),
        agent_connections: AtomicUsize::new(0),
        active_waits: AtomicUsize::new(0),
        plugins: plugin_publisher,
    });
    reconcile_run_checkouts(&state).await;
    let _ = reconcile_recovered_agent_runs(&state).await;
    let listener = UnixListener::bind(&args.socket)?;
    std::fs::set_permissions(&args.socket, std::fs::Permissions::from_mode(0o600))?;
    let agent_listener = UnixListener::bind(&agent_socket_path)?;
    std::fs::set_permissions(&agent_socket_path, std::fs::Permissions::from_mode(0o600))?;
    let _control_socket_guard = SocketGuard(args.socket.clone());
    let _agent_socket_guard = SocketGuard(agent_socket_path.clone());
    let scheduler_task = tokio::spawn(reconcile_scheduler_policies(Arc::clone(&state)));
    let share_expiry_task = tokio::spawn(expire_shares(Arc::clone(&state)));
    let provider_expiry_task = tokio::spawn(expire_provider_facts(Arc::clone(&state)));
    println!("superplexr runtime listening at {}", args.socket.display());

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                verify_peer(&stream)?;
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(error) = handle_connection(stream, state).await {
                        eprintln!("connection failed: {error}");
                    }
                });
            }
            accepted = agent_listener.accept() => {
                let (stream, _) = accepted?;
                verify_peer(&stream)?;
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(error) = handle_agent_connection(stream, state).await {
                        eprintln!("agent connection failed: {error}");
                    }
                });
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                break;
            }
        }
    }
    scheduler_task.abort();
    share_expiry_task.abort();
    provider_expiry_task.abort();
    plugin_supervisor.shutdown();
    Ok(())
}

async fn reconcile_run_checkouts(state: &Arc<AppState>) {
    let provisioning = state
        .run_checkouts
        .lock()
        .await
        .list(None)
        .into_iter()
        .filter(|checkout| checkout.state == superplexr_protocol::RunCheckoutState::Provisioning)
        .collect::<Vec<_>>();
    for checkout in provisioning {
        let run_id = checkout.run_id;
        let result = tokio::task::spawn_blocking(move || run_checkout::provision(&checkout)).await;
        let (state_value, error) = match result {
            Ok(Ok(())) => (superplexr_protocol::RunCheckoutState::Ready, None),
            Ok(Err(error)) => (
                superplexr_protocol::RunCheckoutState::Failed,
                Some(error.to_string()),
            ),
            Err(error) => (
                superplexr_protocol::RunCheckoutState::Failed,
                Some(format!("checkout recovery worker failed: {error}")),
            ),
        };
        if let Err(error) = state
            .run_checkouts
            .lock()
            .await
            .transition(run_id, state_value, error)
        {
            eprintln!("Run checkout recovery failed for {run_id}: {error}");
        }
    }
}

async fn expire_shares(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let expired = state.shares.lock().await.expire_due();
        match expired {
            Ok(expired) => {
                for share_id in expired {
                    release_share_controllers(&state, share_id);
                    let _ = state.share_revocations.send(share_id);
                }
            }
            Err(error) => eprintln!("Share expiry failed: {error}"),
        }
    }
}

async fn expire_provider_facts(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let Ok(now) = provider_status::now_unix_micros() else {
            continue;
        };
        let expired = state.provider_status.lock().await.take_newly_expired(now);
        if expired.is_empty() {
            continue;
        }
        let missions = state.store.lock().await.all();
        let facts = state.provider_status.lock().await;
        for run_id in expired {
            let Some(mission) = missions
                .iter()
                .find(|mission| mission.runs.contains_key(&run_id))
            else {
                continue;
            };
            if let Some(activity) =
                provider_status::derive_activity(mission, run_id, facts.get(run_id), now)
            {
                publish_activity(state.as_ref(), activity);
            }
        }
    }
}

fn remove_stale_socket(path: &std::path::Path) -> Result<(), ServerError> {
    if !path.exists() {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(path)?;
    let is_socket = !metadata.file_type().is_symlink() && metadata.file_type().is_socket();
    if !is_socket || StdUnixStream::connect(path).is_ok() {
        return Err(ServerError::SocketExists(path.to_owned()));
    }
    std::fs::remove_file(path)?;
    Ok(())
}

fn secure_directory(path: &std::path::Path) -> Result<(), std::io::Error> {
    if path.canonicalize()? == std::env::current_dir()?.canonicalize()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing to use the working directory itself as private runtime state",
        ));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "runtime directory {} is not a real directory",
                path.display()
            ),
        ));
    }
    // SAFETY: `geteuid` has no preconditions and does not dereference memory.
    let current_uid = unsafe { libc::geteuid() };
    if metadata.uid() != current_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "runtime directory {} is owned by another user",
                path.display()
            ),
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

fn secure_existing_state(root: &std::path::Path) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path == root.join("run-checkouts") {
            secure_managed_checkout_root(&path)?;
        } else if path == root.join("plugins") {
            // Executable bits are meaningful plugin data. The plugin module
            // performs stricter owner, symlink, mode, path, and size checks.
        } else {
            secure_state_entry(&path)?;
        }
    }
    Ok(())
}

fn secure_managed_checkout_root(path: &std::path::Path) -> Result<(), std::io::Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid reads immutable process credentials.
    let current_uid = unsafe { libc::geteuid() };
    if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != current_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "managed Run checkout root {} is not a real owner-controlled directory",
                path.display()
            ),
        ));
    }
    // Repository content intentionally retains its Git-recorded executable bits
    // and symlinks. Only the private container directory is runtime-owned.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

fn secure_state_entry(path: &std::path::Path) -> Result<(), std::io::Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("runtime state contains symlink {}", path.display()),
        ));
    }
    if metadata.is_dir() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
        for entry in std::fs::read_dir(path)? {
            secure_state_entry(&entry?.path())?;
        }
    } else if metadata.is_file() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    } else if !metadata.file_type().is_socket() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "runtime state contains unsupported entry {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn secure_terminal_history(state_dir: &std::path::Path) -> Result<(), std::io::Error> {
    let sessions = state_dir.join("sessions");
    if !sessions.exists() {
        return Ok(());
    }
    let metadata = std::fs::symlink_metadata(&sessions)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "terminal history path is not a real directory",
        ));
    }
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700))?;
    for entry in std::fs::read_dir(&sessions)? {
        let path = entry?.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("terminal history contains symlink {}", path.display()),
            ));
        }
        if !metadata.is_dir() {
            continue;
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        let journal = path.join("output.raw");
        if journal.is_file() {
            std::fs::set_permissions(journal, std::fs::Permissions::from_mode(0o600))?;
        }
        let metadata = path.join("terminal.json");
        if metadata.is_file() {
            std::fs::set_permissions(metadata, std::fs::Permissions::from_mode(0o600))?;
        }
        let archived = path.join("archived");
        if archived.is_file() {
            std::fs::set_permissions(archived, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_peer(stream: &UnixStream) -> Result<(), std::io::Error> {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: the socket fd is valid for this call and both output pointers
    // reference initialized values for the duration of `getpeereid`.
    let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    // SAFETY: `geteuid` has no preconditions and does not dereference memory.
    if result == 0 && uid == unsafe { libc::geteuid() } {
        Ok(())
    } else if result != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "control connection belongs to another user",
        ))
    }
}

#[cfg(target_os = "linux")]
fn verify_peer(stream: &UnixStream) -> Result<(), std::io::Error> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the fd is a live Unix socket; `credentials` and `length` are
    // writable for the exact sizes passed to `getsockopt`.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            std::ptr::addr_of_mut!(credentials).cast(),
            &mut length,
        )
    };
    // SAFETY: `geteuid` has no preconditions and does not dereference memory.
    if result == 0 && credentials.uid == unsafe { libc::geteuid() } {
        Ok(())
    } else if result != 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "control connection belongs to another user",
        ))
    }
}

fn authenticate_agent_peer(
    stream: &UnixStream,
    state: &AppState,
) -> Result<agent_channel::AgentIdentity, std::io::Error> {
    let peer_pid = stream.peer_cred()?.pid().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "agent peer PID is unavailable",
        )
    })?;
    // SAFETY: getpgid reads kernel process metadata for a positive peer PID
    // supplied by the connected Unix socket's kernel credentials.
    let process_group_id = unsafe { libc::getpgid(peer_pid) };
    if process_group_id <= 0 {
        return Err(std::io::Error::last_os_error());
    }
    let terminals = state
        .terminals
        .read()
        .map_err(|_| std::io::Error::other("terminal registry lock was poisoned"))?;
    for record in terminals.values() {
        let projection = record
            .projection
            .read()
            .map_err(|_| std::io::Error::other("terminal projection lock was poisoned"))?;
        let summary = &projection.summary;
        if summary.status == TerminalSessionStatus::Running
            && summary.process_id == u32::try_from(process_group_id).ok()
            && let (Some(mission_id), Some(run_id)) = (summary.mission_id, summary.run_id)
        {
            return Ok(agent_channel::AgentIdentity {
                mission_id,
                run_id,
                session_id: summary.session_id,
                peer_pid,
                process_group_id,
            });
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "agent peer does not belong to a live runtime-owned Run process group",
    ))
}

async fn agent_identity_is_active(
    identity: agent_channel::AgentIdentity,
    state: &AppState,
) -> Result<bool, RequestError> {
    // SAFETY: the PID originated from kernel peer credentials. A missing peer
    // returns -1 and revokes the channel.
    let process_group_id = unsafe { libc::getpgid(identity.peer_pid) };
    if process_group_id != identity.process_group_id {
        return Ok(false);
    }
    let runtime_active = {
        let record = terminal_record(state, identity.session_id)?;
        let projection = record
            .projection
            .read()
            .map_err(|_| RequestError::RegistryPoisoned)?;
        projection.summary.status == TerminalSessionStatus::Running
            && projection.summary.mission_id == Some(identity.mission_id)
            && projection.summary.run_id == Some(identity.run_id)
            && projection.summary.process_id == u32::try_from(identity.process_group_id).ok()
    };
    if !runtime_active {
        return Ok(false);
    }
    let mission = state.store.lock().await.get(identity.mission_id)?;
    Ok(mission
        .runs
        .get(&identity.run_id)
        .is_some_and(|run| !run.status.is_finished()))
}

async fn handle_agent_connection(
    stream: UnixStream,
    state: Arc<AppState>,
) -> Result<(), ServerError> {
    let identity = authenticate_agent_peer(&stream, &state)?;
    let _connection_count = ConnectionCount::enter(&state.open_connections);
    let _agent_connection_count = ConnectionCount::enter(&state.agent_connections);
    let (read, write) = stream.into_split();
    let mut read = AsyncWireReader::new(BufReader::new(read));
    let mut write = AsyncWireWriter::new(write);
    accept_wire_handshake(&mut read, &mut write).await?;
    let mut connection_client = None;
    loop {
        let request: ClientRequest = match read
            .receive_json(FrameKind::Request, 0)
            .await
            .map_err(ProtocolError::from)
        {
            Ok(request) => request,
            Err(ProtocolError::Io(error) | ProtocolError::Wire(WireError::Io(error)))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if connection_client.is_some_and(|client_id| client_id != request.client_id) {
            write_frame(
                &mut write,
                &ServerResponse::error(
                    request.request_id,
                    "client_identity_changed",
                    RequestError::ClientIdentityChanged.to_string(),
                ),
            )
            .await?;
            continue;
        }
        connection_client = Some(request.client_id);
        if request.version != PROTOCOL_VERSION {
            write_frame(
                &mut write,
                &ServerResponse::error(
                    request.request_id,
                    "unsupported_protocol_version",
                    format!(
                        "client sent version {}, runtime supports {PROTOCOL_VERSION}",
                        request.version
                    ),
                ),
            )
            .await?;
            continue;
        }
        if !agent_identity_is_active(identity, &state).await? {
            write_frame(
                &mut write,
                &ServerResponse::error(
                    request.request_id,
                    "agent_authorization_expired",
                    "agent Run is no longer active",
                ),
            )
            .await?;
            continue;
        }
        if agent_channel::authorize(identity, &request.action).is_err() {
            write_frame(
                &mut write,
                &ServerResponse::error(
                    request.request_id,
                    "agent_request_denied",
                    "agent channel does not authorize this request",
                ),
            )
            .await?;
            continue;
        }
        let response = match handle_request_with_context(
            request.action,
            RequestContext {
                client_id: request.client_id,
                surface_id: None,
                control_epoch: None,
                share_id: None,
                request_id: request.request_id,
                expected_mission_version: request.expected_mission_version,
                agent_identity: Some(identity),
            },
            &state,
        )
        .await
        {
            Ok(body) => ServerResponse::success(request.request_id, body),
            Err(error) => {
                ServerResponse::error(request.request_id, "request_failed", error.to_string())
            }
        };
        write_frame(&mut write, &response).await?;
    }
}

fn authorize_request(
    authority: &ClientAuthority,
    request: &Request,
    state: &AppState,
) -> Result<(), RequestError> {
    let ClientAuthority::Shared(share) = authority else {
        return Ok(());
    };
    let read_allowed = match request {
        Request::Ping
        | Request::ShareIdentity
        | Request::Unsubscribe { .. }
        | Request::ListMissions
        | Request::SubscribeMissions => true,
        Request::SubscribeRunActivities { mission_id } => {
            mission_id.is_none_or(|mission_id| share.mission_ids.contains(&mission_id))
        }
        Request::GetMission { mission_id }
        | Request::MissionHistory { mission_id, .. }
        | Request::SchedulerPlan { mission_id, .. }
        | Request::GetRunActivity { mission_id, .. }
        | Request::ListRunActivities { mission_id }
        | Request::ListRunEvidence { mission_id, .. } => share.mission_ids.contains(mission_id),
        Request::ListTerminals { .. } | Request::SubscribeTerminals => true,
        Request::TerminalSnapshot { session_id }
        | Request::TerminalCapture { session_id }
        | Request::TerminalWait { session_id, .. }
        | Request::TerminalHistoryFrame { session_id, .. }
        | Request::TerminalSearch { session_id, .. }
        | Request::SubscribeTerminal { session_id } => {
            terminal_in_share(state, share, *session_id)?
        }
        _ => false,
    };
    let controller_allowed = share.role == ShareRole::Controller
        && match request {
            Request::TerminalKey { session_id, .. }
            | Request::TerminalPaste { session_id, .. }
            | Request::TerminalFocus { session_id, .. }
            | Request::TerminalMouse { session_id, .. }
            | Request::TerminalScroll { session_id, .. }
            | Request::TerminalSelect { session_id, .. }
            | Request::TerminalClearSelection { session_id }
            | Request::TerminalSelectionText { session_id }
            | Request::TerminalResize { session_id, .. }
            | Request::ReleaseTerminalControl { session_id } => {
                terminal_in_share(state, share, *session_id)?
            }
            Request::ClaimTerminalControl { session_id, force } => {
                !force && terminal_in_share(state, share, *session_id)?
            }
            _ => false,
        };
    if read_allowed || controller_allowed {
        Ok(())
    } else {
        Err(RequestError::ShareDenied)
    }
}

fn terminal_in_share(
    state: &AppState,
    share: &ShareSummary,
    session_id: SessionId,
) -> Result<bool, RequestError> {
    if share.session_ids.contains(&session_id) {
        return Ok(true);
    }
    let record = terminal_record(state, session_id)?;
    let projection = record
        .projection
        .read()
        .map_err(|_| RequestError::RegistryPoisoned)?;
    Ok(projection
        .summary
        .mission_id
        .is_some_and(|mission_id| share.mission_ids.contains(&mission_id)))
}

fn filter_response(
    authority: &ClientAuthority,
    response: ResponseBody,
    state: &AppState,
) -> Result<ResponseBody, RequestError> {
    let ClientAuthority::Shared(share) = authority else {
        return Ok(response);
    };
    match response {
        ResponseBody::Missions { mut missions } => {
            missions.retain(|mission| share.mission_ids.contains(&mission.id));
            Ok(ResponseBody::Missions { missions })
        }
        ResponseBody::Terminals { terminals } => Ok(ResponseBody::Terminals {
            terminals: terminals
                .into_iter()
                .filter(|terminal| {
                    terminal_in_share(state, share, terminal.session_id).unwrap_or(false)
                })
                .collect(),
        }),
        response => Ok(response),
    }
}

async fn handle_connection(stream: UnixStream, state: Arc<AppState>) -> Result<(), ServerError> {
    let _connection_count = ConnectionCount::enter(&state.open_connections);
    let (read, write) = stream.into_split();
    let mut read = AsyncWireReader::new(BufReader::new(read));
    let mut write = AsyncWireWriter::new(write);
    accept_wire_handshake(&mut read, &mut write).await?;
    let write = Arc::new(Mutex::new(write));
    let mut connection_client = None;
    let mut connection_authority = None;
    let mut next_stream_id = 1_u32;
    let mut subscriptions = HashMap::<u32, tokio::task::JoinHandle<()>>::new();
    loop {
        let request: ClientRequest = match read
            .receive_json(FrameKind::Request, 0)
            .await
            .map_err(ProtocolError::from)
        {
            Ok(request) => request,
            Err(ProtocolError::Io(error) | ProtocolError::Wire(WireError::Io(error)))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                if let Some(client_id) = connection_client {
                    release_client_controllers(&state, client_id);
                }
                for subscription in subscriptions.into_values() {
                    subscription.abort();
                }
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        if connection_client.is_some_and(|client_id| client_id != request.client_id) {
            let response = ServerResponse::error(
                request.request_id,
                "client_identity_changed",
                RequestError::ClientIdentityChanged.to_string(),
            );
            write_connection_response(&write, &response).await?;
            continue;
        }
        connection_client = Some(request.client_id);
        if request.version != PROTOCOL_VERSION {
            let response = ServerResponse::error(
                request.request_id,
                "unsupported_protocol_version",
                format!(
                    "client sent version {}, runtime supports {PROTOCOL_VERSION}",
                    request.version
                ),
            );
            write_connection_response(&write, &response).await?;
            continue;
        }

        let authority = match request.share_token.as_deref() {
            Some(token) => match state.shares.lock().await.authenticate(token) {
                Ok(share) => ClientAuthority::Shared(share),
                Err(_) => {
                    write_connection_response(
                        &write,
                        &ServerResponse::error(
                            request.request_id,
                            "share_authorization_denied",
                            "share token is invalid, expired, or revoked",
                        ),
                    )
                    .await?;
                    continue;
                }
            },
            None => ClientAuthority::Owner,
        };
        if connection_authority.is_some_and(|identity| identity != authority.identity()) {
            write_connection_response(
                &write,
                &ServerResponse::error(
                    request.request_id,
                    "share_identity_changed",
                    RequestError::ShareIdentityChanged.to_string(),
                ),
            )
            .await?;
            continue;
        }
        connection_authority = Some(authority.identity());
        if let Err(error) = authorize_request(&authority, &request.action, &state) {
            write_connection_response(
                &write,
                &ServerResponse::error(
                    request.request_id,
                    "share_request_denied",
                    error.to_string(),
                ),
            )
            .await?;
            continue;
        }
        subscriptions.retain(|_, subscription| !subscription.is_finished());
        if subscriptions.len() >= 256
            && matches!(
                &request.action,
                Request::SubscribeTerminal { .. }
                    | Request::SubscribeMissions
                    | Request::SubscribeRunActivities { .. }
                    | Request::SubscribeTerminals
                    | Request::SubscribeSessionGroups { .. }
            )
        {
            write_connection_response(
                &write,
                &ServerResponse::error(
                    request.request_id,
                    "subscription_limit",
                    "one connection may own at most 256 subscription streams",
                ),
            )
            .await?;
            continue;
        }

        let action = match request.action {
            Request::ShareIdentity => {
                let ClientAuthority::Shared(share) = &authority else {
                    write_connection_response(
                        &write,
                        &ServerResponse::error(
                            request.request_id,
                            "share_request_denied",
                            RequestError::ShareDenied.to_string(),
                        ),
                    )
                    .await?;
                    continue;
                };
                write_connection_response(
                    &write,
                    &ServerResponse::success(
                        request.request_id,
                        ResponseBody::ShareIdentity {
                            share: share.clone(),
                        },
                    ),
                )
                .await?;
                continue;
            }
            Request::SubscribeTerminal { session_id } => {
                let stream_id = next_stream_id;
                next_stream_id = next_stream_id.saturating_add(1);
                let writer = SubscriptionWriter {
                    wire: Arc::clone(&write),
                    stream_id,
                };
                let ending = writer.clone();
                let state = Arc::clone(&state);
                subscriptions.insert(
                    stream_id,
                    tokio::spawn(async move {
                        let _ = handle_subscription(
                            request.request_id,
                            session_id,
                            authority,
                            state,
                            writer,
                        )
                        .await;
                        ending.end().await;
                    }),
                );
                continue;
            }
            Request::SubscribeMissions => {
                let stream_id = next_stream_id;
                next_stream_id = next_stream_id.saturating_add(1);
                let writer = SubscriptionWriter {
                    wire: Arc::clone(&write),
                    stream_id,
                };
                let ending = writer.clone();
                let state = Arc::clone(&state);
                subscriptions.insert(
                    stream_id,
                    tokio::spawn(async move {
                        let _ = handle_mission_subscription(
                            request.request_id,
                            authority,
                            state,
                            writer,
                        )
                        .await;
                        ending.end().await;
                    }),
                );
                continue;
            }
            Request::SubscribeRunActivities { mission_id } => {
                let stream_id = next_stream_id;
                next_stream_id = next_stream_id.saturating_add(1);
                let writer = SubscriptionWriter {
                    wire: Arc::clone(&write),
                    stream_id,
                };
                let ending = writer.clone();
                let state = Arc::clone(&state);
                subscriptions.insert(
                    stream_id,
                    tokio::spawn(async move {
                        let _ = handle_activity_subscription(
                            request.request_id,
                            mission_id,
                            authority,
                            state,
                            writer,
                        )
                        .await;
                        ending.end().await;
                    }),
                );
                continue;
            }
            Request::SubscribeTerminals => {
                let stream_id = next_stream_id;
                next_stream_id = next_stream_id.saturating_add(1);
                let writer = SubscriptionWriter {
                    wire: Arc::clone(&write),
                    stream_id,
                };
                let ending = writer.clone();
                let state = Arc::clone(&state);
                subscriptions.insert(
                    stream_id,
                    tokio::spawn(async move {
                        let _ = handle_terminal_index_subscription(
                            request.request_id,
                            authority,
                            state,
                            writer,
                        )
                        .await;
                        ending.end().await;
                    }),
                );
                continue;
            }
            Request::SubscribeSessionGroups { mission_id } => {
                let stream_id = next_stream_id;
                next_stream_id = next_stream_id.saturating_add(1);
                let writer = SubscriptionWriter {
                    wire: Arc::clone(&write),
                    stream_id,
                };
                let ending = writer.clone();
                let state = Arc::clone(&state);
                subscriptions.insert(
                    stream_id,
                    tokio::spawn(async move {
                        let _ = handle_session_group_subscription(
                            request.request_id,
                            mission_id,
                            authority,
                            state,
                            writer,
                        )
                        .await;
                        ending.end().await;
                    }),
                );
                continue;
            }
            Request::Unsubscribe { stream_id } => {
                if let Some(subscription) = subscriptions.remove(&stream_id) {
                    subscription.abort();
                }
                write_connection_response(
                    &write,
                    &ServerResponse::success(
                        request.request_id,
                        ResponseBody::SubscriptionEnded { stream_id },
                    ),
                )
                .await?;
                continue;
            }
            action => action,
        };

        let response = match handle_request_with_context(
            action,
            RequestContext {
                client_id: request.client_id,
                surface_id: request.surface_id,
                control_epoch: request.control_epoch,
                share_id: authority.identity(),
                request_id: request.request_id,
                expected_mission_version: request.expected_mission_version,
                agent_identity: None,
            },
            &state,
        )
        .await
        {
            Ok(body) => ServerResponse::success(
                request.request_id,
                filter_response(&authority, body, &state)?,
            ),
            Err(error) => {
                ServerResponse::error(request.request_id, "request_failed", error.to_string())
            }
        };
        write_connection_response(&write, &response).await?;
    }
}

type ServerWireReader = AsyncWireReader<BufReader<OwnedReadHalf>>;
type ServerWireWriter = AsyncWireWriter<OwnedWriteHalf>;
type SharedServerWireWriter = Arc<Mutex<ServerWireWriter>>;

async fn write_connection_response(
    writer: &SharedServerWireWriter,
    response: &ServerResponse,
) -> Result<(), ProtocolError> {
    let mut writer = writer.lock().await;
    writer
        .send_json(FrameKind::Response, 0, response)
        .await
        .map_err(ProtocolError::from)
}

#[derive(Clone)]
struct SubscriptionWriter {
    wire: SharedServerWireWriter,
    stream_id: u32,
}

impl SubscriptionWriter {
    async fn response(&self, response: &ServerResponse) -> Result<(), ProtocolError> {
        let mut wire = self.wire.lock().await;
        wire.send_json(FrameKind::Response, 0, response)
            .await
            .map_err(ProtocolError::from)
    }

    async fn accept(&self, request_id: uuid::Uuid, body: ResponseBody) -> Result<(), ProtocolError> {
        self.response(&ServerResponse::success(request_id, body))
            .await
    }

    async fn event<T: serde::Serialize>(&self, value: &T) -> Result<(), ProtocolError> {
        let mut wire = self.wire.lock().await;
        wire.send_json(FrameKind::EventBatch, self.stream_id, value)
            .await
            .map_err(ProtocolError::from)
    }

    async fn terminal(&self, value: &ServerEvent) -> Result<(), ProtocolError> {
        let mut wire = self.wire.lock().await;
        write_server_event_v3_on_stream(&mut wire, self.stream_id, value).await
    }

    async fn end(&self) {
        let mut wire = self.wire.lock().await;
        let _ = wire
            .send_json(
                FrameKind::Close,
                self.stream_id,
                &superplexr_protocol::wire_v3::Close {
                    code: "subscription_ended".to_owned(),
                    message: "subscription stream ended".to_owned(),
                },
            )
            .await;
    }
}

trait ServerWireMessage: serde::Serialize {
    const KIND: FrameKind;
    const STREAM_ID: u32;
}

impl ServerWireMessage for ServerResponse {
    const KIND: FrameKind = FrameKind::Response;
    const STREAM_ID: u32 = 0;
}

impl ServerWireMessage for MissionEvent {
    const KIND: FrameKind = FrameKind::EventBatch;
    const STREAM_ID: u32 = 1;
}

impl ServerWireMessage for RunActivityEvent {
    const KIND: FrameKind = FrameKind::EventBatch;
    const STREAM_ID: u32 = 1;
}

impl ServerWireMessage for TerminalIndexEvent {
    const KIND: FrameKind = FrameKind::EventBatch;
    const STREAM_ID: u32 = 1;
}

impl ServerWireMessage for SessionGroupEvent {
    const KIND: FrameKind = FrameKind::EventBatch;
    const STREAM_ID: u32 = 1;
}

async fn write_frame<T>(
    writer: &mut ServerWireWriter,
    value: &T,
) -> Result<(), ProtocolError>
where
    T: ServerWireMessage,
{
    writer
        .send_json(T::KIND, T::STREAM_ID, value)
        .await
        .map_err(ProtocolError::from)
}

fn runtime_wire_id() -> uuid::Uuid {
    static ID: std::sync::OnceLock<uuid::Uuid> = std::sync::OnceLock::new();
    *ID.get_or_init(uuid::Uuid::new_v4)
}

async fn accept_wire_handshake(
    reader: &mut ServerWireReader,
    writer: &mut ServerWireWriter,
) -> Result<(), ProtocolError> {
    tokio::time::timeout(
        Duration::from_secs(5),
        server_handshake(reader, writer, runtime_wire_id()),
    )
    .await
    .map_err(|_| {
        ProtocolError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "local protocol handshake exceeded five seconds",
        ))
    })?
    .map_err(ProtocolError::from)?;
    Ok(())
}

async fn handle_terminal_index_subscription(
    request_id: uuid::Uuid,
    authority: ClientAuthority,
    state: Arc<AppState>,
    writer: SubscriptionWriter,
) -> Result<(), ServerError> {
    // Subscribe first, then snapshot. A concurrent change may be duplicated but
    // can never be lost between the two operations.
    let mut events = state.terminal_events.subscribe();
    let mut revocations = state.share_revocations.subscribe();
    writer
        .accept(
            request_id,
            ResponseBody::TerminalIndexSubscriptionAccepted {
                stream_id: writer.stream_id,
            },
        )
        .await?;
    let terminals = match filter_response(&authority, list_terminals(&state, false)?, &state)? {
        ResponseBody::Terminals { mut terminals } => {
            terminals.sort_by_key(|terminal| terminal.session_id.to_string());
            terminals
        }
        _ => unreachable!("terminal index always returns terminal summaries"),
    };
    for terminal in terminals {
        writer
            .event(&TerminalIndexEvent::TerminalChanged { terminal })
            .await?;
    }
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            revoked = revocations.recv() => {
                if share_was_revoked(&authority, revoked) { return Ok(()); }
                continue;
            }
        };
        match event {
            Ok(terminal) => {
                if let ClientAuthority::Shared(share) = &authority
                    && !terminal_in_share(&state, share, terminal.session_id)?
                {
                    continue;
                }
                writer
                    .event(&TerminalIndexEvent::TerminalChanged { terminal })
                    .await?;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let terminals =
                    match filter_response(&authority, list_terminals(&state, false)?, &state)? {
                        ResponseBody::Terminals { terminals } => terminals,
                        _ => unreachable!("terminal index always returns terminal summaries"),
                    };
                for terminal in terminals {
                    writer
                        .event(&TerminalIndexEvent::TerminalChanged { terminal })
                        .await?;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return Ok(()),
        }
    }
}

async fn handle_session_group_subscription(
    request_id: uuid::Uuid,
    mission_id: Option<MissionId>,
    authority: ClientAuthority,
    state: Arc<AppState>,
    writer: SubscriptionWriter,
) -> Result<(), ServerError> {
    // Subscribe before snapshotting. Duplicate complete replacements are safe;
    // losing a mutation between these operations would not be.
    let mut events = state.session_group_events.subscribe();
    let mut revocations = state.share_revocations.subscribe();
    writer
        .accept(
            request_id,
            ResponseBody::SessionGroupSubscriptionAccepted {
                stream_id: writer.stream_id,
            },
        )
        .await?;
    for group in state.session_groups.lock().await.list(mission_id) {
        writer
            .event(&SessionGroupEvent::GroupChanged { group })
            .await?;
    }
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            revoked = revocations.recv() => {
                if share_was_revoked(&authority, revoked) { return Ok(()); }
                continue;
            }
        };
        match event {
            Ok(SessionGroupEvent::GroupChanged { group }) => {
                if mission_id.is_none_or(|id| group.mission_id == Some(id)) {
                    writer
                        .event(&SessionGroupEvent::GroupChanged { group })
                        .await?;
                }
            }
            Ok(SessionGroupEvent::GroupDeleted { group_id }) => {
                // A removal no longer carries its former Mission scope. Sending
                // it to all subscribers is harmless and prevents stale rows.
                writer
                    .event(&SessionGroupEvent::GroupDeleted { group_id })
                    .await?;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                for group in state.session_groups.lock().await.list(mission_id) {
                    writer
                        .event(&SessionGroupEvent::GroupChanged { group })
                        .await?;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return Ok(()),
        }
    }
}

async fn handle_mission_subscription(
    request_id: uuid::Uuid,
    authority: ClientAuthority,
    state: Arc<AppState>,
    writer: SubscriptionWriter,
) -> Result<(), ServerError> {
    // Subscribe before taking the snapshot so a concurrent commit cannot be lost.
    // Duplicate versions are harmless and discarded by consumers.
    let mut events = state.mission_events.subscribe();
    let mut revocations = state.share_revocations.subscribe();
    writer
        .accept(
            request_id,
            ResponseBody::MissionSubscriptionAccepted {
                stream_id: writer.stream_id,
            },
        )
        .await?;

    let initial = state.store.lock().await.all();
    for mission in initial {
        if let ClientAuthority::Shared(share) = &authority
            && !share.mission_ids.contains(&mission.id)
        {
            continue;
        }
        writer
            .event(&MissionEvent::MissionChanged { mission })
            .await?;
    }

    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            revoked = revocations.recv() => {
                if share_was_revoked(&authority, revoked) { return Ok(()); }
                continue;
            }
        };
        match event {
            Ok(mission) => {
                if let ClientAuthority::Shared(share) = &authority
                    && !share.mission_ids.contains(&mission.id)
                {
                    continue;
                }
                writer
                    .event(&MissionEvent::MissionChanged { mission })
                    .await?;
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                let missions = state.store.lock().await.all();
                for mission in missions {
                    if let ClientAuthority::Shared(share) = &authority
                        && !share.mission_ids.contains(&mission.id)
                    {
                        continue;
                    }
                    writer
                        .event(&MissionEvent::MissionChanged { mission })
                        .await?;
                }
            }
            Err(broadcast::error::RecvError::Closed) => return Ok(()),
        }
    }
}

async fn handle_activity_subscription(
    request_id: uuid::Uuid,
    requested_mission: Option<MissionId>,
    authority: ClientAuthority,
    state: Arc<AppState>,
    writer: SubscriptionWriter,
) -> Result<(), ServerError> {
    let mut events = state.activity_events.subscribe();
    let mut revocations = state.share_revocations.subscribe();
    writer
        .accept(
            request_id,
            ResponseBody::RunActivitySubscriptionAccepted {
                stream_id: writer.stream_id,
            },
        )
        .await?;
    for activity in activity_snapshots(&state).await? {
        if activity_visible(&authority, requested_mission, &activity) {
            writer
                .event(&RunActivityEvent::ActivityChanged { activity })
                .await?;
        }
    }
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            revoked = revocations.recv() => {
                if share_was_revoked(&authority, revoked) { return Ok(()); }
                continue;
            }
        };
        match event {
            Ok(activity) => {
                if activity_visible(&authority, requested_mission, &activity) {
                    writer
                        .event(&RunActivityEvent::ActivityChanged { activity })
                        .await?;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                for activity in activity_snapshots(&state).await? {
                    if activity_visible(&authority, requested_mission, &activity) {
                        writer
                            .event(&RunActivityEvent::ActivityChanged { activity })
                            .await?;
                    }
                }
            }
            Err(broadcast::error::RecvError::Closed) => return Ok(()),
        }
    }
}

fn activity_visible(
    authority: &ClientAuthority,
    requested_mission: Option<MissionId>,
    activity: &RunActivitySummary,
) -> bool {
    requested_mission.is_none_or(|mission_id| activity.mission_id == mission_id)
        && match authority {
            ClientAuthority::Owner => true,
            ClientAuthority::Shared(share) => share.mission_ids.contains(&activity.mission_id),
        }
}

async fn activity_snapshots(
    state: &AppState,
) -> Result<Vec<RunActivitySummary>, RequestError> {
    let missions = state.store.lock().await.all();
    let facts = state.provider_status.lock().await;
    let now = provider_status::now_unix_micros()?;
    let mut activities = missions
        .iter()
        .flat_map(|mission| {
            mission.runs.keys().filter_map(|run_id| {
                provider_status::derive_activity(mission, *run_id, facts.get(*run_id), now)
            })
        })
        .collect::<Vec<_>>();
    activities.sort_by_key(|activity| (activity.mission_id, activity.run_id));
    Ok(activities)
}

async fn publish_mission(state: &AppState, mission: Mission) {
    let _ = state.mission_events.send(mission.clone());
    let facts = state.provider_status.lock().await;
    let Ok(now) = provider_status::now_unix_micros() else {
        return;
    };
    for run_id in mission.runs.keys() {
        if let Some(activity) =
            provider_status::derive_activity(&mission, *run_id, facts.get(*run_id), now)
        {
            publish_activity(state, activity);
        }
    }
}

fn publish_activity(state: &AppState, activity: RunActivitySummary) {
    let _ = state
        .plugins
        .publish(PluginEvent::RunActivityChanged {
            run_id: activity.run_id,
            state: match activity.state {
                superplexr_protocol::RunActivityState::Pending => PluginActivityState::Pending,
                superplexr_protocol::RunActivityState::Working => PluginActivityState::Working,
                superplexr_protocol::RunActivityState::Idle => PluginActivityState::Idle,
                superplexr_protocol::RunActivityState::WaitingInput => {
                    PluginActivityState::WaitingInput
                }
                superplexr_protocol::RunActivityState::WaitingApproval => {
                    PluginActivityState::WaitingApproval
                }
                superplexr_protocol::RunActivityState::Blocked => PluginActivityState::Blocked,
                superplexr_protocol::RunActivityState::Paused => PluginActivityState::Paused,
                superplexr_protocol::RunActivityState::Completed => PluginActivityState::Completed,
                superplexr_protocol::RunActivityState::Failed => PluginActivityState::Failed,
                superplexr_protocol::RunActivityState::Cancelled => PluginActivityState::Cancelled,
                superplexr_protocol::RunActivityState::Unknown => PluginActivityState::Unknown,
            },
        });
    let _ = state.activity_events.send(activity);
}

fn publish_terminal(state: &AppState, terminal: &TerminalSessionSummary) {
    let _ = state.plugins.publish(PluginEvent::TerminalChanged {
        session_id: terminal.session_id,
        state: match terminal.status {
            TerminalSessionStatus::Running => PluginTerminalState::Running,
            TerminalSessionStatus::Exited => PluginTerminalState::Exited,
            TerminalSessionStatus::Failed => PluginTerminalState::Failed,
        },
        archived: terminal.archived,
    });
    let _ = state.terminal_events.send(terminal.clone());
}

fn plugin_list(plugins: &PluginPublisher) -> ResponseBody {
    let snapshot = plugins.snapshot();
    ResponseBody::Plugins {
        plugins: snapshot
            .plugins
            .into_iter()
            .map(|plugin| PluginRuntimeSummary {
                id: plugin.id,
                name: plugin.name,
                version: plugin.version,
                state: match plugin.state {
                    PluginRuntimeState::Starting => PluginRuntimeStateSummary::Starting,
                    PluginRuntimeState::Running => PluginRuntimeStateSummary::Running,
                    PluginRuntimeState::Backoff => PluginRuntimeStateSummary::Backoff,
                    PluginRuntimeState::Failed => PluginRuntimeStateSummary::Failed,
                    PluginRuntimeState::Stopped => PluginRuntimeStateSummary::Stopped,
                },
                restart_count: plugin.restart_count,
                dropped_events: plugin.dropped_events,
                last_status: plugin.last_status.map(|status| PluginStatusSummary {
                    key: status.key,
                    level: match status.level {
                        PluginStatusLevel::Info => PluginStatusLevelSummary::Info,
                        PluginStatusLevel::Success => PluginStatusLevelSummary::Success,
                        PluginStatusLevel::Warning => PluginStatusLevelSummary::Warning,
                        PluginStatusLevel::Error => PluginStatusLevelSummary::Error,
                    },
                    text: status.text,
                }),
                last_error: plugin.last_error,
            })
            .collect(),
        dropped_events: snapshot.dropped_events,
    }
}

async fn handle_subscription(
    request_id: uuid::Uuid,
    session_id: SessionId,
    authority: ClientAuthority,
    state: Arc<AppState>,
    writer: SubscriptionWriter,
) -> Result<(), ServerError> {
    let record = match terminal_record(&state, session_id) {
        Ok(record) => record,
        Err(error) => {
            writer
                .response(&ServerResponse::error(
                    request_id,
                    "request_failed",
                    error.to_string(),
                ))
                .await?;
            return Ok(());
        }
    };

    writer
        .accept(
            request_id,
            ResponseBody::TerminalSubscriptionAccepted {
                session_id,
                stream_id: writer.stream_id,
            },
        )
        .await?;

    let (current_frame, final_event) = {
        let projection = record
            .projection
            .read()
            .map_err(|_| std::io::Error::other("terminal projection lock was poisoned"))?;
        (projection.frame.clone(), projection.final_event.clone())
    };
    let mut previous_frame = current_frame;
    if let Some(frame) = &previous_frame {
        writer
            .terminal(&ServerEvent::TerminalFrame {
                session_id,
                frame: Box::new((**frame).clone()),
            })
            .await?;
    }
    if let Some(event) = final_event {
        writer.terminal(&event).await?;
        return Ok(());
    }

    let events = match record
        .live_handle(session_id)
        .and_then(|handle| handle.subscribe().map_err(RequestError::from))
    {
        Ok(events) => events,
        Err(error) => {
            writer
                .terminal(&ServerEvent::TerminalFailed {
                    session_id,
                    message: error.to_string(),
                })
                .await?;
            return Ok(());
        }
    };
    let (event_send, mut event_receive) = mpsc::channel(64);
    let mut revocations = state.share_revocations.subscribe();
    thread::Builder::new()
        .name(format!("superplexr-subscription-{session_id}"))
        .spawn(move || {
            while let Ok(event) = events.recv() {
                let terminal = is_terminal_event(&event);
                if event_send.blocking_send(event).is_err() || terminal {
                    break;
                }
            }
        })?;

    loop {
        let event = tokio::select! {
            event = event_receive.recv() => event,
            revoked = revocations.recv() => {
                if share_was_revoked(&authority, revoked) { return Ok(()); }
                continue;
            }
        };
        let Some(event) = event else {
            return Ok(());
        };
        let terminal = is_terminal_event(&event);
        match event {
            SessionEvent::Frame(frame) => {
                if previous_frame
                    .as_ref()
                    .is_some_and(|previous| previous.sequence >= frame.sequence)
                {
                    continue;
                }
                let outbound = previous_frame
                    .as_ref()
                    .and_then(|previous| FrameDelta::between(previous, &frame))
                    .map_or_else(
                        || ServerEvent::TerminalFrame {
                            session_id,
                            frame: Box::new((*frame).clone()),
                        },
                        |delta| ServerEvent::TerminalDelta {
                            session_id,
                            delta: Box::new(delta),
                        },
                    );
                previous_frame = Some(frame);
                writer.terminal(&outbound).await?;
                continue;
            }
            SessionEvent::ForegroundProcessChanged { .. } => continue,
            event => {
                writer
                    .terminal(&protocol_event(session_id, event))
                    .await?
            }
        }
        if terminal {
            break;
        }
    }
    Ok(())
}

fn share_was_revoked(
    authority: &ClientAuthority,
    event: Result<uuid::Uuid, broadcast::error::RecvError>,
) -> bool {
    match (authority.identity(), event) {
        (Some(expected), Ok(actual)) => expected == actual,
        // Missing a revocation event must fail closed for an observer.
        (Some(_), Err(broadcast::error::RecvError::Lagged(_))) => true,
        _ => false,
    }
}

async fn handle_request_with_context(
    request: Request,
    context: RequestContext,
    state: &Arc<AppState>,
) -> Result<ResponseBody, RequestError> {
    let RequestContext {
        client_id,
        surface_id,
        control_epoch,
        share_id,
        request_id,
        expected_mission_version,
        agent_identity,
    } = context;
    match request {
        Request::Ping => Ok(ResponseBody::Pong),
        Request::RuntimeDiagnostics => runtime_diagnostics(state).await,
        Request::ListPlugins => Ok(plugin_list(&state.plugins)),
        Request::CreateMission {
            mission_id,
            intent,
            created_by,
        } => {
            let mission = state
                .store
                .lock()
                .await
                .create(mission_id, intent, created_by)
                .await?;
            publish_mission(state, mission.clone()).await;
            Ok(ResponseBody::Mission { mission })
        }
        Request::Dispatch {
            mission_id,
            mut command,
        } => {
            if let Some((events, mission)) = state
                .store
                .lock()
                .await
                .committed(mission_id, request_id)?
            {
                return Ok(ResponseBody::EventsCommitted { events, mission });
            }
            let admission = match &command {
                Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::AdmitChangeIntent { run_id, .. },
                } => Some((*run_id, None)),
                Command::VerifiedDelivery {
                    command:
                        VerifiedDeliveryCommand::PromoteContingentClaims {
                            run_id, claims, ..
                        },
                } => Some((*run_id, Some(claims.as_slice()))),
                _ => None,
            };
            let _admission_guard = if let Some((run_id, promotions)) = admission {
                let guard = state.agent_launch_gate.lock().await;
                ensure_change_intent_globally_compatible(
                    state,
                    mission_id,
                    run_id,
                    promotions,
                )
                .await?;
                Some(guard)
            } else {
                None
            };
            let mut candidate_artifacts = stamp_candidate_realized_changes(
                state,
                mission_id,
                request_id,
                &mut command,
            )
            .await?;
            verify_candidate_settlement(state, mission_id, &command).await?;
            let (events, mission) = if candidate_artifacts.is_empty() {
                state
                    .store
                    .lock()
                    .await
                    .dispatch(
                        mission_id,
                        command,
                        expected_mission_version,
                        request_id,
                        request_id,
                    )
                    .await?
            } else {
                candidate_artifacts.push(command);
                state
                    .store
                    .lock()
                    .await
                    .dispatch_batch(
                        mission_id,
                        candidate_artifacts,
                        expected_mission_version,
                        request_id,
                        request_id,
                    )
                    .await?
            };
            publish_mission(state, mission.clone()).await;
            Ok(ResponseBody::EventsCommitted { events, mission })
        }
        Request::GetMission { mission_id } => Ok(ResponseBody::Mission {
            mission: state.store.lock().await.get(mission_id)?,
        }),
        Request::MissionHistory {
            mission_id,
            before_sequence,
            limit,
        } => {
            if !(1..=500).contains(&limit) {
                return Err(RequestError::InvalidHistoryLimit);
            }
            let (entries, has_more) =
                state
                    .store
                    .lock()
                    .await
                    .history(mission_id, before_sequence, limit)?;
            Ok(ResponseBody::MissionHistory {
                mission_id,
                entries,
                has_more,
            })
        }
        Request::SchedulerPlan {
            mission_id,
            max_concurrency,
        } => {
            let mission = state.store.lock().await.get(mission_id)?;
            let plan = aged_scheduler_plan(&mission, max_concurrency)?;
            Ok(ResponseBody::SchedulerPlan { mission_id, plan })
        }
        Request::LaunchConfiguredSchedulerBatch {
            mission_id,
            max_concurrency,
            session_name_prefix,
            cwd,
            grid,
        } => {
            let global_limit = configured_global_limit(state).await;
            launch_configured_scheduler_batch(
                mission_id,
                max_concurrency,
                &session_name_prefix,
                &cwd,
                grid,
                client_id,
                surface_id,
                expected_mission_version,
                request_id,
                global_limit,
                state,
            )
            .await
        }
        Request::SetSchedulerPolicy { policy } => {
            state.store.lock().await.get(policy.mission_id)?;
            state.scheduler_policies.lock().await.set(policy.clone())?;
            Ok(ResponseBody::SchedulerPolicy { policy })
        }
        Request::ListSchedulerPolicies => Ok(ResponseBody::SchedulerPolicies {
            policies: state.scheduler_policies.lock().await.list(),
        }),
        Request::SetSchedulerSettings { settings } => {
            state
                .scheduler_policies
                .lock()
                .await
                .set_settings(settings)?;
            Ok(ResponseBody::SchedulerSettings { settings })
        }
        Request::GetSchedulerSettings => Ok(ResponseBody::SchedulerSettings {
            settings: state.scheduler_policies.lock().await.settings(),
        }),
        Request::PrepareRunCheckout {
            mission_id,
            run_id,
            repository,
            base_ref,
        } => {
            let mission = state.store.lock().await.get(mission_id)?;
            let run = mission.runs.get(&run_id).ok_or(StoreError::Domain(
                superplexr_core::DomainError::RunNotFound(run_id),
            ))?;
            if run.status != superplexr_core::RunStatus::Pending {
                return Err(
                    StoreError::Domain(superplexr_core::DomainError::RunNotPending(run_id)).into(),
                );
            }
            let _checkout_guard = state.checkout_gate.lock().await;
            let managed_root = state.run_checkouts.lock().await.managed_root().to_owned();
            let reservation = tokio::task::spawn_blocking(move || {
                run_checkout::preflight(&managed_root, mission_id, run_id, &repository, &base_ref)
            })
            .await
            .map_err(|error| RequestError::HistoryWorker(error.to_string()))??;
            let checkout = state.run_checkouts.lock().await.reserve(reservation)?;
            if checkout.state == superplexr_protocol::RunCheckoutState::Ready {
                return Ok(ResponseBody::RunCheckout { checkout });
            }
            if matches!(
                checkout.state,
                superplexr_protocol::RunCheckoutState::Retiring
                    | superplexr_protocol::RunCheckoutState::Retired
            ) {
                return Err(run_checkout::CheckoutError::NotReady.into());
            }
            state.run_checkouts.lock().await.transition(
                run_id,
                superplexr_protocol::RunCheckoutState::Provisioning,
                None,
            )?;
            let provisioning = checkout.clone();
            let result =
                tokio::task::spawn_blocking(move || run_checkout::provision(&provisioning))
                    .await
                    .map_err(|error| RequestError::HistoryWorker(error.to_string()))?;
            match result {
                Ok(()) => {
                    let checkout = state.run_checkouts.lock().await.transition(
                        run_id,
                        superplexr_protocol::RunCheckoutState::Ready,
                        None,
                    )?;
                    Ok(ResponseBody::RunCheckout { checkout })
                }
                Err(error) => {
                    let _ = state.run_checkouts.lock().await.transition(
                        run_id,
                        superplexr_protocol::RunCheckoutState::Failed,
                        Some(error.to_string()),
                    );
                    Err(error.into())
                }
            }
        }
        Request::ListRunCheckouts { mission_id } => Ok(ResponseBody::RunCheckouts {
            checkouts: state.run_checkouts.lock().await.list(mission_id),
        }),
        Request::RetireRunCheckout {
            mission_id,
            run_id,
            merged_into_ref,
        } => {
            let mission = state.store.lock().await.get(mission_id)?;
            let run = mission.runs.get(&run_id).ok_or(StoreError::Domain(
                superplexr_core::DomainError::RunNotFound(run_id),
            ))?;
            if !run.status.is_finished() {
                return Err(run_checkout::CheckoutError::RunActive.into());
            }
            let running_session = state
                .terminals
                .read()
                .map_err(|_| RequestError::RegistryPoisoned)?
                .values()
                .any(|record| {
                    record.projection.read().is_ok_and(|projection| {
                        projection.summary.run_id == Some(run_id)
                            && projection.summary.status == TerminalSessionStatus::Running
                    })
                });
            if running_session {
                return Err(run_checkout::CheckoutError::RunActive.into());
            }
            let _checkout_guard = state.checkout_gate.lock().await;
            let checkout = state
                .run_checkouts
                .lock()
                .await
                .get_ready(mission_id, run_id)?;
            let retiring = checkout.clone();
            tokio::task::spawn_blocking(move || run_checkout::retire(&retiring, &merged_into_ref))
                .await
                .map_err(|error| RequestError::HistoryWorker(error.to_string()))??;
            let checkout = state.run_checkouts.lock().await.transition(
                run_id,
                superplexr_protocol::RunCheckoutState::Retired,
                None,
            )?;
            Ok(ResponseBody::RunCheckout { checkout })
        }
        Request::ReportProviderFact {
            mission_id,
            run_id,
            fact,
        } => {
            if agent_identity.is_some_and(|identity| {
                identity.mission_id != mission_id || identity.run_id != run_id
            }) {
                return Err(RequestError::InvalidAgentBinding);
            }
            let mission = state.store.lock().await.get(mission_id)?;
            if !mission.runs.contains_key(&run_id) {
                return Err(StoreError::Domain(superplexr_core::DomainError::RunNotFound(run_id)).into());
            }
            let source = if agent_identity.is_some() {
                ProviderFactSource::AuthenticatedAgent
            } else {
                ProviderFactSource::OwnerHook
            };
            let recorded = state
                .provider_status
                .lock()
                .await
                .record(mission_id, run_id, fact, source)?;
            let activity = provider_status::derive_activity(
                &mission,
                run_id,
                Some(&recorded),
                provider_status::now_unix_micros()?,
            )
            .ok_or(StoreError::Domain(superplexr_core::DomainError::RunNotFound(run_id)))?;
            publish_activity(state, activity.clone());
            Ok(ResponseBody::ProviderFactRecorded {
                fact: recorded,
                activity,
            })
        }
        Request::GetRunActivity { mission_id, run_id } => {
            let mission = state.store.lock().await.get(mission_id)?;
            let facts = state.provider_status.lock().await;
            let activity = provider_status::derive_activity(
                &mission,
                run_id,
                facts.get(run_id),
                provider_status::now_unix_micros()?,
            )
            .ok_or(StoreError::Domain(superplexr_core::DomainError::RunNotFound(run_id)))?;
            Ok(ResponseBody::RunActivity { activity })
        }
        Request::ListRunActivities { mission_id } => {
            let mission = state.store.lock().await.get(mission_id)?;
            let facts = state.provider_status.lock().await;
            let now = provider_status::now_unix_micros()?;
            let mut run_ids = mission.runs.keys().copied().collect::<Vec<_>>();
            run_ids.sort();
            let activities = run_ids
                .into_iter()
                .filter_map(|run_id| {
                    provider_status::derive_activity(&mission, run_id, facts.get(run_id), now)
                })
                .collect();
            Ok(ResponseBody::RunActivities { activities })
        }
        Request::ReportRunEvidence {
            mission_id,
            run_id,
            evidence,
        } => {
            if agent_identity.is_some_and(|identity| {
                identity.mission_id != mission_id || identity.run_id != run_id
            }) {
                return Err(RequestError::InvalidAgentBinding);
            }
            let mission = state.store.lock().await.get(mission_id)?;
            if !mission.runs.contains_key(&run_id) {
                return Err(StoreError::Domain(superplexr_core::DomainError::RunNotFound(run_id)).into());
            }
            let source = if agent_identity.is_some() {
                RunEvidenceSource::AuthenticatedAgent
            } else {
                RunEvidenceSource::OwnerHook
            };
            let evidence = state
                .run_evidence
                .lock()
                .await
                .record(mission_id, run_id, evidence, source)?;
            Ok(ResponseBody::RunEvidenceRecorded { evidence })
        }
        Request::ReportFault { fault } => {
            // An agent may only file Faults against its own Run; the owner may
            // file any, including ones with no Mission at all.
            let source = match agent_identity {
                Some(identity) => {
                    if fault.mission_id.is_some_and(|id| id != identity.mission_id)
                        || fault.run_id.is_some_and(|id| id != identity.run_id)
                    {
                        return Err(RequestError::InvalidAgentBinding);
                    }
                    FaultSource::AuthenticatedAgent
                }
                None => FaultSource::OwnerHook,
            };
            let fault = state.faults.lock().await.report(fault, source)?;
            Ok(ResponseBody::FaultRecorded { fault })
        }
        Request::ListFaults {
            mission_id,
            session_id,
            include_closed,
        } => {
            let faults = state
                .faults
                .lock()
                .await
                .list(mission_id, session_id, include_closed);
            Ok(ResponseBody::Faults { faults })
        }
        Request::GetFault { fault_id } => {
            let fault = state.faults.lock().await.get(fault_id)?;
            Ok(ResponseBody::FaultRecorded { fault })
        }
        Request::ReproduceFault {
            fault_id,
            timeout_seconds,
        } => {
            // The replay is deliberately not held under the store lock: it runs
            // a real command and may take seconds.
            let fault = state.faults.lock().await.get(fault_id)?;
            let receipt = reproduce_fault(&fault, timeout_seconds).await;
            let fault = state.faults.lock().await.record_repro(fault_id, receipt)?;
            Ok(ResponseBody::FaultRecorded { fault })
        }
        Request::ClassifyFault {
            fault_id,
            runs,
            timeout_seconds,
            isolated,
        } => {
            // Replays run without the store lock: each one is a real command.
            let fault = state.faults.lock().await.get(fault_id)?;
            let runs = u32::from(
                runs.unwrap_or(DEFAULT_CLASSIFY_RUNS)
                    .clamp(MIN_CLASSIFY_RUNS, MAX_CLASSIFY_RUNS),
            );
            let (classification, receipt) = classify_fault(
                &fault,
                runs,
                timeout_seconds,
                isolated,
                &state.terminal_state_dir,
            )
            .await;
            let fault = state
                .faults
                .lock()
                .await
                .record_classification(fault_id, classification, receipt)?;
            Ok(ResponseBody::FaultRecorded { fault })
        }
        Request::GuardFaults {
            limit,
            timeout_seconds,
        } => {
            // Candidates are chosen under the lock, then replayed without it:
            // each one runs a real command and may take seconds.
            let candidates = state
                .faults
                .lock()
                .await
                .guard_candidates(usize::from(limit.unwrap_or(DEFAULT_GUARD_LIMIT)).min(
                    usize::from(MAX_GUARD_LIMIT),
                ));
            let mut checked = Vec::with_capacity(candidates.len());
            let mut reopened = Vec::new();
            for fault in candidates {
                let fault_id = fault.fault_id;
                let receipt = reproduce_fault(&fault, timeout_seconds).await;
                let (updated, regressed) = state
                    .faults
                    .lock()
                    .await
                    .record_guard_replay(fault_id, receipt)?;
                checked.push(fault_id);
                if regressed {
                    reopened.push(updated);
                }
            }
            Ok(ResponseBody::FaultsGuarded { checked, reopened })
        }
        Request::ResolveFault { fault_id, note } => {
            let fault = state.faults.lock().await.resolve(fault_id, note)?;
            Ok(ResponseBody::FaultRecorded { fault })
        }
        Request::DismissFault { fault_id, note } => {
            let fault = state.faults.lock().await.dismiss(fault_id, note)?;
            Ok(ResponseBody::FaultRecorded { fault })
        }
        Request::AssignFaultFix {
            fault_id,
            mission_id,
            run_id,
        } => {
            // The Run must exist before a Fault points at it, so a Fault can
            // never reference work that was never planned.
            let mission = state.store.lock().await.get(mission_id)?;
            if !mission.runs.contains_key(&run_id) {
                return Err(StoreError::Domain(
                    superplexr_core::DomainError::RunNotFound(run_id),
                )
                .into());
            }
            let fault = state.faults.lock().await.assign_fix(fault_id, run_id)?;
            Ok(ResponseBody::FaultRecorded { fault })
        }
        Request::ListRunEvidence { mission_id, run_id } => {
            let mission = state.store.lock().await.get(mission_id)?;
            if !mission.runs.contains_key(&run_id) {
                return Err(StoreError::Domain(superplexr_core::DomainError::RunNotFound(run_id)).into());
            }
            let evidence = state.run_evidence.lock().await.list(mission_id, run_id);
            Ok(ResponseBody::RunEvidence { evidence })
        }
        Request::CreateShare {
            label,
            role,
            mission_ids,
            session_ids,
            expires_in_seconds,
        } => {
            {
                let store = state.store.lock().await;
                for mission_id in &mission_ids {
                    if store.get(*mission_id).is_err() {
                        return Err(RequestError::InvalidShareScope);
                    }
                }
            }
            for session_id in &session_ids {
                if terminal_record(state, *session_id).is_err() {
                    return Err(RequestError::InvalidShareScope);
                }
            }
            let (share, token) = state.shares.lock().await.create(
                label,
                role,
                mission_ids,
                session_ids,
                expires_in_seconds,
            )?;
            Ok(ResponseBody::ShareCreated { share, token })
        }
        Request::ListShares => Ok(ResponseBody::Shares {
            shares: state.shares.lock().await.list(),
        }),
        Request::RevokeShare { share_id } => {
            let share = state.shares.lock().await.revoke(share_id)?;
            release_share_controllers(state, share_id);
            let _ = state.share_revocations.send(share_id);
            Ok(ResponseBody::ShareRevoked { share })
        }
        Request::LaunchAgentRun {
            mission_id,
            run_id,
            session_name,
            spec,
        } => {
            let snapshot = driver_snapshot("direct-command", 0, &spec, None)?;
            let global_limit = configured_global_limit(state).await;
            launch_agent_run(
                mission_id,
                run_id,
                session_name,
                spec,
                snapshot,
                client_id,
                surface_id,
                expected_mission_version,
                request_id,
                global_limit,
                state,
            )
            .await
        }
        Request::LaunchConfiguredAgentRun {
            mission_id,
            run_id,
            session_id,
            session_name,
            mut cwd,
            use_run_checkout,
            grid,
        } => {
            let mission = state.store.lock().await.get(mission_id)?;
            let run = mission.runs.get(&run_id).ok_or(StoreError::Domain(
                superplexr_core::DomainError::RunNotFound(run_id),
            ))?;
            let ActorKind::Agent { engine } = &run.actor.kind else {
                return Err(RequestError::ConfiguredDriverRequiresAgent);
            };
            if use_run_checkout
                || mission
                    .verified_delivery
                    .change_intents
                    .contains_key(&run_id)
                || mission
                    .verified_delivery
                    .delivery_run_inputs
                    .contains_key(&run_id)
            {
                cwd = state
                    .run_checkouts
                    .lock()
                    .await
                    .get_ready(mission_id, run_id)?
                    .worktree_path;
            }
            let prepared = engine_driver::prepare(
                &state.terminal_state_dir.join("engines.json"),
                engine_driver::PrepareRequest {
                    engine,
                    objective: &run.objective,
                    mission_id,
                    run_id,
                    session_id,
                    cwd,
                    grid,
                },
            )?;
            let snapshot = driver_snapshot(
                engine,
                engine_driver::CONFIG_VERSION,
                &prepared.spec,
                prepared.sandbox.as_ref(),
            )?;
            let global_limit = configured_global_limit(state).await;
            launch_agent_run(
                mission_id,
                run_id,
                session_name,
                prepared.spec,
                snapshot,
                client_id,
                surface_id,
                expected_mission_version,
                request_id,
                global_limit,
                state,
            )
            .await
        }
        Request::PreviewConfiguredAgentRun {
            mission_id,
            run_id,
            mut cwd,
            use_run_checkout,
            grid,
        } => {
            let mission = state.store.lock().await.get(mission_id)?;
            let run = mission.runs.get(&run_id).ok_or(StoreError::Domain(
                superplexr_core::DomainError::RunNotFound(run_id),
            ))?;
            let ActorKind::Agent { engine } = &run.actor.kind else {
                return Err(RequestError::ConfiguredDriverRequiresAgent);
            };
            if use_run_checkout
                || mission
                    .verified_delivery
                    .change_intents
                    .contains_key(&run_id)
                || mission
                    .verified_delivery
                    .delivery_run_inputs
                    .contains_key(&run_id)
            {
                cwd = state
                    .run_checkouts
                    .lock()
                    .await
                    .get_ready(mission_id, run_id)?
                    .worktree_path;
            }
            let prepared = engine_driver::prepare(
                &state.terminal_state_dir.join("engines.json"),
                engine_driver::PrepareRequest {
                    engine,
                    objective: &run.objective,
                    mission_id,
                    run_id,
                    session_id: SessionId::new(),
                    cwd,
                    grid,
                },
            )?;
            Ok(ResponseBody::ConfiguredAgentLaunchPreview {
                preview: ConfiguredAgentLaunchPreview {
                    mission_id,
                    run_id,
                    engine: engine.clone(),
                    program: prepared.spec.program,
                    args: prepared.spec.args,
                    cwd: prepared.spec.cwd,
                    environment_keys: prepared.spec.environment_delta.into_keys().collect(),
                    grid: prepared.spec.grid,
                    sandbox_backend: prepared
                        .sandbox
                        .as_ref()
                        .map(|sandbox| sandbox.backend.to_owned()),
                    sandbox_profile: prepared
                        .sandbox
                        .as_ref()
                        .map(|sandbox| sandbox.profile.to_owned()),
                    sandbox_network_isolated: prepared
                        .sandbox
                        .as_ref()
                        .is_some_and(|sandbox| sandbox.network_isolated),
                },
            })
        }
        Request::ListMissions => Ok(ResponseBody::Missions {
            missions: state.store.lock().await.list(),
        }),
        Request::StartTerminal { spec } => {
            if spec.mission_id.is_some() || spec.run_id.is_some() {
                return Err(RequestError::InvalidAgentBinding);
            }
            start_terminal(spec, client_id, surface_id, state)
        }
        Request::ListTerminals { include_archived } => list_terminals(state, include_archived),
        Request::CreateSessionGroup { group: spec } => {
            validate_session_group_members(state, spec.mission_id, &spec.session_ids)?;
            let group = state
                .session_groups
                .lock()
                .await
                .create(SessionGroupSummary {
                    group_id: spec.group_id,
                    mission_id: spec.mission_id,
                    name: spec.name,
                    session_ids: spec.session_ids,
                    position: spec.position,
                    pinned: spec.pinned,
                    detached: spec.detached,
                    version: 0,
                })?;
            let _ = state
                .session_group_events
                .send(SessionGroupEvent::GroupChanged {
                    group: group.clone(),
                });
            Ok(ResponseBody::SessionGroup { group })
        }
        Request::ListSessionGroups { mission_id } => Ok(ResponseBody::SessionGroups {
            groups: state.session_groups.lock().await.list(mission_id),
        }),
        Request::UpdateSessionGroup {
            group_id,
            expected_version,
            change,
        } => {
            if let superplexr_protocol::SessionGroupChange::AddSession { session_id } = &change {
                let mission_id = state
                    .session_groups
                    .lock()
                    .await
                    .list(None)
                    .into_iter()
                    .find(|group| group.group_id == group_id)
                    .ok_or(session_group_store::SessionGroupError::NotFound(group_id))?
                    .mission_id;
                validate_session_group_members(state, mission_id, &[*session_id])?;
            }
            let group = state
                .session_groups
                .lock()
                .await
                .update(group_id, expected_version, change)?;
            let _ = state
                .session_group_events
                .send(SessionGroupEvent::GroupChanged {
                    group: group.clone(),
                });
            Ok(ResponseBody::SessionGroup { group })
        }
        Request::DeleteSessionGroup {
            group_id,
            expected_version,
        } => {
            state
                .session_groups
                .lock()
                .await
                .delete(group_id, expected_version)?;
            let _ = state
                .session_group_events
                .send(SessionGroupEvent::GroupDeleted { group_id });
            Ok(ResponseBody::SessionGroupDeleted { group_id })
        }
        Request::TerminalSnapshot { session_id } => terminal_snapshot(state, session_id),
        Request::TerminalCapture { session_id } => Ok(ResponseBody::TerminalCaptured {
            capture: terminal_capture(state, session_id)?,
        }),
        Request::TerminalWait {
            session_id,
            condition,
            timeout_millis,
        } => terminal_wait(state, session_id, condition, timeout_millis).await,
        Request::TerminalHistoryFrame {
            session_id,
            viewport,
        } => {
            let requested_row = match viewport {
                superplexr_terminal::HistoryViewport::RowsBeforeBottom(row)
                | superplexr_terminal::HistoryViewport::RowFromTop(row) => row,
            };
            if requested_row > 100_000 {
                return Err(RequestError::InvalidTerminalHistoryOffset);
            }
            let record = terminal_record(state, session_id)?;
            // The bottom of history is the screen the projection already holds:
            // recovery builds it at startup, and a live actor keeps it current.
            // Replaying the journal to rediscover it costs O(journal) on a hot
            // path, and the desktop asks for exactly this frame for every
            // session it restores. For a live session the projection is also
            // the more accurate answer, because journal writes are buffered.
            if let Some(frame) = bottom_history_frame(&record, viewport)? {
                return Ok(ResponseBody::TerminalHistoryFrame {
                    session_id,
                    viewport,
                    frame: Box::new((*frame).clone()),
                });
            }
            let state_dir = state.terminal_state_dir.clone();
            let frame = tokio::task::spawn_blocking(move || {
                replay_terminal_model(&state_dir, session_id, ReplayBudget::HISTORY)?
                    .frame_at_history_viewport(viewport)
                    .map_err(RequestError::from)
            })
            .await
            .map_err(|error| RequestError::HistoryWorker(error.to_string()))??;
            Ok(ResponseBody::TerminalHistoryFrame {
                session_id,
                viewport,
                frame: Box::new(frame),
            })
        }
        Request::TerminalKey { session_id, input } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.send_key(input)?;
            Ok(accepted(session_id))
        }
        Request::TerminalPaste {
            session_id,
            bytes,
            confirmed,
        } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.paste(bytes, confirmed)?;
            Ok(accepted(session_id))
        }
        Request::TerminalFocus {
            session_id,
            focused,
        } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.focus(focused)?;
            Ok(accepted(session_id))
        }
        Request::TerminalMouse { session_id, input } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.mouse(input)?;
            Ok(accepted(session_id))
        }
        Request::TerminalScroll { session_id, scroll } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.scroll(scroll)?;
            Ok(accepted(session_id))
        }
        Request::TerminalSelect {
            session_id,
            anchor,
            head,
            rectangle,
        } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record
                .live_handle(session_id)?
                .select(anchor, head, rectangle)?;
            Ok(accepted(session_id))
        }
        Request::TerminalClearSelection { session_id } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.clear_selection()?;
            Ok(accepted(session_id))
        }
        Request::TerminalSelectionText { session_id } => {
            let record = terminal_record(state, session_id)?;
            let text = record.live_handle(session_id)?.selection_text()?;
            Ok(ResponseBody::TerminalSelectionText { session_id, text })
        }
        Request::TerminalSearch {
            session_id,
            query,
            case_sensitive,
            limit,
        } => {
            if query.len() > 1024 {
                return Err(RequestError::SearchQueryTooLong);
            }
            let record = terminal_record(state, session_id)?;
            let limit = limit.min(1_000);
            let matches = match record.live_handle(session_id).and_then(|handle| {
                handle
                    .search(query.clone(), case_sensitive, limit)
                    .map_err(RequestError::from)
            }) {
                Ok(matches) => matches,
                Err(RequestError::TerminalNotRunning(_))
                | Err(RequestError::Runtime(RuntimeError::ActorStopped)) => {
                    let state_dir = state.terminal_state_dir.clone();
                    tokio::task::spawn_blocking(move || {
                        replay_terminal_model(&state_dir, session_id, ReplayBudget::HISTORY)?
                            .search(&query, case_sensitive, limit)
                            .map_err(RequestError::from)
                    })
                    .await
                    .map_err(|error| RequestError::HistoryWorker(error.to_string()))??
                }
                Err(error) => return Err(error),
            };
            Ok(ResponseBody::TerminalSearchResults {
                session_id,
                matches,
            })
        }
        Request::TerminalResize {
            session_id,
            grid,
            cell_width_px,
            cell_height_px,
        } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record
                .live_handle(session_id)?
                .resize(grid, cell_width_px, cell_height_px)?;
            Ok(accepted(session_id))
        }
        Request::KillTerminal { session_id } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.kill()?;
            Ok(accepted(session_id))
        }
        Request::InterruptTerminal { session_id } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.interrupt()?;
            Ok(accepted(session_id))
        }
        Request::TerminateTerminal { session_id } => {
            let record = terminal_record(state, session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            record.live_handle(session_id)?.terminate()?;
            Ok(accepted(session_id))
        }
        Request::ArchiveTerminal { session_id } => {
            set_terminal_archived(state, session_id, true)?;
            Ok(accepted(session_id))
        }
        Request::RestoreTerminal { session_id } => {
            set_terminal_archived(state, session_id, false)?;
            Ok(accepted(session_id))
        }
        Request::ClaimTerminalControl { session_id, force } => {
            let record = terminal_record(state, session_id)?;
            record.live_handle(session_id)?;
            let mut projection = record
                .projection
                .write()
                .map_err(|_| RequestError::RegistryPoisoned)?;
            let same_owner = projection.summary.controller_client_id == Some(client_id)
                && projection.summary.controller_surface_id == surface_id;
            if !force && projection.summary.controller_client_id.is_some() && !same_owner {
                return Err(RequestError::NotController {
                    session_id,
                    client_id,
                });
            }
            if !same_owner {
                projection.summary.control_epoch =
                    next_control_epoch(projection.summary.control_epoch);
                projection.summary.controller_client_id = Some(client_id);
                projection.summary.controller_surface_id = surface_id;
                projection.summary.controller_share_id = share_id;
            }
            publish_terminal(state, &projection.summary);
            Ok(ResponseBody::TerminalControlChanged {
                session_id,
                control_epoch: projection.summary.control_epoch,
            })
        }
        Request::ReleaseTerminalControl { session_id } => {
            let record = terminal_record(state, session_id)?;
            record.live_handle(session_id)?;
            require_controller(&record, session_id, client_id, surface_id, control_epoch)?;
            let mut projection = record
                .projection
                .write()
                .map_err(|_| RequestError::RegistryPoisoned)?;
            projection.summary.controller_client_id = None;
            projection.summary.controller_surface_id = None;
            projection.summary.controller_share_id = None;
            projection.summary.control_epoch = next_control_epoch(projection.summary.control_epoch);
            publish_terminal(state, &projection.summary);
            Ok(ResponseBody::TerminalControlChanged {
                session_id,
                control_epoch: projection.summary.control_epoch,
            })
        }
        Request::ShareIdentity
        | Request::Unsubscribe { .. }
        | Request::SubscribeMissions
        | Request::SubscribeRunActivities { .. }
        | Request::SubscribeTerminals
        | Request::SubscribeSessionGroups { .. }
        | Request::SubscribeTerminal { .. } => {
            unreachable!("subscriptions are handled per stream")
        }
    }
}

async fn stamp_candidate_realized_changes(
    state: &Arc<AppState>,
    mission_id: MissionId,
    request_id: uuid::Uuid,
    command: &mut Command,
) -> Result<Vec<Command>, RequestError> {
    let Command::VerifiedDelivery {
        command: VerifiedDeliveryCommand::SubmitCandidate { run_id, candidate },
    } = command
    else {
        return Ok(Vec::new());
    };
    let mission = state.store.lock().await.get(mission_id)?;
    let Some(intent) = mission
        .verified_delivery
        .change_intents
        .get(run_id)
        .cloned()
    else {
        return Ok(Vec::new());
    };
    // Preserve the domain's stale-fence rejection without paying for a Git scan.
    if candidate.execution_lease_epoch != Some(intent.lease_epoch)
        || intent.state != ChangeIntentState::Admitted
    {
        return Ok(Vec::new());
    }
    let checkout = state
        .run_checkouts
        .lock()
        .await
        .get_ready(mission_id, *run_id)?;
    let candidate_run_id = *run_id;
    let state_dir = state.terminal_state_dir.clone();
    let (snapshot, review) = tokio::task::spawn_blocking(move || {
        let snapshot = realized_change::inspect(&checkout, &intent)?;
        let review = review_artifact::materialize(
            &state_dir,
            mission_id,
            candidate_run_id,
            &snapshot.manifest,
        )?;
        Ok::<_, RequestError>((snapshot, review))
    })
    .await
    .map_err(|error| RequestError::RealizedChangeWorker(error.to_string()))??;
    let revision = snapshot
        .manifest
        .snapshot_revision
        .as_ref()
        .ok_or(realized_change::RealizedChangeError::SnapshotCorrupt)?;
    candidate.revision.clone_from(revision);
    candidate
        .content_sha256
        .clone_from(&snapshot.manifest.patch_sha256);
    let patch_artifact_id = ArtifactId::from_uuid(derive_batch_uuid(
        b"candidate-patch",
        request_id,
        candidate_run_id,
    ));
    let review_artifact_id = ArtifactId::from_uuid(derive_batch_uuid(
        b"candidate-review",
        request_id,
        candidate_run_id,
    ));
    for artifact_id in [patch_artifact_id, review_artifact_id] {
        if !candidate.artifact_ids.contains(&artifact_id) {
            candidate.artifact_ids.push(artifact_id);
        }
    }
    let digest = format!("sha256:{}", snapshot.manifest.patch_sha256);
    candidate.realized_changes = Some(snapshot.manifest);
    Ok(vec![
        Command::RecordArtifact {
            artifact_id: patch_artifact_id,
            run_id: candidate_run_id,
            name: "Candidate patch".to_owned(),
            media_type: "text/x-diff".to_owned(),
            locator: snapshot.patch_locator,
            digest: Some(digest),
        },
        Command::RecordArtifact {
            artifact_id: review_artifact_id,
            run_id: candidate_run_id,
            name: "Candidate review contract".to_owned(),
            media_type: "application/vnd.superplexr.candidate-review+json".to_owned(),
            locator: review.locator,
            digest: Some(format!("sha256:{}", review.sha256)),
        },
    ])
}

async fn verify_candidate_settlement(
    state: &Arc<AppState>,
    mission_id: MissionId,
    command: &Command,
) -> Result<(), RequestError> {
    let Command::AcceptRunResult { run_id, .. } = command else {
        return Ok(());
    };
    let mission = state.store.lock().await.get(mission_id)?;
    if !mission
        .verified_delivery
        .change_intents
        .contains_key(run_id)
    {
        return Ok(());
    }
    let candidate = mission
        .verified_delivery
        .candidates
        .get(run_id)
        .ok_or(realized_change::RealizedChangeError::CandidateManifestMissing)?;
    let manifest = candidate
        .realized_changes
        .clone()
        .ok_or(realized_change::RealizedChangeError::CandidateManifestMissing)?;
    let checkout = state
        .run_checkouts
        .lock()
        .await
        .get_ready(mission_id, *run_id)?;
    tokio::task::spawn_blocking(move || {
        realized_change::verify_snapshot(&checkout, &manifest)
    })
    .await
    .map_err(|error| RequestError::RealizedChangeWorker(error.to_string()))??;
    Ok(())
}

async fn runtime_diagnostics(state: &Arc<AppState>) -> Result<ResponseBody, RequestError> {
    let plugin_snapshot = state.plugins.snapshot();
    let plugins_running = plugin_snapshot
        .plugins
        .iter()
        .filter(|plugin| plugin.state == PluginRuntimeState::Running)
        .count();
    let plugins_degraded = plugin_snapshot
        .plugins
        .iter()
        .filter(|plugin| {
            matches!(
                plugin.state,
                PluginRuntimeState::Backoff | PluginRuntimeState::Failed
            )
        })
        .count();
    let missions = state.store.lock().await.list().len();
    let (scheduler_policies, scheduler_global_limit) = {
        let scheduler = state.scheduler_policies.lock().await;
        (
            scheduler.list(),
            scheduler.settings().global_max_concurrency,
        )
    };
    let scheduler_policies_enabled = scheduler_policies
        .iter()
        .filter(|policy| policy.enabled)
        .count();
    let scheduler_global_occupied = running_agent_terminals(state)?;
    let run_checkouts = state.run_checkouts.lock().await.list(None);
    let run_checkouts_ready = run_checkouts
        .iter()
        .filter(|checkout| checkout.state == superplexr_protocol::RunCheckoutState::Ready)
        .count();
    let run_checkouts_failed = run_checkouts
        .iter()
        .filter(|checkout| checkout.state == superplexr_protocol::RunCheckoutState::Failed)
        .count();
    let terminals = state
        .terminals
        .read()
        .map_err(|_| RequestError::RegistryPoisoned)?;
    let mut running = 0;
    let mut exited = 0;
    let mut failed = 0;
    let mut controlled = 0;
    let mut archived = 0;
    for terminal in terminals.values() {
        let projection = terminal
            .projection
            .read()
            .map_err(|_| RequestError::RegistryPoisoned)?;
        match projection.summary.status {
            TerminalSessionStatus::Running => running += 1,
            TerminalSessionStatus::Exited => exited += 1,
            TerminalSessionStatus::Failed => failed += 1,
        }
        controlled += usize::from(projection.summary.controller_client_id.is_some());
        archived += usize::from(projection.summary.archived);
    }
    Ok(ResponseBody::RuntimeDiagnostics {
        diagnostics: RuntimeDiagnostics {
            runtime_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION,
            wire_profile: "v3-json-control-protobuf-terminal-zstd-multiplexed".to_owned(),
            process_id: std::process::id(),
            platform: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            uptime_seconds: state.started_at.elapsed().as_secs(),
            open_connections: state.open_connections.load(Ordering::Relaxed),
            agent_connections: state.agent_connections.load(Ordering::Relaxed),
            mission_subscribers: state.mission_events.receiver_count(),
            terminal_index_subscribers: state.terminal_events.receiver_count(),
            terminal_waits_active: state.active_waits.load(Ordering::Relaxed),
            missions,
            scheduler_policies: scheduler_policies.len(),
            scheduler_policies_enabled,
            scheduler_global_limit,
            scheduler_global_occupied,
            run_checkouts_total: run_checkouts.len(),
            run_checkouts_ready,
            run_checkouts_failed,
            terminals_total: terminals.len(),
            terminals_running: running,
            terminals_exited: exited,
            terminals_failed: failed,
            terminals_controlled: controlled,
            terminals_archived: archived,
            plugins_discovered: plugin_snapshot.plugins.len(),
            plugins_running,
            plugins_degraded,
            plugin_events_dropped: plugin_snapshot.dropped_events,
        },
    })
}

#[cfg(test)]
async fn handle_request(
    request: Request,
    client_id: uuid::Uuid,
    state: &Arc<AppState>,
) -> Result<ResponseBody, RequestError> {
    handle_request_with_context(
        request,
        RequestContext {
            client_id,
            surface_id: None,
            control_epoch: None,
            share_id: None,
            request_id: uuid::Uuid::new_v4(),
            expected_mission_version: None,
            agent_identity: None,
        },
        state,
    )
    .await
}

#[cfg(test)]
async fn handle_request_with_id(
    request: Request,
    client_id: uuid::Uuid,
    request_id: uuid::Uuid,
    state: &Arc<AppState>,
) -> Result<ResponseBody, RequestError> {
    handle_request_with_context(
        request,
        RequestContext {
            client_id,
            surface_id: None,
            control_epoch: None,
            share_id: None,
            request_id,
            expected_mission_version: None,
            agent_identity: None,
        },
        state,
    )
    .await
}

#[cfg(test)]
async fn handle_request_as_surface(
    request: Request,
    client_id: uuid::Uuid,
    surface_id: uuid::Uuid,
    control_epoch: Option<u64>,
    state: &Arc<AppState>,
) -> Result<ResponseBody, RequestError> {
    handle_request_with_context(
        request,
        RequestContext {
            client_id,
            surface_id: Some(surface_id),
            control_epoch,
            share_id: None,
            request_id: uuid::Uuid::new_v4(),
            expected_mission_version: None,
            agent_identity: None,
        },
        state,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn launch_agent_run(
    mission_id: MissionId,
    run_id: RunId,
    session_name: String,
    mut spec: TerminalSessionSpec,
    driver_snapshot: RunDriverSnapshot,
    client_id: uuid::Uuid,
    surface_id: Option<uuid::Uuid>,
    expected_mission_version: Option<u64>,
    request_id: uuid::Uuid,
    global_limit: u16,
    state: &Arc<AppState>,
) -> Result<ResponseBody, RequestError> {
    if spec.mission_id != Some(mission_id) || spec.run_id != Some(run_id) {
        return Err(RequestError::InvalidAgentBinding);
    }
    let _admission = state.agent_launch_gate.lock().await;
    spec.environment_delta.insert(
        "SUPERPLEXR_MISSION_ID".to_owned(),
        Some(mission_id.to_string()),
    );
    spec.environment_delta
        .insert("SUPERPLEXR_RUN_ID".to_owned(), Some(run_id.to_string()));
    spec.environment_delta.insert(
        "SUPERPLEXR_SESSION_ID".to_owned(),
        Some(spec.session_id.to_string()),
    );
    spec.environment_delta.insert(
        "SUPERPLEXR_AGENT_SOCKET".to_owned(),
        Some(state.agent_socket_path.to_string_lossy().into_owned()),
    );
    spec.environment_delta.insert(
        "SUPERPLEXR_AGENT_AUTH".to_owned(),
        Some("process-group-peer-credentials-v1".to_owned()),
    );
    spec.environment_delta
        .insert("SUPERPLEXR_AGENT_TOKEN_FD".to_owned(), None);
    match terminal_record(state, spec.session_id) {
        Ok(existing) => {
            let terminal = existing
                .projection
                .read()
                .map_err(|_| RequestError::RegistryPoisoned)?
                .summary
                .clone();
            if terminal.mission_id == Some(mission_id) && terminal.run_id == Some(run_id) {
                return Ok(ResponseBody::AgentRunLaunched {
                    events: Vec::new(),
                    mission: state.store.lock().await.get(mission_id)?,
                    terminal,
                });
            }
            return Err(RequestError::TerminalAlreadyExists(spec.session_id));
        }
        Err(RequestError::TerminalNotFound(_)) => {}
        Err(error) => return Err(error),
    }
    if running_agent_terminals(state)? >= global_limit {
        return Err(RequestError::GlobalAgentConcurrencyLimit(global_limit));
    }
    ensure_change_intent_globally_compatible(state, mission_id, run_id, None).await?;
    let current_mission = state.store.lock().await.get(mission_id)?;
    let run = current_mission.runs.get(&run_id).ok_or(StoreError::Domain(
        superplexr_core::DomainError::RunNotFound(run_id),
    ))?;
    let actor = run.actor.clone();
    let objective = run.objective.clone();
    let change_intent = current_mission
        .verified_delivery
        .change_intents
        .get(&run_id)
        .cloned();
    if change_intent.is_some()
        || current_mission
            .verified_delivery
            .delivery_run_inputs
            .contains_key(&run_id)
    {
        let checkout = state
            .run_checkouts
            .lock()
            .await
            .get_ready(mission_id, run_id)?;
        if spec.cwd != checkout.worktree_path
            || change_intent
                .as_ref()
                .is_some_and(|intent| intent.base_revision != checkout.base_revision)
        {
            return Err(RequestError::ManagedCheckoutRequired(run_id));
        }
    }
    if let Some(input) = current_mission
        .verified_delivery
        .delivery_run_inputs
        .get(&run_id)
    {
        let purpose = match input.purpose {
            superplexr_core::DeliveryRunPurpose::Verification => "verification",
            superplexr_core::DeliveryRunPurpose::Retry => "retry",
        };
        spec.environment_delta.insert(
            "SUPERPLEXR_DELIVERY_PURPOSE".to_owned(),
            Some(purpose.to_owned()),
        );
        spec.environment_delta.insert(
            "SUPERPLEXR_SOURCE_RUN_ID".to_owned(),
            Some(input.source_run_id.to_string()),
        );
        spec.environment_delta.insert(
            "SUPERPLEXR_CANDIDATE_REVISION".to_owned(),
            Some(input.candidate.revision.clone()),
        );
        spec.environment_delta.insert(
            "SUPERPLEXR_CANDIDATE_SHA256".to_owned(),
            Some(input.candidate.content_sha256.clone()),
        );
        if let Some(note) = &input.return_note {
            spec.environment_delta.insert(
                "SUPERPLEXR_RETURN_NOTE".to_owned(),
                Some(note.clone()),
            );
        }
    }
    let session_id = spec.session_id;
    let mut commands = Vec::with_capacity(6);
    if let Some(intent) = &change_intent
        && intent.state == ChangeIntentState::Proposed
    {
        commands.push(Command::VerifiedDelivery {
            command: VerifiedDeliveryCommand::AdmitChangeIntent {
                run_id,
                expected_version: intent.version,
                lease_epoch: 0,
            },
        });
    }
    commands.extend([
        Command::ResolveRunDriver {
            run_id,
            snapshot: driver_snapshot.clone(),
        },
        Command::VerifiedDelivery {
            command: VerifiedDeliveryCommand::RecordHarnessSnapshot {
                run_id,
                snapshot: harness_snapshot(&objective, driver_snapshot),
            },
        },
        Command::StartReadyRun { run_id },
        Command::StartSession {
            session_id,
            name: session_name,
            started_by: actor,
        },
        Command::AssignSession { session_id, run_id },
    ]);
    let (events, mission) = state
        .store
        .lock()
        .await
        .dispatch_batch(
            mission_id,
            commands,
            expected_mission_version,
            request_id,
            request_id,
        )
        .await?;
    if let Some(intent) = mission.verified_delivery.change_intents.get(&run_id) {
        spec.environment_delta.insert(
            "SUPERPLEXR_EXECUTION_LEASE_EPOCH".to_owned(),
            Some(intent.lease_epoch.to_string()),
        );
    }
    publish_mission(state, mission.clone()).await;

    match start_terminal(spec, client_id, surface_id, state) {
        Ok(ResponseBody::TerminalStarted { terminal }) => Ok(ResponseBody::AgentRunLaunched {
            events,
            mission,
            terminal,
        }),
        Ok(_) => unreachable!("terminal launch has one success response"),
        Err(error) => {
            let failure = format!("agent process launch failed: {error}");
            compensate_failed_launch(state, mission_id, run_id, session_id, &failure).await;
            Err(error)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn launch_configured_scheduler_batch(
    mission_id: MissionId,
    max_concurrency: u16,
    session_name_prefix: &str,
    cwd: &std::path::Path,
    grid: superplexr_terminal::GridSize,
    client_id: uuid::Uuid,
    surface_id: Option<uuid::Uuid>,
    mut expected_mission_version: Option<u64>,
    request_id: uuid::Uuid,
    global_limit: u16,
    state: &Arc<AppState>,
) -> Result<ResponseBody, RequestError> {
    let mission = state.store.lock().await.get(mission_id)?;
    let requested_plan = aged_scheduler_plan(&mission, max_concurrency)?;
    let global_available = global_limit.saturating_sub(running_agent_terminals(state)?);
    let effective_limit = max_concurrency.min(
        requested_plan
            .occupied_slots
            .saturating_add(global_available),
    );
    let plan = if effective_limit == max_concurrency {
        requested_plan
    } else {
        aged_scheduler_plan(&mission, effective_limit)?
    };
    let mut launched = Vec::with_capacity(plan.startable.len());
    let mut failures = Vec::new();
    for scheduled in &plan.startable {
        let ActorKind::Agent { engine } = &scheduled.actor.kind else {
            failures.push(ScheduledAgentLaunchFailure {
                run_id: scheduled.run_id,
                message: "configured driver launch requires an agent-owned Run".to_owned(),
            });
            continue;
        };
        let prepared_cwd = if mission
            .verified_delivery
            .change_intents
            .contains_key(&scheduled.run_id)
            || mission
                .verified_delivery
                .delivery_run_inputs
                .contains_key(&scheduled.run_id)
        {
            match state
                .run_checkouts
                .lock()
                .await
                .get_ready(mission_id, scheduled.run_id)
            {
                Ok(checkout) => checkout.worktree_path,
                Err(error) => {
                    failures.push(ScheduledAgentLaunchFailure {
                        run_id: scheduled.run_id,
                        message: error.to_string(),
                    });
                    continue;
                }
            }
        } else {
            cwd.to_owned()
        };
        let session_id =
            SessionId::from_uuid(derive_batch_uuid(b"session", request_id, scheduled.run_id));
        let prepared = match engine_driver::prepare(
            &state.terminal_state_dir.join("engines.json"),
            engine_driver::PrepareRequest {
                engine,
                objective: &scheduled.objective,
                mission_id,
                run_id: scheduled.run_id,
                session_id,
                cwd: prepared_cwd,
                grid,
            },
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                failures.push(ScheduledAgentLaunchFailure {
                    run_id: scheduled.run_id,
                    message: error.to_string(),
                });
                continue;
            }
        };
        let snapshot = driver_snapshot(
            engine,
            engine_driver::CONFIG_VERSION,
            &prepared.spec,
            prepared.sandbox.as_ref(),
        )?;
        let short_run_id = scheduled.run_id.to_string();
        let session_name = format!("{session_name_prefix}-{}", &short_run_id[..8]);
        let launch_request_id = derive_batch_uuid(b"launch", request_id, scheduled.run_id);
        match launch_agent_run(
            mission_id,
            scheduled.run_id,
            session_name,
            prepared.spec,
            snapshot,
            client_id,
            surface_id,
            expected_mission_version,
            launch_request_id,
            global_limit,
            state,
        )
        .await
        {
            Ok(ResponseBody::AgentRunLaunched {
                events,
                mission,
                terminal,
            }) => {
                expected_mission_version = Some(mission.version);
                launched.push(ScheduledAgentLaunch {
                    run_id: scheduled.run_id,
                    events_committed: u16::try_from(events.len()).unwrap_or(u16::MAX),
                    mission_version: mission.version,
                    terminal,
                });
            }
            Ok(_) => unreachable!("agent launch has one success response"),
            Err(error) => failures.push(ScheduledAgentLaunchFailure {
                run_id: scheduled.run_id,
                message: error.to_string(),
            }),
        }
    }
    Ok(ResponseBody::ConfiguredSchedulerBatchLaunched {
        mission_id,
        plan,
        launched,
        failures,
    })
}

async fn reconcile_scheduler_policies(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut first_policy = 0_usize;
    loop {
        interval.tick().await;
        let (mut policies, global_limit) = {
            let scheduler = state.scheduler_policies.lock().await;
            (
                scheduler
                    .list()
                    .into_iter()
                    .filter(|policy| policy.enabled)
                    .collect::<Vec<_>>(),
                scheduler.settings().global_max_concurrency,
            )
        };
        if policies.is_empty() {
            first_policy = 0;
            continue;
        }
        let rotation = first_policy % policies.len();
        policies.rotate_left(rotation);
        first_policy = (first_policy + 1) % policies.len();
        for policy in policies {
            let global_occupied = match running_agent_terminals(&state) {
                Ok(occupied) => occupied,
                Err(error) => {
                    eprintln!("scheduler could not count running agents: {error}");
                    break;
                }
            };
            let global_available = global_limit.saturating_sub(global_occupied);
            if global_available == 0 {
                break;
            }
            let mission = match state.store.lock().await.get(policy.mission_id) {
                Ok(mission) => mission,
                Err(error) => {
                    eprintln!(
                        "scheduler could not inspect Mission {}: {error}",
                        policy.mission_id
                    );
                    continue;
                }
            };
            let mission_plan = match aged_scheduler_plan(&mission, policy.max_concurrency) {
                Ok(plan) => plan,
                Err(error) => {
                    eprintln!(
                        "scheduler could not plan Mission {}: {error}",
                        policy.mission_id
                    );
                    continue;
                }
            };
            let effective_limit = policy
                .max_concurrency
                .min(mission_plan.occupied_slots.saturating_add(global_available));
            if let Err(error) = launch_configured_scheduler_batch(
                policy.mission_id,
                effective_limit,
                &policy.session_name_prefix,
                &policy.cwd,
                policy.grid,
                uuid::Uuid::new_v4(),
                None,
                None,
                uuid::Uuid::new_v4(),
                global_limit,
                &state,
            )
            .await
            {
                eprintln!(
                    "scheduler reconciliation failed for Mission {}: {error}",
                    policy.mission_id
                );
            }
        }
    }
}

async fn configured_global_limit(state: &AppState) -> u16 {
    state
        .scheduler_policies
        .lock()
        .await
        .settings()
        .global_max_concurrency
}

fn aged_scheduler_plan(
    mission: &Mission,
    max_concurrency: u16,
) -> Result<superplexr_core::SchedulerPlan, RequestError> {
    let now_unix_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .as_micros()
        .try_into()
        .map_err(|_| std::io::Error::other("system timestamp exceeds u64"))?;
    mission
        .scheduler_plan_at(max_concurrency, Some(now_unix_micros))
        .map_err(StoreError::from)
        .map_err(RequestError::from)
}

fn running_agent_terminals(state: &AppState) -> Result<u16, RequestError> {
    let terminals = state
        .terminals
        .read()
        .map_err(|_| RequestError::RegistryPoisoned)?;
    Ok(u16::try_from(
        terminals
            .values()
            .filter(|record| {
                record.projection.read().is_ok_and(|projection| {
                    projection.summary.status == TerminalSessionStatus::Running
                        && projection.summary.run_id.is_some()
                })
            })
            .count(),
    )
    .unwrap_or(u16::MAX))
}

fn derive_batch_uuid(namespace: &[u8], request_id: uuid::Uuid, run_id: RunId) -> uuid::Uuid {
    let mut hasher = Sha256::new();
    hasher.update(namespace);
    hasher.update(request_id.as_bytes());
    hasher.update(run_id.to_string().as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}

fn driver_snapshot(
    driver_id: &str,
    profile_version: u16,
    spec: &TerminalSessionSpec,
    sandbox: Option<&sandbox::SandboxAttestation>,
) -> Result<RunDriverSnapshot, RequestError> {
    let encoded = serde_json::to_vec(spec)?;
    let digest = Sha256::digest(encoded);
    let process_spec_sha256 = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let argument_count = u16::try_from(spec.args.len()).map_err(|_| {
        RequestError::DriverSnapshot("process specification exceeds 65535 arguments".to_owned())
    })?;
    Ok(RunDriverSnapshot {
        driver_id: driver_id.to_owned(),
        profile_version,
        process_spec_sha256,
        argument_count,
        environment_keys: spec.environment_delta.keys().cloned().collect(),
        sandbox_backend: sandbox.map(|sandbox| sandbox.backend.to_owned()),
        sandbox_profile: sandbox.map(|sandbox| sandbox.profile.to_owned()),
        sandbox_network_isolated: sandbox.is_some_and(|sandbox| sandbox.network_isolated),
    })
}

fn harness_snapshot(objective: &str, driver: RunDriverSnapshot) -> RunHarnessSnapshot {
    let objective_sha256 = sha256_hex(objective.as_bytes());
    let context_sha256 = sha256_hex(
        format!("superplexr-context-v1\0objective\0{objective}").as_bytes(),
    );
    RunHarnessSnapshot {
        schema_version: 1,
        objective_sha256,
        driver,
        tools_sha256: None,
        skills_sha256: None,
        context_sha256,
        evaluator_sha256: None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn ensure_change_intent_globally_compatible(
    state: &AppState,
    mission_id: MissionId,
    run_id: RunId,
    promotions: Option<&[ChangeClaimKey]>,
) -> Result<(), RequestError> {
    let store = state.store.lock().await;
    let mission = store.get(mission_id)?;
    let Some(intent) = mission.verified_delivery.change_intents.get(&run_id) else {
        return Ok(());
    };
    if intent.state == ChangeIntentState::Released {
        return Err(StoreError::Domain(
            superplexr_core::DomainError::ChangeIntentNotAdmitted(run_id),
        )
        .into());
    }
    let mut prospective = intent.clone();
    if let Some(promotions) = promotions {
        for claim in &mut prospective.claims {
            if promotions.iter().any(|promotion| {
                promotion.path == claim.path && promotion.operation == claim.operation
            }) {
                claim.scope = superplexr_core::ChangeScope::Committed;
            }
        }
    }
    for other_mission in store.all() {
        for other in other_mission.verified_delivery.change_intents.values() {
            if other.state == ChangeIntentState::Admitted && prospective.conflicts_with(other) {
                return Err(StoreError::Domain(
                    superplexr_core::DomainError::ChangeIntentConflict {
                        run_id,
                        conflicting_run_id: other.run_id,
                    },
                )
                .into());
            }
        }
    }
    Ok(())
}

async fn compensate_failed_launch(
    state: &Arc<AppState>,
    mission_id: MissionId,
    run_id: RunId,
    session_id: SessionId,
    failure: &str,
) {
    let key = uuid::Uuid::new_v4();
    let result = state
        .store
        .lock()
        .await
        .dispatch_batch(
            mission_id,
            vec![
                Command::FinishSession {
                    session_id,
                    exit_code: None,
                },
                Command::FinishRun {
                    run_id,
                    outcome: FinishOutcome::Failed,
                    summary: failure.to_owned(),
                },
            ],
            None,
            key,
            key,
        )
        .await;
    match result {
        Ok((_, mission)) => {
            publish_mission(state, mission).await;
        }
        Err(compensation_error) => {
            eprintln!("agent launch compensation failed: {compensation_error}");
        }
    }
}

fn start_terminal(
    mut spec: TerminalSessionSpec,
    client_id: uuid::Uuid,
    surface_id: Option<uuid::Uuid>,
    state: &Arc<AppState>,
) -> Result<ResponseBody, RequestError> {
    // Shell integration keys off this: it stays inert in terminals superplexr
    // does not own, so a person's shell behaves normally elsewhere.
    spec.environment_delta.insert(
        "SUPERPLEXR_SESSION".to_owned(),
        Some(spec.session_id.to_string()),
    );
    {
        let terminals = state
            .terminals
            .read()
            .map_err(|_| RequestError::RegistryPoisoned)?;
        if terminals.contains_key(&spec.session_id) {
            return Err(RequestError::TerminalAlreadyExists(spec.session_id));
        }
    }
    let binding = match (spec.mission_id, spec.run_id) {
        (Some(mission_id), Some(run_id)) => Some(TerminalBinding {
            mission_id,
            run_id,
            session_id: spec.session_id,
        }),
        (None, None) => None,
        _ => return Err(RequestError::InvalidAgentBinding),
    };
    persist_terminal_spec(&spec, &state.terminal_state_dir)?;
    let session_id = spec.session_id;
    let spawn_cwd = spec.cwd.clone();
    let (handle, events) = SessionHandle::spawn_subscribed(
        SessionSpec {
            id: session_id,
            program: spec.program,
            args: spec.args,
            cwd: spec.cwd,
            environment_delta: spec.environment_delta,
            grid: spec.grid,
        },
        &state.terminal_state_dir,
    )?;
    let frame = handle.snapshot()?;
    let summary = TerminalSessionSummary {
        session_id,
        mission_id: spec.mission_id,
        run_id: spec.run_id,
        process_id: handle.process_id(),
        foreground_process: None,
        tty_name: handle.tty_name().map(PathBuf::from),
        status: TerminalSessionStatus::Running,
        archived: false,
        latest_sequence: frame.sequence,
        controller_client_id: Some(client_id),
        controller_surface_id: surface_id,
        controller_share_id: None,
        control_epoch: 1,
        cwd: Some(spawn_cwd),
    };
    let projection = Arc::new(RwLock::new(TerminalProjection {
        summary: summary.clone(),
        frame: Some(frame),
        final_event: None,
    }));
    observe_terminal(
        session_id,
        Arc::clone(&projection),
        events,
        binding,
        Arc::clone(state),
    )?;
    state
        .terminals
        .write()
        .map_err(|_| RequestError::RegistryPoisoned)?
        .insert(
            session_id,
            TerminalRecord {
                handle: Some(handle),
                projection,
            },
        );
    publish_terminal(state, &summary);
    Ok(ResponseBody::TerminalStarted { terminal: summary })
}

/// Journal suffix replayed when rebuilding a dead terminal's last screen at
/// daemon startup.
///
/// This used to be 64 MiB, which assumed replay was fast. It is not: escape-dense
/// output (a repainting TUI) parses at roughly a third of a megabyte per second,
/// so 64 MiB is about three minutes *per session*. Recovery runs before the daemon
/// serves anything, so a handful of such sessions made the daemon — and every
/// client waiting on it — unresponsive for minutes.
///
/// A screen only needs a short suffix, because applications repaint.
const RECOVERY_REPLAY_BYTES: u64 = 1024 * 1024;

/// Journal suffix replayed for an explicit scrollback request.
///
/// Larger than recovery: the person asked for history and is waiting on that one
/// answer, off the async path, rather than on daemon startup.
const HISTORY_REPLAY_BYTES: u64 = 8 * 1024 * 1024;

/// How much work one journal replay may spend.
///
/// Both limits are needed. A byte cap alone cannot bound replay time, because
/// cost per byte varies by two orders of magnitude with escape density; a
/// deadline alone would make the recovered screen depend on machine load.
#[derive(Clone, Copy, Debug)]
struct ReplayBudget {
    bytes: u64,
    deadline: Duration,
}

impl ReplayBudget {
    /// Rebuilding a dead terminal's last screen during daemon startup.
    const RECOVERY: Self = Self {
        bytes: RECOVERY_REPLAY_BYTES,
        deadline: Duration::from_secs(1),
    };

    /// Answering an explicit scrollback or search request.
    const HISTORY: Self = Self {
        bytes: HISTORY_REPLAY_BYTES,
        deadline: Duration::from_secs(10),
    };
}

/// Scrollback requests get a larger window than startup recovery: a person is
/// waiting on that one answer, rather than on the daemon becoming able to serve.
const _: () = {
    assert!(ReplayBudget::HISTORY.bytes > ReplayBudget::RECOVERY.bytes);
    assert!(HISTORY_REPLAY_BYTES <= superplexr_runtime::JournalLimits::DEFAULT.retained_bytes());
};

fn replay_terminal_model(
    state_dir: &std::path::Path,
    session_id: SessionId,
    budget: ReplayBudget,
) -> Result<TerminalModel, RequestError> {
    let directory = state_dir.join("sessions").join(session_id.to_string());
    let metadata_path = directory.join("terminal.json");
    let journal_path = directory.join("output.raw");
    for path in [&metadata_path, &journal_path] {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(RequestError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("terminal history contains symlink {}", path.display()),
            )));
        }
    }
    let spec: TerminalSessionSpec = serde_json::from_slice(&std::fs::read(&metadata_path)?)?;
    if spec.session_id != session_id {
        return Err(RequestError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "terminal history metadata ID does not match its directory",
        )));
    }
    let mut journal = std::fs::File::open(journal_path)?;
    let length = journal.metadata()?.len();
    if length > budget.bytes {
        journal.seek(SeekFrom::Start(length - budget.bytes))?;
    }
    let mut model = TerminalModel::new(spec.grid)?;
    let mut buffer = [0_u8; 64 * 1024];
    let started = Instant::now();
    loop {
        let read = journal.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        model.advance(TerminalAction::Output(&buffer[..read]))?;
        if started.elapsed() >= budget.deadline {
            // Stop with the screen as of here. Stale, but real, and the caller
            // is either daemon startup or a person waiting on one answer.
            break;
        }
    }
    Ok(model)
}

fn persist_terminal_spec(
    spec: &TerminalSessionSpec,
    state_dir: &std::path::Path,
) -> Result<(), RequestError> {
    let directory = state_dir.join("sessions").join(spec.session_id.to_string());
    std::fs::create_dir_all(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let path = directory.join("terminal.json");
    let temporary = directory.join("terminal.json.tmp");
    let bytes = serde_json::to_vec_pretty(spec)?;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(temporary, &path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn recover_terminal_history(
    state_dir: &std::path::Path,
) -> Result<HashMap<SessionId, TerminalRecord>, ServerError> {
    let root = state_dir.join("sessions");
    if !root.is_dir() {
        return Ok(HashMap::new());
    }
    let mut records = HashMap::new();
    for entry in std::fs::read_dir(root)? {
        let directory = entry?.path();
        if !directory.is_dir() {
            continue;
        }
        let metadata_path = directory.join("terminal.json");
        let journal_path = directory.join("output.raw");
        let archived_path = directory.join("archived");
        if !metadata_path.is_file() || !journal_path.is_file() {
            continue;
        }
        if std::fs::symlink_metadata(&metadata_path)?
            .file_type()
            .is_symlink()
            || std::fs::symlink_metadata(&journal_path)?
                .file_type()
                .is_symlink()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "terminal history contains a symlink in {}",
                    directory.display()
                ),
            )
            .into());
        }
        if archived_path.exists()
            && std::fs::symlink_metadata(&archived_path)?
                .file_type()
                .is_symlink()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "terminal archive marker is a symlink in {}",
                    directory.display()
                ),
            )
            .into());
        }
        let spec: TerminalSessionSpec = serde_json::from_slice(&std::fs::read(&metadata_path)?)?;
        let mut model = replay_terminal_model(state_dir, spec.session_id, ReplayBudget::RECOVERY)?;
        let frame = model.frame()?;
        let summary = TerminalSessionSummary {
            session_id: spec.session_id,
            mission_id: spec.mission_id,
            run_id: spec.run_id,
            process_id: None,
            foreground_process: None,
            tty_name: None,
            status: TerminalSessionStatus::Exited,
            archived: archived_path.is_file(),
            latest_sequence: frame.sequence,
            controller_client_id: None,
            controller_surface_id: None,
            controller_share_id: None,
            control_epoch: 0,
            cwd: Some(spec.cwd.clone()),
        };
        records.insert(
            spec.session_id,
            TerminalRecord {
                handle: None,
                projection: Arc::new(RwLock::new(TerminalProjection {
                    summary,
                    frame: Some(Arc::new(frame)),
                    final_event: Some(ServerEvent::TerminalFailed {
                        session_id: spec.session_id,
                        message:
                            "PTY ended when its previous runtime stopped; recovered journal history"
                                .to_owned(),
                    }),
                })),
            },
        );
    }
    Ok(records)
}

fn require_controller(
    record: &TerminalRecord,
    session_id: SessionId,
    client_id: uuid::Uuid,
    surface_id: Option<uuid::Uuid>,
    control_epoch: Option<u64>,
) -> Result<(), RequestError> {
    let projection = record
        .projection
        .read()
        .map_err(|_| RequestError::RegistryPoisoned)?;
    if projection.summary.controller_client_id != Some(client_id)
        || projection.summary.controller_surface_id != surface_id
    {
        return Err(RequestError::NotController {
            session_id,
            client_id,
        });
    }
    if surface_id.is_some() && control_epoch != Some(projection.summary.control_epoch) {
        return Err(RequestError::StaleControlEpoch {
            session_id,
            provided: control_epoch,
            actual: projection.summary.control_epoch,
        });
    }
    Ok(())
}

const fn next_control_epoch(epoch: u64) -> u64 {
    match epoch.saturating_add(1) {
        0 => 1,
        next => next,
    }
}

fn release_client_controllers(state: &AppState, client_id: uuid::Uuid) {
    let Ok(terminals) = state.terminals.read() else {
        return;
    };
    for record in terminals.values() {
        if let Ok(mut projection) = record.projection.write()
            && projection.summary.controller_client_id == Some(client_id)
        {
            projection.summary.controller_client_id = None;
            projection.summary.controller_surface_id = None;
            projection.summary.controller_share_id = None;
            projection.summary.control_epoch = next_control_epoch(projection.summary.control_epoch);
            publish_terminal(state, &projection.summary);
        }
    }
}

fn release_share_controllers(state: &AppState, share_id: uuid::Uuid) {
    let Ok(terminals) = state.terminals.read() else {
        return;
    };
    for record in terminals.values() {
        if let Ok(mut projection) = record.projection.write()
            && projection.summary.controller_share_id == Some(share_id)
        {
            projection.summary.controller_client_id = None;
            projection.summary.controller_surface_id = None;
            projection.summary.controller_share_id = None;
            projection.summary.control_epoch = next_control_epoch(projection.summary.control_epoch);
            publish_terminal(state, &projection.summary);
        }
    }
}

fn observe_terminal(
    session_id: SessionId,
    projection: Arc<RwLock<TerminalProjection>>,
    events: std::sync::mpsc::Receiver<SessionEvent>,
    binding: Option<TerminalBinding>,
    state: Arc<AppState>,
) -> Result<(), std::io::Error> {
    let runtime = tokio::runtime::Handle::current();
    thread::Builder::new()
        .name(format!("superplexr-projection-{session_id}"))
        .spawn(move || {
            while let Ok(event) = events.recv() {
                let terminal = is_terminal_event(&event);
                // A shell that emits OSC 133 gives us the exact failing command, so
                // each failure becomes its own Fault instead of waiting for the whole
                // session to end.
                if let SessionEvent::CommandFinished(block) = &event
                    && block.failed()
                    && !block.command.trim().is_empty()
                {
                    let state = Arc::clone(&state);
                    let block = block.clone();
                    runtime.spawn(async move {
                        record_command_fault(&state, session_id, binding, block).await;
                    });
                }
                let foreground_process = match &event {
                    SessionEvent::ForegroundProcessChanged { process_id } => {
                        foreground_process(*process_id)
                    }
                    _ => None,
                };
                if let Ok(mut projection) = projection.write() {
                    match &event {
                        SessionEvent::Frame(frame) => {
                            projection.summary.latest_sequence = frame.sequence;
                            projection.frame = Some(Arc::clone(frame));
                        }
                        SessionEvent::ForegroundProcessChanged { .. } => {
                            projection.summary.foreground_process = foreground_process;
                        }
                        SessionEvent::Exited(_) => {
                            projection.summary.status = TerminalSessionStatus::Exited;
                            projection.summary.foreground_process = None;
                            clear_projection_control(&mut projection);
                            projection.final_event =
                                Some(protocol_event(session_id, event.clone()));
                        }
                        SessionEvent::Failed { .. } => {
                            projection.summary.status = TerminalSessionStatus::Failed;
                            projection.summary.foreground_process = None;
                            clear_projection_control(&mut projection);
                            projection.final_event =
                                Some(protocol_event(session_id, event.clone()));
                        }
                        SessionEvent::Bell { .. }
                        | SessionEvent::CommandFinished(_)
                        | SessionEvent::PasteConfirmation(_)
                        | SessionEvent::TerminationEscalationRequired => {}
                    }
                    if terminal {
                        publish_terminal(&state, &projection.summary);
                    } else if matches!(event, SessionEvent::ForegroundProcessChanged { .. }) {
                        let _ = state.terminal_events.send(projection.summary.clone());
                    }
                }
                if terminal {
                    // A bound Run that ends badly is a Fault: record the exact
                    // command, its directory, and the tail of what it printed,
                    // before the Run is finished and the session is gone.
                    if let Some((exit_code, outcome, _)) = terminal_completion(&event)
                        && outcome == FinishOutcome::Failed
                    {
                        let tail = projection
                            .read()
                            .ok()
                            .and_then(|projection| projection.frame.clone())
                            .map(|frame| frame_tail(&frame))
                            .unwrap_or_default();
                        let state = Arc::clone(&state);
                        let kind = match &event {
                            SessionEvent::Failed { .. } => superplexr_protocol::FaultKind::Crashed,
                            _ => superplexr_protocol::FaultKind::CommandFailed,
                        };
                        runtime.spawn(async move {
                            record_terminal_fault(&state, session_id, binding, exit_code, kind, tail)
                                .await;
                        });
                    }
                    if let Some(binding) = binding
                        && let Some((exit_code, outcome, summary)) = terminal_completion(&event)
                    {
                        let state = Arc::clone(&state);
                        runtime.spawn(async move {
                            finish_bound_run(&state, binding, exit_code, outcome, summary).await;
                        });
                    }
                    break;
                }
            }
        })?;
    Ok(())
}

/// Visible rows kept as a Fault's output slice. Enough to carry a stack trace
/// or a failing assertion without storing the whole journal.
const FAULT_TAIL_ROWS: usize = 80;

/// Render the last non-empty rows of a frame as plain text.
fn frame_tail(frame: &superplexr_terminal::FullFrame) -> String {
    let lines = frame
        .rows
        .iter()
        .map(|row| row.text())
        .collect::<Vec<_>>();
    let end = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(0, |index| index + 1);
    let start = end.saturating_sub(FAULT_TAIL_ROWS);
    lines[start..end].join("\n")
}

/// Record a Fault for a terminal that ended in failure.
///
/// Only sessions whose launch spec is retained produce a Fault: without the
/// program and directory there is nothing to replay, and a record that cannot
/// be reproduced would be narrative rather than evidence.
async fn record_terminal_fault(
    state: &Arc<AppState>,
    session_id: SessionId,
    binding: Option<TerminalBinding>,
    exit_code: Option<i32>,
    kind: superplexr_protocol::FaultKind,
    output: String,
) {
    let directory = state
        .terminal_state_dir
        .join("sessions")
        .join(session_id.to_string());
    let Ok(bytes) = std::fs::read(directory.join("terminal.json")) else {
        return;
    };
    let Ok(spec) = serde_json::from_slice::<TerminalSessionSpec>(&bytes) else {
        return;
    };
    // An interactive shell exits with whatever its last command returned.
    // That is the shell being closed, not a failure of anything.
    if is_interactive_shell(&spec.program, &spec.args) {
        return;
    }
    let command = std::iter::once(spec.program.display().to_string())
        .chain(spec.args.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");
    let summary = match exit_code {
        Some(code) => format!("{command} exited {code}"),
        None => format!("{command} did not exit cleanly"),
    };
    let fault = superplexr_protocol::FaultInput {
        kind,
        command,
        cwd: spec.cwd,
        exit_code,
        revision: None,
        summary,
        output,
        session_id: Some(session_id),
        mission_id: binding.map(|binding| binding.mission_id),
        run_id: binding.map(|binding| binding.run_id),
    };
    if let Err(error) = state
        .faults
        .lock()
        .await
        .report(fault, FaultSource::TerminalExit)
    {
        eprintln!("could not record Fault for terminal {session_id}: {error}");
    }
}

/// Record a Fault for one failing shell command observed through OSC 133.
///
/// Unlike a session-level Fault this always has an exact command line, so the
/// record is replayable without consulting the launch spec.
/// True for a shell started to be typed into, rather than to run something.
///
/// Its exit status is its last command's, so a session ending in `zsh
/// exited 1` says only that the person closed a shell whose previous command
/// had failed. A shell given a command (`-c`) or a script is different: that
/// is a program, and its exit status means what it says.
fn is_interactive_shell(program: &std::path::Path, args: &[String]) -> bool {
    let shell = program
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                "sh" | "bash" | "zsh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "pwsh"
            )
        });
    // Login and interactive flags are still an interactive shell; anything
    // else (a command, a script) is a program.
    shell
        && args
            .iter()
            .all(|arg| matches!(arg.as_str(), "-l" | "-i" | "-il" | "-li" | "--login" | "--interactive"))
}

/// Exit statuses that mean the command was stopped rather than that it
/// failed: 130 is SIGINT (Ctrl-C) and 143 is SIGTERM, as shells report them.
const fn is_interruption(exit_code: Option<i32>) -> bool {
    matches!(exit_code, Some(130 | 143))
}

async fn record_command_fault(
    state: &Arc<AppState>,
    session_id: SessionId,
    binding: Option<TerminalBinding>,
    block: superplexr_runtime::command_blocks::CommandBlock,
) {
    // Someone stopping a command is not the command failing.
    if is_interruption(block.exit_code) {
        return;
    }
    let cwd = terminal_working_directory(state, session_id);
    let Some(cwd) = cwd else {
        return;
    };
    let summary = match block.exit_code {
        Some(code) => format!("{} exited {code}", block.command),
        None => format!("{} failed", block.command),
    };
    let fault = superplexr_protocol::FaultInput {
        kind: superplexr_protocol::FaultKind::CommandFailed,
        command: block.command,
        cwd,
        exit_code: block.exit_code,
        revision: None,
        summary,
        output: block.output,
        session_id: Some(session_id),
        mission_id: binding.map(|binding| binding.mission_id),
        run_id: binding.map(|binding| binding.run_id),
    };
    if let Err(error) = state
        .faults
        .lock()
        .await
        .report(fault, FaultSource::TerminalExit)
    {
        eprintln!("could not record Fault for terminal {session_id}: {error}");
    }
}

/// Directory a session was launched in, from its retained spec. A command
/// cannot be replayed without one, so a missing spec suppresses the Fault.
fn terminal_working_directory(state: &AppState, session_id: SessionId) -> Option<PathBuf> {
    let directory = state
        .terminal_state_dir
        .join("sessions")
        .join(session_id.to_string());
    let bytes = std::fs::read(directory.join("terminal.json")).ok()?;
    serde_json::from_slice::<TerminalSessionSpec>(&bytes)
        .ok()
        .map(|spec| spec.cwd)
}

/// Longest a replay may run before it is abandoned.
const DEFAULT_REPRO_TIMEOUT_SECONDS: u16 = 120;
const MAX_REPRO_TIMEOUT_SECONDS: u16 = 900;

/// Replay a Fault's exact command in its recorded directory and report what
/// happened. A replay that cannot run at all is recorded as an error, never as
/// a pass: only a command that actually ran and succeeded clears a Fault.
/// Replays in one classification sample when the caller names no count.
///
/// Five is enough to tell "always" from "sometimes" for the common case of a
/// failure that shows up a good fraction of the time; a rarer one needs more,
/// and the caller can ask, up to a ceiling that keeps one request bounded.
const DEFAULT_CLASSIFY_RUNS: u8 = 5;
const MIN_CLASSIFY_RUNS: u8 = 2;
const MAX_CLASSIFY_RUNS: u8 = 25;

/// Replay a Fault repeatedly and summarise how often it failed.
///
/// Returns the sample and the receipt that becomes the Fault's current
/// evidence: a failing one if any replay failed, otherwise the last. The
/// store sets `reproduced` from the sample's failure count, so a lucky pass
/// inside a mixed sample can never close the Fault.
async fn classify_fault(
    fault: &superplexr_protocol::FaultSummary,
    runs: u32,
    timeout_seconds: Option<u16>,
    isolated: bool,
    state_dir: &std::path::Path,
) -> (
    superplexr_protocol::FaultClassification,
    superplexr_protocol::ReproReceipt,
) {
    let classified_at_unix_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros() as u64)
        .unwrap_or_default();
    let timeout = Duration::from_secs(u64::from(
        timeout_seconds
            .unwrap_or(DEFAULT_REPRO_TIMEOUT_SECONDS)
            .clamp(1, MAX_REPRO_TIMEOUT_SECONDS),
    ));
    let mut failures = 0;
    let mut errors = 0;
    let mut revision = None;
    let mut failing = None;
    let mut last = None;
    for index in 0..runs {
        let receipt = if isolated {
            let fault = fault.clone();
            let root = state_dir.join("fault-worktrees");
            tokio::task::spawn_blocking(move || isolated_replay(&fault, timeout, &root, index))
                .await
                .unwrap_or_else(|error| replay_error(format!("replay task failed: {error}")))
        } else {
            reproduce_fault(fault, timeout_seconds).await
        };
        if receipt.error.is_some() {
            errors += 1;
        } else if receipt.reproduced {
            failures += 1;
            if failing.is_none() {
                failing = Some(receipt.clone());
            }
        }
        if revision.is_none() {
            revision.clone_from(&receipt.revision);
        }
        last = Some(receipt);
    }
    let classification = superplexr_protocol::FaultClassification {
        classified_at_unix_micros,
        runs,
        failures,
        errors,
        verdict: superplexr_protocol::FaultClassification::verdict_for(runs, failures, errors),
        isolated,
        revision,
    };
    let receipt = failing
        .or(last)
        .unwrap_or_else(|| replay_error("no replay ran".to_owned()));
    (classification, receipt)
}

/// A receipt for a replay that never ran.
fn replay_error(error: String) -> superplexr_protocol::ReproReceipt {
    superplexr_protocol::ReproReceipt {
        attempted_at_unix_micros: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_micros() as u64)
            .unwrap_or_default(),
        reproduced: false,
        exit_code: None,
        output: String::new(),
        duration_ms: 0,
        revision: None,
        error: Some(error),
    }
}

/// One replay in a fresh detached worktree of the Fault directory's
/// repository, at its current HEAD, run from the same relative directory.
///
/// The worktree is removed afterwards whatever happened. A directory outside
/// a repository cannot be isolated this way and yields an error receipt
/// rather than a silent in-place run, so the sample says what it measured.
fn isolated_replay(
    fault: &superplexr_protocol::FaultSummary,
    timeout: Duration,
    worktree_root: &std::path::Path,
    index: u32,
) -> superplexr_protocol::ReproReceipt {
    let attempted_at_unix_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros() as u64)
        .unwrap_or_default();
    let repo_root = match git_output(&fault.cwd, &["rev-parse", "--show-toplevel"]) {
        Some(root) => PathBuf::from(root),
        None => {
            return replay_error(format!(
                "{} is not inside a git repository, so it cannot be isolated",
                fault.cwd.display()
            ));
        }
    };
    let relative = fault
        .cwd
        .strip_prefix(&repo_root)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let worktree = worktree_root
        .join(fault.fault_id.to_string())
        .join(index.to_string());
    if let Err(error) = std::fs::create_dir_all(worktree_root) {
        return replay_error(format!("worktree root is unavailable: {error}"));
    }
    let added = StdCommand::new("git")
        .args(["worktree", "add", "--detach", "--force"])
        .arg(&worktree)
        .arg("HEAD")
        .current_dir(&repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match added {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            return replay_error(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Err(error) => return replay_error(format!("git is unavailable: {error}")),
    }
    let started = Instant::now();
    let outcome = run_repro(&fault.command, &worktree.join(relative), timeout);
    let duration_ms = started.elapsed().as_millis() as u64;
    // Remove it even if the replay failed to start; a leftover worktree would
    // pin the branch state and fill the disk.
    let _ = StdCommand::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&worktree)
        .current_dir(&repo_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = std::fs::remove_dir_all(&worktree);
    match outcome {
        Ok(ReproOutcome {
            exit_code,
            output,
            revision,
        }) => superplexr_protocol::ReproReceipt {
            attempted_at_unix_micros,
            reproduced: exit_code != Some(0),
            exit_code,
            output: truncate_output(&output),
            duration_ms,
            revision,
            error: None,
        },
        Err(error) => superplexr_protocol::ReproReceipt {
            attempted_at_unix_micros,
            reproduced: false,
            exit_code: None,
            output: String::new(),
            duration_ms,
            revision: None,
            error: Some(error),
        },
    }
}

fn git_output(directory: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = StdCommand::new("git")
        .args(args)
        .current_dir(directory)
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

/// Resolved Faults re-checked in one guard pass when the caller names no limit.
///
/// Each check runs a real command, so a pass is deliberately a bounded slice of
/// the resolved set rather than all of it. Oldest proof first means the least
/// recently confirmed Faults come round again first.
const DEFAULT_GUARD_LIMIT: u16 = 20;
/// Ceiling on one guard pass, whatever the caller asks for.
const MAX_GUARD_LIMIT: u16 = 200;

async fn reproduce_fault(
    fault: &superplexr_protocol::FaultSummary,
    timeout_seconds: Option<u16>,
) -> superplexr_protocol::ReproReceipt {
    let attempted_at_unix_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_micros() as u64)
        .unwrap_or_default();
    let timeout = Duration::from_secs(u64::from(
        timeout_seconds
            .unwrap_or(DEFAULT_REPRO_TIMEOUT_SECONDS)
            .clamp(1, MAX_REPRO_TIMEOUT_SECONDS),
    ));
    let command = fault.command.clone();
    let cwd = fault.cwd.clone();
    let started = Instant::now();

    let outcome = tokio::task::spawn_blocking(move || run_repro(&command, &cwd, timeout))
        .await
        .unwrap_or_else(|error| Err(format!("replay task failed: {error}")));
    let duration_ms = started.elapsed().as_millis() as u64;

    match outcome {
        Ok(ReproOutcome {
            exit_code,
            output,
            revision,
        }) => superplexr_protocol::ReproReceipt {
            attempted_at_unix_micros,
            reproduced: exit_code != Some(0),
            exit_code,
            output: truncate_output(&output),
            duration_ms,
            revision,
            error: None,
        },
        Err(error) => superplexr_protocol::ReproReceipt {
            attempted_at_unix_micros,
            // A replay that never ran proves nothing, so the Fault stays open.
            reproduced: false,
            exit_code: None,
            output: String::new(),
            duration_ms,
            revision: None,
            error: Some(error),
        },
    }
}

struct ReproOutcome {
    exit_code: Option<i32>,
    output: String,
    revision: Option<String>,
}

/// Run one replay to completion, merging stdout and stderr the way a terminal
/// would. Blocking; call from a blocking task.
fn run_repro(command: &str, cwd: &Path, timeout: Duration) -> Result<ReproOutcome, String> {
    let directory = repro_directory(cwd)?;
    let revision = repository_revision(&directory);
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| OsString::from("/bin/sh"));

    let mut child = StdCommand::new(&shell)
        .arg("-c")
        .arg(command)
        .current_dir(&directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("replay could not start: {error}"))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let collector = thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stdout) = stdout {
            let mut buffer = String::new();
            if std::io::Read::read_to_string(&mut stdout, &mut buffer).is_ok() {
                text.push_str(&buffer);
            }
        }
        if let Some(mut stderr) = stderr {
            let mut buffer = String::new();
            if std::io::Read::read_to_string(&mut stderr, &mut buffer).is_ok() {
                text.push_str(&buffer);
            }
        }
        text
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = collector.join();
                    return Err(format!("replay exceeded {} seconds", timeout.as_secs()));
                }
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                let _ = collector.join();
                return Err(format!("replay could not be observed: {error}"));
            }
        }
    };
    let output = collector.join().unwrap_or_default();
    Ok(ReproOutcome {
        exit_code: status.code(),
        output,
        revision,
    })
}

/// Best-effort revision of the directory a replay ran in. Absent when the
/// directory is not a repository or Git is unavailable.
fn repository_revision(directory: &Path) -> Option<String> {
    let output = StdCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?;
    let revision = revision.trim();
    (!revision.is_empty()).then(|| revision.to_owned())
}

fn foreground_process(process_id: u32) -> Option<TerminalForegroundProcess> {
    #[cfg(target_os = "linux")]
    let raw_name = std::fs::read_to_string(format!("/proc/{process_id}/comm")).ok();
    #[cfg(not(target_os = "linux"))]
    let raw_name = std::process::Command::new("ps")
        .args(["-p", &process_id.to_string(), "-o", "comm="])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());

    normalize_process_executable(raw_name.as_deref()?).map(|executable| {
        TerminalForegroundProcess {
            process_id,
            executable,
        }
    })
}

fn normalize_process_executable(raw_name: &str) -> Option<String> {
    let raw_name = raw_name.trim();
    if raw_name.is_empty() {
        return None;
    }
    std::path::Path::new(raw_name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn terminal_completion(event: &SessionEvent) -> Option<(Option<i32>, FinishOutcome, String)> {
    match event {
        SessionEvent::Exited(exit) => {
            let exit_code = i32::try_from(exit.code).ok();
            let outcome = if exit.success {
                FinishOutcome::Succeeded
            } else {
                FinishOutcome::Failed
            };
            Some((
                exit_code,
                outcome,
                exit.signal.as_ref().map_or_else(
                    || format!("agent process exited with code {}", exit.code),
                    |signal| format!("agent process exited after {signal}"),
                ),
            ))
        }
        SessionEvent::Failed { message } => Some((
            None,
            FinishOutcome::Failed,
            format!("agent terminal failed: {message}"),
        )),
        _ => None,
    }
}

async fn finish_bound_run(
    state: &Arc<AppState>,
    binding: TerminalBinding,
    exit_code: Option<i32>,
    outcome: FinishOutcome,
    summary: String,
) {
    let key = uuid::Uuid::new_v4();
    let result = state
        .store
        .lock()
        .await
        .dispatch_batch(
            binding.mission_id,
            vec![
                Command::FinishSession {
                    session_id: binding.session_id,
                    exit_code,
                },
                Command::FinishRun {
                    run_id: binding.run_id,
                    outcome,
                    summary,
                },
            ],
            None,
            key,
            key,
        )
        .await;
    match result {
        Ok((_, mission)) => {
            publish_mission(state, mission).await;
        }
        Err(error) => eprintln!(
            "failed to finalize bound agent Run {} for Session {}: {error}",
            binding.run_id, binding.session_id
        ),
    }
}

async fn reconcile_recovered_agent_runs(state: &Arc<AppState>) -> usize {
    let bindings = match state.terminals.read() {
        Ok(terminals) => terminals
            .values()
            .filter_map(|record| {
                let projection = record.projection.read().ok()?;
                Some(TerminalBinding {
                    mission_id: projection.summary.mission_id?,
                    run_id: projection.summary.run_id?,
                    session_id: projection.summary.session_id,
                })
            })
            .collect::<Vec<_>>(),
        Err(_) => return 0,
    };
    let mut attempted = 0;
    for binding in bindings {
        let mission = match state.store.lock().await.get(binding.mission_id) {
            Ok(mission) => mission,
            Err(error) => {
                eprintln!(
                    "failed to inspect recovered agent Run {} for Session {}: {error}",
                    binding.run_id, binding.session_id
                );
                continue;
            }
        };
        let mut commands = Vec::with_capacity(2);
        if mission
            .sessions
            .get(&binding.session_id)
            .is_some_and(|session| !session.status.is_finished())
        {
            commands.push(Command::FinishSession {
                session_id: binding.session_id,
                exit_code: None,
            });
        }
        if mission
            .runs
            .get(&binding.run_id)
            .is_some_and(|run| !run.status.is_finished())
        {
            commands.push(Command::FinishRun {
                run_id: binding.run_id,
                outcome: FinishOutcome::Failed,
                summary: "agent process ended when its previous runtime stopped".to_owned(),
            });
        }
        if commands.is_empty() {
            continue;
        }
        attempted += 1;
        let key = uuid::Uuid::new_v4();
        match state
            .store
            .lock()
            .await
            .dispatch_batch(binding.mission_id, commands, None, key, key)
            .await
        {
            Ok((_, mission)) => {
                publish_mission(state, mission).await;
            }
            Err(error) => eprintln!(
                "failed to reconcile recovered agent Run {} for Session {}: {error}",
                binding.run_id, binding.session_id
            ),
        }
    }
    attempted
}

fn clear_projection_control(projection: &mut TerminalProjection) {
    let had_controller = projection.summary.controller_client_id.is_some()
        || projection.summary.controller_surface_id.is_some();
    projection.summary.controller_client_id = None;
    projection.summary.controller_surface_id = None;
    if had_controller {
        projection.summary.control_epoch = next_control_epoch(projection.summary.control_epoch);
    }
}

fn terminal_record(
    state: &AppState,
    session_id: SessionId,
) -> Result<TerminalRecord, RequestError> {
    state
        .terminals
        .read()
        .map_err(|_| RequestError::RegistryPoisoned)?
        .get(&session_id)
        .cloned()
        .ok_or(RequestError::TerminalNotFound(session_id))
}

fn list_terminals(state: &AppState, include_archived: bool) -> Result<ResponseBody, RequestError> {
    let terminals = state
        .terminals
        .read()
        .map_err(|_| RequestError::RegistryPoisoned)?
        .values()
        .filter(|record| {
            include_archived
                || record
                    .projection
                    .read()
                    .is_ok_and(|projection| !projection.summary.archived)
        })
        .map(|record| {
            record
                .projection
                .read()
                .map(|projection| projection.summary.clone())
                .map_err(|_| RequestError::RegistryPoisoned)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ResponseBody::Terminals { terminals })
}

fn validate_session_group_members(
    state: &AppState,
    _mission_id: Option<MissionId>,
    session_ids: &[SessionId],
) -> Result<(), RequestError> {
    for session_id in session_ids {
        terminal_record(state, *session_id)?;
    }
    Ok(())
}

fn set_terminal_archived(
    state: &AppState,
    session_id: SessionId,
    archived: bool,
) -> Result<(), RequestError> {
    let record = terminal_record(state, session_id)?;
    {
        let projection = record
            .projection
            .read()
            .map_err(|_| RequestError::RegistryPoisoned)?;
        if archived && projection.summary.status == TerminalSessionStatus::Running {
            return Err(RequestError::ArchiveRunningTerminal(session_id));
        }
        if projection.summary.archived == archived {
            return Ok(());
        }
    }

    let directory = state
        .terminal_state_dir
        .join("sessions")
        .join(session_id.to_string());
    let marker = directory.join("archived");
    if archived {
        if marker.exists() {
            let metadata = std::fs::symlink_metadata(&marker)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(RequestError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "terminal archive marker is not a real file",
                )));
            }
        } else {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&marker)?;
            file.write_all(b"archived\n")?;
            file.sync_all()?;
            std::fs::File::open(&directory)?.sync_all()?;
        }
    } else {
        match std::fs::remove_file(&marker) {
            Ok(()) => std::fs::File::open(&directory)?.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    let summary = {
        let mut projection = record
            .projection
            .write()
            .map_err(|_| RequestError::RegistryPoisoned)?;
        projection.summary.archived = archived;
        projection.summary.clone()
    };
    publish_terminal(state, &summary);
    Ok(())
}

/// The projection's frame, when the request is for the bottom of history.
///
/// Only `RowsBeforeBottom(0)` qualifies. Every other viewport addresses rows
/// that have scrolled out of the screen the projection holds, and those still
/// need a replay.
fn bottom_history_frame(
    record: &TerminalRecord,
    viewport: superplexr_terminal::HistoryViewport,
) -> Result<Option<Arc<superplexr_terminal::FullFrame>>, RequestError> {
    if !matches!(
        viewport,
        superplexr_terminal::HistoryViewport::RowsBeforeBottom(0)
    ) {
        return Ok(None);
    }
    Ok(record
        .projection
        .read()
        .map_err(|_| RequestError::RegistryPoisoned)?
        .frame
        .clone())
}

fn terminal_snapshot(
    state: &AppState,
    session_id: SessionId,
) -> Result<ResponseBody, RequestError> {
    let record = terminal_record(state, session_id)?;
    let frame = record
        .projection
        .read()
        .map_err(|_| RequestError::RegistryPoisoned)?
        .frame
        .clone()
        .ok_or(RequestError::TerminalNotFound(session_id))?;
    Ok(ResponseBody::TerminalFrame {
        session_id,
        frame: Box::new((*frame).clone()),
    })
}

fn terminal_capture(
    state: &AppState,
    session_id: SessionId,
) -> Result<TerminalCapture, RequestError> {
    let record = terminal_record(state, session_id)?;
    terminal_capture_from_record(&record, session_id)
}

fn terminal_capture_from_record(
    record: &TerminalRecord,
    session_id: SessionId,
) -> Result<TerminalCapture, RequestError> {
    let (terminal, frame) = {
        let projection = record
            .projection
            .read()
            .map_err(|_| RequestError::RegistryPoisoned)?;
        (
            projection.summary.clone(),
            projection
                .frame
                .clone()
                .ok_or(RequestError::TerminalNotFound(session_id))?,
        )
    };
    let captured_at_unix_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| std::io::Error::other(error.to_string()))?
        .as_micros()
        .try_into()
        .map_err(|_| std::io::Error::other("system timestamp exceeds u64"))?;
    Ok(TerminalCapture {
        terminal,
        frame: Box::new((*frame).clone()),
        captured_at_unix_micros,
    })
}

async fn terminal_wait(
    state: &Arc<AppState>,
    session_id: SessionId,
    condition: TerminalWaitCondition,
    timeout_millis: u64,
) -> Result<ResponseBody, RequestError> {
    if !(1..=3_600_000).contains(&timeout_millis) {
        return Err(RequestError::InvalidWaitTimeout);
    }
    match &condition {
        TerminalWaitCondition::Text { query, .. }
            if query.is_empty() || query.len() > 1024 || query.contains('\0') =>
        {
            return Err(RequestError::InvalidWaitQuery);
        }
        TerminalWaitCondition::Quiet { quiet_millis } if !(50..=60_000).contains(quiet_millis) => {
            return Err(RequestError::InvalidQuietWait);
        }
        _ => {}
    }
    let _wait_guard = ActiveWaitGuard::acquire(&state.active_waits)?;
    let started = Instant::now();
    let timeout = Duration::from_millis(timeout_millis);
    let record = terminal_record(state, session_id)?;
    // Subscribe before reading the initial frame. Any race is therefore a
    // harmless duplicate sequence rather than missed output.
    let subscription = record
        .handle
        .as_ref()
        .map(SessionHandle::subscribe)
        .transpose()?;
    let initial = terminal_capture_from_record(&record, session_id)?;
    if wait_condition_matches(&condition, &initial) {
        return Ok(wait_satisfied(condition, started, initial));
    }

    let Some(events) = subscription else {
        if let TerminalWaitCondition::Quiet { quiet_millis } = condition {
            let quiet = Duration::from_millis(quiet_millis);
            if quiet > timeout {
                return Err(RequestError::WaitTimeout);
            }
            tokio::time::sleep(quiet).await;
            return Ok(wait_satisfied(
                TerminalWaitCondition::Quiet { quiet_millis },
                started,
                terminal_capture_from_record(&record, session_id)?,
            ));
        }
        return Err(RequestError::WaitCannotChange);
    };

    let (send, mut receive) = mpsc::channel(64);
    thread::Builder::new()
        .name(format!("superplexr-wait-{session_id}"))
        .spawn(move || {
            while let Ok(event) = events.recv() {
                let terminal = is_terminal_event(&event);
                if send.blocking_send(event).is_err() || terminal {
                    break;
                }
            }
        })?;

    let mut last_sequence = initial.frame.sequence;
    let mut quiet_since = started;
    loop {
        let elapsed = started.elapsed();
        if elapsed >= timeout {
            return Err(RequestError::WaitTimeout);
        }
        let overall_remaining = timeout - elapsed;
        let receive_for = match &condition {
            TerminalWaitCondition::Quiet { quiet_millis } => {
                let quiet = Duration::from_millis(*quiet_millis);
                let stable = quiet_since.elapsed();
                if stable >= quiet {
                    return Ok(wait_satisfied(
                        condition,
                        started,
                        terminal_capture_from_record(&record, session_id)?,
                    ));
                }
                overall_remaining.min(quiet - stable)
            }
            _ => overall_remaining,
        };

        match tokio::time::timeout(receive_for, receive.recv()).await {
            Err(_) => {
                if matches!(&condition, TerminalWaitCondition::Quiet { .. })
                    && started.elapsed() < timeout
                {
                    return Ok(wait_satisfied(
                        condition,
                        started,
                        terminal_capture_from_record(&record, session_id)?,
                    ));
                }
                return Err(RequestError::WaitTimeout);
            }
            Ok(Some(SessionEvent::Frame(frame))) => {
                if frame.sequence <= last_sequence {
                    continue;
                }
                last_sequence = frame.sequence;
                quiet_since = Instant::now();
                if frame_matches_wait(&condition, &frame) {
                    return Ok(wait_satisfied(
                        condition,
                        started,
                        terminal_capture_from_record(&record, session_id)?,
                    ));
                }
            }
            Ok(Some(event)) if is_terminal_event(&event) => {
                let capture = terminal_capture_from_record(&record, session_id)?;
                if wait_condition_matches(&condition, &capture) {
                    return Ok(wait_satisfied(condition, started, capture));
                }
                if matches!(&condition, TerminalWaitCondition::Quiet { .. }) {
                    quiet_since = Instant::now();
                    continue;
                }
                return Err(RequestError::WaitCannotChange);
            }
            Ok(Some(_)) => {}
            Ok(None) => {
                let capture = terminal_capture_from_record(&record, session_id)?;
                if wait_condition_matches(&condition, &capture) {
                    return Ok(wait_satisfied(condition, started, capture));
                }
                if let TerminalWaitCondition::Quiet { quiet_millis } = condition {
                    let quiet = Duration::from_millis(quiet_millis);
                    let remaining = quiet.saturating_sub(quiet_since.elapsed());
                    if started.elapsed().saturating_add(remaining) > timeout {
                        return Err(RequestError::WaitTimeout);
                    }
                    tokio::time::sleep(remaining).await;
                    return Ok(wait_satisfied(
                        TerminalWaitCondition::Quiet { quiet_millis },
                        started,
                        terminal_capture_from_record(&record, session_id)?,
                    ));
                }
                return Err(RequestError::WaitStreamClosed);
            }
        }
    }
}

fn wait_condition_matches(condition: &TerminalWaitCondition, capture: &TerminalCapture) -> bool {
    match condition {
        TerminalWaitCondition::Text { .. } => frame_matches_wait(condition, &capture.frame),
        TerminalWaitCondition::Quiet { .. } => false,
        TerminalWaitCondition::Exit => capture.terminal.status != TerminalSessionStatus::Running,
    }
}

fn frame_matches_wait(
    condition: &TerminalWaitCondition,
    frame: &superplexr_terminal::FullFrame,
) -> bool {
    let TerminalWaitCondition::Text {
        query,
        case_sensitive,
    } = condition
    else {
        return false;
    };
    if *case_sensitive {
        frame.rows.iter().any(|row| row.text().contains(query))
    } else {
        let query = query.to_lowercase();
        frame
            .rows
            .iter()
            .any(|row| row.text().to_lowercase().contains(&query))
    }
}

fn wait_satisfied(
    condition: TerminalWaitCondition,
    started: Instant,
    capture: TerminalCapture,
) -> ResponseBody {
    ResponseBody::TerminalWaitSatisfied {
        condition,
        elapsed_millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        capture,
    }
}

const fn accepted(session_id: SessionId) -> ResponseBody {
    ResponseBody::TerminalCommandAccepted { session_id }
}

const fn is_terminal_event(event: &SessionEvent) -> bool {
    matches!(event, SessionEvent::Exited(_) | SessionEvent::Failed { .. })
}

fn protocol_event(session_id: SessionId, event: SessionEvent) -> ServerEvent {
    match event {
        SessionEvent::Frame(frame) => ServerEvent::TerminalFrame {
            session_id,
            frame: Box::new((*frame).clone()),
        },
        SessionEvent::ForegroundProcessChanged { .. } => {
            unreachable!("foreground process changes use the terminal index stream")
        }
        SessionEvent::CommandFinished(_) => {
            unreachable!("command blocks become Faults instead of terminal events")
        }
        SessionEvent::Bell { count } => ServerEvent::TerminalBell { session_id, count },
        SessionEvent::PasteConfirmation(confirmation) => ServerEvent::PasteConfirmation {
            session_id,
            confirmation,
        },
        SessionEvent::TerminationEscalationRequired => {
            ServerEvent::TerminalTerminationEscalationRequired { session_id }
        }
        SessionEvent::Exited(exit) => ServerEvent::TerminalExited {
            session_id,
            code: exit.code,
            signal: exit.signal,
            success: exit.success,
        },
        SessionEvent::Failed { message } => ServerEvent::TerminalFailed {
            session_id,
            message,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, time::Duration};
    use superplexr_core::{
        Actor, ActorId, ChangeClaim, ChangeIntentSpec, ChangeIntentState, ChangeOperation,
        ChangeScope, RunCandidate, RunPriority, RunStatus,
    };
    use superplexr_protocol::{SchedulerPolicy, SchedulerSettings, TerminalSessionSpec};
    use superplexr_terminal::{GridSize, SelectionPoint};
    use uuid::Uuid;

    /// The desktop asks for the bottom frame of every session it restores, so
    /// that request must not read the journal at all.
    ///
    /// It used to replay it, which is what turned restoring twelve exited
    /// sessions into a multi-minute freeze: the frame recovery had already
    /// built was ignored, and each session paid an O(journal) replay.
    #[test]
    fn the_bottom_of_history_is_served_without_reading_the_journal() {
        let session_id = SessionId::new();
        let grid = GridSize::new(80, 24).expect("grid should be valid");
        let mut model = TerminalModel::new(grid).expect("model should build");
        model
            .advance(TerminalAction::Output(b"held screen\r\n"))
            .expect("output should parse");
        let mut frame = model.frame().expect("frame should render");
        frame.sequence = 7;
        let record = TerminalRecord {
            handle: None,
            projection: Arc::new(RwLock::new(TerminalProjection {
                summary: TerminalSessionSummary {
                    session_id,
                    mission_id: None,
                    run_id: None,
                    process_id: None,
                    foreground_process: None,
                    tty_name: None,
                    status: TerminalSessionStatus::Exited,
                    archived: false,
                    latest_sequence: frame.sequence,
                    controller_client_id: None,
                    controller_surface_id: None,
                    controller_share_id: None,
                    control_epoch: 0,
                    cwd: None,
                },
                frame: Some(Arc::new(frame)),
                final_event: None,
            })),
        };

        let served = bottom_history_frame(
            &record,
            superplexr_terminal::HistoryViewport::RowsBeforeBottom(0),
        )
        .expect("bottom frame should be readable")
        .expect("the projection holds the bottom frame");
        assert_eq!(served.sequence, 7, "the projection's own frame must be used");

        // Anything above the bottom addresses rows that have scrolled out of
        // the held screen, so it still has to replay.
        for viewport in [
            superplexr_terminal::HistoryViewport::RowsBeforeBottom(1),
            superplexr_terminal::HistoryViewport::RowFromTop(0),
        ] {
            assert!(
                bottom_history_frame(&record, viewport)
                    .expect("viewport should be readable")
                    .is_none(),
                "{viewport:?} must not be answered from the bottom frame"
            );
        }
    }

    /// Startup recovery must not depend on how large a session's journal grew.
    ///
    /// A 600 MB journal of escape-dense output once took minutes to replay, and
    /// recovery runs before the daemon serves anything, so every client waiting
    /// on it appeared frozen. Both the byte window and the deadline are needed:
    /// cost per byte varies by two orders of magnitude with escape density.
    #[test]
    fn recovery_replay_is_bounded_in_both_bytes_and_time() {
        let root = std::env::temp_dir().join(format!("superplexr-replay-cap-{}", SessionId::new()));
        let session_id = SessionId::new();
        let directory = root.join("sessions").join(session_id.to_string());
        std::fs::create_dir_all(&directory).expect("session directory should be creatable");

        let spec = TerminalSessionSpec {
            session_id,
            mission_id: None,
            run_id: None,
            program: std::path::PathBuf::from("/bin/sh"),
            args: Vec::new(),
            cwd: std::env::current_dir().expect("working directory should exist"),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).expect("grid should be valid"),
        };
        std::fs::write(
            directory.join("terminal.json"),
            serde_json::to_vec(&spec).expect("spec should serialize"),
        )
        .expect("metadata should be writable");

        // A journal far larger than the recovery window, ending in a line that
        // must survive: recovery replays the tail, never the head.
        let mut journal = vec![b'.'; usize::try_from(RECOVERY_REPLAY_BYTES).expect("fits") * 4];
        journal.extend_from_slice(b"\r\nTAILMARKER\r\n");
        std::fs::write(directory.join("output.raw"), &journal)
            .expect("journal should be writable");

        let started = Instant::now();
        let mut model = replay_terminal_model(&root, session_id, ReplayBudget::RECOVERY)
            .expect("recovery should rebuild a screen");
        let elapsed = started.elapsed();

        assert!(
            elapsed < ReplayBudget::RECOVERY.deadline * 3,
            "recovering a {} byte journal took {elapsed:?}; the replay window is not bounded",
            journal.len()
        );
        let frame = model.frame().expect("recovered screen should render");
        let text = frame
            .rows
            .iter()
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("TAILMARKER"),
            "recovery replayed the wrong end of the journal; the last screen is the tail"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Closing a shell is not a Fault, however its last command went.
    #[test]
    fn closing_an_interactive_shell_is_not_a_fault() {
        let p = std::path::Path::new;
        let none: [String; 0] = [];
        assert!(is_interactive_shell(p("/bin/zsh"), &none));
        assert!(is_interactive_shell(p("/opt/homebrew/bin/fish"), &["-l".to_owned()]));
        assert!(!is_interactive_shell(p("/bin/zsh"), &["-c".to_owned(), "cargo test".to_owned()]));
        assert!(!is_interactive_shell(p("/bin/sh"), &["build.sh".to_owned()]));
        assert!(!is_interactive_shell(p("/usr/bin/python3"), &none));
    }

    /// Ctrl-C and SIGTERM stop a command; they do not make it a Fault.
    #[test]
    fn an_interrupted_command_is_not_a_fault() {
        assert!(is_interruption(Some(130)));
        assert!(is_interruption(Some(143)));
        assert!(!is_interruption(Some(1)));
        assert!(!is_interruption(Some(127)), "a missing command is a real failure");
        assert!(!is_interruption(None));
    }

    #[test]
    fn foreground_process_names_are_bounded_to_the_executable() {
        assert_eq!(
            normalize_process_executable("  /opt/homebrew/bin/codex\n"),
            Some("codex".to_owned())
        );
        assert_eq!(normalize_process_executable("\n"), None);
    }

    fn test_frame(rows: Vec<std::sync::Arc<superplexr_terminal::Row>>) -> superplexr_terminal::FullFrame {
        superplexr_terminal::FullFrame {
            sequence: 1,
            grid: superplexr_terminal::GridSize::new(80, 24).expect("grid"),
            rows,
            styles: Vec::new(),
            cursor: None,
            default_foreground: superplexr_terminal::Rgb {
                red: 0,
                green: 0,
                blue: 0,
            },
            default_background: superplexr_terminal::Rgb {
                red: 0,
                green: 0,
                blue: 0,
            },
            mouse_tracking: false,
            title: None,
            current_directory: None,
        }
    }

    #[test]
    fn frame_tails_keep_the_last_meaningful_rows() {
        use superplexr_terminal::{Cell, Row};
        fn row(text: &str) -> std::sync::Arc<Row> {
            std::sync::Arc::new(Row {
                wrapped: false,
                cells: text
                    .chars()
                    .map(|character| Cell {
                        grapheme: character.to_string(),
                        width: 1,
                        style_index: 0,
                        hyperlink: None,
                    })
                    .collect(),
            })
        }

        let mut rows = (0..FAULT_TAIL_ROWS + 20)
            .map(|index| row(&format!("line-{index}")))
            .collect::<Vec<_>>();
        // Trailing blank rows are the common case for a cleared screen.
        rows.push(row("   "));
        rows.push(row(""));
        let frame = test_frame(rows);

        let tail = frame_tail(&frame);
        let lines = tail.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), FAULT_TAIL_ROWS);
        assert_eq!(
            lines.last().copied(),
            Some(format!("line-{}", FAULT_TAIL_ROWS + 19)).as_deref()
        );
        assert!(!tail.contains("line-0"), "the oldest rows are dropped");

        assert!(frame_tail(&test_frame(Vec::new())).is_empty());
        assert!(frame_tail(&test_frame(vec![row("  ")])).is_empty());
    }

    async fn report_for_classification(
        state: &Arc<AppState>,
        command: &str,
        cwd: &std::path::Path,
    ) -> superplexr_protocol::FaultSummary {
        match handle_request(
            Request::ReportFault {
                fault: superplexr_protocol::FaultInput {
                    kind: superplexr_protocol::FaultKind::TestFailed,
                    command: command.to_owned(),
                    cwd: cwd.to_path_buf(),
                    exit_code: Some(1),
                    revision: None,
                    summary: "sometimes".to_owned(),
                    output: "failing".to_owned(),
                    session_id: None,
                    mission_id: None,
                    run_id: None,
                },
            },
            Uuid::new_v4(),
            state,
        )
        .await
        .expect("owner should report a Fault")
        {
            ResponseBody::FaultRecorded { fault } => fault,
            body => panic!("unexpected report response: {body:?}"),
        }
    }

    async fn classify(
        state: &Arc<AppState>,
        fault_id: superplexr_core::FaultId,
        runs: u8,
        isolated: bool,
    ) -> superplexr_protocol::FaultClassification {
        match handle_request(
            Request::ClassifyFault {
                fault_id,
                runs: Some(runs),
                timeout_seconds: Some(30),
                isolated,
            },
            Uuid::new_v4(),
            state,
        )
        .await
        .expect("owner should classify a Fault")
        {
            ResponseBody::FaultRecorded { fault } => fault
                .classification
                .expect("classification should be recorded"),
            body => panic!("unexpected classify response: {body:?}"),
        }
    }

    /// A failure that happens some of the time is told apart from one that
    /// is always there, and neither a lucky pass nor a sample can close it.
    #[tokio::test]
    async fn classification_tells_flaky_from_real_and_flaky_stays_open() {
        let root = std::env::temp_dir().join(format!("superplexr-fault-classify-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test root should be creatable");
        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });

        // A counter file makes the command fail on odd runs only.
        let flaky = report_for_classification(
            &state,
            "c=$(cat n 2>/dev/null || echo 0); c=$((c+1)); echo $c > n; test $((c % 2)) -eq 0",
            &root,
        )
        .await;
        let sample = classify(&state, flaky.fault_id, 4, false).await;
        assert_eq!(sample.verdict, superplexr_protocol::FaultVerdict::Flaky);
        assert_eq!((sample.runs, sample.failures, sample.errors), (4, 2, 0));
        assert!(!sample.isolated);
        // The last replay passed, and that must not be enough.
        assert!(matches!(
            handle_request(
                Request::ResolveFault {
                    fault_id: flaky.fault_id,
                    note: "passed once".to_owned(),
                },
                Uuid::new_v4(),
                &state,
            )
            .await,
            Err(RequestError::Fault(fault_store::FaultError::Unproven(_)))
        ));

        let real = report_for_classification(&state, "false", &root).await;
        let sample = classify(&state, real.fault_id, 3, false).await;
        assert_eq!(sample.verdict, superplexr_protocol::FaultVerdict::Real);
        assert_eq!((sample.runs, sample.failures), (3, 3));

        // Outside a repository an isolated run cannot happen, and says so
        // rather than quietly running in place.
        let sample = classify(&state, real.fault_id, 2, true).await;
        assert_eq!(sample.verdict, superplexr_protocol::FaultVerdict::Inconclusive);
        assert_eq!(sample.errors, 2);
        assert!(sample.isolated);

        std::fs::remove_dir_all(&root).expect("test root should be removable");
    }

    /// Isolated replays run in fresh worktrees of the repository and leave
    /// none behind.
    #[tokio::test]
    async fn isolated_classification_uses_fresh_worktrees_and_cleans_up() {
        let root = std::env::temp_dir().join(format!("superplexr-fault-isolated-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test root should be creatable");
        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });

        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo dir");
        let git = |args: &[&str]| {
            let status = StdCommand::new("git")
                .args(args)
                .current_dir(&repo)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("git should run");
            assert!(status.success(), "git {args:?} should succeed");
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join("probe.sh"), "exit 1\n").expect("probe");
        git(&["add", "probe.sh"]);
        git(&[
            "-c", "user.email=t@example.invalid", "-c", "user.name=t",
            "commit", "-q", "-m", "probe",
        ]);
        // An untracked file that only exists in place. The command needs it,
        // so it passes in place and fails in a fresh worktree: the difference
        // isolation makes, made observable.
        std::fs::write(repo.join("only-here"), "x").expect("untracked");

        let fault =
            report_for_classification(&state, "test -f probe.sh && test -f only-here", &repo)
                .await;
        let sample = classify(&state, fault.fault_id, 3, true).await;
        assert!(sample.isolated);
        assert_eq!(sample.verdict, superplexr_protocol::FaultVerdict::Real);
        assert_eq!((sample.runs, sample.failures, sample.errors), (3, 3, 0));
        assert!(sample.revision.is_some(), "the worktree's revision is recorded");

        let listed = StdCommand::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&repo)
            .output()
            .expect("git should run");
        let worktrees = String::from_utf8_lossy(&listed.stdout)
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count();
        assert_eq!(worktrees, 1, "every replay worktree must be removed");
        let leftovers = std::fs::read_dir(root.join("fault-worktrees").join(fault.fault_id.to_string()))
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(leftovers, 0, "no replay directory may be left behind");

        // In place, the untracked file is visible and the command passes.
        let sample = classify(&state, fault.fault_id, 2, false).await;
        assert_eq!(sample.verdict, superplexr_protocol::FaultVerdict::Passing);

        std::fs::remove_dir_all(&root).expect("test root should be removable");
    }

    /// A Fault that was proven fixed must not be able to come back unnoticed.
    ///
    /// This runs the whole loop against real processes: report, replay, resolve
    /// on passing evidence, break it again, then guard.
    #[tokio::test]
    async fn the_guard_reopens_a_resolved_fault_that_breaks_again() {
        let root = std::env::temp_dir().join(format!("superplexr-fault-guard-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test root should be creatable");
        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });

        // The same marker trick as the resolution test: the replayed command
        // fails exactly while the marker exists.
        let marker = root.join("broken");
        std::fs::write(&marker, b"broken").expect("marker should be writable");
        let command = format!("test ! -f {}", marker.display());

        let reported = match handle_request(
            Request::ReportFault {
                fault: superplexr_protocol::FaultInput {
                    kind: superplexr_protocol::FaultKind::TestFailed,
                    command: command.clone(),
                    cwd: root.clone(),
                    exit_code: Some(1),
                    revision: None,
                    summary: "marker present".to_owned(),
                    output: "failing".to_owned(),
                    session_id: None,
                    mission_id: None,
                    run_id: None,
                },
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should report a Fault")
        {
            ResponseBody::FaultRecorded { fault } => fault,
            body => panic!("unexpected report response: {body:?}"),
        };

        // Fix it for real, prove it, and close it.
        std::fs::remove_file(&marker).expect("marker should be removable");
        match handle_request(
            Request::ReproduceFault {
                fault_id: reported.fault_id,
                timeout_seconds: Some(30),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should replay a Fault")
        {
            ResponseBody::FaultRecorded { fault } => assert!(fault.repro_passes()),
            body => panic!("unexpected replay response: {body:?}"),
        }
        match handle_request(
            Request::ResolveFault {
                fault_id: reported.fault_id,
                note: "marker removed".to_owned(),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("a proven Fault should resolve")
        {
            ResponseBody::FaultRecorded { fault } => {
                assert!(!fault.is_open());
                assert_eq!(fault.regressions, 0);
            }
            body => panic!("unexpected resolve response: {body:?}"),
        }

        // While it stays fixed, a guard pass must leave it closed.
        match handle_request(
            Request::GuardFaults {
                limit: None,
                timeout_seconds: Some(30),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should guard Faults")
        {
            ResponseBody::FaultsGuarded { checked, reopened } => {
                assert_eq!(checked, vec![reported.fault_id]);
                assert!(reopened.is_empty(), "a Fault that still passes stays closed");
            }
            body => panic!("unexpected guard response: {body:?}"),
        }

        // Now reintroduce the failure. The guard must find it.
        std::fs::write(&marker, b"broken again").expect("marker should be writable");
        match handle_request(
            Request::GuardFaults {
                limit: None,
                timeout_seconds: Some(30),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should guard Faults")
        {
            ResponseBody::FaultsGuarded { checked, reopened } => {
                assert_eq!(checked, vec![reported.fault_id]);
                assert_eq!(reopened.len(), 1, "the reintroduced failure must reopen");
                assert_eq!(reopened[0].fault_id, reported.fault_id);
                assert!(reopened[0].is_open());
                assert_eq!(reopened[0].regressions, 1);
                assert!(
                    reopened[0].proof.is_some(),
                    "the evidence that closed it is kept"
                );
            }
            body => panic!("unexpected guard response: {body:?}"),
        }

        // Reopened means reopened: it cannot be closed again by assertion.
        assert!(matches!(
            handle_request(
                Request::ResolveFault {
                    fault_id: reported.fault_id,
                    note: "it is fine really".to_owned(),
                },
                Uuid::new_v4(),
                &state,
            )
            .await,
            Err(RequestError::Fault(fault_store::FaultError::Unproven(_)))
        ));

        std::fs::remove_dir_all(&root).expect("test root should be removable");
    }

    #[tokio::test]
    async fn faults_close_only_when_a_replay_actually_passes() {
        let root = std::env::temp_dir().join(format!("superplexr-fault-e2e-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("test root should be creatable");
        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });

        // A marker file decides whether the replayed command fails, so the
        // same Fault can be reproduced and then observed to pass.
        let marker = root.join("broken");
        std::fs::write(&marker, b"broken").expect("marker should be writable");
        let command = format!("test ! -f {}", marker.display());

        let reported = match handle_request(
            Request::ReportFault {
                fault: superplexr_protocol::FaultInput {
                    kind: superplexr_protocol::FaultKind::TestFailed,
                    command: command.clone(),
                    cwd: root.clone(),
                    exit_code: Some(1),
                    revision: None,
                    summary: "marker present".to_owned(),
                    output: "failing".to_owned(),
                    session_id: None,
                    mission_id: None,
                    run_id: None,
                },
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should report a Fault")
        {
            ResponseBody::FaultRecorded { fault } => fault,
            body => panic!("unexpected report response: {body:?}"),
        };
        assert!(reported.is_open());
        assert_eq!(reported.source, FaultSource::OwnerHook);
        assert_eq!(reported.repro_attempts, 0);

        // An open Fault is listed by default.
        match handle_request(
            Request::ListFaults {
                mission_id: None,
                session_id: None,
                include_closed: false,
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should list Faults")
        {
            ResponseBody::Faults { faults } => {
                assert_eq!(faults.len(), 1);
                assert_eq!(faults[0].fault_id, reported.fault_id);
            }
            body => panic!("unexpected list response: {body:?}"),
        }

        // Replaying while still broken reproduces the failure.
        let reproduced = match handle_request(
            Request::ReproduceFault {
                fault_id: reported.fault_id,
                timeout_seconds: Some(30),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should replay a Fault")
        {
            ResponseBody::FaultRecorded { fault } => fault,
            body => panic!("unexpected replay response: {body:?}"),
        };
        let receipt = reproduced.repro.as_ref().expect("a receipt should exist");
        assert!(receipt.reproduced, "the marker still exists: {receipt:?}");
        assert_eq!(receipt.exit_code, Some(1));
        assert!(receipt.error.is_none());
        assert!(!reproduced.repro_passes());

        // Resolution is refused while the failure still reproduces.
        let refused = handle_request(
            Request::ResolveFault {
                fault_id: reported.fault_id,
                note: "trust me".to_owned(),
            },
            Uuid::new_v4(),
            &state,
        )
        .await;
        assert!(
            matches!(refused, Err(RequestError::Fault(_))),
            "a still-failing Fault must not resolve: {refused:?}"
        );

        // Fix the underlying condition, replay again, then resolve.
        std::fs::remove_file(&marker).expect("marker should be removable");
        let passing = match handle_request(
            Request::ReproduceFault {
                fault_id: reported.fault_id,
                timeout_seconds: Some(30),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should replay a Fault")
        {
            ResponseBody::FaultRecorded { fault } => fault,
            body => panic!("unexpected replay response: {body:?}"),
        };
        assert!(passing.repro_passes());
        assert_eq!(passing.repro_attempts, 2);

        let resolved = match handle_request(
            Request::ResolveFault {
                fault_id: reported.fault_id,
                note: "marker removed".to_owned(),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("a proven Fault should resolve")
        {
            ResponseBody::FaultRecorded { fault } => fault,
            body => panic!("unexpected resolve response: {body:?}"),
        };
        assert!(!resolved.is_open());

        // Closed Faults leave the default list but remain retrievable.
        match handle_request(
            Request::ListFaults {
                mission_id: None,
                session_id: None,
                include_closed: false,
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should list Faults")
        {
            ResponseBody::Faults { faults } => assert!(faults.is_empty()),
            body => panic!("unexpected list response: {body:?}"),
        }
        match handle_request(
            Request::GetFault {
                fault_id: reported.fault_id,
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should read a closed Fault")
        {
            ResponseBody::FaultRecorded { fault } => assert!(!fault.is_open()),
            body => panic!("unexpected get response: {body:?}"),
        }

        std::fs::remove_dir_all(root).expect("test state should be removable");
    }

    #[tokio::test]
    async fn observer_shares_filter_scope_deny_mutation_and_revoke_live_authority() {
        let root = std::env::temp_dir().join(format!("superplexr-share-auth-{}", Uuid::new_v4()));
        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        let visible = MissionId::new();
        let hidden = MissionId::new();
        for (mission_id, intent) in [(visible, "visible"), (hidden, "hidden")] {
            handle_request(
                Request::CreateMission {
                    mission_id,
                    intent: intent.to_owned(),
                    created_by: Actor::human("owner").expect("owner Actor should be valid"),
                },
                Uuid::new_v4(),
                &state,
            )
            .await
            .expect("mission should be created");
        }

        let (share, token) = match handle_request(
            Request::CreateShare {
                label: "reviewer".to_owned(),
                role: ShareRole::Observer,
                mission_ids: vec![visible],
                session_ids: Vec::new(),
                expires_in_seconds: 3_600,
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should create share")
        {
            ResponseBody::ShareCreated { share, token } => (share, token),
            body => panic!("unexpected create response: {body:?}"),
        };
        let authenticated = state
            .shares
            .lock()
            .await
            .authenticate(&token)
            .expect("share token should authenticate");
        let authority = ClientAuthority::Shared(authenticated);
        assert!(
            authorize_request(
                &authority,
                &Request::GetMission {
                    mission_id: visible
                },
                &state
            )
            .is_ok()
        );
        assert!(matches!(
            authorize_request(
                &authority,
                &Request::GetMission { mission_id: hidden },
                &state
            ),
            Err(RequestError::ShareDenied)
        ));
        assert!(matches!(
            authorize_request(
                &authority,
                &Request::CreateMission {
                    mission_id: MissionId::new(),
                    intent: "denied".to_owned(),
                    created_by: Actor::human("guest").expect("guest Actor should be valid"),
                },
                &state,
            ),
            Err(RequestError::ShareDenied)
        ));
        let filtered = filter_response(
            &authority,
            handle_request(Request::ListMissions, Uuid::new_v4(), &state)
                .await
                .expect("owner listing should load"),
            &state,
        )
        .expect("observer response should filter");
        assert!(matches!(
            filtered,
            ResponseBody::Missions { missions }
                if missions.len() == 1 && missions[0].id == visible
        ));

        let mut revocations = state.share_revocations.subscribe();
        handle_request(
            Request::RevokeShare {
                share_id: share.share_id,
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("owner should revoke share");
        let revoked = revocations.recv().await.expect("revocation should publish");
        assert!(share_was_revoked(&authority, Ok(revoked)));
        assert!(state.shares.lock().await.authenticate(&token).is_err());

        let session_id = SessionId::new();
        state
            .terminals
            .write()
            .expect("terminal registry should lock")
            .insert(
                session_id,
                TerminalRecord {
                    handle: None,
                    projection: Arc::new(RwLock::new(TerminalProjection {
                        summary: TerminalSessionSummary {
                            session_id,
                            mission_id: None,
                            run_id: None,
                            process_id: None,
                            foreground_process: None,
                            tty_name: None,
                            status: TerminalSessionStatus::Exited,
                            archived: false,
                            latest_sequence: 0,
                            controller_client_id: None,
                            controller_surface_id: None,
                            controller_share_id: None,
                            control_epoch: 4,
                            cwd: None,
                        },
                        frame: None,
                        final_event: None,
                    })),
                },
            );
        let (controller, controller_token) = match handle_request(
            Request::CreateShare {
                label: "pair controller".to_owned(),
                role: ShareRole::Controller,
                mission_ids: Vec::new(),
                session_ids: vec![session_id],
                expires_in_seconds: 3_600,
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("controller Share should be created")
        {
            ResponseBody::ShareCreated { share, token } => (share, token),
            body => panic!("unexpected controller response: {body:?}"),
        };
        let controller_authority = ClientAuthority::Shared(
            state
                .shares
                .lock()
                .await
                .authenticate(&controller_token)
                .expect("controller token should authenticate"),
        );
        assert!(
            authorize_request(
                &controller_authority,
                &Request::TerminalFocus {
                    session_id,
                    focused: true,
                },
                &state,
            )
            .is_ok()
        );
        assert!(matches!(
            authorize_request(
                &controller_authority,
                &Request::ClaimTerminalControl {
                    session_id,
                    force: true,
                },
                &state,
            ),
            Err(RequestError::ShareDenied)
        ));
        assert!(matches!(
            authorize_request(
                &controller_authority,
                &Request::KillTerminal { session_id },
                &state,
            ),
            Err(RequestError::ShareDenied)
        ));
        {
            let record = terminal_record(&state, session_id).expect("terminal should exist");
            let mut projection = record.projection.write().expect("projection should lock");
            projection.summary.controller_client_id = Some(Uuid::new_v4());
            projection.summary.controller_surface_id = Some(Uuid::new_v4());
            projection.summary.controller_share_id = Some(controller.share_id);
        }
        handle_request(
            Request::RevokeShare {
                share_id: controller.share_id,
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("controller Share should revoke");
        let record = terminal_record(&state, session_id).expect("terminal should remain");
        let projection = record.projection.read().expect("projection should lock");
        assert_eq!(projection.summary.controller_client_id, None);
        assert_eq!(projection.summary.controller_surface_id, None);
        assert_eq!(projection.summary.controller_share_id, None);
        assert_eq!(projection.summary.control_epoch, 5);

        drop(state);
        std::fs::remove_dir_all(root).expect("isolated share state should be removable");
    }

    #[tokio::test]
    async fn runtime_diagnostics_are_redacted_and_report_resource_counts() {
        let root = std::env::temp_dir().join(format!("superplexr-status-{}", SessionId::new()));
        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(3),
            agent_connections: AtomicUsize::new(1),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });

        let settings = SchedulerSettings {
            global_max_concurrency: 7,
        };
        assert!(matches!(
            handle_request(
                Request::SetSchedulerSettings { settings },
                uuid::Uuid::new_v4(),
                &state,
            )
            .await
            .expect("valid scheduler settings should persist"),
            ResponseBody::SchedulerSettings { settings: persisted } if persisted == settings
        ));
        assert!(
            handle_request(
                Request::SetSchedulerSettings {
                    settings: SchedulerSettings {
                        global_max_concurrency: 0,
                    },
                },
                uuid::Uuid::new_v4(),
                &state,
            )
            .await
            .is_err(),
            "invalid settings must be rejected"
        );
        assert!(matches!(
            handle_request(
                Request::GetSchedulerSettings,
                uuid::Uuid::new_v4(),
                &state,
            )
            .await
            .expect("scheduler settings should be readable"),
            ResponseBody::SchedulerSettings { settings: persisted } if persisted == settings
        ));

        let response = runtime_diagnostics(&state)
            .await
            .expect("diagnostics should be available");
        let ResponseBody::RuntimeDiagnostics { diagnostics } = response else {
            panic!("diagnostics request returned a different response");
        };
        assert_eq!(diagnostics.protocol_version, PROTOCOL_VERSION);
        assert_eq!(
            diagnostics.wire_profile,
            "v3-json-control-protobuf-terminal-zstd-multiplexed"
        );
        assert_eq!(diagnostics.open_connections, 3);
        assert_eq!(diagnostics.agent_connections, 1);
        assert_eq!(diagnostics.missions, 0);
        assert_eq!(diagnostics.scheduler_policies, 0);
        assert_eq!(diagnostics.scheduler_policies_enabled, 0);
        assert_eq!(diagnostics.scheduler_global_limit, 7);
        assert_eq!(diagnostics.scheduler_global_occupied, 0);
        assert_eq!(diagnostics.terminals_total, 0);
        assert!(!diagnostics.platform.is_empty());
        assert!(!diagnostics.architecture.is_empty());

        drop(state);
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[tokio::test]
    async fn agent_peer_credentials_bind_to_one_live_run_and_revoke_on_finish() {
        let root = std::env::temp_dir().join(format!("superplexr-agent-auth-{}", SessionId::new()));
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let session_id = SessionId::new();
        let mut store = MissionStore::open(root.join("missions"))
            .await
            .expect("mission store should open");
        store
            .create(
                mission_id,
                "Authenticate one agent process group".to_owned(),
                Actor::human("operator").expect("operator should be valid"),
            )
            .await
            .expect("mission should be created");
        let key = uuid::Uuid::new_v4();
        store
            .dispatch(
                mission_id,
                Command::StartRun {
                    run_id,
                    parent: None,
                    actor: Actor::agent("worker", "fixture").expect("agent should be valid"),
                    objective: "Exercise agent authorization".to_owned(),
                },
                None,
                key,
                key,
            )
            .await
            .expect("run should start");
        // SAFETY: getpgrp has no preconditions and only reads process metadata.
        let process_group_id = unsafe { libc::getpgrp() };
        let summary = TerminalSessionSummary {
            session_id,
            mission_id: Some(mission_id),
            run_id: Some(run_id),
            process_id: u32::try_from(process_group_id).ok(),
            foreground_process: None,
            tty_name: None,
            status: TerminalSessionStatus::Running,
            archived: false,
            latest_sequence: 0,
            controller_client_id: None,
            controller_surface_id: None,
            controller_share_id: None,
            control_epoch: 0,
            cwd: None,
        };
        let state = Arc::new(AppState {
            store: Mutex::new(store),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::from([(
                session_id,
                TerminalRecord {
                    handle: None,
                    projection: Arc::new(RwLock::new(TerminalProjection {
                        summary,
                        frame: None,
                        final_event: None,
                    })),
                },
            )])),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        let (peer, server) = UnixStream::pair().expect("Unix pair should open");
        let identity = authenticate_agent_peer(&server, &state)
            .expect("matching peer process group should authenticate");
        assert_eq!(identity.mission_id, mission_id);
        assert_eq!(identity.run_id, run_id);
        assert!(
            agent_identity_is_active(identity, &state)
                .await
                .expect("active state should be readable")
        );

        let key = uuid::Uuid::new_v4();
        state
            .store
            .lock()
            .await
            .dispatch(
                mission_id,
                Command::FinishRun {
                    run_id,
                    outcome: FinishOutcome::Succeeded,
                    summary: "done".to_owned(),
                },
                None,
                key,
                key,
            )
            .await
            .expect("run should finish");
        assert!(
            !agent_identity_is_active(identity, &state)
                .await
                .expect("revoked state should be readable")
        );

        drop(peer);
        drop(server);
        drop(state);
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[tokio::test]
    async fn recovery_fails_only_unfinished_agent_bindings() {
        let root = std::env::temp_dir().join(format!("superplexr-recovery-{}", SessionId::new()));
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let session_id = SessionId::new();
        let actor = Actor::agent("recovered", "fixture").expect("agent should be valid");
        let mut store = MissionStore::open(root.join("missions"))
            .await
            .expect("mission store should open");
        store
            .create(
                mission_id,
                "Recover an interrupted agent".to_owned(),
                Actor::human("operator").expect("operator should be valid"),
            )
            .await
            .expect("mission should be created");
        let plan_key = uuid::Uuid::new_v4();
        store
            .dispatch(
                mission_id,
                Command::PlanRun {
                    run_id,
                    parent: None,
                    dependencies: Vec::new(),
                    retry_of: None,
                    actor: actor.clone(),
                    objective: "Run until runtime interruption".to_owned(),
                    priority: RunPriority::Normal,
                },
                None,
                plan_key,
                plan_key,
            )
            .await
            .expect("run should be planned");
        let launch_key = uuid::Uuid::new_v4();
        store
            .dispatch_batch(
                mission_id,
                vec![
                    Command::ResolveRunDriver {
                        run_id,
                        snapshot: RunDriverSnapshot {
                            driver_id: "fixture".to_owned(),
                            profile_version: 1,
                            process_spec_sha256: "0".repeat(64),
                            argument_count: 0,
                            environment_keys: Vec::new(),
                            sandbox_backend: None,
                            sandbox_profile: None,
                            sandbox_network_isolated: false,
                        },
                    },
                    Command::StartReadyRun { run_id },
                    Command::StartSession {
                        session_id,
                        name: "interrupted".to_owned(),
                        started_by: actor,
                    },
                    Command::AssignSession { session_id, run_id },
                ],
                None,
                launch_key,
                launch_key,
            )
            .await
            .expect("agent binding should commit");
        let state = Arc::new(AppState {
            store: Mutex::new(store),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::from([(
                session_id,
                TerminalRecord {
                    handle: None,
                    projection: Arc::new(RwLock::new(TerminalProjection {
                        summary: TerminalSessionSummary {
                            session_id,
                            mission_id: Some(mission_id),
                            run_id: Some(run_id),
                            process_id: None,
                            foreground_process: None,
                            tty_name: None,
                            status: TerminalSessionStatus::Exited,
                            archived: false,
                            latest_sequence: 0,
                            controller_client_id: None,
                            controller_surface_id: None,
                            controller_share_id: None,
                            control_epoch: 0,
                            cwd: None,
                        },
                        frame: None,
                        final_event: None,
                    })),
                },
            )])),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });

        assert_eq!(reconcile_recovered_agent_runs(&state).await, 1);
        let mission = state
            .store
            .lock()
            .await
            .get(mission_id)
            .expect("mission should remain readable");
        assert_eq!(mission.runs[&run_id].status, RunStatus::Failed);
        assert!(mission.sessions[&session_id].status.is_finished());
        drop(state);
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[tokio::test]
    async fn configured_scheduler_enforces_slots_snapshots_drivers_and_reports_failures() {
        let root = std::env::temp_dir().join(format!("superplexr-scheduler-{}", SessionId::new()));
        let mission_id = MissionId::new();
        let first_run = RunId::new();
        let second_run = RunId::new();
        let missing_run = RunId::new();
        let mut store = MissionStore::open(root.join("missions"))
            .await
            .expect("mission store should open");
        store
            .create(
                mission_id,
                "Run agents under a deterministic slot policy".to_owned(),
                Actor::human("operator").expect("operator should be valid"),
            )
            .await
            .expect("mission should be created");
        for (run_id, engine, priority) in [
            (first_run, "fixture", RunPriority::Urgent),
            (second_run, "fixture", RunPriority::Normal),
            (missing_run, "not-configured", RunPriority::Background),
        ] {
            let key = uuid::Uuid::new_v4();
            store
                .dispatch(
                    mission_id,
                    Command::PlanRun {
                        run_id,
                        parent: None,
                        dependencies: Vec::new(),
                        retry_of: None,
                        actor: Actor::agent(format!("worker-{run_id}"), engine)
                            .expect("agent should be valid"),
                        objective: format!("Execute {run_id}"),
                        priority,
                    },
                    None,
                    key,
                    key,
                )
                .await
                .expect("run should be planned");
        }
        std::fs::create_dir_all(&root).expect("state root should exist");
        std::fs::write(
            root.join("engines.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "drivers": {
                    "fixture": {
                        "program": "/bin/sh",
                        "args": ["-c", "sleep 0.4; printf 'scheduled-agent\\n'"],
                        "environment_delta": {"FIXTURE_MODE": "scheduler"}
                    }
                }
            }))
            .expect("driver config should serialize"),
        )
        .expect("driver config should write");
        let state = Arc::new(AppState {
            store: Mutex::new(store),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        let request_id = uuid::Uuid::new_v4();
        let request = || Request::LaunchConfiguredSchedulerBatch {
            mission_id,
            max_concurrency: 1,
            session_name_prefix: "scheduled".to_owned(),
            cwd: std::env::current_dir().expect("current directory should resolve"),
            grid: GridSize::new(40, 6).expect("valid grid"),
        };
        let first = handle_request_with_context(
            request(),
            RequestContext {
                client_id: uuid::Uuid::new_v4(),
                surface_id: None,
                control_epoch: None,
                share_id: None,
                request_id,
                expected_mission_version: None,
                agent_identity: None,
            },
            &state,
        )
        .await
        .expect("first scheduled run should launch");
        let first_session = match first {
            ResponseBody::ConfiguredSchedulerBatchLaunched {
                plan,
                launched,
                failures,
                ..
            } => {
                assert_eq!(plan.startable.len(), 1);
                assert_eq!(plan.startable[0].run_id, first_run);
                assert_eq!(launched.len(), 1);
                assert!(failures.is_empty());
                assert_eq!(launched[0].events_committed, 5);
                launched[0].terminal.session_id
            }
            body => panic!("scheduler returned {body:?}"),
        };
        let mission = state
            .store
            .lock()
            .await
            .get(mission_id)
            .expect("mission should be readable");
        let snapshot = mission.runs[&first_run]
            .driver_snapshot
            .as_ref()
            .expect("launch should snapshot its driver");
        assert_eq!(snapshot.driver_id, "fixture");
        assert_eq!(snapshot.profile_version, engine_driver::CONFIG_VERSION);
        assert_eq!(snapshot.argument_count, 2);
        assert_eq!(snapshot.environment_keys, ["FIXTURE_MODE"]);
        assert!(snapshot.sandbox_backend.is_none());
        assert!(snapshot.sandbox_profile.is_none());
        assert_eq!(snapshot.process_spec_sha256.len(), 64);
        let harness = mission
            .verified_delivery
            .harness_snapshots
            .get(&first_run)
            .expect("launch should snapshot its execution harness");
        assert_eq!(&harness.driver, snapshot);
        assert_eq!(harness.objective_sha256.len(), 64);
        assert_eq!(harness.context_sha256.len(), 64);
        drop(mission);

        let while_occupied = handle_request_with_context(
            request(),
            RequestContext {
                client_id: uuid::Uuid::new_v4(),
                surface_id: None,
                control_epoch: None,
                share_id: None,
                request_id,
                expected_mission_version: None,
                agent_identity: None,
            },
            &state,
        )
        .await
        .expect("reconciling while occupied should be safe");
        assert!(matches!(
            while_occupied,
            ResponseBody::ConfiguredSchedulerBatchLaunched {
                plan,
                launched,
                failures,
                ..
            } if plan.occupied_slots == 1 && launched.is_empty() && failures.is_empty()
        ));
        assert_eq!(
            state
                .terminals
                .read()
                .expect("terminal registry should be readable")
                .len(),
            1,
            "a repeated scheduler request must not duplicate a running Run"
        );

        wait_for_run_status(&state, mission_id, first_run, RunStatus::Succeeded).await;
        let second = handle_request(request(), uuid::Uuid::new_v4(), &state)
            .await
            .expect("second scheduled run should launch after capacity frees");
        assert!(matches!(
            second,
            ResponseBody::ConfiguredSchedulerBatchLaunched { launched, .. }
                if launched.len() == 1 && launched[0].run_id == second_run
        ));
        wait_for_run_status(&state, mission_id, second_run, RunStatus::Succeeded).await;

        let missing = handle_request(request(), uuid::Uuid::new_v4(), &state)
            .await
            .expect("driver resolution failure should be reported in-band");
        assert!(matches!(
            missing,
            ResponseBody::ConfiguredSchedulerBatchLaunched {
                launched,
                failures,
                ..
            } if launched.is_empty()
                && failures.len() == 1
                && failures[0].run_id == missing_run
                && failures[0].message.contains("not-configured")
        ));
        let mission = state
            .store
            .lock()
            .await
            .get(mission_id)
            .expect("mission should remain readable");
        assert_eq!(mission.runs[&missing_run].status, RunStatus::Pending);
        assert!(mission.runs[&missing_run].driver_snapshot.is_none());

        assert!(terminal_record(&state, first_session).is_ok());
        drop(state);
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[tokio::test]
    async fn concurrent_agent_launches_cannot_race_past_global_admission() {
        let root = std::env::temp_dir().join(format!("superplexr-admission-{}", SessionId::new()));
        let mission_id = MissionId::new();
        let run_ids = [RunId::new(), RunId::new()];
        let mut store = MissionStore::open(root.join("missions"))
            .await
            .expect("mission store should open");
        store
            .create(
                mission_id,
                "Serialize global agent admission".to_owned(),
                Actor::human("operator").expect("operator should be valid"),
            )
            .await
            .expect("mission should be created");
        for run_id in run_ids {
            let key = uuid::Uuid::new_v4();
            store
                .dispatch(
                    mission_id,
                    Command::PlanRun {
                        run_id,
                        parent: None,
                        dependencies: Vec::new(),
                        retry_of: None,
                        actor: Actor::agent(format!("admission-{run_id}"), "direct-command")
                            .expect("agent should be valid"),
                        objective: format!("Admission test {run_id}"),
                        priority: RunPriority::Normal,
                    },
                    None,
                    key,
                    key,
                )
                .await
                .expect("run should be planned");
        }
        let state = Arc::new(AppState {
            store: Mutex::new(store),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        let make_spec = |run_id| TerminalSessionSpec {
            session_id: SessionId::new(),
            mission_id: Some(mission_id),
            run_id: Some(run_id),
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_owned(), "sleep 0.4".to_owned()],
            cwd: std::env::current_dir().expect("current directory should resolve"),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(40, 6).expect("valid grid"),
        };
        let first_spec = make_spec(run_ids[0]);
        let second_spec = make_spec(run_ids[1]);
        let first_retry_spec = first_spec.clone();
        let second_retry_spec = second_spec.clone();
        let first_snapshot = driver_snapshot("direct-command", 0, &first_spec, None)
            .expect("snapshot should resolve");
        let second_snapshot = driver_snapshot("direct-command", 0, &second_spec, None)
            .expect("snapshot should resolve");
        let first_retry_snapshot = first_snapshot.clone();
        let second_retry_snapshot = second_snapshot.clone();
        let (first, second) = tokio::join!(
            launch_agent_run(
                mission_id,
                run_ids[0],
                "first admission".to_owned(),
                first_spec,
                first_snapshot,
                uuid::Uuid::new_v4(),
                None,
                None,
                uuid::Uuid::new_v4(),
                1,
                &state,
            ),
            launch_agent_run(
                mission_id,
                run_ids[1],
                "second admission".to_owned(),
                second_spec,
                second_snapshot,
                uuid::Uuid::new_v4(),
                None,
                None,
                uuid::Uuid::new_v4(),
                1,
                &state,
            )
        );
        let (winner, loser, loser_spec, loser_snapshot) = match (first, second) {
            (Ok(_), Err(error)) => {
                assert!(matches!(
                    error,
                    RequestError::GlobalAgentConcurrencyLimit(1)
                ));
                (
                    run_ids[0],
                    run_ids[1],
                    second_retry_spec,
                    second_retry_snapshot,
                )
            }
            (Err(error), Ok(_)) => {
                assert!(matches!(
                    error,
                    RequestError::GlobalAgentConcurrencyLimit(1)
                ));
                (
                    run_ids[1],
                    run_ids[0],
                    first_retry_spec,
                    first_retry_snapshot,
                )
            }
            outcomes => panic!("exactly one launch should be admitted: {outcomes:?}"),
        };
        assert_eq!(
            running_agent_terminals(&state).expect("count should work"),
            1
        );
        assert_eq!(
            state
                .terminals
                .read()
                .expect("terminal registry should be readable")
                .len(),
            1
        );
        wait_for_run_status(&state, mission_id, winner, RunStatus::Succeeded).await;
        launch_agent_run(
            mission_id,
            loser,
            "retry admission".to_owned(),
            loser_spec,
            loser_snapshot,
            uuid::Uuid::new_v4(),
            None,
            None,
            uuid::Uuid::new_v4(),
            1,
            &state,
        )
        .await
        .expect("capacity release should admit the pending Run");
        wait_for_run_status(&state, mission_id, loser, RunStatus::Succeeded).await;

        drop(state);
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[tokio::test]
    async fn continuous_scheduler_persists_disable_and_resumes_after_runtime_restart() {
        let root = std::env::temp_dir().join(format!("superplexr-auto-{}", SessionId::new()));
        let mission_id = MissionId::new();
        let first_run = RunId::new();
        let second_run = RunId::new();
        let restart_run = RunId::new();
        let missing_run = RunId::new();
        let mut store = MissionStore::open(root.join("missions"))
            .await
            .expect("mission store should open");
        store
            .create(
                mission_id,
                "Continuously reconcile configured agents".to_owned(),
                Actor::human("operator").expect("operator should be valid"),
            )
            .await
            .expect("mission should be created");
        for (run_id, priority) in [
            (first_run, RunPriority::Urgent),
            (second_run, RunPriority::Normal),
        ] {
            let key = uuid::Uuid::new_v4();
            store
                .dispatch(
                    mission_id,
                    Command::PlanRun {
                        run_id,
                        parent: None,
                        dependencies: Vec::new(),
                        retry_of: None,
                        actor: Actor::agent(format!("auto-{run_id}"), "fixture")
                            .expect("agent should be valid"),
                        objective: format!("Automatically execute {run_id}"),
                        priority,
                    },
                    None,
                    key,
                    key,
                )
                .await
                .expect("run should be planned");
        }
        std::fs::write(
            root.join("engines.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "drivers": {
                    "fixture": {
                        "program": "/bin/sh",
                        "args": ["-c", "sleep 0.4; printf 'continuous-agent\\n'"],
                        "environment_delta": {}
                    }
                }
            }))
            .expect("driver config should serialize"),
        )
        .expect("driver config should write");
        let state = Arc::new(AppState {
            store: Mutex::new(store),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        let enabled = SchedulerPolicy {
            mission_id,
            enabled: true,
            max_concurrency: 1,
            session_name_prefix: "continuous".to_owned(),
            cwd: std::env::current_dir().expect("current directory should resolve"),
            grid: GridSize::new(40, 6).expect("valid grid"),
        };
        let response = handle_request(
            Request::SetSchedulerPolicy {
                policy: enabled.clone(),
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("policy should be enabled");
        assert!(matches!(
            response,
            ResponseBody::SchedulerPolicy { policy } if policy == enabled
        ));
        let task = tokio::spawn(reconcile_scheduler_policies(Arc::clone(&state)));
        wait_for_run_status(&state, mission_id, first_run, RunStatus::Running).await;
        assert_eq!(
            state
                .terminals
                .read()
                .expect("terminal registry should be readable")
                .len(),
            1
        );

        let mut disabled = enabled.clone();
        disabled.enabled = false;
        handle_request(
            Request::SetSchedulerPolicy {
                policy: disabled.clone(),
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("policy should disable");
        wait_for_run_status(&state, mission_id, first_run, RunStatus::Succeeded).await;
        tokio::time::sleep(Duration::from_millis(650)).await;
        assert_eq!(
            state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("mission should be readable")
                .runs[&second_run]
                .status,
            RunStatus::Pending,
            "disabled policy must not consume newly open capacity"
        );

        handle_request(
            Request::SetSchedulerPolicy {
                policy: enabled.clone(),
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("policy should re-enable");
        wait_for_run_status(&state, mission_id, second_run, RunStatus::Succeeded).await;
        task.abort();
        let _ = task.await;
        plan_test_agent(&state, mission_id, restart_run, "fixture").await;
        drop(state);

        let recovered = recover_terminal_history(&root).expect("terminal history should recover");
        let recovered_state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should reopen"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should reopen"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should reopen"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(recovered),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        let recovered_policies = recovered_state.scheduler_policies.lock().await.list();
        assert_eq!(
            recovered_policies.as_slice(),
            std::slice::from_ref(&enabled)
        );
        assert_eq!(
            reconcile_recovered_agent_runs(&recovered_state).await,
            0,
            "finished Run/Session bindings must not be finalized again on restart"
        );
        let recovered_task =
            tokio::spawn(reconcile_scheduler_policies(Arc::clone(&recovered_state)));
        wait_for_run_status(
            &recovered_state,
            mission_id,
            restart_run,
            RunStatus::Succeeded,
        )
        .await;

        plan_test_agent(&recovered_state, mission_id, missing_run, "missing").await;
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let mission = recovered_state
            .store
            .lock()
            .await
            .get(mission_id)
            .expect("mission should remain readable");
        assert_eq!(mission.runs[&missing_run].status, RunStatus::Pending);
        assert!(mission.runs[&missing_run].driver_snapshot.is_none());
        drop(mission);
        assert_eq!(
            recovered_state
                .terminals
                .read()
                .expect("terminal registry should be readable")
                .len(),
            3,
            "repeated bad-driver reconciliation must not create a terminal"
        );
        handle_request(
            Request::SetSchedulerPolicy { policy: disabled },
            uuid::Uuid::new_v4(),
            &recovered_state,
        )
        .await
        .expect("recovered policy should disable");
        recovered_task.abort();
        let _ = recovered_task.await;
        drop(recovered_state);

        assert!(
            !SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                .expect("policy should remain readable")
                .list()[0]
                .enabled
        );
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[tokio::test]
    async fn continuous_scheduler_enforces_global_capacity_with_mission_fairness() {
        let root = std::env::temp_dir().join(format!("superplexr-global-{}", SessionId::new()));
        let first_mission = MissionId::new();
        let second_mission = MissionId::new();
        let first_runs = [RunId::new(), RunId::new()];
        let second_runs = [RunId::new(), RunId::new()];
        let mut store = MissionStore::open(root.join("missions"))
            .await
            .expect("mission store should open");
        for (mission_id, runs) in [
            (first_mission, first_runs.as_slice()),
            (second_mission, second_runs.as_slice()),
        ] {
            store
                .create(
                    mission_id,
                    format!("Fairly schedule Mission {mission_id}"),
                    Actor::human("operator").expect("operator should be valid"),
                )
                .await
                .expect("mission should be created");
            for run_id in runs {
                let key = uuid::Uuid::new_v4();
                store
                    .dispatch(
                        mission_id,
                        Command::PlanRun {
                            run_id: *run_id,
                            parent: None,
                            dependencies: Vec::new(),
                            retry_of: None,
                            actor: Actor::agent(format!("fair-{run_id}"), "fixture")
                                .expect("agent should be valid"),
                            objective: format!("Fair work {run_id}"),
                            priority: RunPriority::Normal,
                        },
                        None,
                        key,
                        key,
                    )
                    .await
                    .expect("run should be planned");
            }
        }
        std::fs::write(
            root.join("engines.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "drivers": {
                    "fixture": {
                        "program": "/bin/sh",
                        "args": ["-c", "sleep 0.3; printf 'globally-bounded\\n'"],
                        "environment_delta": {}
                    }
                }
            }))
            .expect("driver config should serialize"),
        )
        .expect("driver config should write");
        let state = Arc::new(AppState {
            store: Mutex::new(store),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        for mission_id in [first_mission, second_mission] {
            handle_request(
                Request::SetSchedulerPolicy {
                    policy: SchedulerPolicy {
                        mission_id,
                        enabled: true,
                        max_concurrency: 2,
                        session_name_prefix: "fair".to_owned(),
                        cwd: std::env::current_dir().expect("current directory should resolve"),
                        grid: GridSize::new(40, 6).expect("valid grid"),
                    },
                },
                uuid::Uuid::new_v4(),
                &state,
            )
            .await
            .expect("policy should persist");
        }

        let maximum_running = Arc::new(AtomicUsize::new(0));
        let monitor_state = Arc::clone(&state);
        let monitor_maximum = Arc::clone(&maximum_running);
        let monitor = tokio::spawn(async move {
            loop {
                let running = usize::from(
                    running_agent_terminals(&monitor_state)
                        .expect("running terminal count should remain readable"),
                );
                monitor_maximum.fetch_max(running, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        state
            .scheduler_policies
            .lock()
            .await
            .set_settings(superplexr_protocol::SchedulerSettings {
                global_max_concurrency: 1,
            })
            .expect("global scheduler settings should persist");
        let reconciler = tokio::spawn(reconcile_scheduler_policies(Arc::clone(&state)));
        tokio::time::sleep(Duration::from_millis(700)).await;
        for (mission_id, runs) in [
            (first_mission, first_runs.as_slice()),
            (second_mission, second_runs.as_slice()),
        ] {
            let mission = state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("mission should be readable");
            assert_eq!(
                runs.iter()
                    .filter(|run_id| mission.runs[run_id].status != RunStatus::Pending)
                    .count(),
                1,
                "round-robin order should give each Mission one turn before a second"
            );
        }
        for run_id in first_runs {
            wait_for_run_status(&state, first_mission, run_id, RunStatus::Succeeded).await;
        }
        for run_id in second_runs {
            wait_for_run_status(&state, second_mission, run_id, RunStatus::Succeeded).await;
        }
        reconciler.abort();
        monitor.abort();
        let _ = reconciler.await;
        let _ = monitor.await;
        assert_eq!(
            maximum_running.load(Ordering::Relaxed),
            1,
            "global cap must hold across Mission policies"
        );

        drop(state);
        assert_eq!(
            SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                .expect("scheduler settings should reopen")
                .settings()
                .global_max_concurrency,
            1
        );
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    async fn plan_test_agent(
        state: &Arc<AppState>,
        mission_id: MissionId,
        run_id: RunId,
        engine: &str,
    ) {
        let key = uuid::Uuid::new_v4();
        state
            .store
            .lock()
            .await
            .dispatch(
                mission_id,
                Command::PlanRun {
                    run_id,
                    parent: None,
                    dependencies: Vec::new(),
                    retry_of: None,
                    actor: Actor::agent(format!("auto-{run_id}"), engine)
                        .expect("agent should be valid"),
                    objective: format!("Automatically execute {run_id}"),
                    priority: RunPriority::Normal,
                },
                None,
                key,
                key,
            )
            .await
            .expect("run should be planned");
    }

    async fn wait_for_run_status(
        state: &Arc<AppState>,
        mission_id: MissionId,
        run_id: RunId,
        expected: RunStatus,
    ) {
        for _ in 0..200 {
            let status = state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("mission should be readable")
                .runs[&run_id]
                .status;
            if status == expected {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
            tokio::task::yield_now().await;
        }
        panic!("run {run_id} did not reach {expected:?}");
    }

    #[tokio::test]
    async fn agent_launch_binds_one_terminal_and_finishes_the_run_from_process_exit() {
        let root = std::env::temp_dir().join(format!("superplexr-agent-{}", SessionId::new()));
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        let session_id = SessionId::new();
        let mut store = MissionStore::open(root.join("missions"))
            .await
            .expect("mission store should open");
        store
            .create(
                mission_id,
                "Exercise the agent launch saga".to_owned(),
                Actor::human("operator").expect("operator should be valid"),
            )
            .await
            .expect("mission should be created");
        let key = uuid::Uuid::new_v4();
        store
            .dispatch(
                mission_id,
                Command::PlanRun {
                    run_id,
                    parent: None,
                    dependencies: Vec::new(),
                    retry_of: None,
                    actor: Actor::agent("worker", "generic-command")
                        .expect("agent should be valid"),
                    objective: "Print a durable result".to_owned(),
                    priority: RunPriority::Normal,
                },
                None,
                key,
                key,
            )
            .await
            .expect("run should be planned");
        let key = uuid::Uuid::new_v4();
        store
            .dispatch(
                mission_id,
                Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::DeclareChangeIntent {
                        run_id,
                        expected_version: None,
                        spec: ChangeIntentSpec {
                            repository_identity: "launch-fixture".to_owned(),
                            base_revision: "abc123".to_owned(),
                            claims: vec![ChangeClaim {
                                path: "src/main.rs".to_owned(),
                                operation: ChangeOperation::Modify,
                                scope: ChangeScope::Committed,
                            }],
                        },
                    },
                },
                None,
                key,
                key,
            )
            .await
            .expect("change intent should be proposed before launch");
        let state = Arc::new(AppState {
            store: Mutex::new(store),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        std::fs::write(
            root.join("engines.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "drivers": {
                    "generic-command": {
                        "program": "/bin/sh",
                        "args": [
                            "-c",
                            format!(
                                "test \"$SUPERPLEXR_MISSION_ID\" = '{mission_id}' && \
                                 test \"$SUPERPLEXR_RUN_ID\" = '{run_id}' && \
                                 test \"$SUPERPLEXR_SESSION_ID\" = '{session_id}' && \
                                 test -n \"$SUPERPLEXR_EXECUTION_LEASE_EPOCH\" && \
                                printf 'agent-result\\n'"
                            )
                        ],
                        "environment_delta": {
                            "CONFIGURED_ONLY": "not-in-preview"
                        }
                    }
                }
            }))
            .expect("driver config should serialize"),
        )
        .expect("driver config should write");
        let repository = root.with_extension("repository");
        std::fs::create_dir(&repository).expect("fixture repository should create");
        for arguments in [
            vec!["init", "-q"],
            vec!["config", "user.name", "superplexr fixture"],
            vec!["config", "user.email", "fixture@superplexr.invalid"],
        ] {
            let status = std::process::Command::new("git")
                .args(arguments)
                .current_dir(&repository)
                .status()
                .expect("fixture Git command should run");
            assert!(status.success());
        }
        std::fs::write(repository.join("README.md"), "fixture\n")
            .expect("fixture file should write");
        for arguments in [vec!["add", "."], vec!["commit", "-qm", "base"]] {
            let status = std::process::Command::new("git")
                .args(arguments)
                .current_dir(&repository)
                .status()
                .expect("fixture Git command should run");
            assert!(status.success());
        }
        let prepared_checkout = handle_request(
            Request::PrepareRunCheckout {
                mission_id,
                run_id,
                repository,
                base_ref: "HEAD".to_owned(),
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("writable Run checkout should prepare");
        let ResponseBody::RunCheckout { checkout } = prepared_checkout else {
            panic!("checkout preparation returned an unexpected response")
        };
        handle_request(
            Request::Dispatch {
                mission_id,
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::DeclareChangeIntent {
                        run_id,
                        expected_version: Some(1),
                        spec: ChangeIntentSpec {
                            repository_identity: "launch-fixture".to_owned(),
                            base_revision: checkout.base_revision,
                            claims: vec![ChangeClaim {
                                path: "src/main.rs".to_owned(),
                                operation: ChangeOperation::Modify,
                                scope: ChangeScope::Committed,
                            }],
                        },
                    },
                },
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("intent base should bind to the prepared checkout");

        let version_before_preview = state
            .store
            .lock()
            .await
            .get(mission_id)
            .expect("mission should be readable")
            .version;
        let preview = handle_request(
            Request::PreviewConfiguredAgentRun {
                mission_id,
                run_id,
                cwd: std::env::current_dir().expect("current directory should resolve"),
                use_run_checkout: false,
                grid: GridSize::new(40, 6).expect("valid grid"),
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("configured driver should be previewable");
        assert!(matches!(
            preview,
            ResponseBody::ConfiguredAgentLaunchPreview {
                preview: ConfiguredAgentLaunchPreview {
                    engine,
                    environment_keys,
                    ..
                }
            } if engine == "generic-command"
                && environment_keys == ["CONFIGURED_ONLY"]
        ));
        assert_eq!(
            state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("mission should remain readable")
                .version,
            version_before_preview,
            "preview must not mutate Mission state"
        );

        let launched = handle_request(
            Request::LaunchConfiguredAgentRun {
                mission_id,
                run_id,
                session_id,
                session_name: "generic command".to_owned(),
                cwd: std::env::current_dir().expect("current directory should resolve"),
                use_run_checkout: false,
                grid: GridSize::new(40, 6).expect("valid grid"),
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("ready run should launch");
        assert!(matches!(
            launched,
            ResponseBody::AgentRunLaunched {
                terminal: TerminalSessionSummary {
                    mission_id: Some(bound_mission),
                    run_id: Some(bound_run),
                    ..
                },
                ..
            } if bound_mission == mission_id && bound_run == run_id
        ));
        let mission_after_launch = state
            .store
            .lock()
            .await
            .get(mission_id)
            .expect("mission should remain readable");
        let admitted = &mission_after_launch.verified_delivery.change_intents[&run_id];
        assert_eq!(admitted.state, ChangeIntentState::Admitted);
        assert!(admitted.lease_epoch > 0);
        let harness = &mission_after_launch.verified_delivery.harness_snapshots[&run_id];
        assert_eq!(
            Some(&harness.driver),
            mission_after_launch.runs[&run_id].driver_snapshot.as_ref()
        );

        let mut completed = false;
        for _ in 0..100 {
            let mission = state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("mission should remain readable");
            if mission.runs[&run_id].status == RunStatus::Succeeded
                && mission.sessions[&session_id].status.is_finished()
            {
                completed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            tokio::task::yield_now().await;
        }
        if !completed {
            let mission = state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("mission should remain readable");
            let terminal_status = terminal_record(&state, session_id)
                .expect("terminal should remain indexed")
                .projection
                .read()
                .expect("projection should remain readable")
                .summary
                .status;
            panic!(
                "process exit should finish both Run and Session: run={:?}, session={:?}, terminal={terminal_status:?}",
                mission.runs[&run_id].status, mission.sessions[&session_id].status
            );
        }
        let retried = handle_request(
            Request::LaunchAgentRun {
                mission_id,
                run_id,
                session_name: "generic command".to_owned(),
                spec: TerminalSessionSpec {
                    session_id,
                    mission_id: Some(mission_id),
                    run_id: Some(run_id),
                    program: PathBuf::from("/does/not/run/on-retry"),
                    args: Vec::new(),
                    cwd: std::env::current_dir().expect("current directory should resolve"),
                    environment_delta: BTreeMap::new(),
                    grid: GridSize::new(40, 6).expect("valid grid"),
                },
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("the same bound Session should make launch retries idempotent");
        assert!(matches!(
            retried,
            ResponseBody::AgentRunLaunched {
                events,
                mission,
                terminal: TerminalSessionSummary { session_id: id, .. },
            } if events.is_empty()
                && id == session_id
                && mission.runs[&run_id].status == RunStatus::Succeeded
        ));
        let history = handle_request(
            Request::TerminalHistoryFrame {
                session_id,
                viewport: superplexr_terminal::HistoryViewport::RowsBeforeBottom(0),
            },
            uuid::Uuid::new_v4(),
            &state,
        )
        .await
        .expect("agent output should remain retained");
        assert!(matches!(
            history,
            ResponseBody::TerminalHistoryFrame { frame, .. }
                if frame.rows.iter().any(|row| row.text().contains("agent-result"))
        ));

        drop(state);
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[tokio::test]
    async fn surface_epochs_reject_sibling_and_stale_input_after_takeover() {
        let root = std::env::temp_dir().join(format!("superplexr-lease-{}", SessionId::new()));
        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        let session_id = SessionId::new();
        let client_id = uuid::Uuid::new_v4();
        let first_surface = uuid::Uuid::new_v4();
        let second_surface = uuid::Uuid::new_v4();
        let started = handle_request_as_surface(
            Request::StartTerminal {
                spec: TerminalSessionSpec {
                    session_id,
                    mission_id: None,
                    run_id: None,
                    program: PathBuf::from("/bin/cat"),
                    args: Vec::new(),
                    cwd: std::env::current_dir().expect("current directory should resolve"),
                    environment_delta: BTreeMap::new(),
                    grid: GridSize::new(20, 4).expect("valid grid"),
                },
            },
            client_id,
            first_surface,
            None,
            &state,
        )
        .await
        .expect("terminal should start");
        assert!(matches!(
            started,
            ResponseBody::TerminalStarted {
                terminal: TerminalSessionSummary {
                    control_epoch: 1,
                    controller_surface_id: Some(id),
                    ..
                }
            } if id == first_surface
        ));

        assert!(matches!(
            handle_request_as_surface(
                Request::TerminalPaste {
                    session_id,
                    bytes: b"sibling".to_vec(),
                    confirmed: true,
                },
                client_id,
                second_surface,
                Some(1),
                &state,
            )
            .await,
            Err(RequestError::NotController { .. })
        ));

        let takeover = handle_request_as_surface(
            Request::ClaimTerminalControl {
                session_id,
                force: true,
            },
            client_id,
            second_surface,
            Some(1),
            &state,
        )
        .await
        .expect("explicit takeover should succeed");
        assert!(matches!(
            takeover,
            ResponseBody::TerminalControlChanged {
                control_epoch: 2,
                ..
            }
        ));

        handle_request_as_surface(
            Request::ReleaseTerminalControl { session_id },
            client_id,
            second_surface,
            Some(2),
            &state,
        )
        .await
        .expect("current surface should release control");
        let reclaimed = handle_request_as_surface(
            Request::ClaimTerminalControl {
                session_id,
                force: false,
            },
            client_id,
            first_surface,
            Some(1),
            &state,
        )
        .await
        .expect("unowned terminal should be claimable");
        assert!(matches!(
            reclaimed,
            ResponseBody::TerminalControlChanged {
                control_epoch: 4,
                ..
            }
        ));
        assert!(matches!(
            handle_request_as_surface(
                Request::TerminalPaste {
                    session_id,
                    bytes: b"stale".to_vec(),
                    confirmed: true,
                },
                client_id,
                first_surface,
                Some(1),
                &state,
            )
            .await,
            Err(RequestError::StaleControlEpoch {
                actual: 4,
                provided: Some(1),
                ..
            })
        ));
        handle_request_as_surface(
            Request::KillTerminal { session_id },
            client_id,
            first_surface,
            Some(4),
            &state,
        )
        .await
        .expect("current epoch should retain control");
        drop(state);
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[tokio::test]
    async fn daemon_registry_controls_and_retains_a_real_pty_session() {
        let root = std::env::temp_dir().join(format!("superplexr-server-{}", SessionId::new()));
        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        let session_id = SessionId::new();
        let client_id = uuid::Uuid::new_v4();
        let started = handle_request(
            Request::StartTerminal {
                spec: TerminalSessionSpec {
                    session_id,
                    mission_id: None,
                    run_id: None,
                    program: PathBuf::from("/bin/cat"),
                    args: Vec::new(),
                    cwd: std::env::current_dir().expect("current directory should resolve"),
                    environment_delta: BTreeMap::new(),
                    grid: GridSize::new(20, 4).expect("valid grid"),
                },
            },
            client_id,
            &state,
        )
        .await
        .expect("terminal should start");
        assert!(matches!(started, ResponseBody::TerminalStarted { .. }));
        assert!(matches!(
            handle_request(
                Request::ArchiveTerminal { session_id },
                client_id,
                &state,
            )
            .await,
            Err(RequestError::ArchiveRunningTerminal(id)) if id == session_id
        ));

        let observer_id = uuid::Uuid::new_v4();
        assert!(matches!(
            handle_request(
                Request::TerminalPaste {
                    session_id,
                    bytes: b"denied".to_vec(),
                    confirmed: true,
                },
                observer_id,
                &state,
            )
            .await,
            Err(RequestError::NotController { .. })
        ));
        assert!(matches!(
            handle_request(
                Request::ClaimTerminalControl {
                    session_id,
                    force: false,
                },
                observer_id,
                &state,
            )
            .await,
            Err(RequestError::NotController { .. })
        ));
        handle_request(
            Request::ClaimTerminalControl {
                session_id,
                force: true,
            },
            observer_id,
            &state,
        )
        .await
        .expect("explicit takeover should transfer control");
        assert!(matches!(
            handle_request(
                Request::TerminalPaste {
                    session_id,
                    bytes: b"denied".to_vec(),
                    confirmed: true,
                },
                client_id,
                &state,
            )
            .await,
            Err(RequestError::NotController { .. })
        ));
        handle_request(
            Request::ClaimTerminalControl {
                session_id,
                force: true,
            },
            client_id,
            &state,
        )
        .await
        .expect("controller should be able to take control back explicitly");

        handle_request(
            Request::TerminalPaste {
                session_id,
                bytes: b"alpha".to_vec(),
                confirmed: true,
            },
            client_id,
            &state,
        )
        .await
        .expect("input should be accepted");

        let mut saw_output = false;
        for _ in 0..50 {
            if let ResponseBody::TerminalFrame { frame, .. } =
                terminal_snapshot(&state, session_id).expect("snapshot should exist")
                && frame.rows.iter().any(|row| row.text().contains("alpha"))
            {
                saw_output = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            saw_output,
            "PTY output should reach the retained projection"
        );
        let capture = handle_request(Request::TerminalCapture { session_id }, client_id, &state)
            .await
            .expect("atomic capture should succeed");
        assert!(matches!(
            capture,
            ResponseBody::TerminalCaptured { capture }
                if capture.terminal.session_id == session_id
                    && capture.frame.rows.iter().any(|row| row.text().contains("alpha"))
        ));

        let wait_state = Arc::clone(&state);
        let text_wait = tokio::spawn(async move {
            handle_request(
                Request::TerminalWait {
                    session_id,
                    condition: TerminalWaitCondition::Text {
                        query: "omega".to_owned(),
                        case_sensitive: true,
                    },
                    timeout_millis: 1_000,
                },
                observer_id,
                &wait_state,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        handle_request(
            Request::TerminalPaste {
                session_id,
                bytes: b"omega".to_vec(),
                confirmed: true,
            },
            client_id,
            &state,
        )
        .await
        .expect("second input should be accepted");
        assert!(matches!(
            text_wait.await.expect("wait task should join"),
            Ok(ResponseBody::TerminalWaitSatisfied {
                condition: TerminalWaitCondition::Text { query, .. },
                capture,
                ..
            }) if query == "omega"
                && capture.frame.rows.iter().any(|row| row.text().contains("omega"))
        ));
        assert!(matches!(
            handle_request(
                Request::TerminalWait {
                    session_id,
                    condition: TerminalWaitCondition::Quiet { quiet_millis: 50 },
                    timeout_millis: 1_000,
                },
                observer_id,
                &state,
            )
            .await,
            Ok(ResponseBody::TerminalWaitSatisfied {
                condition: TerminalWaitCondition::Quiet { quiet_millis: 50 },
                ..
            })
        ));

        handle_request(
            Request::TerminalSelect {
                session_id,
                anchor: SelectionPoint { column: 0, row: 0 },
                head: SelectionPoint { column: 4, row: 0 },
                rectangle: false,
            },
            client_id,
            &state,
        )
        .await
        .expect("selection should be accepted");
        let copied = handle_request(
            Request::TerminalSelectionText { session_id },
            client_id,
            &state,
        )
        .await
        .expect("selection should format");
        assert!(matches!(
            copied,
            ResponseBody::TerminalSelectionText { text: Some(text), .. } if text == "alpha"
        ));

        let wait_state = Arc::clone(&state);
        let exit_wait = tokio::spawn(async move {
            handle_request(
                Request::TerminalWait {
                    session_id,
                    condition: TerminalWaitCondition::Exit,
                    timeout_millis: 1_000,
                },
                observer_id,
                &wait_state,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        handle_request(Request::KillTerminal { session_id }, client_id, &state)
            .await
            .expect("terminal should terminate");
        assert!(matches!(
            exit_wait.await.expect("exit wait task should join"),
            Ok(ResponseBody::TerminalWaitSatisfied {
                condition: TerminalWaitCondition::Exit,
                capture,
                ..
            }) if capture.terminal.status != TerminalSessionStatus::Running
        ));
        for _ in 0..50 {
            let exited = match list_terminals(&state, false).expect("terminal index should remain")
            {
                ResponseBody::Terminals { terminals } => terminals
                    .iter()
                    .any(|terminal| terminal.status != TerminalSessionStatus::Running),
                _ => false,
            };
            if exited {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        {
            let retained =
                terminal_record(&state, session_id).expect("terminal should remain indexed");
            let projection = retained
                .projection
                .read()
                .expect("retained projection should remain readable");
            assert_eq!(projection.summary.status, TerminalSessionStatus::Exited);
            assert_eq!(projection.summary.controller_client_id, None);
            assert_eq!(projection.summary.controller_surface_id, None);
            assert_eq!(projection.summary.control_epoch, 4);
        }
        let diagnostics = runtime_diagnostics(&state)
            .await
            .expect("diagnostics should remain available");
        assert!(matches!(
            diagnostics,
            ResponseBody::RuntimeDiagnostics { diagnostics }
                if diagnostics.terminals_controlled == 0
        ));
        let retained_search = handle_request(
            Request::TerminalSearch {
                session_id,
                query: "alpha".to_owned(),
                case_sensitive: true,
                limit: 10,
            },
            client_id,
            &state,
        )
        .await
        .expect("closed terminal history should remain searchable");
        assert!(matches!(
            retained_search,
            ResponseBody::TerminalSearchResults { matches, .. }
                if matches.iter().any(|found| found.preview.contains("alpha"))
        ));
        let retained_history = handle_request(
            Request::TerminalHistoryFrame {
                session_id,
                viewport: superplexr_terminal::HistoryViewport::RowsBeforeBottom(0),
            },
            client_id,
            &state,
        )
        .await
        .expect("closed terminal history should remain pageable");
        assert!(matches!(
            retained_history,
            ResponseBody::TerminalHistoryFrame { frame, .. }
                if frame.rows.iter().any(|row| row.text().contains("alpha"))
        ));
        handle_request(Request::ArchiveTerminal { session_id }, client_id, &state)
            .await
            .expect("closed terminal should archive without deleting history");
        assert!(matches!(
            list_terminals(&state, false).expect("visible index should remain readable"),
            ResponseBody::Terminals { terminals }
                if terminals.iter().all(|terminal| terminal.session_id != session_id)
        ));
        assert!(matches!(
            list_terminals(&state, true).expect("archive index should remain readable"),
            ResponseBody::Terminals { terminals }
                if terminals.iter().any(|terminal|
                    terminal.session_id == session_id && terminal.archived)
        ));
        drop(state);
        let recovered = recover_terminal_history(&root).expect("journal history should recover");
        let retained = recovered
            .get(&session_id)
            .expect("terminated terminal should remain indexed");
        {
            let projection = retained
                .projection
                .read()
                .expect("retained projection lock should remain healthy");
            assert_eq!(projection.summary.status, TerminalSessionStatus::Exited);
            assert!(projection.summary.archived);
            assert!(projection.summary.process_id.is_none());
            assert!(
                projection
                    .frame
                    .as_ref()
                    .is_some_and(|frame| frame.rows.iter().any(|row| row.text().contains("alpha")))
            );
        }
        let recovered_state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("missions"))
                    .await
                    .expect("mission store should reopen"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("scheduler-policies.json"))
                    .expect("scheduler policy store should reopen"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("shares.json")).expect("share store should reopen"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(root.join("run-checkouts.json"), root.join("run-checkouts"))
                    .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(recovered),
            terminal_state_dir: root.clone(),
            agent_socket_path: root.join("agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        assert!(matches!(
            handle_request(
                Request::ClaimTerminalControl {
                    session_id,
                    force: true,
                },
                client_id,
                &recovered_state,
            )
            .await,
            Err(RequestError::TerminalNotRunning(id)) if id == session_id
        ));
        let recovered_search = handle_request(
            Request::TerminalSearch {
                session_id,
                query: "alpha".to_owned(),
                case_sensitive: true,
                limit: 10,
            },
            client_id,
            &recovered_state,
        )
        .await
        .expect("recovered terminal history should remain searchable");
        assert!(matches!(
            recovered_search,
            ResponseBody::TerminalSearchResults { matches, .. }
                if matches.iter().any(|found| found.preview.contains("alpha"))
        ));
        let recovered_history = handle_request(
            Request::TerminalHistoryFrame {
                session_id,
                viewport: superplexr_terminal::HistoryViewport::RowsBeforeBottom(0),
            },
            client_id,
            &recovered_state,
        )
        .await
        .expect("recovered terminal history should remain pageable");
        assert!(matches!(
            recovered_history,
            ResponseBody::TerminalHistoryFrame { frame, .. }
                if frame.rows.iter().any(|row| row.text().contains("alpha"))
        ));
        handle_request(
            Request::RestoreTerminal { session_id },
            client_id,
            &recovered_state,
        )
        .await
        .expect("archived history should be restorable after recovery");
        assert!(matches!(
            list_terminals(&recovered_state, false)
                .expect("restored index should remain readable"),
            ResponseBody::Terminals { terminals }
                if terminals.iter().any(|terminal|
                    terminal.session_id == session_id && !terminal.archived)
        ));
        drop(recovered_state);
        std::fs::remove_dir_all(root).expect("isolated test state should be removable");
    }

    #[test]
    fn startup_security_preserves_managed_checkout_modes_and_symlinks() {
        let root =
            std::env::temp_dir().join(format!("superplexr-secure-checkout-{}", Uuid::new_v4()));
        let checkout = root.join("run-checkouts").join("mission").join("run");
        std::fs::create_dir_all(&checkout).expect("checkout fixture should exist");
        let executable = checkout.join("tool.sh");
        std::fs::write(&executable, "#!/bin/sh\n").expect("fixture should write");
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
            .expect("fixture mode should set");
        std::os::unix::fs::symlink("tool.sh", checkout.join("tool-link"))
            .expect("repository symlink should exist");

        secure_existing_state(&root)
            .expect("runtime state should secure without rewriting Git data");
        assert_eq!(
            std::fs::metadata(&executable)
                .expect("executable should remain")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(
            std::fs::symlink_metadata(checkout.join("tool-link"))
                .expect("symlink should remain")
                .file_type()
                .is_symlink()
        );
        std::fs::remove_dir_all(root).expect("isolated fixture should be removable");
    }

    #[tokio::test]
    async fn managed_checkout_launches_the_run_and_refuses_dirty_retirement() {
        let root = std::env::temp_dir().join(format!("superplexr-checkout-e2e-{}", Uuid::new_v4()));
        let repository = root.join("repository");
        std::fs::create_dir_all(&repository).expect("repository should exist");
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "superplexr test"],
            vec!["config", "user.email", "test@superplexr.invalid"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&repository)
                    .args(args)
                    .status()
                    .expect("Git fixture command should start")
                    .success()
            );
        }
        std::fs::write(repository.join("README.md"), "checkout fixture\n")
            .expect("fixture should write");
        for args in [
            vec!["add", "README.md"],
            vec!["commit", "-q", "-m", "fixture"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&repository)
                    .args(args)
                    .status()
                    .expect("Git fixture command should start")
                    .success()
            );
        }

        let state = Arc::new(AppState {
            store: Mutex::new(
                MissionStore::open(root.join("state/missions"))
                    .await
                    .expect("mission store should open"),
            ),
            scheduler_policies: Mutex::new(
                SchedulerPolicyStore::open(root.join("state/scheduler-policies.json"))
                    .expect("scheduler policy store should open"),
            ),
            shares: Mutex::new(
                ShareStore::open(root.join("state/shares.json")).expect("share store should open"),
            ),
            run_checkouts: Mutex::new(
                RunCheckoutStore::open(
                    root.join("state/run-checkouts.json"),
                    root.join("state/run-checkouts"),
                )
                .expect("Run checkout store should open"),
            ),
            checkout_gate: Mutex::new(()),
            agent_launch_gate: Mutex::new(()),
            terminals: RwLock::new(HashMap::new()),
            terminal_state_dir: root.join("state"),
            agent_socket_path: root.join("state/agent.sock"),
            mission_events: broadcast::channel(256).0,
            activity_events: broadcast::channel(512).0,
            terminal_events: broadcast::channel(256).0,
            session_group_events: broadcast::channel(256).0,
            share_revocations: broadcast::channel(64).0,
            started_at: Instant::now(),
            open_connections: AtomicUsize::new(0),
            agent_connections: AtomicUsize::new(0),
            active_waits: AtomicUsize::new(0),
            plugins: PluginPublisher::disabled(),
            provider_status: Mutex::new(ProviderStatusStore::transient()),
            run_evidence: Mutex::new(RunEvidenceStore::transient()),
            faults: Mutex::new(FaultStore::transient()),
            session_groups: Mutex::new(
                SessionGroupStore::open(root.join("session-groups.json"))
                    .expect("Session group store should open"),
            ),
        });
        std::fs::write(
            root.join("state/engines.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "drivers": {
                    "pwd": {
                        "program": "/bin/sh",
                        "args": ["-c", "test -f README.md && if test -n \"$SUPERPLEXR_DELIVERY_PURPOSE\"; then test -n \"$SUPERPLEXR_SOURCE_RUN_ID\" && test -n \"$SUPERPLEXR_CANDIDATE_REVISION\" && printf %s \"$SUPERPLEXR_DELIVERY_PURPOSE:$SUPERPLEXR_SOURCE_RUN_ID \"; fi; sleep 1; printf checkout-cwd-ok"]
                    }
                }
            }))
            .expect("engine config should encode"),
        )
        .expect("engine config should write");
        let mission_id = MissionId::new();
        let run_id = RunId::new();
        handle_request(
            Request::CreateMission {
                mission_id,
                intent: "prove managed checkout execution".to_owned(),
                created_by: Actor::human("owner").expect("owner should be valid"),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("Mission should create");
        handle_request(
            Request::Dispatch {
                mission_id,
                command: Command::PlanRun {
                    run_id,
                    parent: None,
                    dependencies: Vec::new(),
                    retry_of: None,
                    actor: Actor::agent("checkout-agent", "pwd").expect("agent should be valid"),
                    objective: "print the isolated working directory".to_owned(),
                    priority: RunPriority::Normal,
                },
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("Run should plan");
        let checkout = match handle_request(
            Request::PrepareRunCheckout {
                mission_id,
                run_id,
                repository: repository.clone(),
                base_ref: "HEAD".to_owned(),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("checkout should prepare")
        {
            ResponseBody::RunCheckout { checkout } => checkout,
            body => panic!("unexpected checkout response: {body:?}"),
        };
        assert_eq!(checkout.state, superplexr_protocol::RunCheckoutState::Ready);
        handle_request(
            Request::Dispatch {
                mission_id,
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::DeclareChangeIntent {
                        run_id,
                        expected_version: None,
                        spec: ChangeIntentSpec {
                            repository_identity: "checkout-e2e".to_owned(),
                            base_revision: checkout.base_revision.clone(),
                            claims: vec![ChangeClaim {
                                path: "README.md".to_owned(),
                                operation: ChangeOperation::Modify,
                                scope: ChangeScope::Committed,
                            }],
                        },
                    },
                },
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("change intent should declare");
        handle_request(
            Request::Dispatch {
                mission_id,
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::AdmitChangeIntent {
                        run_id,
                        expected_version: 1,
                        lease_epoch: 0,
                    },
                },
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("change intent should admit");

        let preview = handle_request(
            Request::PreviewConfiguredAgentRun {
                mission_id,
                run_id,
                cwd: PathBuf::new(),
                use_run_checkout: true,
                grid: GridSize::new(80, 12).expect("grid should be valid"),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("checkout launch should preview");
        assert!(matches!(
            preview,
            ResponseBody::ConfiguredAgentLaunchPreview { preview }
                if preview.cwd == checkout.worktree_path
        ));

        let session_id = SessionId::new();
        handle_request(
            Request::LaunchConfiguredAgentRun {
                mission_id,
                run_id,
                session_id,
                session_name: "checkout-pwd".to_owned(),
                cwd: PathBuf::new(),
                use_run_checkout: true,
                grid: GridSize::new(80, 12).expect("grid should be valid"),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("Run should launch inside checkout");
        std::fs::write(
            checkout.worktree_path.join("README.md"),
            "candidate result\n",
        )
        .expect("agent result should write");
        let admitted_lease = state
            .store
            .lock()
            .await
            .get(mission_id)
            .expect("Mission should remain readable")
            .verified_delivery
            .change_intents[&run_id]
            .lease_epoch;
        let candidate_request_id = Uuid::new_v4();
        let candidate_response = handle_request_with_id(
            Request::Dispatch {
                mission_id,
                command: Command::VerifiedDelivery {
                    command: VerifiedDeliveryCommand::SubmitCandidate {
                        run_id,
                        candidate: RunCandidate {
                            revision: "caller-value-is-not-authoritative".to_owned(),
                            content_sha256: "0".repeat(64),
                            execution_lease_epoch: Some(admitted_lease),
                            realized_changes: None,
                            artifact_ids: Vec::new(),
                            submitted_by: ActorId::new("checkout-agent")
                                .expect("agent actor should be valid"),
                        },
                    },
                },
            },
            Uuid::new_v4(),
            candidate_request_id,
            &state,
        )
        .await
        .expect("Candidate should publish through realized-change inspection");
        let (candidate, patch_artifact, review_artifact) = match candidate_response {
            ResponseBody::EventsCommitted { mission, events } => {
                assert_eq!(
                    events.len(),
                    3,
                    "review Artifacts and Candidate commit atomically"
                );
                let candidate = mission
                    .verified_delivery
                    .candidates
                    .get(&run_id)
                    .cloned()
                    .expect("Candidate should be durable");
                let patch = mission
                    .artifacts
                    .values()
                    .find(|artifact| artifact.name == "Candidate patch")
                    .cloned()
                    .expect("patch Artifact should be durable");
                let review = mission
                    .artifacts
                    .values()
                    .find(|artifact| artifact.name == "Candidate review contract")
                    .cloned()
                    .expect("review Artifact should be durable");
                (candidate, patch, review)
            }
            body => panic!("unexpected Candidate response: {body:?}"),
        };
        let manifest = candidate
            .realized_changes
            .as_ref()
            .expect("runtime should author the realized-change manifest");
        assert_eq!(
            Some(candidate.revision.as_str()),
            manifest.snapshot_revision.as_deref()
        );
        assert_ne!(candidate.revision, checkout.base_revision);
        assert_eq!(candidate.content_sha256, manifest.patch_sha256);
        assert_eq!(patch_artifact.media_type, "text/x-diff");
        let expected_patch_digest = format!("sha256:{}", manifest.patch_sha256);
        assert_eq!(
            patch_artifact.digest.as_deref(),
            Some(expected_patch_digest.as_str())
        );
        assert!(patch_artifact.locator.starts_with("git-diff:"));
        assert_eq!(
            review_artifact.media_type,
            "application/vnd.superplexr.candidate-review+json"
        );
        let review_bytes = std::fs::read(&review_artifact.locator)
            .expect("review Artifact locator should be readable");
        let review_json: serde_json::Value =
            serde_json::from_slice(&review_bytes).expect("review Artifact should be JSON");
        assert_eq!(review_json["snapshot_revision"], candidate.revision);
        assert_eq!(review_json["required_checks"][0]["state"], "not_run");
        assert!(manifest.changes.iter().any(|change| {
            change.path == "README.md" && change.operation == ChangeOperation::Modify
        }));

        let expansion = checkout.worktree_path.join("undeclared.txt");
        std::fs::write(&expansion, "post-publication expansion\n")
            .expect("expansion fixture should write");
        verify_candidate_settlement(
                &state,
                mission_id,
                &Command::AcceptRunResult {
                    run_id,
                    by: ActorId::new("owner").expect("owner should be valid"),
                    note: "reviewed".to_owned(),
                },
            )
            .await
            .expect("later checkout writes must not mutate the frozen Candidate");
        let replay = handle_request_with_id(
            Request::Dispatch {
                mission_id,
                command: Command::CompleteMission,
            },
            Uuid::new_v4(),
            candidate_request_id,
            &state,
        )
        .await
        .expect("idempotent retry must not rescan the changed checkout");
        assert!(matches!(
            replay,
            ResponseBody::EventsCommitted { mission, .. }
                if mission.verified_delivery.candidates[&run_id] == candidate
        ));
        std::fs::remove_file(expansion).expect("expansion fixture should remove");
        for _ in 0..100 {
            if state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("Mission should remain readable")
                .runs[&run_id]
                .status
                .is_finished()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("Mission should remain readable")
                .runs[&run_id]
                .status
                .is_finished()
        );
        let frame = match terminal_snapshot(&state, session_id).expect("frame should remain") {
            ResponseBody::TerminalFrame { frame, .. } => frame,
            body => panic!("unexpected frame response: {body:?}"),
        };
        assert!(
            frame
                .rows
                .iter()
                .any(|row| row.text().contains("checkout-cwd-ok"))
        );
        let verifier_run_id = RunId::new();
        let verifier_response = handle_request(
            Request::Dispatch {
                mission_id,
                command: Command::CreateVerifierRun {
                    subject_run_id: run_id,
                    verifier_run_id,
                    actor: Actor::agent("candidate-verifier", "pwd")
                        .expect("verifier actor should be valid"),
                    priority: RunPriority::Urgent,
                },
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("frozen Candidate should create verifier");
        assert!(matches!(
            verifier_response,
            ResponseBody::EventsCommitted { ref events, ref mission }
                if events.len() == 2
                    && mission.verified_delivery.delivery_run_inputs[&verifier_run_id]
                        .candidate == candidate
        ));
        let verifier_checkout = match handle_request(
            Request::PrepareRunCheckout {
                mission_id,
                run_id: verifier_run_id,
                repository: repository.clone(),
                base_ref: candidate.revision.clone(),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("verifier Candidate checkout should prepare")
        {
            ResponseBody::RunCheckout { checkout } => checkout,
            body => panic!("unexpected verifier checkout response: {body:?}"),
        };
        let verifier_preview = handle_request(
            Request::PreviewConfiguredAgentRun {
                mission_id,
                run_id: verifier_run_id,
                cwd: repository.clone(),
                use_run_checkout: false,
                grid: GridSize::new(80, 12).expect("grid should be valid"),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("verifier preview should use the frozen Candidate checkout");
        assert!(matches!(
            verifier_preview,
            ResponseBody::ConfiguredAgentLaunchPreview { preview }
                if preview.cwd == verifier_checkout.worktree_path
        ));
        let verifier_session_id = SessionId::new();
        handle_request(
            Request::LaunchConfiguredAgentRun {
                mission_id,
                run_id: verifier_run_id,
                session_id: verifier_session_id,
                session_name: "candidate-verifier".to_owned(),
                cwd: repository,
                use_run_checkout: false,
                grid: GridSize::new(80, 12).expect("grid should be valid"),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("verifier should launch from the frozen Candidate");
        for _ in 0..100 {
            if state
                .store
                .lock()
                .await
                .get(mission_id)
                .expect("Mission should remain readable")
                .runs[&verifier_run_id]
                .status
                .is_finished()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let verifier_frame =
            match terminal_snapshot(&state, verifier_session_id).expect("frame should remain") {
                ResponseBody::TerminalFrame { frame, .. } => frame,
                body => panic!("unexpected verifier frame response: {body:?}"),
            };
        assert!(verifier_frame.rows.iter().any(|row| {
            let text = row.text();
            text.contains("verification:") && text.contains(&run_id.to_string())
        }));
        std::fs::write(
            checkout.worktree_path.join("README.md"),
            "checkout fixture\n",
        )
        .expect("fixture should restore base content before retirement");

        let untracked = checkout.worktree_path.join("agent-result.txt");
        std::fs::write(&untracked, "valuable uncommitted result\n")
            .expect("untracked result should write");
        assert!(matches!(
            handle_request(
                Request::RetireRunCheckout {
                    mission_id,
                    run_id,
                    merged_into_ref: "HEAD".to_owned(),
                },
                Uuid::new_v4(),
                &state,
            )
            .await,
            Err(RequestError::Checkout(run_checkout::CheckoutError::Dirty))
        ));
        assert!(untracked.is_file(), "refused retirement must preserve work");
        std::fs::remove_file(untracked).expect("test result should remove");
        let retired = handle_request(
            Request::RetireRunCheckout {
                mission_id,
                run_id,
                merged_into_ref: "HEAD".to_owned(),
            },
            Uuid::new_v4(),
            &state,
        )
        .await
        .expect("clean merged checkout should retire");
        assert!(matches!(
            retired,
            ResponseBody::RunCheckout { checkout }
                if checkout.state == superplexr_protocol::RunCheckoutState::Retired
        ));
        drop(state);
        std::fs::remove_dir_all(root).expect("isolated fixture should be removable");
    }
}
