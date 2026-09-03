mod app_menu;
mod app_zoom;
mod assets;
mod extension;
mod fault_panel;
mod overlay_scrollbar;
mod pane_layout;
mod performance_monitor;
mod provider_usage;
mod provider_usage_panel;
mod session_sidebar;
mod status_bar;
mod terminal_element;
mod terminal_surface;
mod theme;
mod workspace_store;
mod workspace_tabs;

use std::time::{Duration, Instant};
#[cfg(not(test))]
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    thread,
};

#[cfg(not(test))]
use clap::Parser;
use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, DragMoveEvent, Entity, FontWeight, IntoElement,
    MouseButton, Pixels, Point, PromptLevel, Render, Window, WindowBounds, WindowControlArea,
    WindowDecorations, WindowOptions, accesskit::Role, actions, div, point, prelude::*, px,
    relative,
};
#[cfg(not(test))]
use gpui::{KeyBinding, Subscription, WindowHandle, size};
#[cfg(not(test))]
use termi9ne_client::{ClientError, ControlClient, DaemonSession};
#[cfg(not(test))]
use termi9ne_core::{
    Actor, ActorId, Command, DeliveryRunPurpose, GrantEnforcement, GrantId, Mission, MissionId,
    RunId, SessionId, SignalId, SignalKind,
};
#[cfg(not(test))]
use termi9ne_protocol::{
    MissionHistoryEntry, PROTOCOL_VERSION, RunActivitySource, RunActivityState, RunActivitySummary,
    SessionGroupChange, SessionGroupId, SessionGroupSpec, SessionGroupSummary, TerminalSessionSpec,
    TerminalSessionStatus, TerminalSessionSummary, default_socket_path,
};
use termi9ne_protocol::{
    PluginRuntimeStateSummary, PluginRuntimeSummary, TerminalForegroundProcess,
};
#[cfg(not(test))]
use termi9ne_terminal::GridSize;

use crate::app_zoom::{AppZoom, ui_size};
use crate::pane_layout::{
    ColumnSplitDrag, PaneLayoutStore, RowSplitDrag, SPLITTER_THICKNESS, SidebarEdgeDrag,
    SplitDragPreview,
};
use crate::performance_monitor::{
    PerformanceSample, PerformanceStats, format_bytes, format_rate, format_uptime,
    normalized_display_fps,
};
#[cfg(not(test))]
use crate::performance_monitor::{
    measured_render_fps, render_probe_progress, spawn_performance_sampler,
};
use crate::provider_usage::{ProviderKind, ProviderUsageSettings, ProviderUsageSnapshot};
use crate::session_sidebar::SessionSidebarState;
use crate::status_bar::{StatusBar, WidgetPriority, metric as status_metric};
use crate::terminal_surface::TerminalSurface;
#[cfg(not(test))]
use crate::terminal_surface::TerminalSurfaceEvent;
#[cfg(not(test))]
use crate::theme::IntoThemeColor;
use crate::theme::{
    ACTIVE, CHALK, ColorToken, DECK, FAULT, HAIRLINE, PANEL, PRODUCT_FONT, RELAY, SIGNAL, SUCCESS,
    TRACE, ThemeSelection, UI_FONT, rgb,
};
#[cfg(not(test))]
use crate::workspace_store::{SessionRecord, WorkspaceDocument, WorkspaceRecord, WorkspaceStore};
use crate::workspace_tabs::{TabPlacement, TabStripLayout, WorkspaceId, WorkspaceTabs};

const FPS_BENCHMARK_DURATION: Duration = Duration::from_secs(5);
const MACOS_TRAFFIC_LIGHT_INSET: f32 = 72.0;

fn macos_titlebar_leading_inset(is_fullscreen: bool) -> f32 {
    if is_fullscreen {
        0.0
    } else {
        MACOS_TRAFFIC_LIGHT_INSET
    }
}

/// Name a new workspace after the directory it opens in, falling back to a
/// numbered name when the directory is unknown.
fn workspace_display_name(sequence: u64) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|directory| {
            directory
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.trim().is_empty())
        .map(|name| {
            if sequence <= 1 {
                name
            } else {
                format!("{name} {sequence}")
            }
        })
        .unwrap_or_else(|| format!("Workspace {sequence}"))
}

fn initial_provider_usage(settings: &ProviderUsageSettings) -> Vec<ProviderUsageSnapshot> {
    ProviderKind::ALL
        .iter()
        .map(|kind| ProviderUsageSnapshot::initial(*kind, settings.enabled(*kind)))
        .collect()
}

#[cfg(not(test))]
#[derive(Debug, Parser)]
#[command(about = "Native termi9ne workspace for local or forwarded runtimes")]
struct DesktopArgs {
    #[arg(long, default_value_os_t = desktop_default_socket_path())]
    socket: PathBuf,
    #[arg(long, default_value_os_t = desktop_default_state_dir())]
    state_dir: PathBuf,
    /// Connect to an existing socket only; never spawn a local runtime.
    #[arg(long)]
    connect_only: bool,
    /// Compact runtime identity shown in the titlebar (for example, STAGING).
    #[arg(long)]
    runtime_label: Option<String>,
    /// Built-in theme name (`graphite` or `paper`) or a validated JSON theme file.
    #[arg(long, value_name = "NAME_OR_PATH")]
    theme: Option<String>,
    /// Read an observer Share token from an owner-only regular file.
    #[arg(long, requires = "connect_only")]
    share_token_file: Option<PathBuf>,
    /// Run the exact embedded daemon build without opening a desktop window.
    #[arg(long, hide = true)]
    internal_daemon: bool,
    /// Run an output-driven renderer benchmark, print JSON, and exit.
    #[arg(long, hide = true, conflicts_with = "idle_benchmark")]
    render_benchmark: bool,
    /// Measure a settled, quiet desktop, print JSON, and exit.
    #[arg(long, hide = true, conflicts_with = "render_benchmark")]
    idle_benchmark: bool,
}

#[cfg(not(test))]
fn desktop_default_socket_path() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from(format!(".termi9ne-dev/v{PROTOCOL_VERSION}/control.sock"))
    } else {
        default_socket_path()
    }
}

#[cfg(not(test))]
fn desktop_default_state_dir() -> PathBuf {
    if cfg!(debug_assertions) {
        PathBuf::from(format!(".termi9ne-dev/v{PROTOCOL_VERSION}"))
    } else {
        PathBuf::from(".termi9ne")
    }
}

actions!(
    termi9ne,
    [
        NewWorkspace,
        CloseActiveWorkspace,
        RestoreClosedWorkspace,
        NextWorkspace,
        PreviousWorkspace,
        ToggleWorkspacePin,
        ToggleCommandDeck,
        NewSession,
        NewTerminal,
        NextSession,
        PreviousSession,
        ToggleSidebar,
        ToggleFocusMode,
        OpenGraphInspector,
        OpenPluginManager,
        OpenProviderSettings,
        OpenFaults,
        ActivateWorkspace1,
        ActivateWorkspace2,
        ActivateWorkspace3,
        ActivateWorkspace4,
        ActivateWorkspace5,
        ActivateWorkspace6,
        ActivateWorkspace7,
        ActivateWorkspace8,
        ActivateLastWorkspace,
        AboutTermi9ne,
        CopyTerminal,
        PasteTerminal,
        HideApplication,
        HideOtherApplications,
        ShowAllApplications,
        QuitApplication,
        MinimizeWindow,
        ZoomWindow,
        ToggleFullScreen,
        IncreaseAppZoom,
        DecreaseAppZoom,
        ResetAppZoom,
        UseGraphiteTheme,
        UsePaperTheme,
        ReloadTheme,
    ]
);

#[cfg(test)]
const FOUNDATION_SHELL: &[u8] = b"\x1b[2J\x1b[H$ termi9ne verify --platform native\r\n\x1b[1;34mghostty\x1b[0m  terminal state pinned\r\n\x1b[1;32minput\x1b[0m    keyboard, IME, focus and paste connected\r\n\x1b[1;33mnext\x1b[0m     connect a real PTY";
#[cfg(test)]
const MACOS_SMOKE: &[u8] = b"\x1b[2J\x1b[H$ cargo test -p termi9ne-desktop\r\n\r\nrunning 7 tests\r\n\x1b[1;32mtest result: ok\x1b[0m. 7 passed; 0 failed\r\n\r\n$ _";
#[cfg(test)]
const PTY_SPIKE: &[u8] = b"\x1b[2J\x1b[H$ termi9ne session create shell\r\n\x1b[1;33mwaiting\x1b[0m  PTY runtime milestone\r\n\x1b[2mfixture surface; no child process attached yet\x1b[0m";
#[cfg(test)]
const WAYLAND_SMOKE: &[u8] = b"\x1b[2J\x1b[H$ termi9ne verify --display wayland\r\n\x1b[1;32mpass\x1b[0m  GPUI window and terminal frame\r\n\x1b[1;32mpass\x1b[0m  keyboard and clipboard adapter\r\n\r\n$ _";
#[cfg(test)]
const X11_SMOKE: &[u8] = b"\x1b[2J\x1b[H$ termi9ne verify --display x11\r\n\x1b[1;32mpass\x1b[0m  virtual display smoke\r\n\x1b[1;33mnext\x1b[0m  package on a native Linux runner";
#[cfg(test)]
const EMPTY_SHELL: &[u8] =
    b"\x1b[2J\x1b[H\x1b[2mfixture terminal; PTY connection is the next milestone\x1b[0m\r\n$ _";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionStatus {
    Working,
    /// An agent is blocked on a person: input, approval, or a blocker.
    Waiting,
    Idle,
    Closed,
    Terminated,
}

impl SessionStatus {
    fn label(self, actor: &str) -> String {
        match self {
            Self::Working => format!("{actor} working"),
            Self::Waiting => format!("{actor} waiting"),
            Self::Idle => "idle".to_owned(),
            Self::Closed => "closed".to_owned(),
            Self::Terminated => "terminated".to_owned(),
        }
    }

    fn color(self) -> ColorToken {
        match self {
            Self::Working => SUCCESS,
            Self::Waiting => SIGNAL,
            Self::Idle => TRACE,
            Self::Closed => HAIRLINE,
            Self::Terminated => FAULT,
        }
    }

    fn is_live(self) -> bool {
        matches!(self, Self::Working | Self::Waiting | Self::Idle)
    }
}

#[derive(Clone)]
struct SessionView {
    #[cfg(not(test))]
    group_id: SessionGroupId,
    #[cfg(not(test))]
    group_version: u64,
    #[cfg(not(test))]
    pinned: bool,
    name: String,
    actor: String,
    status: SessionStatus,
    terminals: Vec<usize>,
}

struct WorkspaceView {
    sessions: Vec<SessionView>,
    selected_session: usize,
    focus_mode: bool,
    #[cfg(not(test))]
    mission_id: Option<MissionId>,
    #[cfg(not(test))]
    mission: Option<Mission>,
}

impl WorkspaceView {
    fn live_session_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|session| session.status.is_live())
            .count()
    }

    fn attention_count(&self) -> usize {
        #[cfg(not(test))]
        {
            self.mission
                .as_ref()
                .map_or(0, |mission| mission.attention_queue().len())
        }
        #[cfg(test)]
        {
            0
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct SurfaceFixture {
    id: &'static str,
    output: &'static [u8],
}

#[cfg(test)]
const SURFACE_FIXTURES: [SurfaceFixture; 5] = [
    SurfaceFixture {
        id: "foundation-shell",
        output: FOUNDATION_SHELL,
    },
    SurfaceFixture {
        id: "macos-smoke",
        output: MACOS_SMOKE,
    },
    SurfaceFixture {
        id: "pty-spike",
        output: PTY_SPIKE,
    },
    SurfaceFixture {
        id: "wayland-smoke",
        output: WAYLAND_SMOKE,
    },
    SurfaceFixture {
        id: "x11-smoke",
        output: X11_SMOKE,
    },
];

#[derive(Clone)]
struct DraggedWorkspaceTab {
    id: WorkspaceId,
    title: String,
    position: Point<Pixels>,
}

impl DraggedWorkspaceTab {
    fn at(mut self, position: Point<Pixels>) -> Self {
        self.position = position;
        self
    }
}

impl Render for DraggedWorkspaceTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .left(self.position.x - px(96.0))
            .top(self.position.y - px(18.0))
            .w(px(192.0))
            .h(px(36.0))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .rounded(px(4.0))
            .border_1()
            .border_color(rgb(RELAY))
            .bg(rgb(ACTIVE).alpha(0.94))
            .font_family(PRODUCT_FONT)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(CHALK))
            .shadow_lg()
            .child(div().size(px(6.0)).rounded_full().bg(rgb(SUCCESS)))
            .child(self.title.clone())
    }
}

struct Termi9neDesktop {
    surfaces: Vec<Entity<TerminalSurface>>,
    surface_foreground_processes: Vec<Option<TerminalForegroundProcess>>,
    workspaces: WorkspaceTabs<WorkspaceView>,
    session_sidebar: SessionSidebarState,
    active_surface: Option<usize>,
    sidebar_open: bool,
    sidebar_custom_width: Option<f32>,
    app_zoom: AppZoom,
    pane_layouts: PaneLayoutStore,
    /// Developer telemetry rail; hidden by default outside tests.
    status_bar_visible: bool,
    /// Faults known to this desktop, newest first.
    faults: Vec<termi9ne_protocol::FaultSummary>,
    fault_panel_open: bool,
    selected_fault: Option<termi9ne_core::FaultId>,
    fault_busy: bool,
    fault_error: Option<String>,
    /// Surface whose shell should be replaced on the next render pass.
    pending_terminal_restart: Option<usize>,
    command_deck_open: bool,
    command_focus: gpui::FocusHandle,
    command_query: String,
    pending_termination: Option<usize>,
    tab_overflow_open: bool,
    next_fixture: u64,
    runtime_label: String,
    theme_selection: ThemeSelection,
    theme_name: String,
    attached_runtime: bool,
    shared_mode: bool,
    plugin_manager_open: bool,
    plugin_refreshing: bool,
    plugin_summaries: Vec<PluginRuntimeSummary>,
    plugin_events_dropped: u64,
    plugin_error: Option<String>,
    provider_usage: Vec<ProviderUsageSnapshot>,
    provider_usage_settings: ProviderUsageSettings,
    provider_settings_open: bool,
    provider_settings_tab: ProviderKind,
    #[cfg(not(test))]
    provider_usage_generation: u64,
    performance_stats: PerformanceStats,
    performance_benchmark_running: bool,
    performance_render_probe_running: bool,
    performance_render_probe_progress: f32,
    #[cfg(not(test))]
    performance_last_frame_count: u64,
    #[cfg(not(test))]
    performance_sampled_at: Instant,
    #[cfg(not(test))]
    performance_monitor_started: bool,
    #[cfg(not(test))]
    performance_probed_display: Option<u64>,
    #[cfg(not(test))]
    automated_benchmark: bool,
    #[cfg(not(test))]
    automated_idle_benchmark: bool,
    #[cfg(not(test))]
    startup_started_at: Instant,
    #[cfg(not(test))]
    startup_to_first_render_ms: Option<f32>,
    #[cfg(not(test))]
    control: ControlClient,
    #[cfg(not(test))]
    surface_sessions: Vec<SessionId>,
    #[cfg(not(test))]
    surface_missions: Vec<Option<MissionId>>,
    #[cfg(not(test))]
    surface_runs: Vec<Option<RunId>>,
    #[cfg(not(test))]
    run_activities: HashMap<RunId, RunActivitySummary>,
    #[cfg(not(test))]
    workspace_store: WorkspaceStore,
    #[cfg(not(test))]
    surface_subscriptions: Vec<Subscription>,
    #[cfg(not(test))]
    surface_activity: HashMap<usize, u64>,
    #[cfg(not(test))]
    activity_generation: u64,
    #[cfg(not(test))]
    surface_statuses: Vec<TerminalSessionStatus>,
    #[cfg(not(test))]
    pending_terminal_updates: Vec<TerminalSessionSummary>,
    #[cfg(not(test))]
    archived_terminals: Vec<TerminalSessionSummary>,
    #[cfg(not(test))]
    detached_groups: Vec<SessionGroupSummary>,
    #[cfg(not(test))]
    archive_drawer_open: bool,
    #[cfg(not(test))]
    window_handle: Option<WindowHandle<Termi9neDesktop>>,
    #[cfg(not(test))]
    selected_attention: Option<SignalId>,
    #[cfg(not(test))]
    graph_inspector_open: bool,
    #[cfg(not(test))]
    selected_graph_run: Option<RunId>,
    #[cfg(not(test))]
    review_focus: gpui::FocusHandle,
    #[cfg(not(test))]
    review_note: String,
    #[cfg(not(test))]
    review_action_busy: bool,
    #[cfg(not(test))]
    review_action_status: Option<(bool, String)>,
    #[cfg(not(test))]
    mission_history: Vec<MissionHistoryEntry>,
    #[cfg(not(test))]
    mission_history_has_more: bool,
}

#[cfg(not(test))]
struct LiveSurface {
    session_id: SessionId,
    mission_id: Option<MissionId>,
    run_id: Option<RunId>,
    status: TerminalSessionStatus,
    foreground_process: Option<TerminalForegroundProcess>,
    surface: Entity<TerminalSurface>,
}

#[cfg(not(test))]
struct LiveDesktopConfig {
    runtime_label: String,
    attached_runtime: bool,
    workspace_state_path: PathBuf,
    theme_override: Option<ThemeSelection>,
    automated_benchmark: bool,
    automated_idle_benchmark: bool,
    startup_started_at: Instant,
}

#[cfg(not(test))]
const fn session_status(status: TerminalSessionStatus) -> SessionStatus {
    match status {
        TerminalSessionStatus::Running => SessionStatus::Idle,
        TerminalSessionStatus::Exited => SessionStatus::Closed,
        TerminalSessionStatus::Failed => SessionStatus::Terminated,
    }
}

#[cfg(not(test))]
fn aggregate_status(terminals: &[usize], surfaces: &[LiveSurface]) -> SessionStatus {
    if terminals
        .iter()
        .any(|index| surfaces[*index].status == TerminalSessionStatus::Running)
    {
        SessionStatus::Idle
    } else if terminals
        .iter()
        .any(|index| surfaces[*index].status == TerminalSessionStatus::Failed)
    {
        SessionStatus::Terminated
    } else {
        SessionStatus::Closed
    }
}

#[cfg(not(test))]
fn aggregate_status_from_state(
    terminals: &[usize],
    statuses: &[TerminalSessionStatus],
    activity: &HashMap<usize, u64>,
) -> SessionStatus {
    if terminals.iter().any(|index| activity.contains_key(index)) {
        SessionStatus::Working
    } else if terminals
        .iter()
        .any(|index| statuses.get(*index) == Some(&TerminalSessionStatus::Running))
    {
        SessionStatus::Idle
    } else if terminals
        .iter()
        .any(|index| statuses.get(*index) == Some(&TerminalSessionStatus::Failed))
    {
        SessionStatus::Terminated
    } else {
        SessionStatus::Closed
    }
}

#[cfg(not(test))]
fn resolve_mission(
    control: &ControlClient,
    mission_id: Option<MissionId>,
    intent: &str,
) -> Option<Mission> {
    if let Some(mission_id) = mission_id {
        match control.get_mission(mission_id) {
            Ok(mission) => return Some(mission),
            Err(error) => eprintln!("mission restore failed for {mission_id}: {error}"),
        }
    }

    let mission_id = MissionId::new();
    let actor = Actor::human("you").expect("built-in human actor must be valid");
    match control.create_mission(mission_id, intent, actor) {
        Ok(mission) => Some(mission),
        Err(error) => {
            eprintln!("mission creation failed: {error}");
            None
        }
    }
}

#[cfg(not(test))]
fn run_depth(mission: &Mission, run_id: RunId) -> usize {
    let mut depth = 0;
    let mut current = mission.runs.get(&run_id).and_then(|run| run.parent);
    while let Some(parent) = current {
        depth += 1;
        if depth >= mission.runs.len() {
            break;
        }
        current = mission.runs.get(&parent).and_then(|run| run.parent);
    }
    depth
}

#[cfg(not(test))]
fn short_id(id: impl ToString) -> String {
    id.to_string().chars().take(8).collect()
}

#[cfg(not(test))]
fn status_color_for_run(status: termi9ne_core::RunStatus) -> ColorToken {
    match status {
        termi9ne_core::RunStatus::Pending => TRACE,
        termi9ne_core::RunStatus::Running => SUCCESS,
        termi9ne_core::RunStatus::AwaitingAttention => SIGNAL,
        termi9ne_core::RunStatus::Paused => SIGNAL,
        termi9ne_core::RunStatus::Succeeded => RELAY,
        termi9ne_core::RunStatus::Failed | termi9ne_core::RunStatus::Cancelled => FAULT,
    }
}

#[cfg(not(test))]
fn metric_chip(label: &str, value: usize, color: impl IntoThemeColor) -> AnyElement {
    div()
        .h(px(24.0))
        .px_2()
        .flex()
        .items_center()
        .gap_1()
        .rounded(px(4.0))
        .bg(rgb(PANEL))
        .font_family(UI_FONT)
        .text_xs()
        .text_color(rgb(TRACE))
        .child(div().text_color(rgb(color)).child(value.to_string()))
        .child(label.to_owned())
        .into_any_element()
}

#[cfg(not(test))]
fn review_fact(label: &str, value: String, color: impl IntoThemeColor) -> AnyElement {
    div()
        .pl_2()
        .border_l_2()
        .border_color(rgb(color))
        .flex()
        .flex_col()
        .gap_1()
        .font_family(UI_FONT)
        .child(
            div()
                .text_xs()
                .text_color(rgb(TRACE))
                .child(label.to_owned()),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(CHALK))
                .whitespace_normal()
                .child(value),
        )
        .into_any_element()
}

fn plugin_row(plugin: &PluginRuntimeSummary) -> AnyElement {
    let (state, color) = match plugin.state {
        PluginRuntimeStateSummary::Starting => ("starting", SIGNAL),
        PluginRuntimeStateSummary::Running => ("running", SUCCESS),
        PluginRuntimeStateSummary::Backoff => ("restarting", SIGNAL),
        PluginRuntimeStateSummary::Failed => ("failed", FAULT),
        PluginRuntimeStateSummary::Stopped => ("stopped", TRACE),
    };
    let detail = plugin
        .last_status
        .as_ref()
        .map(|status| status.text.clone())
        .or_else(|| plugin.last_error.clone())
        .unwrap_or_else(|| "Waiting for a semantic event".to_owned());
    div()
        .flex()
        .items_center()
        .gap_3()
        .p_3()
        .rounded(px(4.0))
        .border_1()
        .border_color(rgb(HAIRLINE))
        .bg(rgb(ACTIVE))
        .child(div().size(px(7.0)).rounded_full().bg(rgb(color)))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(CHALK))
                        .child(plugin.name.clone()),
                )
                .child(
                    div()
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(TRACE))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(detail),
                ),
        )
        .child(
            div()
                .ml_auto()
                .flex()
                .flex_col()
                .items_end()
                .gap_1()
                .font_family(UI_FONT)
                .text_xs()
                .child(div().text_color(rgb(color)).child(state))
                .child(div().text_color(rgb(TRACE)).child(format!(
                    "v{} · restart {} · drop {}",
                    plugin.version, plugin.restart_count, plugin.dropped_events
                ))),
        )
        .into_any_element()
}

fn performance_max_status_widget(
    value: String,
    probe_progress: f32,
    probe_running: bool,
) -> AnyElement {
    let block_left = 1.0 + probe_progress.clamp(0.0, 1.0) * 21.0;
    div()
        .id("status-max-fps")
        .debug_selector(|| "status-max-fps".to_owned())
        .h_full()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .font_family(UI_FONT)
        .whitespace_nowrap()
        .child(div().text_color(rgb(TRACE)).child("MAX"))
        .child(div().text_color(rgb(SUCCESS)).child(value))
        .child(
            div()
                .id("performance-render-probe")
                .debug_selector(|| "performance-render-probe".to_owned())
                .relative()
                .w(px(27.0))
                .h(px(7.0))
                .overflow_hidden()
                .rounded(px(2.0))
                .border_1()
                .border_color(rgb(if probe_running { RELAY } else { HAIRLINE }))
                .bg(rgb(PANEL))
                .child(
                    div()
                        .absolute()
                        .top(px(1.0))
                        .left(px(block_left))
                        .size(px(3.0))
                        .rounded(px(2.0))
                        .bg(rgb(if probe_running { SUCCESS } else { TRACE })),
                ),
        )
        .into_any_element()
}

#[cfg(not(test))]
fn schedule_display_refresh_probe(
    display_id: u64,
    started_at: Instant,
    starting_frame_count: u64,
    window: &mut Window,
    cx: &mut Context<Termi9neDesktop>,
) {
    cx.on_next_frame(window, move |desktop, window, cx| {
        let current_display_id = window.display(cx).map(|display| u64::from(display.id()));
        if current_display_id != Some(display_id) {
            desktop.performance_render_probe_running = false;
            desktop.performance_probed_display = None;
            cx.notify();
            return;
        }

        let elapsed = started_at.elapsed();
        let probe_complete = elapsed >= Duration::from_secs(1);
        let ending_frame_count = window
            .frame_duration_snapshot()
            .draw_duration_histogram
            .len();
        let measured_fps = probe_complete
            .then(|| measured_render_fps(starting_frame_count, ending_frame_count, elapsed))
            .flatten();
        if desktop.performance_probed_display != Some(display_id) {
            return;
        }

        desktop.performance_render_probe_progress = render_probe_progress(elapsed);
        if probe_complete {
            desktop.performance_render_probe_running = false;
            desktop.performance_stats.display_max_fps = measured_fps.map(normalized_display_fps);
        } else {
            desktop.performance_render_probe_running = true;
            schedule_display_refresh_probe(
                display_id,
                started_at,
                starting_frame_count,
                window,
                cx,
            );
        }
        // Each probe tick must paint the block. Merely observing frame callbacks
        // measures callback cadence and can leave MAX pending in an idle window.
        cx.notify();
    });
}

fn schedule_fps_benchmark(
    started_at: Instant,
    starting_frame_count: u64,
    window: &mut Window,
    cx: &mut Context<Termi9neDesktop>,
) {
    cx.on_next_frame(window, move |desktop, window, cx| {
        let elapsed = started_at.elapsed();
        if elapsed < FPS_BENCHMARK_DURATION {
            // Notify the root view instead of calling `request_animation_frame`: the latter
            // requires an active paint/prepaint view and panics when RUN starts from mouse input.
            cx.notify();
            schedule_fps_benchmark(started_at, starting_frame_count, window, cx);
            return;
        }

        let ending_frame_count = window
            .frame_duration_snapshot()
            .draw_duration_histogram
            .len();
        let measured_fps =
            ending_frame_count.saturating_sub(starting_frame_count) as f32 / elapsed.as_secs_f32();
        let presented_fps = normalized_display_fps(measured_fps);
        desktop.performance_benchmark_running = false;
        desktop.performance_stats.record_benchmark(presented_fps);
        #[cfg(not(test))]
        if desktop.automated_benchmark {
            let p95_ms = window
                .frame_duration_snapshot()
                .draw_duration_histogram
                .value_at_quantile(0.95) as f32
                / 1_000_000.0;
            let frame_budget_ms = 1_000.0 / presented_fps.max(1.0);
            let budget_utilization = p95_ms / frame_budget_ms;
            let startup_ms = desktop.startup_to_first_render_ms.unwrap_or_default();
            let resident_mib = desktop.performance_stats.resident_bytes as f64 / 1_048_576.0;
            println!(
                "{{\"benchmark\":\"desktop_output_render\",\"fps\":{presented_fps:.2},\"measured_fps\":{measured_fps:.2},\"frame_p95_ms\":{p95_ms:.3},\"frame_budget_ms\":{frame_budget_ms:.3},\"budget_utilization\":{budget_utilization:.3},\"startup_to_first_render_ms\":{startup_ms:.1},\"desktop_cpu_percent\":{:.2},\"desktop_rss_mib\":{resident_mib:.1},\"visible_surfaces\":{}}}",
                desktop.performance_stats.cpu_percent,
                desktop.selected_terminals().len()
            );
            cx.quit();
        }
        cx.notify();
    });
}

