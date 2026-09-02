//! Native blocking client for the local termi9ne daemon.
//!
//! GPUI and CLI callers do not need to own a Tokio runtime. A control channel
//! is shared and serialized, while one reader dispatches independently
//! sequenced subscription streams without placing socket I/O on GPUI threads.

use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    fs::OpenOptions,
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::UnixStream,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use termi9ne_core::{Actor, Command, Event, FaultId, Mission, MissionId, SchedulerPlan, SessionId};
use termi9ne_protocol::{
    ClientRequest, ConfiguredAgentLaunchPreview, FaultInput, FaultSummary, MissionEvent,
    MissionHistoryEntry, MissionSummary, PROTOCOL_VERSION, PluginRuntimeSummary, ProtocolError,
    ProviderFactInput, ProviderFactSummary, Request, ResponseBody, ResponseResult,
    RunActivityEvent, RunActivitySummary, RunCheckoutSummary, RunEvidenceInput, RunEvidenceSummary,
    RuntimeDiagnostics, ScheduledAgentLaunch, ScheduledAgentLaunchFailure, SchedulerPolicy,
    SchedulerSettings, ServerEvent, ServerResponse, SessionGroupChange, SessionGroupEvent,
    SessionGroupId, SessionGroupSpec, SessionGroupSummary, ShareRole, ShareSummary,
    TerminalCapture, TerminalIndexEvent, TerminalSessionSpec, TerminalSessionSummary,
    TerminalWaitCondition, decode_terminal_event,
    wire_v3::{
        BlockingWireReader, BlockingWireWriter, FrameKind, Hello, ReceivedFrame, SyncWire,
        WireError,
    },
};
use termi9ne_terminal::{
    FullFrame, GridSize, HistoryViewport, KeyInput, MouseInput, SearchMatch, SelectionPoint,
    ViewportScroll,
};
use thiserror::Error;
use uuid::Uuid;

const DAEMON_START_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Error)]
pub enum ClientError {
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
}

struct MultiplexedWire {
    writer: Mutex<BlockingWireWriter<UnixStream>>,
    pending: Mutex<HashMap<Uuid, Sender<Result<ServerResponse, String>>>>,
    subscriptions: Mutex<HashMap<u32, Sender<ReceivedFrame>>>,
    early_frames: Mutex<HashMap<u32, VecDeque<ReceivedFrame>>>,
    closed: AtomicBool,
}

impl MultiplexedWire {
    fn start(
        reader: BlockingWireReader<UnixStream>,
        writer: BlockingWireWriter<UnixStream>,
    ) -> Result<Arc<Self>, ClientError> {
        let wire = Arc::new(Self {
            writer: Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            subscriptions: Mutex::new(HashMap::new()),
            early_frames: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        });
        let dispatcher = Arc::clone(&wire);
        thread::Builder::new()
            .name("termi9ne-wire-dispatch".to_owned())
            .spawn(move || dispatcher.dispatch(reader))?;
        Ok(wire)
    }

