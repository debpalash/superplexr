//! Runtime-owned durable terminal sessions.
//!
//! A [`SessionHandle`] is cheap to clone and may be held by protocol clients,
//! desktop projections, or tests. The PTY, child process, output journal, and
//! canonical Ghostty terminal model remain on one owning actor thread.

pub mod command_blocks;

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use termi9ne_core::SessionId;
use termi9ne_terminal::{
    FullFrame, GridSize, KeyInput, MouseInput, PasteConfirmation, SearchMatch, SelectionPoint,
    TerminalAction, TerminalEffects, TerminalError, TerminalModel, ViewportScroll,
};
use thiserror::Error;

const COMMAND_CAPACITY: usize = 256;
const SUBSCRIBER_CAPACITY: usize = 64;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
const FRAME_INTERVAL: Duration = Duration::from_millis(8);
const ACTOR_TICK: Duration = Duration::from_millis(16);
const FOREGROUND_PROCESS_INTERVAL: Duration = Duration::from_millis(400);
const EXIT_DRAIN_TIMEOUT: Duration = Duration::from_millis(500);
const HUP_GRACE: Duration = Duration::from_secs(2);
const TERM_GRACE: Duration = Duration::from_secs(3);

/// Complete process configuration validated before a PTY child is started.
#[derive(Clone, Debug)]
pub struct SessionSpec {
    pub id: SessionId,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment_delta: BTreeMap<String, Option<String>>,
    pub grid: GridSize,
}

impl SessionSpec {
    /// Build an ordinary interactive shell specification using the configured
    /// shell, falling back to `/bin/sh`.
    pub fn shell(id: SessionId, grid: GridSize) -> Result<Self, RuntimeError> {
        Ok(Self {
            id,
            program: std::env::var_os("SHELL")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/bin/sh")),
            args: Vec::new(),
            cwd: std::env::current_dir()?,
            environment_delta: BTreeMap::new(),
            grid,
        })
    }

    fn validate(&self) -> Result<(), RuntimeError> {
        if !self.cwd.is_dir() {
            return Err(RuntimeError::InvalidWorkingDirectory(self.cwd.clone()));
        }
        for (key, value) in &self.environment_delta {
            if key.is_empty() || key.contains(['=', '\0']) {
                return Err(RuntimeError::InvalidEnvironmentKey(key.clone()));
            }
            if value.as_ref().is_some_and(|value| value.contains('\0')) {
                return Err(RuntimeError::InvalidEnvironmentValue(key.clone()));
            }
        }
        Ok(())
    }
}

/// Terminal process outcome retained after the final frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionExit {
    pub code: u32,
    pub signal: Option<String>,
    pub success: bool,
}

/// Ordered updates published by a live Session actor.
#[derive(Clone, Debug)]
pub enum SessionEvent {
    Frame(Arc<FullFrame>),
    /// The PTY foreground process group changed. Resolution of its executable
    /// stays outside the latency-sensitive Session actor.
    ForegroundProcessChanged {
        process_id: u32,
    },
    Bell {
        count: u64,
    },
    /// One shell command completed, as reported by OSC 133 marks. Only shells
    /// that emit those marks produce this event.
    CommandFinished(command_blocks::CommandBlock),
    PasteConfirmation(PasteConfirmation),
    TerminationEscalationRequired,
    Exited(SessionExit),
    Failed {
        message: String,
    },
}

/// Errors exposed at the runtime seam.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("session working directory is not a directory: {0}")]
    InvalidWorkingDirectory(PathBuf),
    #[error("invalid environment key: {0:?}")]
    InvalidEnvironmentKey(String),
    #[error("environment value for {0:?} contains NUL")]
    InvalidEnvironmentValue(String),
    #[error("PTY setup failed: {0}")]
    Pty(String),
    #[error("runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Terminal(#[from] TerminalError),
    #[error("session actor stopped")]
    ActorStopped,
    #[error("session actor did not respond before the deadline")]
    ActorTimeout,
}