impl Termi9neDesktop {
    fn apply_app_zoom(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_rem_size(self.app_zoom.rem_size());
        window.refresh();
        cx.notify();
    }

    fn increase_app_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.app_zoom.increase() {
            self.apply_app_zoom(window, cx);
        }
    }

    fn decrease_app_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.app_zoom.decrease() {
            self.apply_app_zoom(window, cx);
        }
    }

    fn reset_app_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.app_zoom.reset();
        self.apply_app_zoom(window, cx);
    }

    #[cfg(test)]
    fn new(
        surfaces: Vec<Entity<TerminalSurface>>,
        search_focus: gpui::FocusHandle,
        #[cfg(not(test))] control: ControlClient,
    ) -> Self {
        let workspaces = WorkspaceTabs::new(vec![
            (
                "Terminal foundation".to_owned(),
                WorkspaceView {
                    sessions: vec![
                        SessionView {
                            #[cfg(not(test))]
                            group_id: SessionGroupId::new(),
                            #[cfg(not(test))]
                            group_version: 0,
                            #[cfg(not(test))]
                            pinned: false,
                            name: "foundation".to_owned(),
                            actor: "codex".to_owned(),
                            status: SessionStatus::Working,
                            terminals: vec![0, 1],
                        },
                        SessionView {
                            #[cfg(not(test))]
                            group_id: SessionGroupId::new(),
                            #[cfg(not(test))]
                            group_version: 0,
                            #[cfg(not(test))]
                            pinned: false,
                            name: "pty-spike".to_owned(),
                            actor: "agent".to_owned(),
                            status: SessionStatus::Idle,
                            terminals: vec![2],
                        },
                        SessionView {
                            #[cfg(not(test))]
                            group_id: SessionGroupId::new(),
                            #[cfg(not(test))]
                            group_version: 0,
                            #[cfg(not(test))]
                            pinned: false,
                            name: "prototype-v0".to_owned(),
                            actor: String::new(),
                            status: SessionStatus::Closed,
                            terminals: Vec::new(),
                        },
                    ],
                    selected_session: 0,
                    focus_mode: false,
                },
            ),
            (
                "Linux parity".to_owned(),
                WorkspaceView {
                    sessions: vec![
                        SessionView {
                            #[cfg(not(test))]
                            group_id: SessionGroupId::new(),
                            #[cfg(not(test))]
                            group_version: 0,
                            #[cfg(not(test))]
                            pinned: false,
                            name: "linux-smoke".to_owned(),
                            actor: "codex".to_owned(),
                            status: SessionStatus::Working,
                            terminals: vec![3, 4],
                        },
                        SessionView {
                            #[cfg(not(test))]
                            group_id: SessionGroupId::new(),
                            #[cfg(not(test))]
                            group_version: 0,
                            #[cfg(not(test))]
                            pinned: false,
                            name: "x11-old".to_owned(),
                            actor: String::new(),
                            status: SessionStatus::Terminated,
                            terminals: Vec::new(),
                        },
                    ],
                    selected_session: 0,
                    focus_mode: false,
                },
            ),
        ])
        .expect("initial workspaces must not be empty");
        let command_focus = search_focus.clone();
        let surface_foreground_processes = vec![None; surfaces.len()];

        Self {
            surfaces,
            surface_foreground_processes,
            workspaces,
            session_sidebar: SessionSidebarState::new(search_focus),
            active_surface: Some(0),
            sidebar_open: true,
            sidebar_custom_width: None,
            app_zoom: AppZoom::default(),
            pane_layouts: PaneLayoutStore::default(),
            status_bar_visible: cfg!(test),
            faults: Vec::new(),
            fault_panel_open: false,
            selected_fault: None,
            fault_busy: false,
            fault_error: None,
            pending_terminal_restart: None,
            command_deck_open: false,
            command_focus,
            command_query: String::new(),
            pending_termination: None,
            tab_overflow_open: false,
            next_fixture: 1,
            runtime_label: "FIXTURE".to_owned(),
            theme_selection: ThemeSelection::default(),
            theme_name: "Graphite".to_owned(),
            attached_runtime: false,
            shared_mode: false,
            plugin_manager_open: false,
            plugin_refreshing: false,
            plugin_summaries: Vec::new(),
            plugin_events_dropped: 0,
            plugin_error: None,
            provider_usage: initial_provider_usage(&ProviderUsageSettings::default()),
            provider_usage_settings: ProviderUsageSettings::default(),
            provider_settings_open: false,
            provider_settings_tab: ProviderKind::Claude,
            #[cfg(not(test))]
            provider_usage_generation: 0,
            performance_stats: PerformanceStats::default(),
            performance_benchmark_running: false,
            performance_render_probe_running: false,
            performance_render_probe_progress: 0.0,
            #[cfg(not(test))]
            performance_last_frame_count: 0,
            #[cfg(not(test))]
            performance_sampled_at: Instant::now(),
            #[cfg(not(test))]
            performance_monitor_started: false,
            #[cfg(not(test))]
            performance_probed_display: None,
            #[cfg(not(test))]
            control,
        }
    }

    #[cfg(not(test))]
    fn new_live(
        live_surfaces: Vec<LiveSurface>,
        control: ControlClient,
        config: LiveDesktopConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_focus = cx.focus_handle();
        let command_focus = cx.focus_handle();
        let review_focus = cx.focus_handle();
        let workspace_store = WorkspaceStore::new(config.workspace_state_path);
        let mut archived_terminals = control
            .list_all_terminals()
            .unwrap_or_default()
            .into_iter()
            .filter(|terminal| terminal.archived)
            .collect::<Vec<_>>();
        archived_terminals.sort_by_key(|terminal| terminal.session_id.to_string());
        let loaded = match workspace_store.load() {
            Ok(document) => document,
            Err(error) => {
                eprintln!("workspace restore failed: {error}");
                None
            }
        };
        let provider_usage_settings = loaded
            .as_ref()
            .map(|document| document.provider_usage.clone())
            .unwrap_or_default();
        let requested_theme = config
            .theme_override
            .or_else(|| loaded.as_ref().map(|document| document.theme.clone()))
            .unwrap_or_default();
        let (theme_selection, theme_name) = match theme::activate(&requested_theme) {
            Ok(summary) => (requested_theme, summary.name),
            Err(error) => {
                eprintln!("theme activation failed: {error}");
                let fallback = ThemeSelection::default();
                let summary = theme::activate(&fallback)
                    .expect("the embedded Graphite theme must always be valid");
                (fallback, summary.name)
            }
        };
        cx.set_menus(app_menu::native_menus(&theme_selection));
        let surfaces = live_surfaces
            .iter()
            .map(|live| live.surface.clone())
            .collect::<Vec<_>>();
        let surface_foreground_processes = live_surfaces
            .iter()
            .map(|live| live.foreground_process.clone())
            .collect::<Vec<_>>();
        let surface_sessions = live_surfaces
            .iter()
            .map(|live| live.session_id)
            .collect::<Vec<_>>();
        let surface_statuses = live_surfaces
            .iter()
            .map(|live| live.status)
            .collect::<Vec<_>>();
        let surface_missions = live_surfaces
            .iter()
            .map(|live| live.mission_id)
            .collect::<Vec<_>>();
        let surface_runs = live_surfaces
            .iter()
            .map(|live| live.run_id)
            .collect::<Vec<_>>();
        let terminal_index = live_surfaces
            .iter()
            .enumerate()
            .map(|(index, live)| {
                (
                    live.session_id,
                    (index, live.status, live.mission_id, live.run_id),
                )
            })
            .collect::<HashMap<_, _>>();
        let mut used = HashSet::new();
        let mut next_sequence = 2;
        let mut active_hint = WorkspaceId::from_raw(1);
        let mut restored = Vec::new();
        let mut restored_layouts = Vec::new();
        let mut sidebar_custom_width = None;

        if let Some(document) = loaded {
            next_sequence = document.next_sequence.max(1);
            active_hint = WorkspaceId::from_raw(document.active_workspace);
            sidebar_custom_width = document.sidebar_width.map(f32::from);
            for workspace in document.workspaces {
                let title = workspace.title.clone();
                let mut sessions = Vec::new();
                for session in workspace.sessions {
                    let terminals = session
                        .terminals
                        .into_iter()
                        .filter_map(|id| {
                            terminal_index
                                .get(&id)
                                .and_then(|(index, _, bound_mission, _)| {
                                    (bound_mission.is_none()
                                        || *bound_mission == workspace.mission_id)
                                        .then(|| {
                                            used.insert(id);
                                            *index
                                        })
                                })
                        })
                        .collect::<Vec<_>>();
                    let status = aggregate_status(&terminals, &live_surfaces);
                    if let Some(layout) = session.pane_layout {
                        restored_layouts.push((terminals.clone(), layout));
                    }
                    sessions.push(SessionView {
                        group_id: session.group_id.unwrap_or_default(),
                        group_version: session.group_version,
                        pinned: session.pinned,
                        name: session.name,
                        actor: session.actor,
                        status,
                        terminals,
                    });
                }
                if !sessions.is_empty() {
                    let selected_session = workspace.selected_session.min(sessions.len() - 1);
                    restored.push((
                        WorkspaceId::from_raw(workspace.id),
                        title.clone(),
                        workspace.pinned,
                        WorkspaceView {
                            sessions,
                            selected_session,
                            focus_mode: workspace.focus_mode,
                            mission_id: workspace.mission_id,
                            mission: resolve_mission(&control, workspace.mission_id, &title),
                        },
                    ));
                }
            }
        }

        for (index, live) in live_surfaces.iter().enumerate() {
            let Some(mission_id) = live.mission_id else {
                continue;
            };
            if used.contains(&live.session_id) {
                continue;
            }
            let Some((_, _, _, workspace)) = restored
                .iter_mut()
                .find(|workspace| workspace.3.mission_id == Some(mission_id))
            else {
                continue;
            };
            let Some(domain_session) = workspace
                .mission
                .as_ref()
                .and_then(|mission| mission.sessions.get(&live.session_id))
            else {
                continue;
            };
            workspace.sessions.retain(|session| {
                !session.terminals.is_empty() || session.name != "mission history"
            });
            workspace.sessions.push(SessionView {
                group_id: SessionGroupId::new(),
                group_version: 0,
                pinned: false,
                name: domain_session.name.clone(),
                actor: domain_session.controller.id.to_string(),
                status: session_status(live.status),
                terminals: vec![index],
            });
            used.insert(live.session_id);
        }

        let mut orphan_sessions = live_surfaces
            .iter()
            .enumerate()
            .filter(|(_, live)| !used.contains(&live.session_id))
            .map(|(index, live)| SessionView {
                group_id: SessionGroupId::new(),
                group_version: 0,
                pinned: false,
                name: if index == 0 {
                    "shell".to_owned()
                } else {
                    format!("shell-{}", index + 1)
                },
                actor: "you".to_owned(),
                status: session_status(live.status),
                terminals: vec![index],
            })
            .collect::<Vec<_>>();
        if orphan_sessions.is_empty() {
            orphan_sessions.push(SessionView {
                group_id: SessionGroupId::new(),
                group_version: 0,
                pinned: false,
                name: "shared scope".to_owned(),
                actor: "observer".to_owned(),
                status: SessionStatus::Closed,
                terminals: Vec::new(),
            });
        }
        if restored.is_empty() {
            restored.push((
                WorkspaceId::from_raw(1),
                "Workspace 1".to_owned(),
                false,
                WorkspaceView {
                    sessions: orphan_sessions,
                    selected_session: 0,
                    focus_mode: false,
                    mission_id: None,
                    mission: None,
                },
            ));
            active_hint = WorkspaceId::from_raw(1);
        } else {
            restored[0].3.sessions.extend(orphan_sessions);
            if restored[0].3.sessions[restored[0].3.selected_session]
                .terminals
                .is_empty()
                && let Some(index) = restored[0]
                    .3
                    .sessions
                    .iter()
                    .position(|session| !session.terminals.is_empty())
            {
                restored[0].3.selected_session = index;
            }
        }
        let represented_missions = restored
            .iter()
            .filter_map(|workspace| workspace.3.mission_id)
            .collect::<HashSet<_>>();
        let mut next_workspace_id = restored
            .iter()
            .map(|workspace| workspace.0.raw())
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        if let Ok(missions) = control.list_missions() {
            for summary in missions {
                if represented_missions.contains(&summary.id)
                    || summary.status != termi9ne_core::MissionStatus::Active
                {
                    continue;
                }
                let Ok(mission) = control.get_mission(summary.id) else {
                    continue;
                };
                let mut domain_sessions = mission.sessions.values().collect::<Vec<_>>();
                domain_sessions.sort_by_key(|session| session.id);
                let mut sessions = domain_sessions
                    .into_iter()
                    .map(|session| {
                        let terminals = terminal_index
                            .get(&session.id)
                            .map(|(index, _, _, _)| vec![*index])
                            .unwrap_or_default();
                        let status = if terminals.is_empty() || session.status.is_finished() {
                            SessionStatus::Closed
                        } else {
                            aggregate_status(&terminals, &live_surfaces)
                        };
                        SessionView {
                            group_id: SessionGroupId::new(),
                            group_version: 0,
                            pinned: false,
                            name: session.name.clone(),
                            actor: session.controller.id.to_string(),
                            status,
                            terminals,
                        }
                    })
                    .collect::<Vec<_>>();
                if sessions.is_empty() {
                    sessions.push(SessionView {
                        group_id: SessionGroupId::new(),
                        group_version: 0,
                        pinned: false,
                        name: "mission history".to_owned(),
                        actor: String::new(),
                        status: SessionStatus::Closed,
                        terminals: Vec::new(),
                    });
                }
                restored.push((
                    WorkspaceId::from_raw(next_workspace_id),
                    summary.intent,
                    false,
                    WorkspaceView {
                        sessions,
                        selected_session: 0,
                        focus_mode: false,
                        mission_id: Some(mission.id),
                        mission: Some(mission),
                    },
                ));
                next_workspace_id = next_workspace_id.saturating_add(1);
            }
        }
        if !restored.iter().any(|workspace| workspace.0 == active_hint) {
            active_hint = restored[0].0;
        }
        let workspaces = WorkspaceTabs::restore(restored, active_hint)
            .expect("reconciled workspace state must be valid");
        let active_surface = workspaces
            .active()
            .content()
            .sessions
            .get(workspaces.active().content().selected_session)
            .and_then(|session| session.terminals.first())
            .copied();

        let mut desktop = Self {
            surfaces,
            surface_foreground_processes,
            workspaces,
            session_sidebar: SessionSidebarState::new(search_focus),
            active_surface,
            sidebar_open: true,
            sidebar_custom_width,
            app_zoom: AppZoom::default(),
            pane_layouts: PaneLayoutStore::default(),
            status_bar_visible: false,
            faults: Vec::new(),
            fault_panel_open: false,
            selected_fault: None,
            fault_busy: false,
            fault_error: None,
            pending_terminal_restart: None,
            command_deck_open: false,
            command_focus,
            command_query: String::new(),
            pending_termination: None,
            tab_overflow_open: false,
            next_fixture: next_sequence,
            runtime_label: config.runtime_label,
            theme_selection,
            theme_name,
            attached_runtime: config.attached_runtime,
            shared_mode: control.is_shared(),
            plugin_manager_open: false,
            plugin_refreshing: false,
            plugin_summaries: Vec::new(),
            plugin_events_dropped: 0,
            plugin_error: None,
            provider_usage: initial_provider_usage(&provider_usage_settings),
            provider_usage_settings,
            provider_settings_open: false,
            provider_settings_tab: ProviderKind::Claude,
            provider_usage_generation: 0,
            performance_stats: PerformanceStats::default(),
            performance_benchmark_running: false,
            performance_render_probe_running: false,
            performance_render_probe_progress: 0.0,
            performance_last_frame_count: 0,
            performance_sampled_at: Instant::now(),
            performance_monitor_started: false,
            performance_probed_display: None,
            automated_benchmark: config.automated_benchmark,
            automated_idle_benchmark: config.automated_idle_benchmark,
            startup_started_at: config.startup_started_at,
            startup_to_first_render_ms: None,
            control,
            surface_sessions,
            surface_missions,
            surface_runs,
            run_activities: HashMap::new(),
            workspace_store,
            surface_subscriptions: Vec::new(),
            surface_activity: HashMap::new(),
            activity_generation: 0,
            surface_statuses,
            pending_terminal_updates: Vec::new(),
            archived_terminals,
            detached_groups: Vec::new(),
            archive_drawer_open: false,
            window_handle: None,
            selected_attention: None,
            graph_inspector_open: false,
            selected_graph_run: None,
            review_focus,
            review_note: String::new(),
            review_action_busy: false,
            review_action_status: None,
            mission_history: Vec::new(),
            mission_history_has_more: false,
        };
        for (terminals, layout) in restored_layouts {
            desktop.pane_layouts.insert(terminals, layout);
        }
        for (index, surface) in desktop.surfaces.iter().enumerate() {
            desktop.surface_subscriptions.push(cx.subscribe(
                surface,
                move |desktop, _, event: &TerminalSurfaceEvent, cx| {
                    desktop.handle_surface_event(index, event, cx);
                },
            ));
        }
        desktop.sync_surface_presentation(cx);
        if !desktop.shared_mode {
            desktop.ensure_workspace_missions();
            desktop.reconcile_domain_sessions();
            desktop.reconcile_session_group_index();
        }
        desktop.start_mission_updates(cx);
        desktop.start_activity_updates(cx);
        desktop.start_terminal_updates(cx);
        if !desktop.shared_mode {
            desktop.start_session_group_updates(cx);
        }
        desktop.start_provider_usage_polling(cx);
        desktop.refresh_faults(cx);
        desktop.persist_workspaces();
        desktop
    }

    #[cfg(not(test))]
    fn start_performance_monitor(&self, cx: &mut Context<Self>) {
        let Some(window_handle) = self.window_handle else {
            eprintln!("performance monitor could not resolve the desktop window");
            return;
        };
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        spawn_performance_sampler(send);

        cx.spawn(async move |_, cx| {
            while let Some(sample) = receive.recv().await {
                if window_handle
                    .update(cx, |desktop, window, cx| {
                        desktop.apply_performance_sample(sample, window);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    #[cfg(not(test))]
    fn apply_performance_sample(&mut self, sample: PerformanceSample, window: &Window) {
        let now = Instant::now();
        let elapsed = now
            .duration_since(self.performance_sampled_at)
            .as_secs_f32();
        let frames = window.frame_duration_snapshot();
        let frame_count = frames.draw_duration_histogram.len();
        if elapsed > 0.0 {
            self.performance_stats.update_fps(
                frame_count.saturating_sub(self.performance_last_frame_count) as f32 / elapsed,
            );
        }
        if !frames.draw_duration_histogram.is_empty() {
            self.performance_stats.frame_p95_ms =
                frames.draw_duration_histogram.value_at_quantile(0.95) as f32 / 1_000_000.0;
        }
        self.performance_stats.update_process(sample);
        self.performance_last_frame_count = frame_count;
        self.performance_sampled_at = now;
    }

    #[cfg(not(test))]
    fn start_display_refresh_probe(
        &self,
        display_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let starting_frame_count = window
            .frame_duration_snapshot()
            .draw_duration_histogram
            .len();
        schedule_display_refresh_probe(
            display_id,
            Instant::now(),
            starting_frame_count,
            window,
            cx,
        );
    }

    fn start_fps_benchmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.performance_benchmark_running {
            return;
        }
        self.performance_benchmark_running = true;
        self.performance_stats.benchmark_fps = None;
        cx.notify();

        // Start after the invalidation above has painted. This excludes the
        // caller-to-first-vsync warm-up delay while retaining every measured
        // workload frame.
        cx.on_next_frame(window, |desktop, window, cx| {
            let starting_frame_count = window
                .frame_duration_snapshot()
                .draw_duration_histogram
                .len();
            schedule_fps_benchmark(Instant::now(), starting_frame_count, window, cx);
            desktop.performance_benchmark_running = true;
            cx.notify();
        });
    }

    #[cfg(not(test))]
    fn select_largest_benchmark_session(&mut self, cx: &mut Context<Self>) {
        if let Some((index, _)) = self
            .workspace()
            .sessions
            .iter()
            .enumerate()
            .max_by_key(|(_, session)| session.terminals.len())
        {
            self.workspace_mut().selected_session = index;
            self.active_surface = self.workspace().sessions[index].terminals.first().copied();
            self.sync_surface_presentation(cx);
        }
    }

    #[cfg(not(test))]
    fn start_automated_benchmark(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_largest_benchmark_session(cx);
        let surfaces = self.selected_terminals().to_vec();
        if surfaces.is_empty() {
            eprintln!("desktop renderer benchmark has no active terminal");
            cx.quit();
            return;
        }
        for surface in surfaces {
            self.surfaces[surface].update(cx, |surface, cx| {
                surface.start_output_benchmark(cx);
            });
        }
        self.start_fps_benchmark(window, cx);
    }

    #[cfg(not(test))]
    fn start_automated_idle_benchmark(&mut self, cx: &mut Context<Self>) {
        if !self.automated_idle_benchmark {
            return;
        }
        self.select_largest_benchmark_session(cx);
        cx.spawn(async move |this, cx| {
            // Let startup, the display probe, and the performance sampler settle before
            // recording the quiet-state process sample.
            cx.background_executor()
                .timer(Duration::from_secs(6))
                .await;
            let _ = this.update(cx, |desktop, cx| {
                let startup_ms = desktop.startup_to_first_render_ms.unwrap_or_default();
                let resident_mib = desktop.performance_stats.resident_bytes as f64 / 1_048_576.0;
                println!(
                    "{{\"benchmark\":\"desktop_idle\",\"settle_seconds\":6,\"startup_to_first_render_ms\":{startup_ms:.1},\"desktop_cpu_percent\":{:.2},\"desktop_rss_mib\":{resident_mib:.1},\"visible_surfaces\":{}}}",
                    desktop.performance_stats.cpu_percent,
                    desktop.selected_terminals().len()
                );
                cx.quit();
            });
        })
        .detach();
    }

    #[cfg(not(test))]
    fn start_activity_updates(&self, cx: &mut Context<Self>) {
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let events = match self.control.subscribe_run_activities(None) {
            Ok(events) => events,
            Err(error) => {
                eprintln!("Run Activity subscription failed: {error}");
                return;
            }
        };
        if let Err(error) = thread::Builder::new()
            .name("termi9ne-run-activity".to_owned())
            .spawn(move || {
                while let Ok(activity) = events.recv() {
                    if send.send(activity).is_err() {
                        break;
                    }
                }
            })
        {
            eprintln!("Run Activity update thread failed: {error}");
            return;
        }
        cx.spawn(async move |this, cx| {
            while let Some(activity) = receive.recv().await {
                if this
                    .update(cx, |desktop, cx| {
                        desktop.run_activities.insert(activity.run_id, activity);
                        desktop.refresh_session_statuses();
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    #[cfg(not(test))]
    fn start_terminal_updates(&self, cx: &mut Context<Self>) {
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let events = match self.control.subscribe_terminals() {
            Ok(events) => events,
            Err(error) => {
                eprintln!("terminal index subscription failed: {error}");
                return;
            }
        };
        if let Err(error) = thread::Builder::new()
            .name("termi9ne-terminal-index".to_owned())
            .spawn(move || {
                while let Ok(terminal) = events.recv() {
                    if send.send(terminal).is_err() {
                        break;
                    }
                }
            })
        {
            eprintln!("terminal index update thread failed: {error}");
            return;
        }

        cx.spawn(async move |this, cx| {
            while let Some(terminal) = receive.recv().await {
                if this
                    .update(cx, |desktop, cx| {
                        if terminal.archived {
                            if let Some(archived) = desktop
                                .archived_terminals
                                .iter_mut()
                                .find(|archived| archived.session_id == terminal.session_id)
                            {
                                *archived = terminal.clone();
                            } else {
                                desktop.archived_terminals.push(terminal.clone());
                                desktop
                                    .archived_terminals
                                    .sort_by_key(|terminal| terminal.session_id.to_string());
                            }
                            desktop
                                .pending_terminal_updates
                                .retain(|pending| pending.session_id != terminal.session_id);
                            if let Some(index) = desktop
                                .surface_sessions
                                .iter()
                                .position(|session_id| *session_id == terminal.session_id)
                            {
                                desktop.archive_surface(index);
                            }
                            cx.notify();
                            return;
                        }
                        desktop
                            .archived_terminals
                            .retain(|archived| archived.session_id != terminal.session_id);
                        if let Some(index) = desktop
                            .surface_sessions
                            .iter()
                            .position(|session_id| *session_id == terminal.session_id)
                        {
                            desktop.surface_foreground_processes[index] =
                                terminal.foreground_process.clone();
                            if let Some(status) = desktop.surface_statuses.get_mut(index) {
                                *status = terminal.status;
                            }
                            if terminal.status == TerminalSessionStatus::Running
                                && let Some(surface) = desktop.surfaces.get(index).cloned()
                            {
                                let controller_surface_id = terminal.controller_surface_id;
                                surface.update(cx, |surface, cx| {
                                    surface.synchronize_control_owner(controller_surface_id, cx);
                                });
                            }
                            desktop.refresh_session_statuses();
                            if !desktop.surface_is_represented(index) {
                                desktop.pending_terminal_updates.push(terminal.clone());
                            }
                        } else if let Some(pending) = desktop
                            .pending_terminal_updates
                            .iter_mut()
                            .find(|pending| pending.session_id == terminal.session_id)
                        {
                            *pending = terminal;
                        } else {
                            desktop.pending_terminal_updates.push(terminal);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
                let window_handle = this
                    .read_with(cx, |desktop, _| desktop.window_handle)
                    .ok()
                    .flatten();
                if let Some(window_handle) = window_handle {
                    let _ = window_handle.update(cx, |desktop, window, cx| {
                        desktop.attach_pending_terminals(window, cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    #[cfg(not(test))]
    fn reconcile_session_group_index(&mut self) {
        let Ok(groups) = self.control.list_session_groups(None) else {
            return;
        };
        for group in groups {
            self.apply_session_group(group);
        }
    }

    #[cfg(not(test))]
    fn start_session_group_updates(&self, cx: &mut Context<Self>) {
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let events = match self.control.subscribe_session_groups(None) {
            Ok(events) => events,
            Err(error) => {
                eprintln!("Session group subscription failed: {error}");
                return;
            }
        };
        if let Err(error) = thread::Builder::new()
            .name("termi9ne-session-groups".to_owned())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    if send.send(event).is_err() {
                        break;
                    }
                }
            })
        {
            eprintln!("Session group update thread failed: {error}");
            return;
        }
        cx.spawn(async move |this, cx| {
            while let Some(event) = receive.recv().await {
                let mut events = vec![event];
                while let Ok(event) = receive.try_recv() {
                    events.push(event);
                }
                if this
                    .update(cx, |desktop, cx| {
                        for event in events {
                            match event {
                                termi9ne_protocol::SessionGroupEvent::GroupChanged { group } => {
                                    desktop.apply_session_group(group);
                                }
                                termi9ne_protocol::SessionGroupEvent::GroupDeleted { group_id } => {
                                    desktop
                                        .detached_groups
                                        .retain(|group| group.group_id != group_id);
                                    for tab in desktop.workspaces.tabs_mut() {
                                        if let Some(session) = tab
                                            .content_mut()
                                            .sessions
                                            .iter_mut()
                                            .find(|session| session.group_id == group_id)
                                        {
                                            session.group_version = 0;
                                        }
                                    }
                                }
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    #[cfg(not(test))]
    fn apply_session_group(&mut self, group: SessionGroupSummary) {
        if group.detached {
            if let Some(existing) = self
                .detached_groups
                .iter_mut()
                .find(|existing| existing.group_id == group.group_id)
            {
                *existing = group.clone();
            } else {
                self.detached_groups.push(group.clone());
                self.detached_groups
                    .sort_by_key(|group| (group.position, group.group_id));
            }
            for tab in self.workspaces.tabs_mut() {
                let workspace = tab.content_mut();
                if let Some(index) = workspace
                    .sessions
                    .iter()
                    .position(|session| session.group_id == group.group_id)
                {
                    workspace.sessions.remove(index);
                    if workspace.sessions.is_empty() {
                        workspace.sessions.push(SessionView {
                            group_id: SessionGroupId::new(),
                            group_version: 0,
                            pinned: false,
                            name: "mission history".to_owned(),
                            actor: String::new(),
                            status: SessionStatus::Closed,
                            terminals: Vec::new(),
                        });
                    }
                    workspace.selected_session = workspace
                        .selected_session
                        .min(workspace.sessions.len().saturating_sub(1));
                }
            }
            return;
        }
        self.detached_groups
            .retain(|detached| detached.group_id != group.group_id);
        let surface_by_session = self
            .surface_sessions
            .iter()
            .enumerate()
            .map(|(index, session_id)| (*session_id, index))
            .collect::<HashMap<_, _>>();
        let terminals = group
            .session_ids
            .iter()
            .filter_map(|session_id| surface_by_session.get(session_id).copied())
            .collect::<Vec<_>>();
        for tab in self.workspaces.tabs_mut() {
            let Some(index) = tab.content().sessions.iter().position(|session| {
                session.group_id == group.group_id
                    || (session.group_version == 0
                        && !terminals.is_empty()
                        && session.terminals == terminals)
            }) else {
                continue;
            };
            let workspace = tab.content_mut();
            if group.version < workspace.sessions[index].group_version {
                return;
            }
            let session = &mut workspace.sessions[index];
            session.group_id = group.group_id;
            session.name = group.name;
            session.group_version = group.version;
            session.pinned = group.pinned;
            session.terminals = terminals;
            let selected_group = workspace
                .sessions
                .get(workspace.selected_session)
                .map(|session| session.group_id);
            let moved = workspace.sessions.remove(index);
            let target = (group.position as usize).min(workspace.sessions.len());
            workspace.sessions.insert(target, moved);
            if let Some(selected_group) = selected_group {
                workspace.selected_session = workspace
                    .sessions
                    .iter()
                    .position(|session| session.group_id == selected_group)
                    .unwrap_or(0);
            }
            return;
        }

        let target_id = group
            .mission_id
            .and_then(|mission_id| {
                self.workspaces
                    .tabs()
                    .iter()
                    .find(|tab| tab.content().mission_id == Some(mission_id))
                    .map(|tab| tab.id())
            })
            .unwrap_or_else(|| self.workspaces.active_id());
        let status =
            aggregate_status_from_state(&terminals, &self.surface_statuses, &self.surface_activity);
        if let Some(tab) = self.workspaces.tab_mut(target_id) {
            let workspace = tab.content_mut();
            workspace.sessions.retain(|session| {
                !(session.group_version == 0
                    && session.terminals.is_empty()
                    && session.name == "mission history")
            });
            let target = (group.position as usize).min(workspace.sessions.len());
            workspace.sessions.insert(
                target,
                SessionView {
                    group_id: group.group_id,
                    group_version: group.version,
                    pinned: group.pinned,
                    name: group.name,
                    actor: "you".to_owned(),
                    status,
                    terminals,
                },
            );
            if target <= workspace.selected_session && workspace.sessions.len() > 1 {
                workspace.selected_session += 1;
            }
        }
    }

    #[cfg(not(test))]
    fn attach_pending_terminals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = std::mem::take(&mut self.pending_terminal_updates);
        let mut changed = false;
        for terminal in pending {
            if let Some(index) = self
                .surface_sessions
                .iter()
                .position(|session_id| *session_id == terminal.session_id)
            {
                self.surface_foreground_processes[index] = terminal.foreground_process.clone();
                if let Some(status) = self.surface_statuses.get_mut(index) {
                    *status = terminal.status;
                }
                if let Some(mission) = self.surface_missions.get_mut(index) {
                    *mission = terminal.mission_id;
                }
                if let Some(run) = self.surface_runs.get_mut(index) {
                    *run = terminal.run_id;
                }
                if !self.surface_is_represented(index) {
                    if self.attach_discovered_surface(
                        index,
                        terminal.session_id,
                        terminal.status,
                        terminal.mission_id,
                    ) {
                        changed = true;
                    } else {
                        self.pending_terminal_updates.push(terminal);
                    }
                }
                continue;
            }

            let session_id = terminal.session_id;
            let session = self.control.terminal(session_id);
            let id = format!("terminal-{}", short_id(session_id));
            let surface = cx.new(|surface_cx| {
                TerminalSurface::live(
                    &id,
                    session,
                    terminal.status != TerminalSessionStatus::Running,
                    window,
                    surface_cx,
                )
                .expect("discovered daemon terminal must attach")
            });
            let index = self.surfaces.len();
            self.surface_subscriptions.push(cx.subscribe(
                &surface,
                move |desktop, _, event: &TerminalSurfaceEvent, cx| {
                    desktop.handle_surface_event(index, event, cx);
                },
            ));
            self.surfaces.push(surface);
            self.surface_foreground_processes
                .push(terminal.foreground_process.clone());
            self.surface_sessions.push(session_id);
            self.surface_missions.push(terminal.mission_id);
            self.surface_runs.push(terminal.run_id);
            self.surface_statuses.push(terminal.status);
            if !self.attach_discovered_surface(
                index,
                session_id,
                terminal.status,
                terminal.mission_id,
            ) {
                self.pending_terminal_updates.push(terminal);
                self.surfaces.pop();
                self.surface_foreground_processes.pop();
                self.surface_sessions.pop();
                self.surface_missions.pop();
                self.surface_runs.pop();
                self.surface_statuses.pop();
                self.surface_subscriptions.pop();
                continue;
            }
            changed = true;
        }
        if changed {
            self.sync_surface_presentation(cx);
            self.reconcile_domain_sessions();
            self.refresh_session_statuses();
            self.persist_workspaces();
        }
    }

    #[cfg(not(test))]
    fn attach_discovered_surface(
        &mut self,
        surface_index: usize,
        session_id: SessionId,
        status: TerminalSessionStatus,
        bound_mission_id: Option<MissionId>,
    ) -> bool {
        let target = self.workspaces.tabs().iter().find_map(|tab| {
            if bound_mission_id.is_some() && tab.content().mission_id != bound_mission_id {
                return None;
            }
            let domain_session = tab.content().mission.as_ref()?.sessions.get(&session_id)?;
            Some((
                tab.id(),
                domain_session.name.clone(),
                domain_session.controller.id.to_string(),
            ))
        });

        if let Some((workspace_id, name, actor)) = target
            && let Some(tab) = self.workspaces.tab_mut(workspace_id)
        {
            let workspace = tab.content_mut();
            workspace.sessions.retain(|session| {
                !session.terminals.is_empty() || session.name != "mission history"
            });
            if let Some(session) = workspace
                .sessions
                .iter_mut()
                .find(|session| session.name == name && session.terminals.is_empty())
            {
                session.terminals.push(surface_index);
                session.status = session_status(status);
            } else {
                workspace.sessions.push(SessionView {
                    group_id: SessionGroupId::new(),
                    group_version: 0,
                    pinned: false,
                    name,
                    actor,
                    status: session_status(status),
                    terminals: vec![surface_index],
                });
            }
            return true;
        }

        if bound_mission_id.is_some() {
            return false;
        }

        let workspace = self.workspace_mut();
        workspace.sessions.push(SessionView {
            group_id: SessionGroupId::new(),
            group_version: 0,
            pinned: false,
            name: format!("shell-{}", short_id(session_id)),
            actor: "external".to_owned(),
            status: session_status(status),
            terminals: vec![surface_index],
        });
        true
    }

    #[cfg(not(test))]
    fn surface_is_represented(&self, surface_index: usize) -> bool {
        self.workspaces.tabs().iter().any(|tab| {
            tab.content()
                .sessions
                .iter()
                .any(|session| session.terminals.contains(&surface_index))
        })
    }

    #[cfg(not(test))]
    fn archive_surface(&mut self, surface_index: usize) {
        for tab in self.workspaces.tabs_mut() {
            let workspace = tab.content_mut();
            for session in &mut workspace.sessions {
                session.terminals.retain(|index| *index != surface_index);
            }
            workspace.sessions.retain(|session| {
                !session.terminals.is_empty() || session.name == "mission history"
            });
            if workspace.sessions.is_empty() {
                workspace.sessions.push(SessionView {
                    group_id: SessionGroupId::new(),
                    group_version: 0,
                    pinned: false,
                    name: "mission history".to_owned(),
                    actor: String::new(),
                    status: SessionStatus::Closed,
                    terminals: Vec::new(),
                });
            }
            workspace.selected_session = workspace
                .selected_session
                .min(workspace.sessions.len().saturating_sub(1));
        }
        if self.active_surface == Some(surface_index) {
            self.active_surface = self
                .workspace()
                .sessions
                .get(self.workspace().selected_session)
                .and_then(|session| session.terminals.first())
                .copied();
        }
        self.persist_workspaces();
    }

    fn archive_session(&mut self, session_index: usize, cx: &mut Context<Self>) {
        if self.shared_mode {
            return;
        }
        #[cfg(not(test))]
        {
            let terminals = self
                .workspace()
                .sessions
                .get(session_index)
                .map(|session| session.terminals.clone())
                .unwrap_or_default();
            for surface_index in terminals {
                let Some(session_id) = self.surface_sessions.get(surface_index).copied() else {
                    continue;
                };
                if let Err(error) = self.control.terminal(session_id).archive() {
                    eprintln!("failed to archive terminal Session {session_id}: {error}");
                }
            }
        }
        #[cfg(test)]
        let _ = session_index;
        cx.notify();
    }

    #[cfg(not(test))]
    fn restore_archived_terminal(&mut self, session_id: SessionId, cx: &mut Context<Self>) {
        if self.shared_mode {
            return;
        }
        if let Err(error) = self.control.terminal(session_id).restore() {
            eprintln!("failed to restore terminal Session {session_id}: {error}");
        }
        cx.notify();
    }

    #[cfg(not(test))]
    fn restore_detached_group(
        &self,
        group_id: SessionGroupId,
        version: u64,
        cx: &mut Context<Self>,
    ) {
        let control = self.control.clone();
        let request = cx.background_executor().spawn(async move {
            control.update_session_group(
                group_id,
                version,
                SessionGroupChange::SetDetached { detached: false },
            )
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                match result {
                    Ok(group) => desktop.apply_session_group(group),
                    Err(error) => eprintln!("Session group reattach failed: {error}"),
                }
                desktop.persist_workspaces();
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(not(test))]
    fn toggle_archive_drawer(&mut self, cx: &mut Context<Self>) {
        self.archive_drawer_open = !self.archive_drawer_open;
        cx.notify();
    }

    #[cfg(not(test))]
    fn start_mission_updates(&self, cx: &mut Context<Self>) {
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let events = match self.control.subscribe_missions() {
            Ok(events) => events,
            Err(error) => {
                eprintln!("mission subscription failed: {error}");
                return;
            }
        };
        if let Err(error) = thread::Builder::new()
            .name("termi9ne-mission-updates".to_owned())
            .spawn(move || {
                while let Ok(mission) = events.recv() {
                    if send.send(mission).is_err() {
                        break;
                    }
                }
            })
        {
            eprintln!("mission update thread failed: {error}");
            return;
        }

        cx.spawn(async move |this, cx| {
            while let Some(mission) = receive.recv().await {
                if this
                    .update(cx, |desktop, cx| {
                        let mut changed = false;
                        let mut represented = false;
                        for tab in desktop.workspaces.tabs_mut() {
                            let Some(mission_id) = tab.content().mission_id else {
                                continue;
                            };
                            if mission_id != mission.id {
                                continue;
                            }
                            represented = true;
                            if tab.content().mission.as_ref().map(|value| value.version)
                                != Some(mission.version)
                            {
                                tab.content_mut().mission = Some(mission.clone());
                                changed = true;
                            }
                        }
                        if !represented && mission.status == termi9ne_core::MissionStatus::Active {
                            let title = mission.intent.clone();
                            let content = desktop.workspace_view_for_mission(mission.clone());
                            desktop.workspaces.add_background(title, content);
                            desktop.persist_workspaces();
                            changed = true;
                        }
                        if changed {
                            if desktop.graph_inspector_open
                                && desktop.workspace().mission_id == Some(mission.id)
                            {
                                desktop.refresh_mission_history();
                            }
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                let window_handle = this
                    .read_with(cx, |desktop, _| desktop.window_handle)
                    .ok()
                    .flatten();
                if let Some(window_handle) = window_handle {
                    let _ = window_handle.update(cx, |desktop, window, cx| {
                        desktop.attach_pending_terminals(window, cx);
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    #[cfg(not(test))]
    fn workspace_view_for_mission(&self, mission: Mission) -> WorkspaceView {
        let mut domain_sessions = mission.sessions.values().collect::<Vec<_>>();
        domain_sessions.sort_by_key(|session| session.id);
        let mut sessions = domain_sessions
            .into_iter()
            .map(|session| {
                let terminals = self
                    .surface_sessions
                    .iter()
                    .enumerate()
                    .filter_map(|(index, id)| (*id == session.id).then_some(index))
                    .collect::<Vec<_>>();
                let status = if terminals.is_empty() || session.status.is_finished() {
                    SessionStatus::Closed
                } else {
                    aggregate_status_from_state(
                        &terminals,
                        &self.surface_statuses,
                        &self.surface_activity,
                    )
                };
                SessionView {
                    group_id: SessionGroupId::new(),
                    group_version: 0,
                    pinned: false,
                    name: session.name.clone(),
                    actor: session.controller.id.to_string(),
                    status,
                    terminals,
                }
            })
            .collect::<Vec<_>>();
        if sessions.is_empty() {
            sessions.push(SessionView {
                group_id: SessionGroupId::new(),
                group_version: 0,
                pinned: false,
                name: "mission history".to_owned(),
                actor: String::new(),
                status: SessionStatus::Closed,
                terminals: Vec::new(),
            });
        }
        WorkspaceView {
            sessions,
            selected_session: 0,
            focus_mode: false,
            mission_id: Some(mission.id),
            mission: Some(mission),
        }
    }

    #[cfg(not(test))]
    fn handle_surface_event(
        &mut self,
        surface_index: usize,
        event: &TerminalSurfaceEvent,
        cx: &mut Context<Self>,
    ) {
        let status = match event {
            TerminalSurfaceEvent::Activity => {
                self.activity_generation = self.activity_generation.saturating_add(1);
                let generation = self.activity_generation;
                let was_active = self
                    .surface_activity
                    .insert(surface_index, generation)
                    .is_some();
                if was_active {
                    return;
                }
                if let Some(status) = self.surface_statuses.get_mut(surface_index) {
                    *status = TerminalSessionStatus::Running;
                }
                self.refresh_session_statuses();
                if self.refresh_shell_session_identity(surface_index, cx) {
                    self.persist_workspaces();
                }

                cx.spawn(async move |this, cx| {
                    let mut observed_generation = generation;
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(900))
                            .await;
                        let result = this.update(cx, |desktop, cx| {
                            let current = desktop.surface_activity.get(&surface_index).copied()?;
                            if current == observed_generation {
                                desktop.surface_activity.remove(&surface_index);
                                desktop.refresh_session_statuses();
                                cx.notify();
                            }
                            Some(current)
                        });
                        let Ok(Some(current_generation)) = result else {
                            break;
                        };
                        if current_generation == observed_generation {
                            break;
                        }
                        observed_generation = current_generation;
                    }
                })
                .detach();
                cx.notify();
                return;
            }
            TerminalSurfaceEvent::Focused => {
                self.activate_focused_surface(surface_index, cx);
                return;
            }
            TerminalSurfaceEvent::RestartRequested => {
                self.pending_terminal_restart = Some(surface_index);
                cx.notify();
                return;
            }
            TerminalSurfaceEvent::CloseRequested => {
                self.close_terminal(surface_index, cx);
                return;
            }
            TerminalSurfaceEvent::Exited { success: true } => SessionStatus::Closed,
            TerminalSurfaceEvent::Exited { success: false } | TerminalSurfaceEvent::Failed => {
                SessionStatus::Terminated
            }
        };
        if !matches!(event, TerminalSurfaceEvent::Activity) {
            self.surface_activity.remove(&surface_index);
            if let Some(surface_status) = self.surface_statuses.get_mut(surface_index) {
                *surface_status = match status {
                    SessionStatus::Terminated => TerminalSessionStatus::Failed,
                    SessionStatus::Closed => TerminalSessionStatus::Exited,
                    SessionStatus::Working | SessionStatus::Waiting | SessionStatus::Idle => {
                        TerminalSessionStatus::Running
                    }
                };
            }
            self.refresh_session_statuses();
            self.finish_domain_session(
                surface_index,
                matches!(event, TerminalSurfaceEvent::Exited { success: true }),
            );
        }
        self.persist_workspaces();
        cx.notify();
    }

    #[cfg(not(test))]
    fn refresh_session_statuses(&mut self) {
        let surface_statuses = &self.surface_statuses;
        let surface_activity = &self.surface_activity;
        let surface_runs = &self.surface_runs;
        let run_activities = &self.run_activities;
        for tab in self.workspaces.tabs_mut() {
            for session in &mut tab.content_mut().sessions {
                let explicit_activity = session.terminals.iter().filter_map(|index| {
                    let run_id = surface_runs.get(*index).copied().flatten()?;
                    run_activities.get(&run_id)
                });
                let mut provider_working = false;
                let mut provider_waiting = false;
                let mut provider_not_working = false;
                for activity in explicit_activity {
                    match activity.state {
                        RunActivityState::Working => provider_working = true,
                        RunActivityState::WaitingInput
                        | RunActivityState::WaitingApproval
                        | RunActivityState::Blocked
                            if activity.source != RunActivitySource::None =>
                        {
                            provider_waiting = true;
                        }
                        RunActivityState::Idle if activity.source != RunActivitySource::None => {
                            provider_not_working = true;
                        }
                        RunActivityState::Pending
                        | RunActivityState::Paused
                        | RunActivityState::Completed
                        | RunActivityState::Failed
                        | RunActivityState::Cancelled
                        | RunActivityState::Unknown
                        | RunActivityState::Idle
                        | RunActivityState::WaitingInput
                        | RunActivityState::WaitingApproval
                        | RunActivityState::Blocked => {}
                    }
                }
                session.status = if provider_working {
                    SessionStatus::Working
                } else if provider_waiting {
                    SessionStatus::Waiting
                } else if provider_not_working {
                    SessionStatus::Idle
                } else if session
                    .terminals
                    .iter()
                    .any(|index| surface_activity.contains_key(index))
                {
                    SessionStatus::Working
                } else if session.terminals.iter().any(|index| {
                    surface_statuses.get(*index) == Some(&TerminalSessionStatus::Running)
                }) {
                    SessionStatus::Idle
                } else if session.terminals.iter().any(|index| {
                    surface_statuses.get(*index) == Some(&TerminalSessionStatus::Failed)
                }) {
                    SessionStatus::Terminated
                } else {
                    SessionStatus::Closed
                };
            }
        }
    }

    #[cfg(not(test))]
    fn refresh_shell_session_identity(&mut self, surface_index: usize, cx: &App) -> bool {
        let Some(surface) = self.surfaces.get(surface_index) else {
            return false;
        };
        let (title, directory) = {
            let surface = surface.read(cx);
            (
                surface.terminal_title().map(str::to_owned),
                surface.current_directory().map(str::to_owned),
            )
        };
        // The working directory is a stable identity; the title changes with
        // every foreground program, so it is only a fallback.
        let suggested = directory
            .and_then(|directory| {
                directory
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .or(title);
        let Some(suggested) = suggested else {
            return false;
        };
        let mut changed = false;
        for workspace in self.workspaces.tabs_mut() {
            for session in &mut workspace.content_mut().sessions {
                let generated_name = session.name == "shell" || session.name.starts_with("shell-");
                if generated_name
                    && session.terminals.contains(&surface_index)
                    && session.name != suggested
                {
                    session.name.clone_from(&suggested);
                    changed = true;
                }
            }
        }
        changed
    }

    #[cfg(not(test))]
    fn ensure_workspace_missions(&mut self) {
        let missing = self
            .workspaces
            .tabs()
            .iter()
            .filter(|tab| tab.content().mission.is_none())
            .map(|tab| (tab.id(), tab.title().to_owned()))
            .collect::<Vec<_>>();

        for (workspace_id, intent) in missing {
            let Some(mission) = resolve_mission(&self.control, None, &intent) else {
                continue;
            };
            if let Some(tab) = self.workspaces.tab_mut(workspace_id) {
                tab.content_mut().mission_id = Some(mission.id);
                tab.content_mut().mission = Some(mission);
            }
        }
    }

    #[cfg(not(test))]
    fn reconcile_domain_sessions(&mut self) {
        let mut jobs = Vec::new();
        for tab in self.workspaces.tabs() {
            let Some(mission) = tab.content().mission.as_ref() else {
                continue;
            };
            for session in &tab.content().sessions {
                for (offset, index) in session.terminals.iter().enumerate() {
                    let Some(session_id) = self.surface_sessions.get(*index).copied() else {
                        continue;
                    };
                    if self
                        .surface_missions
                        .get(*index)
                        .copied()
                        .flatten()
                        .is_some()
                    {
                        continue;
                    }
                    if mission.sessions.contains_key(&session_id) {
                        continue;
                    }
                    let name = if offset == 0 {
                        session.name.clone()
                    } else {
                        format!("{}:{}", session.name, offset + 1)
                    };
                    jobs.push((tab.id(), mission.id, session_id, name));
                }
            }
        }

        for (workspace_id, mission_id, session_id, name) in jobs {
            let actor = Actor::human("you").expect("built-in human actor must be valid");
            match self.control.dispatch(
                mission_id,
                Command::StartSession {
                    session_id,
                    name,
                    started_by: actor,
                },
            ) {
                Ok((_, mission)) => {
                    if let Some(tab) = self.workspaces.tab_mut(workspace_id) {
                        tab.content_mut().mission = Some(mission);
                    }
                }
                Err(error) => {
                    eprintln!("mission session reconciliation failed for {session_id}: {error}");
                }
            }
        }
    }

    #[cfg(not(test))]
    fn finish_domain_session(&mut self, surface_index: usize, success: bool) {
        let Some(session_id) = self.surface_sessions.get(surface_index).copied() else {
            return;
        };
        let missions = self
            .workspaces
            .tabs()
            .iter()
            .filter_map(|tab| {
                let mission = tab.content().mission.as_ref()?;
                mission
                    .sessions
                    .get(&session_id)
                    .filter(|session| !session.status.is_finished())
                    .map(|_| (tab.id(), mission.id))
            })
            .collect::<Vec<_>>();
        for (workspace_id, mission_id) in missions {
            let exit_code = Some(i32::from(!success));
            if let Ok((_, mission)) = self.control.dispatch(
                mission_id,
                Command::FinishSession {
                    session_id,
                    exit_code,
                },
            ) && let Some(tab) = self.workspaces.tab_mut(workspace_id)
            {
                tab.content_mut().mission = Some(mission);
            }
        }
    }

    #[cfg(not(test))]
    fn resolve_selected_attention(&mut self, response: &str, cx: &mut Context<Self>) {
        let Some(signal_id) = self.selected_attention else {
            return;
        };
        let Some(mission_id) = self.workspace().mission_id else {
            return;
        };
        let expected_version = self
            .workspace()
            .mission
            .as_ref()
            .map(|mission| mission.version);
        let by = ActorId::new("you").expect("built-in human actor must be valid");
        match self.control.dispatch_at(
            mission_id,
            expected_version,
            Command::ResolveSignal {
                signal_id,
                by,
                response: response.to_owned(),
            },
        ) {
            Ok((_, mission)) => {
                self.workspace_mut().mission = Some(mission);
                self.selected_attention = None;
                self.persist_workspaces();
                cx.notify();
            }
            Err(error) => eprintln!("attention resolution failed: {error}"),
        }
    }

    #[cfg(not(test))]
    fn decide_selected_approval(&mut self, allow: bool, cx: &mut Context<Self>) {
        let Some(signal_id) = self.selected_attention else {
            return;
        };
        let Some(mission_id) = self.workspace().mission_id else {
            return;
        };
        let expected_version = self
            .workspace()
            .mission
            .as_ref()
            .map(|mission| mission.version);
        let by = ActorId::new("you").expect("built-in human actor must be valid");
        match self.control.dispatch_at(
            mission_id,
            expected_version,
            Command::DecideApproval {
                signal_id,
                grant_id: allow.then(GrantId::new),
                by,
                allow,
                resource_scope: String::new(),
                use_count: 1,
                enforcement: GrantEnforcement::Cooperative,
            },
        ) {
            Ok((_, mission)) => {
                self.workspace_mut().mission = Some(mission);
                self.selected_attention = None;
                self.persist_workspaces();
                cx.notify();
            }
            Err(error) => eprintln!("approval decision failed: {error}"),
        }
    }

    #[cfg(not(test))]
    fn dispatch_active_mission(&mut self, command: Command, cx: &mut Context<Self>) {
        let Some(mission_id) = self.workspace().mission_id else {
            return;
        };
        let expected_version = self
            .workspace()
            .mission
            .as_ref()
            .map(|mission| mission.version);
        match self
            .control
            .dispatch_at(mission_id, expected_version, command)
        {
            Ok((_, mission)) => {
                self.workspace_mut().mission = Some(mission);
                if self.graph_inspector_open {
                    self.refresh_mission_history();
                }
                self.persist_workspaces();
                cx.notify();
            }
            Err(error) => eprintln!("mission command failed: {error}"),
        }
    }

    #[cfg(not(test))]
    fn settle_selected_review(&mut self, accepted: bool, cx: &mut Context<Self>) {
        let Some(run_id) = self.selected_graph_run else {
            return;
        };
        let note = if accepted {
            let note = self.review_note.trim();
            if note.is_empty() {
                "Accepted after reviewing candidate and evidence".to_owned()
            } else {
                note.to_owned()
            }
        } else {
            let note = self.review_note.trim();
            if note.is_empty() {
                return;
            }
            note.to_owned()
        };
        let by = ActorId::new("you").expect("built-in human actor must be valid");
        let command = if accepted {
            Command::AcceptRunResult { run_id, by, note }
        } else {
            Command::RejectRunResult { run_id, by, note }
        };
        self.dispatch_active_mission(command, cx);
        self.review_note.clear();
    }

    #[cfg(not(test))]
    fn create_delivery_run(&mut self, purpose: DeliveryRunPurpose, cx: &mut Context<Self>) {
        if self.shared_mode || self.review_action_busy {
            return;
        }
        let Some(source_run_id) = self.selected_graph_run else {
            return;
        };
        let Some(mission) = self.workspace().mission.as_ref() else {
            return;
        };
        let Some(source) = mission.runs.get(&source_run_id) else {
            return;
        };
        let termi9ne_core::ActorKind::Agent { engine } = &source.actor.kind else {
            self.review_action_status = Some((false, "This workflow requires an agent Run".into()));
            cx.notify();
            return;
        };
        let Some(candidate) = mission.verified_delivery.candidates.get(&source_run_id) else {
            self.review_action_status = Some((false, "Publish a Candidate first".into()));
            cx.notify();
            return;
        };
        let mission_id = mission.id;
        let expected_version = Some(mission.version);
        let new_run_id = RunId::new();
        let actor_prefix = match purpose {
            DeliveryRunPurpose::Verification => "verifier",
            DeliveryRunPurpose::Retry => "retry",
        };
        let actor = match Actor::agent(
            format!("{actor_prefix}-{}", short_id(new_run_id)),
            engine.clone(),
        ) {
            Ok(actor) => actor,
            Err(error) => {
                self.review_action_status = Some((false, error.to_string()));
                cx.notify();
                return;
            }
        };
        let command = match purpose {
            DeliveryRunPurpose::Verification => Command::CreateVerifierRun {
                subject_run_id: source_run_id,
                verifier_run_id: new_run_id,
                actor,
                priority: termi9ne_core::RunPriority::Urgent,
            },
            DeliveryRunPurpose::Retry => Command::RetryReturnedRun {
                source_run_id,
                retry_run_id: new_run_id,
                actor,
                priority: termi9ne_core::RunPriority::Urgent,
            },
        };
        let base_revision = candidate.revision.clone();
        let session_id = SessionId::new();
        let session_name = format!("{actor_prefix}-{}", short_id(new_run_id));
        let control = self.control.clone();
        self.review_action_busy = true;
        self.review_action_status = Some((
            true,
            match purpose {
                DeliveryRunPurpose::Verification => "Preparing verifier…".to_owned(),
                DeliveryRunPurpose::Retry => "Preparing returned-work retry…".to_owned(),
            },
        ));
        let request = cx.background_executor().spawn(async move {
            let source_checkout = control
                .list_run_checkouts(Some(mission_id))
                .map_err(|error| error.to_string())?
                .into_iter()
                .find(|checkout| checkout.run_id == source_run_id)
                .ok_or_else(|| "The source Run has no managed checkout".to_owned())?;
            let (_, mission) = control
                .dispatch_at(mission_id, expected_version, command)
                .map_err(|error| error.to_string())?;
            if let Err(error) = control.prepare_run_checkout(
                mission_id,
                new_run_id,
                source_checkout.repository_root,
                base_revision,
            ) {
                return Ok::<_, String>((mission, Err(error.to_string())));
            }
            let grid = GridSize::new(120, 36).expect("delivery Run grid is within bounds");
            match control.launch_configured_agent_run_in_checkout(
                mission_id,
                new_run_id,
                session_id,
                session_name,
                grid,
            ) {
                Ok((_, launched, _)) => Ok((launched, Ok(()))),
                Err(error) => Ok((mission, Err(error.to_string()))),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.review_action_busy = false;
                match result {
                    Ok((mission, Ok(()))) => {
                        desktop.workspace_mut().mission = Some(mission);
                        desktop.selected_graph_run = Some(new_run_id);
                        desktop.review_action_status = Some((
                            true,
                            match purpose {
                                DeliveryRunPurpose::Verification => "Verifier running".to_owned(),
                                DeliveryRunPurpose::Retry => "Retry running".to_owned(),
                            },
                        ));
                        desktop.refresh_mission_history();
                        desktop.persist_workspaces();
                    }
                    Ok((mission, Err(error))) => {
                        desktop.workspace_mut().mission = Some(mission);
                        desktop.selected_graph_run = Some(new_run_id);
                        desktop.review_action_status = Some((
                            false,
                            format!("Run created, but launch preparation failed: {error}"),
                        ));
                    }
                    Err(error) => {
                        desktop.review_action_status = Some((false, error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    #[cfg(not(test))]
    fn handle_review_note_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "backspace" => {
                self.review_note.pop();
            }
            "escape" => self.review_note.clear(),
            _ if !event.keystroke.modifiers.control && !event.keystroke.modifiers.platform => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                    && self.review_note.len() < 4_096
                {
                    self.review_note.push_str(text);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    #[cfg(not(test))]
    fn open_graph_inspector(&mut self, cx: &mut Context<Self>) {
        self.graph_inspector_open = true;
        self.command_deck_open = false;
        self.selected_attention = None;
        if self.selected_graph_run.is_none() {
            self.selected_graph_run = self.workspace().mission.as_ref().and_then(|mission| {
                let mut runs = mission.runs.keys().copied().collect::<Vec<_>>();
                runs.sort();
                runs.first().copied()
            });
        }
        self.refresh_mission_history();
        cx.notify();
    }

    #[cfg(not(test))]
    fn refresh_mission_history(&mut self) {
        let Some(mission_id) = self.workspace().mission_id else {
            self.mission_history.clear();
            self.mission_history_has_more = false;
            return;
        };
        match self.control.mission_history(mission_id, None, 100) {
            Ok((entries, has_more)) => {
                self.mission_history = entries;
                self.mission_history_has_more = has_more;
            }
            Err(error) => eprintln!("mission history failed for {mission_id}: {error}"),
        }
    }

    #[cfg(not(test))]
    fn graph_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.graph_inspector_open {
            return None;
        }
        let mission = self.workspace().mission.as_ref()?;
        let mut runs = mission.runs.values().collect::<Vec<_>>();
        runs.sort_by_key(|run| (run_depth(mission, run.id), run.id));
        let selected_id = self
            .selected_graph_run
            .filter(|id| mission.runs.contains_key(id))
            .or_else(|| runs.first().map(|run| run.id));
        let selected = selected_id.and_then(|id| mission.runs.get(&id));
        let history_count = self.mission_history.len();
        let history_has_more = self.mission_history_has_more;
        let timeline = div()
            .id("mission-history-timeline")
            .h(px(166.0))
            .flex_none()
            .overflow_y_scroll()
            .border_t_1()
            .border_color(rgb(HAIRLINE))
            .bg(rgb(PANEL))
            .p_3()
            .child(
                div()
                    .mb_2()
                    .flex()
                    .items_center()
                    .font_family(UI_FONT)
                    .text_xs()
                    .text_color(rgb(TRACE))
                    .child("OPERATIONAL HISTORY")
                    .child(div().ml_auto().child(format!(
                        "{}{} events",
                        if history_has_more { "latest " } else { "" },
                        history_count
                    ))),
            )
            .children(self.mission_history.iter().rev().take(24).map(|entry| {
                let correlation = entry
                    .correlation_id
                    .map(short_id)
                    .unwrap_or_else(|| "legacy".to_owned());
                let occurred = entry.occurred_at_unix_micros.map_or_else(
                    || "unversioned time".to_owned(),
                    |micros| format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000),
                );
                div()
                    .min_h(px(28.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_t_1()
                    .border_color(rgb(HAIRLINE).alpha(0.5))
                    .font_family(UI_FONT)
                    .text_xs()
                    .child(
                        div()
                            .w(px(42.0))
                            .text_color(rgb(RELAY))
                            .child(format!("#{:03}", entry.mission_sequence)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(rgb(CHALK))
                            .child(entry.payload_type.replace('_', " ")),
                    )
                    .child(
                        div()
                            .text_color(rgb(TRACE))
                            .child(format!("{correlation} · {occurred}")),
                    )
            }));

        let details = if let Some(run) = selected {
            let signal_count = mission
                .signals
                .values()
                .filter(|signal| signal.run_id == run.id)
                .count();
            let unresolved = mission
                .attention_queue()
                .into_iter()
                .filter(|item| item.run_id == run.id)
                .count();
            let mut artifacts = mission
                .artifacts
                .values()
                .filter(|artifact| artifact.run_id == run.id)
                .map(|artifact| {
                    (
                        artifact.name.clone(),
                        artifact.locator.clone(),
                        artifact.digest.clone().map_or_else(
                            || artifact.media_type.clone(),
                            |digest| format!("{} · {digest}", artifact.media_type),
                        ),
                    )
                })
                .collect::<Vec<_>>();
            artifacts.extend(mission.signals.values().filter_map(|signal| {
                if signal.run_id != run.id {
                    return None;
                }
                match &signal.kind {
                    SignalKind::ArtifactReady { name, locator } => {
                        Some((name.clone(), locator.clone(), "legacy signal".to_owned()))
                    }
                    _ => None,
                }
            }));
            let grants = mission
                .grants
                .values()
                .filter(|grant| grant.run_id == run.id)
                .map(|grant| {
                    (
                        short_id(grant.id),
                        grant.operation.clone(),
                        grant.resource_scope.clone(),
                        grant.enforcement,
                        grant.remaining_uses,
                        grant.revoked,
                    )
                })
                .collect::<Vec<_>>();
            let session = run
                .primary_session
                .and_then(|id| mission.sessions.get(&id))
                .map(|session| session.name.clone())
                .unwrap_or_else(|| "headless".to_owned());
            let execution_proof = run.driver_snapshot.as_ref().map(|snapshot| {
                let enforced = snapshot.sandbox_backend.is_some();
                let posture = match (&snapshot.sandbox_profile, &snapshot.sandbox_backend) {
                    (Some(profile), Some(backend)) => format!(
                        "{} · {} · {}",
                        profile.replace('_', " "),
                        backend.replace('_', " "),
                        if snapshot.sandbox_network_isolated {
                            "network isolated"
                        } else {
                            "network inherited"
                        }
                    ),
                    _ => "no OS sandbox attached".to_owned(),
                };
                let fingerprint = snapshot
                    .process_spec_sha256
                    .get(..12)
                    .unwrap_or(&snapshot.process_spec_sha256)
                    .to_owned();
                (enforced, posture, fingerprint)
            });
            let state = if run.status == termi9ne_core::RunStatus::Pending {
                mission
                    .run_readiness(run.id)
                    .map_or_else(|_| "pending".to_owned(), |value| format!("{value:?}"))
            } else {
                format!("{:?}", run.status)
            };
            let dependencies = run
                .dependencies
                .iter()
                .filter_map(|id| {
                    mission.runs.get(id).map(|dependency| {
                        (
                            short_id(*id),
                            dependency.objective.clone(),
                            dependency.status,
                        )
                    })
                })
                .collect::<Vec<_>>();
            let primary_action = match run.phase {
                termi9ne_core::RunPhase::Pending
                    if mission.run_readiness(run.id).ok()
                        == Some(termi9ne_core::RunReadiness::Ready) =>
                {
                    Some(("Start ready run", Command::StartReadyRun { run_id: run.id }))
                }
                termi9ne_core::RunPhase::Running => Some((
                    "Pause run",
                    Command::PauseRun {
                        run_id: run.id,
                        reason: termi9ne_core::PauseReason::Human,
                    },
                )),
                termi9ne_core::RunPhase::Paused if unresolved == 0 => {
                    Some(("Resume run", Command::ResumeRun { run_id: run.id }))
                }
                _ => None,
            };
            let reviewable = run.disposition == termi9ne_core::RunDisposition::AwaitingReview;
            let verification_policy = mission.verified_delivery.verification_policy(run.id);
            let change_intent = mission
                .verified_delivery
                .change_intents
                .get(&run.id)
                .cloned();
            let harness = mission
                .verified_delivery
                .harness_snapshots
                .get(&run.id)
                .cloned();
            let candidate = mission.verified_delivery.candidates.get(&run.id).cloned();
            let mut handoffs = mission
                .verified_delivery
                .handoffs
                .values()
                .filter(|handoff| handoff.run_id == run.id)
                .cloned()
                .collect::<Vec<_>>();
            handoffs.sort_by_key(|handoff| handoff.artifact_id.to_string());
            let handoff = handoffs.pop();
            let mut receipts = mission
                .verified_delivery
                .evaluation_receipts
                .values()
                .filter(|receipt| receipt.subject_run_id == run.id)
                .cloned()
                .collect::<Vec<_>>();
            receipts.sort_by_key(|receipt| receipt.artifact_id.to_string());
            let receipt = receipts.pop();
            let review_note = self.review_note.clone();
            let delivery_input = mission
                .verified_delivery
                .delivery_run_inputs
                .get(&run.id)
                .cloned();
            let has_verifier =
                mission
                    .verified_delivery
                    .delivery_run_inputs
                    .values()
                    .any(|input| {
                        input.source_run_id == run.id
                            && input.purpose == DeliveryRunPurpose::Verification
                    });
            let has_retry = mission
                .verified_delivery
                .delivery_run_inputs
                .values()
                .any(|input| {
                    input.source_run_id == run.id && input.purpose == DeliveryRunPurpose::Retry
                });
            let can_create_verifier = reviewable
                && candidate.is_some()
                && harness.is_some()
                && !has_verifier
                && !self.review_action_busy;
            let can_retry = run.disposition == termi9ne_core::RunDisposition::Rejected
                && candidate
                    .as_ref()
                    .and_then(|candidate| candidate.realized_changes.as_ref())
                    .is_some_and(|manifest| {
                        manifest.changes.iter().any(|change| {
                            change.operation != termi9ne_core::ChangeOperation::Delete
                        })
                    })
                && harness.is_some()
                && change_intent.is_some()
                && !has_retry
                && !self.review_action_busy;
            let review_action_status = self.review_action_status.clone();
            let review_action_busy = self.review_action_busy;

            div()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(RELAY))
                        .child(format!("RUN {}", short_id(run.id))),
                )
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(ui_size(18.0))
                        .child(run.objective.clone()),
                )
                .child(
                    div()
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(TRACE))
                        .child(format!(
                            "{} · {} · {:?} priority · {:?} disposition · {session}",
                            run.actor.id, state, run.priority, run.disposition
                        )),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(metric_chip("signals", signal_count, TRACE))
                        .child(metric_chip("needs you", unresolved, SIGNAL))
                        .child(metric_chip("depends", dependencies.len(), RELAY))
                        .child(metric_chip("grants", grants.len(), SUCCESS))
                        .child(metric_chip("artifacts", artifacts.len(), CHALK)),
                )
                .child(
                    div()
                        .mt_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(rgb(HAIRLINE))
                        .bg(rgb(PANEL).alpha(0.52))
                        .p_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .font_family(UI_FONT)
                                .text_xs()
                                .text_color(rgb(TRACE))
                                .child("VERIFIED DELIVERY")
                                .child(
                                    div()
                                        .ml_auto()
                                        .text_color(rgb(match verification_policy {
                                            termi9ne_core::VerificationPolicy::OwnerReview => TRACE,
                                            termi9ne_core::VerificationPolicy::Independent => {
                                                SUCCESS
                                            }
                                        }))
                                        .child(match verification_policy {
                                            termi9ne_core::VerificationPolicy::OwnerReview => {
                                                "OWNER REVIEW"
                                            }
                                            termi9ne_core::VerificationPolicy::Independent => {
                                                "INDEPENDENT"
                                            }
                                        }),
                                ),
                        )
                        .when_some(change_intent, |section, intent| {
                            let claims = intent
                                .claims
                                .iter()
                                .map(|claim| {
                                    format!(
                                        "{:?} · {:?} · {}",
                                        claim.operation, claim.scope, claim.path
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n");
                            section.child(review_fact(
                                "CHANGE INTENT",
                                format!(
                                    "{:?} · v{} · lease {}\n{} @ {}\n{}",
                                    intent.state,
                                    intent.version,
                                    intent.lease_epoch,
                                    intent.repository_identity,
                                    intent.base_revision,
                                    claims
                                ),
                                if intent.state == termi9ne_core::ChangeIntentState::Admitted {
                                    SUCCESS
                                } else {
                                    TRACE
                                },
                            ))
                        })
                        .when_some(harness, |section, snapshot| {
                            let objective = snapshot
                                .objective_sha256
                                .get(..12)
                                .unwrap_or(&snapshot.objective_sha256);
                            let process = snapshot
                                .driver
                                .process_spec_sha256
                                .get(..12)
                                .unwrap_or(&snapshot.driver.process_spec_sha256);
                            section.child(review_fact(
                                "HARNESS SNAPSHOT",
                                format!(
                                    "{} v{} · objective {} · process {}",
                                    snapshot.driver.driver_id,
                                    snapshot.driver.profile_version,
                                    objective,
                                    process
                                ),
                                RELAY,
                            ))
                        })
                        .when_some(candidate.clone(), |section, candidate| {
                            let revision = candidate
                                .revision
                                .get(..12)
                                .unwrap_or(&candidate.revision);
                            let digest = candidate
                                .content_sha256
                                .get(..12)
                                .unwrap_or(&candidate.content_sha256);
                            let realized = candidate
                                .realized_changes
                                .as_ref()
                                .map_or_else(|| "legacy manifest".to_owned(), |manifest| {
                                    format!(
                                        "{} changed · scanner v{}",
                                        manifest.changes.len(),
                                        manifest.scanner_version
                                    )
                                });
                            section.child(review_fact(
                                "CANDIDATE",
                                format!(
                                    "{} · sha256 {} · lease {} · {} · {} artifact(s) · by {}",
                                    revision,
                                    digest,
                                    candidate
                                        .execution_lease_epoch
                                        .map_or_else(|| "none".to_owned(), |epoch| epoch.to_string()),
                                    realized,
                                    candidate.artifact_ids.len(),
                                    candidate.submitted_by
                                ),
                                SUCCESS,
                            ))
                        })
                        .when_some(
                            candidate
                                .as_ref()
                                .and_then(|candidate| candidate.realized_changes.as_ref()),
                            |section, manifest| {
                                let mut paths = manifest
                                    .changes
                                    .iter()
                                    .take(12)
                                    .map(|change| {
                                        format!("{:?} · {}", change.operation, change.path)
                                    })
                                    .collect::<Vec<_>>();
                                if manifest.changes.len() > paths.len() {
                                    paths.push(format!(
                                        "+{} more",
                                        manifest.changes.len() - paths.len()
                                    ));
                                }
                                let snapshot = manifest
                                    .snapshot_revision
                                    .as_deref()
                                    .and_then(|revision| revision.get(..12))
                                    .map_or_else(
                                        || "LEGACY · mutable checkout view".to_owned(),
                                        |revision| format!("FROZEN · snapshot {revision}"),
                                    );
                                paths.insert(0, snapshot);
                                section.child(review_fact(
                                    "REALIZED CHANGESET",
                                    paths.join("\n"),
                                    RELAY,
                                ))
                            },
                        )
                        .when(candidate.is_none(), |section| {
                            section.child(review_fact(
                                "CANDIDATE",
                                "No immutable candidate submitted".to_owned(),
                                SIGNAL,
                            ))
                        })
                        .when_some(handoff, |section, handoff| {
                            section.child(review_fact(
                                "HANDOFF",
                                format!(
                                    "{}\n{} complete · {} remaining · {} evidence",
                                    handoff.summary,
                                    handoff.completed.len(),
                                    handoff.remaining.len(),
                                    handoff.evidence.len()
                                ),
                                CHALK,
                            ))
                        })
                        .when_some(receipt.clone(), |section, receipt| {
                            let passed = candidate
                                .as_ref()
                                .is_some_and(|candidate| receipt.passes(candidate));
                            section.child(review_fact(
                                "EVALUATION RECEIPT",
                                format!(
                                    "{:?} · {} of {} checks passed · verifier {}\n{}",
                                    receipt.verdict,
                                    receipt.checks.iter().filter(|check| check.passed).count(),
                                    receipt.checks.len(),
                                    short_id(receipt.verifier_run_id),
                                    receipt.summary
                                ),
                                if passed { SUCCESS } else { FAULT },
                            ))
                        })
                        .when(
                            verification_policy == termi9ne_core::VerificationPolicy::Independent
                                && receipt.is_none(),
                            |section| {
                                section.child(review_fact(
                                    "EVALUATION RECEIPT",
                                    "Independent verification is required before acceptance"
                                        .to_owned(),
                                    SIGNAL,
                                ))
                            },
                        )
                        .when_some(delivery_input, |section, input| {
                            let revision = input
                                .candidate
                                .revision
                                .get(..12)
                                .unwrap_or(&input.candidate.revision);
                            let detail = match input.purpose {
                                DeliveryRunPurpose::Verification => format!(
                                    "VERIFY · Candidate {revision} · source {}",
                                    short_id(input.source_run_id)
                                ),
                                DeliveryRunPurpose::Retry => format!(
                                    "RETRY · Candidate {revision} · source {}\n{}",
                                    short_id(input.source_run_id),
                                    input.return_note.as_deref().unwrap_or("Return note missing")
                                ),
                            };
                            section.child(review_fact("FROZEN INPUT", detail, RELAY))
                        }),
                )
                .when_some(execution_proof, |detail, (enforced, posture, fingerprint)| {
                    detail.child(
                        div()
                            .mt_1()
                            .pl_3()
                            .py_1()
                            .border_l_2()
                            .border_color(rgb(if enforced { SUCCESS } else { TRACE }))
                            .flex()
                            .items_center()
                            .gap_3()
                            .font_family(UI_FONT)
                            .text_xs()
                            .child(
                                div()
                                    .text_color(rgb(if enforced { SUCCESS } else { TRACE }))
                                    .child(if enforced {
                                        "FS WRITE ENFORCED"
                                    } else {
                                        "COOPERATIVE PROCESS"
                                    }),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(rgb(CHALK))
                                    .child(posture),
                            )
                            .child(
                                div()
                                    .text_color(rgb(TRACE))
                                    .child(format!("spec {fingerprint}")),
                            ),
                    )
                })
                .when(
                    primary_action.is_some()
                        || (reviewable && !review_action_busy)
                        || can_create_verifier
                        || can_retry,
                    |detail| {
                        detail.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .when_some(primary_action, |actions, (label, command)| {
                                    actions.child(
                                        div()
                                            .id("graph-run-primary-action")
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .rounded(px(4.0))
                                            .bg(rgb(RELAY))
                                            .text_color(rgb(DECK))
                                            .on_click(cx.listener(move |desktop, _, _, cx| {
                                                desktop
                                                    .dispatch_active_mission(command.clone(), cx);
                                            }))
                                            .child(label),
                                    )
                                })
                                .when(can_create_verifier, |actions| {
                                    actions.child(
                                        div()
                                            .id("graph-create-verifier")
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .rounded(px(4.0))
                                            .border_1()
                                            .border_color(rgb(RELAY))
                                            .text_color(rgb(RELAY))
                                            .on_click(cx.listener(|desktop, _, _, cx| {
                                                desktop.create_delivery_run(
                                                    DeliveryRunPurpose::Verification,
                                                    cx,
                                                );
                                            }))
                                            .child("Create verifier"),
                                    )
                                })
                                .when(can_retry, |actions| {
                                    actions.child(
                                        div()
                                            .id("graph-retry-returned")
                                            .h(px(30.0))
                                            .px_3()
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .rounded(px(4.0))
                                            .bg(rgb(RELAY))
                                            .text_color(rgb(DECK))
                                            .on_click(cx.listener(|desktop, _, _, cx| {
                                                desktop.create_delivery_run(
                                                    DeliveryRunPurpose::Retry,
                                                    cx,
                                                );
                                            }))
                                            .child("Retry returned work"),
                                    )
                                })
                                .when(reviewable && !review_action_busy, |actions| {
                                    actions
                                        .child(
                                            div()
                                                .id("graph-review-note")
                                                .h(px(30.0))
                                                .flex_1()
                                                .min_w(px(180.0))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .rounded(px(4.0))
                                                .border_1()
                                                .border_color(rgb(HAIRLINE))
                                                .bg(rgb(PANEL))
                                                .track_focus(&self.review_focus)
                                                .on_key_down(
                                                    cx.listener(Self::handle_review_note_key),
                                                )
                                                .on_click(cx.listener(
                                                    |desktop, _, window, cx| {
                                                        window.focus(&desktop.review_focus, cx);
                                                    },
                                                ))
                                                .font_family(UI_FONT)
                                                .text_xs()
                                                .text_color(rgb(if review_note.is_empty() {
                                                    TRACE
                                                } else {
                                                    CHALK
                                                }))
                                                .child(if review_note.is_empty() {
                                                    "Return note (required)".to_owned()
                                                } else {
                                                    review_note
                                                }),
                                        )
                                        .child(
                                            div()
                                                .id("graph-run-return")
                                                .h(px(30.0))
                                                .px_3()
                                                .flex()
                                                .items_center()
                                                .cursor_pointer()
                                                .rounded(px(4.0))
                                                .border_1()
                                                .border_color(rgb(FAULT))
                                                .text_color(rgb(FAULT))
                                                .on_click(cx.listener(
                                                    move |desktop, _, _, cx| {
                                                        desktop
                                                            .settle_selected_review(false, cx);
                                                    },
                                                ))
                                                .child("Return"),
                                        )
                                        .child(
                                            div()
                                                .id("graph-run-accept")
                                                .h(px(30.0))
                                                .px_3()
                                                .flex()
                                                .items_center()
                                                .cursor_pointer()
                                                .rounded(px(4.0))
                                                .bg(rgb(SUCCESS))
                                                .text_color(rgb(DECK))
                                                .on_click(cx.listener(
                                                    move |desktop, _, _, cx| {
                                                        desktop.settle_selected_review(true, cx);
                                                    },
                                                ))
                                                .child("Accept"),
                                        )
                                }),
                        )
                    },
                )
                .when_some(review_action_status, |detail, (ok, message)| {
                    detail.child(
                        div()
                            .pl_2()
                            .border_l_2()
                            .border_color(rgb(if review_action_busy {
                                RELAY
                            } else if ok {
                                SUCCESS
                            } else {
                                FAULT
                            }))
                            .font_family(UI_FONT)
                            .text_xs()
                            .text_color(rgb(TRACE))
                            .child(message),
                    )
                })
                .when(!dependencies.is_empty(), |detail| {
                    detail.child(
                        div()
                            .mt_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_xs()
                                    .text_color(rgb(TRACE))
                                    .child("PREREQUISITES"),
                            )
                            .children(dependencies.into_iter().map(|(id, objective, status)| {
                                div()
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(HAIRLINE))
                                    .p_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_family(UI_FONT)
                                            .text_xs()
                                            .text_color(rgb(RELAY))
                                            .child(id),
                                    )
                                    .child(div().flex_1().min_w_0().child(objective))
                                    .child(
                                        div()
                                            .font_family(UI_FONT)
                                            .text_xs()
                                            .text_color(rgb(status_color_for_run(status)))
                                            .child(format!("{status:?}")),
                                    )
                            })),
                    )
                })
                .when(!grants.is_empty(), |detail| {
                    detail.child(
                        div()
                            .mt_1()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_xs()
                                    .text_color(rgb(TRACE))
                                    .child("GRANTS"),
                            )
                            .children(grants.into_iter().map(
                                |(id, operation, scope, enforcement, uses, revoked)| {
                                    div()
                                        .rounded(px(4.0))
                                        .border_1()
                                        .border_color(rgb(if revoked { FAULT } else { SUCCESS }))
                                        .p_2()
                                        .child(operation)
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_xs()
                                                .text_color(rgb(TRACE))
                                                .child(format!(
                                                    "{id} · {scope} · {enforcement:?} · {uses} use(s){}",
                                                    if revoked { " · revoked" } else { "" }
                                                )),
                                        )
                                },
                            )),
                    )
                })
                .when_some(run.summary.clone(), |detail, summary| {
                    detail.child(
                        div()
                            .rounded(px(4.0))
                            .bg(rgb(PANEL))
                            .p_3()
                            .text_color(rgb(CHALK))
                            .child(summary),
                    )
                })
                .when(!artifacts.is_empty(), |detail| {
                    detail.child(
                        div()
                            .mt_2()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_xs()
                                    .text_color(rgb(TRACE))
                                    .child("ARTIFACTS"),
                            )
                            .children(artifacts.into_iter().map(|(name, locator, metadata)| {
                                div()
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(HAIRLINE))
                                    .p_2()
                                    .child(name)
                                    .child(
                                        div()
                                            .font_family(UI_FONT)
                                            .text_xs()
                                            .text_color(rgb(TRACE))
                                            .child(locator),
                                    )
                                    .child(
                                        div()
                                            .font_family(UI_FONT)
                                            .text_xs()
                                            .text_color(rgb(TRACE))
                                            .child(metadata),
                                    )
                            })),
                    )
                })
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .font_family(UI_FONT)
                .text_color(rgb(TRACE))
                .child("No runs yet. Delegate work to build the mission graph.")
                .into_any_element()
        };

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x17191E).alpha(0.82))
                .child(
                    div()
                        .w(px(820.0))
                        .h(px(580.0))
                        .flex()
                        .flex_col()
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(rgb(HAIRLINE))
                        .bg(rgb(DECK))
                        .shadow_lg()
                        .overflow_hidden()
                        .child(
                            div()
                                .h(px(42.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(rgb(HAIRLINE))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Mission graph"),
                                )
                                .child(
                                    div()
                                        .ml_2()
                                        .font_family(UI_FONT)
                                        .text_xs()
                                        .text_color(rgb(TRACE))
                                        .child(format!(
                                            "{} runs · v{}",
                                            runs.len(),
                                            mission.version
                                        )),
                                )
                                .child(
                                    div()
                                        .id("close-graph-inspector")
                                        .ml_auto()
                                        .size(px(24.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .rounded(px(4.0))
                                        .text_color(rgb(TRACE))
                                        .hover(|button| {
                                            button.bg(rgb(ACTIVE)).text_color(rgb(CHALK))
                                        })
                                        .on_click(cx.listener(|desktop, _, _, cx| {
                                            desktop.graph_inspector_open = false;
                                            cx.notify();
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .id("graph-run-list")
                                        .w(px(310.0))
                                        .h_full()
                                        .overflow_y_scroll()
                                        .border_r_1()
                                        .border_color(rgb(HAIRLINE))
                                        .children(runs.into_iter().map(|run| {
                                            let id = run.id;
                                            let depth = run_depth(mission, id).min(4) as f32;
                                            let selected = selected_id == Some(id);
                                            let attention = mission
                                                .attention_queue()
                                                .into_iter()
                                                .any(|item| item.run_id == id);
                                            let state = if run.status
                                                == termi9ne_core::RunStatus::Pending
                                            {
                                                mission.run_readiness(id).map_or_else(
                                                    |_| "pending".to_owned(),
                                                    |value| format!("{value:?}"),
                                                )
                                            } else {
                                                format!("{:?}", run.status)
                                            };
                                            div()
                                                .id(format!("graph-run-{id}"))
                                                .relative()
                                                .min_h(px(56.0))
                                                .pl(px(12.0 + depth * 16.0))
                                                .pr_2()
                                                .py_2()
                                                .flex()
                                                .gap_2()
                                                .cursor_pointer()
                                                .border_b_1()
                                                .border_color(rgb(HAIRLINE))
                                                .bg(rgb(if selected { ACTIVE } else { DECK }))
                                                .hover(|node| node.bg(rgb(PANEL)))
                                                .on_click(cx.listener(move |desktop, _, _, cx| {
                                                    desktop.selected_graph_run = Some(id);
                                                    desktop.review_note.clear();
                                                    desktop.review_action_status = None;
                                                    cx.notify();
                                                }))
                                                .child(
                                                    div().mt_1().size(px(8.0)).rounded(px(2.0)).bg(
                                                        rgb(if attention {
                                                            SIGNAL
                                                        } else {
                                                            status_color_for_run(run.status)
                                                        }),
                                                    ),
                                                )
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_col()
                                                        .min_w_0()
                                                        .child(
                                                            div()
                                                                .overflow_hidden()
                                                                .whitespace_nowrap()
                                                                .child(run.objective.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .font_family(UI_FONT)
                                                                .text_xs()
                                                                .text_color(rgb(TRACE))
                                                                .child(format!(
                                                                    "{} · {}",
                                                                    run.actor.id, state
                                                                )),
                                                        ),
                                                )
                                        })),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .child(
                                            div()
                                                .id("graph-run-details")
                                                .flex_1()
                                                .min_h_0()
                                                .overflow_y_scroll()
                                                .child(details),
                                        )
                                        .child(timeline),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    #[cfg(not(test))]
    fn attention_sheet(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let signal_id = self.selected_attention?;
        let mission = self.workspace().mission.as_ref()?;
        let signal = mission.signals.get(&signal_id)?;
        let run = mission.runs.get(&signal.run_id)?;
        let (eyebrow, request, primary, secondary) = match &signal.kind {
            SignalKind::InputNeeded { question } => {
                ("INPUT NEEDED", question.clone(), "Send response", None)
            }
            SignalKind::ApprovalNeeded { operation, risk } => (
                match risk {
                    termi9ne_core::Risk::High => "HIGH-RISK APPROVAL",
                    termi9ne_core::Risk::Medium => "APPROVAL · MEDIUM RISK",
                    termi9ne_core::Risk::Low => "APPROVAL · LOW RISK",
                },
                operation.clone(),
                "Allow once",
                Some("Deny"),
            ),
            SignalKind::Blocked { reason } => {
                ("RUN BLOCKED", reason.clone(), "Send guidance", None)
            }
            SignalKind::Escalation {
                conflict,
                requested_resolution,
                ..
            } => (
                "ESCALATION",
                format!("{conflict}\n\nRequested resolution: {requested_resolution}"),
                "Resolve escalation",
                None,
            ),
            SignalKind::Progress { .. } | SignalKind::ArtifactReady { .. } => return None,
        };
        let is_approval = matches!(signal.kind, SignalKind::ApprovalNeeded { .. });
        let actor = run.actor.id.to_string();
        let objective = run.objective.clone();

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x17191E).alpha(0.76))
                .child(
                    div()
                        .w(px(480.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(rgb(SIGNAL))
                        .bg(rgb(DECK))
                        .shadow_lg()
                        .overflow_hidden()
                        .child(
                            div()
                                .h(px(38.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(rgb(HAIRLINE))
                                .font_family(UI_FONT)
                                .text_xs()
                                .text_color(rgb(SIGNAL))
                                .child(eyebrow)
                                .child(
                                    div()
                                        .id("close-attention-sheet")
                                        .ml_auto()
                                        .size(px(22.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .rounded(px(4.0))
                                        .text_color(rgb(TRACE))
                                        .hover(|button| {
                                            button.bg(rgb(ACTIVE)).text_color(rgb(CHALK))
                                        })
                                        .on_click(cx.listener(|desktop, _, _, cx| {
                                            desktop.selected_attention = None;
                                            cx.notify();
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .p_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    div()
                                        .font_family(PRODUCT_FONT)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_size(ui_size(18.0))
                                        .text_color(rgb(CHALK))
                                        .child(request),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_xs()
                                        .text_color(rgb(TRACE))
                                        .child(format!("{actor} · {objective}")),
                                )
                                .child(
                                    div()
                                        .rounded(px(4.0))
                                        .bg(rgb(PANEL))
                                        .p_2()
                                        .font_family(UI_FONT)
                                        .text_xs()
                                        .text_color(rgb(TRACE))
                                        .child(
                                            "Agent-reported request · recorded in mission history",
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .justify_end()
                                        .gap_2()
                                        .when_some(secondary, |actions, label| {
                                            actions.child(
                                                div()
                                                    .id("attention-secondary-action")
                                                    .h(px(30.0))
                                                    .px_3()
                                                    .flex()
                                                    .items_center()
                                                    .cursor_pointer()
                                                    .rounded(px(4.0))
                                                    .border_1()
                                                    .border_color(rgb(HAIRLINE))
                                                    .text_color(rgb(FAULT))
                                                    .on_click(cx.listener(
                                                        move |desktop, _, _, cx| {
                                                            if is_approval {
                                                                desktop.decide_selected_approval(
                                                                    false, cx,
                                                                );
                                                            } else {
                                                                desktop.resolve_selected_attention(
                                                                    "Denied by operator",
                                                                    cx,
                                                                );
                                                            }
                                                        },
                                                    ))
                                                    .child(label),
                                            )
                                        })
                                        .child(
                                            div()
                                                .id("attention-primary-action")
                                                .h(px(30.0))
                                                .px_3()
                                                .flex()
                                                .items_center()
                                                .cursor_pointer()
                                                .rounded(px(4.0))
                                                .bg(rgb(SIGNAL))
                                                .text_color(rgb(DECK))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .on_click(cx.listener(move |desktop, _, _, cx| {
                                                    if is_approval {
                                                        desktop.decide_selected_approval(true, cx);
                                                    } else {
                                                        desktop.resolve_selected_attention(
                                                            "Guidance supplied by operator",
                                                            cx,
                                                        );
                                                    }
                                                }))
                                                .child(primary),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn workspace(&self) -> &WorkspaceView {
        self.workspaces.active().content()
    }

    fn workspace_mut(&mut self) -> &mut WorkspaceView {
        self.workspaces.active_mut().content_mut()
    }

    fn selected_session(&self) -> &SessionView {
        &self.workspace().sessions[self.workspace().selected_session]
    }

    fn selected_session_mut(&mut self) -> &mut SessionView {
        let selected = self.workspace().selected_session;
        &mut self.workspace_mut().sessions[selected]
    }

    fn selected_terminals(&self) -> &[usize] {
        &self.selected_session().terminals
    }

    fn active_terminal_index(&self) -> Option<usize> {
        self.active_surface
            .filter(|terminal| self.selected_terminals().contains(terminal))
            .or_else(|| self.selected_terminals().first().copied())
    }

    fn copy_active_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.active_terminal_index() else {
            return;
        };
        let surface = self.surfaces[terminal].clone();
        surface.update(cx, TerminalSurface::copy_visible);
    }

    fn paste_into_active_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(terminal) = self.active_terminal_index() else {
            return;
        };
        let surface = self.surfaces[terminal].clone();
        surface.update(cx, |surface, cx| {
            surface.paste_from_clipboard(false, cx);
        });
    }

    fn persist_workspaces(&self) {
        #[cfg(not(test))]
        {
            let workspaces = self
                .workspaces
                .tabs()
                .iter()
                .map(|tab| {
                    let workspace = tab.content();
                    WorkspaceRecord {
                        id: tab.id().raw(),
                        mission_id: workspace.mission_id,
                        title: tab.title().to_owned(),
                        pinned: tab.pinned(),
                        selected_session: workspace.selected_session,
                        focus_mode: workspace.focus_mode,
                        sessions: workspace
                            .sessions
                            .iter()
                            .map(|session| SessionRecord {
                                group_id: Some(session.group_id),
                                group_version: session.group_version,
                                pinned: session.pinned,
                                name: session.name.clone(),
                                actor: session.actor.clone(),
                                terminals: session
                                    .terminals
                                    .iter()
                                    .filter_map(|index| self.surface_sessions.get(*index).copied())
                                    .collect(),
                                pane_layout: self.pane_layouts.get(&session.terminals).cloned(),
                            })
                            .collect(),
                    }
                })
                .collect();
            let document = WorkspaceDocument::new(
                self.workspaces.active_id().raw(),
                self.next_fixture,
                self.theme_selection.clone(),
                self.provider_usage_settings.clone(),
                self.sidebar_custom_width.map(|width| width.round() as u16),
                workspaces,
            );
            if let Err(error) = self.workspace_store.save(&document) {
                eprintln!("workspace persistence failed: {error}");
            }
        }
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        if !self.sidebar_open {
            self.session_sidebar.dismiss_session_overlays();
        }
        cx.notify();
    }

    fn select_theme(&mut self, selection: ThemeSelection, cx: &mut Context<Self>) {
        match theme::activate(&selection) {
            Ok(summary) => {
                self.theme_selection = selection;
                self.theme_name = summary.name;
                cx.set_menus(app_menu::native_menus(&self.theme_selection));
                self.persist_workspaces();
                cx.notify();
            }
            Err(error) => eprintln!("theme activation failed: {error}"),
        }
    }

    fn reload_theme(&mut self, cx: &mut Context<Self>) {
        self.select_theme(self.theme_selection.clone(), cx);
    }

    fn toggle_command_deck(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.command_deck_open = !self.command_deck_open;
        if self.command_deck_open {
            self.tab_overflow_open = false;
            window.focus(&self.command_focus, cx);
        } else {
            self.command_query.clear();
            self.focus_active_terminal(window, cx);
        }
        cx.notify();
    }

    fn handle_command_deck_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "backspace" => {
                self.command_query.pop();
            }
            "escape" => {
                self.command_deck_open = false;
                self.command_query.clear();
                self.focus_active_terminal(window, cx);
            }
            "enter" => self.execute_command_query(window, cx),
            _ if !event.keystroke.modifiers.control && !event.keystroke.modifiers.platform => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                {
                    self.command_query.push_str(text);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn execute_command_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.command_query.trim().to_lowercase();
        if query.contains("interrupt") || query == "ctrl-c" {
            if let Some(surface) = self
                .active_surface
                .and_then(|index| self.surfaces.get(index))
            {
                surface.update(cx, |surface, cx| surface.interrupt(cx));
            }
        } else if let Some(command) = self.command_query.trim().strip_prefix('$') {
            let command = command.trim().to_owned();
            if !command.is_empty() {
                self.add_session(window, cx);
                if let Some(surface_index) = self.active_surface
                    && let Some(surface) = self.surfaces.get(surface_index)
                {
                    surface.update(cx, |surface, cx| surface.run_command(&command, cx));
                }
            }
        } else if query.contains("workspace") {
            self.add_workspace(window, cx);
        } else if query.contains("terminal") || query.contains("pane") {
            self.add_terminal(window, cx);
        } else if query.contains("graph") || query.contains("history") {
            #[cfg(not(test))]
            self.open_graph_inspector(cx);
        } else if query.contains("fault") || query.contains("broken") || query.contains("error") {
            self.open_fault_panel(None, cx);
        } else if query.contains("perf")
            || query.contains("telemetry")
            || query.contains("status bar")
        {
            self.toggle_status_bar(cx);
        } else if query.contains("provider")
            || query.contains("usage")
            || query.contains("limit")
            || query.contains("settings")
        {
            self.open_provider_settings(None, cx);
        } else if query.contains("focus") || query.contains("grid") {
            self.toggle_focus_mode(cx);
        } else if query.contains("pin") {
            self.toggle_active_workspace_pin(cx);
        } else {
            self.add_session(window, cx);
        }
        self.command_query.clear();
    }

    fn request_session_termination(&mut self, cx: &mut Context<Self>) {
        if self.shared_mode {
            self.command_deck_open = false;
            cx.notify();
            return;
        }
        self.pending_termination = Some(self.workspace().selected_session);
        self.command_deck_open = false;
        #[cfg(not(test))]
        {
            self.graph_inspector_open = false;
            self.selected_attention = None;
        }
        cx.notify();
    }

    fn confirm_session_termination(&mut self, cx: &mut Context<Self>) {
        if self.shared_mode {
            self.pending_termination = None;
            cx.notify();
            return;
        }
        let Some(session_index) = self.pending_termination.take() else {
            return;
        };
        let Some(session) = self.workspace().sessions.get(session_index) else {
            return;
        };
        let terminals = session.terminals.clone();
        for terminal in terminals {
            if let Some(surface) = self.surfaces.get(terminal) {
                surface.update(cx, |surface, cx| surface.terminate(cx));
            }
        }
        cx.notify();
    }

    fn termination_confirmation(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session_index = self.pending_termination?;
        let session = self.workspace().sessions.get(session_index)?;
        let name = session.name.clone();
        let count = session.terminals.len();
        let force = session.terminals.iter().any(|index| {
            self.surfaces
                .get(*index)
                .is_some_and(|surface| surface.read(cx).requires_force_kill())
        });
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(0x17191E).alpha(0.8))
                .child(
                    div()
                        .w(px(440.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(rgb(FAULT))
                        .bg(rgb(DECK))
                        .shadow_lg()
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(ui_size(18.0))
                                .child(if force {
                                    format!("Force kill {name}?")
                                } else {
                                    format!("Terminate {name}?")
                                }),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_xs()
                                .text_color(rgb(TRACE))
                                .child(if force {
                                    format!(
                                        "This sends SIGKILL to {count} PTY process(es) that ignored SIGHUP and SIGTERM. Final frames remain available."
                                    )
                                } else {
                                    format!(
                                        "This sends SIGHUP, waits two seconds, then SIGTERM to {count} PTY process(es). Force kill is never automatic."
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("cancel-session-termination")
                                        .h(px(30.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .rounded(px(4.0))
                                        .border_1()
                                        .border_color(rgb(HAIRLINE))
                                        .on_click(cx.listener(|desktop, _, _, cx| {
                                            desktop.pending_termination = None;
                                            cx.notify();
                                        }))
                                        .child("Cancel"),
                                )
                                .child(
                                    div()
                                        .id("confirm-session-termination")
                                        .h(px(30.0))
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .rounded(px(4.0))
                                        .bg(rgb(FAULT))
                                        .text_color(rgb(DECK))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .on_click(cx.listener(|desktop, _, _, cx| {
                                            desktop.confirm_session_termination(cx);
                                        }))
                                        .child(if force {
                                            "Force kill session"
                                        } else {
                                            "Terminate session"
                                        }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn toggle_focus_mode(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace_mut();
        workspace.focus_mode = !workspace.focus_mode;
        self.command_deck_open = false;
        #[cfg(not(test))]
        self.sync_surface_presentation(cx);
        self.persist_workspaces();
        cx.notify();
    }

    fn focus_active_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.active_surface = self.selected_terminals().first().copied();
        #[cfg(not(test))]
        self.sync_surface_presentation(cx);
        if let Some(surface_index) = self.active_surface
            && let Some(surface) = self.surfaces.get(surface_index)
        {
            window.focus(&surface.read(cx).focus_handle_owned(), cx);
        }
    }

    #[cfg(not(test))]
    fn sync_surface_presentation(&mut self, cx: &mut Context<Self>) {
        let presented = self
            .selected_terminals()
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        for (index, surface) in self.surfaces.iter().enumerate() {
            let is_presented = presented.contains(&index);
            surface.update(cx, |surface, cx| {
                surface.set_presented(is_presented, cx);
            });
        }
    }

    fn select_workspace(&mut self, id: WorkspaceId, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.activate(id).is_err() {
            return;
        }
        self.session_sidebar.dismiss_session_overlays();
        self.focus_active_terminal(window, cx);
        self.command_deck_open = false;
        self.tab_overflow_open = false;
        #[cfg(not(test))]
        {
            self.selected_attention = None;
        }
        self.persist_workspaces();
        cx.notify();
    }

    fn cycle_workspace(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.workspaces.cycle(offset);
        self.session_sidebar.dismiss_session_overlays();
        self.focus_active_terminal(window, cx);
        self.command_deck_open = false;
        self.tab_overflow_open = false;
        self.persist_workspaces();
        cx.notify();
    }

    fn activate_workspace_at(
        &mut self,
        index: usize,
        last: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.workspaces.tabs();
        let target = if last { tabs.last() } else { tabs.get(index) };
        if let Some(target) = target {
            self.select_workspace(target.id(), window, cx);
        }
    }

    fn cycle_session(&mut self, offset: isize, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.workspace().sessions.len();
        if count == 0 {
            return;
        }
        let current = self.workspace().selected_session;
        let next = if offset >= 0 {
            (current + offset.unsigned_abs()) % count
        } else {
            (current + count - (offset.unsigned_abs() % count)) % count
        };
        self.select_session(next, window, cx);
    }

    fn select_session(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index >= self.workspace().sessions.len() {
            return;
        }
        self.session_sidebar.dismiss_session_overlays();
        self.workspace_mut().selected_session = index;
        self.workspace_mut().focus_mode = false;
        self.focus_active_terminal(window, cx);
        self.persist_workspaces();
        cx.notify();
    }

    fn make_surface(&mut self, prefix: &str, window: &mut Window, cx: &mut Context<Self>) -> usize {
        let id = format!("{prefix}-{}", self.next_fixture);
        self.next_fixture += 1;
        #[cfg(not(test))]
        let session = spawn_shell(&self.control);
        #[cfg(not(test))]
        let session_id = session.id();
        let surface = cx.new(|surface_cx| {
            #[cfg(test)]
            {
                TerminalSurface::fixture(&id, EMPTY_SHELL, window, surface_cx)
                    .expect("new terminal fixture must initialize")
            }
            #[cfg(not(test))]
            {
                TerminalSurface::live(&id, session, false, window, surface_cx)
                    .expect("new live terminal must initialize")
            }
        });
        let index = self.surfaces.len();
        #[cfg(not(test))]
        self.surface_subscriptions.push(cx.subscribe(
            &surface,
            move |desktop, _, event: &TerminalSurfaceEvent, cx| {
                desktop.handle_surface_event(index, event, cx);
            },
        ));
        self.surfaces.push(surface);
        self.surface_foreground_processes.push(None);
        #[cfg(not(test))]
        self.surface_sessions.push(session_id);
        #[cfg(not(test))]
        self.surface_missions.push(None);
        #[cfg(not(test))]
        self.surface_runs.push(None);
        #[cfg(not(test))]
        self.surface_statuses.push(TerminalSessionStatus::Running);
        index
    }

    fn add_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shared_mode {
            self.command_deck_open = false;
            cx.notify();
            return;
        }
        self.session_sidebar.dismiss_session_overlays();
        let sequence = self.next_fixture;
        let terminal = self.make_surface("session", window, cx);
        let workspace = self.workspace_mut();
        workspace.sessions.push(SessionView {
            #[cfg(not(test))]
            group_id: SessionGroupId::new(),
            #[cfg(not(test))]
            group_version: 0,
            #[cfg(not(test))]
            pinned: false,
            name: format!("shell-{sequence}"),
            actor: "you".to_owned(),
            status: SessionStatus::Idle,
            terminals: vec![terminal],
        });
        workspace.selected_session = workspace.sessions.len() - 1;
        workspace.focus_mode = false;
        #[cfg(not(test))]
        let group_id = workspace.sessions.last().map(|session| session.group_id);
        self.active_surface = Some(terminal);
        #[cfg(not(test))]
        self.sync_surface_presentation(cx);
        self.command_deck_open = false;
        #[cfg(not(test))]
        self.reconcile_domain_sessions();
        #[cfg(not(test))]
        if let Some(group_id) = group_id {
            self.sync_session_group(group_id, None, cx);
        }
        self.persist_workspaces();
        cx.notify();
    }

    fn add_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shared_mode {
            self.command_deck_open = false;
            cx.notify();
            return;
        }
        let terminal = self.make_surface("terminal", window, cx);
        self.selected_session_mut().terminals.push(terminal);
        #[cfg(not(test))]
        let group_id = self.selected_session().group_id;
        self.active_surface = Some(terminal);
        #[cfg(not(test))]
        self.sync_surface_presentation(cx);
        self.command_deck_open = false;
        window.focus(&self.surfaces[terminal].read(cx).focus_handle_owned(), cx);
        #[cfg(not(test))]
        self.reconcile_domain_sessions();
        #[cfg(not(test))]
        if let Some(session_id) = self.surface_sessions.get(terminal).copied() {
            self.sync_session_group(
                group_id,
                Some(SessionGroupChange::AddSession { session_id }),
                cx,
            );
        }
        self.persist_workspaces();
        cx.notify();
    }

    #[cfg(not(test))]
    fn sync_session_group(
        &self,
        group_id: SessionGroupId,
        change: Option<SessionGroupChange>,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self.workspaces.tabs().iter().find_map(|tab| {
            let position = tab
                .content()
                .sessions
                .iter()
                .position(|session| session.group_id == group_id)?;
            let session = &tab.content().sessions[position];
            Some((
                tab.content().mission_id,
                session.name.clone(),
                session
                    .terminals
                    .iter()
                    .filter_map(|index| self.surface_sessions.get(*index).copied())
                    .collect::<Vec<_>>(),
                position as u32,
                session.pinned,
                session.group_version,
            ))
        });
        let Some((mission_id, name, session_ids, position, pinned, version)) = snapshot else {
            return;
        };
        let create_detached = matches!(
            change.as_ref(),
            Some(SessionGroupChange::SetDetached { detached: true })
        );
        let control = self.control.clone();
        let request = cx.background_executor().spawn(async move {
            if version == 0 {
                control.create_session_group(SessionGroupSpec {
                    group_id,
                    mission_id,
                    name,
                    session_ids,
                    position,
                    pinned,
                    detached: create_detached,
                })
            } else if let Some(change) = change {
                control.update_session_group(group_id, version, change)
            } else {
                control.update_session_group(
                    group_id,
                    version,
                    SessionGroupChange::SetPosition { position },
                )
            }
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                match result {
                    Ok(group) => desktop.apply_session_group(group),
                    Err(error) => eprintln!("Session group update failed: {error}"),
                }
                desktop.persist_workspaces();
                cx.notify();
            });
        })
        .detach();
    }

    fn add_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shared_mode {
            self.command_deck_open = false;
            self.tab_overflow_open = false;
            cx.notify();
            return;
        }
        self.session_sidebar.dismiss_session_overlays();
        let sequence = self.next_fixture;
        let terminal = self.make_surface("workspace", window, cx);
        self.workspaces.add(
            workspace_display_name(sequence),
            WorkspaceView {
                sessions: vec![SessionView {
                    #[cfg(not(test))]
                    group_id: SessionGroupId::new(),
                    #[cfg(not(test))]
                    group_version: 0,
                    #[cfg(not(test))]
                    pinned: false,
                    name: "shell".to_owned(),
                    actor: "you".to_owned(),
                    status: SessionStatus::Idle,
                    terminals: vec![terminal],
                }],
                selected_session: 0,
                focus_mode: false,
                #[cfg(not(test))]
                mission_id: None,
                #[cfg(not(test))]
                mission: None,
            },
        );
        self.active_surface = Some(terminal);
        #[cfg(not(test))]
        self.sync_surface_presentation(cx);
        self.command_deck_open = false;
        self.tab_overflow_open = false;
        #[cfg(not(test))]
        {
            self.ensure_workspace_missions();
            self.reconcile_domain_sessions();
        }
        window.focus(&self.surfaces[terminal].read(cx).focus_handle_owned(), cx);
        self.persist_workspaces();
        cx.notify();
    }

    fn close_workspace(&mut self, id: WorkspaceId, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.close(id).is_err() {
            return;
        }
        self.session_sidebar.dismiss_session_overlays();
        self.focus_active_terminal(window, cx);
        self.command_deck_open = false;
        self.tab_overflow_open = false;
        self.persist_workspaces();
        cx.notify();
    }

    fn close_active_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.workspaces.active_id();
        self.close_workspace(id, window, cx);
    }

    fn restore_closed_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.restore_closed().is_none() {
            return;
        }
        self.session_sidebar.dismiss_session_overlays();
        self.focus_active_terminal(window, cx);
        self.command_deck_open = false;
        self.tab_overflow_open = false;
        self.persist_workspaces();
        cx.notify();
    }

    fn toggle_active_workspace_pin(&mut self, cx: &mut Context<Self>) {
        let id = self.workspaces.active_id();
        if self.workspaces.toggle_pin(id).is_ok() {
            self.command_deck_open = false;
            self.persist_workspaces();
            cx.notify();
        }
    }

    fn reorder_workspace(
        &mut self,
        moving: WorkspaceId,
        target: WorkspaceId,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.workspaces.move_before(moving, target), Ok(true)) {
            self.persist_workspaces();
            cx.notify();
        }
    }

    fn workspace_tab(&self, placement: TabPlacement, cx: &mut Context<Self>) -> AnyElement {
        let tab = self
            .workspaces
            .tab(placement.id)
            .expect("tab layout only contains open workspaces");
        let workspace = tab.content();
        let active = self.workspaces.active_id() == placement.id;
        let pinned = tab.pinned();
        let stored_title = tab.title();
        let title = if stored_title.trim().eq_ignore_ascii_case("workspace") {
            workspace
                .sessions
                .get(workspace.selected_session)
                .map(|session| session.name.clone())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| stored_title.to_owned())
        } else {
            stored_title.to_owned()
        };
        let live_count = workspace.live_session_count();
        let attention_count = workspace.attention_count();
        let status_color = if workspace
            .sessions
            .iter()
            .any(|session| session.status == SessionStatus::Working)
        {
            SUCCESS
        } else if workspace
            .sessions
            .iter()
            .any(|session| session.status == SessionStatus::Terminated)
        {
            FAULT
        } else {
            TRACE
        };
        let id = placement.id;
        let accessibility_label = format!(
            "{title}, {live_count} live session{}, {attention_count} item{} needing attention",
            if live_count == 1 { "" } else { "s" },
            if attention_count == 1 { "" } else { "s" }
        );
        let drag = DraggedWorkspaceTab {
            id,
            title: title.clone(),
            position: Point::default(),
        };

        div()
            .id(("workspace-tab", id.raw()))
            .role(Role::Tab)
            .aria_label(accessibility_label)
            .aria_selected(active)
            .group("workspace-tab")
            .relative()
            .w(px((placement.width - 2.0).max(1.0)))
            .h(px(32.0))
            .ml(px(2.0))
            .mt(px(4.0))
            .mb(px(4.0))
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .overflow_hidden()
            .rounded(px(4.0))
            .border_1()
            .cursor_pointer()
            .bg(rgb(if active { ACTIVE } else { PANEL }))
            .border_color(rgb(if active { HAIRLINE } else { PANEL }))
            .font_family(PRODUCT_FONT)
            .text_color(rgb(if active { CHALK } else { TRACE }))
            .hover(|tab| {
                tab.bg(rgb(if active { ACTIVE } else { DECK }))
                    .border_color(rgb(HAIRLINE))
                    .text_color(rgb(CHALK))
            })
            .on_click(cx.listener(move |desktop, _, window, cx| {
                desktop.select_workspace(id, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |desktop, _, window, cx| {
                    desktop.close_workspace(id, window, cx);
                }),
            )
            .on_drag(drag, |drag, position, _, cx| {
                cx.new(|_| drag.clone().at(position))
            })
            .drag_over::<DraggedWorkspaceTab>(move |tab, dragged, _, _| {
                if dragged.id == id {
                    tab
                } else {
                    tab.border_l_2().border_color(rgb(RELAY))
                }
            })
            .on_drop(
                cx.listener(move |desktop, dragged: &DraggedWorkspaceTab, _, cx| {
                    desktop.reorder_workspace(dragged.id, id, cx);
                }),
            )
            .child(
                div()
                    .size(px(if active { 7.0 } else { 6.0 }))
                    .flex_none()
                    .rounded_full()
                    .bg(rgb(status_color)),
            )
            .when(pinned, |tab| {
                tab.child(
                    div()
                        .w_full()
                        .text_center()
                        .font_weight(FontWeight::BOLD)
                        .child(title.chars().next().unwrap_or('W').to_string()),
                )
            })
            .when(!pinned, |tab| {
                tab.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_weight(if active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .child(title),
                )
                .when(placement.width >= 132.0 && attention_count > 0, |tab| {
                    tab.child(
                        div()
                            .min_w(px(18.0))
                            .h(px(16.0))
                            .px_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .bg(rgb(if attention_count > 0 {
                                SIGNAL
                            } else if active {
                                PANEL
                            } else {
                                DECK
                            }))
                            .font_family(UI_FONT)
                            .text_xs()
                            .text_color(rgb(if attention_count > 0 { DECK } else { TRACE }))
                            .child(attention_count.to_string()),
                    )
                })
                .when(
                    self.workspaces.len() > 1 && placement.width >= 104.0,
                    |tab| {
                        tab.child(
                            div()
                                .id(("close-workspace", id.raw()))
                                .role(Role::Button)
                                .aria_label("Close workspace")
                                .size(px(18.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.0))
                                .font_family(UI_FONT)
                                .text_xs()
                                .text_color(rgb(TRACE))
                                .opacity(if active { 1.0 } else { 0.0 })
                                .group_hover("workspace-tab", |close| close.opacity(1.0))
                                .hover(|close| close.bg(rgb(HAIRLINE)).text_color(rgb(CHALK)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |desktop, _, window, cx| {
                                    cx.stop_propagation();
                                    desktop.close_workspace(id, window, cx);
                                }))
                                .child("×"),
                        )
                    },
                )
            })
            .when(active, |tab| {
                tab.child(
                    div()
                        .id("active-workspace-signal")
                        .debug_selector(|| "active-workspace-signal".to_owned())
                        .absolute()
                        .top_0()
                        .left(px(8.0))
                        .right(px(8.0))
                        .h(px(2.0))
                        .bg(rgb(RELAY)),
                )
            })
            .into_any_element()
    }

    fn linux_window_controls(&self, window: &Window) -> AnyElement {
        let maximize_glyph = if window.is_maximized() { "▣" } else { "□" };

        div()
            .h_full()
            .flex()
            .items_stretch()
            .font_family(UI_FONT)
            .text_sm()
            .text_color(rgb(TRACE))
            .child(
                div()
                    .id("window-minimize")
                    .role(Role::Button)
                    .aria_label("Minimize window")
                    .w(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_move(|_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, cx| {
                        cx.stop_propagation();
                        window.minimize_window();
                    })
                    .child("−"),
            )
            .child(
                div()
                    .id("window-maximize")
                    .role(Role::Button)
                    .aria_label(if window.is_maximized() {
                        "Restore window"
                    } else {
                        "Maximize window"
                    })
                    .w(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_move(|_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, cx| {
                        cx.stop_propagation();
                        window.zoom_window();
                    })
                    .child(maximize_glyph),
            )
            .child(
                div()
                    .id("window-close")
                    .role(Role::Button)
                    .aria_label("Close window")
                    .w(px(40.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|button| button.bg(rgb(FAULT)).text_color(rgb(CHALK)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_move(|_, _, cx| cx.stop_propagation())
                    .on_click(|_, window, cx| {
                        cx.stop_propagation();
                        window.remove_window();
                    })
                    .child("×"),
            )
            .into_any_element()
    }

    fn workspace_tabs(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let titlebar_leading_inset = if cfg!(target_os = "macos") {
            macos_titlebar_leading_inset(window.is_fullscreen())
        } else {
            0.0
        };
        let fixed_width = if cfg!(target_os = "macos") {
            150.0 + titlebar_leading_inset
        } else if cfg!(target_os = "linux") {
            260.0
        } else {
            188.0
        };
        let available_width = (f32::from(window.viewport_size().width) - fixed_width).max(88.0);
        let layout = TabStripLayout::calculate(&self.workspaces, available_width);
        let hidden_count = layout.hidden.len();

        div()
            .id("workspace-titlebar")
            .role(Role::TabList)
            .aria_label("Workspaces")
            .h(px(40.0))
            .flex()
            .items_stretch()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(HAIRLINE))
            .when(titlebar_leading_inset > 0.0, |titlebar| {
                titlebar.child(div().w(px(titlebar_leading_inset)).flex_none())
            })
            .child(
                div()
                    .id("titlebar-brand")
                    .w(px(44.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_move();
                    })
                    .border_r_1()
                    .border_color(rgb(HAIRLINE))
                    .font_family(UI_FONT)
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(RELAY))
                    .child("t9"),
            )
            .children(
                layout
                    .visible
                    .into_iter()
                    .map(|placement| self.workspace_tab(placement, cx)),
            )
            .when(hidden_count > 0, |titlebar| {
                titlebar.child(
                    div()
                        .id("workspace-overflow-toggle")
                        .role(Role::Button)
                        .aria_label(format!("Show {hidden_count} more workspaces"))
                        .aria_expanded(self.tab_overflow_open)
                        .min_w(px(34.0))
                        .h(px(28.0))
                        .ml(px(4.0))
                        .mt(px(6.0))
                        .mb(px(6.0))
                        .px_2()
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .cursor_pointer()
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(rgb(HAIRLINE))
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(if self.tab_overflow_open { RELAY } else { TRACE }))
                        .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                        .on_click(cx.listener(|desktop, _, _, cx| {
                            desktop.tab_overflow_open = !desktop.tab_overflow_open;
                            desktop.command_deck_open = false;
                            cx.notify();
                        }))
                        .child("⌄")
                        .child(hidden_count.to_string()),
                )
            })
            .when(!self.shared_mode, |titlebar| {
                titlebar.child(
                    div()
                        .id("new-workspace")
                        .debug_selector(|| "new-workspace".to_owned())
                        .size(px(28.0))
                        .flex_none()
                        .ml(px(4.0))
                        .mt(px(6.0))
                        .mb(px(6.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(px(4.0))
                        .border_1()
                        .border_color(rgb(HAIRLINE))
                        .font_family(UI_FONT)
                        .text_sm()
                        .text_color(rgb(RELAY))
                        .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                        .on_click(cx.listener(|desktop, _, window, cx| {
                            desktop.add_workspace(window, cx);
                        }))
                        .child("+"),
                )
            })
            .child(
                div()
                    .id("titlebar-drag-region")
                    .h_full()
                    .flex_1()
                    .min_w(px(16.0))
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(
                div()
                    .id("status-bar-toggle")
                    .role(Role::Button)
                    .aria_label(if self.status_bar_visible {
                        "Hide telemetry bar"
                    } else {
                        "Show telemetry bar"
                    })
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_2()
                    .cursor_pointer()
                    .font_family(UI_FONT)
                    .text_xs()
                    .text_color(rgb(if self.status_bar_visible {
                        RELAY
                    } else {
                        TRACE
                    }))
                    .hover(|button| button.text_color(rgb(CHALK)))
                    .on_click(cx.listener(|desktop, _, _, cx| {
                        desktop.toggle_status_bar(cx);
                    }))
                    .child("◔"),
            )
            .child(
                div()
                    .id("runtime-identity")
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .font_family(UI_FONT)
                    .text_xs()
                    .text_color(rgb(if self.attached_runtime { RELAY } else { TRACE }))
                    .child(
                        div()
                            .size(px(6.0))
                            .rounded_full()
                            .bg(rgb(if self.attached_runtime {
                                RELAY
                            } else {
                                SUCCESS
                            })),
                    )
                    .child(self.runtime_label.clone()),
            )
            .child(
                div()
                    .id("command-deck-toggle")
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_3()
                    .cursor_pointer()
                    .font_family(UI_FONT)
                    .text_xs()
                    .text_color(rgb(if self.command_deck_open { RELAY } else { TRACE }))
                    .on_click(cx.listener(|desktop, _, window, cx| {
                        desktop.toggle_command_deck(window, cx);
                    }))
                    .child("⌘K"),
            )
            .when(cfg!(target_os = "linux"), |titlebar| {
                titlebar.child(self.linux_window_controls(window))
            })
    }

    fn workspace_overflow(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_id = self.workspaces.active_id();
        let tabs: Vec<_> = self
            .workspaces
            .tabs()
            .iter()
            .map(|tab| {
                let status_color = if tab
                    .content()
                    .sessions
                    .iter()
                    .any(|session| session.status == SessionStatus::Working)
                {
                    SUCCESS
                } else if tab
                    .content()
                    .sessions
                    .iter()
                    .any(|session| session.status == SessionStatus::Terminated)
                {
                    FAULT
                } else {
                    TRACE
                };
                (
                    tab.id(),
                    tab.title().to_owned(),
                    tab.pinned(),
                    tab.content().live_session_count(),
                    status_color,
                )
            })
            .collect();

        div()
            .id("workspace-overflow")
            .absolute()
            .top(px(40.0))
            .right(px(48.0))
            .w(px(300.0))
            .max_h(px(420.0))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .border_1()
            .border_color(rgb(HAIRLINE))
            .rounded_b(px(4.0))
            .bg(rgb(PANEL))
            .shadow_lg()
            .child(
                div()
                    .h(px(30.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .font_family(UI_FONT)
                    .text_xs()
                    .text_color(rgb(TRACE))
                    .child("All workspaces")
                    .child(div().ml_auto().child(tabs.len().to_string())),
            )
            .children(
                tabs.into_iter()
                    .map(|(id, title, pinned, live_count, status_color)| {
                        div()
                            .id(("overflow-workspace", id.raw()))
                            .h(px(34.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .cursor_pointer()
                            .bg(rgb(if id == active_id { ACTIVE } else { PANEL }))
                            .hover(|row| row.bg(rgb(ACTIVE)))
                            .on_click(cx.listener(move |desktop, _, window, cx| {
                                desktop.select_workspace(id, window, cx);
                            }))
                            .child(div().size(px(6.0)).rounded_full().bg(rgb(status_color)))
                            .when(pinned, |row| {
                                row.child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_xs()
                                        .text_color(rgb(RELAY))
                                        .child("PIN"),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_family(PRODUCT_FONT)
                                    .font_weight(if id == active_id {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .child(title),
                            )
                            .child(
                                div()
                                    .font_family(UI_FONT)
                                    .text_xs()
                                    .text_color(rgb(TRACE))
                                    .child(live_count.to_string()),
                            )
                    }),
            )
    }

    /// Split the selected session's terminals into responsive columns.
    /// Returns the ordered terminal surface indices and the column layout.
    fn grid_columns(&self, viewport_width: f32) -> (Vec<usize>, Vec<Vec<usize>>) {
        let session = self.selected_session();
        let mut terminals = session.terminals.clone();
        if self.workspace().focus_mode {
            terminals.truncate(1);
        }
        let sidebar_width = session_sidebar::sidebar_width(
            viewport_width,
            self.sidebar_open,
            self.sidebar_custom_width,
        );
        let main_width = (viewport_width - sidebar_width).max(0.0);
        let column_count = match main_width {
            width if width >= 1_920.0 => 4,
            width if width >= 1_280.0 => 3,
            width if width >= 760.0 => 2,
            _ => 1,
        }
        .min(terminals.len())
        .max(1);
        let mut columns = vec![Vec::new(); column_count];
        for (order, terminal) in terminals.iter().copied().enumerate() {
            columns[order % column_count].push(terminal);
        }
        (terminals, columns)
    }

    fn set_sidebar_custom_width(
        &mut self,
        pointer_x: f32,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) {
        if !self.sidebar_open {
            return;
        }
        let width = pane_layout::sidebar_drag_width(pointer_x, viewport_width);
        if self.sidebar_custom_width != Some(width) {
            self.sidebar_custom_width = Some(width);
            self.persist_workspaces();
            cx.notify();
        }
    }

    pub(crate) fn reset_sidebar_width(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_custom_width.take().is_some() {
            self.persist_workspaces();
            cx.notify();
        }
    }

    fn adjust_pane_layout(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut pane_layout::PaneLayout),
    ) {
        let viewport_width = f32::from(window.viewport_size().width);
        let (terminals, columns) = self.grid_columns(viewport_width);
        let sizes = columns.iter().map(Vec::len).collect::<Vec<_>>();
        self.pane_layouts.update(&terminals, &sizes, change);
        self.persist_workspaces();
        cx.notify();
    }

    fn terminal_directory(&self, surface_index: usize, cx: &App) -> Option<String> {
        #[cfg(not(test))]
        {
            self.surfaces
                .get(surface_index)
                .and_then(|surface| surface.read(cx).current_directory().map(str::to_owned))
        }
        #[cfg(test)]
        {
            let _ = (surface_index, cx);
            None
        }
    }

    /// Keyboard focus moved into a terminal by mouse; make it the target for
    /// paste, copy, and every other active-terminal action.
    #[cfg(not(test))]
    fn activate_focused_surface(&mut self, surface_index: usize, cx: &mut Context<Self>) {
        let Some(session_index) = self
            .workspace()
            .sessions
            .iter()
            .position(|session| session.terminals.contains(&surface_index))
        else {
            return;
        };
        let session_changed = self.workspace().selected_session != session_index;
        if !session_changed && self.active_surface == Some(surface_index) {
            return;
        }
        self.workspace_mut().selected_session = session_index;
        self.active_surface = Some(surface_index);
        self.session_sidebar.dismiss_session_overlays();
        if session_changed {
            self.sync_surface_presentation(cx);
        }
        cx.notify();
    }

    /// Replace an ended terminal with a fresh shell in the same slot.
    fn restart_terminal(&mut self, old_index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.shared_mode || !self.surface_is_in_workspaces(old_index) {
            return;
        }
        let new_index = self.make_surface("terminal", window, cx);
        let active_id = self.workspaces.active_id();
        let mut select_in_active = None;
        #[cfg(not(test))]
        for tab in self.workspaces.tabs_mut() {
            let in_active = tab.id() == active_id;
            for (session_index, session) in tab.content_mut().sessions.iter_mut().enumerate() {
                for terminal in &mut session.terminals {
                    if *terminal == old_index {
                        *terminal = new_index;
                        if in_active {
                            select_in_active = Some(session_index);
                        }
                    }
                }
            }
        }
        #[cfg(test)]
        {
            let _ = active_id;
            for (session_index, session) in self.workspace_mut().sessions.iter_mut().enumerate() {
                for terminal in &mut session.terminals {
                    if *terminal == old_index {
                        *terminal = new_index;
                        select_in_active = Some(session_index);
                    }
                }
            }
        }
        if let Some(session_index) = select_in_active {
            self.workspace_mut().selected_session = session_index;
            self.active_surface = Some(new_index);
        }
        #[cfg(not(test))]
        {
            self.sync_surface_presentation(cx);
            self.reconcile_domain_sessions();
        }
        if let Some(surface) = self.surfaces.get(new_index) {
            let focus = surface.read(cx).focus_handle_owned();
            window.focus(&focus, cx);
        }
        self.persist_workspaces();
        cx.notify();
    }

    /// Remove an ended terminal from its session without touching history.
    fn close_terminal(&mut self, surface_index: usize, cx: &mut Context<Self>) {
        if self.shared_mode {
            return;
        }
        #[cfg(not(test))]
        self.archive_surface(surface_index);
        #[cfg(test)]
        for session in &mut self.workspace_mut().sessions {
            session.terminals.retain(|index| *index != surface_index);
        }
        let selected = self.workspace().selected_session;
        let session_count = self.workspace().sessions.len();
        if session_count > 0 && selected >= session_count {
            self.workspace_mut().selected_session = session_count - 1;
        }
        self.active_surface = self.active_terminal_index();
        #[cfg(not(test))]
        self.sync_surface_presentation(cx);
        self.persist_workspaces();
        cx.notify();
    }

    fn surface_is_in_workspaces(&self, surface_index: usize) -> bool {
        self.workspaces.tabs().iter().any(|tab| {
            tab.content()
                .sessions
                .iter()
                .any(|session| session.terminals.contains(&surface_index))
        })
    }

    /// Make `terminal` the large pane of the focus layout, or leave the focus
    /// layout when it already is.
    fn focus_terminal(&mut self, terminal: usize, window: &mut Window, cx: &mut Context<Self>) {
        let session_index = self.workspace().selected_session;
        let already_focused =
            self.workspace().focus_mode && self.active_terminal_index() == Some(terminal);
        self.workspace_mut().focus_mode = !already_focused;
        self.select_terminal_in_session(session_index, terminal, window, cx);
    }

    fn toggle_status_bar(&mut self, cx: &mut Context<Self>) {
        self.status_bar_visible = !self.status_bar_visible;
        self.command_deck_open = false;
        cx.notify();
    }

    /// One terminal with its header: status dot, mark, title, directory, and
    /// the focus and close actions.
    fn terminal_pane(&self, terminal: usize, cx: &mut Context<Self>) -> AnyElement {
        let session_index = self.workspace().selected_session;
        let agent = self.terminal_agent(terminal);
        let status = self.terminal_status(session_index, terminal);
        let title = self.terminal_title(terminal, agent, cx);
        let directory = self
            .terminal_directory(terminal, cx)
            .map(|directory| match std::env::var("HOME") {
                Ok(home) if !home.is_empty() && directory.starts_with(&home) => {
                    format!("~{}", &directory[home.len()..])
                }
                _ => directory,
            });
        let active = self.active_terminal_index() == Some(terminal);
        let focused_layout = self.workspace().focus_mode && active;
        let ended = self
            .surfaces
            .get(terminal)
            .is_some_and(|surface| surface.read(cx).has_ended());
        let accessibility_label = format!("{title}, {}", status.label("agent"));

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                div()
                    .id(("pane-header", terminal))
                    .debug_selector(move || format!("pane-header-{terminal}"))
                    .role(Role::Tab)
                    .aria_label(accessibility_label)
                    .aria_selected(active)
                    .relative()
                    .h(px(22.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(8.0))
                    .bg(rgb(PANEL))
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .font_family(UI_FONT)
                    .text_size(ui_size(10.0))
                    .text_color(rgb(if active { CHALK } else { TRACE }))
                    .when(ended, |header| header.opacity(0.7))
                    .cursor_pointer()
                    .on_click(cx.listener(move |desktop, _, window, cx| {
                        let session_index = desktop.workspace().selected_session;
                        desktop.select_terminal_in_session(session_index, terminal, window, cx);
                    }))
                    .when(active, |header| {
                        header.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(2.0))
                                .bg(rgb(RELAY)),
                        )
                    })
                    .child(
                        div()
                            .size(px(6.0))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(status.color())),
                    )
                    .when_some(agent, |header, agent| {
                        header.child(
                            gpui::svg()
                                .path(agent.icon_path())
                                .size(px(11.0))
                                .flex_none()
                                .text_color(rgb(if active { CHALK } else { TRACE })),
                        )
                    })
                    .child(
                        div()
                            .flex_none()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .max_w(px(220.0))
                            .font_weight(if active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(title),
                    )
                    .when_some(directory, |header, directory| {
                        header.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(ui_size(9.0))
                                .text_color(rgb(TRACE))
                                .child(directory),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(("pane-focus", terminal))
                            .role(Role::Button)
                            .aria_label(if focused_layout {
                                "Back to grid layout"
                            } else {
                                "Focus this terminal"
                            })
                            .size(px(18.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .cursor_pointer()
                            .text_color(rgb(if focused_layout { RELAY } else { TRACE }))
                            .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                            .on_click(cx.listener(move |desktop, _, window, cx| {
                                cx.stop_propagation();
                                desktop.focus_terminal(terminal, window, cx);
                            }))
                            .child(if focused_layout { "⤡" } else { "⤢" }),
                    )
                    .when(ended && !self.shared_mode, |header| {
                        header.child(
                            div()
                                .id(("pane-close", terminal))
                                .role(Role::Button)
                                .aria_label("Close terminal")
                                .size(px(18.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(3.0))
                                .cursor_pointer()
                                .text_color(rgb(TRACE))
                                .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                                .on_click(cx.listener(move |desktop, _, _, cx| {
                                    cx.stop_propagation();
                                    desktop.close_terminal(terminal, cx);
                                }))
                                .child("×"),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.surfaces[terminal].clone()),
            )
            .into_any_element()
    }

    /// The focus layout: the active terminal large on the left, every other
    /// terminal of the session stacked in a strip on the right.
    fn focus_layout(&self, terminals: &[usize], cx: &mut Context<Self>) -> AnyElement {
        let main = self
            .active_terminal_index()
            .filter(|index| terminals.contains(index))
            .unwrap_or(terminals[0]);
        let strip = terminals
            .iter()
            .copied()
            .filter(|terminal| *terminal != main)
            .collect::<Vec<_>>();
        let strip_len = strip.len().max(1) as f32;
        div()
            .id("terminal-focus-layout")
            .debug_selector(|| "terminal-focus-layout".to_owned())
            .flex()
            .size_full()
            .child(
                div()
                    .w(relative(0.72))
                    .h_full()
                    .min_w_0()
                    .flex()
                    .child(self.terminal_pane(main, cx)),
            )
            .child(
                div()
                    .w(relative(0.28))
                    .h_full()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .border_l_1()
                    .border_color(rgb(HAIRLINE))
                    .children(strip.into_iter().map(|terminal| {
                        div()
                            .w_full()
                            .h(relative(1.0 / strip_len))
                            .min_h_0()
                            .flex()
                            .child(self.terminal_pane(terminal, cx))
                    })),
            )
            .into_any_element()
    }

    fn terminal_grid(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let session = self.selected_session();
        if session.terminals.is_empty() {
            let status = session.status.label(&session.actor);
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .bg(rgb(DECK))
                .font_family(UI_FONT)
                .text_sm()
                .text_color(rgb(TRACE))
                .child(status)
                .child(
                    div()
                        .mt_2()
                        .text_xs()
                        .child("No live terminals. Open retained history from the session menu."),
                )
                .into_any_element();
        }

        let viewport_width = f32::from(window.viewport_size().width);
        let (terminals, columns) = self.grid_columns(viewport_width);
        if terminals.len() == 1 {
            return div()
                .size_full()
                .flex()
                .child(self.terminal_pane(terminals[0], cx))
                .into_any_element();
        }
        if self.workspace().focus_mode {
            return self.focus_layout(&terminals, cx);
        }

        let sizes = columns.iter().map(Vec::len).collect::<Vec<_>>();
        let layout = self.pane_layouts.fitted(&terminals, &sizes);
        let column_fractions = layout.column_fractions();
        let column_count = columns.len();

        div()
            .id("terminal-grid")
            .relative()
            .flex()
            .size_full()
            .on_drag_move(cx.listener(
                |desktop, event: &DragMoveEvent<ColumnSplitDrag>, window, cx| {
                    let fraction = f32::from(event.event.position.x - event.bounds.left())
                        / f32::from(event.bounds.size.width).max(1.0);
                    let column = event.drag(cx).column;
                    desktop.adjust_pane_layout(window, cx, |layout| {
                        layout.set_column_split(column, fraction);
                    });
                },
            ))
            .on_drag_move(cx.listener(
                |desktop, event: &DragMoveEvent<RowSplitDrag>, window, cx| {
                    let fraction = f32::from(event.event.position.y - event.bounds.top())
                        / f32::from(event.bounds.size.height).max(1.0);
                    let RowSplitDrag { column, row } = *event.drag(cx);
                    desktop.adjust_pane_layout(window, cx, |layout| {
                        layout.set_row_split(column, row, fraction);
                    });
                },
            ))
            .children(
                columns
                    .into_iter()
                    .enumerate()
                    .map(|(column_index, column)| {
                        let row_fractions = layout.row_fractions(column_index);
                        let row_count = column.len();
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .w(relative(column_fractions[column_index]))
                            .min_w_0()
                            .h_full()
                            .children(column.into_iter().enumerate().map(
                                |(row_index, terminal)| {
                                    div()
                                        .relative()
                                        .flex()
                                        .w_full()
                                        .h(relative(row_fractions[row_index]))
                                        .min_h_0()
                                        .child(self.terminal_pane(terminal, cx))
                                        .when(row_index + 1 < row_count, |pane| {
                                            pane.child(
                                    div()
                                        .id(("row-split", column_index * 64 + row_index))
                                        .role(Role::Splitter)
                                        .aria_label("Resize terminal rows")
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .bottom(px(-SPLITTER_THICKNESS / 2.0))
                                        .h(px(SPLITTER_THICKNESS))
                                        .cursor(CursorStyle::ResizeUpDown)
                                        .hover(|handle| handle.bg(rgb(RELAY).alpha(0.45)))
                                        .on_drag(
                                            RowSplitDrag {
                                                column: column_index,
                                                row: row_index,
                                            },
                                            |_, _, _, cx| cx.new(|_| SplitDragPreview),
                                        )
                                        .on_click(cx.listener(
                                            move |desktop, event: &gpui::ClickEvent, window, cx| {
                                                if event.click_count() == 2 {
                                                    desktop.adjust_pane_layout(window, cx, |l| {
                                                        l.reset_rows(column_index)
                                                    });
                                                }
                                            },
                                        )),
                                )
                                        })
                                },
                            ))
                            .when(column_index + 1 < column_count, |column| {
                                column.child(
                                    div()
                                        .id(("column-split", column_index))
                                        .role(Role::Splitter)
                                        .aria_label("Resize terminal columns")
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .right(px(-SPLITTER_THICKNESS / 2.0))
                                        .w(px(SPLITTER_THICKNESS))
                                        .cursor(CursorStyle::ResizeLeftRight)
                                        .hover(|handle| handle.bg(rgb(RELAY).alpha(0.45)))
                                        .on_drag(
                                            ColumnSplitDrag {
                                                column: column_index,
                                            },
                                            |_, _, _, cx| cx.new(|_| SplitDragPreview),
                                        )
                                        .on_click(cx.listener(
                                            move |desktop, event: &gpui::ClickEvent, window, cx| {
                                                if event.click_count() == 2 {
                                                    desktop.adjust_pane_layout(window, cx, |l| {
                                                        l.reset_columns()
                                                    });
                                                }
                                            },
                                        )),
                                )
                            })
                    }),
            )
            .into_any_element()
    }

    #[cfg(not(test))]
    fn open_plugin_manager(&mut self, cx: &mut Context<Self>) {
        self.plugin_manager_open = true;
        self.refresh_plugins(cx);
        cx.notify();
    }

    #[cfg(not(test))]
    fn refresh_plugins(&mut self, cx: &mut Context<Self>) {
        if self.plugin_refreshing {
            return;
        }
        self.plugin_refreshing = true;
        self.plugin_error = None;
        let control = self.control.clone();
        let request = cx
            .background_executor()
            .spawn(async move { control.list_plugins() });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.plugin_refreshing = false;
                match result {
                    Ok((plugins, dropped_events)) => {
                        desktop.plugin_summaries = plugins;
                        desktop.plugin_events_dropped = dropped_events;
                        desktop.plugin_error = None;
                    }
                    Err(error) => desktop.plugin_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn plugin_manager(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.plugin_manager_open {
            return None;
        }
        let running = self
            .plugin_summaries
            .iter()
            .filter(|plugin| plugin.state == PluginRuntimeStateSummary::Running)
            .count();
        let summary = if self.plugin_refreshing {
            "Refreshing…".to_owned()
        } else {
            format!(
                "{} installed · {running} running · {} dropped",
                self.plugin_summaries.len(),
                self.plugin_events_dropped
            )
        };

        Some(
            div()
                .id("plugin-manager-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(DECK).alpha(0.78))
                .on_click(cx.listener(|desktop, _, _, cx| {
                    desktop.plugin_manager_open = false;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("plugin-manager")
                        .w(px(620.0))
                        .max_h(px(560.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(rgb(HAIRLINE))
                        .bg(rgb(PANEL))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h(px(54.0))
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_4()
                                .border_b_1()
                                .border_color(rgb(HAIRLINE))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .child(
                                            div()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child("Plugins"),
                                        )
                                        .child(
                                            div()
                                                .font_family(UI_FONT)
                                                .text_xs()
                                                .text_color(rgb(TRACE))
                                                .child(summary),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("plugin-manager-refresh")
                                        .ml_auto()
                                        .h(px(26.0))
                                        .flex()
                                        .items_center()
                                        .px_2()
                                        .rounded(px(3.0))
                                        .cursor_pointer()
                                        .font_family(UI_FONT)
                                        .text_xs()
                                        .text_color(rgb(RELAY))
                                        .hover(|button| button.bg(rgb(ACTIVE)))
                                        .on_click(cx.listener(|desktop, _, _, cx| {
                                            cx.stop_propagation();
                                            #[cfg(not(test))]
                                            desktop.refresh_plugins(cx);
                                            #[cfg(test)]
                                            {
                                                desktop.plugin_refreshing = false;
                                                cx.notify();
                                            }
                                        }))
                                        .child(if self.plugin_refreshing {
                                            "…"
                                        } else {
                                            "REFRESH"
                                        }),
                                )
                                .child(
                                    div()
                                        .id("plugin-manager-close")
                                        .size(px(26.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(3.0))
                                        .cursor_pointer()
                                        .text_color(rgb(TRACE))
                                        .hover(|button| {
                                            button.bg(rgb(ACTIVE)).text_color(rgb(CHALK))
                                        })
                                        .on_click(cx.listener(|desktop, _, _, cx| {
                                            desktop.plugin_manager_open = false;
                                            cx.notify();
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .id("plugin-manager-list")
                                .flex()
                                .flex_col()
                                .overflow_y_scroll()
                                .p_3()
                                .gap_2()
                                .when_some(self.plugin_error.as_ref(), |list, error| {
                                    list.child(
                                        div()
                                            .p_3()
                                            .rounded(px(4.0))
                                            .border_1()
                                            .border_color(rgb(FAULT))
                                            .text_color(rgb(FAULT))
                                            .child(error.clone()),
                                    )
                                })
                                .when(
                                    self.plugin_summaries.is_empty()
                                        && self.plugin_error.is_none()
                                        && !self.plugin_refreshing,
                                    |list| {
                                        list.child(
                                            div()
                                                .h(px(120.0))
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .justify_center()
                                                .gap_2()
                                                .text_color(rgb(TRACE))
                                                .child("No executable plugins installed")
                                                .child(
                                                    div()
                                                        .font_family(UI_FONT)
                                                        .text_xs()
                                                        .child("Install from the CLI, then restart the runtime"),
                                                ),
                                        )
                                    },
                                )
                                .children(self.plugin_summaries.iter().map(plugin_row)),
                        )
                        .child(
                            div()
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .px_4()
                                .border_t_1()
                                .border_color(rgb(HAIRLINE))
                                .font_family(UI_FONT)
                                .text_xs()
                                .text_color(rgb(TRACE))
                                .child("Out-of-process · bounded events · no terminal content"),
                        ),
                )
                .into_any_element(),
        )
    }

    fn performance_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let stats = self.performance_stats;
        let cpu_color = match stats.cpu_percent {
            cpu if cpu >= 75.0 => FAULT,
            cpu if cpu >= 35.0 => SIGNAL,
            _ => SUCCESS,
        };
        let frame_color = match stats.frame_p95_ms {
            ms if ms >= 32.0 => FAULT,
            ms if ms >= 16.7 => SIGNAL,
            _ => RELAY,
        };
        let cpu = if stats.ready {
            format!("{:.1}%", stats.cpu_percent)
        } else {
            "--".to_owned()
        };
        let memory = if stats.ready {
            format_bytes(stats.resident_bytes)
        } else {
            "--".to_owned()
        };
        let draws_per_second = if stats.ready {
            format!("{:.0}", stats.fps)
        } else {
            "--".to_owned()
        };
        let benchmark_fps = if self.performance_benchmark_running {
            "…".to_owned()
        } else {
            stats
                .benchmark_fps
                .map(|fps| format!("{fps:.0}"))
                .unwrap_or_else(|| "—".to_owned())
        };
        let display_max_fps = stats
            .display_max_fps
            .map(|fps| format!("{fps:.0}"))
            .unwrap_or_else(|| "…".to_owned());
        let frame = if stats.ready {
            format!("{:.0}ms", stats.frame_p95_ms)
        } else {
            "--".to_owned()
        };
        let read = if stats.ready {
            format_rate(stats.read_bytes_per_second)
        } else {
            "--".to_owned()
        };
        let written = if stats.ready {
            format_rate(stats.written_bytes_per_second)
        } else {
            "--".to_owned()
        };
        let uptime = if stats.ready {
            format_uptime(stats.uptime_seconds)
        } else {
            "--".to_owned()
        };

        let health = div()
            .id("status-performance-health")
            .debug_selector(|| "status-performance-health".to_owned())
            .h_full()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(CHALK))
            .child(div().size(px(6.0)).rounded_full().bg(rgb(cpu_color)))
            .child("PERF")
            .into_any_element();
        let run = div()
            .id("performance-fps-test")
            .debug_selector(|| "performance-fps-test".to_owned())
            .h_full()
            .flex()
            .items_center()
            .px_2()
            .cursor_pointer()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if self.performance_benchmark_running {
                SIGNAL
            } else {
                RELAY
            }))
            .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
            .on_click(cx.listener(|desktop, _, window, cx| {
                desktop.start_fps_benchmark(window, cx);
            }))
            .child(if self.performance_benchmark_running {
                "TESTING"
            } else {
                "TEST"
            })
            .into_any_element();

        StatusBar::new(f32::from(window.viewport_size().width))
            .leading(WidgetPriority::Essential, health)
            .leading(
                WidgetPriority::Standard,
                status_metric(
                    "status-pty-count",
                    "PTY",
                    self.surfaces.len().to_string(),
                    TRACE,
                ),
            )
            .leading(
                WidgetPriority::Detailed,
                status_metric("status-read-rate", "↓", read, TRACE),
            )
            .leading(
                WidgetPriority::Detailed,
                status_metric("status-write-rate", "↑", written, TRACE),
            )
            .leading(
                WidgetPriority::Detailed,
                status_metric("status-uptime", "UP", uptime, TRACE),
            )
            .trailing(
                WidgetPriority::Standard,
                status_metric("status-draw-rate", "DRAW", draws_per_second, RELAY),
            )
            .trailing(
                WidgetPriority::Standard,
                status_metric("status-test-fps", "TEST", benchmark_fps, RELAY),
            )
            .trailing(
                WidgetPriority::Essential,
                status_metric("status-frame-p95", "P95", frame, frame_color),
            )
            .trailing(
                WidgetPriority::Essential,
                status_metric("status-cpu", "CPU", cpu, cpu_color),
            )
            .trailing(
                WidgetPriority::Essential,
                status_metric("status-memory", "MEM", memory, CHALK),
            )
            .trailing(
                WidgetPriority::Essential,
                performance_max_status_widget(
                    display_max_fps,
                    self.performance_render_probe_progress,
                    self.performance_render_probe_running,
                ),
            )
            .trailing(WidgetPriority::Essential, run)
            .into_any_element()
    }

    fn command_deck(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active_pinned = self.workspaces.active().pinned();
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt(px(64.0))
            .bg(rgb(0x17191E).alpha(0.88))
            .child(
                div()
                    .w(px(520.0))
                    .flex()
                    .flex_col()
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(rgb(HAIRLINE))
                    .bg(rgb(DECK))
                    .overflow_hidden()
                    .child(
                        div()
                            .id("command-deck-input")
                            .h(px(38.0))
                            .flex()
                            .items_center()
                            .px_3()
                            .border_b_1()
                            .border_color(rgb(HAIRLINE))
                            .font_family(UI_FONT)
                            .track_focus(&self.command_focus)
                            .on_key_down(cx.listener(Self::handle_command_deck_key))
                            .on_click(cx.listener(|desktop, _, window, cx| {
                                window.focus(&desktop.command_focus, cx);
                            }))
                            .child(div().mr_2().text_color(rgb(RELAY)).child(">"))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(rgb(if self.command_query.is_empty() {
                                        TRACE
                                    } else {
                                        CHALK
                                    }))
                                    .child(if self.command_query.is_empty() {
                                        "Type an action, or $ shell command".to_owned()
                                    } else {
                                        self.command_query.clone()
                                    }),
                            )
                            .child(div().ml_auto().text_xs().text_color(rgb(TRACE)).child("⌘K")),
                    )
                    .when(!self.shared_mode, |deck| {
                        deck.child(
                            div()
                                .id("deck-new-session")
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .px_3()
                                .cursor_pointer()
                                .bg(rgb(ACTIVE))
                                .on_click(cx.listener(|desktop, _, window, cx| {
                                    desktop.add_session(window, cx);
                                }))
                                .child("New session")
                                .child(
                                    div()
                                        .ml_auto()
                                        .font_family(UI_FONT)
                                        .text_xs()
                                        .text_color(rgb(TRACE))
                                        .child("⌘T"),
                                ),
                        )
                    })
                    .when(!self.shared_mode, |deck| {
                        deck.child(
                            div()
                                .id("deck-new-workspace")
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .px_3()
                                .cursor_pointer()
                                .on_click(cx.listener(|desktop, _, window, cx| {
                                    desktop.add_workspace(window, cx);
                                }))
                                .child("New workspace"),
                        )
                    })
                    .when(!self.shared_mode, |deck| {
                        deck.child(
                            div()
                                .id("deck-new-terminal")
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .px_3()
                                .cursor_pointer()
                                .on_click(cx.listener(|desktop, _, window, cx| {
                                    desktop.add_terminal(window, cx);
                                }))
                                .child("Add terminal to session"),
                        )
                    })
                    .child(
                        div()
                            .id("deck-focus-mode")
                            .h(px(34.0))
                            .flex()
                            .items_center()
                            .px_3()
                            .cursor_pointer()
                            .on_click(
                                cx.listener(|desktop, _, _, cx| desktop.toggle_focus_mode(cx)),
                            )
                            .child(if self.workspace().focus_mode {
                                "Grid layout"
                            } else {
                                "Focus layout"
                            }),
                    )
                    .children({
                        #[cfg(not(test))]
                        {
                            Some(
                                div()
                                    .id("deck-mission-graph")
                                    .h(px(34.0))
                                    .flex()
                                    .items_center()
                                    .px_3()
                                    .cursor_pointer()
                                    .on_click(cx.listener(|desktop, _, _, cx| {
                                        desktop.open_graph_inspector(cx);
                                    }))
                                    .child("Open mission graph")
                                    .into_any_element(),
                            )
                        }
                        #[cfg(test)]
                        {
                            None::<AnyElement>
                        }
                    })
                    .child(
                        div()
                            .id("deck-provider-usage")
                            .h(px(34.0))
                            .flex()
                            .items_center()
                            .px_3()
                            .cursor_pointer()
                            .on_click(cx.listener(|desktop, _, _, cx| {
                                desktop.open_provider_settings(None, cx);
                            }))
                            .child("Agent provider usage")
                            .child(
                                div()
                                    .ml_auto()
                                    .font_family(UI_FONT)
                                    .text_xs()
                                    .text_color(rgb(TRACE))
                                    .child(if cfg!(target_os = "macos") {
                                        "⌘,"
                                    } else {
                                        "^,"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("deck-pin-workspace")
                            .h(px(34.0))
                            .flex()
                            .items_center()
                            .px_3()
                            .cursor_pointer()
                            .on_click(cx.listener(|desktop, _, _, cx| {
                                desktop.toggle_active_workspace_pin(cx);
                            }))
                            .child(if active_pinned {
                                "Unpin workspace"
                            } else {
                                "Pin workspace"
                            }),
                    )
                    .when(self.workspaces.can_restore(), |deck| {
                        deck.child(
                            div()
                                .id("deck-restore-workspace")
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .px_3()
                                .cursor_pointer()
                                .on_click(cx.listener(|desktop, _, window, cx| {
                                    desktop.restore_closed_workspace(window, cx);
                                }))
                                .child("Reopen closed workspace")
                                .child(
                                    div()
                                        .ml_auto()
                                        .font_family(UI_FONT)
                                        .text_xs()
                                        .text_color(rgb(TRACE))
                                        .child("⌘⇧T"),
                                ),
                        )
                    })
                    .when(self.workspaces.len() > 1, |deck| {
                        deck.child(
                            div()
                                .id("deck-close-workspace")
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .px_3()
                                .cursor_pointer()
                                .text_color(rgb(FAULT))
                                .on_click(cx.listener(|desktop, _, window, cx| {
                                    desktop.close_active_workspace(window, cx);
                                }))
                                .child("Close workspace"),
                        )
                    })
                    .when(!self.shared_mode, |deck| {
                        deck.child(
                            div()
                                .id("deck-terminate-session")
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .px_3()
                                .cursor_pointer()
                                .text_color(rgb(FAULT))
                                .on_click(cx.listener(|desktop, _, _, cx| {
                                    desktop.request_session_termination(cx);
                                }))
                                .child("Terminate selected session…"),
                        )
                    })
                    .child(
                        div()
                            .h(px(26.0))
                            .flex()
                            .items_center()
                            .px_3()
                            .border_t_1()
                            .border_color(rgb(HAIRLINE))
                            .font_family(UI_FONT)
                            .text_xs()
                            .text_color(rgb(TRACE))
                            .child("Actions · Workspaces · Sessions · $ tracked shell commands"),
                    ),
            )
    }
}

impl Render for Termi9neDesktop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(surface_index) = self.pending_terminal_restart.take() {
            self.restart_terminal(surface_index, window, cx);
        }
        #[cfg(not(test))]
        {
            if self.startup_to_first_render_ms.is_none() {
                self.startup_to_first_render_ms =
                    Some(self.startup_started_at.elapsed().as_secs_f32() * 1_000.0);
            }
            self.window_handle = self
                .window_handle
                .or_else(|| window.window_handle().downcast::<Self>());
            if !self.performance_monitor_started && self.window_handle.is_some() {
                self.performance_monitor_started = true;
                self.start_performance_monitor(cx);
            }
            if let Some(display_id) = window.display(cx).map(|display| u64::from(display.id()))
                && self.performance_probed_display != Some(display_id)
            {
                self.performance_probed_display = Some(display_id);
                self.performance_stats.display_max_fps = None;
                self.performance_render_probe_running = true;
                self.performance_render_probe_progress = 0.0;
                self.start_display_refresh_probe(display_id, window, cx);
            }
            self.attach_pending_terminals(window, cx);
        }
        div()
            .relative()
            .key_context("Workspace")
            .on_action(cx.listener(|desktop, _: &NewWorkspace, window, cx| {
                desktop.add_workspace(window, cx);
            }))
            .on_action(
                cx.listener(|desktop, _: &CloseActiveWorkspace, window, cx| {
                    desktop.close_active_workspace(window, cx);
                }),
            )
            .on_action(
                cx.listener(|desktop, _: &RestoreClosedWorkspace, window, cx| {
                    desktop.restore_closed_workspace(window, cx);
                }),
            )
            .on_action(cx.listener(|desktop, _: &NextWorkspace, window, cx| {
                desktop.cycle_workspace(1, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &PreviousWorkspace, window, cx| {
                desktop.cycle_workspace(-1, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ToggleWorkspacePin, _, cx| {
                desktop.toggle_active_workspace_pin(cx);
            }))
            .on_action(cx.listener(|desktop, _: &ToggleCommandDeck, window, cx| {
                desktop.toggle_command_deck(window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &NewSession, window, cx| {
                desktop.add_session(window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &NewTerminal, window, cx| {
                desktop.add_terminal(window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &NextSession, window, cx| {
                desktop.cycle_session(1, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &PreviousSession, window, cx| {
                desktop.cycle_session(-1, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ToggleSidebar, _, cx| {
                desktop.toggle_sidebar(cx);
            }))
            .on_action(cx.listener(|desktop, _: &ToggleFocusMode, _, cx| {
                desktop.toggle_focus_mode(cx);
            }))
            .on_action(cx.listener(|desktop, _: &OpenGraphInspector, _, cx| {
                #[cfg(not(test))]
                desktop.open_graph_inspector(cx);
                #[cfg(test)]
                let _ = (desktop, cx);
            }))
            .on_action(cx.listener(|desktop, _: &OpenPluginManager, _, cx| {
                #[cfg(not(test))]
                desktop.open_plugin_manager(cx);
                #[cfg(test)]
                {
                    desktop.plugin_manager_open = true;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|desktop, _: &OpenProviderSettings, _, cx| {
                desktop.open_provider_settings(None, cx);
            }))
            .on_action(cx.listener(|desktop, _: &OpenFaults, _, cx| {
                desktop.open_fault_panel(None, cx);
            }))
            .on_action(cx.listener(|desktop, _: &UseGraphiteTheme, _, cx| {
                desktop.select_theme(ThemeSelection::BuiltIn("graphite".to_owned()), cx);
            }))
            .on_action(cx.listener(|desktop, _: &UsePaperTheme, _, cx| {
                desktop.select_theme(ThemeSelection::BuiltIn("paper".to_owned()), cx);
            }))
            .on_action(cx.listener(|desktop, _: &ReloadTheme, _, cx| {
                desktop.reload_theme(cx);
            }))
            .on_action(cx.listener(|desktop, _: &IncreaseAppZoom, window, cx| {
                desktop.increase_app_zoom(window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &DecreaseAppZoom, window, cx| {
                desktop.decrease_app_zoom(window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ResetAppZoom, window, cx| {
                desktop.reset_app_zoom(window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &CopyTerminal, _, cx| {
                desktop.copy_active_terminal(cx);
            }))
            .on_action(cx.listener(|desktop, _: &PasteTerminal, _, cx| {
                desktop.paste_into_active_terminal(cx);
            }))
            .on_action(cx.listener(|desktop, _: &AboutTermi9ne, window, cx| {
                let detail = format!(
                    "Agent-native terminal workspaces\nVersion {}\nTheme {}",
                    env!("CARGO_PKG_VERSION"),
                    desktop.theme_name,
                );
                drop(window.prompt(PromptLevel::Info, "termi9ne", Some(&detail), &["OK"], cx));
            }))
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, _| {
                window.minimize_window();
            }))
            .on_action(cx.listener(|_, _: &ZoomWindow, window, _| {
                window.zoom_window();
            }))
            .on_action(cx.listener(|_, _: &ToggleFullScreen, window, _| {
                window.toggle_fullscreen();
            }))
            .on_action(cx.listener(|desktop, _: &ActivateWorkspace1, window, cx| {
                desktop.activate_workspace_at(0, false, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ActivateWorkspace2, window, cx| {
                desktop.activate_workspace_at(1, false, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ActivateWorkspace3, window, cx| {
                desktop.activate_workspace_at(2, false, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ActivateWorkspace4, window, cx| {
                desktop.activate_workspace_at(3, false, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ActivateWorkspace5, window, cx| {
                desktop.activate_workspace_at(4, false, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ActivateWorkspace6, window, cx| {
                desktop.activate_workspace_at(5, false, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ActivateWorkspace7, window, cx| {
                desktop.activate_workspace_at(6, false, window, cx);
            }))
            .on_action(cx.listener(|desktop, _: &ActivateWorkspace8, window, cx| {
                desktop.activate_workspace_at(7, false, window, cx);
            }))
            .on_action(
                cx.listener(|desktop, _: &ActivateLastWorkspace, window, cx| {
                    desktop.activate_workspace_at(0, true, window, cx);
                }),
            )
            .on_drag_move(
                cx.listener(|desktop, event: &DragMoveEvent<SidebarEdgeDrag>, _, cx| {
                    let pointer_x = f32::from(event.event.position.x - event.bounds.left());
                    let viewport_width = f32::from(event.bounds.size.width);
                    desktop.set_sidebar_custom_width(pointer_x, viewport_width, cx);
                }),
            )
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .bg(rgb(PANEL))
            .text_color(rgb(CHALK))
            .font_family(PRODUCT_FONT)
            .text_sm()
            .child(self.workspace_tabs(window, cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.session_sidebar(window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .bg(rgb(DECK))
                            .child(self.terminal_grid(window, cx)),
                    ),
            )
            .children(
                self.status_bar_visible
                    .then(|| self.performance_status_bar(window, cx)),
            )
            .when(self.command_deck_open, |workspace| {
                workspace.child(self.command_deck(cx))
            })
            .when(self.tab_overflow_open, |workspace| {
                workspace.child(self.workspace_overflow(cx))
            })
            .children({
                #[cfg(not(test))]
                {
                    self.attention_sheet(cx)
                }
                #[cfg(test)]
                {
                    None::<AnyElement>
                }
            })
            .children(
                (!self.termination_is_anchored())
                    .then(|| self.termination_confirmation(cx))
                    .flatten(),
            )
            .children({
                #[cfg(not(test))]
                {
                    self.graph_inspector(cx)
                }
                #[cfg(test)]
                {
                    None::<AnyElement>
                }
            })
            .children(self.plugin_manager(cx))
            .children(self.provider_settings(cx))
            .children(self.fault_panel(cx))
    }
}

#[cfg(test)]
fn create_surfaces(window: &mut Window, cx: &mut App) -> Vec<Entity<TerminalSurface>> {
    SURFACE_FIXTURES
        .iter()
        .map(|fixture| {
            cx.new(|surface_cx| {
                TerminalSurface::fixture(fixture.id, fixture.output, window, surface_cx)
                    .expect("terminal fixture must initialize")
            })
        })
        .collect()
}

#[cfg(not(test))]
fn create_surfaces(window: &mut Window, cx: &mut App, control: &ControlClient) -> Vec<LiveSurface> {
    let mut sessions = control
        .list_terminals()
        .expect("daemon terminal index must be available")
        .into_iter()
        .map(|terminal| {
            (
                terminal.session_id,
                terminal.status,
                terminal.mission_id,
                terminal.run_id,
                terminal.foreground_process,
                control.terminal(terminal.session_id),
            )
        })
        .collect::<Vec<_>>();
    sessions.sort_by_key(|(session_id, _, _, _, _, _)| session_id.to_string());
    sessions.truncate(64);
    if sessions.is_empty() && !control.is_shared() {
        let session = spawn_shell(control);
        sessions.push((
            session.id(),
            TerminalSessionStatus::Running,
            None,
            None,
            None,
            session,
        ));
    }

    sessions
        .into_iter()
        .enumerate()
        .map(
            |(index, (session_id, status, mission_id, run_id, foreground_process, session))| {
                let id = format!("terminal-{}", index + 1);
                let surface = cx.new(|surface_cx| {
                    TerminalSurface::live(
                        &id,
                        session,
                        status != TerminalSessionStatus::Running,
                        window,
                        surface_cx,
                    )
                    .expect("live terminal must initialize")
                });
                LiveSurface {
                    session_id,
                    mission_id,
                    run_id,
                    status,
                    foreground_process,
                    surface,
                }
            },
        )
        .collect()
}

#[cfg(not(test))]
fn spawn_shell(control: &ControlClient) -> DaemonSession {
    control
        .start_terminal(TerminalSessionSpec {
            session_id: SessionId::new(),
            mission_id: None,
            run_id: None,
            program: std::env::var_os("SHELL")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/bin/sh")),
            args: Vec::new(),
            cwd: std::env::current_dir().expect("desktop working directory must be available"),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(120, 36).expect("default terminal grid must be valid"),
        })
        .expect("daemon-owned PTY shell must start for the desktop")
}

fn desktop_window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.0), px(13.0))),
        }),
        app_owns_titlebar_drag: true,
        window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
        ..Default::default()
    }
}

#[cfg(not(test))]
fn main() {
    let startup_started_at = Instant::now();
    let arguments = DesktopArgs::parse();
    if arguments.internal_daemon {
        if let Err(error) = termi9ne_server::run_blocking(arguments.socket, arguments.state_dir) {
            eprintln!("termi9ne daemon failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    let share_token = match arguments
        .share_token_file
        .as_deref()
        .map(read_share_token)
        .transpose()
    {
        Ok(token) => token,
        Err(error) => {
            eprintln!("termi9ne could not read the Share token: {error}");
            std::process::exit(1);
        }
    };
    let attached_runtime = arguments.connect_only;
    let shared_mode = share_token.is_some();
    let explicit_label = arguments
        .runtime_label
        .clone()
        .or_else(|| shared_mode.then(|| "SHARED".to_owned()))
        .or_else(|| (cfg!(debug_assertions) && !attached_runtime).then(|| "DEV".to_owned()));
    let runtime_label = normalized_runtime_label(explicit_label, attached_runtime);
    let theme_override = arguments
        .theme
        .as_deref()
        .map(ThemeSelection::from_argument);
    let control_result = if let Some(token) = &share_token {
        ControlClient::connect_with_share(&arguments.socket, token.clone())
    } else if attached_runtime {
        ControlClient::connect(&arguments.socket)
    } else {
        match std::env::current_exe() {
            Ok(executable) => ControlClient::connect_or_spawn_program(
                &arguments.socket,
                &arguments.state_dir,
                executable,
                &[OsString::from("--internal-daemon")],
            ),
            Err(error) => Err(ClientError::Io(error)),
        }
    };
    let control = match control_result {
        Ok(control) => control,
        Err(error) => {
            eprintln!("termi9ne could not connect to its runtime: {error}");
            if matches!(error, ClientError::Version { .. }) {
                eprintln!(
                    "A different runtime version already owns {}. Keep it running for active PTYs, or choose another --socket and --state-dir.",
                    arguments.socket.display()
                );
            }
            std::process::exit(1);
        }
    };
    let workspace_state_path = arguments.state_dir.join("workspaces.json");
    let render_benchmark = arguments.render_benchmark;
    let idle_benchmark = arguments.idle_benchmark;

    gpui_platform::application()
        .with_assets(assets::DesktopAssets)
        .run(move |cx: &mut App| {
            #[cfg(target_os = "macos")]
            cx.on_action(|_: &HideApplication, cx| cx.hide());
            #[cfg(target_os = "macos")]
            cx.on_action(|_: &HideOtherApplications, cx| cx.hide_other_apps());
            #[cfg(target_os = "macos")]
            cx.on_action(|_: &ShowAllApplications, cx| cx.unhide_other_apps());
            cx.on_action(|_: &QuitApplication, cx| cx.quit());

            #[cfg(target_os = "macos")]
            cx.bind_keys([
                KeyBinding::new("cmd-t", NewWorkspace, Some("Workspace")),
                KeyBinding::new("cmd-w", CloseActiveWorkspace, Some("Workspace")),
                KeyBinding::new("cmd-shift-t", RestoreClosedWorkspace, Some("Workspace")),
                KeyBinding::new("ctrl-tab", NextWorkspace, Some("Workspace")),
                KeyBinding::new("ctrl-shift-tab", PreviousWorkspace, Some("Workspace")),
                KeyBinding::new("cmd-shift-p", ToggleWorkspacePin, Some("Workspace")),
                KeyBinding::new("cmd-k", ToggleCommandDeck, Some("Workspace")),
                KeyBinding::new("cmd-n", NewSession, Some("Workspace")),
                KeyBinding::new("cmd-d", NewTerminal, Some("Workspace")),
                KeyBinding::new("alt-down", NextSession, Some("Workspace")),
                KeyBinding::new("alt-up", PreviousSession, Some("Workspace")),
                KeyBinding::new("cmd-shift-b", ToggleSidebar, Some("Workspace")),
                KeyBinding::new("cmd-shift-enter", ToggleFocusMode, Some("Workspace")),
                KeyBinding::new("cmd-shift-g", OpenGraphInspector, Some("Workspace")),
                KeyBinding::new("cmd-,", OpenProviderSettings, Some("Workspace")),
                KeyBinding::new("cmd-shift-f", OpenFaults, Some("Workspace")),
                KeyBinding::new("cmd-c", CopyTerminal, Some("Workspace")),
                KeyBinding::new("cmd-v", PasteTerminal, Some("Workspace")),
                KeyBinding::new("cmd-=", IncreaseAppZoom, Some("Workspace")),
                KeyBinding::new("cmd-+", IncreaseAppZoom, Some("Workspace")),
                KeyBinding::new("cmd--", DecreaseAppZoom, Some("Workspace")),
                KeyBinding::new("cmd-0", ResetAppZoom, Some("Workspace")),
                KeyBinding::new("ctrl-=", IncreaseAppZoom, Some("Workspace")),
                KeyBinding::new("ctrl-+", IncreaseAppZoom, Some("Workspace")),
                KeyBinding::new("ctrl--", DecreaseAppZoom, Some("Workspace")),
                KeyBinding::new("ctrl-0", ResetAppZoom, Some("Workspace")),
                KeyBinding::new("cmd-h", HideApplication, None),
                KeyBinding::new("alt-cmd-h", HideOtherApplications, None),
                KeyBinding::new("cmd-q", QuitApplication, None),
                KeyBinding::new("cmd-m", MinimizeWindow, Some("Workspace")),
                KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, Some("Workspace")),
                KeyBinding::new("cmd-1", ActivateWorkspace1, Some("Workspace")),
                KeyBinding::new("cmd-2", ActivateWorkspace2, Some("Workspace")),
                KeyBinding::new("cmd-3", ActivateWorkspace3, Some("Workspace")),
                KeyBinding::new("cmd-4", ActivateWorkspace4, Some("Workspace")),
                KeyBinding::new("cmd-5", ActivateWorkspace5, Some("Workspace")),
                KeyBinding::new("cmd-6", ActivateWorkspace6, Some("Workspace")),
                KeyBinding::new("cmd-7", ActivateWorkspace7, Some("Workspace")),
                KeyBinding::new("cmd-8", ActivateWorkspace8, Some("Workspace")),
                KeyBinding::new("cmd-9", ActivateLastWorkspace, Some("Workspace")),
            ]);
            #[cfg(target_os = "linux")]
            cx.bind_keys([
                KeyBinding::new("ctrl-shift-t", NewWorkspace, Some("Workspace")),
                KeyBinding::new("ctrl-shift-w", CloseActiveWorkspace, Some("Workspace")),
                KeyBinding::new("ctrl-shift-r", RestoreClosedWorkspace, Some("Workspace")),
                KeyBinding::new("ctrl-tab", NextWorkspace, Some("Workspace")),
                KeyBinding::new("ctrl-shift-tab", PreviousWorkspace, Some("Workspace")),
                KeyBinding::new("ctrl-shift-p", ToggleWorkspacePin, Some("Workspace")),
                KeyBinding::new("ctrl-k", ToggleCommandDeck, Some("Workspace")),
                KeyBinding::new("ctrl-shift-n", NewSession, Some("Workspace")),
                KeyBinding::new("ctrl-shift-d", NewTerminal, Some("Workspace")),
                KeyBinding::new("alt-down", NextSession, Some("Workspace")),
                KeyBinding::new("alt-up", PreviousSession, Some("Workspace")),
                KeyBinding::new("ctrl-shift-b", ToggleSidebar, Some("Workspace")),
                KeyBinding::new("ctrl-shift-enter", ToggleFocusMode, Some("Workspace")),
                KeyBinding::new("ctrl-shift-g", OpenGraphInspector, Some("Workspace")),
                KeyBinding::new("ctrl-,", OpenProviderSettings, Some("Workspace")),
                KeyBinding::new("ctrl-shift-f", OpenFaults, Some("Workspace")),
                KeyBinding::new("ctrl-shift-c", CopyTerminal, Some("Workspace")),
                KeyBinding::new("ctrl-shift-v", PasteTerminal, Some("Workspace")),
                KeyBinding::new("ctrl-=", IncreaseAppZoom, Some("Workspace")),
                KeyBinding::new("ctrl-+", IncreaseAppZoom, Some("Workspace")),
                KeyBinding::new("ctrl--", DecreaseAppZoom, Some("Workspace")),
                KeyBinding::new("ctrl-0", ResetAppZoom, Some("Workspace")),
                KeyBinding::new("f11", ToggleFullScreen, Some("Workspace")),
                KeyBinding::new("ctrl-1", ActivateWorkspace1, Some("Workspace")),
                KeyBinding::new("ctrl-2", ActivateWorkspace2, Some("Workspace")),
                KeyBinding::new("ctrl-3", ActivateWorkspace3, Some("Workspace")),
                KeyBinding::new("ctrl-4", ActivateWorkspace4, Some("Workspace")),
                KeyBinding::new("ctrl-5", ActivateWorkspace5, Some("Workspace")),
                KeyBinding::new("ctrl-6", ActivateWorkspace6, Some("Workspace")),
                KeyBinding::new("ctrl-7", ActivateWorkspace7, Some("Workspace")),
                KeyBinding::new("ctrl-8", ActivateWorkspace8, Some("Workspace")),
                KeyBinding::new("ctrl-9", ActivateLastWorkspace, Some("Workspace")),
            ]);

            cx.set_menus(app_menu::native_menus(
                theme_override
                    .as_ref()
                    .unwrap_or(&ThemeSelection::default()),
            ));

            let bounds = Bounds::centered(None, size(px(1_280.0), px(800.0)), cx);
            let window_handle = cx
                .open_window(desktop_window_options(bounds), move |window, cx| {
                    let surfaces = create_surfaces(window, cx, &control);
                    cx.new(|cx| {
                        Termi9neDesktop::new_live(
                            surfaces,
                            control.clone(),
                            LiveDesktopConfig {
                                runtime_label: runtime_label.clone(),
                                attached_runtime,
                                workspace_state_path: workspace_state_path.clone(),
                                theme_override: theme_override.clone(),
                                automated_benchmark: render_benchmark,
                                automated_idle_benchmark: idle_benchmark,
                                startup_started_at,
                            },
                            cx,
                        )
                    })
                })
                .expect("termi9ne desktop window must open");
            cx.activate(true);
            window_handle
                .update(cx, |desktop, window, cx| {
                    desktop.focus_active_terminal(window, cx);
                })
                .expect("initial terminal focus must remain available");
            if render_benchmark {
                window_handle
                    .update(cx, |desktop, window, cx| {
                        desktop.start_automated_benchmark(window, cx);
                    })
                    .expect("desktop benchmark window must remain available");
            } else if idle_benchmark {
                window_handle
                    .update(cx, |desktop, _, cx| {
                        desktop.start_automated_idle_benchmark(cx);
                    })
                    .expect("desktop idle benchmark window must remain available");
            }
        });
}

fn normalized_runtime_label(explicit: Option<String>, attached_runtime: bool) -> String {
    explicit
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(|label| {
            label
                .chars()
                .flat_map(char::to_uppercase)
                .take(16)
                .collect()
        })
        .unwrap_or_else(|| if attached_runtime { "REMOTE" } else { "LOCAL" }.to_owned())
}

#[cfg(not(test))]
fn read_share_token(path: &Path) -> Result<String, std::io::Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    // SAFETY: geteuid reads immutable process credentials.
    let current_uid = unsafe { libc::geteuid() };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != current_uid
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > 512
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Share token file is not owner-only",
        ));
    }
    let contents = std::fs::read_to_string(path)?;
    let token = contents.trim();
    if token.is_empty() || token.len() > 512 || token.chars().any(char::is_whitespace) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Share token is empty or malformed",
        ));
    }
    Ok(token.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AnyWindowHandle, TestAppContext};

    #[test]
    fn runtime_identity_is_bounded_and_never_blank() {
        assert_eq!(normalized_runtime_label(None, false), "LOCAL");
        assert_eq!(normalized_runtime_label(None, true), "REMOTE");
        assert_eq!(
            normalized_runtime_label(Some("  staging west cluster  ".to_owned()), true),
            "STAGING WEST CLU"
        );
        assert_eq!(
            normalized_runtime_label(Some("   ".to_owned()), true),
            "REMOTE"
        );
    }

    #[test]
    fn waiting_sessions_count_as_live_and_use_the_signal_color() {
        assert!(SessionStatus::Waiting.is_live());
        assert_eq!(SessionStatus::Waiting.color(), SIGNAL);
        assert_eq!(SessionStatus::Waiting.label("codex"), "codex waiting");
        assert_eq!(SessionStatus::Closed.color(), HAIRLINE);
    }

    #[test]
    fn custom_titlebar_drag_is_owned_by_the_app() {
        let options = desktop_window_options(Bounds::default());
        assert!(
            options.app_owns_titlebar_drag,
            "native titlebar dragging must not intercept workspace-tab drags"
        );
    }

    #[test]
    fn macos_titlebar_reclaims_traffic_light_space_in_fullscreen() {
        assert_eq!(macos_titlebar_leading_inset(false), 72.0);
        assert_eq!(macos_titlebar_leading_inset(true), 0.0);
    }

    fn add_workspace(cx: &mut TestAppContext) -> gpui::WindowHandle<Termi9neDesktop> {
        cx.add_window(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        })
    }

    #[gpui::test]
    fn workspace_shell_paints_live_and_closed_sessions(cx: &mut TestAppContext) {
        let window = add_workspace(cx);
        let any_window = AnyWindowHandle::from(window);

        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("live terminal grid should paint");

        window
            .update(cx, |desktop, window, cx| {
                desktop.select_session(2, window, cx);
                desktop.toggle_sidebar(cx);
                desktop.toggle_command_deck(window, cx);
            })
            .expect("workspace should remain available");
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("closed session state and command deck should paint");
    }

    #[gpui::test]
    fn application_zoom_actions_scale_the_window_and_reset_to_default(cx: &mut TestAppContext) {
        let window = add_workspace(cx);
        let any_window = AnyWindowHandle::from(window);
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("workspace actions should be registered after drawing");
        window
            .update(cx, |desktop, window, cx| {
                desktop.focus_active_terminal(window, cx);
            })
            .expect("active terminal should anchor the workspace action context");

        cx.dispatch_action(any_window, IncreaseAppZoom);
        window
            .read_with(cx, |desktop, _| {
                assert_eq!(desktop.app_zoom.percentage(), 110);
            })
            .expect("zoom state should remain available");
        cx.update_window(any_window, |_, window, _| {
            assert!((f32::from(window.rem_size()) - 17.6).abs() < 0.001);
        })
        .expect("window metrics should follow application zoom");

        cx.dispatch_action(any_window, DecreaseAppZoom);
        cx.dispatch_action(any_window, DecreaseAppZoom);
        window
            .read_with(cx, |desktop, _| {
                assert_eq!(desktop.app_zoom.percentage(), 90);
            })
            .expect("zoom-out action should update application state");

        cx.dispatch_action(any_window, ResetAppZoom);
        window
            .read_with(cx, |desktop, _| {
                assert_eq!(desktop.app_zoom.percentage(), 100);
            })
            .expect("reset action should restore application state");
        cx.update_window(any_window, |_, window, _| {
            assert_eq!(window.rem_size(), px(16.0));
        })
        .expect("reset should restore the default root resolution");
    }

    #[gpui::test]
    fn performance_status_bar_is_one_thin_widget_rail(cx: &mut TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });
        cx.update(|_, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.performance_stats.update_process(PerformanceSample {
                    cpu_percent: 12.5,
                    resident_bytes: 192 * 1024 * 1024,
                    read_bytes_per_second: 2048.0,
                    written_bytes_per_second: 1024.0,
                    uptime_seconds: 65,
                });
                cx.notify();
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let status_bar = cx
            .debug_bounds("status-bar")
            .expect("the bottom status bar should be rendered");
        assert_eq!(f32::from(status_bar.size.height), 28.0);
        assert!(cx.debug_bounds("status-cpu").is_some());
        assert!(cx.debug_bounds("status-memory").is_some());
        assert!(cx.debug_bounds("status-max-fps").is_some());
        assert!(cx.debug_bounds("performance-fps-test").is_some());
        assert!(cx.debug_bounds("performance-overlay").is_none());
        assert!(cx.debug_bounds("performance-overlay-collapsed").is_none());
    }

    #[gpui::test]
    fn plugin_manager_paints_runtime_health(cx: &mut TestAppContext) {
        let window = add_workspace(cx);
        let any_window = AnyWindowHandle::from(window);
        window
            .update(cx, |desktop, _, _| {
                desktop.plugin_summaries = vec![PluginRuntimeSummary {
                    id: "termi9ne.agent-status".to_owned(),
                    name: "Agent status".to_owned(),
                    version: "0.1.0".to_owned(),
                    state: PluginRuntimeStateSummary::Running,
                    restart_count: 1,
                    dropped_events: 0,
                    last_status: None,
                    last_error: None,
                }];
            })
            .expect("plugin fixture should update");
        window
            .update(cx, |desktop, _, cx| {
                desktop.plugin_manager_open = true;
                cx.notify();
            })
            .expect("plugin manager should open");
        window
            .read_with(cx, |desktop, _| {
                assert!(desktop.plugin_manager_open);
                assert_eq!(desktop.plugin_summaries.len(), 1);
            })
            .expect("plugin manager state should remain available");
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("plugin manager should paint");
    }

    #[gpui::test]
    fn provider_usage_strip_paints_in_both_sidebar_modes(cx: &mut TestAppContext) {
        use crate::provider_usage::{ProviderUsage, ProviderUsageStatus, UsageWindow, WindowSpan};
        let (_desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });
        cx.update(|_, cx| {
            cx.update_entity(&_desktop, |desktop, cx| {
                let usage = ProviderUsage {
                    plan: Some("Max".to_owned()),
                    source: "fixture".to_owned(),
                    windows: vec![
                        UsageWindow {
                            label: "5-hour".to_owned(),
                            span: WindowSpan::Short,
                            used_percent: 42.0,
                            resets_at_unix: Some(i64::MAX / 2),
                        },
                        UsageWindow {
                            label: "Weekly".to_owned(),
                            span: WindowSpan::Long,
                            used_percent: 91.0,
                            resets_at_unix: None,
                        },
                    ],
                    notes: vec!["Extra usage enabled".to_owned()],
                };
                desktop.provider_usage[0].apply(ProviderUsageStatus::Ready(usage.clone()), 10);
                desktop.provider_usage[1].apply(
                    ProviderUsageStatus::Failed {
                        detail: "HTTP 500".to_owned(),
                        previous: None,
                    },
                    10,
                );
                desktop.provider_usage[1].apply(ProviderUsageStatus::Ready(usage), 20);
                desktop.provider_usage[1].apply(
                    ProviderUsageStatus::Failed {
                        detail: "HTTP 503".to_owned(),
                        previous: None,
                    },
                    30,
                );
                desktop.provider_usage[2]
                    .apply(ProviderUsageStatus::NotSignedIn("no login".to_owned()), 10);
                cx.notify();
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("provider-usage-footer").is_some(),
            "the expanded sidebar should end with the provider usage strip"
        );

        cx.update(|_, cx| {
            cx.update_entity(&_desktop, |desktop, cx| desktop.toggle_sidebar(cx));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("provider-usage-footer").is_some(),
            "the collapsed rail should keep the provider usage strip"
        );
    }

    #[gpui::test]
    fn the_fault_panel_lists_faults_and_shows_the_selected_evidence(cx: &mut TestAppContext) {
        use termi9ne_protocol::{FaultKind, FaultSource, FaultState, FaultSummary, ReproReceipt};
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });

        let open = FaultSummary {
            fault_id: termi9ne_core::FaultId::new(),
            kind: FaultKind::TestFailed,
            command: "cargo test -p thing".to_owned(),
            cwd: std::path::PathBuf::from("/work"),
            exit_code: Some(101),
            revision: None,
            summary: "2 tests failed".to_owned(),
            output: "assertion failed".to_owned(),
            session_id: None,
            mission_id: None,
            run_id: None,
            source: FaultSource::TerminalExit,
            observed_at_unix_micros: 2,
            state: FaultState::Open,
            repro: Some(ReproReceipt {
                attempted_at_unix_micros: 3,
                reproduced: true,
                exit_code: Some(101),
                output: "same".to_owned(),
                duration_ms: 120,
                revision: None,
                error: None,
            }),
            repro_attempts: 1,
            fix_run_id: None,
        };
        let closed = FaultSummary {
            fault_id: termi9ne_core::FaultId::new(),
            summary: "old".to_owned(),
            state: FaultState::Dismissed {
                note: "known flake".to_owned(),
                at_unix_micros: 4,
            },
            repro: None,
            repro_attempts: 0,
            ..open.clone()
        };
        let open_id = open.fault_id;

        cx.update(|_, cx| {
            cx.update_entity(&desktop, |desktop, cx| {
                desktop.faults = vec![open, closed];
                desktop.open_fault_panel(Some(open_id), cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("fault-panel").is_some());
        assert!(
            cx.debug_bounds("fault-sidebar").is_none(),
            "the sidebar card is live-only"
        );
        cx.read(|cx| {
            let desktop = desktop.read(cx);
            assert!(desktop.fault_panel_open);
            assert_eq!(desktop.selected_fault, Some(open_id));
            // A still-failing replay must not offer resolution.
            assert!(!desktop.faults[0].repro_passes());
        });

        // A passing replay unlocks the resolve action.
        cx.update(|_, cx| {
            cx.update_entity(&desktop, |desktop, cx| {
                if let Some(receipt) = desktop.faults[0].repro.as_mut() {
                    receipt.reproduced = false;
                }
                cx.notify();
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.read(|cx| assert!(desktop.read(cx).faults[0].repro_passes()));

        // Replacing a Fault keeps list identity rather than duplicating it.
        cx.update(|_, cx| {
            cx.update_entity(&desktop, |desktop, _| {
                let mut updated = desktop.faults[0].clone();
                updated.summary = "updated".to_owned();
                desktop.replace_fault(updated);
            });
        });
        cx.read(|cx| {
            let desktop = desktop.read(cx);
            assert_eq!(desktop.faults.len(), 2);
            assert_eq!(desktop.faults[0].summary, "updated");
        });
    }

    #[gpui::test]
    fn provider_settings_open_on_the_clicked_provider(cx: &mut TestAppContext) {
        let (_desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("provider-settings").is_none());

        cx.update(|_, cx| {
            cx.update_entity(&_desktop, |desktop, cx| {
                desktop.open_provider_settings(Some(ProviderKind::Codex), cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("provider-settings").is_some());
        cx.read(|cx| {
            let desktop = _desktop.read(cx);
            assert!(desktop.provider_settings_open);
            assert_eq!(desktop.provider_settings_tab, ProviderKind::Codex);
        });

        cx.update(|_, cx| {
            cx.update_entity(&_desktop, |desktop, cx| {
                desktop.set_provider_enabled(ProviderKind::Codex, false, cx);
                desktop.set_provider_refresh_interval(900, cx);
            });
        });
        cx.read(|cx| {
            let desktop = _desktop.read(cx);
            assert!(!desktop.provider_usage_settings.codex);
            assert_eq!(desktop.provider_usage_settings.refresh_interval_secs, 900);
            assert!(matches!(
                desktop.provider_usage[1].status,
                crate::provider_usage::ProviderUsageStatus::Disabled
            ));
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn performance_status_bar_shows_the_automatic_render_probe(cx: &mut TestAppContext) {
        let (_desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            cx.debug_bounds("performance-render-probe").is_some(),
            "the automatic MAX workload should be visible in the status bar"
        );
    }

    #[gpui::test]
    fn workspace_tabs_expose_a_clear_active_signal_and_new_action(cx: &mut TestAppContext) {
        let (_desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("active-workspace-signal").is_some());
        assert!(cx.debug_bounds("new-workspace").is_some());
    }

    #[gpui::test]
    fn scrollable_sidebar_uses_a_hidden_overlay_activity_rail(cx: &mut TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            let mut desktop = Termi9neDesktop::new(surfaces, cx.focus_handle());
            let seed = desktop.workspace().sessions[0].clone();
            for index in 0..40 {
                let mut session = seed.clone();
                session.name = format!("scroll-fixture-{index}");
                session.terminals.clear();
                desktop.workspace_mut().sessions.push(session);
            }
            desktop
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("session-scrollbar").is_none());

        cx.update(|_, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.reveal_sidebar_scrollbar(cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            cx.debug_bounds("session-scrollbar").is_some(),
            "a scrollable sidebar should reveal an overlay rail without changing width"
        );
    }

    #[gpui::test]
    fn fps_benchmark_can_start_outside_the_render_pass(cx: &mut TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let run_button = cx
            .debug_bounds("performance-fps-test")
            .expect("RUN button should be rendered");

        cx.simulate_click(run_button.center(), gpui::Modifiers::default());

        let callback_count = cx.update(|window, cx| window.simulate_next_frame(cx));
        assert!(
            callback_count > 0,
            "benchmark should schedule its warm-up redraw"
        );
        let measured_callback_count = cx.update(|window, cx| window.simulate_next_frame(cx));
        assert!(
            measured_callback_count > 0,
            "warm-up completion must schedule the measured redraw loop"
        );

        desktop.read_with(cx, |desktop, _| {
            assert!(desktop.performance_benchmark_running);
        });
    }

    #[gpui::test]
    fn fps_benchmark_completion_updates_the_overlay_entity(cx: &mut TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| {
            let starting_frame_count = window
                .frame_duration_snapshot()
                .draw_duration_histogram
                .len();
            desktop.update(cx, |desktop, cx| {
                desktop.performance_benchmark_running = true;
                schedule_fps_benchmark(
                    Instant::now() - FPS_BENCHMARK_DURATION,
                    starting_frame_count,
                    window,
                    cx,
                );
            });
        });

        assert_eq!(cx.update(|window, cx| window.simulate_next_frame(cx)), 1);
        desktop.read_with(cx, |desktop, _| {
            assert!(!desktop.performance_benchmark_running);
            assert!(desktop.performance_stats.benchmark_fps.is_some());
        });
    }

    #[gpui::test]
    fn tabs_sessions_and_terminal_counts_own_independent_state(cx: &mut TestAppContext) {
        let window = add_workspace(cx);

        window
            .update(cx, |desktop, window, cx| {
                assert_eq!(desktop.workspaces.len(), 2);
                assert_eq!(desktop.selected_terminals(), &[0, 1]);

                desktop.select_session(1, window, cx);
                assert_eq!(desktop.selected_terminals(), &[2]);

                let linux_workspace = desktop.workspaces.tabs()[1].id();
                desktop.select_workspace(linux_workspace, window, cx);
                assert_eq!(desktop.selected_terminals(), &[3, 4]);

                desktop.add_session(window, cx);
                assert_eq!(desktop.selected_terminals().len(), 1);
                assert_eq!(desktop.workspace().sessions.len(), 3);

                desktop.add_workspace(window, cx);
                assert_eq!(desktop.workspaces.len(), 3);
                assert_eq!(desktop.selected_terminals().len(), 1);

                let active_workspace = desktop.workspaces.active_id();
                desktop.close_workspace(active_workspace, window, cx);
                assert_eq!(desktop.workspaces.len(), 2);

                desktop.restore_closed_workspace(window, cx);
                assert_eq!(desktop.workspaces.len(), 3);
            })
            .expect("workspace state should remain available");
    }

    #[gpui::test]
    fn shared_projection_blocks_owner_lifecycle_actions(cx: &mut TestAppContext) {
        let window = add_workspace(cx);

        window
            .update(cx, |desktop, window, cx| {
                desktop.shared_mode = true;
                let workspace_count = desktop.workspaces.len();
                let session_count = desktop.workspace().sessions.len();
                let surface_count = desktop.surfaces.len();

                desktop.add_workspace(window, cx);
                desktop.add_session(window, cx);
                desktop.add_terminal(window, cx);
                desktop.request_session_termination(cx);
                desktop.archive_session(0, cx);

                assert_eq!(desktop.workspaces.len(), workspace_count);
                assert_eq!(desktop.workspace().sessions.len(), session_count);
                assert_eq!(desktop.surfaces.len(), surface_count);
                assert!(desktop.pending_termination.is_none());
            })
            .expect("shared projection should remain read-only outside terminal input");
    }

    #[gpui::test]
    fn keyboard_navigation_wraps_sessions_and_selects_browser_tab_positions(
        cx: &mut TestAppContext,
    ) {
        let window = add_workspace(cx);
        window
            .update(cx, |desktop, window, cx| {
                let first = desktop.workspaces.tabs()[0].id();
                let second = desktop.workspaces.tabs()[1].id();
                assert_eq!(desktop.workspaces.active_id(), first);

                desktop.activate_workspace_at(1, false, window, cx);
                assert_eq!(desktop.workspaces.active_id(), second);
                desktop.activate_workspace_at(0, false, window, cx);
                assert_eq!(desktop.workspaces.active_id(), first);

                assert_eq!(desktop.workspace().selected_session, 0);
                desktop.cycle_session(-1, window, cx);
                assert_eq!(desktop.workspace().selected_session, 2);
                desktop.cycle_session(1, window, cx);
                assert_eq!(desktop.workspace().selected_session, 0);

                desktop.add_workspace(window, cx);
                let last = desktop.workspaces.active_id();
                desktop.activate_workspace_at(0, false, window, cx);
                desktop.activate_workspace_at(0, true, window, cx);
                assert_eq!(desktop.workspaces.active_id(), last);
            })
            .expect("keyboard navigation should keep workspace state valid");
    }

    #[gpui::test]
    fn native_menu_actions_reach_the_workspace_and_active_terminal(cx: &mut TestAppContext) {
        let window = add_workspace(cx);
        let any_window = AnyWindowHandle::from(window);
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("workspace actions should be registered after drawing");
        window
            .update(cx, |desktop, window, cx| {
                desktop.focus_active_terminal(window, cx);
            })
            .expect("active terminal should accept menu actions");

        cx.dispatch_action(any_window, CopyTerminal);
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("Copy menu action should write terminal content");
        assert!(copied.contains("termi9ne verify --platform native"));

        cx.dispatch_action(any_window, NewWorkspace);
        window
            .read_with(cx, |desktop, _| {
                assert_eq!(desktop.workspaces.len(), 3);
            })
            .expect("New Workspace menu action should update the workspace model");

        cx.dispatch_action(any_window, ToggleSidebar);
        window
            .read_with(cx, |desktop, _| {
                assert!(!desktop.sidebar_open);
            })
            .expect("Toggle Sidebar menu action should update the layout");

        cx.dispatch_action(any_window, UsePaperTheme);
        window
            .read_with(cx, |desktop, _| {
                assert!(desktop.theme_selection.is_builtin("paper"));
                assert_eq!(desktop.theme_name, "Paper");
            })
            .expect("Theme menu action should update presentation state");
        cx.dispatch_action(any_window, UseGraphiteTheme);
    }
}
