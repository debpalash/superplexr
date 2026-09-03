//! Bounded, out-of-process plugin supervision for superplexr.
//!
//! The host-facing interface is deliberately synchronous and non-blocking:
//! [`PluginPublisher::publish`] only attempts to enqueue one semantic event.
//! Process I/O, handshakes, protocol validation, restarts, and shutdown all
//! remain on supervisor-owned threads and never enter a terminal or UI loop.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, RwLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use superplexr_core::{RunId, SessionId};
use thiserror::Error;

pub const PLUGIN_PROTOCOL_VERSION: u16 = 1;
pub const MANIFEST_FILE_NAME: &str = "plugin.json";
const MANIFEST_SCHEMA_VERSION: u16 = 1;
const MAX_MANIFEST_BYTES: u64 = 32 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const COMMAND_QUEUE_CAPACITY: usize = 256;
const PLUGIN_QUEUE_CAPACITY: usize = 64;
const OUTPUT_QUEUE_CAPACITY: usize = 128;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);
const SUPERVISOR_TICK: Duration = Duration::from_millis(20);
const MAX_ID_BYTES: usize = 96;
const MAX_NAME_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 64;
const MAX_STATUS_KEY_BYTES: usize = 128;
const MAX_STATUS_TEXT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    ObserveTerminals,
    ObserveRunActivity,
    PublishStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub schema_version: u16,
    pub id: String,
    pub name: String,
    pub version: String,
    pub executable: PathBuf,
    pub capabilities: Vec<PluginCapability>,
}