#[derive(Debug)]
enum ActorMessage {
    PtyOutput(Vec<u8>),
    PtyReadEnded(Option<String>),
    Key(KeyInput),
    Paste {
        bytes: Vec<u8>,
        confirmed: bool,
    },
    Focus(bool),
    Mouse(MouseInput),
    Scroll(ViewportScroll),
    Select {
        anchor: SelectionPoint,
        head: SelectionPoint,
        rectangle: bool,
    },
    ClearSelection,
    SelectionText(SyncSender<Result<Option<String>, String>>),
    Search {
        query: String,
        case_sensitive: bool,
        limit: usize,
        reply: SyncSender<Result<Vec<SearchMatch>, String>>,
    },
    Resize {
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    Subscribe(SyncSender<SessionEvent>),
    Snapshot(SyncSender<Result<Arc<FullFrame>, String>>),
    Interrupt,
    Terminate,
    Kill,
}

/// Cloneable control interface for one durable runtime-owned Session.
#[derive(Clone, Debug)]
pub struct SessionHandle {
    id: SessionId,
    commands: SyncSender<ActorMessage>,
    process_id: Option<u32>,
    tty_name: Option<PathBuf>,
    journal_path: PathBuf,
}

impl SessionHandle {
    /// Spawn the configured process and actor. Success means the child, PTY,
    /// journal, and canonical terminal model are all ready.
    pub fn spawn(spec: SessionSpec, state_dir: impl AsRef<Path>) -> Result<Self, RuntimeError> {
        Self::spawn_internal(spec, state_dir.as_ref(), None)
    }

    /// Spawn with an observer installed before the child can publish output or
    /// exit. This closes the lifecycle race for very short-lived commands.
    pub fn spawn_subscribed(
        spec: SessionSpec,
        state_dir: impl AsRef<Path>,
    ) -> Result<(Self, Receiver<SessionEvent>), RuntimeError> {
        let (send, receive) = mpsc::sync_channel(SUBSCRIBER_CAPACITY);
        let handle = Self::spawn_internal(spec, state_dir.as_ref(), Some(send))?;
        Ok((handle, receive))
    }

    fn spawn_internal(
        spec: SessionSpec,
        state_dir: &Path,
        initial_subscriber: Option<SyncSender<SessionEvent>>,
    ) -> Result<Self, RuntimeError> {
        spec.validate()?;
        let id = spec.id;
        let session_dir = state_dir.join("sessions").join(id.to_string());
        let journal_path = session_dir.join("output.raw");
        let (commands, actor_commands) = mpsc::sync_channel(COMMAND_CAPACITY);
        let actor_sender = commands.clone();
        let (startup_send, startup_receive) = mpsc::sync_channel(1);

        thread::Builder::new()
            .name(format!("termi9ne-session-{id}"))
            .spawn(move || {
                if let Err(error) = run_session_actor(
                    spec,
                    session_dir,
                    actor_sender,
                    actor_commands,
                    startup_send.clone(),
                    initial_subscriber,
                ) {
                    let _ = startup_send.send(Err(error));
                }
            })?;

        let startup =
            startup_receive
                .recv_timeout(STARTUP_TIMEOUT)
                .map_err(|error| match error {
                    RecvTimeoutError::Timeout => RuntimeError::ActorTimeout,
                    RecvTimeoutError::Disconnected => RuntimeError::ActorStopped,
                })??;

        Ok(Self {
            id,
            commands,
            process_id: startup.process_id,
            tty_name: startup.tty_name,
            journal_path,
        })
    }

    #[must_use]
    pub const fn id(&self) -> SessionId {
        self.id
    }

    #[must_use]
    pub const fn process_id(&self) -> Option<u32> {
        self.process_id
    }

    #[must_use]
    pub fn tty_name(&self) -> Option<&Path> {
        self.tty_name.as_deref()
    }

    #[must_use]
    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    pub fn subscribe(&self) -> Result<Receiver<SessionEvent>, RuntimeError> {
        let (send, receive) = mpsc::sync_channel(SUBSCRIBER_CAPACITY);
        self.send(ActorMessage::Subscribe(send))?;
        Ok(receive)
    }

