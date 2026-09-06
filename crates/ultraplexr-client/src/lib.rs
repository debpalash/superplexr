//! Native blocking client for the local ultraplexr daemon.
//!
//! GPUI and CLI callers do not need to own a Tokio runtime. A control channel
//! is shared and serialized, while one reader dispatches independently
//! sequenced subscription streams without placing socket I/O on GPUI threads.

pub mod history_worker;
mod share_token;
#[cfg(test)]
mod share_token_tests;
pub use share_token::read_share_token_file;
mod metadata;
pub use metadata::{
    CollectionChange, CollectionTracker, MetadataCancellation, MetadataDelivery, MetadataReceiver,
    MetadataUpdate,
};
mod receive_queue;
#[cfg(test)]
mod receive_wire_tests;
#[cfg(test)]
mod verification_wire_tests;
mod terminal_input;
pub use terminal_input::{TerminalInput, TerminalInputLease};
#[cfg(test)]
mod input_writer_tests;
mod search_stream;
pub mod transport;
pub use search_stream::{SearchCancel, SearchStream};
#[cfg(test)]
use std::os::unix::net::UnixStream;
use transport::{Connector, TransportReader, TransportWriter, UnixSocketConnector};

use std::{
    collections::HashMap,
    ffi::OsString,
    fs::OpenOptions,
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;
use ultraplexr_core::{
    Actor, Command, Event, FaultId, Mission, MissionId, SchedulerPlan, SessionId,
};
#[cfg(test)]
use ultraplexr_protocol::wire_v3::SyncWire;
use ultraplexr_protocol::{
    ClientRequest, ConfiguredAgentLaunchPreview, FaultInput, FaultSummary, MissionEvent,
    MissionHistoryEntry, MissionSummary, PROTOCOL_VERSION, PluginRuntimeSummary, ProtocolError,
    ProviderFactInput, ProviderFactSummary, Request, ResponseBody, ResponseResult,
    RunActivityEvent, RunActivitySummary, RunCheckoutSummary, RunEvidenceInput, RunEvidenceSummary,
    RuntimeDiagnostics, ScheduledAgentLaunch, ScheduledAgentLaunchFailure, SchedulerPolicy,
    SchedulerSettings, ServerEvent, ServerResponse, SessionGroupChange, SessionGroupEvent,
    SessionGroupId, SessionGroupSpec, SessionGroupSummary, ShareRole, ShareSummary,
    TerminalCapture, TerminalIndexEvent, TerminalSessionSpec, TerminalSessionSummary,
    TerminalWaitCondition, decode_terminal_event,
    wire_v3::{FrameKind, ReceivedFrame},
};
use ultraplexr_terminal::{
    FullFrame, GridSize, HistoryViewport, KeyInput, MouseInput, SearchMatch, SelectionPoint,
    ViewportScroll,
};
use uuid::Uuid;

/// How long a client waits for a daemon response before giving up.
///
/// This wait used to be unbounded. A daemon busy in a long startup replay left
/// every caller parked forever, and because the desktop issues some requests
/// from its main thread, a slow daemon presented as a permanently frozen app
/// with no way back. A timeout turns that into a reportable error.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling for replaying a Fault, which runs a real command.
///
/// The daemon caps the replay itself at 900 seconds; this leaves headroom above
/// that so the daemon's own bounded answer arrives before the client stops
/// waiting for it.
const REPRODUCE_FAULT_TIMEOUT: Duration = Duration::from_secs(960);

/// The daemon's own default guard batch size, used only to size the wait when
/// the caller names no limit. The daemon remains the authority on the batch.
const DEFAULT_GUARD_LIMIT_HINT: u16 = 20;

/// Headroom above a deadline the daemon has already been told to honour.
///
/// The daemon answers such requests itself when its own limit expires, so the
/// client should outlast it and report that answer rather than pre-empt it.
const RESPONSE_HEADROOM: Duration = Duration::from_secs(30);

/// How long this request may take before the wait is treated as a failure.
///
/// Requests that carry their own deadline get that deadline plus headroom.
/// Bounding those at the default would break exactly the callers who asked to
/// wait longer than it.
#[must_use]
pub fn request_timeout(request: &Request) -> Duration {
    match request {
        // Replay runs a real command; the daemon caps it at 900 seconds.
        Request::ReproduceFault { .. } => REPRODUCE_FAULT_TIMEOUT,
        // A guard pass is many such replays in series, so it needs room for
        // all of them rather than for one.
        Request::GuardFaults { limit, .. } => REPRODUCE_FAULT_TIMEOUT
            .saturating_mul(u32::from(limit.unwrap_or(DEFAULT_GUARD_LIMIT_HINT)).max(1)),
        // The caller chose how long to wait for the terminal condition.
        Request::TerminalWait { timeout_millis, .. } => {
            Duration::from_millis(*timeout_millis).saturating_add(RESPONSE_HEADROOM)
        }
        _ => DEFAULT_REQUEST_TIMEOUT,
    }
}

const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(25);

pub const RECEIVE_LIMIT_MESSAGE: &str = "Subscription record exceeds the client receive limit; automatic reconnect stopped. Reduce its size and reattach.";

#[derive(Debug, Error)]
pub enum ClientError {
    #[error(
        "subscription record retains {bytes} bytes; client limit is {limit}; automatic reconnect stopped"
    )]
    ReceiveLimit { bytes: usize, limit: usize },
    #[error("invalid metadata snapshot: {0}")]
    MetadataSnapshot(&'static str),
    #[error("daemon I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("daemon rejected the request ({code}): {message}")]
    Remote { code: String, message: String },
    #[error("daemon returned protocol version {actual}; client requires {PROTOCOL_VERSION}")]
    Version { actual: u16 },
    #[error("daemon response id did not match its request")]
    RequestMismatch,
    #[error("daemon returned an unexpected response: {0:?}")]
    UnexpectedResponse(Box<ResponseBody>),
    #[error("daemon binary is missing beside the desktop executable: {0}")]
    DaemonBinaryMissing(PathBuf),
    #[error("daemon process exited before becoming ready: {0}")]
    DaemonExited(String),
    #[error("daemon did not become ready within five seconds")]
    DaemonStartTimeout,
    #[error("control connection lock was poisoned")]
    ControlPoisoned,
    #[error("daemon did not answer within {timeout:?}")]
    RequestTimeout { timeout: Duration },
}

struct MultiplexedWire {
    writer: Mutex<TransportWriter>,
    pending: Mutex<HashMap<Uuid, PendingResponse>>,
    streams: receive_queue::Streams,
    interrupt: transport::Interrupt,
    closed: AtomicBool,
    oversized_record: AtomicUsize,
    features: Vec<String>,
    searches: Mutex<HashMap<Uuid, std::sync::mpsc::SyncSender<ReceivedFrame>>>,
}

struct PendingResponse {
    sender: Sender<Result<ServerResponse, String>>,
    subscription: bool,
}

fn accepted_subscription(body: &ResponseBody) -> Option<u32> {
    match body {
        ResponseBody::SessionGroupSubscriptionAccepted { stream_id }
        | ResponseBody::TerminalIndexSubscriptionAccepted { stream_id }
        | ResponseBody::RunActivitySubscriptionAccepted { stream_id }
        | ResponseBody::MissionSubscriptionAccepted { stream_id }
        | ResponseBody::TerminalSubscriptionAccepted { stream_id, .. } => Some(*stream_id),
        _ => None,
    }
}

impl MultiplexedWire {
    fn start(connection: transport::Connection) -> Result<Arc<Self>, ClientError> {
        let wire = Arc::new(Self {
            writer: Mutex::new(connection.writer),
            pending: Mutex::new(HashMap::new()),
            streams: receive_queue::Streams::new(receive_queue::Budget::process()),
            interrupt: connection.interrupt,
            closed: AtomicBool::new(false),
            oversized_record: AtomicUsize::new(0),
            features: connection.features,
            searches: Mutex::new(HashMap::new()),
        });
        // A blocking reader must not keep its own wire alive forever. Dropping
        // the final client/lease owner interrupts the read and ends the thread.
        let dispatcher = Arc::downgrade(&wire);
        thread::Builder::new()
            .name("ultraplexr-wire-dispatch".to_owned())
            .spawn(move || Self::dispatch(dispatcher, connection.reader))?;
        Ok(wire)
    }