impl PluginManifest {
    #[must_use]
    pub fn agent_status() -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            id: "superplexr.agent-status".to_owned(),
            name: "Agent status".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            executable: PathBuf::from("superplexr-agent-status-plugin"),
            capabilities: vec![
                PluginCapability::ObserveTerminals,
                PluginCapability::ObserveRunActivity,
                PluginCapability::PublishStatus,
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginTerminalState {
    Running,
    Exited,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginActivityState {
    Pending,
    Working,
    Idle,
    WaitingInput,
    WaitingApproval,
    Blocked,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginEvent {
    TerminalChanged {
        session_id: SessionId,
        state: PluginTerminalState,
        archived: bool,
    },
    RunActivityChanged {
        run_id: RunId,
        state: PluginActivityState,
    },
}

impl PluginEvent {
    fn required_capability(&self) -> PluginCapability {
        match self {
            Self::TerminalChanged { .. } => PluginCapability::ObserveTerminals,
            Self::RunActivityChanged { .. } => PluginCapability::ObserveRunActivity,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostMessage {
    Hello {
        plugin_id: String,
        granted_capabilities: Vec<PluginCapability>,
    },
    Event {
        event: PluginEvent,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatusLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginMessage {
    Ready {
        plugin_id: String,
    },
    Status {
        key: String,
        level: PluginStatusLevel,
        text: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostEnvelope {
    pub protocol_version: u16,
    pub sequence: u64,
    pub message: HostMessage,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PluginEnvelope {
    pub protocol_version: u16,
    pub message: PluginMessage,
}

/// Small SDK seam used by executable plugins over stdin/stdout.
pub struct PluginConnection<R, W: Write> {
    reader: BufReader<R>,
    writer: BufWriter<W>,
}

impl<R: Read, W: Write> PluginConnection<R, W> {
    #[must_use]
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer: BufWriter::new(writer),
        }
    }

    pub fn receive(&mut self) -> Result<HostEnvelope, PluginError> {
        let envelope: HostEnvelope = read_json_line(&mut self.reader)?;
        if envelope.protocol_version != PLUGIN_PROTOCOL_VERSION {
            return Err(PluginError::UnsupportedProtocol);
        }
        Ok(envelope)
    }

    pub fn send(&mut self, message: PluginMessage) -> Result<(), PluginError> {
        write_json_line(
            &mut self.writer,
            &PluginEnvelope {
                protocol_version: PLUGIN_PROTOCOL_VERSION,
                message,
            },
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PluginRuntimeState {
    Starting,
    Running,
    Backoff,
    Failed,
    Stopped,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginStatus {
    pub key: String,
    pub level: PluginStatusLevel,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginSnapshot {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: PluginRuntimeState,
    pub restart_count: u32,
    pub dropped_events: u64,
    pub last_status: Option<PluginStatus>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SupervisorSnapshot {
    pub plugins: Vec<PluginSnapshot>,
    pub dropped_events: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublishOutcome {
    Enqueued,
    Dropped,
    Disabled,
}

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("plugin JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("plugin root is not a private owner-controlled directory")]
    UnsafeRoot,
    #[error("plugin manifest {0} must be a bounded owner-controlled regular file")]
    UnsafeManifest(PathBuf),
    #[error("plugin manifest {0} exceeds {MAX_MANIFEST_BYTES} bytes")]
    ManifestTooLarge(PathBuf),
    #[error("plugin manifest is invalid: {0}")]
    InvalidManifest(String),
    #[error("plugin executable {0} is unsafe or is not owner-executable")]
    UnsafeExecutable(PathBuf),
    #[error("plugin message exceeds {MAX_MESSAGE_BYTES} bytes")]
    MessageTooLarge,
    #[error("plugin channel closed")]
    ChannelClosed,
    #[error("plugin protocol version is unsupported")]
    UnsupportedProtocol,
    #[error("plugin installation target already exists: {0}")]
    AlreadyInstalled(PathBuf),
}

#[derive(Clone)]
struct PluginDescriptor {
    manifest: PluginManifest,
    directory: PathBuf,
    executable: PathBuf,
}

struct SharedState {
    snapshot: RwLock<SupervisorSnapshot>,
    dropped_events: AtomicU64,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            snapshot: RwLock::new(SupervisorSnapshot::default()),
            dropped_events: AtomicU64::new(0),
        }
    }
}

#[derive(Clone)]
pub struct PluginPublisher {
    commands: Option<SyncSender<SupervisorCommand>>,
    shared: Arc<SharedState>,
}

impl PluginPublisher {
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            commands: None,
            shared: Arc::new(SharedState::default()),
        }
    }

    /// Attempts one bounded enqueue and never waits for a plugin or worker.
    pub fn publish(&self, event: PluginEvent) -> PublishOutcome {
        let Some(commands) = &self.commands else {
            return PublishOutcome::Disabled;
        };
        match commands.try_send(SupervisorCommand::Publish(event)) {
            Ok(()) => PublishOutcome::Enqueued,
            Err(TrySendError::Full(_)) => {
                self.shared.dropped_events.fetch_add(1, Ordering::Relaxed);
                PublishOutcome::Dropped
            }
            Err(TrySendError::Disconnected(_)) => PublishOutcome::Disabled,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> SupervisorSnapshot {
        let mut snapshot = self.shared.snapshot.read().map_or_else(
            |poisoned| poisoned.into_inner().clone(),
            |value| value.clone(),
        );
        snapshot.dropped_events = snapshot
            .dropped_events
            .saturating_add(self.shared.dropped_events.load(Ordering::Relaxed));
        snapshot
    }
}

pub struct PluginSupervisor {
    publisher: PluginPublisher,
    worker: Option<JoinHandle<()>>,
}

impl PluginSupervisor {
    pub fn start(root: impl AsRef<Path>) -> Result<Self, PluginError> {
        let descriptors = discover(root.as_ref())?;
        if descriptors.is_empty() {
            return Ok(Self {
                publisher: PluginPublisher::disabled(),
                worker: None,
            });
        }
        let shared = Arc::new(SharedState {
            snapshot: RwLock::new(SupervisorSnapshot {
                plugins: descriptors
                    .iter()
                    .map(|descriptor| PluginSnapshot {
                        id: descriptor.manifest.id.clone(),
                        name: descriptor.manifest.name.clone(),
                        version: descriptor.manifest.version.clone(),
                        state: PluginRuntimeState::Starting,
                        restart_count: 0,
                        dropped_events: 0,
                        last_status: None,
                        last_error: None,
                    })
                    .collect(),
                dropped_events: 0,
            }),
            dropped_events: AtomicU64::new(0),
        });
        let (send, receive) = mpsc::sync_channel(COMMAND_QUEUE_CAPACITY);
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("superplexr-plugin-supervisor".to_owned())
            .spawn(move || supervisor_loop(descriptors, receive, worker_shared))?;
        Ok(Self {
            publisher: PluginPublisher {
                commands: Some(send),
                shared,
            },
            worker: Some(worker),
        })
    }

    #[must_use]
    pub fn publisher(&self) -> PluginPublisher {
        self.publisher.clone()
    }

    #[must_use]
    pub fn snapshot(&self) -> SupervisorSnapshot {
        self.publisher.snapshot()
    }

    pub fn shutdown(mut self) {
        self.stop_worker();
    }

    fn stop_worker(&mut self) {
        if self.worker.is_none() {
            return;
        }
        if let Some(commands) = &self.publisher.commands {
            let _ = commands.send(SupervisorCommand::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for PluginSupervisor {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

enum SupervisorCommand {
    Publish(PluginEvent),
    Shutdown,
}

enum PluginOutput {
    Message {
        id: String,
        generation: u64,
        message: PluginMessage,
    },
    Failed {
        id: String,
        generation: u64,
        error: String,
    },
}

struct RunningPlugin {
    descriptor: PluginDescriptor,
    child: Child,
    input: SyncSender<HostEnvelope>,
    ready: bool,
    handshake_deadline: Instant,
    restart_at: Option<Instant>,
    restart_count: u32,
    dropped_events: u64,
    generation: u64,
}

fn supervisor_loop(
    descriptors: Vec<PluginDescriptor>,
    commands: Receiver<SupervisorCommand>,
    shared: Arc<SharedState>,
) {
    let (output_send, output_receive) = mpsc::sync_channel(OUTPUT_QUEUE_CAPACITY);
    let mut plugins = descriptors
        .into_iter()
        .filter_map(
            |descriptor| match spawn_plugin(descriptor.clone(), 1, &output_send) {
                Ok(plugin) => Some(plugin),
                Err(error) => {
                    update_snapshot(&shared, &descriptor.manifest.id, |snapshot| {
                        snapshot.state = PluginRuntimeState::Failed;
                        snapshot.last_error = Some(error.to_string());
                    });
                    None
                }
            },
        )
        .collect::<Vec<_>>();
    let mut sequence = 1_u64;

    loop {
        match commands.recv_timeout(SUPERVISOR_TICK) {
            Ok(SupervisorCommand::Publish(event)) => {
                publish_to_plugins(&mut plugins, event, &mut sequence, &shared);
            }
            Ok(SupervisorCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                shutdown_plugins(&mut plugins, &mut sequence, &shared);
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        for _ in 0..OUTPUT_QUEUE_CAPACITY {
            let Ok(output) = output_receive.try_recv() else {
                break;
            };
            accept_plugin_output(&mut plugins, output, &shared);
        }
        maintain_plugins(&mut plugins, &output_send, &shared);
    }
}

fn publish_to_plugins(
    plugins: &mut [RunningPlugin],
    event: PluginEvent,
    sequence: &mut u64,
    shared: &SharedState,
) {
    let required = event.required_capability();
    for plugin in plugins {
        if !plugin.ready || !plugin.descriptor.manifest.capabilities.contains(&required) {
            continue;
        }
        let envelope = HostEnvelope {
            protocol_version: PLUGIN_PROTOCOL_VERSION,
            sequence: *sequence,
            message: HostMessage::Event {
                event: event.clone(),
            },
        };
        *sequence = sequence.saturating_add(1);
        match plugin.input.try_send(envelope) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                plugin.dropped_events = plugin.dropped_events.saturating_add(1);
                update_snapshot(shared, &plugin.descriptor.manifest.id, |snapshot| {
                    snapshot.dropped_events = plugin.dropped_events;
                });
            }
            Err(TrySendError::Disconnected(_)) => {
                mark_for_restart(plugin, "plugin input channel closed", shared);
            }
        }
    }
}

fn accept_plugin_output(plugins: &mut [RunningPlugin], output: PluginOutput, shared: &SharedState) {
    let (id, generation, result) = match output {
        PluginOutput::Message {
            id,
            generation,
            message,
        } => (id, generation, Ok(message)),
        PluginOutput::Failed {
            id,
            generation,
            error,
        } => (id, generation, Err(error)),
    };
    let Some(plugin) = plugins
        .iter_mut()
        .find(|plugin| plugin.descriptor.manifest.id == id)
    else {
        return;
    };
    if plugin.generation != generation {
        return;
    }
    match result {
        Ok(PluginMessage::Ready { plugin_id }) => {
            if plugin.ready || plugin_id != plugin.descriptor.manifest.id {
                mark_for_restart(plugin, "plugin handshake identity mismatch", shared);
                return;
            }
            plugin.ready = true;
            update_snapshot(shared, &id, |snapshot| {
                snapshot.state = PluginRuntimeState::Running;
                snapshot.last_error = None;
            });
        }
        Ok(PluginMessage::Status { key, level, text }) => {
            if !plugin.ready
                || !plugin
                    .descriptor
                    .manifest
                    .capabilities
                    .contains(&PluginCapability::PublishStatus)
                || !bounded_text(&key, MAX_STATUS_KEY_BYTES)
                || !bounded_text(&text, MAX_STATUS_TEXT_BYTES)
            {
                mark_for_restart(
                    plugin,
                    "plugin emitted an invalid or unauthorized status",
                    shared,
                );
                return;
            }
            update_snapshot(shared, &id, |snapshot| {
                snapshot.last_status = Some(PluginStatus { key, level, text });
            });
        }
        Err(error) => mark_for_restart(plugin, &error, shared),
    }
}

fn maintain_plugins(
    plugins: &mut [RunningPlugin],
    output_send: &SyncSender<PluginOutput>,
    shared: &SharedState,
) {
    let now = Instant::now();
    for plugin in plugins {
        if !plugin.ready && plugin.restart_at.is_none() && now >= plugin.handshake_deadline {
            mark_for_restart(plugin, "plugin handshake timed out", shared);
        }
        if plugin.restart_at.is_none() {
            match plugin.child.try_wait() {
                Ok(Some(status)) => {
                    mark_for_restart(plugin, &format!("plugin exited with {status}"), shared);
                }
                Ok(None) => {}
                Err(error) => {
                    mark_for_restart(plugin, &format!("plugin wait failed: {error}"), shared)
                }
            }
        }
        let Some(restart_at) = plugin.restart_at else {
            continue;
        };
        if now < restart_at {
            continue;
        }
        let descriptor = plugin.descriptor.clone();
        let generation = plugin.generation.saturating_add(1);
        match spawn_plugin(descriptor, generation, output_send) {
            Ok(mut replacement) => {
                replacement.restart_count = plugin.restart_count;
                replacement.dropped_events = plugin.dropped_events;
                *plugin = replacement;
                update_snapshot(shared, &plugin.descriptor.manifest.id, |snapshot| {
                    snapshot.state = PluginRuntimeState::Starting;
                    snapshot.restart_count = plugin.restart_count;
                });
            }
            Err(error) => {
                plugin.restart_count = plugin.restart_count.saturating_add(1);
                plugin.restart_at = Some(now + restart_delay(plugin.restart_count));
                update_snapshot(shared, &plugin.descriptor.manifest.id, |snapshot| {
                    snapshot.state = PluginRuntimeState::Backoff;
                    snapshot.restart_count = plugin.restart_count;
                    snapshot.last_error = Some(error.to_string());
                });
            }
        }
    }
}

fn mark_for_restart(plugin: &mut RunningPlugin, error: &str, shared: &SharedState) {
    if plugin.restart_at.is_some() {
        return;
    }
    let _ = plugin.child.kill();
    let _ = plugin.child.wait();
    plugin.ready = false;
    plugin.restart_count = plugin.restart_count.saturating_add(1);
    plugin.restart_at = Some(Instant::now() + restart_delay(plugin.restart_count));
    update_snapshot(shared, &plugin.descriptor.manifest.id, |snapshot| {
        snapshot.state = PluginRuntimeState::Backoff;
        snapshot.restart_count = plugin.restart_count;
        snapshot.last_error = Some(error.to_owned());
    });
}

fn restart_delay(restart_count: u32) -> Duration {
    let exponent = restart_count.saturating_sub(1).min(7);
    Duration::from_millis(250_u64.saturating_mul(1_u64 << exponent)).min(Duration::from_secs(30))
}

fn shutdown_plugins(plugins: &mut [RunningPlugin], sequence: &mut u64, shared: &SharedState) {
    for plugin in plugins.iter_mut() {
        let _ = plugin.input.try_send(HostEnvelope {
            protocol_version: PLUGIN_PROTOCOL_VERSION,
            sequence: *sequence,
            message: HostMessage::Shutdown,
        });
        *sequence = sequence.saturating_add(1);
    }
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    while Instant::now() < deadline {
        let mut all_exited = true;
        for plugin in plugins.iter_mut() {
            match plugin.child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => all_exited = false,
            }
        }
        if all_exited {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    for plugin in plugins {
        if plugin.child.try_wait().ok().flatten().is_none() {
            let _ = plugin.child.kill();
        }
        let _ = plugin.child.wait();
        update_snapshot(shared, &plugin.descriptor.manifest.id, |snapshot| {
            snapshot.state = PluginRuntimeState::Stopped;
        });
    }
}

fn spawn_plugin(
    descriptor: PluginDescriptor,
    generation: u64,
    output_send: &SyncSender<PluginOutput>,
) -> Result<RunningPlugin, PluginError> {
    let mut child = Command::new(&descriptor.executable)
        .current_dir(&descriptor.directory)
        .env_clear()
        .env("SUPERPLEXR_PLUGIN_ID", &descriptor.manifest.id)
        .env(
            "SUPERPLEXR_PLUGIN_PROTOCOL_VERSION",
            PLUGIN_PROTOCOL_VERSION.to_string(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("plugin stdin was not piped"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("plugin stdout was not piped"))?;
    let (input_send, input_receive) = mpsc::sync_channel(PLUGIN_QUEUE_CAPACITY);
    let writer_id = descriptor.manifest.id.clone();
    let writer_output = output_send.clone();
    thread::Builder::new()
        .name(format!("plugin-write-{writer_id}"))
        .spawn(move || {
            plugin_writer(writer_id, generation, stdin, input_receive, writer_output);
        })?;
    let reader_id = descriptor.manifest.id.clone();
    let reader_output = output_send.clone();
    thread::Builder::new()
        .name(format!("plugin-read-{reader_id}"))
        .spawn(move || plugin_reader(reader_id, generation, stdout, reader_output))?;
    input_send
        .try_send(HostEnvelope {
            protocol_version: PLUGIN_PROTOCOL_VERSION,
            sequence: 0,
            message: HostMessage::Hello {
                plugin_id: descriptor.manifest.id.clone(),
                granted_capabilities: descriptor.manifest.capabilities.clone(),
            },
        })
        .map_err(|_| PluginError::ChannelClosed)?;
    Ok(RunningPlugin {
        descriptor,
        child,
        input: input_send,
        ready: false,
        handshake_deadline: Instant::now() + HANDSHAKE_TIMEOUT,
        restart_at: None,
        restart_count: 0,
        dropped_events: 0,
        generation,
    })
}

fn plugin_writer(
    id: String,
    generation: u64,
    stdin: impl Write,
    input: Receiver<HostEnvelope>,
    output: SyncSender<PluginOutput>,
) {
    let mut writer = BufWriter::new(stdin);
    while let Ok(envelope) = input.recv() {
        if let Err(error) = write_json_line(&mut writer, &envelope) {
            let _ = output.send(PluginOutput::Failed {
                id,
                generation,
                error: error.to_string(),
            });
            return;
        }
    }
}

fn plugin_reader(id: String, generation: u64, stdout: impl Read, output: SyncSender<PluginOutput>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let envelope = match read_json_line::<_, PluginEnvelope>(&mut reader) {
            Ok(envelope) => envelope,
            Err(error) => {
                let _ = output.send(PluginOutput::Failed {
                    id,
                    generation,
                    error: error.to_string(),
                });
                return;
            }
        };
        if envelope.protocol_version != PLUGIN_PROTOCOL_VERSION {
            let _ = output.send(PluginOutput::Failed {
                id,
                generation,
                error: PluginError::UnsupportedProtocol.to_string(),
            });
            return;
        }
        if output
            .send(PluginOutput::Message {
                id: id.clone(),
                generation,
                message: envelope.message,
            })
            .is_err()
        {
            return;
        }
    }
}

fn update_snapshot(shared: &SharedState, id: &str, update: impl FnOnce(&mut PluginSnapshot)) {
    let mut snapshot = shared
        .snapshot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(plugin) = snapshot.plugins.iter_mut().find(|plugin| plugin.id == id) {
        update(plugin);
    }
}

fn discover(root: &Path) -> Result<Vec<PluginDescriptor>, PluginError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    validate_private_directory(root)?;
    let mut entries = fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut descriptors = Vec::new();
    let mut ids = HashSet::new();
    for entry in entries {
        let directory = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        validate_private_directory(&directory)?;
        let manifest_path = directory.join(MANIFEST_FILE_NAME);
        let metadata = fs::symlink_metadata(&manifest_path)
            .map_err(|_| PluginError::UnsafeManifest(manifest_path.clone()))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.uid() != current_uid()
            || metadata.permissions().mode() & 0o022 != 0
        {
            return Err(PluginError::UnsafeManifest(manifest_path));
        }
        if metadata.len() > MAX_MANIFEST_BYTES {
            return Err(PluginError::ManifestTooLarge(manifest_path));
        }
        let manifest: PluginManifest = serde_json::from_reader(File::open(&manifest_path)?)?;
        validate_manifest(&manifest)?;
        if !ids.insert(manifest.id.clone()) {
            return Err(PluginError::InvalidManifest(format!(
                "duplicate plugin ID {:?}",
                manifest.id
            )));
        }
        let executable = resolve_executable(&directory, &manifest.executable)?;
        descriptors.push(PluginDescriptor {
            manifest,
            directory,
            executable,
        });
    }
    Ok(descriptors)
}

pub fn validate_manifest(manifest: &PluginManifest) -> Result<(), PluginError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(PluginError::InvalidManifest(format!(
            "unsupported schema version {}",
            manifest.schema_version
        )));
    }
    if !valid_id(&manifest.id) {
        return Err(PluginError::InvalidManifest("invalid plugin ID".to_owned()));
    }
    if !bounded_text(&manifest.name, MAX_NAME_BYTES) {
        return Err(PluginError::InvalidManifest(
            "invalid plugin name".to_owned(),
        ));
    }
    if !bounded_text(&manifest.version, MAX_VERSION_BYTES) {
        return Err(PluginError::InvalidManifest(
            "invalid plugin version".to_owned(),
        ));
    }
    if manifest.capabilities.is_empty() {
        return Err(PluginError::InvalidManifest(
            "plugin declares no capabilities".to_owned(),
        ));
    }
    let mut capabilities = HashSet::new();
    if manifest
        .capabilities
        .iter()
        .any(|capability| !capabilities.insert(*capability))
    {
        return Err(PluginError::InvalidManifest(
            "plugin declares duplicate capabilities".to_owned(),
        ));
    }
    validate_relative_executable(&manifest.executable)
}

fn validate_relative_executable(path: &Path) -> Result<(), PluginError> {
    if path.as_os_str().is_empty()
        || path.components().any(|component| {
            !matches!(component, Component::Normal(_))
                || component.as_os_str().to_string_lossy().contains('\0')
        })
    {
        return Err(PluginError::InvalidManifest(
            "executable must be a relative path without traversal".to_owned(),
        ));
    }
    Ok(())
}

fn resolve_executable(directory: &Path, relative: &Path) -> Result<PathBuf, PluginError> {
    validate_relative_executable(relative)?;
    let executable = directory.join(relative);
    let mut candidate = directory.to_owned();
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        candidate.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&candidate)
            .map_err(|_| PluginError::UnsafeExecutable(executable.clone()))?;
        let final_component = index + 1 == components.len();
        if metadata.file_type().is_symlink()
            || metadata.uid() != current_uid()
            || metadata.permissions().mode() & 0o022 != 0
            || (final_component
                && (!metadata.is_file() || metadata.permissions().mode() & 0o100 == 0))
            || (!final_component && !metadata.is_dir())
        {
            return Err(PluginError::UnsafeExecutable(executable));
        }
    }
    Ok(executable)
}

fn validate_private_directory(path: &Path) -> Result<(), PluginError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| PluginError::UnsafeRoot)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != current_uid()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(PluginError::UnsafeRoot);
    }
    Ok(())
}

fn current_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and does not dereference memory.
    unsafe { libc::geteuid() }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn read_json_line<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> Result<T, PluginError> {
    let mut bytes = Vec::with_capacity(1024);
    let read = reader
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if read == 0 {
        return Err(PluginError::ChannelClosed);
    }
    if bytes.len() > MAX_MESSAGE_BYTES || !bytes.ends_with(b"\n") {
        return Err(PluginError::MessageTooLarge);
    }
    bytes.pop();
    Ok(serde_json::from_slice(&bytes)?)
}

fn write_json_line<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), PluginError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() + 1 > MAX_MESSAGE_BYTES {
        return Err(PluginError::MessageTooLarge);
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// Installs the bundled agent-status plugin without replacing an existing one.
pub fn install_agent_status(
    plugin_root: impl AsRef<Path>,
    source_binary: impl AsRef<Path>,
) -> Result<PathBuf, PluginError> {
    let root = plugin_root.as_ref();
    if !root.exists() {
        fs::create_dir_all(root)?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    validate_private_directory(root)?;
    let source = source_binary.as_ref();
    let source_metadata = fs::symlink_metadata(source)?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
        return Err(PluginError::UnsafeExecutable(source.to_owned()));
    }
    let target = root.join("superplexr.agent-status");
    if target.exists() {
        return Err(PluginError::AlreadyInstalled(target));
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| io::Error::other(error.to_string()))?
        .as_nanos();
    let staging = root.join(format!(
        ".agent-status-install-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&staging)?;
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
    let mut guard = InstallGuard {
        path: staging.clone(),
        committed: false,
    };
    let executable = staging.join("superplexr-agent-status-plugin");
    fs::copy(source, &executable)?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
    let manifest_path = staging.join(MANIFEST_FILE_NAME);
    let mut manifest_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&manifest_path)?;
    serde_json::to_writer_pretty(&mut manifest_file, &PluginManifest::agent_status())?;
    manifest_file.write_all(b"\n")?;
    manifest_file.sync_all()?;
    File::open(&staging)?.sync_all()?;
    fs::rename(&staging, &target)?;
    File::open(root)?.sync_all()?;
    guard.committed = true;
    Ok(target)
}

struct InstallGuard {
    path: PathBuf,
    committed: bool,
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(1);

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "superplexr-plugin-{}-{}",
                std::process::id(),
                NEXT_TEMP_ROOT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("temporary plugin root should be created");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .expect("temporary plugin root should be private");
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn manifest_rejects_traversal_and_duplicate_capabilities() {
        let mut manifest = PluginManifest::agent_status();
        manifest.executable = PathBuf::from("../escape");
        assert!(validate_manifest(&manifest).is_err());
        manifest.executable = PathBuf::from("plugin");
        manifest.capabilities.push(PluginCapability::PublishStatus);
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn disabled_publication_is_constant_and_non_blocking() {
        let publisher = PluginPublisher::disabled();
        assert_eq!(
            publisher.publish(PluginEvent::TerminalChanged {
                session_id: SessionId::new(),
                state: PluginTerminalState::Running,
                archived: false,
            }),
            PublishOutcome::Disabled
        );
        assert_eq!(publisher.snapshot(), SupervisorSnapshot::default());
    }

    #[test]
    fn saturated_host_queue_drops_instead_of_waiting() {
        let (send, _receive) = mpsc::sync_channel(1);
        let publisher = PluginPublisher {
            commands: Some(send),
            shared: Arc::new(SharedState::default()),
        };
        let event = PluginEvent::TerminalChanged {
            session_id: SessionId::new(),
            state: PluginTerminalState::Running,
            archived: false,
        };
        assert_eq!(publisher.publish(event.clone()), PublishOutcome::Enqueued);
        assert_eq!(publisher.publish(event), PublishOutcome::Dropped);
        assert_eq!(publisher.snapshot().dropped_events, 1);
    }

    #[test]
    fn restart_backoff_is_bounded() {
        assert_eq!(restart_delay(1), Duration::from_millis(250));
        assert_eq!(restart_delay(2), Duration::from_millis(500));
        assert_eq!(restart_delay(u32::MAX), Duration::from_secs(30));
    }

    #[test]
    fn executable_path_rejects_an_intermediate_symlink() {
        let root = TempRoot::new();
        let outside = root.0.join(".outside");
        fs::create_dir(&outside).expect("outside fixture should exist");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o700))
            .expect("outside fixture should be private");
        let executable = outside.join("fixture");
        fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("outside executable should write");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("outside executable should be owner-executable");
        let plugin = root.0.join("fixture");
        fs::create_dir(&plugin).expect("plugin fixture should exist");
        fs::set_permissions(&plugin, fs::Permissions::from_mode(0o700))
            .expect("plugin fixture should be private");
        symlink(&outside, plugin.join("bin")).expect("fixture symlink should be created");
        let manifest = PluginManifest {
            executable: PathBuf::from("bin/fixture"),
            ..PluginManifest::agent_status()
        };
        let manifest_path = plugin.join(MANIFEST_FILE_NAME);
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("fixture manifest should encode"),
        )
        .expect("fixture manifest should write");
        fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600))
            .expect("fixture manifest should be private");
        assert!(matches!(
            PluginSupervisor::start(&root.0),
            Err(PluginError::UnsafeExecutable(_))
        ));
    }

    #[test]
    fn supervisor_handshakes_and_accepts_bounded_status() {
        let root = TempRoot::new();
        let plugin = root.0.join("fixture");
        fs::create_dir(&plugin).expect("fixture directory should exist");
        fs::set_permissions(&plugin, fs::Permissions::from_mode(0o700))
            .expect("fixture directory should be private");
        let executable = plugin.join("fixture.sh");
        fs::write(
            &executable,
            "#!/bin/sh\nIFS= read -r hello || exit 1\nprintf '%s\\n' '{\"protocol_version\":1,\"message\":{\"type\":\"ready\",\"plugin_id\":\"fixture.status\"}}'\nIFS= read -r event || exit 1\nprintf '%s\\n' '{\"protocol_version\":1,\"message\":{\"type\":\"status\",\"key\":\"fixture\",\"level\":\"success\",\"text\":\"event received\"}}'\nIFS= read -r shutdown || exit 0\n",
        )
        .expect("fixture executable should write");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("fixture executable should be private and executable");
        let manifest = PluginManifest {
            schema_version: 1,
            id: "fixture.status".to_owned(),
            name: "Fixture status".to_owned(),
            version: "1.0.0".to_owned(),
            executable: PathBuf::from("fixture.sh"),
            capabilities: vec![
                PluginCapability::ObserveTerminals,
                PluginCapability::PublishStatus,
            ],
        };
        let manifest_path = plugin.join(MANIFEST_FILE_NAME);
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("fixture manifest should encode"),
        )
        .expect("fixture manifest should write");
        fs::set_permissions(&manifest_path, fs::Permissions::from_mode(0o600))
            .expect("fixture manifest should be private");

        let supervisor = PluginSupervisor::start(&root.0).expect("supervisor should start");
        wait_until(Duration::from_secs(2), || {
            supervisor.snapshot().plugins[0].state == PluginRuntimeState::Running
        });
        assert_eq!(
            supervisor
                .publisher()
                .publish(PluginEvent::TerminalChanged {
                    session_id: SessionId::new(),
                    state: PluginTerminalState::Running,
                    archived: false,
                }),
            PublishOutcome::Enqueued
        );
        wait_until(Duration::from_secs(2), || {
            supervisor.snapshot().plugins[0]
                .last_status
                .as_ref()
                .is_some_and(|status| status.text == "event received")
        });
        supervisor.shutdown();
    }

    fn wait_until(timeout: Duration, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(condition(), "condition should become true before timeout");
    }
}