    fn dispatch(self: Arc<Self>, mut reader: BlockingWireReader<UnixStream>) {
        'dispatch: while let Ok(frame) = reader.receive() {
            if frame.header.stream_id == 0 && frame.header.kind == FrameKind::Response {
                match serde_json::from_slice::<ServerResponse>(&frame.payload) {
                    Ok(response) => {
                        if let Ok(mut pending) = self.pending.lock()
                            && let Some(sender) = pending.remove(&response.request_id)
                        {
                            let _ = sender.send(Ok(response));
                        }
                    }
                    Err(error) => {
                        if let Ok(mut pending) = self.pending.lock() {
                            for (_, sender) in pending.drain() {
                                let _ = sender.send(Err(error.to_string()));
                            }
                        }
                        break;
                    }
                }
                continue;
            }
            let stream_id = frame.header.stream_id;
            let sender = self
                .subscriptions
                .lock()
                .ok()
                .and_then(|subscriptions| subscriptions.get(&stream_id).cloned());
            if let Some(sender) = sender {
                let _ = sender.send(frame);
            } else if stream_id != 0
                && let Ok(mut early) = self.early_frames.lock()
            {
                let queue = early.entry(stream_id).or_default();
                if queue.len() >= 1_024 {
                    // A subscription is registered immediately after its
                    // acceptance response. Reaching this bound signals a
                    // malformed or stalled peer; close instead of silently
                    // dropping ordered events.
                    break 'dispatch;
                }
                queue.push_back(frame);
            }
        }
        self.closed.store(true, Ordering::Release);
        if let Ok(mut pending) = self.pending.lock() {
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err("daemon connection closed".to_owned()));
            }
        }
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            subscriptions.clear();
        }
    }

    fn exchange(&self, request: &ClientRequest) -> Result<ServerResponse, ClientError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(disconnected_error("daemon connection is closed"));
        }
        let (send, receive) = std::sync::mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?
            .insert(request.request_id, send);
        let send_result = self
            .writer
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?
            .send_json(FrameKind::Request, 0, request);
        if let Err(error) = send_result {
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&request.request_id);
            }
            return Err(ProtocolError::from(error).into());
        }
        receive
            .recv()
            .map_err(|_| disconnected_error("daemon response dispatcher stopped"))?
            .map_err(|message| disconnected_error(&message))
    }

    fn subscribe(
        self: &Arc<Self>,
        stream_id: u32,
        unsubscribe_request: ClientRequest,
    ) -> Result<MultiplexedSubscription, ClientError> {
        if stream_id == 0 {
            return Err(disconnected_error(
                "runtime assigned subscription stream zero",
            ));
        }
        let (send, receive) = std::sync::mpsc::channel();
        self.subscriptions
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?
            .insert(stream_id, send.clone());
        if let Ok(mut early) = self.early_frames.lock()
            && let Some(mut frames) = early.remove(&stream_id)
        {
            while let Some(frame) = frames.pop_front() {
                if send.send(frame).is_err() {
                    break;
                }
            }
        }
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
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            subscriptions.remove(&stream_id);
        }
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.send_json(FrameKind::Request, 0, request);
        }
    }
}

struct MultiplexedSubscription {
    stream_id: u32,
    receive: Receiver<ReceivedFrame>,
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
    fn receive_json<T: serde::de::DeserializeOwned>(&self) -> Result<T, ClientError> {
        let frame = self
            .receive
            .recv()
            .map_err(|_| disconnected_error("subscription stream closed"))?;
        if frame.header.kind != FrameKind::EventBatch || frame.header.stream_id != self.stream_id {
            return Err(ProtocolError::Wire(WireError::UnexpectedFrame {
                expected: FrameKind::EventBatch,
                expected_stream: self.stream_id,
                actual: frame.header.kind,
                actual_stream: frame.header.stream_id,
            })
            .into());
        }
        Ok(serde_json::from_slice(&frame.payload).map_err(ProtocolError::from)?)
    }

