//! Versioned local control protocol shared by humans, agents, and UI clients.

mod terminal_codec;
mod terminal_proto;
pub mod wire_v3;

pub use terminal_codec::{decode as decode_terminal_event, encode as encode_terminal_event};

use std::{
    collections::BTreeMap,
    fmt,
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use termi9ne_core::{
    Actor, Command, Event, FaultId, Mission, MissionId, RunId, SchedulerPlan, SessionId,
};
use termi9ne_terminal::{
    CellStyle, Cursor, FullFrame, GridSize, HistoryViewport, KeyInput, MouseInput,
    PasteConfirmation, Rgb, Row, SearchMatch, SelectionPoint, ViewportScroll,
};
use thiserror::Error;
use tokio::io::AsyncWrite;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 25;

/// Stable identity for an owner-arranged collection of terminal Sessions.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SessionGroupId(Uuid);

impl SessionGroupId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
}

impl Default for SessionGroupId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionGroupId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::str::FromStr for SessionGroupId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value).map(Self)
    }
}

/// Complete authoritative replacement for one sidebar Session group.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionGroupSummary {
    pub group_id: SessionGroupId,
    #[serde(default)]
    pub mission_id: Option<MissionId>,
    pub name: String,
    pub session_ids: Vec<SessionId>,
    pub position: u32,
    pub pinned: bool,
    #[serde(default)]
    pub detached: bool,
    pub version: u64,
}

/// Validated creation input for one Session group.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionGroupSpec {
    pub group_id: SessionGroupId,
    #[serde(default)]
    pub mission_id: Option<MissionId>,
    pub name: String,
    pub session_ids: Vec<SessionId>,
    pub position: u32,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub detached: bool,
}

/// One concurrency-checked mutation of a Session group.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionGroupChange {
    Rename { name: String },
    SetPosition { position: u32 },
    SetPinned { pinned: bool },
    SetDetached { detached: bool },
    AddSession { session_id: SessionId },
    RemoveSession { session_id: SessionId },
}

