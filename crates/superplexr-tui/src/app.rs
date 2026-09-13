use base64::{Engine, engine::general_purpose::STANDARD};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, Clear, ClearType},
};
use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use superplexr_client::{ClientError, ControlClient};
use superplexr_terminal::{GridSize, MAX_PASTE_BYTES};
use superplexr_tui::{
    FeedStatus,
    attach_worker::Worker as AttachWorker,
    group_actions::{Action as GroupAction, Worker as GroupWorker},
    input::{Action, Router, risky_paste},
    input_lane::{Command as InputCommand, ReturnState},
    navigator::{GroupEdit, GroupField, Navigator, Tab, Target},
    navigator_worker::Worker as NavigationWorker,
    render,
    search::{Panel, Update as SearchUpdate, Worker},
    workspace::{Axis, Open, Pane, Workspace},
};

const HELP: &str = "^] n:navigate a:attention v:split s:stack o:focus x:close d:detach";
const INPUT_HELP: &str = "^] /:search c:control r:release p:pause h:history l:live y:copy";

#[derive(Clone, Copy)]
struct HistoryPageRequest {
    pane: usize,
    session: superplexr_core::SessionId,
    continuity: u64,
    offset: u32,
}

#[derive(Clone, Copy)]
enum ViewChange {
    Navigate { tab: Tab, mode: Open },
    Next,
    Close,
}
impl ViewChange {
    fn from_action(action: &Action) -> Option<Self> {
        Some(match action {
            Action::Navigate => Self::Navigate {
                tab: Tab::Sessions,
                mode: Open::Replace,
            },
            Action::Attention => Self::Navigate {
                tab: Tab::Attention,
                mode: Open::Replace,
            },
            Action::SplitVertical => Self::Navigate {
                tab: Tab::Sessions,
                mode: Open::Split(Axis::Vertical),
            },
            Action::SplitHorizontal => Self::Navigate {
                tab: Tab::Sessions,
                mode: Open::Split(Axis::Horizontal),
            },
            Action::NextPane => Self::Next,
            Action::ClosePane => Self::Close,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy)]
struct PendingViewChange {
    surface: [u8; 16],
    continuity: u64,
    change: ViewChange,
}

pub fn run(
    client: &ControlClient,
    workspace: &mut Workspace,
    osc52: bool,
    workflow_read: bool,
    stop: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut size = terminal::size()?;
    let mut router = Router::default();
    let mut search: Option<Panel> = None;
    let mut search_worker: Option<Worker> = None;
    let mut history_page: Option<HistoryPageRequest> = None;
    let mut nav = if workspace.panes.is_empty() {
        Some(Navigator::default())
    } else {
        None
    };
    let mut purpose = Open::Replace;
    let mut navigation_worker = None;
    let mut attach_worker: Option<AttachWorker> = None;
    let mut attaching = false;
    let mut pending_change: Option<PendingViewChange> = None;
    let mut group_worker: Option<GroupWorker> = None;
    let mut group_action_note = String::new();
    if let Some(nav) = &mut nav {
        refresh(nav, client, &mut navigation_worker);
    }
    let mut refresh_at = Instant::now();
    let mut dirty = true;
    let mut clear = true;
    while !stop.load(Ordering::Relaxed) {
        let active_grid = workspace.active_grid(size);
        for (index, pane) in workspace.panes.iter_mut().enumerate() {
            if let Some(update) = pane.session.take_update()? {
                if history_page.is_some_and(|request| {
                    request.pane == index
                        && request.session == pane.id
                        && (request.continuity != update.continuity
                            || !matches!(update.status, FeedStatus::Live | FeedStatus::Ended))
                }) {
                    history_page = None;
                    if let Some(worker) = &search_worker {
                        worker.cancel();
                    }
                    pane.status =
                        "History request cancelled after connection change; ^] h retry".into();
                    dirty = true;
                }
                if index == workspace.active
                    && pane.continuity != update.continuity
                    && let Some(panel) = search.as_mut().filter(|panel| panel.session_id == pane.id)
                {
                    if let Some(worker) = &search_worker {
                        worker.cancel();
                    }
                    panel.reset();
                    panel.editing = true;
                    panel.note =
                        "Connection changed; Enter to run a new search after reconnect".into();
                    dirty = true;
                }
                pane.continuity = update.continuity;
                pane.feed = update.status;
                pane.latest = update.frame;
                if !pane.paused {
                    pane.shown = pane.latest.clone();
                    if nav.is_none() && search.is_none() {
                        dirty = true;
                    }
                }
                match update.status {
                    FeedStatus::Live => {
                        if pane.initial_claim && index == workspace.active && nav.is_none() {
                            pane.initial_claim = false;
                            pane.status = match pane
                                .session
                                .queue_input(InputCommand::Claim(active_grid))
                            {
                                Ok(()) => {
                                    "Requesting Control… input stays disabled until acknowledged"
                                        .into()
                                }
                                Err(error) => error.to_string(),
                            };
                        } else if !pane.paused
                            && !router.waiting()
                            && pane.pending_paste.is_none()
                            && !pane.session.input_failed()
                            && !pane.session.claim_pending()
                            && !pending_change.is_some_and(|pending| {
                                pending.surface == pane.session.surface_identity()
                            })
                        {
                            pane.status = if pane.session.controlling() {
                                "Controlling"
                            } else {
                                "Observing"
                            }
                            .into();
                        }
                    }
                    FeedStatus::Reconnecting => {
                        pane.initial_claim = false;
                        pane.pending_paste = None;
                        pane.status = "Reconnecting; request Control again after reconnect".into();
                        dirty = true;
                    }
                    FeedStatus::Ended => {
                        pane.pending_paste = None;
                        pane.status = "Session ended; retained history available".into();
                        dirty = true;
                    }
                    FeedStatus::Rejected | FeedStatus::ReceiveLimited => {
                        for pane in &mut workspace.panes {
                            pane.shown = None;
                            pane.latest = None;
                            pane.painted = None;
                            pane.pending_paste = None;
                        }
                        execute!(io::stdout(), Clear(ClearType::All))?;
                        if update.status == FeedStatus::ReceiveLimited {
                            return Err(superplexr_client::RECEIVE_LIMIT_MESSAGE.into());
                        }
                        return Err(
                            "Session access ended; detached without stopping processes".into()
                        );
                    }
                }
            }
        }
        for pane in &mut workspace.panes {
            if let Some(message) = pane.session.take_input_message() {
                // A late acknowledgement (for example the preceding Control
                // claim) must not hide a paste confirmation already on screen.
                // Loss of authority still cancels that pending confirmation.
                if pane.pending_paste.is_some() && pane.session.controlling() {
                    continue;
                }
                pane.pending_paste = None;
                pane.status = message;
                dirty = true;
            }
        }
        if let Some(pending) = pending_change {
            let current = workspace.panes.get(workspace.active).is_some_and(|pane| {
                pane.session.surface_identity() == pending.surface
                    && pane.continuity == pending.continuity
            }) && nav.is_none()
                && search.is_none();
            if !current {
                pending_change = None;
                if let Some(pane) = workspace.panes.get_mut(workspace.active) {
                    pane.status = "View change cancelled after connection or selection change; request it again explicitly".into();
                }
                dirty = true;
            } else if !router.waiting() {
                match workspace.panes[workspace.active].session.return_state() {
                    ReturnState::Released => {
                        pending_change = None;
                        if let Err(error) = apply_view_change(
                            workspace,
                            pending.change,
                            client,
                            &mut nav,
                            &mut purpose,
                            &mut navigation_worker,
                        ) && let Some(pane) = workspace.panes.get_mut(workspace.active)
                        {
                            pane.status = error.to_string();
                        }
                        refresh_at = Instant::now();
                        clear = true;
                        dirty = true;
                    }
                    ReturnState::Unconfirmed => {
                        pending_change = None;
                        workspace.panes[workspace.active].status = "View change cancelled: Control unconfirmed; ^] r retries a known return, or ^] d detaches safely before reconnect".into();
                        dirty = true;
                    }
                    ReturnState::Pending => {}
                }
            }
        }
        if let (Some(request), Some(worker)) = (history_page, &search_worker)
            && let Some(update) = worker.take_update()
        {
            history_page = None;
            if let Some(pane) = workspace.panes.get_mut(workspace.active)
                && workspace.active == request.pane
                && pane.id == request.session
                && pane.continuity == request.continuity
                && matches!(pane.feed, FeedStatus::Live | FeedStatus::Ended)
                && nav.is_none()
                && search.is_none()
            {
                match update {
                    SearchUpdate::History(frame) => {
                        pane.shown = Some(frame);
                        pane.paused = true;
                        pane.painted = None;
                        pane.offset = request.offset;
                        pane.status =
                            format!("History: {} rows before bottom; ^] l live", request.offset);
                    }
                    SearchUpdate::Error(error) => {
                        pane.status = format!("History failed: {error}; ^] h retry or l live");
                    }
                    SearchUpdate::Page(_) => {
                        pane.status = "History request superseded; ^] h retry or l live".into();
                    }
                }
                dirty = true;
            }
        }
        if let (Some(panel), Some(worker)) = (&mut search, &search_worker)
            && let Some(update) = worker.take_update()
        {
            dirty = true;
            match update {
                SearchUpdate::Page(page) => {
                    if !panel.page(page) {
                        worker.cancel();
                    }
                }
                SearchUpdate::Error(error) => {
                    panel.loading_history = false;
                    panel.note = format!("{error}; / edit query and Enter to retry");
                }
                SearchUpdate::History(frame) => {
                    if let Some(pane) = workspace.panes.get_mut(workspace.active)
                        && pane.id == panel.session_id
                    {
                        pane.shown = Some(frame);
                        pane.paused = true;
                        pane.painted = None;
                        pane.offset = 0;
                        pane.status = "Search history (row positions may change); ^] l live".into();
                    }
                    search = None;
                    clear = true;
                }
            }
        }
        if attaching
            && nav.is_some()
            && let Some(result) = attach_worker
                .as_ref()
                .and_then(|worker| worker.finish(workspace))
        {
            attaching = false;
            match result {
                Ok(()) => {
                    nav = None;
                    clear = true;
                    group_action_note.clear();
                }
                Err(superplexr_tui::Error::Client(
                    error @ (ClientError::Remote { .. } | ClientError::ReceiveLimit { .. }),
                )) => {
                    execute!(io::stdout(), Clear(ClearType::All))?;
                    return Err(error.into());
                }
                Err(error) => group_action_note = format!("Open failed: {error}; Enter to retry"),
            }
            dirty = true;
        }
        if attaching && nav.is_none() {
            if let Some(worker) = &attach_worker {
                worker.cancel();
            }
            attaching = false;
        }
        if let Some(result) = group_worker.as_mut().and_then(GroupWorker::take) {
            group_action_note = match result {
                Ok(message) | Err(message) => message,
            };
            if let Some(navigator) = &mut nav
                && !navigator.workflow_active()
                && navigator.verifier_edit.is_none()
            {
                refresh(navigator, client, &mut navigation_worker);
            }
            dirty = true;
        }
        if let Some(navigator) = &mut nav {
            navigator.workflow_read = workflow_read;
            if navigator.action_note != group_action_note {
                navigator.action_note.clone_from(&group_action_note);
                dirty = true;
            }
            if let Some(result) = navigation_worker.as_ref().and_then(NavigationWorker::take) {
                match result {
                    Ok(entries) => dirty |= navigator.apply(entries),
                    Err(ClientError::Remote { .. }) if navigator.workflow_active() => {
                        navigator.entries.clear();
                        navigator.note = "Verifier unavailable or outside this Share's Mission scope; r retries explicitly.".into();
                        dirty = true;
                    }
                    Err(
                        error @ (ClientError::Remote { .. } | ClientError::ReceiveLimit { .. }),
                    ) => {
                        execute!(io::stdout(), Clear(ClearType::All))?;
                        return Err(error.into());
                    }
                    Err(error) if navigator.workflow_active() => {
                        navigator.entries.clear();
                        navigator.note = if matches!(error, ClientError::Io(ref error) if error.kind() == io::ErrorKind::Unsupported) {
                            "Runtime lacks this compact verification read; update it before retrying. W allows manual UUID entry."
                        } else {
                            "Verification status was not confirmed; r retries explicitly. No workflow action was sent."
                        }.into();
                        dirty = true;
                    }
                    Err(error) => {
                        navigator.note = format!("Refresh failed: {error}; r to retry");
                        dirty = true;
                    }
                }
                refresh_at = Instant::now();
            }
            if navigator.group_edit.is_none()
                && navigator.verifier_edit.is_none()
                && !navigator.workflow_active()
                && refresh_at.elapsed() >= Duration::from_secs(2)
            {
                if let Some(worker) = &navigation_worker {
                    if !worker.busy() {
                        worker.request(navigator.tab, navigator.mission, navigator.group);
                    }
                } else {
                    refresh(navigator, client, &mut navigation_worker);
                    dirty = true;
                }
                refresh_at = Instant::now();
            }
        } else if let Some(worker) = &navigation_worker {
            worker.cancel();
        }
        if dirty {
            if let Some(panel) = &search {
                render::draw_search(&mut io::stdout(), panel, size)?;
            } else if let Some(nav) = &nav {
                render::draw_navigator(
                    &mut io::stdout(),
                    nav,
                    size,
                    matches!(purpose, Open::Split(_) | Open::Duplicate(_)),
                )?;
            } else {
                if clear {
                    execute!(io::stdout(), Clear(ClearType::All))?;
                    workspace.invalidate();
                    clear = false;
                }
                let mut layout = workspace.layout(size);
                // Cursor belongs only to the focused pane; paint it last.
                layout.sort_by_key(|(index, _)| *index == workspace.active);
                let split = workspace.panes.len() > 1;
                for (index, rect) in layout {
                    let pane = &mut workspace.panes[index];
                    let mode = if pane.paused {
                        "PAUSED"
                    } else if pane.session.controlling() {
                        "CONTROL"
                    } else {
                        "OBSERVE"
                    };
                    // Focus and Control identity remain visible in split view,
                    // including while showing prefix help from an earlier visit.
                    let footer = if split {
                        format!(
                            "{} {} {mode} | {}",
                            if index == workspace.active { ">" } else { " " },
                            &pane.id.to_string()[..8],
                            pane.status
                        )
                    } else if pane.status == HELP || pane.status == INPUT_HELP {
                        pane.status.clone()
                    } else {
                        format!(
                            "{mode} | {} | ^] / search; n navigate; d detach",
                            pane.status
                        )
                    };
                    render::draw_pane(
                        &mut io::stdout(),
                        pane.shown.as_deref(),
                        pane.painted.as_deref(),
                        rect,
                        &footer,
                        index == workspace.active && !pane.paused && pane.feed == FeedStatus::Live,
                    )?;
                    pane.painted = pane.shown.clone();
                }
            }
            dirty = false;
        }
        if !event::poll(Duration::from_millis(40))? {
            continue;
        }
        let event = event::read()?;
        if let Event::Resize(columns, rows) = event {
            size = (columns, rows);
            clear = true;
            dirty = true;
            let grid = workspace.active_grid(size);
            if nav.is_none()
                && search.is_none()
                && let Some(pane) = workspace.panes.get_mut(workspace.active)
                && (pane.session.controlling() || pane.session.claim_pending())
                && let Err(error) = pane.session.queue_input(InputCommand::Resize(grid))
            {
                pane.status = error.to_string();
            }
            continue;
        }
        if let Some(navigator) = &mut nav {
            if attaching && matches!(&event, Event::Paste(_)) {
                if let Some(worker) = &attach_worker {
                    worker.cancel();
                }
                attaching = false;
                group_action_note = "Opening cancelled; no process was stopped".into();
                dirty = true;
            }
            if let Event::Paste(text) = &event
                && let Some(draft) = &mut navigator.verifier_edit
            {
                draft.append(text);
                dirty = true;
                continue;
            }
            if let Event::Paste(text) = &event
                && let Some(draft) = &mut navigator.group_edit
            {
                draft.append(text);
                dirty = true;
                continue;
            }
            if let Event::Key(key) = event
                && key.kind != KeyEventKind::Release
            {
                dirty = true;
                let submitting_session = !navigator.editing
                    && navigator.group_edit.is_none()
                    && navigator.verifier_edit.is_none()
                    && matches!(key.code, KeyCode::Enter | KeyCode::Char('D'))
                    && navigator
                        .current()
                        .is_some_and(|entry| matches!(entry.target, Target::Session(_)));
                if attaching && !submitting_session {
                    if let Some(worker) = &attach_worker {
                        worker.cancel();
                    }
                    attaching = false;
                    group_action_note = "Opening cancelled; no process was stopped".into();
                    if key.code == KeyCode::Esc {
                        continue;
                    }
                }
                if navigator.verifier_edit.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            navigator.verifier_edit = None;
                            navigator.note =
                                "Inspection entry cancelled. r refreshes the current view.".into();
                        }
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            navigator.verifier_edit = None;
                            navigator.note =
                                "Inspection entry cancelled. r refreshes the current view.".into();
                        }
                        KeyCode::Enter => {
                            if let Some(draft) = &navigator.verifier_edit {
                                match draft.inspection() {
                                    Ok(inspection) => {
                                        navigator.open_verifier(inspection);
                                        navigator.verifier_edit = None;
                                        refresh(navigator, client, &mut navigation_worker);
                                    }
                                    Err(message) => navigator.note = message.into(),
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            if let Some(draft) = &mut navigator.verifier_edit {
                                draft.backspace();
                            }
                        }
                        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                            if let Some(draft) = &mut navigator.verifier_edit {
                                draft.clear();
                            }
                        }
                        KeyCode::Char(character)
                            if !key.modifiers.intersects(
                                KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                            ) =>
                        {
                            if let Some(draft) = &mut navigator.verifier_edit {
                                draft.append(&character.to_string());
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if navigator.group_edit.is_some() {
                    match key.code {
                        KeyCode::Esc => navigator.group_edit = None,
                        KeyCode::Enter => {
                            if let Some(draft) = &navigator.group_edit {
                                match draft.action() {
                                    Ok(action) => {
                                        navigator.group_edit = None;
                                        group_action_note =
                                            submit_group_action(client, &mut group_worker, action);
                                    }
                                    Err(error) => group_action_note = error.into(),
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            if let Some(draft) = &mut navigator.group_edit {
                                draft.backspace();
                            }
                        }
                        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                            if let Some(draft) = &mut navigator.group_edit {
                                draft.clear();
                            }
                        }
                        KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => {
                            navigator.group_edit = None
                        }
                        KeyCode::Char(character)
                            if !key.modifiers.intersects(
                                KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                            ) =>
                        {
                            if let Some(draft) = &mut navigator.group_edit {
                                draft.append(&character.to_string());
                            }
                        }
                        _ => {}
                    }
                    continue;
                }
                if navigator.editing {
                    match key.code {
                        KeyCode::Enter | KeyCode::Esc => navigator.editing = false,
                        KeyCode::Backspace => {
                            navigator.query.pop();
                            navigator.selected = 0;
                        }
                        KeyCode::Char(c) if !c.is_control() && navigator.query.len() < 256 => {
                            navigator.query.push(c);
                            navigator.selected = 0;
                        }
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Esc => {
                        if workspace.panes.is_empty() {
                            break;
                        }
                        nav = None;
                        clear = true;
                    }
                    KeyCode::Char('q') => break,
                    KeyCode::Down | KeyCode::Char('j') => navigator.step(true),
                    KeyCode::Up | KeyCode::Char('k') => navigator.step(false),
                    KeyCode::Char('/') => navigator.editing = true,
                    KeyCode::Char('w' | 'W') if workflow_read => {
                        let mission = navigator.inspection.map(|inspection| inspection.mission)
                            .or(navigator.mission)
                            .or_else(|| navigator.current().and_then(|entry| match &entry.target {
                                Target::Mission(id) => Some(*id),
                                _ => None,
                            }));
                        if let Some(mission) = mission {
                            if key.code == KeyCode::Char('W') {
                                if let Some(worker) = &navigation_worker { worker.cancel(); }
                                navigator.verifier_edit = Some(superplexr_tui::workflow::Draft::new(mission));
                                navigator.note = "Enter an existing verifier Run UUID; inspection cannot execute or accept work.".into();
                            } else {
                                navigator.open_catalog(superplexr_tui::workflow::CatalogPage { mission, after: None });
                                refresh(navigator, client, &mut navigation_worker);
                            }
                            group_action_note.clear();
                        } else {
                            navigator.note = "Select a Mission in the Missions view, or enter one, before pressing w.".into();
                        }
                    }
                    KeyCode::Char('n') if navigator.catalog.is_some() && navigator.inspection.is_none() => {
                        if let Some(page) = navigator.next_verifier_page() {
                            navigator.open_catalog(page);
                            refresh(navigator, client, &mut navigation_worker);
                        } else {
                            navigator.note = "No next page in this observation; r refreshes, 0 reads the first page.".into();
                        }
                    }
                    KeyCode::Char('0') if navigator.catalog.is_some() && navigator.inspection.is_none() => {
                        if let Some(mut page) = navigator.catalog {
                            page.after = None;
                            navigator.open_catalog(page);
                            refresh(navigator, client, &mut navigation_worker);
                        }
                    }
                    KeyCode::Char('D')
                        if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
                    {
                        match navigator.current().map(|entry| entry.target.clone()) {
                            Some(Target::Group(group)) => {
                                navigator.group = Some(group.group_id);
                                navigator.mission = None;
                                navigator.tab = Tab::Sessions;
                                navigator.query.clear();
                                navigator.selected = 0;
                                purpose = Open::Duplicate(Axis::Vertical);
                                group_action_note = "Choose a Session for another view; no group or process will be duplicated".into();
                                refresh(navigator, client, &mut navigation_worker);
                            }
                            Some(Target::Session(id)) => {
                                let (started, note) = begin_attach(client, &mut attach_worker, id, Open::Duplicate(Axis::Vertical));
                                attaching |= started; group_action_note = note;
                            }
                            _ => group_action_note = "Select a Session or group to open another view".into(),
                        }
                    }
                    KeyCode::F(2 | 3) | KeyCode::Char('p')
                        if !client.is_shared() && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
                    {
                        if group_worker.as_ref().is_some_and(GroupWorker::busy) {
                            group_action_note = "A group action is pending; wait before editing another group".into();
                            continue;
                        }
                        let selected = navigator.current().and_then(|entry| match &entry.target {
                            Target::Group(group) => Some(group.clone()),
                            _ => None,
                        });
                        if let Some(group) = selected {
                            match key.code {
                                KeyCode::F(2) => navigator.group_edit = Some(GroupEdit::new(group, GroupField::Name)),
                                KeyCode::F(3) => navigator.group_edit = Some(GroupEdit::new(group, GroupField::Position)),
                                KeyCode::Char('p') => group_action_note = submit_group_action(client, &mut group_worker, GroupAction::Pin(group)),
                                _ => {}
                            }
                        } else {
                            group_action_note = "Select a group in the Groups view first (g)".into();
                        }
                    }
                    KeyCode::Tab => {
                        navigator.inspection = None;
                        navigator.catalog = None;
                        navigator.tab = navigator.tab.next();
                        if client.is_shared() && navigator.tab == Tab::Groups {
                            navigator.tab = navigator.tab.next();
                        }
                        navigator.mission = None;
                        navigator.group = None;
                        navigator.query.clear();
                        navigator.selected = 0;
                        refresh(navigator, client, &mut navigation_worker);
                    }
                    KeyCode::Backspace => {
                        if navigator.inspection.take().is_none() && navigator.catalog.take().is_none() {
                            if navigator.group.take().is_some() { navigator.tab = Tab::Groups; }
                            navigator.mission = None;
                        }
                        navigator.query.clear();
                        navigator.selected = 0;
                        refresh(navigator, client, &mut navigation_worker);
                    }
                    KeyCode::Char('r') => refresh(navigator, client, &mut navigation_worker),
                    KeyCode::Char('g') if !client.is_shared() => {
                        navigator.inspection = None;
                        navigator.catalog = None;
                        navigator.tab = Tab::Groups;
                        navigator.group = None;
                        navigator.query.clear();
                        navigator.selected = 0;
                        refresh(navigator, client, &mut navigation_worker);
                    }
                    KeyCode::Enter if navigator.inspection.is_some() => {
                        navigator.note = "Read-only inspection: no launch, attachment, collection or acceptance actions. r refreshes.".into();
                    }
                    KeyCode::Enter if size.1 < 7 => {
                        navigator.note =
                            "Enlarge the terminal to at least 7 rows to choose an item.".into()
                    }
                    KeyCode::Enter => match navigator.current().map(|entry| entry.target.clone()) {
                        Some(Target::Verifier(inspection)) => {
                            navigator.open_verifier(inspection);
                            refresh(navigator, client, &mut navigation_worker);
                        }
                        Some(Target::VerifierPage(page)) => {
                            navigator.open_catalog(page);
                            refresh(navigator, client, &mut navigation_worker);
                        }
                        Some(Target::Mission(id)) => {
                            navigator.mission = Some(id);
                            navigator.group = None;
                            navigator.tab = Tab::Sessions;
                            navigator.query.clear();
                            navigator.selected = 0;
                            refresh(navigator, client, &mut navigation_worker);
                        }
                        Some(Target::Group(group)) => {
                            navigator.group = Some(group.group_id);
                            navigator.mission = None;
                            navigator.tab = Tab::Sessions;
                            navigator.query.clear();
                            navigator.selected = 0;
                            refresh(navigator, client, &mut navigation_worker);
                        }
                        Some(Target::Session(id)) => {
                            let (started, note) = begin_attach(client, &mut attach_worker, id, purpose);
                            attaching |= started; group_action_note = note;
                        }
                        Some(Target::Unavailable) => {
                            navigator.note = if navigator.catalog.is_some() {
                                "Read-only verifier discovery; select a verifier, n next, or W enter a UUID."
                            } else {
                                "No accessible primary Session. Attention remains unresolved."
                            }.into()
                        }
                        None => {}
                    },
                    _ => {}
                }
            }
            continue;
        }
        let mut routed_action = None;
        if let (Some(panel), Some(worker)) = (&mut search, &search_worker) {
            match &event {
                Event::Paste(text) => {
                    if panel.editing {
                        panel.append(text);
                    }
                    dirty = true;
                    continue;
                }
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    dirty = true;
                    let action = router.route(*key);
                    match action {
                        Action::Help => {
                            panel.note = "^] d detach | o focus | n navigate | Esc back".into();
                            continue;
                        }
                        Action::Key(_) => {
                            if key.code == KeyCode::Esc {
                                worker.cancel();
                                search = None;
                                clear = true;
                            } else {
                                search_key(panel, worker, client, *key, size);
                            }
                            continue;
                        }
                        Action::None => continue,
                        action => {
                            worker.cancel();
                            search = None;
                            clear = true;
                            routed_action = Some(action);
                        }
                    }
                }
                _ => continue,
            }
        }
        let grid = workspace.active_grid(size);
        let Some(pane) = workspace.panes.get_mut(workspace.active) else {
            nav = Some(Navigator::default());
            continue;
        };
        match event {
            Event::Paste(text) => {
                if pending_change.take().is_some() {
                    pane.status =
                        "View change cancelled; paste was not sent and input remains disabled"
                            .into();
                } else {
                    paste(pane, text);
                }
                dirty = true;
            }
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                dirty = true;
                if confirm_paste(pane, key) {
                    continue;
                }
                let action = routed_action.unwrap_or_else(|| router.route(key));
                if pending_change.is_some() && key.code == KeyCode::Esc {
                    pending_change = None;
                    router = Router::default();
                    pane.status =
                        "View change cancelled; Control return continues, input remains disabled"
                            .into();
                    continue;
                }
                if pending_change.is_some() && matches!(action, Action::Key(_)) {
                    pane.status =
                        "Returning Control before changing views… Esc cancels; input is disabled"
                            .into();
                    continue;
                }
                if pending_change.is_some()
                    && ViewChange::from_action(&action).is_none()
                    && !matches!(action, Action::Help | Action::Copy | Action::None)
                {
                    pending_change = None;
                }
                if matches!(
                    action,
                    Action::Detach
                        | Action::Search
                        | Action::Live
                        | Action::Pause
                        | Action::Navigate
                        | Action::Attention
                        | Action::SplitVertical
                        | Action::SplitHorizontal
                        | Action::NextPane
                        | Action::ClosePane
                ) && history_page.take().is_some()
                    && let Some(worker) = &search_worker
                {
                    worker.cancel();
                }
                if let Some(change) = ViewChange::from_action(&action) {
                    let pending = PendingViewChange {
                        surface: pane.session.surface_identity(),
                        continuity: pane.continuity,
                        change,
                    };
                    if pending_change.is_some_and(|old| {
                        old.surface == pending.surface && old.continuity == pending.continuity
                    }) && pane.session.return_state() == ReturnState::Pending
                    {
                        pending_change = Some(pending);
                        pane.status =
                            "Updated pending view change; returning Control… Esc cancels".into();
                        continue;
                    }
                    pane.pending_paste = None;
                    pane.initial_claim = false;
                    match pane.session.release_control() {
                        Ok(()) => {
                            pending_change = None;
                            if let Err(error) = apply_view_change(
                                workspace,
                                change,
                                client,
                                &mut nav,
                                &mut purpose,
                                &mut navigation_worker,
                            ) && let Some(pane) = workspace.panes.get_mut(workspace.active)
                            {
                                pane.status = error.to_string();
                            }
                            refresh_at = Instant::now();
                            clear = true;
                        }
                        Err(superplexr_tui::Error::ReleasePending) => {
                            pending_change = Some(pending);
                            pane.status = "Returning Control before changing views… Esc cancels; input is disabled".into();
                        }
                        Err(error) => {
                            pending_change = None;
                            pane.status = error.to_string();
                        }
                    }
                    continue;
                }
                match action {
                    Action::Detach => break,
                    Action::History => {
                        if matches!(pane.feed, FeedStatus::Live | FeedStatus::Ended) {
                            if search_worker.is_none() {
                                search_worker = Some(Worker::new()?);
                            }
                            let offset = history_page
                                .filter(|request| {
                                    request.pane == workspace.active
                                        && request.session == pane.id
                                        && request.continuity == pane.continuity
                                })
                                .map_or(pane.offset, |request| request.offset)
                                .saturating_add(u32::from(grid.rows))
                                .min(100_000);
                            pane.paused = true;
                            let _ = pane.session.release_control();
                            pane.pending_paste = None;
                            pane.initial_claim = false;
                            pane.status = format!(
                                "Loading history: {offset} rows before bottom; ^] l cancels"
                            );
                            history_page = Some(HistoryPageRequest {
                                pane: workspace.active,
                                session: pane.id,
                                continuity: pane.continuity,
                                offset,
                            });
                            if let Some(worker) = &search_worker {
                                worker.history_offset(client.terminal(pane.id), offset);
                            }
                        } else {
                            pane.status =
                                "Wait for the session to reconnect before paging history".into();
                        }
                    }
                    Action::Search => {
                        if matches!(pane.feed, FeedStatus::Live | FeedStatus::Ended) {
                            if search_worker.is_none() {
                                search_worker = Some(Worker::new()?);
                            }
                            pane.pending_paste = None;
                            pane.initial_claim = false;
                            let _ = pane.session.release_control();
                            search = Some(Panel::new(pane.id));
                        } else {
                            pane.status =
                                "Wait for the session to reconnect before searching".into();
                        }
                    }
                    action => pane_action(pane, action, grid, osc52)?,
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn apply_view_change(
    workspace: &mut Workspace,
    change: ViewChange,
    client: &ControlClient,
    nav: &mut Option<Navigator>,
    purpose: &mut Open,
    navigation_worker: &mut Option<NavigationWorker>,
) -> Result<(), superplexr_tui::Error> {
    match change {
        ViewChange::Navigate { tab, mode } => {
            workspace.release_active()?;
            let mut navigator = Navigator {
                tab,
                ..Default::default()
            };
            refresh(&mut navigator, client, navigation_worker);
            *nav = Some(navigator);
            *purpose = mode;
        }
        ViewChange::Next => workspace.next_pane()?,
        ViewChange::Close => {
            workspace.close_active()?;
            if workspace.panes.is_empty() {
                let mut navigator = Navigator::default();
                refresh(&mut navigator, client, navigation_worker);
                *nav = Some(navigator);
                *purpose = Open::Replace;
            }
        }
    }
    Ok(())
}

fn search_key(
    panel: &mut Panel,
    worker: &Worker,
    client: &ControlClient,
    key: KeyEvent,
    size: (u16, u16),
) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        worker.cancel();
        panel.loading_history = false;
        panel.note = format!(
            "Search cancelled: {} partial results; / edit query",
            panel.matches.len()
        );
        return;
    }
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return;
    }
    if panel.editing {
        match key.code {
            KeyCode::Char(c) => panel.append(&c.to_string()),
            KeyCode::Backspace => {
                panel.query.pop();
            }
            KeyCode::Enter if !panel.query.is_empty() => {
                panel.reset();
                panel.editing = false;
                panel.note = "Searching; Ctrl-C cancels, Esc returns to terminal".into();
                worker.search(client.terminal(panel.session_id), panel.query.clone());
            }
            _ => {}
        }
    } else {
        match key.code {
            KeyCode::Char('/') => {
                worker.cancel();
                panel.reset();
                panel.editing = true;
                panel.note = "Edit query; Enter starts a new search".into();
            }
            KeyCode::Down | KeyCode::Char('j') => panel.step(true),
            KeyCode::Up | KeyCode::Char('k') => panel.step(false),
            KeyCode::Enter if size.1 < 5 => {
                panel.note = "Enlarge terminal to at least 5 rows to choose a result".into();
            }
            KeyCode::Enter if !panel.loading_history => {
                if let Some(found) = panel.matches.get(panel.selected) {
                    worker.history(client.terminal(panel.session_id), found.clone());
                    panel.loading_history = true;
                    panel.note = "Loading history; Esc cancels".into();
                }
            }
            _ => {}
        }
    }
}

fn begin_attach(
    client: &ControlClient,
    worker: &mut Option<AttachWorker>,
    id: superplexr_core::SessionId,
    mode: Open,
) -> (bool, String) {
    if worker.is_none() {
        match AttachWorker::spawn(client.clone()) {
            Ok(created) => *worker = Some(created),
            Err(error) => return (false, format!("Unable to start attachment worker: {error}")),
        }
    }
    if worker
        .as_ref()
        .is_some_and(|worker| worker.request(id, mode))
    {
        (
            true,
            "Opening observing view… Esc cancels; no process will be created".into(),
        )
    } else {
        (
            false,
            "A view is opening or retiring; wait before opening another".into(),
        )
    }
}

fn submit_group_action(
    client: &ControlClient,
    worker: &mut Option<GroupWorker>,
    action: GroupAction,
) -> String {
    if worker.is_none() {
        match GroupWorker::new(client.clone()) {
            Ok(created) => *worker = Some(created),
            Err(error) => return format!("Could not start group action: {error}"),
        }
    }
    match worker.as_mut().map(|worker| worker.submit(action)) {
        Some(Ok(())) => {
            "Saving group change; one action at a time. Detaching does not undo an admitted write."
                .into()
        }
        Some(Err(error)) => error,
        None => "Group action worker unavailable".into(),
    }
}

fn refresh(nav: &mut Navigator, client: &ControlClient, worker: &mut Option<NavigationWorker>) {
    if worker.is_none() {
        match NavigationWorker::spawn(client.clone()) {
            Ok(created) => *worker = Some(created),
            Err(error) => {
                nav.note = format!("Could not start navigation worker: {error}");
                return;
            }
        }
    }
    if let Some(worker) = worker {
        let changed = if let Some(inspection) = nav.inspection {
            worker.inspect(inspection)
        } else if let Some(page) = nav.catalog {
            worker.catalog(page)
        } else {
            worker.request(nav.tab, nav.mission, nav.group)
        };
        if changed || nav.workflow_active() {
            nav.entries.clear();
            nav.selected = 0;
        }
        nav.note = "Refreshing; navigation and detach remain available.".into();
    }
}

fn paste(pane: &mut Pane, text: String) {
    if !pane.session.controlling() || pane.paused {
        pane.status = "Paste blocked in observe/paused mode".into();
    } else if text.len() > MAX_PASTE_BYTES {
        pane.status = "Paste exceeds 8 MiB limit".into();
    } else if risky_paste(&text) {
        pane.status = format!(
            "Paste {} bytes with controls/newlines? y confirms; any other key cancels",
            text.len()
        );
        pane.pending_paste = Some(text);
    } else {
        pane.status = match pane.session.queue_input(InputCommand::Paste {
            text,
            confirmed: false,
        }) {
            Ok(()) => "Paste queued; awaiting acknowledgement".into(),
            Err(error) => error.to_string(),
        };
    }
}
fn confirm_paste(pane: &mut Pane, key: KeyEvent) -> bool {
    let Some(text) = pane.pending_paste.take() else {
        return false;
    };
    pane.status = if key.code == KeyCode::Char('y') {
        match pane.session.queue_input(InputCommand::Paste {
            text,
            confirmed: true,
        }) {
            Ok(()) => "Paste queued; awaiting acknowledgement".into(),
            Err(error) => error.to_string(),
        }
    } else {
        "Paste cancelled".into()
    };
    true
}
fn pane_action(pane: &mut Pane, action: Action, grid: GridSize, osc52: bool) -> io::Result<()> {
    match action {
        Action::Claim => {
            if pane.paused {
                pane.status = "Return to live output before requesting Control (^] l)".into();
                return Ok(());
            }
            pane.status = match pane.session.queue_input(InputCommand::Claim(grid)) {
                Ok(()) => "Requesting Control… input stays disabled until acknowledged".into(),
                Err(error) => error.to_string(),
            }
        }
        Action::Release => {
            pane.status = match pane.session.release_control() {
                Ok(()) => "Control returned".into(),
                Err(error) => error.to_string(),
            }
        }
        Action::Pause => {
            if pane.paused {
                pane.live();
            } else {
                pane.paused = true;
                let _ = pane.session.release_control();
                pane.status =
                    "Paused locally; host continues. Use outer terminal selection to copy.".into();
            }
        }
        Action::Live => pane.live(),
        Action::Copy => {
            pane.status = if !osc52 {
                "Clipboard escape disabled; opt in with --osc52 or pause and select.".into()
            } else if let Some(frame) = &pane.shown {
                let text = render::copy_text(frame);
                if text.len() > 64 * 1024 {
                    "Visible text exceeds clipboard export limit (64 KiB)".into()
                } else {
                    write!(io::stdout(), "\x1b]52;c;{}\x07", STANDARD.encode(text))?;
                    io::stdout().flush()?;
                    "Copy requested from outer terminal (permission required)".into()
                }
            } else {
                "No frame to copy".into()
            };
        }
        Action::Help => {
            pane.status = if pane.status == HELP {
                INPUT_HELP
            } else {
                HELP
            }
            .into()
        }
        Action::Key(key) => {
            if pane.paused {
                pane.status = "Input blocked while paused; ^] l live".into();
            } else if let Err(error) = pane.session.queue_input(InputCommand::Key(key)) {
                pane.status = error.to_string();
            }
        }
        _ => {}
    }
    Ok(())
}