    fn dispatch(owner: std::sync::Weak<Self>, mut reader: TransportReader) {
        while let Ok(frame) = reader.receive() {
            let Some(wire) = owner.upgrade() else { return };
            if wire.closed.load(Ordering::Acquire) {
                return;
            }
            if frame.header.kind == FrameKind::SearchPage {
                use ultraplexr_protocol::search_stream::MAX_PAGE_BYTES;
                #[derive(serde::Deserialize)]
                struct SearchIdentity {
                    search_id: Uuid,
                }
                if frame.header.stream_id == 0 || frame.payload.len() > MAX_PAGE_BYTES {
                    break;
                }
                let Ok(page) = serde_json::from_slice::<SearchIdentity>(&frame.payload) else {
                    break;
                };
                if let Ok(mut searches) = wire.searches.lock()
                    && let Some(sender) = searches.get(&page.search_id)
                    && sender.try_send(frame).is_err()
                {
                    searches.remove(&page.search_id);
                }
                // Registration precedes the start request. Late/cancelled pages
                // are discarded, never placed in the generic early-event queue.
                continue;
            }
            if frame.header.stream_id == 0 && frame.header.kind == FrameKind::Response {
                match serde_json::from_slice::<ServerResponse>(&frame.payload) {
                    Ok(response) => {
                        let acceptance = match &response.result {
                            ResponseResult::Success { body } => {
                                if let ResponseBody::SubscriptionEnded { stream_id } = **body {
                                    wire.streams.finish(stream_id);
                                }
                                accepted_subscription(body)
                            }
                            _ => None,
                        };
                        let pending = wire
                            .pending
                            .lock()
                            .ok()
                            .and_then(|mut pending| pending.remove(&response.request_id));
                        if let Some(pending) = pending {
                            if let Some(id) = acceptance {
                                let result = if pending.subscription
                                    && response.version == PROTOCOL_VERSION
                                {
                                    wire.streams.announce(id)
                                } else {
                                    Err("unexpected subscription acceptance")
                                };
                                if let Err(reason) = result {
                                    let _ = pending.sender.send(Err(reason.into()));
                                    wire.retire(reason);
                                    return;
                                }
                            }
                            if pending.sender.send(Ok(response)).is_err() && acceptance.is_some() {
                                wire.retire("subscription caller disappeared before acceptance");
                                return;
                            }
                        } else if acceptance.is_some() {
                            wire.retire("subscription acceptance has no pending request");
                            return;
                        }
                    }
                    Err(_) => break,
                }
                continue;
            }
            if let Err(reason) = wire.streams.push(frame) {
                match reason {
                    receive_queue::PushError::Transient(reason) => wire.retire(reason),
                    receive_queue::PushError::Oversized(bytes) => {
                        wire.oversized_record.store(bytes, Ordering::Release);
                        wire.retire(RECEIVE_LIMIT_MESSAGE);
                    }
                }
                return;
            }
        }
        if let Some(wire) = owner.upgrade() {
            wire.retire("daemon connection closed or invalid frame received");
        }
    }

    fn receive_error(&self, reason: &str) -> ClientError {
        let bytes = self.oversized_record.load(Ordering::Acquire);
        if bytes != 0 {
            ClientError::ReceiveLimit {
                bytes,
                limit: receive_queue::STREAM_BYTES,
            }
        } else {
            disconnected_error(reason)
        }
    }

    fn retire(&self, reason: &'static str) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        (self.interrupt)();
        self.streams.close(reason);
        if let Ok(mut searches) = self.searches.lock() {
            searches.clear();
        }
        if let Ok(mut pending) = self.pending.lock() {
            for (_, pending) in pending.drain() {
                let _ = pending.sender.send(Err(reason.to_owned()));
            }
        }
    }

    fn exchange(&self, request: &ClientRequest) -> Result<ServerResponse, ClientError> {
        self.exchange_checked(request, || true)
    }

    fn exchange_checked(
        &self,
        request: &ClientRequest,
        valid: impl Fn() -> bool,
    ) -> Result<ServerResponse, ClientError> {
        if self.closed.load(Ordering::Acquire) || !valid() {
            return Err(self.receive_error("daemon connection is closed"));
        }
        let (send, receive) = std::sync::mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?
            .insert(
                request.request_id,
                PendingResponse {
                    sender: send,
                    subscription: matches!(
                        request.action,
                        Request::SubscribeTerminal { .. }
                            | Request::SubscribeMissions
                            | Request::SubscribeRunActivities { .. }
                            | Request::SubscribeTerminals
                            | Request::SubscribeSessionGroups { .. }
                    ),
                },
            );
        let send_result = (|| {
            let mut writer = self
                .writer
                .lock()
                .map_err(|_| ClientError::ControlPoisoned)?;
            // A retired input must not wait behind another write and then use
            // authority that changed while the writer was occupied.
            if self.closed.load(Ordering::Acquire) || !valid() {
                return Err(self.receive_error("input authority retired before write"));
            }
            if let Err(error) = writer.send_json(FrameKind::Request, 0, request) {
                // A partial frame cannot safely be followed by another write.
                // Interrupt before releasing the writer to a waiting sender.
                self.retire("native request write failed");
                return Err(ProtocolError::from(error).into());
            }
            Ok(())
        })();
        if let Err(error) = send_result {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&request.request_id);
            }
            return Err(if self.oversized_record.load(Ordering::Acquire) != 0 {
                self.receive_error("native request write failed")
            } else {
                error
            });
        }
        let timeout = request_timeout(&request.action);
        match receive.recv_timeout(timeout) {
            Ok(response) => response.map_err(|message| self.receive_error(&message)),
            Err(RecvTimeoutError::Disconnected) => {
                Err(self.receive_error("daemon response dispatcher stopped"))
            }
            Err(RecvTimeoutError::Timeout) => {
                // Drop the slot, or a late reply would sit in the map forever.
                // The connection is left open: the daemon is slow, not broken,
                // and the next request may well succeed.
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&request.request_id);
                }
                Err(ClientError::RequestTimeout { timeout })
            }
        }
    }

    fn subscribe(
        self: &Arc<Self>,
        stream_id: u32,
        unsubscribe_request: ClientRequest,
    ) -> Result<MultiplexedSubscription, ClientError> {
        let receive = self
            .streams
            .attach(stream_id)
            .map_err(|reason| self.receive_error(reason))?;
        Ok(MultiplexedSubscription {
            stream_id,
            receive,
            cancel: SubscriptionCancel {
                stream_id,
                request: unsubscribe_request,
                wire: Arc::clone(self),
            },
        })
    }

    fn unsubscribe(&self, stream_id: u32, request: &ClientRequest) {
        self.streams.cancel(stream_id);
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut writer) = self.writer.lock() {
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            if writer.send_json(FrameKind::Request, 0, request).is_err() {
                // A failed/partial unsubscribe frame must not be followed by
                // another request on a potentially desynchronized wire.
                self.retire("native unsubscribe write failed");
            }
        }
    }
}

impl Drop for MultiplexedWire {
    fn drop(&mut self) {
        self.retire("native connection owner dropped");
    }
}

struct MultiplexedSubscription {
    stream_id: u32,
    receive: receive_queue::Receiver,
    cancel: SubscriptionCancel,
}

#[derive(Clone)]
struct SubscriptionCancel {
    stream_id: u32,
    request: ClientRequest,
    wire: Arc<MultiplexedWire>,
}

impl SubscriptionCancel {
    fn cancel(&self) {
        self.wire.unsubscribe(self.stream_id, &self.request);
    }
}

impl MultiplexedSubscription {
    fn receive_terminal(&self) -> Result<ServerEvent, ClientError> {
        let frame = self
            .receive
            .recv()
            .map_err(|reason| self.cancel.wire.receive_error(reason))?;
        if frame.header.stream_id != self.stream_id {
            return Err(disconnected_error(
                "terminal event arrived on the wrong stream",
            ));
        }
        Ok(decode_terminal_event(frame.header.kind, &frame.payload)?)
    }
}

impl Drop for MultiplexedSubscription {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn disconnected_error(message: &str) -> ClientError {
    ClientError::Io(std::io::Error::new(
        std::io::ErrorKind::ConnectionAborted,
        message.to_owned(),
    ))
}

#[derive(Clone)]
pub struct ControlClient {
    connector: Arc<dyn Connector>,
    client_id: Uuid,
    share_token: Option<Arc<str>>,
    share_role: Option<ShareRole>,
    stream: Arc<Mutex<Arc<MultiplexedWire>>>,
    group_mutations: Arc<Mutex<()>>,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlClient")
            .field("client_id", &self.client_id)
            .field("observer_authorized", &self.share_token.is_some())
            .field("share_role", &self.share_role)
            .finish_non_exhaustive()
    }
}

impl ControlClient {
    #[must_use]
    pub fn is_observer(&self) -> bool {
        self.share_role == Some(ShareRole::Observer)
    }

    #[must_use]
    pub fn is_shared(&self) -> bool {
        self.share_role.is_some()
    }

    pub fn connect(socket_path: impl AsRef<Path>) -> Result<Self, ClientError> {
        Self::connect_with_optional_share(socket_path, None)
    }

    pub fn connect_as_observer(
        socket_path: impl AsRef<Path>,
        share_token: impl Into<String>,
    ) -> Result<Self, ClientError> {
        Self::connect_with_share(socket_path, share_token)
    }

    pub fn connect_with_share(
        socket_path: impl AsRef<Path>,
        share_token: impl Into<String>,
    ) -> Result<Self, ClientError> {
        Self::connect_with_optional_share(socket_path, Some(share_token.into()))
    }