#[must_use]
pub fn default_socket_path() -> PathBuf {
    PathBuf::from(".termi9ne/control.sock")
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ClientRequest {
    pub version: u16,
    #[serde(default = "Uuid::new_v4")]
    pub client_id: Uuid,
    pub request_id: Uuid,
    #[serde(default)]
    pub surface_id: Option<Uuid>,
    #[serde(default)]
    pub control_epoch: Option<u64>,
    #[serde(default)]
    pub expected_mission_version: Option<u64>,
    /// Optional observer capability. Owners leave this absent. The token is
    /// never returned by diagnostics, listings, or persisted share records.
    #[serde(default)]
    pub share_token: Option<String>,
    pub action: Request,
}

impl ClientRequest {
    #[must_use]
    pub fn new(action: Request) -> Self {
        Self::for_client(Uuid::new_v4(), action)
    }

    #[must_use]
    pub fn for_client(client_id: Uuid, action: Request) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            client_id,
            request_id: Uuid::new_v4(),
            surface_id: None,
            control_epoch: None,
            expected_mission_version: None,
            share_token: None,
            action,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    RuntimeDiagnostics,
    ListPlugins,
    CreateMission {
        mission_id: MissionId,
        intent: String,
        created_by: Actor,
    },
    Dispatch {
        mission_id: MissionId,
        command: Command,
    },
    GetMission {
        mission_id: MissionId,
    },
    MissionHistory {
        mission_id: MissionId,
        #[serde(default)]
        before_sequence: Option<u64>,
        limit: u16,
    },
    SchedulerPlan {
        mission_id: MissionId,
        max_concurrency: u16,
    },
    /// Resolve and launch the currently startable agent Runs through their
    /// owner-configured engine profiles under one deterministic slot plan.
    LaunchConfiguredSchedulerBatch {
        mission_id: MissionId,
        max_concurrency: u16,
        session_name_prefix: String,
        cwd: PathBuf,
        grid: GridSize,
    },
    /// Persist an owner-controlled, opt-in continuous scheduling policy.
    SetSchedulerPolicy {
        policy: SchedulerPolicy,
    },
    ListSchedulerPolicies,
    SetSchedulerSettings {
        settings: SchedulerSettings,
    },
    GetSchedulerSettings,
    /// Prepare one isolated, durable Git worktree for a pending Run.
    PrepareRunCheckout {
        mission_id: MissionId,
        run_id: RunId,
        repository: PathBuf,
        base_ref: String,
    },
    /// List owner-visible managed Run checkouts.
    ListRunCheckouts {
        #[serde(default)]
        mission_id: Option<MissionId>,
    },
    /// Retire a clean checkout only after its HEAD is reachable from this ref.
    RetireRunCheckout {
        mission_id: MissionId,
        run_id: RunId,
        merged_into_ref: String,
    },
    /// Persist one expiring adapter observation without mutating Mission truth.
    ReportProviderFact {
        mission_id: MissionId,
        run_id: RunId,
        fact: ProviderFactInput,
    },
    /// Derive the current explainable activity for one Run.
    GetRunActivity {
        mission_id: MissionId,
        run_id: RunId,
    },
    /// Derive activity for every Run in one Mission.
    ListRunActivities {
        mission_id: MissionId,
    },
    /// Persist one provider-neutral CI or change-review observation.
    ReportRunEvidence {
        mission_id: MissionId,
        run_id: RunId,
        evidence: RunEvidenceInput,
    },
    /// List durable external evidence attached to one Run.
    ListRunEvidence {
        mission_id: MissionId,
        run_id: RunId,
    },
    /// Record one observed failure with the evidence needed to replay it.
    ReportFault {
        fault: FaultInput,
    },
    /// List Faults, newest first.
    ListFaults {
        #[serde(default)]
        mission_id: Option<MissionId>,
        #[serde(default)]
        session_id: Option<SessionId>,
        /// Include Faults that are already resolved or dismissed.
        #[serde(default)]
        include_closed: bool,
    },
    /// Read one Fault by identity.
    GetFault {
        fault_id: FaultId,
    },
    /// Replay a Fault's exact command in its recorded directory and attach the
    /// resulting receipt. This executes the recorded command.
    ReproduceFault {
        fault_id: FaultId,
        /// Abort the replay after this many seconds.
        #[serde(default)]
        timeout_seconds: Option<u16>,
    },
    /// Close a Fault. Refused unless its latest replay passed.
    ResolveFault {
        fault_id: FaultId,
        note: String,
    },
    /// Close a Fault without repro evidence, recording why.
    DismissFault {
        fault_id: FaultId,
        note: String,
    },
    /// Re-run the replay of Faults that were resolved, and reopen any that
    /// fail again. This executes their recorded commands.
    ///
    /// A Fault that closes only on a passing replay is worth little if it can
    /// come back unnoticed; this is what makes resolution stay true.
    GuardFaults {
        /// Most Faults to check in one pass, oldest proof first.
        #[serde(default)]
        limit: Option<u16>,
        /// Abort each replay after this many seconds.
        #[serde(default)]
        timeout_seconds: Option<u16>,
    },
    /// Record which Run has been asked to fix a Fault.
    AssignFaultFix {
        fault_id: FaultId,
        mission_id: MissionId,
        run_id: RunId,
    },
    /// Mint a time-bounded, scoped capability. The plaintext secret is returned
    /// once and only its digest is persisted.
    CreateShare {
        label: String,
        role: ShareRole,
        mission_ids: Vec<MissionId>,
        session_ids: Vec<SessionId>,
        expires_in_seconds: u64,
    },
    ListShares,
    ShareIdentity,
    RevokeShare {
        share_id: Uuid,
    },
    /// Atomically bind a ready planned Run to a durable PTY launch.
    LaunchAgentRun {
        mission_id: MissionId,
        run_id: termi9ne_core::RunId,
        session_name: String,
        spec: TerminalSessionSpec,
    },
    /// Resolve the Run actor's named engine through owner-controlled runtime config.
    LaunchConfiguredAgentRun {
        mission_id: MissionId,
        run_id: termi9ne_core::RunId,
        session_id: SessionId,
        session_name: String,
        cwd: PathBuf,
        #[serde(default)]
        use_run_checkout: bool,
        grid: GridSize,
    },
    /// Resolve a named driver without mutating graph or process state.
    PreviewConfiguredAgentRun {
        mission_id: MissionId,
        run_id: termi9ne_core::RunId,
        cwd: PathBuf,
        #[serde(default)]
        use_run_checkout: bool,
        grid: GridSize,
    },
    ListMissions,
    SubscribeMissions,
    /// Subscribe to explainable Run Activity snapshots and replacements.
    SubscribeRunActivities {
        #[serde(default)]
        mission_id: Option<MissionId>,
    },
    StartTerminal {
        spec: TerminalSessionSpec,
    },
    ListTerminals {
        #[serde(default)]
        include_archived: bool,
    },
    SubscribeTerminals,
    CreateSessionGroup {
        group: SessionGroupSpec,
    },
    ListSessionGroups {
        #[serde(default)]
        mission_id: Option<MissionId>,
    },
    UpdateSessionGroup {
        group_id: SessionGroupId,
        expected_version: u64,
        change: SessionGroupChange,
    },
    DeleteSessionGroup {
        group_id: SessionGroupId,
        expected_version: u64,
    },
    SubscribeSessionGroups {
        #[serde(default)]
        mission_id: Option<MissionId>,
    },
    /// Idempotently stop one connection-scoped subscription stream.
    Unsubscribe {
        stream_id: u32,
    },
    TerminalSnapshot {
        session_id: SessionId,
    },
    /// Atomically capture terminal identity and the exact canonical frame.
    TerminalCapture {
        session_id: SessionId,
    },
    /// Block on canonical terminal state with a mandatory bounded timeout.
    TerminalWait {
        session_id: SessionId,
        condition: TerminalWaitCondition,
        timeout_millis: u64,
    },
    /// Fetch a deterministic viewport from retained scrollback. Zero is the
    /// bottom viewport; increasing offsets move toward older output.
    TerminalHistoryFrame {
        session_id: SessionId,
        viewport: HistoryViewport,
    },
    TerminalKey {
        session_id: SessionId,
        input: KeyInput,
    },
    TerminalPaste {
        session_id: SessionId,
        bytes: Vec<u8>,
        confirmed: bool,
    },
    TerminalFocus {
        session_id: SessionId,
        focused: bool,
    },
    TerminalMouse {
        session_id: SessionId,
        input: MouseInput,
    },
    TerminalScroll {
        session_id: SessionId,
        scroll: ViewportScroll,
    },
    TerminalSelect {
        session_id: SessionId,
        anchor: SelectionPoint,
        head: SelectionPoint,
        rectangle: bool,
    },
    TerminalClearSelection {
        session_id: SessionId,
    },
    TerminalSelectionText {
        session_id: SessionId,
    },
    TerminalSearch {
        session_id: SessionId,
        query: String,
        case_sensitive: bool,
        limit: usize,
    },
    TerminalResize {
        session_id: SessionId,
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    KillTerminal {
        session_id: SessionId,
    },
    InterruptTerminal {
        session_id: SessionId,
    },
    TerminateTerminal {
        session_id: SessionId,
    },
    ArchiveTerminal {
        session_id: SessionId,
    },
    RestoreTerminal {
        session_id: SessionId,
    },
    ClaimTerminalControl {
        session_id: SessionId,
        force: bool,
    },
    ReleaseTerminalControl {
        session_id: SessionId,
    },
    SubscribeTerminal {
        session_id: SessionId,
    },
}

/// Serializable process configuration accepted by the daemon boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalSessionSpec {
    pub session_id: SessionId,
    #[serde(default)]
    pub mission_id: Option<MissionId>,
    #[serde(default)]
    pub run_id: Option<termi9ne_core::RunId>,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment_delta: BTreeMap<String, Option<String>>,
    pub grid: GridSize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalSessionStatus {
    Running,
    Exited,
    Failed,
}

/// Bounded, non-secret identity of the PTY foreground process. Arguments are
/// deliberately excluded because prompts and credentials may appear there.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalForegroundProcess {
    pub process_id: u32,
    pub executable: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalSessionSummary {
    pub session_id: SessionId,
    #[serde(default)]
    pub mission_id: Option<MissionId>,
    #[serde(default)]
    pub run_id: Option<termi9ne_core::RunId>,
    pub process_id: Option<u32>,
    #[serde(default)]
    pub foreground_process: Option<TerminalForegroundProcess>,
    pub tty_name: Option<PathBuf>,
    pub status: TerminalSessionStatus,
    #[serde(default)]
    pub archived: bool,
    pub latest_sequence: u64,
    #[serde(default)]
    pub controller_client_id: Option<Uuid>,
    #[serde(default)]
    pub controller_surface_id: Option<Uuid>,
    #[serde(default)]
    pub controller_share_id: Option<Uuid>,
    #[serde(default)]
    pub control_epoch: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalWaitCondition {
    Text {
        query: String,
        #[serde(default)]
        case_sensitive: bool,
    },
    Quiet {
        quiet_millis: u64,
    },
    Exit,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TerminalCapture {
    pub terminal: TerminalSessionSummary,
    pub frame: Box<FullFrame>,
    pub captured_at_unix_micros: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConfiguredAgentLaunchPreview {
    pub mission_id: MissionId,
    pub run_id: termi9ne_core::RunId,
    pub engine: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment_keys: Vec<String>,
    pub grid: GridSize,
    #[serde(default)]
    pub sandbox_backend: Option<String>,
    #[serde(default)]
    pub sandbox_profile: Option<String>,
    #[serde(default)]
    pub sandbox_network_isolated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledAgentLaunch {
    pub run_id: RunId,
    pub events_committed: u16,
    pub mission_version: u64,
    pub terminal: TerminalSessionSummary,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledAgentLaunchFailure {
    pub run_id: RunId,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchedulerPolicy {
    pub mission_id: MissionId,
    pub enabled: bool,
    pub max_concurrency: u16,
    pub session_name_prefix: String,
    pub cwd: PathBuf,
    pub grid: GridSize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchedulerSettings {
    pub global_max_concurrency: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunCheckoutState {
    Provisioning,
    Ready,
    Retiring,
    Retired,
    Failed,
}

/// Owner-visible identity and lifecycle for one managed Git worktree.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunCheckoutSummary {
    pub checkout_id: Uuid,
    pub mission_id: MissionId,
    pub run_id: RunId,
    pub repository_root: PathBuf,
    pub worktree_path: PathBuf,
    pub base_revision: String,
    pub branch: String,
    pub state: RunCheckoutState,
    pub created_at_unix_micros: u64,
    #[serde(default)]
    pub retired_at_unix_micros: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderActivityState {
    Working,
    Idle,
    WaitingInput,
    WaitingApproval,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderFactInput {
    pub provider_id: String,
    pub adapter_version: String,
    pub state: ProviderActivityState,
    pub summary: String,
    pub valid_for_seconds: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFactSource {
    AuthenticatedAgent,
    OwnerHook,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderFactSummary {
    pub mission_id: MissionId,
    pub run_id: RunId,
    pub provider_id: String,
    pub adapter_version: String,
    pub state: ProviderActivityState,
    pub summary: String,
    pub source: ProviderFactSource,
    pub observed_at_unix_micros: u64,
    pub expires_at_unix_micros: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunActivityState {
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunActivitySource {
    RunLifecycle,
    Signal,
    AuthenticatedAgent,
    OwnerHook,
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunActivitySummary {
    pub mission_id: MissionId,
    pub run_id: RunId,
    pub state: RunActivityState,
    pub source: RunActivitySource,
    pub explanation: String,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub adapter_version: Option<String>,
    #[serde(default)]
    pub observed_at_unix_micros: Option<u64>,
    #[serde(default)]
    pub expires_at_unix_micros: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckEvidenceState {
    Queued,
    Running,
    Passed,
    Failed,
    Cancelled,
    Skipped,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeRequestEvidenceState {
    Draft,
    Open,
    Approved,
    ChangesRequested,
    Merged,
    Closed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvidenceKind {
    Check {
        name: String,
        state: CheckEvidenceState,
    },
    ChangeRequest {
        title: String,
        state: ChangeRequestEvidenceState,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunEvidenceInput {
    pub provider_id: String,
    pub adapter_version: String,
    pub evidence_key: String,
    pub revision: String,
    pub summary: String,
    #[serde(default)]
    pub url: Option<String>,
    pub kind: RunEvidenceKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEvidenceSource {
    AuthenticatedAgent,
    OwnerHook,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunEvidenceSummary {
    pub mission_id: MissionId,
    pub run_id: RunId,
    pub provider_id: String,
    pub adapter_version: String,
    pub evidence_key: String,
    pub revision: String,
    pub summary: String,
    #[serde(default)]
    pub url: Option<String>,
    pub kind: RunEvidenceKind,
    pub source: RunEvidenceSource,
    pub observed_at_unix_micros: u64,
}

/// What kind of failure a Fault records. Detectors classify from exit status
/// and recognised runner output; no model interprets these.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultKind {
    /// A command exited non-zero without a more specific classification.
    CommandFailed,
    TestFailed,
    BuildFailed,
    Panic,
    /// The process died on a signal.
    Crashed,
}

/// Who observed the failure. A Fault never claims more authority than its
/// observer had.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultSource {
    /// The runtime saw a daemon-owned terminal end non-zero.
    TerminalExit,
    /// The owner, a shell hook, or CI reported it over the control socket.
    OwnerHook,
    /// An authenticated agent Run reported it over the agent channel.
    AuthenticatedAgent,
}

/// The failure as first observed. Everything here is evidence, not narrative.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FaultInput {
    pub kind: FaultKind,
    /// The exact command line that failed, as it should be replayed.
    pub command: String,
    /// Directory the command ran in; a repro runs in this same directory.
    pub cwd: PathBuf,
    #[serde(default)]
    pub exit_code: Option<i32>,
    /// Repository revision at observation time, when one is known.
    #[serde(default)]
    pub revision: Option<String>,
    /// One line naming the failure.
    pub summary: String,
    /// Bounded slice of the failing output. Never the whole scrollback.
    pub output: String,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub mission_id: Option<MissionId>,
    #[serde(default)]
    pub run_id: Option<RunId>,
}

/// The result of replaying a Fault's command. `reproduced` is the whole point:
/// it is measured, never asserted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReproReceipt {
    pub attempted_at_unix_micros: u64,
    /// True when the replay failed the same way, so the Fault is still real.
    pub reproduced: bool,
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub output: String,
    pub duration_ms: u64,
    /// Revision the replay ran at, when one is known.
    #[serde(default)]
    pub revision: Option<String>,
    /// Set when the replay could not run at all, rather than running and passing.
    #[serde(default)]
    pub error: Option<String>,
}

/// Lifecycle of a Fault. A Fault leaves `Open` only through evidence
/// (`Resolved`, which requires a passing repro) or an explicit owner
/// `Dismissed` note.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FaultState {
    Open,
    Resolved { note: String, at_unix_micros: u64 },
    Dismissed { note: String, at_unix_micros: u64 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FaultSummary {
    pub fault_id: FaultId,
    pub kind: FaultKind,
    pub command: String,
    pub cwd: PathBuf,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub revision: Option<String>,
    pub summary: String,
    pub output: String,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub mission_id: Option<MissionId>,
    #[serde(default)]
    pub run_id: Option<RunId>,
    pub source: FaultSource,
    pub observed_at_unix_micros: u64,
    pub state: FaultState,
    /// Most recent replay attempt, when one has run.
    #[serde(default)]
    pub repro: Option<ReproReceipt>,
    pub repro_attempts: u32,
    /// Run currently assigned to fix this Fault, when one has been launched.
    #[serde(default)]
    pub fix_run_id: Option<RunId>,
    /// The replay that proved this Fault fixed, kept when it was resolved.
    ///
    /// `repro` holds the most recent attempt and is overwritten by the
    /// regression guard, so without this the evidence that closed the Fault
    /// would be lost exactly when a regression makes it interesting.
    #[serde(default)]
    pub proof: Option<ReproReceipt>,
    /// Times this Fault was resolved and later failed again.
    ///
    /// A failure that keeps coming back is a different problem from one that
    /// happened once, and only a count distinguishes them.
    #[serde(default)]
    pub regressions: u32,
}

impl FaultSummary {
    /// True while this Fault still needs attention.
    #[must_use]
    pub fn is_open(&self) -> bool {
        matches!(self.state, FaultState::Open)
    }

    /// True when the latest replay ran and did not fail, which is the only
    /// evidence that permits resolution.
    #[must_use]
    pub fn repro_passes(&self) -> bool {
        self.repro
            .as_ref()
            .is_some_and(|receipt| !receipt.reproduced && receipt.error.is_none())
    }

    /// The objective handed to an actor asked to fix this Fault.
    ///
    /// Deliberately states the evidence and the acceptance test, and nothing
    /// about how to fix it: the replay decides whether the work is done.
    #[must_use]
    pub fn fix_objective(&self) -> String {
        use std::fmt::Write as _;
        let mut objective = String::new();
        let _ = writeln!(objective, "Fix this failure: {}", self.summary);
        let _ = writeln!(objective, "command: {}", self.command);
        let _ = writeln!(objective, "cwd: {}", self.cwd.display());
        if let Some(exit_code) = self.exit_code {
            let _ = writeln!(objective, "exit: {exit_code}");
        }
        if let Some(revision) = &self.revision {
            let _ = writeln!(objective, "revision: {revision}");
        }
        let output = self.output.trim();
        if !output.is_empty() {
            let _ = writeln!(objective, "\nfailing output:\n{output}");
        }
        let _ = write!(
            objective,
            "\nDone means `{}` succeeds in that directory. It is verified by replay, not by claiming it.",
            self.command
        );
        objective
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareRole {
    Observer,
    Controller,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShareSummary {
    pub share_id: Uuid,
    pub label: String,
    pub role: ShareRole,
    pub mission_ids: Vec<MissionId>,
    pub session_ids: Vec<SessionId>,
    pub created_at_micros: u64,
    pub expires_at_micros: u64,
    #[serde(default)]
    pub revoked_at_micros: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ServerResponse {
    pub version: u16,
    pub request_id: Uuid,
    pub result: ResponseResult,
}

impl ServerResponse {
    #[must_use]
    pub fn success(request_id: Uuid, body: ResponseBody) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            result: ResponseResult::Success {
                body: Box::new(body),
            },
        }
    }

    #[must_use]
    pub fn error(request_id: Uuid, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id,
            result: ResponseResult::Error {
                code: code.into(),
                message: message.into(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseResult {
    Success { body: Box<ResponseBody> },
    Error { code: String, message: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseBody {
    Pong,
    RuntimeDiagnostics {
        diagnostics: RuntimeDiagnostics,
    },
    Plugins {
        plugins: Vec<PluginRuntimeSummary>,
        dropped_events: u64,
    },
    Mission {
        mission: Mission,
    },
    Missions {
        missions: Vec<MissionSummary>,
    },
    MissionHistory {
        mission_id: MissionId,
        entries: Vec<MissionHistoryEntry>,
        has_more: bool,
    },
    SchedulerPlan {
        mission_id: MissionId,
        plan: SchedulerPlan,
    },
    ConfiguredSchedulerBatchLaunched {
        mission_id: MissionId,
        plan: SchedulerPlan,
        launched: Vec<ScheduledAgentLaunch>,
        failures: Vec<ScheduledAgentLaunchFailure>,
    },
    SchedulerPolicy {
        policy: SchedulerPolicy,
    },
    SchedulerPolicies {
        policies: Vec<SchedulerPolicy>,
    },
    SchedulerSettings {
        settings: SchedulerSettings,
    },
    RunCheckout {
        checkout: RunCheckoutSummary,
    },
    RunCheckouts {
        checkouts: Vec<RunCheckoutSummary>,
    },
    ProviderFactRecorded {
        fact: ProviderFactSummary,
        activity: RunActivitySummary,
    },
    RunActivity {
        activity: RunActivitySummary,
    },
    RunActivities {
        activities: Vec<RunActivitySummary>,
    },
    RunEvidenceRecorded {
        evidence: RunEvidenceSummary,
    },
    RunEvidence {
        evidence: Vec<RunEvidenceSummary>,
    },
    FaultRecorded {
        fault: FaultSummary,
    },
    Faults {
        faults: Vec<FaultSummary>,
    },
    /// Outcome of one regression-guard pass.
    FaultsGuarded {
        /// Every Fault whose replay was re-run.
        checked: Vec<FaultId>,
        /// Those that failed again and are open once more.
        reopened: Vec<FaultSummary>,
    },
    ShareCreated {
        share: ShareSummary,
        token: String,
    },
    Shares {
        shares: Vec<ShareSummary>,
    },
    ShareRevoked {
        share: ShareSummary,
    },
    ShareIdentity {
        share: ShareSummary,
    },
    AgentRunLaunched {
        events: Vec<Event>,
        mission: Mission,
        terminal: TerminalSessionSummary,
    },
    ConfiguredAgentLaunchPreview {
        preview: ConfiguredAgentLaunchPreview,
    },
    EventsCommitted {
        events: Vec<Event>,
        mission: Mission,
    },
    TerminalStarted {
        terminal: TerminalSessionSummary,
    },
    Terminals {
        terminals: Vec<TerminalSessionSummary>,
    },
    SessionGroup {
        group: SessionGroupSummary,
    },
    SessionGroups {
        groups: Vec<SessionGroupSummary>,
    },
    SessionGroupDeleted {
        group_id: SessionGroupId,
    },
    TerminalFrame {
        session_id: SessionId,
        frame: Box<FullFrame>,
    },
    TerminalCaptured {
        capture: TerminalCapture,
    },
    TerminalWaitSatisfied {
        condition: TerminalWaitCondition,
        elapsed_millis: u64,
        capture: TerminalCapture,
    },
    TerminalHistoryFrame {
        session_id: SessionId,
        viewport: HistoryViewport,
        frame: Box<FullFrame>,
    },
    TerminalCommandAccepted {
        session_id: SessionId,
    },
    TerminalControlChanged {
        session_id: SessionId,
        control_epoch: u64,
    },
    TerminalSubscriptionAccepted {
        session_id: SessionId,
        stream_id: u32,
    },
    TerminalIndexSubscriptionAccepted {
        stream_id: u32,
    },
    SessionGroupSubscriptionAccepted {
        stream_id: u32,
    },
    MissionSubscriptionAccepted {
        stream_id: u32,
    },
    RunActivitySubscriptionAccepted {
        stream_id: u32,
    },
    SubscriptionEnded {
        stream_id: u32,
    },
    TerminalSelectionText {
        session_id: SessionId,
        text: Option<String>,
    },
    TerminalSearchResults {
        session_id: SessionId,
        matches: Vec<SearchMatch>,
    },
}

/// Redacted, bounded health information for the local runtime.
///
/// Counts describe the instant at which the response was assembled. Paths,
/// environment values, terminal contents, and process arguments are excluded.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeDiagnostics {
    pub runtime_version: String,
    pub protocol_version: u16,
    pub wire_profile: String,
    pub process_id: u32,
    pub platform: String,
    pub architecture: String,
    pub uptime_seconds: u64,
    pub open_connections: usize,
    #[serde(default)]
    pub agent_connections: usize,
    pub mission_subscribers: usize,
    pub terminal_index_subscribers: usize,
    #[serde(default)]
    pub terminal_waits_active: usize,
    pub missions: usize,
    #[serde(default)]
    pub scheduler_policies: usize,
    #[serde(default)]
    pub scheduler_policies_enabled: usize,
    #[serde(default)]
    pub scheduler_global_limit: u16,
    #[serde(default)]
    pub scheduler_global_occupied: u16,
    #[serde(default)]
    pub run_checkouts_total: usize,
    #[serde(default)]
    pub run_checkouts_ready: usize,
    #[serde(default)]
    pub run_checkouts_failed: usize,
    pub terminals_total: usize,
    pub terminals_running: usize,
    pub terminals_exited: usize,
    pub terminals_failed: usize,
    pub terminals_controlled: usize,
    pub terminals_archived: usize,
    #[serde(default)]
    pub plugins_discovered: usize,
    #[serde(default)]
    pub plugins_running: usize,
    #[serde(default)]
    pub plugins_degraded: usize,
    #[serde(default)]
    pub plugin_events_dropped: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginRuntimeStateSummary {
    Starting,
    Running,
    Backoff,
    Failed,
    Stopped,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginStatusLevelSummary {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginStatusSummary {
    pub key: String,
    pub level: PluginStatusLevelSummary,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PluginRuntimeSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: PluginRuntimeStateSummary,
    pub restart_count: u32,
    pub dropped_events: u64,
    #[serde(default)]
    pub last_status: Option<PluginStatusSummary>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MissionEvent {
    MissionChanged { mission: Mission },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunActivityEvent {
    ActivityChanged { activity: RunActivitySummary },
}

/// One durable Mission event with its storage and causation metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MissionHistoryEntry {
    pub mission_sequence: u64,
    pub event_id: Option<Uuid>,
    pub occurred_at_unix_micros: Option<u64>,
    pub correlation_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
    pub payload_type: String,
    pub payload: Event,
}

/// Index updates following a successful `SubscribeTerminals` response.
///
/// Consumers receive a race-free snapshot as individual `TerminalChanged`
/// events before live changes continue on the same stream. Repeated summaries
/// are intentional and should replace the consumer's previous value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalIndexEvent {
    TerminalChanged { terminal: TerminalSessionSummary },
}

/// Race-free group snapshot followed by complete live replacements/removals.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionGroupEvent {
    GroupChanged { group: SessionGroupSummary },
    GroupDeleted { group_id: SessionGroupId },
}

/// Events following a successful `SubscribeTerminal` response on the same stream.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    TerminalFrame {
        session_id: SessionId,
        frame: Box<FullFrame>,
    },
    TerminalDelta {
        session_id: SessionId,
        delta: Box<FrameDelta>,
    },
    TerminalBell {
        session_id: SessionId,
        count: u64,
    },
    PasteConfirmation {
        session_id: SessionId,
        confirmation: PasteConfirmation,
    },
    TerminalTerminationEscalationRequired {
        session_id: SessionId,
    },
    TerminalExited {
        session_id: SessionId,
        code: u32,
        signal: Option<String>,
        success: bool,
    },
    TerminalFailed {
        session_id: SessionId,
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangedRow {
    pub index: u16,
    pub row: Row,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FrameDelta {
    pub base_sequence: u64,
    pub sequence: u64,
    pub grid: GridSize,
    pub changed_rows: Vec<ChangedRow>,
    pub styles: Vec<CellStyle>,
    pub cursor: Option<Cursor>,
    pub default_foreground: Rgb,
    pub default_background: Rgb,
    pub mouse_tracking: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub current_directory: Option<String>,
}

impl FrameDelta {
    #[must_use]
    pub fn between(base: &FullFrame, next: &FullFrame) -> Option<Self> {
        if base.grid != next.grid
            || base.rows.len() != next.rows.len()
            || next.sequence <= base.sequence
        {
            return None;
        }
        let changed_rows = base
            .rows
            .iter()
            .zip(&next.rows)
            .enumerate()
            .filter(|(_, (old, new))| !Arc::ptr_eq(old, new) && old != new)
            .map(|(index, (_, new))| ChangedRow {
                index: u16::try_from(index).expect("terminal row limits fit in u16"),
                row: (**new).clone(),
            })
            .collect();
        Some(Self {
            base_sequence: base.sequence,
            sequence: next.sequence,
            grid: next.grid,
            changed_rows,
            styles: next.styles.clone(),
            cursor: next.cursor,
            default_foreground: next.default_foreground,
            default_background: next.default_background,
            mouse_tracking: next.mouse_tracking,
            title: next.title.clone(),
            current_directory: next.current_directory.clone(),
        })
    }

    pub fn apply_to(&self, frame: &mut FullFrame) -> Result<(), ProtocolError> {
        if frame.sequence != self.base_sequence || frame.grid != self.grid {
            return Err(ProtocolError::DeltaBaseMismatch {
                expected: frame.sequence,
                actual: self.base_sequence,
            });
        }
        if self.sequence <= self.base_sequence {
            return Err(ProtocolError::InvalidDeltaSequence(self.sequence));
        }
        for changed in &self.changed_rows {
            let row = frame
                .rows
                .get_mut(usize::from(changed.index))
                .ok_or(ProtocolError::InvalidDeltaRow(changed.index))?;
            *row = Arc::new(changed.row.clone());
        }
        frame.sequence = self.sequence;
        frame.styles.clone_from(&self.styles);
        frame.cursor = self.cursor;
        frame.default_foreground = self.default_foreground;
        frame.default_background = self.default_background;
        frame.mouse_tracking = self.mouse_tracking;
        frame.title.clone_from(&self.title);
        frame.current_directory.clone_from(&self.current_directory);
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MissionSummary {
    pub id: MissionId,
    pub intent: String,
    pub status: termi9ne_core::MissionStatus,
    pub run_count: usize,
    pub session_count: usize,
    pub attention_count: usize,
    pub version: u64,
}

impl From<&Mission> for MissionSummary {
    fn from(mission: &Mission) -> Self {
        Self {
            id: mission.id,
            intent: mission.intent.clone(),
            status: mission.status,
            run_count: mission.runs.len(),
            session_count: mission.sessions.len(),
            attention_count: mission.attention_queue().len(),
            version: mission.version,
        }
    }
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("protocol I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid protocol JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("terminal Protocol Buffers decoding failed: {0}")]
    ProtobufDecode(#[from] prost::DecodeError),
    #[error("invalid terminal Protocol Buffers payload: {0}")]
    InvalidTerminalProtobuf(String),
    #[error(transparent)]
    Wire(#[from] wire_v3::WireError),
    #[error("terminal delta base {actual} does not match projection sequence {expected}")]
    DeltaBaseMismatch { expected: u64, actual: u64 },
    #[error("terminal delta sequence {0} is not newer than its base")]
    InvalidDeltaSequence(u64),
    #[error("terminal delta row {0} is outside the projection")]
    InvalidDeltaRow(u16),
}

/// Receive one protocol-v3 terminal event on subscription stream one.
pub fn read_server_event_v3_sync<S>(
    wire: &mut wire_v3::SyncWire<S>,
) -> Result<ServerEvent, ProtocolError>
where
    S: Read + Write,
{
    let frame = wire.receive()?;
    if frame.header.stream_id != 1 {
        return Err(wire_v3::WireError::UnexpectedFrame {
            expected: wire_v3::FrameKind::EventBatch,
            expected_stream: 1,
            actual: frame.header.kind,
            actual_stream: frame.header.stream_id,
        }
        .into());
    }
    decode_terminal_event(frame.header.kind, &frame.payload)
}

/// Send one protocol-v3 protobuf/JSON terminal event on stream one.
pub async fn write_server_event_v3<W>(
    writer: &mut wire_v3::AsyncWireWriter<W>,
    value: &ServerEvent,
) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    write_server_event_v3_on_stream(writer, 1, value).await
}

/// Send one protocol-v3 protobuf/JSON terminal event on an assigned stream.
pub async fn write_server_event_v3_on_stream<W>(
    writer: &mut wire_v3::AsyncWireWriter<W>,
    stream_id: u32,
    value: &ServerEvent,
) -> Result<(), ProtocolError>
where
    W: AsyncWrite + Unpin,
{
    let (kind, bytes) = encode_terminal_event(value)?;
    writer.send_bytes(kind, stream_id, bytes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor as IoCursor;
    use termi9ne_terminal::Cell;
    use tokio::io::{BufReader, duplex};

    #[tokio::test]
    async fn a_request_round_trips_as_one_frame() {
        let request = ClientRequest::new(Request::Ping);
        let (left, right) = duplex(1024);
        let (_left_read, left_write) = tokio::io::split(left);
        let write_request = request.clone();

        let write = tokio::spawn(async move {
            let mut writer = wire_v3::AsyncWireWriter::new(left_write);
            writer
                .send_json(wire_v3::FrameKind::Request, 0, &write_request)
                .await
        });
        let mut reader = wire_v3::AsyncWireReader::new(BufReader::new(right));
        let decoded: ClientRequest = reader
            .receive_json(wire_v3::FrameKind::Request, 0)
            .await
            .expect("frame should decode");
        write
            .await
            .expect("writer task should finish")
            .expect("write should succeed");

        assert_eq!(decoded, request);
    }

    #[test]
    fn synchronous_and_async_framing_use_the_same_wire_format() {
        let request = ClientRequest::new(Request::Ping);
        let mut writer = wire_v3::SyncWire::new(IoCursor::new(Vec::new()));
        writer
            .send_json(wire_v3::FrameKind::Request, 0, &request)
            .expect("sync frame should encode");
        let sync_bytes = writer.into_inner().into_inner();
        let mut reader = wire_v3::SyncWire::new(IoCursor::new(sync_bytes.clone()));
        let decoded: ClientRequest = reader
            .receive_json(wire_v3::FrameKind::Request, 0)
            .expect("sync frame should decode");

        assert_eq!(decoded, request);
        assert_eq!(&sync_bytes[..4], b"T9NE");
    }

    #[test]
    fn terminal_events_round_trip_through_the_v3_subscription_codec() {
        let event = ServerEvent::TerminalBell {
            session_id: SessionId::new(),
            count: 7,
        };
        let (kind, bytes) = encode_terminal_event(&event).expect("terminal event should encode");
        let decoded = decode_terminal_event(kind, &bytes)
            .expect("terminal event should decode from its codec");

        assert_eq!(decoded, event);
        assert_eq!(kind, wire_v3::FrameKind::EventBatch);
    }

    #[test]
    fn truncated_and_invalid_json_frames_are_distinguished() {
        let mut writer = wire_v3::SyncWire::new(IoCursor::new(Vec::new()));
        writer
            .send_bytes(wire_v3::FrameKind::Request, 0, b"{}".to_vec())
            .expect("fixture frame");
        let mut truncated = writer.into_inner().into_inner();
        truncated.pop();
        let mut reader = wire_v3::SyncWire::new(IoCursor::new(truncated));
        assert!(matches!(
            reader.receive_json::<ClientRequest>(wire_v3::FrameKind::Request, 0),
            Err(wire_v3::WireError::Io(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof
        ));

        let mut writer = wire_v3::SyncWire::new(IoCursor::new(Vec::new()));
        writer
            .send_bytes(wire_v3::FrameKind::Request, 0, b"{".to_vec())
            .expect("fixture frame");
        let invalid_json = writer.into_inner().into_inner();
        let mut reader = wire_v3::SyncWire::new(IoCursor::new(invalid_json));
        assert!(matches!(
            reader.receive_json::<ClientRequest>(wire_v3::FrameKind::Request, 0),
            Err(wire_v3::WireError::Json(_))
        ));
    }

    #[test]
    fn row_deltas_require_an_exact_base_and_reconstruct_the_next_frame() {
        let row = |text: &str| Row {
            wrapped: false,
            cells: vec![Cell {
                grapheme: text.to_owned(),
                width: 1,
                style_index: 0,
                hyperlink: None,
            }],
        };
        let color = Rgb {
            red: 1,
            green: 2,
            blue: 3,
        };
        let mut base = FullFrame {
            sequence: 4,
            grid: GridSize::new(2, 2).expect("valid grid"),
            rows: vec![Arc::new(row("a")), Arc::new(row("b"))],
            styles: vec![CellStyle {
                foreground: color,
                background: color,
                bold: false,
                italic: false,
                faint: false,
                blink: false,
                inverse: false,
                invisible: false,
                strikethrough: false,
                overline: false,
                underline: termi9ne_terminal::UnderlineStyle::None,
            }],
            cursor: None,
            default_foreground: color,
            default_background: color,
            mouse_tracking: false,
            title: Some("shell".to_owned()),
            current_directory: Some("file://localhost/tmp".to_owned()),
        };
        let mut next = base.clone();
        next.sequence = 5;
        next.rows[1] = Arc::new(row("c"));
        let delta = FrameDelta::between(&base, &next).expect("same-grid frame should delta");

        assert_eq!(delta.changed_rows.len(), 1);
        assert_eq!(delta.changed_rows[0].index, 1);
        let event = ServerEvent::TerminalDelta {
            session_id: SessionId::new(),
            delta: Box::new(delta.clone()),
        };
        let (kind, bytes) = encode_terminal_event(&event).expect("delta should encode as protobuf");
        assert_eq!(kind, wire_v3::FrameKind::FrameDelta);
        assert_eq!(
            decode_terminal_event(kind, &bytes).expect("nested terminal delta should decode"),
            event
        );
        delta.apply_to(&mut base).expect("exact delta should apply");
        assert_eq!(base, next);
        assert!(matches!(
            delta.apply_to(&mut base),
            Err(ProtocolError::DeltaBaseMismatch { .. })
        ));
    }
}