    pub fn snapshot(&self) -> Result<Arc<FullFrame>, RuntimeError> {
        let (send, receive) = mpsc::sync_channel(1);
        self.send(ActorMessage::Snapshot(send))?;
        receive
            .recv_timeout(SNAPSHOT_TIMEOUT)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => RuntimeError::ActorTimeout,
                RecvTimeoutError::Disconnected => RuntimeError::ActorStopped,
            })?
            .map_err(RuntimeError::Pty)
    }

    pub fn send_key(&self, input: KeyInput) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Key(input))
    }

    pub fn paste(&self, bytes: Vec<u8>, confirmed: bool) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Paste { bytes, confirmed })
    }

    pub fn focus(&self, focused: bool) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Focus(focused))
    }

    pub fn mouse(&self, input: MouseInput) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Mouse(input))
    }

    pub fn scroll(&self, scroll: ViewportScroll) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Scroll(scroll))
    }

    pub fn select(
        &self,
        anchor: SelectionPoint,
        head: SelectionPoint,
        rectangle: bool,
    ) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Select {
            anchor,
            head,
            rectangle,
        })
    }

    pub fn clear_selection(&self) -> Result<(), RuntimeError> {
        self.send(ActorMessage::ClearSelection)
    }

    pub fn selection_text(&self) -> Result<Option<String>, RuntimeError> {
        let (send, receive) = mpsc::sync_channel(1);
        self.send(ActorMessage::SelectionText(send))?;
        receive
            .recv_timeout(SNAPSHOT_TIMEOUT)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => RuntimeError::ActorTimeout,
                RecvTimeoutError::Disconnected => RuntimeError::ActorStopped,
            })?
            .map_err(RuntimeError::Pty)
    }

    pub fn search(
        &self,
        query: String,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Vec<SearchMatch>, RuntimeError> {
        let (send, receive) = mpsc::sync_channel(1);
        self.send(ActorMessage::Search {
            query,
            case_sensitive,
            limit,
            reply: send,
        })?;
        receive
            .recv_timeout(SNAPSHOT_TIMEOUT)
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => RuntimeError::ActorTimeout,
                RecvTimeoutError::Disconnected => RuntimeError::ActorStopped,
            })?
            .map_err(RuntimeError::Pty)
    }

    pub fn resize(
        &self,
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Resize {
            grid,
            cell_width_px,
            cell_height_px,
        })
    }

    pub fn kill(&self) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Kill)
    }

    pub fn interrupt(&self) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Interrupt)
    }

    pub fn terminate(&self) -> Result<(), RuntimeError> {
        self.send(ActorMessage::Terminate)
    }

    fn send(&self, message: ActorMessage) -> Result<(), RuntimeError> {
        self.commands
            .send(message)
            .map_err(|_| RuntimeError::ActorStopped)
    }
}

struct Startup {
    process_id: Option<u32>,
    tty_name: Option<PathBuf>,
}