    fn receive_terminal(&self) -> Result<ServerEvent, ClientError> {
        let frame = self
            .receive
            .recv()
            .map_err(|_| disconnected_error("terminal subscription stream closed"))?;
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
    socket_path: PathBuf,
    client_id: Uuid,
    share_token: Option<Arc<str>>,
    share_role: Option<ShareRole>,
    stream: Arc<Mutex<Arc<MultiplexedWire>>>,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlClient")
            .field("socket_path", &self.socket_path)
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
        let socket_path = socket_path.as_ref().to_path_buf();
        let client_id = Uuid::new_v4();
        let stream = connect_wire(&socket_path, client_id)?;
        let mut client = Self {
            socket_path,
            client_id,
            share_token: share_token.map(Arc::<str>::from),
            share_role: None,
            stream: Arc::new(Mutex::new(stream)),
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
            .join("termi9ne-server");
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
                if let Ok(reconnected) = connect_wire(&self.socket_path, self.client_id)
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
        let reconnected = connect_wire(&self.socket_path, self.client_id)?;
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

    pub fn subscribe_terminals(&self) -> Result<Receiver<TerminalSessionSummary>, ClientError> {
        let stream = self.open_terminal_index_subscription()?;
        let (send, receive) = std::sync::mpsc::channel();
        let reconnect = self.clone();
        thread::Builder::new()
            .name("termi9ne-terminal-index".to_owned())
            .spawn(move || {
                let mut stream = stream;
                loop {
                    match stream.receive_json() {
                        Ok(TerminalIndexEvent::TerminalChanged { terminal }) => {
                            if send.send(terminal).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_millis(50));
                            match reconnect.open_terminal_index_subscription() {
                                Ok(reconnected) => stream = reconnected,
                                Err(_) => thread::sleep(Duration::from_millis(200)),
                            }
                        }
                    }
                }
            })?;
        Ok(receive)
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
    ) -> Result<Receiver<SessionGroupEvent>, ClientError> {
        let stream = self.open_session_group_subscription(mission_id)?;
        let (send, receive) = std::sync::mpsc::channel();
        let reconnect = self.clone();
        thread::Builder::new()
            .name("termi9ne-session-groups".to_owned())
            .spawn(move || {
                let mut stream = stream;
                loop {
                    match stream.receive_json() {
                        Ok(event) => {
                            if send.send(event).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_millis(50));
                            match reconnect.open_session_group_subscription(mission_id) {
                                Ok(reconnected) => stream = reconnected,
                                Err(_) => thread::sleep(Duration::from_millis(200)),
                            }
                        }
                    }
                }
            })?;
        Ok(receive)
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
    ) -> Result<Vec<RunEvidenceSummary>, ClientError> {
        match self.request(Request::ListRunEvidence { mission_id, run_id })? {
            ResponseBody::RunEvidence { evidence } => Ok(evidence),
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn subscribe_run_activities(
        &self,
        mission_id: Option<MissionId>,
    ) -> Result<Receiver<RunActivitySummary>, ClientError> {
        let stream = self.open_run_activity_subscription(mission_id)?;
        let (send, receive) = std::sync::mpsc::channel();
        let reconnect = self.clone();
        thread::Builder::new()
            .name("termi9ne-run-activity".to_owned())
            .spawn(move || {
                let mut stream = stream;
                loop {
                    match stream.receive_json() {
                        Ok(RunActivityEvent::ActivityChanged { activity }) => {
                            if send.send(activity).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_millis(50));
                            match reconnect.open_run_activity_subscription(mission_id) {
                                Ok(reconnected) => stream = reconnected,
                                Err(_) => thread::sleep(Duration::from_millis(200)),
                            }
                        }
                    }
                }
            })?;
        Ok(receive)
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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
        run_id: termi9ne_core::RunId,
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

    pub fn subscribe_missions(&self) -> Result<Receiver<Mission>, ClientError> {
        let stream = self.open_mission_subscription()?;
        let (send, receive) = std::sync::mpsc::channel();
        let reconnect = self.clone();
        thread::Builder::new()
            .name("termi9ne-mission-subscription".to_owned())
            .spawn(move || {
                let mut stream = stream;
                loop {
                    match stream.receive_json() {
                        Ok(MissionEvent::MissionChanged { mission }) => {
                            if send.send(mission).is_err() {
                                break;
                            }
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_millis(50));
                            match reconnect.open_mission_subscription() {
                                Ok(reconnected) => stream = reconnected,
                                Err(_) => thread::sleep(Duration::from_millis(200)),
                            }
                        }
                    }
                }
            })?;
        Ok(receive)
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

/// Cancellation handle for a live terminal event subscription.
#[must_use = "dropping this handle keeps the subscription alive; call cancel to suspend it"]
pub struct EventSubscription {
    cancelled: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<SubscriptionCancel>>>,
}

impl EventSubscription {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        if let Ok(stream) = self.stream.lock()
            && let Some(stream) = stream.as_ref()
        {
            stream.cancel();
        }
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
        match self.control.request(Request::TerminalSearch {
            session_id: self.session_id,
            query: query.into(),
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
        match self.request_as_surface(Request::ClaimTerminalControl {
            session_id: self.session_id,
            force,
        })? {
            ResponseBody::TerminalControlChanged { control_epoch, .. } => {
                self.control_epoch.store(control_epoch, Ordering::Release);
                Ok(())
            }
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }

    pub fn release_control(&self) -> Result<(), ClientError> {
        match self.request_as_surface(Request::ReleaseTerminalControl {
            session_id: self.session_id,
        })? {
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
        let stream = self.open_subscription()?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancellation_stream = Arc::new(Mutex::new(Some(stream.cancel.clone())));
        let reconnect = self.clone();
        let control = self.control.clone();
        let session_id = self.session_id;
        let thread_cancelled = Arc::clone(&cancelled);
        let thread_stream = Arc::clone(&cancellation_stream);
        thread::Builder::new()
            .name(format!("termi9ne-client-events-{}", self.session_id))
            .spawn(move || {
                let mut stream = stream;
                let mut projection: Option<FullFrame> = None;
                loop {
                    if thread_cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    match stream.receive_terminal() {
                        Ok(event) => {
                            let event = match event {
                                ServerEvent::TerminalFrame { frame, .. } => {
                                    projection = Some((*frame).clone());
                                    ServerEvent::TerminalFrame { session_id, frame }
                                }
                                ServerEvent::TerminalDelta { delta, .. } => {
                                    let applied = projection
                                        .as_mut()
                                        .is_some_and(|frame| delta.apply_to(frame).is_ok());
                                    if applied {
                                        ServerEvent::TerminalDelta { session_id, delta }
                                    } else {
                                        projection = control.terminal(session_id).snapshot().ok();
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
                            if !deliver(event) || terminal {
                                break;
                            }
                        }
                        Err(_) => {
                            if thread_cancelled.load(Ordering::Acquire) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(50));
                            match reconnect.open_subscription() {
                                Ok(reconnected) => {
                                    if thread_cancelled.load(Ordering::Acquire) {
                                        break;
                                    }
                                    if let Ok(mut cancellation) = thread_stream.lock() {
                                        *cancellation = Some(reconnected.cancel.clone());
                                    }
                                    stream = reconnected;
                                    projection = None;
                                }
                                Err(_) => thread::sleep(Duration::from_millis(200)),
                            }
                        }
                    }
                }
            })?;
        Ok(EventSubscription {
            cancelled,
            stream: cancellation_stream,
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

fn connect_wire(socket_path: &Path, client_id: Uuid) -> Result<Arc<MultiplexedWire>, ClientError> {
    let stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(DAEMON_START_TIMEOUT))?;
    stream.set_write_timeout(Some(DAEMON_START_TIMEOUT))?;
    let mut wire = SyncWire::new(stream);
    wire.client_handshake(&Hello::new(client_id, "desktop", client_id))
        .map_err(ProtocolError::from)?;
    wire.get_ref().set_read_timeout(None)?;
    wire.get_ref().set_write_timeout(None)?;
    let (reader, writer) = wire.into_blocking_split()?;
    MultiplexedWire::start(reader, writer)
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
        let (reader, writer) = SyncWire::new(client_socket)
            .into_blocking_split()
            .expect("client wire should split");
        let wire = MultiplexedWire::start(reader, writer).expect("dispatcher should start");
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
        assert_eq!(
            subscription
                .receive_json::<serde_json::Value>()
                .expect("early event should be retained"),
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