    fn connect_with_optional_share(
        socket_path: impl AsRef<Path>,
        share_token: Option<String>,
    ) -> Result<Self, ClientError> {
        Self::connect_with_connector(
            Arc::new(UnixSocketConnector(socket_path.as_ref().to_path_buf())),
            share_token,
        )
    }

    /// Connect through a supplied trusted transport, retaining normal Share
    /// authorization, multiplexing and reconnect semantics.
    pub fn connect_with_connector(
        connector: Arc<dyn Connector>,
        share_token: Option<String>,
    ) -> Result<Self, ClientError> {
        let client_id = Uuid::new_v4();
        let stream = connect_wire(connector.as_ref(), client_id)?;
        let mut client = Self {
            connector,
            client_id,
            share_token: share_token.map(Arc::<str>::from),
            share_role: None,
            stream: Arc::new(Mutex::new(stream)),
            group_mutations: Arc::new(Mutex::new(())),
        };
        match client.request(Request::Ping)? {
            ResponseBody::Pong => {
                if client.share_token.is_some() {
                    match client.request(Request::ShareIdentity)? {
                        ResponseBody::ShareIdentity { share } => {
                            client.share_role = Some(share.role);
                        }
                        body => return Err(ClientError::UnexpectedResponse(Box::new(body))),
                    }
                }
                Ok(client)
            }
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Connect to a running daemon or launch the packaged sibling daemon.
    pub fn connect_or_spawn(
        socket_path: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
    ) -> Result<Self, ClientError> {
        let executable = std::env::current_exe()?;
        let daemon = executable
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("ultraplexr-server");
        Self::connect_or_spawn_program(socket_path, state_dir, daemon, &[])
    }

    /// Connect to a runtime or launch a daemon hosted by an explicit program.
    ///
    /// `leading_arguments` precede the standard `--socket` and `--state-dir`
    /// arguments. The desktop uses this to launch its exact embedded server
    /// build, avoiding client/daemon drift during local development.
    pub fn connect_or_spawn_program(
        socket_path: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
        daemon: impl AsRef<Path>,
        leading_arguments: &[OsString],
    ) -> Result<Self, ClientError> {
        let socket_path = socket_path.as_ref().to_path_buf();
        match Self::connect(&socket_path) {
            Ok(client) => return Ok(client),
            Err(error) if !daemon_may_be_absent(&error) => return Err(error),
            Err(_) => {}
        }

        let daemon = daemon.as_ref().to_path_buf();
        if !daemon.is_file() {
            return Err(ClientError::DaemonBinaryMissing(daemon));
        }
        let state_dir = state_dir.as_ref();
        std::fs::create_dir_all(state_dir)?;
        let state_metadata = std::fs::symlink_metadata(state_dir)?;
        if state_metadata.file_type().is_symlink() || !state_metadata.is_dir() {
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "runtime state path is not a real directory",
            )));
        }
        std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))?;
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(state_dir.join("daemon.log"))?;
        log.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        let errors = log.try_clone()?;
        let mut child = ProcessCommand::new(daemon)
            .args(leading_arguments)
            .arg("--socket")
            .arg(&socket_path)
            .arg("--state-dir")
            .arg(state_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(errors))
            .process_group(0)
            .spawn()?;

        let deadline = Instant::now() + DAEMON_START_TIMEOUT;
        while Instant::now() < deadline {
            match Self::connect(&socket_path) {
                Ok(client) => return Ok(client),
                Err(error) if !daemon_may_be_absent(&error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
                Err(_) => {}
            }
            if let Some(status) = child.try_wait()? {
                return Err(ClientError::DaemonExited(status.to_string()));
            }
            thread::sleep(DAEMON_POLL_INTERVAL);
        }
        let _ = child.kill();
        let _ = child.wait();
        Err(ClientError::DaemonStartTimeout)
    }

    pub fn request(&self, action: Request) -> Result<ResponseBody, ClientError> {
        self.request_at(action, None)
    }

    fn request_at(
        &self,
        action: Request,
        expected_mission_version: Option<u64>,
    ) -> Result<ResponseBody, ClientError> {
        self.request_with_metadata(action, expected_mission_version, Uuid::new_v4())
    }

    fn request_with_metadata(
        &self,
        action: Request,
        expected_mission_version: Option<u64>,
        request_id: Uuid,
    ) -> Result<ResponseBody, ClientError> {
        self.request_with_terminal_metadata(
            action,
            expected_mission_version,
            request_id,
            None,
            None,
        )
    }

    fn request_with_terminal_metadata(
        &self,
        action: Request,
        expected_mission_version: Option<u64>,
        request_id: Uuid,
        surface_id: Option<Uuid>,
        control_epoch: Option<u64>,
    ) -> Result<ResponseBody, ClientError> {
        let mut request = self.client_request(action);
        request.request_id = request_id;
        request.surface_id = surface_id;
        request.control_epoch = control_epoch;
        request.expected_mission_version = expected_mission_version;
        let stream = {
            let stream = self
                .stream
                .lock()
                .map_err(|_| ClientError::ControlPoisoned)?;
            Arc::clone(&stream)
        };
        let response = match stream.exchange(&request) {
            Ok(response) => response,
            Err(error) => {
                // Repair the shared channel for the next action, but never
                // replay this action: terminal input is deliberately not
                // idempotent and an ACK may have been lost after execution.
                if let Ok(reconnected) = connect_wire(self.connector.as_ref(), self.client_id)
                    && let Ok(mut stream) = self.stream.lock()
                {
                    *stream = reconnected;
                }
                return Err(error);
            }
        };
        decode_response(&request, response)
    }

    fn client_request(&self, action: Request) -> ClientRequest {
        let mut request = ClientRequest::for_client(self.client_id, action);
        request.share_token = self.share_token.as_deref().map(str::to_owned);
        request
    }

    fn current_wire(&self) -> Result<Arc<MultiplexedWire>, ClientError> {
        self.stream
            .lock()
            .map(|stream| Arc::clone(&stream))
            .map_err(|_| ClientError::ControlPoisoned)
    }

    fn subscription_wire(&self) -> Result<Arc<MultiplexedWire>, ClientError> {
        let current = self.current_wire()?;
        if !current.closed.load(Ordering::Acquire) {
            return Ok(current);
        }
        let reconnected = connect_wire(self.connector.as_ref(), self.client_id)?;
        let mut slot = self
            .stream
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?;
        if slot.closed.load(Ordering::Acquire) {
            *slot = Arc::clone(&reconnected);
            Ok(reconnected)
        } else {
            Ok(Arc::clone(&slot))
        }
    }

    pub fn start_terminal(&self, spec: TerminalSessionSpec) -> Result<DaemonSession, ClientError> {
        let session_id = spec.session_id;
        let surface_id = Uuid::new_v4();
        match self.request_with_terminal_metadata(
            Request::StartTerminal { spec },
            None,
            Uuid::new_v4(),
            Some(surface_id),
            None,
        )? {
            ResponseBody::TerminalStarted { terminal } => Ok(DaemonSession {
                session_id,
                surface_id,
                control_epoch: Arc::new(AtomicU64::new(terminal.control_epoch)),
                control: self.clone(),
            }),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn terminal(&self, session_id: SessionId) -> DaemonSession {
        DaemonSession {
            session_id,
            surface_id: Uuid::new_v4(),
            control_epoch: Arc::new(AtomicU64::new(0)),
            control: self.clone(),
        }
    }

    pub fn list_terminals(&self) -> Result<Vec<TerminalSessionSummary>, ClientError> {
        self.list_terminals_with_archived(false)
    }

    pub fn list_all_terminals(&self) -> Result<Vec<TerminalSessionSummary>, ClientError> {
        self.list_terminals_with_archived(true)
    }

    fn list_terminals_with_archived(
        &self,
        include_archived: bool,
    ) -> Result<Vec<TerminalSessionSummary>, ClientError> {
        match self.request(Request::ListTerminals { include_archived })? {
            ResponseBody::Terminals { terminals } => Ok(terminals),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn subscribe_terminals(
        &self,
    ) -> Result<MetadataReceiver<TerminalSessionSummary>, ClientError> {
        let stream = self.open_terminal_index_subscription()?;
        let reconnect = self.clone();
        Ok(MetadataReceiver::new(
            stream,
            move || reconnect.open_terminal_index_subscription(),
            |bytes| {
                let TerminalIndexEvent::TerminalChanged { terminal } =
                    serde_json::from_slice(bytes)?;
                Ok(terminal)
            },
        ))
    }

    pub fn create_session_group(
        &self,
        group: SessionGroupSpec,
    ) -> Result<SessionGroupSummary, ClientError> {
        match self.request(Request::CreateSessionGroup { group })? {
            ResponseBody::SessionGroup { group } => Ok(group),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn list_session_groups(
        &self,
        mission_id: Option<MissionId>,
    ) -> Result<Vec<SessionGroupSummary>, ClientError> {
        match self.request(Request::ListSessionGroups { mission_id })? {
            ResponseBody::SessionGroups { groups } => Ok(groups),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn update_session_group(
        &self,
        group_id: SessionGroupId,
        expected_version: u64,
        change: SessionGroupChange,
    ) -> Result<SessionGroupSummary, ClientError> {
        match self.request(Request::UpdateSessionGroup {
            group_id,
            expected_version,
            change,
        })? {
            ResponseBody::SessionGroup { group } => Ok(group),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Apply an explicit group edit against fresh shared state. Clones serialize
    /// their edits; a concurrent client can cause a bounded CAS retry. Never
    /// retry an ambiguous transport failure, since the write may have committed.
    pub fn sync_session_group(
        &self,
        spec: SessionGroupSpec,
        change: SessionGroupChange,
    ) -> Result<SessionGroupSummary, ClientError> {
        let _guard = self
            .group_mutations
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?;
        let groups = self.list_session_groups(spec.mission_id)?;
        let mut group = match groups.into_iter().find(|group| {
            group.group_id == spec.group_id
                || (!spec.session_ids.is_empty() && group.session_ids == spec.session_ids)
        }) {
            Some(group) => group,
            None => self.create_session_group(spec)?,
        };
        for attempt in 0..4 {
            match self.update_session_group(group.group_id, group.version, change.clone()) {
                Ok(updated) => return Ok(updated),
                Err(error @ ClientError::Remote { .. }) if attempt < 3 => {
                    let latest = self
                        .list_session_groups(group.mission_id)?
                        .into_iter()
                        .find(|latest| latest.group_id == group.group_id);
                    match latest {
                        Some(latest) if latest.version > group.version => group = latest,
                        _ => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("bounded group update loop always returns")
    }

    pub fn delete_session_group(
        &self,
        group_id: SessionGroupId,
        expected_version: u64,
    ) -> Result<(), ClientError> {
        match self.request(Request::DeleteSessionGroup {
            group_id,
            expected_version,
        })? {
            ResponseBody::SessionGroupDeleted { group_id: deleted } if deleted == group_id => {
                Ok(())
            }
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn subscribe_session_groups(
        &self,
        mission_id: Option<MissionId>,
    ) -> Result<MetadataReceiver<SessionGroupEvent>, ClientError> {
        let stream = self.open_session_group_subscription(mission_id)?;
        let reconnect = self.clone();
        Ok(MetadataReceiver::new(
            stream,
            move || reconnect.open_session_group_subscription(mission_id),
            |bytes| serde_json::from_slice(bytes),
        ))
    }

    fn open_session_group_subscription(
        &self,
        mission_id: Option<MissionId>,
    ) -> Result<MultiplexedSubscription, ClientError> {
        let stream = self.subscription_wire()?;
        let request = self.client_request(Request::SubscribeSessionGroups { mission_id });
        let response = stream.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::SessionGroupSubscriptionAccepted { stream_id } => stream.subscribe(
                stream_id,
                self.client_request(Request::Unsubscribe { stream_id }),
            ),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    fn open_terminal_index_subscription(&self) -> Result<MultiplexedSubscription, ClientError> {
        let stream = self.subscription_wire()?;
        let request = self.client_request(Request::SubscribeTerminals);
        let response = stream.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::TerminalIndexSubscriptionAccepted { stream_id } => stream.subscribe(
                stream_id,
                self.client_request(Request::Unsubscribe { stream_id }),
            ),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn create_mission(
        &self,
        mission_id: MissionId,
        intent: impl Into<String>,
        created_by: Actor,
    ) -> Result<Mission, ClientError> {
        match self.request(Request::CreateMission {
            mission_id,
            intent: intent.into(),
            created_by,
        })? {
            ResponseBody::Mission { mission } => Ok(mission),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn get_mission(&self, mission_id: MissionId) -> Result<Mission, ClientError> {
        match self.request(Request::GetMission { mission_id })? {
            ResponseBody::Mission { mission } => Ok(mission),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Compact read with explicit capability admission on the same wire used
    /// for the request. Never falls back to transferring a whole Mission.
    pub fn verification_status(
        &self,
        mission_id: MissionId,
        verifier_run_id: ultraplexr_core::RunId,
    ) -> Result<ultraplexr_core::VerificationStatus, ClientError> {
        let wire = self.subscription_wire()?;
        if !wire
            .features
            .iter()
            .any(|feature| feature == ultraplexr_protocol::VERIFICATION_STATUS_FEATURE)
        {
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "runtime does not support compact verification status; update the runtime",
            )));
        }
        let request = self.client_request(Request::VerificationStatus {
            mission_id,
            verifier_run_id,
        });
        let response = wire.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::VerificationStatus { status }
                if status.mission_id == mission_id
                    && status.verifier_run_id == verifier_run_id
                    && status.receipts.len() <= 16
                    && status.receipt_count >= status.receipts.len()
                    && status.passing_receipt_count <= status.receipt_count
                    && status.receipts_truncated
                        == (status.receipt_count > status.receipts.len())
                    && status.execution_finished
                        == (status.phase == ultraplexr_core::RunPhase::Finished)
                    && status.observation_only
                    && !status.evidence_rechecked
                    && status
                        .receipts
                        .iter()
                        .all(|receipt| receipt.passing_checks <= receipt.checks)
                    && status.candidate_revision.len() <= 256
                    && status.candidate_sha256.len() == 64
                    && status
                        .candidate_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit()) =>
            {
                Ok(status)
            }
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Read a bounded live verifier page. No Mission clone, implicit page walk,
    /// or compatibility fallback to GetMission. Limits are clamped to 1..=64.
    pub fn verification_catalog(
        &self,
        mission_id: MissionId,
        after: Option<ultraplexr_core::RunId>,
        limit: u16,
    ) -> Result<ultraplexr_core::VerificationCatalog, ClientError> {
        let limit = limit.clamp(1, ultraplexr_core::MAX_VERIFIER_PAGE);
        let wire = self.subscription_wire()?;
        if !wire
            .features
            .iter()
            .any(|feature| feature == ultraplexr_protocol::VERIFICATION_CATALOG_FEATURE)
        {
            return Err(ClientError::Io(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "runtime does not support compact verifier discovery; update the runtime",
            )));
        }
        let request = self.client_request(Request::VerificationCatalog {
            mission_id,
            after,
            limit,
        });
        let response = wire.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::VerificationCatalog { catalog }
                if catalog.mission_id == mission_id
                    && catalog.after == after
                    && catalog.limit == limit
                    && catalog.entries.len() <= usize::from(limit)
                    && catalog
                        .entries
                        .iter()
                        .all(|entry| after.is_none_or(|after| entry.verifier_run_id > after))
                    && catalog
                        .entries
                        .windows(2)
                        .all(|pair| pair[0].verifier_run_id < pair[1].verifier_run_id)
                    && catalog.next_after.is_none_or(|next| {
                        catalog.entries.len() == usize::from(limit)
                            && catalog
                                .entries
                                .last()
                                .is_some_and(|entry| entry.verifier_run_id == next)
                    }) =>
            {
                Ok(catalog)
            }
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn mission_history(
        &self,
        mission_id: MissionId,
        before_sequence: Option<u64>,
        limit: u16,
    ) -> Result<(Vec<MissionHistoryEntry>, bool), ClientError> {
        match self.request(Request::MissionHistory {
            mission_id,
            before_sequence,
            limit,
        })? {
            ResponseBody::MissionHistory {
                entries, has_more, ..
            } => Ok((entries, has_more)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn list_missions(&self) -> Result<Vec<MissionSummary>, ClientError> {
        match self.request(Request::ListMissions)? {
            ResponseBody::Missions { missions } => Ok(missions),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn runtime_diagnostics(&self) -> Result<RuntimeDiagnostics, ClientError> {
        match self.request(Request::RuntimeDiagnostics)? {
            ResponseBody::RuntimeDiagnostics { diagnostics } => Ok(diagnostics),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn list_plugins(&self) -> Result<(Vec<PluginRuntimeSummary>, u64), ClientError> {
        match self.request(Request::ListPlugins)? {
            ResponseBody::Plugins {
                plugins,
                dropped_events,
            } => Ok((plugins, dropped_events)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn scheduler_plan(
        &self,
        mission_id: MissionId,
        max_concurrency: u16,
    ) -> Result<SchedulerPlan, ClientError> {
        match self.request(Request::SchedulerPlan {
            mission_id,
            max_concurrency,
        })? {
            ResponseBody::SchedulerPlan { plan, .. } => Ok(plan),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn launch_configured_scheduler_batch(
        &self,
        mission_id: MissionId,
        max_concurrency: u16,
        session_name_prefix: impl Into<String>,
        cwd: PathBuf,
        grid: GridSize,
    ) -> Result<
        (
            SchedulerPlan,
            Vec<ScheduledAgentLaunch>,
            Vec<ScheduledAgentLaunchFailure>,
        ),
        ClientError,
    > {
        match self.request(Request::LaunchConfiguredSchedulerBatch {
            mission_id,
            max_concurrency,
            session_name_prefix: session_name_prefix.into(),
            cwd,
            grid,
        })? {
            ResponseBody::ConfiguredSchedulerBatchLaunched {
                plan,
                launched,
                failures,
                ..
            } => Ok((plan, launched, failures)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn set_scheduler_policy(
        &self,
        policy: SchedulerPolicy,
    ) -> Result<SchedulerPolicy, ClientError> {
        match self.request(Request::SetSchedulerPolicy { policy })? {
            ResponseBody::SchedulerPolicy { policy } => Ok(policy),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn list_scheduler_policies(&self) -> Result<Vec<SchedulerPolicy>, ClientError> {
        match self.request(Request::ListSchedulerPolicies)? {
            ResponseBody::SchedulerPolicies { policies } => Ok(policies),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn set_scheduler_settings(
        &self,
        settings: SchedulerSettings,
    ) -> Result<SchedulerSettings, ClientError> {
        match self.request(Request::SetSchedulerSettings { settings })? {
            ResponseBody::SchedulerSettings { settings } => Ok(settings),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn scheduler_settings(&self) -> Result<SchedulerSettings, ClientError> {
        match self.request(Request::GetSchedulerSettings)? {
            ResponseBody::SchedulerSettings { settings } => Ok(settings),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn prepare_run_checkout(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        repository: PathBuf,
        base_ref: impl Into<String>,
    ) -> Result<RunCheckoutSummary, ClientError> {
        match self.request(Request::PrepareRunCheckout {
            mission_id,
            run_id,
            repository,
            base_ref: base_ref.into(),
        })? {
            ResponseBody::RunCheckout { checkout } => Ok(checkout),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn list_run_checkouts(
        &self,
        mission_id: Option<MissionId>,
    ) -> Result<Vec<RunCheckoutSummary>, ClientError> {
        match self.request(Request::ListRunCheckouts { mission_id })? {
            ResponseBody::RunCheckouts { checkouts } => Ok(checkouts),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn retire_run_checkout(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        merged_into_ref: impl Into<String>,
    ) -> Result<RunCheckoutSummary, ClientError> {
        match self.request(Request::RetireRunCheckout {
            mission_id,
            run_id,
            merged_into_ref: merged_into_ref.into(),
        })? {
            ResponseBody::RunCheckout { checkout } => Ok(checkout),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn report_provider_fact(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        fact: ProviderFactInput,
    ) -> Result<(ProviderFactSummary, RunActivitySummary), ClientError> {
        match self.request(Request::ReportProviderFact {
            mission_id,
            run_id,
            fact,
        })? {
            ResponseBody::ProviderFactRecorded { fact, activity } => Ok((fact, activity)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn run_activity(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
    ) -> Result<RunActivitySummary, ClientError> {
        match self.request(Request::GetRunActivity { mission_id, run_id })? {
            ResponseBody::RunActivity { activity } => Ok(activity),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn list_run_activities(
        &self,
        mission_id: MissionId,
    ) -> Result<Vec<RunActivitySummary>, ClientError> {
        match self.request(Request::ListRunActivities { mission_id })? {
            ResponseBody::RunActivities { activities } => Ok(activities),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Record which Run has been asked to fix a Fault.
    pub fn assign_fault_fix(
        &self,
        fault_id: FaultId,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
    ) -> Result<FaultSummary, ClientError> {
        match self.request(Request::AssignFaultFix {
            fault_id,
            mission_id,
            run_id,
        })? {
            ResponseBody::FaultRecorded { fault } => Ok(fault),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Record one observed failure with the evidence needed to replay it.
    pub fn report_fault(&self, fault: FaultInput) -> Result<FaultSummary, ClientError> {
        match self.request(Request::ReportFault { fault })? {
            ResponseBody::FaultRecorded { fault } => Ok(fault),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// List Faults newest first, optionally narrowed to a Mission or Session.
    pub fn list_faults(
        &self,
        mission_id: Option<MissionId>,
        session_id: Option<SessionId>,
        include_closed: bool,
    ) -> Result<Vec<FaultSummary>, ClientError> {
        match self.request(Request::ListFaults {
            mission_id,
            session_id,
            include_closed,
        })? {
            ResponseBody::Faults { faults } => Ok(faults),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn fault(&self, fault_id: FaultId) -> Result<FaultSummary, ClientError> {
        match self.request(Request::GetFault { fault_id })? {
            ResponseBody::FaultRecorded { fault } => Ok(fault),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Replay a Fault's command and attach the receipt. This runs the recorded
    /// command, so it may take as long as that command takes.
    /// Re-run the replay of resolved Faults and reopen any that fail again.
    ///
    /// Returns every Fault checked, and those that regressed.
    pub fn guard_faults(
        &self,
        limit: Option<u16>,
        timeout_seconds: Option<u16>,
    ) -> Result<(Vec<FaultId>, Vec<FaultSummary>), ClientError> {
        match self.request(Request::GuardFaults {
            limit,
            timeout_seconds,
        })? {
            ResponseBody::FaultsGuarded { checked, reopened } => Ok((checked, reopened)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn reproduce_fault(
        &self,
        fault_id: FaultId,
        timeout_seconds: Option<u16>,
    ) -> Result<FaultSummary, ClientError> {
        match self.request(Request::ReproduceFault {
            fault_id,
            timeout_seconds,
        })? {
            ResponseBody::FaultRecorded { fault } => Ok(fault),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Close a Fault as fixed. The daemon refuses unless a replay passed.
    pub fn resolve_fault(
        &self,
        fault_id: FaultId,
        note: String,
    ) -> Result<FaultSummary, ClientError> {
        match self.request(Request::ResolveFault { fault_id, note })? {
            ResponseBody::FaultRecorded { fault } => Ok(fault),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Close a Fault without replay evidence, recording why.
    pub fn dismiss_fault(
        &self,
        fault_id: FaultId,
        note: String,
    ) -> Result<FaultSummary, ClientError> {
        match self.request(Request::DismissFault { fault_id, note })? {
            ResponseBody::FaultRecorded { fault } => Ok(fault),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn report_run_evidence(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        evidence: RunEvidenceInput,
    ) -> Result<RunEvidenceSummary, ClientError> {
        match self.request(Request::ReportRunEvidence {
            mission_id,
            run_id,
            evidence,
        })? {
            ResponseBody::RunEvidenceRecorded { evidence } => Ok(evidence),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn list_run_evidence(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
    ) -> Result<Vec<RunEvidenceSummary>, ClientError> {
        match self.request(Request::ListRunEvidence { mission_id, run_id })? {
            ResponseBody::RunEvidence { evidence } => Ok(evidence),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn subscribe_run_activities(
        &self,
        mission_id: Option<MissionId>,
    ) -> Result<MetadataReceiver<RunActivitySummary>, ClientError> {
        let stream = self.open_run_activity_subscription(mission_id)?;
        let reconnect = self.clone();
        Ok(MetadataReceiver::new(
            stream,
            move || reconnect.open_run_activity_subscription(mission_id),
            |bytes| {
                let RunActivityEvent::ActivityChanged { activity } = serde_json::from_slice(bytes)?;
                Ok(activity)
            },
        ))
    }

    fn open_run_activity_subscription(
        &self,
        mission_id: Option<MissionId>,
    ) -> Result<MultiplexedSubscription, ClientError> {
        let stream = self.subscription_wire()?;
        let request = self.client_request(Request::SubscribeRunActivities { mission_id });
        let response = stream.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::RunActivitySubscriptionAccepted { stream_id } => stream.subscribe(
                stream_id,
                self.client_request(Request::Unsubscribe { stream_id }),
            ),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn create_share(
        &self,
        label: impl Into<String>,
        role: ShareRole,
        mission_ids: Vec<MissionId>,
        session_ids: Vec<SessionId>,
        expires_in_seconds: u64,
    ) -> Result<(ShareSummary, String), ClientError> {
        match self.request(Request::CreateShare {
            label: label.into(),
            role,
            mission_ids,
            session_ids,
            expires_in_seconds,
        })? {
            ResponseBody::ShareCreated { share, token } => Ok((share, token)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn list_shares(&self) -> Result<Vec<ShareSummary>, ClientError> {
        match self.request(Request::ListShares)? {
            ResponseBody::Shares { shares } => Ok(shares),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn revoke_share(&self, share_id: Uuid) -> Result<ShareSummary, ClientError> {
        match self.request(Request::RevokeShare { share_id })? {
            ResponseBody::ShareRevoked { share } => Ok(share),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn launch_agent_run(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        session_name: impl Into<String>,
        spec: TerminalSessionSpec,
    ) -> Result<(Vec<Event>, Mission, TerminalSessionSummary), ClientError> {
        match self.request(Request::LaunchAgentRun {
            mission_id,
            run_id,
            session_name: session_name.into(),
            spec,
        })? {
            ResponseBody::AgentRunLaunched {
                events,
                mission,
                terminal,
            } => Ok((events, mission, terminal)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn launch_configured_agent_run(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        session_id: SessionId,
        session_name: impl Into<String>,
        cwd: PathBuf,
        grid: GridSize,
    ) -> Result<(Vec<Event>, Mission, TerminalSessionSummary), ClientError> {
        match self.request(Request::LaunchConfiguredAgentRun {
            mission_id,
            run_id,
            session_id,
            session_name: session_name.into(),
            cwd,
            use_run_checkout: false,
            grid,
        })? {
            ResponseBody::AgentRunLaunched {
                events,
                mission,
                terminal,
            } => Ok((events, mission, terminal)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn launch_configured_agent_run_in_checkout(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        session_id: SessionId,
        session_name: impl Into<String>,
        grid: GridSize,
    ) -> Result<(Vec<Event>, Mission, TerminalSessionSummary), ClientError> {
        match self.request(Request::LaunchConfiguredAgentRun {
            mission_id,
            run_id,
            session_id,
            session_name: session_name.into(),
            cwd: PathBuf::new(),
            use_run_checkout: true,
            grid,
        })? {
            ResponseBody::AgentRunLaunched {
                events,
                mission,
                terminal,
            } => Ok((events, mission, terminal)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn preview_configured_agent_run(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        cwd: PathBuf,
        grid: GridSize,
    ) -> Result<ConfiguredAgentLaunchPreview, ClientError> {
        match self.request(Request::PreviewConfiguredAgentRun {
            mission_id,
            run_id,
            cwd,
            use_run_checkout: false,
            grid,
        })? {
            ResponseBody::ConfiguredAgentLaunchPreview { preview } => Ok(preview),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn preview_configured_agent_run_in_checkout(
        &self,
        mission_id: MissionId,
        run_id: ultraplexr_core::RunId,
        grid: GridSize,
    ) -> Result<ConfiguredAgentLaunchPreview, ClientError> {
        match self.request(Request::PreviewConfiguredAgentRun {
            mission_id,
            run_id,
            cwd: PathBuf::new(),
            use_run_checkout: true,
            grid,
        })? {
            ResponseBody::ConfiguredAgentLaunchPreview { preview } => Ok(preview),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn subscribe_missions(&self) -> Result<MetadataReceiver<Mission>, ClientError> {
        let stream = self.open_mission_subscription()?;
        let reconnect = self.clone();
        Ok(MetadataReceiver::new(
            stream,
            move || reconnect.open_mission_subscription(),
            |bytes| {
                let MissionEvent::MissionChanged { mission } = serde_json::from_slice(bytes)?;
                Ok(mission)
            },
        ))
    }

    fn open_mission_subscription(&self) -> Result<MultiplexedSubscription, ClientError> {
        let stream = self.subscription_wire()?;
        let request = self.client_request(Request::SubscribeMissions);
        let response = stream.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::MissionSubscriptionAccepted { stream_id } => stream.subscribe(
                stream_id,
                self.client_request(Request::Unsubscribe { stream_id }),
            ),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn dispatch(
        &self,
        mission_id: MissionId,
        command: Command,
    ) -> Result<(Vec<Event>, Mission), ClientError> {
        self.dispatch_at(mission_id, None, command)
    }

    pub fn dispatch_at(
        &self,
        mission_id: MissionId,
        expected_version: Option<u64>,
        command: Command,
    ) -> Result<(Vec<Event>, Mission), ClientError> {
        match self.request_at(
            Request::Dispatch {
                mission_id,
                command,
            },
            expected_version,
        )? {
            ResponseBody::EventsCommitted { events, mission } => Ok((events, mission)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn dispatch_idempotent(
        &self,
        mission_id: MissionId,
        expected_version: Option<u64>,
        idempotency_key: Uuid,
        command: Command,
    ) -> Result<(Vec<Event>, Mission), ClientError> {
        match self.request_with_metadata(
            Request::Dispatch {
                mission_id,
                command,
            },
            expected_version,
            idempotency_key,
        )? {
            ResponseBody::EventsCommitted { events, mission } => Ok((events, mission)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }
}

/// Canonical terminal frames and connection state for resumable projections.
#[derive(Debug)]
pub enum TerminalStreamUpdate {
    /// One record cannot fit the client budget. No implicit retry; a caller may
    /// explicitly reattach after reducing the record or viewport size.
    ReceiveLimited,
    /// A canonical frame or terminal lifecycle event. Delta events are expanded
    /// to complete frames by subscribe_frames_with, including after reconnect.
    Event(ServerEvent),
    /// Transport was lost. The same identity/Share is used for every retry.
    Reconnecting,
    /// The runtime rejected reattachment. Do not retry or escalate authority.
    Rejected,
}

/// Cancellation handle for a live terminal event subscription.
#[must_use = "dropping this handle keeps the subscription alive; call cancel to suspend it"]
pub struct EventSubscription {
    cancelled: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<SubscriptionCancel>>>,
    wake: Arc<(Mutex<()>, Condvar)>,
}

impl EventSubscription {
    /// Local wakeup only: the receiving worker owns its eventual Unsubscribe
    /// write. Cancellation neither waits for the socket writer nor reconnects.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Ok(stream) = self.stream.lock()
            && let Some(stream) = stream.as_ref()
        {
            stream.wire.streams.cancel(stream.stream_id);
        }
        let _guard = self
            .wake
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.wake.1.notify_all();
    }
}

#[derive(Clone, Debug)]
pub struct DaemonSession {
    session_id: SessionId,
    surface_id: Uuid,
    control_epoch: Arc<AtomicU64>,
    control: ControlClient,
}

impl DaemonSession {
    #[must_use]
    pub const fn id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub const fn surface_id(&self) -> Uuid {
        self.surface_id
    }

    #[must_use]
    pub fn control_epoch(&self) -> Option<u64> {
        match self.control_epoch.load(Ordering::Acquire) {
            0 => None,
            epoch => Some(epoch),
        }
    }

    pub fn snapshot(&self) -> Result<FullFrame, ClientError> {
        match self.control.request(Request::TerminalSnapshot {
            session_id: self.session_id,
        })? {
            ResponseBody::TerminalFrame { frame, .. } => Ok(*frame),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn capture(&self) -> Result<TerminalCapture, ClientError> {
        match self.control.request(Request::TerminalCapture {
            session_id: self.session_id,
        })? {
            ResponseBody::TerminalCaptured { capture } => Ok(capture),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn wait(
        &self,
        condition: TerminalWaitCondition,
        timeout: Duration,
    ) -> Result<(u64, TerminalCapture), ClientError> {
        let timeout_millis = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        match self.control.request(Request::TerminalWait {
            session_id: self.session_id,
            condition,
            timeout_millis,
        })? {
            ResponseBody::TerminalWaitSatisfied {
                elapsed_millis,
                capture,
                ..
            } => Ok((elapsed_millis, capture)),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Read a viewport from durable terminal history without moving the live
    /// terminal surface's viewport.
    pub fn history_frame(&self, rows_before_bottom: u32) -> Result<FullFrame, ClientError> {
        match self.control.request(Request::TerminalHistoryFrame {
            session_id: self.session_id,
            viewport: HistoryViewport::RowsBeforeBottom(rows_before_bottom),
        })? {
            ResponseBody::TerminalHistoryFrame { frame, .. } => Ok(*frame),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    /// Read the viewport containing an absolute result row returned by search.
    pub fn history_line(&self, row_from_top: u32) -> Result<FullFrame, ClientError> {
        match self.control.request(Request::TerminalHistoryFrame {
            session_id: self.session_id,
            viewport: HistoryViewport::RowFromTop(row_from_top),
        })? {
            ResponseBody::TerminalHistoryFrame { frame, .. } => Ok(*frame),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn send_key(&self, input: KeyInput) -> Result<(), ClientError> {
        self.accept(Request::TerminalKey {
            session_id: self.session_id,
            input,
        })
    }

    pub fn paste(&self, bytes: Vec<u8>, confirmed: bool) -> Result<(), ClientError> {
        self.accept(Request::TerminalPaste {
            session_id: self.session_id,
            bytes,
            confirmed,
        })
    }

    pub fn focus(&self, focused: bool) -> Result<(), ClientError> {
        self.accept(Request::TerminalFocus {
            session_id: self.session_id,
            focused,
        })
    }

    pub fn mouse(&self, input: MouseInput) -> Result<(), ClientError> {
        self.accept(Request::TerminalMouse {
            session_id: self.session_id,
            input,
        })
    }

    pub fn scroll(&self, scroll: ViewportScroll) -> Result<(), ClientError> {
        self.accept(Request::TerminalScroll {
            session_id: self.session_id,
            scroll,
        })
    }

    pub fn select(
        &self,
        anchor: SelectionPoint,
        head: SelectionPoint,
        rectangle: bool,
    ) -> Result<(), ClientError> {
        self.accept(Request::TerminalSelect {
            session_id: self.session_id,
            anchor,
            head,
            rectangle,
        })
    }

    pub fn clear_selection(&self) -> Result<(), ClientError> {
        self.accept(Request::TerminalClearSelection {
            session_id: self.session_id,
        })
    }

    pub fn selection_text(&self) -> Result<Option<String>, ClientError> {
        match self.control.request(Request::TerminalSelectionText {
            session_id: self.session_id,
        })? {
            ResponseBody::TerminalSelectionText { text, .. } => Ok(text),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn search(
        &self,
        query: impl Into<String>,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Vec<SearchMatch>, ClientError> {
        let query = query.into();
        if self
            .control
            .current_wire()?
            .features
            .iter()
            .any(|feature| feature == ultraplexr_protocol::search_stream::FEATURE)
        {
            let mut pages = self.search_pages(query, case_sensitive, limit)?;
            let mut matches = Vec::new();
            while let Some(page) = pages.next_page()? {
                matches.extend(page.matches);
            }
            return Ok(matches);
        }
        match self.control.request(Request::TerminalSearch {
            session_id: self.session_id,
            query,
            case_sensitive,
            limit,
        })? {
            ResponseBody::TerminalSearchResults { matches, .. } => Ok(matches),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn resize(
        &self,
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), ClientError> {
        self.accept(Request::TerminalResize {
            session_id: self.session_id,
            grid,
            cell_width_px,
            cell_height_px,
        })
    }

    pub fn kill(&self) -> Result<(), ClientError> {
        self.accept(Request::KillTerminal {
            session_id: self.session_id,
        })
    }

    pub fn interrupt(&self) -> Result<(), ClientError> {
        self.accept(Request::InterruptTerminal {
            session_id: self.session_id,
        })
    }

    pub fn terminate(&self) -> Result<(), ClientError> {
        self.accept(Request::TerminateTerminal {
            session_id: self.session_id,
        })
    }

    pub fn archive(&self) -> Result<(), ClientError> {
        self.accept(Request::ArchiveTerminal {
            session_id: self.session_id,
        })
    }

    pub fn restore(&self) -> Result<(), ClientError> {
        self.accept(Request::RestoreTerminal {
            session_id: self.session_id,
        })
    }

    pub fn claim_control(&self, force: bool) -> Result<(), ClientError> {
        // Dropping the input handle does not return Control; this convenience
        // method keeps its existing explicit-return contract while sharing the
        // same ambiguous-ACK protection as connection-pinned input callers.
        self.claim_input(force).map(|_| ())
    }

    /// Return Control on the current connection without repairing or replaying
    /// it. Disconnect already releases its leases; teardown must not establish
    /// a replacement connection just to release an obsolete Surface.
    pub fn release_control(&self) -> Result<(), ClientError> {
        let wire = self.control.current_wire()?;
        let mut request = self
            .control
            .client_request(Request::ReleaseTerminalControl {
                session_id: self.session_id,
            });
        request.surface_id = Some(self.surface_id);
        request.control_epoch = self.control_epoch();
        let response = wire.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::TerminalControlChanged { control_epoch, .. } => {
                self.control_epoch.store(control_epoch, Ordering::Release);
                Ok(())
            }
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn subscribe(&self) -> Result<Receiver<ServerEvent>, ClientError> {
        let (send, receive) = std::sync::mpsc::channel();
        let _subscription = self.subscribe_with(move |event| send.send(event).is_ok())?;
        Ok(receive)
    }

    /// Stream terminal events directly into a frontend-owned queue.
    ///
    /// This avoids an extra relay thread for event-loop based frontends while
    /// preserving the blocking receiver API used by command-line clients.
    pub fn subscribe_with(
        &self,
        mut deliver: impl FnMut(ServerEvent) -> bool + Send + 'static,
    ) -> Result<EventSubscription, ClientError> {
        self.subscribe_updates(false, move |update| match update {
            TerminalStreamUpdate::Event(event) => deliver(event),
            TerminalStreamUpdate::Reconnecting => true,
            TerminalStreamUpdate::Rejected | TerminalStreamUpdate::ReceiveLimited => false,
        })
    }

    /// Events (including deltas) with explicit reconnect/rejection state.
    pub fn subscribe_events_with_status(
        &self,
        deliver: impl FnMut(TerminalStreamUpdate) -> bool + Send + 'static,
    ) -> Result<EventSubscription, ClientError> {
        self.subscribe_updates(false, deliver)
    }

    /// Event-driven canonical frames with explicit reconnect/rejection state.
    /// Reconnect subscribes to the SAME Session and starts from a full snapshot;
    /// it never launches a process or replays input. Call cancel when detaching.
    pub fn subscribe_frames_with(
        &self,
        deliver: impl FnMut(TerminalStreamUpdate) -> bool + Send + 'static,
    ) -> Result<EventSubscription, ClientError> {
        self.subscribe_updates(true, deliver)
    }

    fn subscribe_updates(
        &self,
        full_frames: bool,
        mut deliver: impl FnMut(TerminalStreamUpdate) -> bool + Send + 'static,
    ) -> Result<EventSubscription, ClientError> {
        let stream = self.open_subscription()?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation_stream = Arc::new(Mutex::new(Some(stream.cancel.clone())));
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        let reconnect = self.clone();
        let control = self.control.clone();
        let session_id = self.session_id;
        let thread_cancelled = Arc::clone(&cancelled);
        let thread_stream = Arc::clone(&cancellation_stream);
        let thread_wake = Arc::clone(&wake);
        thread::Builder::new()
            .name(format!("ultraplexr-client-events-{}", self.session_id))
            .spawn(move || {
                let mut stream = stream;
                let mut projection: Option<FullFrame> = None;
                let mut retry_delay = Duration::from_millis(100);
                let mut reconnecting = false;
                loop {
                    if thread_cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    match stream.receive_terminal() {
                        Ok(event) => {
                            if thread_cancelled.load(Ordering::Acquire) {
                                break;
                            }
                            retry_delay = Duration::from_millis(100);
                            reconnecting = false;
                            let event = match event {
                                ServerEvent::TerminalFrame { frame, .. } => {
                                    projection = Some((*frame).clone());
                                    ServerEvent::TerminalFrame { session_id, frame }
                                }
                                ServerEvent::TerminalDelta { delta, .. } => {
                                    let applied = projection
                                        .as_mut()
                                        .is_some_and(|frame| delta.apply_to(frame).is_ok());
                                    if applied && !full_frames {
                                        ServerEvent::TerminalDelta { session_id, delta }
                                    } else {
                                        if !applied {
                                            if thread_cancelled.load(Ordering::Acquire) {
                                                break;
                                            }
                                            projection =
                                                control.terminal(session_id).snapshot().ok();
                                        }
                                        let Some(frame) = projection.clone() else {
                                            continue;
                                        };
                                        ServerEvent::TerminalFrame {
                                            session_id,
                                            frame: Box::new(frame),
                                        }
                                    }
                                }
                                event => event,
                            };
                            let terminal = matches!(
                                event,
                                ServerEvent::TerminalExited { .. }
                                    | ServerEvent::TerminalFailed { .. }
                            );
                            if thread_cancelled.load(Ordering::Acquire) {
                                break;
                            }
                            if !deliver(TerminalStreamUpdate::Event(event)) || terminal {
                                break;
                            }
                        }
                        Err(ClientError::ReceiveLimit { .. }) => {
                            if !thread_cancelled.load(Ordering::Acquire) {
                                deliver(TerminalStreamUpdate::ReceiveLimited);
                            }
                            break;
                        }
                        Err(_) => {
                            if thread_cancelled.load(Ordering::Acquire) {
                                break;
                            }
                            if !reconnecting && !deliver(TerminalStreamUpdate::Reconnecting) {
                                break;
                            }
                            reconnecting = true;
                            let wait = thread_wake
                                .0
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            let (wait, _) = thread_wake
                                .1
                                .wait_timeout_while(wait, retry_delay, |_| {
                                    !thread_cancelled.load(Ordering::Acquire)
                                })
                                .unwrap_or_else(|error| error.into_inner());
                            drop(wait);
                            if thread_cancelled.load(Ordering::Acquire) {
                                break;
                            }
                            retry_delay = (retry_delay * 2).min(Duration::from_secs(2));
                            match reconnect.open_subscription() {
                                Ok(reconnected) => {
                                    if thread_cancelled.load(Ordering::Acquire) {
                                        break;
                                    }
                                    if let Ok(mut cancellation) = thread_stream.lock() {
                                        if thread_cancelled.load(Ordering::Acquire) {
                                            break;
                                        }
                                        *cancellation = Some(reconnected.cancel.clone());
                                    }
                                    stream = reconnected;
                                    projection = None;
                                }
                                Err(ClientError::ReceiveLimit { .. }) => {
                                    if !thread_cancelled.load(Ordering::Acquire) {
                                        deliver(TerminalStreamUpdate::ReceiveLimited);
                                    }
                                    break;
                                }
                                Err(ClientError::Remote { .. } | ClientError::Version { .. }) => {
                                    if !thread_cancelled.load(Ordering::Acquire) {
                                        deliver(TerminalStreamUpdate::Rejected);
                                    }
                                    break;
                                }
                                Err(_) => {}
                            }
                        }
                    }
                }
            })?;
        Ok(EventSubscription {
            cancelled,
            stream: cancellation_stream,
            wake,
        })
    }

    fn open_subscription(&self) -> Result<MultiplexedSubscription, ClientError> {
        let stream = self.control.subscription_wire()?;
        let request = self.control.client_request(Request::SubscribeTerminal {
            session_id: self.session_id,
        });
        let response = stream.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::TerminalSubscriptionAccepted { stream_id, .. } => stream.subscribe(
                stream_id,
                self.control
                    .client_request(Request::Unsubscribe { stream_id }),
            ),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    fn accept(&self, request: Request) -> Result<(), ClientError> {
        match self.request_as_surface(request)? {
            ResponseBody::TerminalCommandAccepted { .. } => Ok(()),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    fn request_as_surface(&self, request: Request) -> Result<ResponseBody, ClientError> {
        self.control.request_with_terminal_metadata(
            request,
            None,
            Uuid::new_v4(),
            Some(self.surface_id),
            self.control_epoch(),
        )
    }
}

fn connect_wire(
    connector: &dyn Connector,
    client_id: Uuid,
) -> Result<Arc<MultiplexedWire>, ClientError> {
    let connection = connector.connect(client_id)?;
    MultiplexedWire::start(connection)
}

fn daemon_may_be_absent(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            )
    )
}

fn decode_response(
    request: &ClientRequest,
    response: ServerResponse,
) -> Result<ResponseBody, ClientError> {
    if response.version != PROTOCOL_VERSION {
        return Err(ClientError::Version {
            actual: response.version,
        });
    }
    if response.request_id != request.request_id {
        return Err(ClientError::RequestMismatch);
    }
    match response.result {
        ResponseResult::Success { body } => Ok(*body),
        ResponseResult::Error { code, message } => Err(ClientError::Remote { code, message }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_wire_multiplexes_subscriptions_and_control_requests() {
        let (client_socket, server_socket) = UnixStream::pair().expect("socket pair should open");
        let shutdown = client_socket.try_clone().expect("shutdown handle");
        let connection = transport::Connection::from_negotiated(
            SyncWire::new(client_socket),
            |stream| Ok((stream.try_clone()?, stream)),
            move || {
                let _ = shutdown.shutdown(std::net::Shutdown::Both);
            },
        )
        .expect("client wire should split");
        let wire = MultiplexedWire::start(connection).expect("dispatcher should start");
        let client_id = Uuid::new_v4();
        let runtime = thread::spawn(move || {
            let mut server = SyncWire::new(server_socket);
            let subscribe: ClientRequest = server
                .receive_json(FrameKind::Request, 0)
                .expect("subscription request should arrive");
            server
                .send_json(
                    FrameKind::Response,
                    0,
                    &ServerResponse::success(
                        subscribe.request_id,
                        ResponseBody::MissionSubscriptionAccepted { stream_id: 7 },
                    ),
                )
                .expect("acceptance should send");
            server
                .send_json(
                    FrameKind::EventBatch,
                    7,
                    &serde_json::json!({"kind": "snapshot", "sequence": 1}),
                )
                .expect("subscription event should send");

            let ping: ClientRequest = server
                .receive_json(FrameKind::Request, 0)
                .expect("control request should share the connection");
            server
                .send_json(
                    FrameKind::Response,
                    0,
                    &ServerResponse::success(ping.request_id, ResponseBody::Pong),
                )
                .expect("control response should send");

            let unsubscribe: ClientRequest = server
                .receive_json(FrameKind::Request, 0)
                .expect("stream cancellation should preserve the connection");
            assert_eq!(unsubscribe.action, Request::Unsubscribe { stream_id: 7 });
        });

        let subscribe = ClientRequest::for_client(client_id, Request::SubscribeMissions);
        let accepted = wire
            .exchange(&subscribe)
            .expect("subscription should be accepted");
        assert!(matches!(
            accepted.result,
            ResponseResult::Success { body }
                if *body == ResponseBody::MissionSubscriptionAccepted { stream_id: 7 }
        ));
        let unsubscribe =
            ClientRequest::for_client(client_id, Request::Unsubscribe { stream_id: 7 });
        let subscription = wire
            .subscribe(7, unsubscribe)
            .expect("logical stream should register");
        let subscription = MetadataReceiver::new(
            subscription,
            || Err(disconnected_error("test does not reconnect")),
            |bytes| serde_json::from_slice::<serde_json::Value>(bytes),
        );
        assert_eq!(
            subscription.recv().expect("early event should be retained"),
            serde_json::json!({"kind": "snapshot", "sequence": 1})
        );

        let ping = ClientRequest::for_client(client_id, Request::Ping);
        let response = wire
            .exchange(&ping)
            .expect("control exchange should remain available");
        assert!(matches!(
            response.result,
            ResponseResult::Success { body } if *body == ResponseBody::Pong
        ));
        drop(subscription);
        runtime.join().expect("runtime fixture should stop");
    }

    #[test]
    fn cancelling_an_event_subscription_marks_it_without_closing_the_shared_connection() {
        let subscription = EventSubscription {
            cancelled: Arc::new(AtomicBool::new(false)),
            stream: Arc::new(Mutex::new(None)),
            wake: Arc::new((Mutex::new(()), Condvar::new())),
        };

        subscription.cancel();

        assert!(subscription.cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn only_an_absent_socket_allows_daemon_launch() {
        assert!(daemon_may_be_absent(&ClientError::Io(
            std::io::Error::from(std::io::ErrorKind::NotFound)
        )));
        assert!(daemon_may_be_absent(&ClientError::Io(
            std::io::Error::from(std::io::ErrorKind::ConnectionRefused)
        )));
        assert!(!daemon_may_be_absent(&ClientError::Version { actual: 10 }));
        assert!(!daemon_may_be_absent(&ClientError::Protocol(
            ProtocolError::Io(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
        )));
    }

    #[test]
    fn incompatible_runtime_is_reported_without_attempting_replacement() {
        let nonce = Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("t9c-{}-{}", std::process::id(), &nonce[..8]));
        std::fs::create_dir_all(&root).expect("test directory should exist");
        let socket = root.join("control.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket)
            .expect("test runtime should bind its socket");
        let runtime = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("client should connect");
            let mut wire = SyncWire::new(stream);
            wire.server_handshake(Uuid::new_v4())
                .expect("runtime should negotiate v3");
            let request: ClientRequest = wire
                .receive_json(FrameKind::Request, 0)
                .expect("runtime should read Ping");
            let mut response = ServerResponse::success(request.request_id, ResponseBody::Pong);
            response.version = PROTOCOL_VERSION - 1;
            wire.send_json(FrameKind::Response, 0, &response)
                .expect("runtime should answer Ping");
        });

        let error = ControlClient::connect_or_spawn_program(
            &socket,
            &root,
            root.join("must-not-be-launched"),
            &[],
        )
        .expect_err("incompatible runtime must fail");
        assert!(matches!(
            error,
            ClientError::Version { actual } if actual == PROTOCOL_VERSION - 1
        ));

        runtime.join().expect("test runtime should finish");
        std::fs::remove_dir_all(root).expect("test directory should be removable");
    }
}

#[cfg(test)]
mod request_timeout_tests {
    use super::*;
    use ultraplexr_core::{FaultId, SessionId};

    /// A request that carries its own deadline must outlast it.
    ///
    /// Bounding these at the default would break exactly the callers who asked
    /// to wait longer than it, and they would see a client timeout instead of
    /// the daemon's own answer.
    #[test]
    fn a_caller_chosen_wait_outlives_the_default_bound() {
        let long_wait = Request::TerminalWait {
            session_id: SessionId::new(),
            condition: TerminalWaitCondition::Quiet { quiet_millis: 10 },
            timeout_millis: 600_000,
        };
        let timeout = request_timeout(&long_wait);
        assert!(
            timeout > Duration::from_millis(600_000),
            "a ten minute wait was bounded at {timeout:?}"
        );
        assert!(timeout > DEFAULT_REQUEST_TIMEOUT);
    }

    /// Replaying a Fault runs a real command, which the daemon caps at 900s.
    #[test]
    fn fault_replay_outlives_the_daemons_own_cap() {
        let replay = Request::ReproduceFault {
            fault_id: FaultId::new(),
            timeout_seconds: Some(900),
        };
        assert!(request_timeout(&replay) > Duration::from_secs(900));
    }

    /// Everything else is bounded, so no caller can wait forever.
    #[test]
    fn ordinary_requests_are_bounded() {
        let ordinary = Request::ListTerminals {
            include_archived: false,
        };
        assert_eq!(request_timeout(&ordinary), DEFAULT_REQUEST_TIMEOUT);
        assert!(DEFAULT_REQUEST_TIMEOUT <= Duration::from_secs(60));
    }
}