fn run_session_actor(
    spec: SessionSpec,
    session_dir: PathBuf,
    actor_sender: SyncSender<ActorMessage>,
    commands: Receiver<ActorMessage>,
    startup: SyncSender<Result<Startup, RuntimeError>>,
    initial_subscriber: Option<SyncSender<SessionEvent>>,
) -> Result<(), RuntimeError> {
    fs::create_dir_all(&session_dir)?;
    fs::set_permissions(&session_dir, fs::Permissions::from_mode(0o700))?;
    let journal = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(session_dir.join("output.raw"))?;
    journal.set_permissions(fs::Permissions::from_mode(0o600))?;
    let mut journal = BufWriter::new(journal);
    let mut model = TerminalModel::new(spec.grid)?;
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(pty_size(spec.grid, 0, 0))
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    let mut command = CommandBuilder::new(&spec.program);
    command.args(&spec.args);
    command.cwd(&spec.cwd);
    command.env("TERM", "xterm-ghostty");
    command.env("COLORTERM", "truecolor");
    command.env("TERM_PROGRAM", "termi9ne");
    command.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
    if let Some(terminfo) = bundled_terminfo_directory() {
        command.env("TERMINFO", terminfo);
    }
    for (key, value) in &spec.environment_delta {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        }
    }

    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    drop(pair.slave);
    let process_id = child.process_id();
    let tty_name = pair.master.tty_name();
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
    let master = pair.master;
    let mut current_frame = Arc::new(model.frame()?);
    let mut subscribers = initial_subscriber.into_iter().collect::<Vec<_>>();
    let mut observed_exit = None;
    let mut last_output = Instant::now();
    let mut last_frame_publish = Instant::now();
    let mut last_foreground_process_check = Instant::now()
        .checked_sub(FOREGROUND_PROCESS_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut foreground_process_id = None;
    let mut frame_dirty = false;
    let mut journal_dirty = false;
    let mut termination = None;
    let mut command_blocks = command_blocks::CommandBlockTracker::new();

    let reader_sender = actor_sender;
    thread::Builder::new()
        .name(format!("termi9ne-pty-reader-{}", spec.id))
        .spawn(move || read_pty(&mut reader, &reader_sender))?;

    startup
        .send(Ok(Startup {
            process_id,
            tty_name,
        }))
        .map_err(|_| RuntimeError::ActorStopped)?;

    loop {
        if last_foreground_process_check.elapsed() >= FOREGROUND_PROCESS_INTERVAL {
            let next_process_id = master
                .process_group_leader()
                .and_then(|process_id| u32::try_from(process_id).ok());
            if next_process_id != foreground_process_id {
                foreground_process_id = next_process_id;
                if let Some(process_id) = next_process_id {
                    broadcast(
                        &mut subscribers,
                        SessionEvent::ForegroundProcessChanged { process_id },
                    );
                }
            }
            last_foreground_process_check = Instant::now();
        }
        if (frame_dirty || journal_dirty) && last_frame_publish.elapsed() >= FRAME_INTERVAL {
            flush_pending_output(
                &mut model,
                &mut current_frame,
                &mut journal,
                &mut journal_dirty,
                &mut frame_dirty,
                &mut subscribers,
            )?;
            last_frame_publish = Instant::now();
        }
        let receive_timeout = if frame_dirty || journal_dirty {
            FRAME_INTERVAL.saturating_sub(last_frame_publish.elapsed())
        } else {
            ACTOR_TICK
        };
        match commands.recv_timeout(receive_timeout) {
            Ok(ActorMessage::PtyOutput(bytes)) => {
                last_output = Instant::now();
                journal.write_all(&bytes)?;
                journal_dirty = true;
                for block in command_blocks.consume(&bytes) {
                    broadcast(&mut subscribers, SessionEvent::CommandFinished(block));
                }
                let effects = model.advance(TerminalAction::Output(&bytes))?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
                frame_dirty = true;
            }
            Ok(ActorMessage::PtyReadEnded(read_error)) => {
                flush_pending_output(
                    &mut model,
                    &mut current_frame,
                    &mut journal,
                    &mut journal_dirty,
                    &mut frame_dirty,
                    &mut subscribers,
                )?;
                let status = match observed_exit.take() {
                    Some(status) => status,
                    None => child
                        .wait()
                        .map_err(|error| RuntimeError::Pty(error.to_string()))?,
                };
                if let Some(message) = read_error.filter(|_| !status.success()) {
                    broadcast(&mut subscribers, SessionEvent::Failed { message });
                }
                broadcast(
                    &mut subscribers,
                    SessionEvent::Exited(SessionExit {
                        code: status.exit_code(),
                        signal: status.signal().map(str::to_owned),
                        success: status.success(),
                    }),
                );
                break;
            }
            Ok(ActorMessage::Key(input)) => {
                let effects = model.advance(TerminalAction::EncodeKey(&input))?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
            }
            Ok(ActorMessage::Paste { bytes, confirmed }) => {
                let effects = model.advance(TerminalAction::Paste {
                    bytes: &bytes,
                    confirmed,
                })?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
            }
            Ok(ActorMessage::Focus(focused)) => {
                let effects = model.advance(TerminalAction::Focus { focused })?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
            }
            Ok(ActorMessage::Mouse(input)) => {
                let effects = model.advance(TerminalAction::Mouse(&input))?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
            }
            Ok(ActorMessage::Scroll(scroll)) => {
                let effects = model.advance(TerminalAction::Scroll(scroll))?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
                frame_dirty = true;
                flush_pending_output(
                    &mut model,
                    &mut current_frame,
                    &mut journal,
                    &mut journal_dirty,
                    &mut frame_dirty,
                    &mut subscribers,
                )?;
                last_frame_publish = Instant::now();
            }
            Ok(ActorMessage::Select {
                anchor,
                head,
                rectangle,
            }) => {
                let effects = model.advance(TerminalAction::Select {
                    anchor,
                    head,
                    rectangle,
                })?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
                frame_dirty = true;
                flush_pending_output(
                    &mut model,
                    &mut current_frame,
                    &mut journal,
                    &mut journal_dirty,
                    &mut frame_dirty,
                    &mut subscribers,
                )?;
                last_frame_publish = Instant::now();
            }
            Ok(ActorMessage::ClearSelection) => {
                let effects = model.advance(TerminalAction::ClearSelection)?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
                frame_dirty = true;
                flush_pending_output(
                    &mut model,
                    &mut current_frame,
                    &mut journal,
                    &mut journal_dirty,
                    &mut frame_dirty,
                    &mut subscribers,
                )?;
                last_frame_publish = Instant::now();
            }
            Ok(ActorMessage::SelectionText(reply)) => {
                let result = model.selected_text().map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            Ok(ActorMessage::Search {
                query,
                case_sensitive,
                limit,
                reply,
            }) => {
                let result = model
                    .search(&query, case_sensitive, limit)
                    .map_err(|error| error.to_string());
                let _ = reply.send(result);
            }
            Ok(ActorMessage::Resize {
                grid,
                cell_width_px,
                cell_height_px,
            }) => {
                master
                    .resize(pty_size(grid, cell_width_px, cell_height_px))
                    .map_err(|error| RuntimeError::Pty(error.to_string()))?;
                let effects = model.advance(TerminalAction::Resize {
                    grid,
                    cell_width_px,
                    cell_height_px,
                })?;
                publish_effects(&effects, &mut writer, &mut subscribers)?;
                frame_dirty = true;
                flush_pending_output(
                    &mut model,
                    &mut current_frame,
                    &mut journal,
                    &mut journal_dirty,
                    &mut frame_dirty,
                    &mut subscribers,
                )?;
                last_frame_publish = Instant::now();
            }
            Ok(ActorMessage::Subscribe(subscriber)) => {
                flush_pending_output(
                    &mut model,
                    &mut current_frame,
                    &mut journal,
                    &mut journal_dirty,
                    &mut frame_dirty,
                    &mut subscribers,
                )?;
                last_frame_publish = Instant::now();
                if subscriber
                    .try_send(SessionEvent::Frame(Arc::clone(&current_frame)))
                    .is_ok()
                {
                    subscribers.push(subscriber);
                }
            }
            Ok(ActorMessage::Snapshot(reply)) => {
                flush_pending_output(
                    &mut model,
                    &mut current_frame,
                    &mut journal,
                    &mut journal_dirty,
                    &mut frame_dirty,
                    &mut subscribers,
                )?;
                last_frame_publish = Instant::now();
                let _ = reply.send(Ok(Arc::clone(&current_frame)));
            }
            Ok(ActorMessage::Interrupt) => {
                signal_foreground(&*master, libc::SIGINT)?;
            }
            Ok(ActorMessage::Terminate) => {
                signal_foreground(&*master, libc::SIGHUP)?;
                termination = Some((Instant::now(), TerminationStage::Hangup));
            }
            Ok(ActorMessage::Kill) => {
                child
                    .kill()
                    .map_err(|error| RuntimeError::Pty(error.to_string()))?;
            }
            Err(RecvTimeoutError::Timeout) => {
                flush_pending_output(
                    &mut model,
                    &mut current_frame,
                    &mut journal,
                    &mut journal_dirty,
                    &mut frame_dirty,
                    &mut subscribers,
                )?;
                last_frame_publish = Instant::now();
                if let Some((started, stage)) = termination {
                    match stage {
                        TerminationStage::Hangup if started.elapsed() >= HUP_GRACE => {
                            signal_foreground(&*master, libc::SIGTERM)?;
                            termination = Some((Instant::now(), TerminationStage::Terminate));
                        }
                        TerminationStage::Terminate if started.elapsed() >= TERM_GRACE => {
                            broadcast(
                                &mut subscribers,
                                SessionEvent::TerminationEscalationRequired,
                            );
                            termination = Some((Instant::now(), TerminationStage::AwaitingForce));
                        }
                        TerminationStage::Hangup
                        | TerminationStage::Terminate
                        | TerminationStage::AwaitingForce => {}
                    }
                }
                if observed_exit.is_none() {
                    observed_exit = child
                        .try_wait()
                        .map_err(|error| RuntimeError::Pty(error.to_string()))?;
                }
                if observed_exit.is_some() && last_output.elapsed() >= EXIT_DRAIN_TIMEOUT {
                    let status = observed_exit
                        .take()
                        .expect("observed exit is checked immediately above");
                    broadcast(
                        &mut subscribers,
                        SessionEvent::Exited(SessionExit {
                            code: status.exit_code(),
                            signal: status.signal().map(str::to_owned),
                            success: status.success(),
                        }),
                    );
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = child.kill();
                break;
            }
        }
    }

    journal.flush()?;
    Ok(())
}

fn bundled_terminfo_directory() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("TERMI9NE_TERMINFO_DIR") {
        let configured = PathBuf::from(configured);
        if configured.is_dir() {
            return Some(configured);
        }
    }
    let executable = std::env::current_exe().ok()?;
    bundled_terminfo_for_executable(&executable)
}

fn bundled_terminfo_for_executable(executable: &Path) -> Option<PathBuf> {
    let binary_directory = executable.parent()?;
    let prefix = binary_directory.parent()?;
    [
        prefix.join("Resources/terminfo"),
        prefix.join("share/termi9ne/terminfo"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

#[derive(Clone, Copy)]
enum TerminationStage {
    Hangup,
    Terminate,
    AwaitingForce,
}

fn signal_foreground(
    master: &dyn portable_pty::MasterPty,
    signal: libc::c_int,
) -> Result<(), RuntimeError> {
    let process_group = master
        .process_group_leader()
        .ok_or_else(|| RuntimeError::Pty("PTY has no foreground process group".to_owned()))?;
    // SAFETY: `kill` does not dereference pointers. A negative PID targets the
    // foreground process group reported by the kernel for this PTY.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result == 0 {
        Ok(())
    } else {
        Err(RuntimeError::Io(std::io::Error::last_os_error()))
    }
}

fn read_pty(reader: &mut dyn Read, actor: &SyncSender<ActorMessage>) {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => {
                let _ = actor.send(ActorMessage::PtyReadEnded(None));
                break;
            }
            Ok(read) => {
                if actor
                    .send(ActorMessage::PtyOutput(buffer[..read].to_vec()))
                    .is_err()
                {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = actor.send(ActorMessage::PtyReadEnded(Some(error.to_string())));
                break;
            }
        }
    }
}

fn flush_pending_output(
    model: &mut TerminalModel,
    current_frame: &mut Arc<FullFrame>,
    journal: &mut dyn Write,
    journal_dirty: &mut bool,
    frame_dirty: &mut bool,
    subscribers: &mut Vec<SyncSender<SessionEvent>>,
) -> Result<(), RuntimeError> {
    if *journal_dirty {
        journal.flush()?;
        *journal_dirty = false;
    }
    if *frame_dirty {
        *current_frame = Arc::new(model.frame()?);
        broadcast(subscribers, SessionEvent::Frame(Arc::clone(current_frame)));
        *frame_dirty = false;
    }
    Ok(())
}

fn publish_effects(
    effects: &TerminalEffects,
    writer: &mut dyn Write,
    subscribers: &mut Vec<SyncSender<SessionEvent>>,
) -> Result<(), RuntimeError> {
    for bytes in &effects.pty_writes {
        writer.write_all(bytes)?;
    }
    if !effects.pty_writes.is_empty() {
        writer.flush()?;
    }
    if effects.bells > 0 {
        broadcast(
            subscribers,
            SessionEvent::Bell {
                count: effects.bells,
            },
        );
    }
    for confirmation in &effects.paste_confirmations {
        broadcast(
            subscribers,
            SessionEvent::PasteConfirmation(confirmation.clone()),
        );
    }
    Ok(())
}

fn broadcast(subscribers: &mut Vec<SyncSender<SessionEvent>>, event: SessionEvent) {
    subscribers.retain(|subscriber| match subscriber.try_send(event.clone()) {
        Ok(()) => true,
        // A newer full frame can be diffed directly against the last frame a
        // subscriber actually received, so an intermediate frame is obsolete.
        // Keep the subscription alive and let its consumer catch up.
        Err(TrySendError::Full(SessionEvent::Frame(_))) => true,
        Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
    });
}

fn pty_size(grid: GridSize, cell_width_px: u32, cell_height_px: u32) -> PtySize {
    PtySize {
        rows: grid.rows,
        cols: grid.columns,
        pixel_width: u16::try_from(u32::from(grid.columns).saturating_mul(cell_width_px))
            .unwrap_or(u16::MAX),
        pixel_height: u16::try_from(u32::from(grid.rows).saturating_mul(cell_height_px))
            .unwrap_or(u16::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("termi9ne-runtime-{name}-{}", SessionId::new()))
    }

    fn scripted_session(script: &str, grid: GridSize) -> SessionSpec {
        SessionSpec {
            id: SessionId::new(),
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_owned(), script.to_owned()],
            cwd: std::env::current_dir().expect("test working directory should exist"),
            environment_delta: BTreeMap::new(),
            grid,
        }
    }

    fn frame_text(frame: &FullFrame) -> String {
        frame
            .rows
            .iter()
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn packaged_runtime_finds_macos_and_linux_terminfo_layouts() {
        let root = test_root("terminfo-layout");
        let macos_terminfo = root.join("termi9ne.app/Contents/Resources/terminfo");
        fs::create_dir_all(&macos_terminfo).expect("macOS resource path should be creatable");
        let macos_runtime = root.join("termi9ne.app/Contents/MacOS/termi9ne-server");
        assert_eq!(
            bundled_terminfo_for_executable(&macos_runtime),
            Some(macos_terminfo)
        );

        let linux_terminfo = root.join("usr/share/termi9ne/terminfo");
        fs::create_dir_all(&linux_terminfo).expect("Linux resource path should be creatable");
        let linux_runtime = root.join("usr/bin/termi9ne-server");
        assert_eq!(
            bundled_terminfo_for_executable(&linux_runtime),
            Some(linux_terminfo)
        );
        fs::remove_dir_all(root).expect("isolated package layout should be removable");
    }

    fn wait_for_frame(
        events: &Receiver<SessionEvent>,
        needle: &str,
    ) -> Result<Arc<FullFrame>, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(SessionEvent::Frame(frame)) if frame_text(&frame).contains(needle) => {
                    return Ok(frame);
                }
                Ok(SessionEvent::Failed { message }) => return Err(message),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("session subscription disconnected".to_owned());
                }
            }
        }
        Err(format!("timed out waiting for terminal text {needle:?}"))
    }

    #[test]
    fn shell_output_input_journal_and_reattach_share_one_canonical_frame() {
        let root = test_root("reattach");
        let spec = scripted_session(
            "printf 'ready\\n'; IFS= read -r line; printf 'got:%s\\n' \"$line\"",
            GridSize::new(40, 8).expect("test grid should be valid"),
        );
        let handle = SessionHandle::spawn(spec, &root).expect("session should spawn");
        assert!(handle.process_id().is_some());
        assert!(handle.tty_name().is_some());

        let first_attachment = handle.subscribe().expect("first attach should work");
        wait_for_frame(&first_attachment, "ready").expect("shell should publish its prompt");
        drop(first_attachment);

        handle
            .paste(b"detached\n".to_vec(), true)
            .expect("detached input should reach the PTY");
        let second_attachment = handle.subscribe().expect("reattach should work");
        let frame = wait_for_frame(&second_attachment, "got:detached")
            .expect("reattached projection should receive canonical state");
        assert!(frame.sequence >= 2);

        let journal = fs::read(handle.journal_path()).expect("raw output journal should exist");
        assert!(
            journal
                .windows(b"got:detached".len())
                .any(|window| window == b"got:detached")
        );
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(handle.journal_path())
                .expect("journal metadata should exist")
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(
                handle
                    .journal_path()
                    .parent()
                    .expect("journal should have a session directory")
            )
            .expect("session metadata should exist")
            .mode()
                & 0o777,
            0o700
        );
        fs::remove_dir_all(&root).expect("test state should be removable");
    }

    #[test]
    fn resize_updates_the_kernel_pty_and_canonical_terminal_together() {
        let root = test_root("resize");
        let initial = GridSize::new(24, 8).expect("initial grid should be valid");
        let resized = GridSize::new(40, 12).expect("resized grid should be valid");
        let spec = scripted_session(
            "printf 'before:'; stty size; IFS= read -r line; printf 'after:'; stty size",
            initial,
        );
        let handle = SessionHandle::spawn(spec, &root).expect("session should spawn");
        let events = handle.subscribe().expect("attach should work");
        wait_for_frame(&events, "before:8 24").expect("child should see the initial PTY size");

        handle
            .resize(resized, 8, 16)
            .expect("resize should reach actor");
        handle
            .paste(b"continue\n".to_vec(), true)
            .expect("input should release the child");
        let frame =
            wait_for_frame(&events, "after:12 40").expect("child should observe the resized PTY");
        assert_eq!(frame.grid, resized);

        fs::remove_dir_all(&root).expect("test state should be removable");
    }

    #[test]
    fn interrupt_targets_the_pty_foreground_process_group() {
        let root = test_root("interrupt");
        let spec = scripted_session(
            "trap 'printf interrupted; exit 0' INT; printf ready; while :; do sleep 1; done",
            GridSize::new(40, 8).expect("test grid should be valid"),
        );
        let handle = SessionHandle::spawn(spec, &root).expect("session should spawn");
        let events = handle.subscribe().expect("attach should work");
        wait_for_frame(&events, "ready").expect("child should become ready");

        handle
            .interrupt()
            .expect("interrupt should reach the foreground process group");
        wait_for_frame(&events, "interrupted").expect("shell trap should observe SIGINT");

        fs::remove_dir_all(&root).expect("test state should be removable");
    }

    #[test]
    fn session_reports_the_kernel_foreground_process_group() {
        let root = test_root("foreground-process");
        let spec = scripted_session(
            "printf ready; sleep 1",
            GridSize::new(40, 8).expect("test grid should be valid"),
        );
        let (_handle, events) =
            SessionHandle::spawn_subscribed(spec, &root).expect("session should spawn");
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut observed = None;
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(SessionEvent::ForegroundProcessChanged { process_id }) => {
                    observed = Some(process_id);
                    break;
                }
                Ok(SessionEvent::Failed { message }) => panic!("session failed: {message}"),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(observed.is_some_and(|process_id| process_id > 0));
        fs::remove_dir_all(&root).expect("test state should be removable");
    }

    #[test]
    fn lagging_subscribers_are_evicted_instead_of_growing_memory() {
        let (send, receive) = mpsc::sync_channel(1);
        let mut subscribers = vec![send];
        broadcast(&mut subscribers, SessionEvent::Bell { count: 1 });
        broadcast(&mut subscribers, SessionEvent::Bell { count: 2 });

        assert!(subscribers.is_empty());
        assert!(matches!(
            receive.recv(),
            Ok(SessionEvent::Bell { count: 1 })
        ));
        assert!(matches!(receive.recv(), Err(std::sync::mpsc::RecvError)));
    }

    #[test]
    fn lagging_frame_subscribers_drop_obsolete_frames_without_detaching() {
        let mut model = TerminalModel::new(GridSize::new(20, 4).expect("valid test grid"))
            .expect("test terminal should initialize");
        let first = Arc::new(model.frame().expect("initial frame should render"));
        model
            .advance(TerminalAction::Output(b"latest"))
            .expect("test output should parse");
        let latest = Arc::new(model.frame().expect("latest frame should render"));
        let (send, receive) = mpsc::sync_channel(1);
        let mut subscribers = vec![send];

        broadcast(&mut subscribers, SessionEvent::Frame(Arc::clone(&first)));
        broadcast(&mut subscribers, SessionEvent::Frame(Arc::clone(&latest)));

        assert_eq!(subscribers.len(), 1);
        assert!(matches!(
            receive.recv(),
            Ok(SessionEvent::Frame(frame)) if frame.sequence == first.sequence
        ));
        broadcast(&mut subscribers, SessionEvent::Frame(Arc::clone(&latest)));
        assert!(matches!(
            receive.recv(),
            Ok(SessionEvent::Frame(frame)) if frame.sequence == latest.sequence
        ));
    }

    #[test]
    fn paced_output_is_coalesced_to_display_rate_without_losing_final_state() {
        let root = test_root("frame-coalescing");
        let spec = scripted_session(
            concat!(
                "i=0; while [ \"$i\" -lt 40 ]; do ",
                "printf 'tick%02d\\r\\n' \"$i\"; ",
                "i=$((i+1)); sleep 0.002; done"
            ),
            GridSize::new(40, 8).expect("test grid should be valid"),
        );
        let (_handle, events) =
            SessionHandle::spawn_subscribed(spec, &root).expect("session should spawn");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut frames = 0_usize;
        let mut final_frame = None;

        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(SessionEvent::Frame(frame)) => {
                    frames += 1;
                    final_frame = Some(frame);
                }
                Ok(SessionEvent::Exited(exit)) => {
                    assert!(exit.success, "paced fixture should exit successfully");
                    break;
                }
                Ok(SessionEvent::Failed { message }) => panic!("paced fixture failed: {message}"),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        let final_frame = final_frame.expect("paced output should publish at least one frame");
        assert!(
            frame_text(&final_frame).contains("tick39"),
            "the last coalesced frame must retain the final PTY output"
        );
        assert!(
            frames <= 30,
            "40 paced writes must be coalesced instead of producing one frame each; got {frames} frames"
        );
        fs::remove_dir_all(&root).expect("test state should be removable");
    }
}
