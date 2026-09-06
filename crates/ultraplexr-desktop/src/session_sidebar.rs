use std::{collections::HashSet, path::PathBuf, time::Duration};

#[cfg(not(test))]
use crate::theme::SIGNAL;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, CursorStyle, FocusHandle, FontWeight,
    IntoElement, KeyDownEvent, MouseButton, MouseMoveEvent, ScrollHandle, Window, accesskit::Role,
    deferred, div, prelude::*, px, svg,
};
#[cfg(not(test))]
use ultraplexr_core::{AttentionKind, SessionId, SignalId};
use ultraplexr_protocol::SessionGroupId;

use crate::{
    SessionStatus, SessionView, UltraplexrDesktop,
    app_zoom::ui_size,
    history_search::{Hit as HistoryHit, Search as HistorySearch, Target as HistoryTarget},
    overlay_scrollbar::{AutoHideScrollbar, scroll_handle_thumb},
    pane_layout::{SPLITTER_THICKNESS, SidebarEdgeDrag, SplitDragPreview},
    theme::{
        ACTIVE, CHALK, DECK, FAULT, HAIRLINE, PANEL, PRODUCT_FONT, RELAY, TRACE, UI_FONT, rgb,
    },
};

const EXPANDED_WIDTH_RATIO: f32 = 0.18;
const EXPANDED_MIN_WIDTH: f32 = 196.0;
const EXPANDED_MAX_WIDTH: f32 = 224.0;
const COLLAPSED_WIDTH: f32 = 40.0;
const SESSION_ROW_HEIGHT: f32 = 46.0;
const TERMINAL_CHILD_HEIGHT: f32 = 34.0;
const EXPANSION_DURATION: Duration = Duration::from_millis(150);
pub(crate) const MAX_SESSION_NAME_CHARS: usize = 64;

#[cfg(test)]
#[path = "session_sidebar_search_tests.rs"]
mod search_tests;

#[cfg(test)]
#[path = "session_sidebar_zoom_tests.rs"]
mod zoom_tests;

fn history_hit_index(
    sessions: &[SessionView],
    hit: &HistoryHit,
    actual: Option<ultraplexr_core::SessionId>,
) -> Option<usize> {
    if actual != Some(hit.session_id) {
        return None;
    }
    sessions.iter().position(|session| {
        session.group_id == hit.group_id && session.terminals.contains(&hit.surface_index)
    })
}

/// Names the runtime assigns when it knows nothing better.
pub(crate) fn is_generated_session_name(name: &str) -> bool {
    name == "shell" || name.starts_with("shell-")
}

/// Drop a leading mark such as `✳ ` from a title: the row already draws the
/// agent's mark, so the text should not repeat it.
pub(crate) fn strip_leading_mark(title: &str) -> &str {
    let mut chars = title.char_indices();
    match (chars.next(), chars.next()) {
        (Some((_, first)), Some((rest, second)))
            if !first.is_alphanumeric() && second.is_whitespace() =>
        {
            title[rest..].trim_start()
        }
        _ => title,
    }
}

pub(crate) fn sidebar_width(viewport_width: f32, expanded: bool, custom: Option<f32>) -> f32 {
    if !expanded {
        COLLAPSED_WIDTH
    } else if let Some(width) = custom {
        crate::pane_layout::sidebar_drag_width(width, viewport_width)
    } else {
        (viewport_width * EXPANDED_WIDTH_RATIO).clamp(EXPANDED_MIN_WIDTH, EXPANDED_MAX_WIDTH)
    }
}

#[cfg(test)]
fn legacy_sidebar_width(viewport_width: f32, expanded: bool) -> f32 {
    if expanded {
        (viewport_width * EXPANDED_WIDTH_RATIO).clamp(EXPANDED_MIN_WIDTH, EXPANDED_MAX_WIDTH)
    } else {
        COLLAPSED_WIDTH
    }
}

/// Interaction state for the session navigator. The actual sessions remain owned by
/// the active workspace; this state only owns presentation concerns.
pub(crate) struct SessionSidebarState {
    query: String,
    history: Option<HistorySearch>,
    history_task: Option<gpui::Task<()>>,
    history_note: String,
    search_open: bool,
    menu_open: Option<usize>,
    rename: Option<SessionRenameDraft>,
    search_focus: FocusHandle,
    scroll: ScrollHandle,
    scrollbar: AutoHideScrollbar,
    expanded_sessions: HashSet<usize>,
}

impl SessionSidebarState {
    pub(crate) fn new(search_focus: FocusHandle) -> Self {
        Self {
            query: String::new(),
            history: None,
            history_task: None,
            history_note: String::new(),
            search_open: false,
            menu_open: None,
            rename: None,
            search_focus,
            scroll: ScrollHandle::new(),
            scrollbar: AutoHideScrollbar::default(),
            expanded_sessions: HashSet::new(),
        }
    }

    pub(crate) fn dismiss_session_overlays(&mut self) {
        self.cancel_history(true);
        self.menu_open = None;
        self.rename = None;
    }
    fn cancel_history(&mut self, clear: bool) {
        self.history_task = None;
        if let Some(search) = &mut self.history {
            search.cancel(clear);
        }
        self.history_note = if clear {
            String::new()
        } else {
            "Search cancelled".into()
        };
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionRenameDraft {
    index: usize,
    value: String,
}

fn session_matches(session: &SessionView, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || session.name.to_lowercase().contains(&query)
        || session.actor.to_lowercase().contains(&query)
        || session
            .status
            .label(&session.actor)
            .to_lowercase()
            .contains(&query)
}

fn visible_indices(sessions: &[SessionView], query: &str) -> Vec<usize> {
    sessions
        .iter()
        .enumerate()
        .filter_map(|(index, session)| session_matches(session, query).then_some(index))
        .collect()
}

fn session_expansion_key(session: &SessionView) -> Option<usize> {
    session.terminals.first().copied()
}

fn status_word(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Working => "working",
        SessionStatus::Waiting => "waiting",
        SessionStatus::Idle => "idle",
        SessionStatus::Closed => "closed",
        SessionStatus::Terminated => "terminated",
    }
}

fn agent_mark(
    kind: crate::provider_usage::ProviderKind,
    size: f32,
    token: crate::theme::ColorToken,
) -> AnyElement {
    div()
        .size(ui_size(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(ui_size(4.0))
        .bg(rgb(PANEL))
        .child(
            svg()
                .path(kind.icon_path())
                .size(ui_size((size - 5.0).max(8.0)))
                .text_color(rgb(token)),
        )
        .into_any_element()
}

impl UltraplexrDesktop {
    fn toggle_session_expansion(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(key) = self
            .workspace()
            .sessions
            .get(index)
            .and_then(session_expansion_key)
        else {
            return;
        };
        if !self.session_sidebar.expanded_sessions.insert(key) {
            self.session_sidebar.expanded_sessions.remove(&key);
        }
        self.session_sidebar.menu_open = None;
        cx.notify();
    }

    pub(crate) fn select_terminal_in_session(
        &mut self,
        session_index: usize,
        surface_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_member = self
            .workspace()
            .sessions
            .get(session_index)
            .is_some_and(|session| session.terminals.contains(&surface_index));
        if !is_member {
            return;
        }
        self.workspace_mut().selected_session = session_index;
        self.active_surface = Some(surface_index);
        if let Some(key) = self
            .workspace()
            .sessions
            .get(session_index)
            .and_then(session_expansion_key)
        {
            self.session_sidebar.expanded_sessions.insert(key);
        }
        self.session_sidebar.menu_open = None;
        #[cfg(not(test))]
        self.sync_surface_presentation(cx);
        if let Some(surface) = self.surfaces.get(surface_index) {
            window.focus(&surface.read(cx).focus_handle_owned(), cx);
        }
        self.persist_workspaces();
        cx.notify();
    }

    pub(crate) fn reveal_sidebar_scrollbar(&mut self, cx: &mut Context<Self>) {
        let generation = self.session_sidebar.scrollbar.reveal();
        cx.notify();
        self.schedule_sidebar_scrollbar_hide(generation, cx);
    }

    fn schedule_sidebar_scrollbar_hide(&mut self, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |desktop, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            let _ = desktop.update(cx, |desktop, cx| {
                if desktop
                    .session_sidebar
                    .scrollbar
                    .hide_if_current(generation)
                {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn reveal_sidebar_scrollbar_near_edge(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.session_sidebar.scroll.bounds();
        let near_edge =
            bounds.contains(&event.position) && event.position.x >= bounds.right() - px(12.0);
        if near_edge {
            if self.session_sidebar.scrollbar.enter_edge() {
                cx.notify();
            }
        } else if let Some(generation) = self.session_sidebar.scrollbar.leave_edge() {
            self.schedule_sidebar_scrollbar_hide(generation, cx);
        }
    }

    fn sidebar_scrollbar(&self) -> Option<AnyElement> {
        if !self.session_sidebar.scrollbar.visible() {
            return None;
        }
        let scroll = &self.session_sidebar.scroll;
        let track_height = (f32::from(scroll.bounds().size.height) - 8.0).max(0.0);
        let thumb = scroll_handle_thumb(
            track_height,
            f32::from(scroll.offset().y),
            f32::from(scroll.max_offset().y),
        )?;
        Some(
            div()
                .id("session-scrollbar")
                .debug_selector(|| "session-scrollbar".to_owned())
                .absolute()
                .top(px(4.0))
                .right(px(2.0))
                .w(px(3.0))
                .h(px(track_height))
                .rounded_full()
                .child(
                    div()
                        .absolute()
                        .top(px(thumb.top_px))
                        .w_full()
                        .h(px(thumb.height_px))
                        .rounded_full()
                        .bg(rgb(TRACE).alpha(0.78)),
                )
                .into_any_element(),
        )
    }

    #[cfg(not(test))]
    fn open_attention(
        &mut self,
        signal_id: SignalId,
        session_id: Option<SessionId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_attention = Some(signal_id);
        if let Some(session_id) = session_id {
            let selected = self.workspace().sessions.iter().position(|session| {
                session
                    .terminals
                    .iter()
                    .any(|index| self.surface_sessions.get(*index).copied() == Some(session_id))
            });
            if let Some(selected) = selected {
                self.workspace_mut().selected_session = selected;
                self.focus_active_terminal(window, cx);
            }
        }
        self.command_deck_open = false;
        cx.notify();
    }

    #[cfg(not(test))]
    fn attention_sidebar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mission = self.workspace().mission.as_ref()?;
        let items = mission.attention_queue();
        if items.is_empty() {
            return None;
        }

        Some(
            div()
                .mx_2()
                .mb_2()
                .rounded(ui_size(5.0))
                .border_1()
                .border_color(rgb(SIGNAL).alpha(0.5))
                .bg(rgb(PANEL))
                .overflow_hidden()
                .child(
                    div()
                        .h(ui_size(27.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .font_family(UI_FONT)
                        .text_size(ui_size(10.0))
                        .text_color(rgb(SIGNAL))
                        .child("Needs you")
                        .child(div().ml_auto().child(items.len().to_string())),
                )
                .children(items.into_iter().map(|item| {
                    let signal_id = item.signal_id;
                    let session_id = mission
                        .runs
                        .get(&item.run_id)
                        .and_then(|run| run.primary_session);
                    let kind = match item.kind {
                        AttentionKind::HighRiskApproval => "HIGH RISK",
                        AttentionKind::Escalation => "ESCALATION",
                        AttentionKind::Blocked => "BLOCKED",
                        AttentionKind::Approval => "APPROVAL",
                        AttentionKind::Input => "INPUT",
                    };
                    div()
                        .id(format!("attention-row-{signal_id}"))
                        .min_h(ui_size(44.0))
                        .px_2()
                        .py_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .border_t_1()
                        .border_color(rgb(HAIRLINE))
                        .hover(|row| row.bg(rgb(ACTIVE)))
                        .on_click(cx.listener(move |desktop, _, window, cx| {
                            desktop.open_attention(signal_id, session_id, window, cx);
                        }))
                        .child(div().size(ui_size(7.0)).rounded_full().bg(rgb(SIGNAL)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .font_family(PRODUCT_FONT)
                                        .text_color(rgb(CHALK))
                                        .child(item.summary),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(ui_size(9.0))
                                        .text_color(rgb(TRACE))
                                        .child(format!("{kind} · {}", item.actor.id)),
                                ),
                        )
                }))
                .into_any_element(),
        )
    }

    fn focus_sidebar_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = true;
        self.session_sidebar.menu_open = None;
        self.session_sidebar.rename = None;
        self.session_sidebar.search_open = true;
        window.focus(&self.session_sidebar.search_focus, cx);
        cx.notify();
    }

    fn toggle_sidebar_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_sidebar.search_open && self.session_sidebar.query.is_empty() {
            self.session_sidebar.cancel_history(true);
            self.session_sidebar.search_open = false;
            self.focus_active_terminal(window, cx);
            cx.notify();
        } else {
            self.focus_sidebar_search(window, cx);
        }
    }

    fn toggle_session_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_sidebar.menu_open == Some(index) {
            self.session_sidebar.menu_open = None;
            cx.notify();
            return;
        }
        self.select_session(index, window, cx);
        self.session_sidebar.menu_open = Some(index);
        cx.notify();
    }

    fn begin_session_rename(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.shared_mode {
            return;
        }
        let Some(name) = self
            .workspace()
            .sessions
            .get(index)
            .map(|session| session.name.clone())
        else {
            return;
        };
        self.select_session(index, window, cx);
        self.session_sidebar.search_open = false;
        self.session_sidebar.cancel_history(true);
        self.session_sidebar.menu_open = None;
        self.session_sidebar.rename = Some(SessionRenameDraft { index, value: name });
        window.focus(&self.session_sidebar.search_focus, cx);
        cx.notify();
    }

    fn cancel_session_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_sidebar.rename = None;
        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    fn commit_session_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rename) = self.session_sidebar.rename.take() else {
            return;
        };
        let name = rename.value.trim();
        #[cfg(not(test))]
        let mut renamed_group = None;
        if !name.is_empty()
            && let Some(session) = self.workspace_mut().sessions.get_mut(rename.index)
        {
            session.name = name.to_owned();
            #[cfg(not(test))]
            {
                session.automatic_name = false;
                renamed_group = Some(session.group_id);
            }
            self.persist_workspaces();
        }
        #[cfg(not(test))]
        if let Some(group_id) = renamed_group {
            self.sync_session_group(
                group_id,
                Some(ultraplexr_protocol::SessionGroupChange::Rename {
                    name: name.to_owned(),
                }),
                cx,
            );
        }
        self.focus_active_terminal(window, cx);
        cx.notify();
    }

    fn handle_session_rename_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        match key {
            "backspace" => {
                if let Some(rename) = self.session_sidebar.rename.as_mut() {
                    rename.value.pop();
                }
            }
            "escape" => self.cancel_session_rename(window, cx),
            "enter" => self.commit_session_rename(window, cx),
            _ if !event.keystroke.modifiers.control && !event.keystroke.modifiers.platform => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                    && let Some(rename) = self.session_sidebar.rename.as_mut()
                    && rename.value.chars().count() + text.chars().count() <= MAX_SESSION_NAME_CHARS
                {
                    rename.value.push_str(text);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn add_terminal_to_session(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_sidebar.menu_open = None;
        self.select_session(index, window, cx);
        self.add_terminal(window, cx);
    }

    fn request_session_termination_at(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.shared_mode {
            return;
        }
        self.workspace_mut().selected_session = index;
        self.session_sidebar.menu_open = None;
        self.request_session_termination(cx);
    }

    fn archive_session_from_menu(&mut self, index: usize, cx: &mut Context<Self>) {
        self.session_sidebar.menu_open = None;
        self.archive_session(index, cx);
    }

    /// Take a finished Session out of the sidebar.
    ///
    /// Nothing is destroyed: the terminals are archived and the group is
    /// detached, both of which are reversible. Without this a Session that has
    /// exited can only be archived, which leaves its row in place, so a long
    /// session of work ends with a sidebar no one can clear.
    /// Whether the termination confirmation is anchored to a visible row.
    ///
    /// It is not, if the sidebar is collapsed or a search has filtered the
    /// Session out. The centred dialog stays as the fallback for exactly those
    /// cases, so a confirmation can never be left with nowhere to appear.
    pub(crate) fn termination_is_anchored(&self) -> bool {
        let Some(index) = self.pending_termination else {
            return false;
        };
        self.sidebar_open
            && visible_indices(&self.workspace().sessions, &self.session_sidebar.query)
                .contains(&index)
    }

    /// Confirm terminating a Session, anchored to its own row.
    ///
    /// This used to dim the whole window behind a centred dialog, which hid the
    /// very list the choice is about and read as far heavier than the action.
    /// The confirmation belongs beside the row it acts on, like the menu it was
    /// opened from.
    fn session_termination_popover(
        &self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let force = self
            .workspace()
            .sessions
            .get(index)
            .map(|session| {
                session.terminals.iter().any(|surface_index| {
                    self.surfaces
                        .get(*surface_index)
                        .is_some_and(|surface| surface.read(cx).requires_force_kill())
                })
            })
            .unwrap_or(false);
        let count = self
            .workspace()
            .sessions
            .get(index)
            .map_or(0, |session| session.terminals.len());
        div()
            .id(("session-terminate-confirm", index))
            .debug_selector(move || format!("session-terminate-confirm-{index}"))
            .w(self.app_zoom.overlay_width(window, 228.0))
            .max_h(self.app_zoom.overlay_height(window, f32::MAX))
            .overflow_y_scroll()
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .rounded(ui_size(5.0))
            .border_1()
            .border_color(rgb(FAULT))
            .bg(rgb(DECK))
            .shadow_lg()
            .occlude()
            .text_size(ui_size(12.0))
            .text_color(rgb(CHALK))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .font_family(UI_FONT)
                    .text_color(rgb(FAULT))
                    .child(if force { "!" } else { "△" })
                    .child(if force {
                        "Force kill?".to_owned()
                    } else {
                        format!("Terminate {count}?")
                    }),
            )
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_size(ui_size(10.0))
                    .text_color(rgb(TRACE))
                    .child(if force {
                        "SIGKILL. Frames stay."
                    } else {
                        "SIGHUP → SIGTERM."
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_1()
                    .child(
                        div()
                            .id(("cancel-session-termination", index))
                            .role(Role::Button)
                            .aria_label("Cancel termination")
                            .h(ui_size(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .rounded(ui_size(3.0))
                            .border_1()
                            .border_color(rgb(HAIRLINE))
                            .text_color(rgb(TRACE))
                            .hover(|button| button.text_color(rgb(CHALK)))
                            .on_click(cx.listener(|desktop, _, _, cx| {
                                cx.stop_propagation();
                                desktop.pending_termination = None;
                                cx.notify();
                            }))
                            .child("Cancel"),
                    )
                    .child(
                        div()
                            .id(("confirm-session-termination", index))
                            .role(Role::Button)
                            .aria_label(if force {
                                "Force kill session"
                            } else {
                                "Terminate session"
                            })
                            .h(ui_size(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .rounded(ui_size(3.0))
                            .bg(rgb(FAULT))
                            .text_color(rgb(DECK))
                            .font_weight(FontWeight::SEMIBOLD)
                            .on_click(cx.listener(|desktop, _, _, cx| {
                                cx.stop_propagation();
                                desktop.confirm_session_termination(cx);
                            }))
                            .child(if force { "Kill" } else { "Terminate" }),
                    ),
            )
            .into_any_element()
    }

    fn remove_session_from_menu(&mut self, index: usize, cx: &mut Context<Self>) {
        self.session_sidebar.menu_open = None;
        if self.shared_mode {
            return;
        }
        #[cfg(not(test))]
        let group_id = self
            .workspace()
            .sessions
            .get(index)
            .map(|session| session.group_id);
        if !self.archive_session(index, cx) {
            cx.notify();
            return;
        }
        #[cfg(not(test))]
        if let Some(group_id) = group_id {
            self.sync_session_group(
                group_id,
                Some(ultraplexr_protocol::SessionGroupChange::SetDetached { detached: true }),
                cx,
            );
        }
        let workspace = self.workspace_mut();
        if index < workspace.sessions.len() {
            workspace.sessions.remove(index);
            if workspace.sessions.is_empty() {
                workspace.sessions.push(SessionView {
                    group_id: SessionGroupId::new(),
                    group_version: 0,
                    pinned: false,
                    automatic_name: false,
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
        self.persist_workspaces();
        cx.notify();
    }

    fn toggle_session_pin(&mut self, index: usize, cx: &mut Context<Self>) {
        self.session_sidebar.menu_open = None;
        #[cfg(not(test))]
        {
            let Some(session) = self.workspace_mut().sessions.get_mut(index) else {
                return;
            };
            session.pinned = !session.pinned;
            let group_id = session.group_id;
            let pinned = session.pinned;
            self.sync_session_group(
                group_id,
                Some(ultraplexr_protocol::SessionGroupChange::SetPinned { pinned }),
                cx,
            );
            self.persist_workspaces();
        }
        #[cfg(test)]
        let _ = index;
        cx.notify();
    }

    fn detach_session_group(&mut self, index: usize, cx: &mut Context<Self>) {
        self.session_sidebar.menu_open = None;
        #[cfg(not(test))]
        if let Some(session) = self.workspace().sessions.get(index) {
            self.sync_session_group(
                session.group_id,
                Some(ultraplexr_protocol::SessionGroupChange::SetDetached { detached: true }),
                cx,
            );
        }
        #[cfg(test)]
        let _ = index;
        cx.notify();
    }

    fn move_session(&mut self, index: usize, target: usize, cx: &mut Context<Self>) {
        let session_count = self.workspace().sessions.len();
        if index >= session_count || target >= session_count || index == target {
            return;
        }
        let selected = self.workspace().selected_session;
        let workspace = self.workspace_mut();
        workspace.sessions.swap(index, target);
        workspace.selected_session = if selected == index {
            target
        } else if selected == target {
            index
        } else {
            selected
        };
        self.session_sidebar.menu_open = Some(target);
        #[cfg(not(test))]
        {
            let changed = [
                self.workspace().sessions[index].group_id,
                self.workspace().sessions[target].group_id,
            ];
            for group_id in changed {
                self.sync_session_group(group_id, None, cx);
            }
        }
        self.persist_workspaces();
        cx.notify();
    }

    fn clear_sidebar_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_sidebar.query.clear();
        self.session_sidebar.cancel_history(true);
        self.focus_sidebar_search(window, cx);
    }

    fn refresh_history_search(&mut self, cx: &mut Context<Self>) {
        self.session_sidebar.cancel_history(true);
        let query = self.session_sidebar.query.trim().to_owned();
        if query.chars().count() < 2 {
            return;
        }
        let workspace = self.workspaces.active_id();
        if self.session_sidebar.history.is_none() {
            match HistorySearch::new(workspace) {
                Ok(search) => self.session_sidebar.history = Some(search),
                Err(error) => {
                    self.session_sidebar.history_note = error.to_string();
                    return;
                }
            }
        }
        self.session_sidebar.history_note = "Waiting for query…".into();
        self.session_sidebar.history_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let started = this.update(cx, |desktop, cx| {
                if desktop.workspaces.active_id() != workspace {
                    return false;
                }
                let targets = desktop.history_targets(cx);
                let Some(search) = &mut desktop.session_sidebar.history else {
                    return false;
                };
                search.start(workspace, query, targets);
                desktop.session_sidebar.history_note = search.note.clone();
                cx.notify();
                true
            });
            if !started.is_ok_and(|started| started) {
                return;
            }
            while this
                .update(cx, |desktop, cx| desktop.poll_history_search(cx))
                .unwrap_or(false)
            {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
            }
        }));
    }

    fn history_targets(&self, cx: &App) -> Vec<HistoryTarget> {
        let targets = self
            .workspace()
            .sessions
            .iter()
            .flat_map(|session| {
                session
                    .terminals
                    .iter()
                    .copied()
                    .map(move |surface_index| (session.group_id, surface_index))
            })
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();
        targets
            .into_iter()
            .filter_map(|(group_id, surface_index)| {
                let session = self
                    .surfaces
                    .get(surface_index)?
                    .read(cx)
                    .daemon_session()?;
                seen.insert(session.id()).then_some(HistoryTarget {
                    group_id,
                    surface_index,
                    session,
                })
            })
            .collect()
    }

    fn history_hit_session(&self, hit: &HistoryHit, cx: &App) -> Option<usize> {
        if self.session_sidebar.history.as_ref()?.workspace != self.workspaces.active_id() {
            return None;
        }
        history_hit_index(
            &self.workspace().sessions,
            hit,
            self.surfaces
                .get(hit.surface_index)?
                .read(cx)
                .daemon_session()
                .map(|session| session.id()),
        )
    }

    fn poll_history_search(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(search) = &mut self.session_sidebar.history else {
            return false;
        };
        if search.workspace != self.workspaces.active_id() || !self.session_sidebar.search_open {
            self.session_sidebar.cancel_history(true);
            cx.notify();
            return false;
        }
        let changed = search.poll();
        let preview = search.preview.take();
        let running = search.running;
        self.session_sidebar.history_note = search.note.clone();
        if let Some((hit, frame)) = preview
            && let Some(index) = self.history_hit_session(&hit, cx)
        {
            self.workspace_mut().selected_session = index;
            self.active_surface = Some(hit.surface_index);
            self.surfaces[hit.surface_index]
                .update(cx, |surface, cx| surface.show_search_history(frame, cx));
            #[cfg(not(test))]
            self.sync_surface_presentation(cx);
        }
        if changed {
            cx.notify();
        }
        running
    }

    fn reveal_history_hit(&mut self, hit: HistoryHit, window: &mut Window, cx: &mut Context<Self>) {
        if self.history_hit_session(&hit, cx).is_none() {
            return;
        }
        let Some(surface) = self.surfaces.get(hit.surface_index) else {
            return;
        };
        let Some(session) = surface.read(cx).daemon_session() else {
            return;
        };
        let focus = surface.read(cx).focus_handle_owned();
        let focus_hit = hit.clone();
        self.session_sidebar.history_task = None;
        let Some(search) = &mut self.session_sidebar.history else {
            return;
        };
        search.reveal(hit, session);
        self.session_sidebar.history_note = search.note.clone();
        self.session_sidebar.history_task = Some(cx.spawn_in(window, async move |this, cx| {
            while this
                .update(cx, |desktop, cx| desktop.poll_history_search(cx))
                .unwrap_or(false)
            {
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
            }
            // Focus only after the successful viewport read. Focusing before
            // it completes emits a selection change that cancels pending work.
            let valid = this
                .update(cx, |desktop, cx| {
                    desktop.history_hit_session(&focus_hit, cx).is_some()
                        && desktop
                            .session_sidebar
                            .history
                            .as_ref()
                            .is_some_and(|search| search.hits.contains(&focus_hit))
                })
                .unwrap_or(false);
            if valid {
                let _ = cx.update(|window, cx| window.focus(&focus, cx));
            }
        }));
        cx.notify();
    }

    fn move_filtered_session_selection(
        &mut self,
        offset: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let visible = visible_indices(&self.workspace().sessions, &self.session_sidebar.query);
        if visible.is_empty() {
            return;
        }

        let current = self.workspace().selected_session;
        let current_position = visible
            .iter()
            .position(|index| *index == current)
            .unwrap_or(0);
        let next = (current_position as isize + offset).rem_euclid(visible.len() as isize) as usize;
        self.select_session(visible[next], window, cx);
        window.focus(&self.session_sidebar.search_focus, cx);
    }

    fn handle_sidebar_search_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        match key {
            "backspace" => {
                self.session_sidebar.query.pop();
            }
            "escape" => {
                self.session_sidebar.cancel_history(true);
                if self.session_sidebar.query.is_empty() {
                    self.session_sidebar.search_open = false;
                    self.focus_active_terminal(window, cx);
                } else {
                    self.session_sidebar.query.clear();
                }
            }
            "up" => self.move_filtered_session_selection(-1, window, cx),
            "down" => self.move_filtered_session_selection(1, window, cx),
            "enter" => {
                if let Some(index) =
                    visible_indices(&self.workspace().sessions, &self.session_sidebar.query)
                        .first()
                        .copied()
                {
                    self.select_session(index, window, cx);
                }
            }
            _ if !event.keystroke.modifiers.control && !event.keystroke.modifiers.platform => {
                if let Some(text) = event.keystroke.key_char.as_deref()
                    && !text.chars().any(char::is_control)
                    && self.session_sidebar.query.len() + text.len() <= 1024
                {
                    self.session_sidebar.query.push_str(text);
                }
            }
            _ => return,
        }
        if !matches!(key, "up" | "down" | "enter" | "escape") {
            self.refresh_history_search(cx);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn terminal_history_results(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.session_sidebar.history_note.is_empty() {
            return None;
        }
        let hits = self
            .session_sidebar
            .history
            .as_ref()
            .map(|search| search.hits.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter(|hit| self.history_hit_session(hit, cx).is_some())
            .cloned()
            .collect::<Vec<_>>();
        let running = self
            .session_sidebar
            .history
            .as_ref()
            .is_some_and(|search| search.running)
            || self.session_sidebar.history_note == "Waiting for query…";
        Some(
            div()
                .id("terminal-history-results")
                .mx_2()
                .mb_2()
                .rounded(ui_size(5.0))
                .border_1()
                .border_color(rgb(HAIRLINE))
                .bg(rgb(PANEL))
                .max_h(ui_size(320.0))
                .overflow_y_scroll()
                .child(
                    div()
                        .h(ui_size(26.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .font_family(UI_FONT)
                        .text_size(ui_size(10.0))
                        .text_color(rgb(RELAY))
                        .child("Terminal history")
                        .child(
                            div()
                                .ml_auto()
                                .text_color(rgb(TRACE))
                                .child(hits.len().to_string()),
                        ),
                )
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(ui_size(10.0))
                        .text_color(rgb(TRACE))
                        .child(self.session_sidebar.history_note.clone()),
                )
                .when(running, |panel| {
                    panel.child(
                        div()
                            .id("cancel-history-search")
                            .debug_selector(|| "cancel-history-search".to_owned())
                            .role(Role::Button)
                            .aria_label("Cancel history search")
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .text_color(rgb(RELAY))
                            .text_xs()
                            .child("Cancel search")
                            .on_click(cx.listener(|desktop, _, _, cx| {
                                desktop.session_sidebar.cancel_history(false);
                                cx.notify();
                            })),
                    )
                })
                .children(hits.iter().cloned().enumerate().map(|(index, hit)| {
                    let session_name = self
                        .workspace()
                        .sessions
                        .iter()
                        .find(|session| session.group_id == hit.group_id)
                        .map(|session| session.name.clone())
                        .unwrap_or_default();
                    let selected_hit = hit.clone();
                    div()
                        .id(("terminal-history-hit", index))
                        .h(ui_size(42.0))
                        .px_2()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .cursor_pointer()
                        .border_t_1()
                        .border_color(rgb(HAIRLINE))
                        .hover(|row| row.bg(rgb(ACTIVE)))
                        .on_click(cx.listener(move |desktop, _, window, cx| {
                            desktop.reveal_history_hit(selected_hit.clone(), window, cx);
                        }))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .font_family(UI_FONT)
                                .text_xs()
                                .text_color(rgb(CHALK))
                                .child(hit.found.preview),
                        )
                        .child(
                            div()
                                .font_family(UI_FONT)
                                .text_size(ui_size(9.0))
                                .text_color(rgb(TRACE))
                                .child(format!(
                                    "{session_name} · line {}:{}",
                                    hit.found.line.saturating_add(1),
                                    hit.found.column.saturating_add(1)
                                )),
                        )
                }))
                .into_any_element(),
        )
    }

    pub(crate) fn terminal_agent(
        &self,
        surface_index: usize,
    ) -> Option<crate::provider_usage::ProviderKind> {
        self.surface_foreground_processes
            .get(surface_index)
            .and_then(Option::as_ref)
            .and_then(|process| {
                crate::provider_usage::ProviderKind::from_process_name(&process.executable)
            })
    }

    pub(crate) fn terminal_status(
        &self,
        _session_index: usize,
        surface_index: usize,
    ) -> SessionStatus {
        #[cfg(not(test))]
        {
            if self.surface_activity.contains_key(&surface_index) {
                return SessionStatus::Working;
            }
            match self.surface_statuses.get(surface_index) {
                Some(ultraplexr_protocol::TerminalSessionStatus::Running) => SessionStatus::Idle,
                Some(ultraplexr_protocol::TerminalSessionStatus::Failed) => {
                    SessionStatus::Terminated
                }
                Some(ultraplexr_protocol::TerminalSessionStatus::Exited) | None => {
                    SessionStatus::Closed
                }
            }
        }
        #[cfg(test)]
        {
            let _ = surface_index;
            self.workspace()
                .sessions
                .get(_session_index)
                .map_or(SessionStatus::Closed, |session| session.status)
        }
    }

    pub(crate) fn terminal_title(
        &self,
        surface_index: usize,
        agent: Option<crate::provider_usage::ProviderKind>,
        cx: &App,
    ) -> String {
        // What the program calls itself beats what we detected it to be: an
        // agent's own title names the task, its label only names the agent.
        match (self.surface_own_title(surface_index, cx), agent) {
            (Some(title), _) => title,
            (None, Some(agent)) => agent.label().to_owned(),
            (None, None) => format!("Terminal {}", surface_index + 1),
        }
    }

    /// The OSC title a surface's program set, without a leading mark.
    fn surface_own_title(&self, surface_index: usize, cx: &App) -> Option<String> {
        self.surfaces
            .get(surface_index)
            .and_then(|surface| surface.read(cx).terminal_title().map(str::to_owned))
            .map(|title| strip_leading_mark(title.trim()).to_owned())
            .filter(|title| !title.is_empty())
    }

    /// Where a surface's process is running: a live OSC 7 report when the
    /// shell sends one, otherwise the directory the runtime started it in.
    fn surface_directory(&self, surface_index: usize, cx: &App) -> Option<PathBuf> {
        #[cfg(not(test))]
        if let Some(live) = self.surfaces.get(surface_index).and_then(|surface| {
            surface
                .read(cx)
                .current_directory()
                .and_then(crate::worktree::path_from_osc7)
        }) {
            return Some(live);
        }
        #[cfg(test)]
        let _ = cx;
        self.surface_cwds.get(surface_index).cloned().flatten()
    }

    /// The repository and branch a Session runs in, from its first terminal
    /// with a known directory.
    pub(crate) fn session_worktree(
        &self,
        session: &SessionView,
        cx: &App,
    ) -> Option<crate::worktree::Worktree> {
        session
            .terminals
            .iter()
            .find_map(|surface_index| self.surface_directory(*surface_index, cx))
            .map(|directory| self.worktree_cache.describe(&directory))
    }

    /// The title an agent in this Session has set for itself, if any.
    fn session_agent_title(&self, session: &SessionView, cx: &App) -> Option<String> {
        session.terminals.iter().find_map(|surface_index| {
            self.terminal_agent(*surface_index)?;
            self.surface_own_title(*surface_index, cx)
        })
    }

    /// What to call a Session in the sidebar.
    ///
    /// A name a person chose always wins. Otherwise the agent's own title
    /// names the task, and failing that the worktree names the place; the
    /// runtime's generated name is only the last resort. This is derived at
    /// render time rather than written into the name, so it keeps following
    /// the terminal instead of freezing at the first thing it saw.
    pub(crate) fn session_display_title(&self, session: &SessionView, cx: &App) -> String {
        if !is_generated_session_name(&session.name) {
            return session.name.clone();
        }
        self.session_agent_title(session, cx)
            .or_else(|| {
                self.session_worktree(session, cx)
                    .map(|worktree| worktree.label())
            })
            .unwrap_or_else(|| session.name.clone())
    }

    fn session_primary_agent(
        &self,
        session: &SessionView,
    ) -> Option<crate::provider_usage::ProviderKind> {
        session
            .terminals
            .iter()
            .find_map(|surface_index| self.terminal_agent(*surface_index))
            .or_else(|| crate::provider_usage::ProviderKind::from_process_name(&session.actor))
    }

    fn terminal_quick_switch(
        &self,
        session_index: usize,
        surface_index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agent = self.terminal_agent(surface_index);
        let status = self.terminal_status(session_index, surface_index);
        let selected = self.workspace().selected_session == session_index
            && self.active_surface == Some(surface_index);
        let label = self.terminal_title(surface_index, agent, cx);
        let mark = agent.map_or_else(
            || {
                div()
                    .font_family(UI_FONT)
                    .text_size(ui_size(9.0))
                    .text_color(rgb(TRACE))
                    .child("›_")
                    .into_any_element()
            },
            |kind| agent_mark(kind, 16.0, if selected { CHALK } else { TRACE }),
        );

        div()
            .id(("terminal-quick-switch", surface_index))
            .role(Role::Tab)
            .aria_label(format!("{label}, {}", status_word(status)))
            .aria_selected(selected)
            .relative()
            .size(ui_size(20.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .rounded(ui_size(5.0))
            .bg(rgb(if selected { PANEL } else { DECK }))
            .hover(|button| button.bg(rgb(PANEL)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |desktop, _, window, cx| {
                cx.stop_propagation();
                desktop.select_terminal_in_session(session_index, surface_index, window, cx);
            }))
            .child(mark)
            .child(
                div()
                    .absolute()
                    .right(ui_size(0.0))
                    .bottom(ui_size(0.0))
                    .size(ui_size(5.0))
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(DECK))
                    .bg(rgb(status.color())),
            )
            .into_any_element()
    }

    fn expanded_terminal_children(
        &self,
        session_index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session = &self.workspace().sessions[session_index];
        let expanded = self
            .session_sidebar
            .expanded_sessions
            .contains(&session_expansion_key(session).unwrap_or(usize::MAX));
        let target_height = session.terminals.len() as f32 * TERMINAL_CHILD_HEIGHT + 4.0;
        let rows = session
            .terminals
            .iter()
            .copied()
            .map(|surface_index| {
                let agent = self.terminal_agent(surface_index);
                let status = self.terminal_status(session_index, surface_index);
                let selected = self.workspace().selected_session == session_index
                    && self.active_surface == Some(surface_index);
                let title = self.terminal_title(surface_index, agent, cx);
                let process = self
                    .surface_foreground_processes
                    .get(surface_index)
                    .and_then(Option::as_ref)
                    .map(|process| process.executable.clone());
                let detail = process.map_or_else(
                    || status_word(status).to_owned(),
                    |process| format!("{process} · {}", status_word(status)),
                );
                // A plain idle shell says nothing the dot does not already say.
                let show_detail = agent.is_some() || status != SessionStatus::Idle;
                let mark = agent.map_or_else(
                    || {
                        div()
                            .size(ui_size(18.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(ui_size(4.0))
                            .bg(rgb(PANEL))
                            .font_family(UI_FONT)
                            .text_size(ui_size(9.0))
                            .text_color(rgb(TRACE))
                            .child("›_")
                            .into_any_element()
                    },
                    |kind| agent_mark(kind, 18.0, if selected { CHALK } else { TRACE }),
                );

                div()
                    .id(("session-terminal-row", surface_index))
                    .debug_selector(move || format!("session-terminal-row-{surface_index}"))
                    .role(Role::Tab)
                    .aria_label(format!("{title}, {detail}"))
                    .aria_selected(selected)
                    .h(ui_size(TERMINAL_CHILD_HEIGHT))
                    .ml(ui_size(26.0))
                    .mr_1()
                    .pl(ui_size(7.0))
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .cursor_pointer()
                    .rounded(ui_size(5.0))
                    .bg(rgb(if selected { PANEL } else { DECK }))
                    .hover(|row| row.bg(rgb(PANEL)))
                    .on_click(cx.listener(move |desktop, _, window, cx| {
                        desktop.select_terminal_in_session(
                            session_index,
                            surface_index,
                            window,
                            cx,
                        );
                    }))
                    .child(mark)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .overflow_hidden()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .font_family(PRODUCT_FONT)
                                    .text_size(ui_size(11.0))
                                    .font_weight(if selected {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(rgb(if selected { CHALK } else { TRACE }))
                                    .child(title),
                            )
                            .when(show_detail, |column| {
                                column.child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .font_family(UI_FONT)
                                        .text_size(ui_size(8.0))
                                        .text_color(rgb(status.color()))
                                        .child(detail),
                                )
                            }),
                    )
                    .child(
                        div()
                            .size(ui_size(5.0))
                            .rounded_full()
                            .bg(rgb(status.color())),
                    )
            })
            .collect::<Vec<_>>();
        let animation_id = format!("session-children-{session_index}-{expanded}");
        div()
            .relative()
            .h(ui_size(if expanded { target_height } else { 0.0 }))
            .overflow_hidden()
            .opacity(if expanded { 1.0 } else { 0.0 })
            .when(!expanded, |children| children.invisible())
            .child(
                div()
                    .absolute()
                    .left(ui_size(17.0))
                    .top(ui_size(1.0))
                    .bottom(ui_size(5.0))
                    .w(ui_size(1.0))
                    .bg(rgb(HAIRLINE)),
            )
            .children(rows)
            .with_animation(
                animation_id,
                Animation::new(EXPANSION_DURATION)
                    .with_easing(|value| 1.0 - (1.0 - value).powi(3))
                    .with_max_fps(60.0),
                move |children, value| {
                    let progress = if expanded { value } else { 1.0 - value };
                    children
                        .h(ui_size(target_height * progress))
                        .opacity(progress)
                },
            )
            .into_any_element()
    }

    fn expanded_session_row(
        &self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let session_count = self.workspace().sessions.len();
        let session = &self.workspace().sessions[index];
        let selected = self.workspace().selected_session == index;
        let status_color = session.status.color();
        let name = self.session_display_title(session, cx);
        let worktree = self.session_worktree(session, cx);
        let terminal_count = session.terminals.len();
        let primary_agent = self.session_primary_agent(session);
        let identity = primary_agent.map_or_else(
            || {
                if session.actor.is_empty() {
                    "shell".to_owned()
                } else {
                    session.actor.clone()
                }
            },
            |agent| agent.label().to_owned(),
        );
        // Identity, place, state; the count only when there is more than one.
        let mut detail = vec![identity];
        if let Some(worktree) = &worktree {
            detail.push(format!("⎇ {}", worktree.label()));
        }
        detail.push(status_word(session.status).to_owned());
        if terminal_count > 1 {
            detail.push(format!("{terminal_count} terminals"));
        }
        let status_detail = detail.join(" · ");
        let expanded = session_expansion_key(session)
            .is_some_and(|key| self.session_sidebar.expanded_sessions.contains(&key));
        // The child rows already list every terminal once expanded; keep the
        // collapsed row to one mark plus a count so the name keeps its room.
        let quick_switch_limit = if expanded { 0 } else { 1 };
        let terminal_quick_switches = session
            .terminals
            .iter()
            .take(quick_switch_limit)
            .copied()
            .map(|surface_index| self.terminal_quick_switch(index, surface_index, cx))
            .collect::<Vec<_>>();
        let hidden_terminal_count = if expanded {
            0
        } else {
            terminal_count.saturating_sub(quick_switch_limit)
        };
        let accessibility_label = format!(
            "{}, {}, {} terminal{}",
            name,
            session.status.label(&session.actor),
            terminal_count,
            if terminal_count == 1 { "" } else { "s" }
        );
        let menu_open = self.session_sidebar.menu_open == Some(index);
        let rename_value = self
            .session_sidebar
            .rename
            .as_ref()
            .filter(|rename| rename.index == index)
            .map(|rename| rename.value.clone());
        let renaming = rename_value.is_some();
        let retained = matches!(
            session.status,
            SessionStatus::Closed | SessionStatus::Terminated
        );
        #[cfg(not(test))]
        let pinned = session.pinned;
        #[cfg(test)]
        let pinned = false;

        let parent = div()
            .id(("session-row", index))
            .debug_selector(move || format!("session-row-{index}"))
            .role(Role::Tab)
            .aria_label(accessibility_label)
            .aria_selected(selected)
            .aria_expanded(expanded)
            .group("session-row")
            .relative()
            .h(ui_size(SESSION_ROW_HEIGHT))
            .mx_1()
            .flex()
            .items_center()
            .min_w_0()
            .cursor_pointer()
            .rounded(ui_size(5.0))
            .bg(rgb(if selected { ACTIVE } else { DECK }))
            .text_color(rgb(if retained && !selected { TRACE } else { CHALK }))
            .hover(|row| row.bg(rgb(if selected { ACTIVE } else { PANEL })))
            .on_click(cx.listener(move |desktop, _, window, cx| {
                if desktop.workspace().selected_session != index {
                    desktop.select_session(index, window, cx);
                }
                desktop.toggle_session_expansion(index, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |desktop, _, window, cx| {
                    cx.stop_propagation();
                    desktop.toggle_session_menu(index, window, cx);
                }),
            )
            .when(selected, |row| {
                row.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(ui_size(6.0))
                        .bottom(ui_size(6.0))
                        .w(ui_size(2.0))
                        .rounded(ui_size(1.0))
                        .bg(rgb(RELAY)),
                )
            })
            .child(
                div()
                    .ml(ui_size(7.0))
                    .mr(ui_size(7.0))
                    .flex()
                    .items_center()
                    .gap(ui_size(3.0))
                    .flex_none()
                    .child(
                        div()
                            .w(ui_size(8.0))
                            .font_family(UI_FONT)
                            .text_size(ui_size(9.0))
                            .text_color(rgb(if expanded { CHALK } else { TRACE }))
                            .child(if expanded { "⌄" } else { "›" }),
                    )
                    .child(primary_agent.map_or_else(
                        || {
                            div()
                                .relative()
                                .size(ui_size(16.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(ui_size(4.0))
                                .bg(rgb(PANEL))
                                .font_family(UI_FONT)
                                .text_size(ui_size(9.0))
                                .text_color(rgb(TRACE))
                                .child("›_")
                                .child(
                                    div()
                                        .absolute()
                                        .right(ui_size(-1.0))
                                        .bottom(ui_size(-1.0))
                                        .size(ui_size(5.0))
                                        .rounded_full()
                                        .bg(rgb(status_color)),
                                )
                                .into_any_element()
                        },
                        |agent| {
                            div()
                                .relative()
                                .child(agent_mark(
                                    agent,
                                    20.0,
                                    if selected { CHALK } else { TRACE },
                                ))
                                .child(
                                    div()
                                        .absolute()
                                        .right(ui_size(-1.0))
                                        .bottom(ui_size(-1.0))
                                        .size(ui_size(5.0))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(rgb(DECK))
                                        .bg(rgb(status_color)),
                                )
                                .into_any_element()
                        },
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .flex_1()
                    .min_w(ui_size(56.0))
                    .overflow_hidden()
                    .when(!renaming, |name_row| {
                        name_row.child(
                            div()
                                .debug_selector(move || format!("session-name-{index}"))
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .font_family(PRODUCT_FONT)
                                .text_size(ui_size(12.0))
                                .font_weight(if selected {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .child(name),
                        )
                    })
                    .when_some(rename_value, |name_row, value| {
                        name_row.child(
                            div()
                                .id(("session-rename", index))
                                .debug_selector(move || format!("session-rename-{index}"))
                                .h(ui_size(24.0))
                                .px_1()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .overflow_hidden()
                                .rounded(ui_size(3.0))
                                .bg(rgb(PANEL))
                                .border_b_1()
                                .border_color(rgb(RELAY))
                                .font_family(PRODUCT_FONT)
                                .font_weight(FontWeight::SEMIBOLD)
                                .track_focus(&self.session_sidebar.search_focus)
                                .on_key_down(cx.listener(Self::handle_session_rename_key))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(|desktop, _, window, cx| {
                                    cx.stop_propagation();
                                    window.focus(&desktop.session_sidebar.search_focus, cx);
                                }))
                                .child(value)
                                .child(
                                    div()
                                        .ml(ui_size(1.0))
                                        .w(ui_size(1.0))
                                        .h(ui_size(14.0))
                                        .flex_none()
                                        .bg(rgb(RELAY)),
                                ),
                        )
                    })
                    .when(!renaming, |name_row| {
                        name_row.child(
                            div()
                                .debug_selector(move || format!("session-detail-{index}"))
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .flex_none()
                                .font_family(UI_FONT)
                                .text_size(ui_size(8.5))
                                .text_color(rgb(status_color))
                                .child(status_detail),
                        )
                    }),
            )
            .child(
                div()
                    .mr_1()
                    .flex()
                    .items_center()
                    .gap(ui_size(2.0))
                    .children(terminal_quick_switches)
                    .when(hidden_terminal_count > 0, |marks| {
                        marks.child(
                            div()
                                .ml(ui_size(1.0))
                                .font_family(UI_FONT)
                                .text_size(ui_size(8.0))
                                .text_color(rgb(TRACE))
                                .child(format!("+{hidden_terminal_count}")),
                        )
                    }),
            )
            .when(pinned && !renaming, |row| {
                row.child(
                    div()
                        .mr_1()
                        .flex_none()
                        .font_family(UI_FONT)
                        .text_size(ui_size(10.0))
                        .text_color(rgb(RELAY))
                        .child("◆"),
                )
            })
            .when(!self.shared_mode, |row| {
                row.child(
                    div()
                        .id(("session-menu-toggle", index))
                        .role(Role::Button)
                        .aria_label("Session actions")
                        .aria_expanded(menu_open)
                        .mr_1()
                        .size(ui_size(22.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(ui_size(3.0))
                        .font_family(UI_FONT)
                        .text_size(ui_size(9.0))
                        .text_color(rgb(TRACE))
                        .opacity(if selected || menu_open { 1.0 } else { 0.0 })
                        .group_hover("session-row", |menu| menu.opacity(1.0))
                        .hover(|menu| menu.bg(rgb(HAIRLINE)).text_color(rgb(CHALK)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |desktop, _, window, cx| {
                            cx.stop_propagation();
                            desktop.toggle_session_menu(index, window, cx);
                        }))
                        .child("…"),
                )
            })
            .when(self.pending_termination == Some(index), |row| {
                let popover = self.session_termination_popover(index, window, cx);
                row.child(
                    deferred(self.app_zoom.anchored_popup(0.0, 29.0, popover)).with_priority(2),
                )
            })
            .when(menu_open, |row| {
                row.child(
                    deferred(
                        self.app_zoom.anchored_popup(
                            0.0,
                            29.0,
                            div()
                                .id(("session-menu", index))
                                .debug_selector(move || format!("session-menu-{index}"))
                                .w(self.app_zoom.overlay_width(window, 196.0))
                                .max_h(self.app_zoom.overlay_height(window, f32::MAX))
                                .overflow_y_scroll()
                                .py_1()
                                .rounded(ui_size(5.0))
                                .border_1()
                                .border_color(rgb(HAIRLINE))
                                .bg(rgb(DECK))
                                .shadow_lg()
                                .occlude()
                                .text_size(ui_size(12.0))
                                .text_color(rgb(CHALK))
                                .child(
                                    div()
                                        .id(("session-rename-action", index))
                                        .debug_selector(move || {
                                            format!("session-rename-action-{index}")
                                        })
                                        .role(Role::MenuItem)
                                        .aria_label("Rename session")
                                        .h(ui_size(30.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .rounded(ui_size(3.0))
                                        .hover(|item| item.bg(rgb(ACTIVE)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(move |desktop, _, window, cx| {
                                            cx.stop_propagation();
                                            desktop.begin_session_rename(index, window, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(ui_size(22.0))
                                                .flex_none()
                                                .font_family(UI_FONT)
                                                .text_size(ui_size(10.0))
                                                .text_color(rgb(TRACE))
                                                .child("✎"),
                                        )
                                        .child("Rename"),
                                )
                                .child(
                                    div()
                                        .id(("session-pin-action", index))
                                        .role(Role::MenuItem)
                                        .aria_label(if pinned {
                                            "Unpin session"
                                        } else {
                                            "Pin session"
                                        })
                                        .h(ui_size(30.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .rounded(ui_size(3.0))
                                        .hover(|item| item.bg(rgb(ACTIVE)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(move |desktop, _, _, cx| {
                                            cx.stop_propagation();
                                            desktop.toggle_session_pin(index, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(ui_size(22.0))
                                                .flex_none()
                                                .font_family(UI_FONT)
                                                .text_size(ui_size(9.0))
                                                .text_color(rgb(TRACE))
                                                .child("◆"),
                                        )
                                        .child(if pinned { "Unpin" } else { "Pin" }),
                                )
                                .child(
                                    div()
                                        .id(("session-detach-action", index))
                                        .role(Role::MenuItem)
                                        .aria_label("Detach session group")
                                        .h(ui_size(30.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .rounded(ui_size(3.0))
                                        .hover(|item| item.bg(rgb(ACTIVE)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(move |desktop, _, _, cx| {
                                            cx.stop_propagation();
                                            desktop.detach_session_group(index, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(ui_size(22.0))
                                                .flex_none()
                                                .font_family(UI_FONT)
                                                .text_color(rgb(TRACE))
                                                .child("↗"),
                                        )
                                        .child("Detach from sidebar"),
                                )
                                .when(!retained, |menu| {
                                    menu.child(
                                        div()
                                            .id(("session-add-terminal", index))
                                            .role(Role::MenuItem)
                                            .aria_label("Add terminal to session group")
                                            .h(ui_size(30.0))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .rounded(ui_size(3.0))
                                            .hover(|item| item.bg(rgb(ACTIVE)))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(move |desktop, _, window, cx| {
                                                cx.stop_propagation();
                                                desktop.add_terminal_to_session(index, window, cx);
                                            }))
                                            .child(
                                                div()
                                                    .w(ui_size(22.0))
                                                    .flex_none()
                                                    .font_family(UI_FONT)
                                                    .text_color(rgb(TRACE))
                                                    .child("+"),
                                            )
                                            .child("Add terminal"),
                                    )
                                })
                                .when(session_count > 1, |menu| {
                                    menu.child(
                                        div()
                                            .h(ui_size(30.0))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .child(
                                                div()
                                                    .w(ui_size(22.0))
                                                    .flex_none()
                                                    .font_family(UI_FONT)
                                                    .text_color(rgb(TRACE))
                                                    .child("↕"),
                                            )
                                            .child("Move")
                                            .child(
                                                div()
                                                    .ml_auto()
                                                    .flex()
                                                    .gap_1()
                                                    .when(index > 0, |controls| {
                                                        controls.child(
                                                            div()
                                                                .id(("session-move-up", index))
                                                                .role(Role::MenuItem)
                                                                .aria_label("Move session up")
                                                                .size(ui_size(22.0))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .cursor_pointer()
                                                                .rounded(ui_size(3.0))
                                                                .bg(rgb(PANEL))
                                                                .font_family(UI_FONT)
                                                                .text_color(rgb(TRACE))
                                                                .hover(|button| {
                                                                    button
                                                                        .bg(rgb(ACTIVE))
                                                                        .text_color(rgb(CHALK))
                                                                })
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    |_, _, cx| {
                                                                        cx.stop_propagation()
                                                                    },
                                                                )
                                                                .on_click(cx.listener(
                                                                    move |desktop, _, _, cx| {
                                                                        cx.stop_propagation();
                                                                        desktop.move_session(
                                                                            index,
                                                                            index - 1,
                                                                            cx,
                                                                        );
                                                                    },
                                                                ))
                                                                .child("↑"),
                                                        )
                                                    })
                                                    .when(index + 1 < session_count, |controls| {
                                                        controls.child(
                                                            div()
                                                                .id(("session-move-down", index))
                                                                .role(Role::MenuItem)
                                                                .aria_label("Move session down")
                                                                .size(ui_size(22.0))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .cursor_pointer()
                                                                .rounded(ui_size(3.0))
                                                                .bg(rgb(PANEL))
                                                                .font_family(UI_FONT)
                                                                .text_color(rgb(TRACE))
                                                                .hover(|button| {
                                                                    button
                                                                        .bg(rgb(ACTIVE))
                                                                        .text_color(rgb(CHALK))
                                                                })
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    |_, _, cx| {
                                                                        cx.stop_propagation()
                                                                    },
                                                                )
                                                                .on_click(cx.listener(
                                                                    move |desktop, _, _, cx| {
                                                                        cx.stop_propagation();
                                                                        desktop.move_session(
                                                                            index,
                                                                            index + 1,
                                                                            cx,
                                                                        );
                                                                    },
                                                                ))
                                                                .child("↓"),
                                                        )
                                                    }),
                                            ),
                                    )
                                })
                                .when(!retained && terminal_count > 0, |menu| {
                                    menu.child(
                                        div()
                                            .id(("session-terminate", index))
                                            .role(Role::MenuItem)
                                            .aria_label("Terminate session")
                                            .h(ui_size(31.0))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .mt_1()
                                            .border_t_1()
                                            .border_color(rgb(HAIRLINE))
                                            .text_color(rgb(FAULT))
                                            .hover(|item| item.bg(rgb(ACTIVE)))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(move |desktop, _, _, cx| {
                                                cx.stop_propagation();
                                                desktop.request_session_termination_at(index, cx);
                                            }))
                                            .child(
                                                div()
                                                    .w(ui_size(22.0))
                                                    .flex_none()
                                                    .font_family(UI_FONT)
                                                    .child("!"),
                                            )
                                            .child("Terminate session…"),
                                    )
                                })
                                .when(retained && terminal_count > 0, |menu| {
                                    menu.child(
                                        div()
                                            .id(("session-archive", index))
                                            .role(Role::MenuItem)
                                            .aria_label("Archive session history")
                                            .h(ui_size(31.0))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .mt_1()
                                            .border_t_1()
                                            .border_color(rgb(HAIRLINE))
                                            .text_color(rgb(TRACE))
                                            .hover(|item| {
                                                item.bg(rgb(ACTIVE)).text_color(rgb(CHALK))
                                            })
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(move |desktop, _, _, cx| {
                                                cx.stop_propagation();
                                                desktop.archive_session_from_menu(index, cx);
                                            }))
                                            .child(
                                                div()
                                                    .w(ui_size(22.0))
                                                    .flex_none()
                                                    .font_family(UI_FONT)
                                                    .child("↧"),
                                            )
                                            .child("Archive history"),
                                    )
                                })
                                .when(retained, |menu| {
                                    menu.child(
                                        div()
                                            .id(("session-remove", index))
                                            .role(Role::MenuItem)
                                            .aria_label("Remove session from sidebar")
                                            .h(ui_size(31.0))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .rounded(ui_size(3.0))
                                            .text_color(rgb(TRACE))
                                            .hover(|item| {
                                                item.bg(rgb(ACTIVE)).text_color(rgb(CHALK))
                                            })
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation();
                                            })
                                            .on_click(cx.listener(move |desktop, _, _, cx| {
                                                cx.stop_propagation();
                                                desktop.remove_session_from_menu(index, cx);
                                            }))
                                            .child(
                                                div()
                                                    .w(ui_size(22.0))
                                                    .flex_none()
                                                    .font_family(UI_FONT)
                                                    .child("✕"),
                                            )
                                            .child("Remove"),
                                    )
                                })
                                .into_any_element(),
                        ),
                    )
                    .with_priority(1),
                )
            });

        div()
            .id(("session-stack", index))
            .flex()
            .flex_col()
            .child(parent)
            .when(terminal_count > 0, |stack| {
                stack.child(self.expanded_terminal_children(index, cx))
            })
            .into_any_element()
    }

    fn collapsed_session_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let session = &self.workspace().sessions[index];
        let selected = self.workspace().selected_session == index;
        let status_color = session.status.color();
        let terminal_count = session.terminals.len();
        let primary_agent = self.session_primary_agent(session);
        let name = self.session_display_title(session, cx);
        let accessibility_label = format!(
            "{}, {}, {} terminal{}",
            name,
            session.status.label(&session.actor),
            terminal_count,
            if terminal_count == 1 { "" } else { "s" }
        );

        div()
            .id(("collapsed-session-row", index))
            .debug_selector(move || format!("collapsed-session-row-{index}"))
            .role(Role::Tab)
            .aria_label(accessibility_label)
            .aria_selected(selected)
            .relative()
            .h(ui_size(32.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(rgb(if selected { ACTIVE } else { DECK }))
            .hover(|row| row.bg(rgb(if selected { ACTIVE } else { PANEL })))
            .on_click(cx.listener(move |desktop, _, window, cx| {
                desktop.select_session(index, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |desktop, _, window, cx| {
                    cx.stop_propagation();
                    desktop.sidebar_open = true;
                    desktop.toggle_session_menu(index, window, cx);
                }),
            )
            .when(selected, |row| {
                row.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(ui_size(7.0))
                        .bottom(ui_size(7.0))
                        .w(ui_size(2.0))
                        .bg(rgb(RELAY)),
                )
            })
            .child(
                div()
                    .relative()
                    .size(ui_size(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(ui_size(5.0))
                    .bg(rgb(if selected { PANEL } else { ACTIVE }))
                    .child(primary_agent.map_or_else(
                        || {
                            div()
                                .font_family(UI_FONT)
                                .text_size(ui_size(9.0))
                                .text_color(rgb(TRACE))
                                .child("›_")
                                .into_any_element()
                        },
                        |agent| agent_mark(agent, 18.0, if selected { CHALK } else { TRACE }),
                    ))
                    .child(
                        div()
                            .absolute()
                            .right(ui_size(-1.0))
                            .bottom(ui_size(-1.0))
                            .size(ui_size(5.0))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(DECK))
                            .bg(rgb(status_color)),
                    )
                    .when(terminal_count > 1, |icon| {
                        icon.child(
                            div()
                                .absolute()
                                .right(ui_size(-5.0))
                                .top(ui_size(-5.0))
                                .min_w(ui_size(12.0))
                                .h(ui_size(12.0))
                                .px(ui_size(2.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(ui_size(6.0))
                                .bg(rgb(PANEL))
                                .font_family(UI_FONT)
                                .text_size(ui_size(8.0))
                                .text_color(rgb(CHALK))
                                .child(terminal_count.to_string()),
                        )
                    }),
            )
            .into_any_element()
    }

    fn sidebar_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let query = self.session_sidebar.query.clone();
        let has_query = !query.is_empty();

        div()
            .id("session-search")
            .role(Role::SearchInput)
            .aria_label("Find a session or terminal history")
            .h(ui_size(28.0))
            .mx_2()
            .mb(ui_size(3.0))
            .px_2()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .cursor_text()
            .rounded(ui_size(4.0))
            .border_1()
            .border_color(rgb(if has_query { RELAY } else { HAIRLINE }))
            .bg(rgb(PANEL))
            .track_focus(&self.session_sidebar.search_focus)
            .on_click(cx.listener(|desktop, _, window, cx| {
                desktop.focus_sidebar_search(window, cx);
            }))
            .on_key_down(cx.listener(Self::handle_sidebar_search_key))
            .child(
                div()
                    .font_family(UI_FONT)
                    .text_xs()
                    .text_color(rgb(if has_query { RELAY } else { TRACE }))
                    .child("/"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_family(PRODUCT_FONT)
                    .text_color(rgb(if has_query { CHALK } else { TRACE }))
                    .child(if has_query {
                        query
                    } else {
                        "Find a session".to_owned()
                    }),
            )
            .when(has_query, |search| {
                search.child(
                    div()
                        .id("clear-session-search")
                        .role(Role::Button)
                        .aria_label("Clear session search")
                        .size(ui_size(18.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(ui_size(4.0))
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(TRACE))
                        .hover(|clear| clear.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|desktop, _, window, cx| {
                            cx.stop_propagation();
                            desktop.clear_sidebar_search(window, cx);
                        }))
                        .child("×"),
                )
            })
            .into_any_element()
    }

    pub(crate) fn session_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sidebar_px = sidebar_width(
            f32::from(window.viewport_size().width) / self.app_zoom.factor(),
            self.sidebar_open,
            self.sidebar_custom_width,
        );
        let sessions = &self.workspace().sessions;
        let session_count = sessions.len();
        let visible = visible_indices(sessions, &self.session_sidebar.query);
        let no_matches = visible.is_empty() && !self.session_sidebar.query.is_empty();

        div()
            .debug_selector(|| "session-sidebar".into())
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_hidden()
            .relative()
            .bg(rgb(DECK))
            .border_r_1()
            .border_color(rgb(HAIRLINE))
            .w(ui_size(sidebar_px))
            .child(
                div()
                    .id(if self.sidebar_open {
                        "sidebar-header"
                    } else {
                        "collapse-sidebar"
                    })
                    .h(ui_size(36.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .font_family(UI_FONT)
                    .text_xs()
                    .text_color(rgb(TRACE))
                    .when(self.sidebar_open, |header| {
                        header
                            .px_2()
                            .child(
                                div()
                                    .font_family(PRODUCT_FONT)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(ui_size(13.0))
                                    .text_color(rgb(CHALK))
                                    .child("Sessions"),
                            )
                            .child(
                                div()
                                    .ml(ui_size(6.0))
                                    .font_family(UI_FONT)
                                    .text_size(ui_size(9.0))
                                    .text_color(rgb(TRACE))
                                    .child(session_count.to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id("toggle-session-search")
                                    .role(Role::Button)
                                    .aria_label("Search sessions")
                                    .aria_expanded(self.session_sidebar.search_open)
                                    .size(ui_size(22.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .rounded(ui_size(4.0))
                                    .font_family(UI_FONT)
                                    .text_size(ui_size(14.0))
                                    .text_color(rgb(if self.session_sidebar.search_open {
                                        RELAY
                                    } else {
                                        TRACE
                                    }))
                                    .hover(|button| button.bg(rgb(PANEL)).text_color(rgb(CHALK)))
                                    .on_click(cx.listener(|desktop, _, window, cx| {
                                        desktop.toggle_sidebar_search(window, cx);
                                    }))
                                    .child("⌕"),
                            )
                            .when(!self.shared_mode, |header| {
                                header.child(
                                    div()
                                        .id("new-session")
                                        .role(Role::Button)
                                        .aria_label("New session")
                                        .size(ui_size(22.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .rounded(ui_size(4.0))
                                        .text_size(ui_size(15.0))
                                        .text_color(rgb(TRACE))
                                        .hover(|button| {
                                            button.bg(rgb(PANEL)).text_color(rgb(CHALK))
                                        })
                                        .on_click(cx.listener(|desktop, _, window, cx| {
                                            desktop.add_session(window, cx);
                                        }))
                                        .child("+"),
                                )
                            })
                            .child(
                                div()
                                    .id("collapse-sidebar")
                                    .role(Role::Button)
                                    .aria_label("Collapse sessions sidebar")
                                    .ml(ui_size(2.0))
                                    .size(ui_size(22.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .rounded(ui_size(4.0))
                                    .text_color(rgb(TRACE))
                                    .hover(|button| button.bg(rgb(PANEL)).text_color(rgb(CHALK)))
                                    .on_click(cx.listener(|desktop, _, _, cx| {
                                        desktop.toggle_sidebar(cx);
                                    }))
                                    .child("‹"),
                            )
                    })
                    .when(!self.sidebar_open, |header| {
                        header
                            .role(Role::Button)
                            .aria_label("Expand sessions sidebar")
                            .justify_center()
                            .cursor_pointer()
                            .hover(|header| header.bg(rgb(PANEL)).text_color(rgb(CHALK)))
                            .on_click(cx.listener(|desktop, _, _, cx| {
                                desktop.toggle_sidebar(cx);
                            }))
                            .child("›")
                    }),
            )
            .when(self.sidebar_open, |sidebar| {
                sidebar
                    .children({
                        #[cfg(not(test))]
                        {
                            self.fault_sidebar(cx)
                        }
                        #[cfg(test)]
                        {
                            None::<AnyElement>
                        }
                    })
                    .children({
                        #[cfg(not(test))]
                        {
                            self.attention_sidebar(cx)
                        }
                        #[cfg(test)]
                        {
                            None::<AnyElement>
                        }
                    })
                    .when(self.session_sidebar.search_open, |sidebar| {
                        sidebar.child(self.sidebar_search(cx))
                    })
                    .children(self.terminal_history_results(cx))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .on_scroll_wheel(cx.listener(|desktop, _, _, cx| {
                                desktop.reveal_sidebar_scrollbar(cx);
                            }))
                            .on_mouse_move(cx.listener(|desktop, event, _, cx| {
                                desktop.reveal_sidebar_scrollbar_near_edge(event, cx);
                            }))
                            .child(
                                div()
                                    .id("session-list-scroll")
                                    .h_full()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.session_sidebar.scroll)
                                    .pt(ui_size(2.0))
                                    .pb_2()
                                    .children(
                                        visible.iter().copied().map(|index| {
                                            self.expanded_session_row(index, window, cx)
                                        }),
                                    )
                                    .when(no_matches, |list| {
                                        list.child(
                                            div()
                                                .px_3()
                                                .py_4()
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .text_center()
                                                .font_family(UI_FONT)
                                                .text_xs()
                                                .text_color(rgb(TRACE))
                                                .child("No matching sessions")
                                                .child(
                                                    div()
                                                        .mt_1()
                                                        .text_color(rgb(CHALK))
                                                        .child("Clear search to show all"),
                                                ),
                                        )
                                    }),
                            )
                            .children(self.sidebar_scrollbar()),
                    )
            })
            .when(!self.sidebar_open, |sidebar| {
                sidebar.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .on_scroll_wheel(cx.listener(|desktop, _, _, cx| {
                            desktop.reveal_sidebar_scrollbar(cx);
                        }))
                        .on_mouse_move(cx.listener(|desktop, event, _, cx| {
                            desktop.reveal_sidebar_scrollbar_near_edge(event, cx);
                        }))
                        .child(
                            div()
                                .id("collapsed-session-list-scroll")
                                .h_full()
                                .min_h_0()
                                .overflow_y_scroll()
                                .track_scroll(&self.session_sidebar.scroll)
                                .children(
                                    (0..session_count)
                                        .map(|index| self.collapsed_session_row(index, cx)),
                                ),
                        )
                        .children(self.sidebar_scrollbar()),
                )
            })
            .when(!self.shared_mode && !self.sidebar_open, |sidebar| {
                sidebar.child(
                    div()
                        .id("new-session")
                        .role(Role::Button)
                        .aria_label("New session")
                        .h(ui_size(32.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .border_t_1()
                        .border_color(rgb(HAIRLINE))
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(CHALK))
                        .hover(|button| button.bg(rgb(PANEL)))
                        .on_click(cx.listener(|desktop, _, window, cx| {
                            desktop.add_session(window, cx);
                        }))
                        .child("+"),
                )
            })
            .child(self.provider_usage_footer(sidebar_px, cx))
            .when(self.sidebar_open, |sidebar| {
                sidebar.child(self.sidebar_resize_handle(cx))
            })
    }

    /// Drag strip on the sidebar's right edge. Double-click restores the
    /// responsive default width.
    fn sidebar_resize_handle(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("sidebar-resize-handle")
            .debug_selector(|| "sidebar-resize-handle".to_owned())
            .role(Role::Splitter)
            .aria_label("Resize sessions sidebar")
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(SPLITTER_THICKNESS))
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|handle| handle.bg(rgb(RELAY).alpha(0.45)))
            .on_drag(SidebarEdgeDrag, |_, _, _, cx| cx.new(|_| SplitDragPreview))
            .on_click(cx.listener(|desktop, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    desktop.reset_sidebar_width(cx);
                }
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str, actor: &str, status: SessionStatus) -> SessionView {
        SessionView {
            group_id: Default::default(),
            group_version: 0,
            pinned: false,
            automatic_name: false,
            name: name.to_owned(),
            actor: actor.to_owned(),
            status,
            terminals: vec![0],
        }
    }

    #[test]
    fn preserves_workspace_order_across_all_session_states() {
        let sessions = vec![
            session("old", "", SessionStatus::Closed),
            session("waiting", "you", SessionStatus::Idle),
            session("build", "codex", SessionStatus::Working),
            session("failed", "", SessionStatus::Terminated),
        ];

        assert_eq!(visible_indices(&sessions, ""), vec![0, 1, 2, 3]);
    }

    #[test]
    fn status_changes_do_not_reorder_the_session_navigator() {
        let mut sessions = vec![
            session("first", "you", SessionStatus::Idle),
            session("second", "codex", SessionStatus::Working),
            session("third", "", SessionStatus::Closed),
        ];
        let before = visible_indices(&sessions, "");

        sessions[0].status = SessionStatus::Working;
        sessions[1].status = SessionStatus::Idle;

        assert_eq!(visible_indices(&sessions, ""), before);
    }

    #[test]
    fn search_matches_name_actor_and_status() {
        let sessions = vec![
            session("foundation", "codex", SessionStatus::Working),
            session("linux-smoke", "you", SessionStatus::Idle),
            session("retained", "", SessionStatus::Closed),
        ];

        assert_eq!(visible_indices(&sessions, "linux"), vec![1]);
        assert_eq!(visible_indices(&sessions, "codex"), vec![0]);
        assert_eq!(visible_indices(&sessions, "closed"), vec![2]);
    }

    #[test]
    fn empty_search_is_case_insensitive_and_whitespace_tolerant() {
        let sessions = vec![session("Linux Smoke", "Codex", SessionStatus::Working)];

        assert_eq!(visible_indices(&sessions, "  LINUX  "), vec![0]);
        assert!(session_matches(&sessions[0], "  "));
    }

    #[test]
    fn sidebar_width_is_compact_and_responsive() {
        assert_eq!(legacy_sidebar_width(800.0, true), 196.0);
        assert!((legacy_sidebar_width(1_200.0, true) - 216.0).abs() < 0.001);
        assert_eq!(legacy_sidebar_width(2_000.0, true), 224.0);
        assert_eq!(legacy_sidebar_width(2_000.0, false), 40.0);
        assert_eq!(sidebar_width(800.0, true, None), 196.0);
        assert_eq!(sidebar_width(800.0, true, Some(300.0)), 300.0);
        assert_eq!(sidebar_width(800.0, true, Some(900.0)), 400.0);
        assert_eq!(sidebar_width(800.0, false, Some(300.0)), 40.0);
    }

    #[gpui::test]
    fn session_management_actions_target_the_chosen_row(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = crate::create_surfaces(window, cx);
            UltraplexrDesktop::new(surfaces, cx.focus_handle())
        });

        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.toggle_session_menu(1, window, cx);
                assert_eq!(desktop.session_sidebar.menu_open, Some(1));

                desktop.begin_session_rename(1, window, cx);
                desktop
                    .session_sidebar
                    .rename
                    .as_mut()
                    .expect("rename draft should open")
                    .value = "build-shell".to_owned();
                desktop.commit_session_rename(window, cx);
                assert_eq!(desktop.workspace().sessions[1].name, "build-shell");

                desktop.toggle_session_menu(1, window, cx);
                desktop.move_session(1, 0, cx);
                assert_eq!(desktop.workspace().selected_session, 0);
                assert_eq!(desktop.workspace().sessions[0].name, "build-shell");
                assert_eq!(desktop.session_sidebar.menu_open, Some(0));

                let terminal_count = desktop.workspace().sessions[0].terminals.len();
                desktop.add_terminal_to_session(0, window, cx);
                assert_eq!(desktop.workspace().selected_session, 0);
                assert_eq!(
                    desktop.workspace().sessions[0].terminals.len(),
                    terminal_count + 1
                );
                assert_eq!(desktop.session_sidebar.menu_open, None);

                desktop.toggle_session_menu(0, window, cx);
                desktop.request_session_termination_at(0, cx);
                assert_eq!(desktop.pending_termination, Some(0));
                assert_eq!(desktop.session_sidebar.menu_open, None);
            });
        });
    }

    /// A Session that has exited must be removable.
    ///
    /// Until this existed a finished Session could only be archived, which
    /// leaves its row in place, so the sidebar accumulated dead Sessions with
    /// no way to clear them.
    #[gpui::test]
    fn a_finished_session_can_be_taken_out_of_the_sidebar(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = crate::create_surfaces(window, cx);
            UltraplexrDesktop::new(surfaces, cx.focus_handle())
        });

        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.workspace_mut().sessions[0].status = SessionStatus::Closed;
                let before = desktop.workspace().sessions.len();
                let removed_name = desktop.workspace().sessions[0].name.clone();

                desktop.toggle_session_menu(0, window, cx);
                desktop.remove_session_from_menu(0, cx);

                assert_eq!(desktop.session_sidebar.menu_open, None);
                assert_eq!(
                    desktop.workspace().sessions.len(),
                    before - 1,
                    "a closed session must leave the sidebar"
                );
                assert!(
                    desktop
                        .workspace()
                        .sessions
                        .iter()
                        .all(|session| session.name != removed_name),
                    "the removed session is still listed"
                );
                assert!(
                    desktop.workspace().selected_session < desktop.workspace().sessions.len(),
                    "selection must stay inside the remaining sessions"
                );
            });
        });
    }

    /// The confirmation is anchored to the row it acts on, and falls back to the
    /// centred dialog only when that row cannot be shown.
    #[gpui::test]
    fn terminate_confirmation_anchors_to_its_row(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = crate::create_surfaces(window, cx);
            UltraplexrDesktop::new(surfaces, cx.focus_handle())
        });

        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                assert!(
                    !desktop.termination_is_anchored(),
                    "nothing is pending, so nothing is anchored"
                );

                desktop.request_session_termination_at(0, cx);
                assert_eq!(desktop.pending_termination, Some(0));
                assert!(
                    desktop.termination_is_anchored(),
                    "an open sidebar showing the row must anchor the confirmation"
                );

                // A search that hides the row leaves it nowhere to appear, so
                // the centred dialog has to take over.
                desktop.session_sidebar.query = "zzz-no-such-session".to_owned();
                assert!(
                    !desktop.termination_is_anchored(),
                    "a filtered-out row cannot anchor the confirmation"
                );
                desktop.session_sidebar.query.clear();

                // Same when the sidebar is collapsed.
                desktop.sidebar_open = false;
                assert!(
                    !desktop.termination_is_anchored(),
                    "a collapsed sidebar cannot anchor the confirmation"
                );
                desktop.sidebar_open = true;

                desktop.pending_termination = None;
                let _ = window;
            });
        });
    }

    #[test]
    fn generated_names_are_recognised_and_marks_are_stripped() {
        assert!(is_generated_session_name("shell"));
        assert!(is_generated_session_name("shell-1cf389a9"));
        assert!(!is_generated_session_name("build-shell"));
        assert!(!is_generated_session_name("ultraplexr@main"));

        assert_eq!(strip_leading_mark("✳ Claude"), "Claude");
        assert_eq!(strip_leading_mark("⚡ fix the freeze"), "fix the freeze");
        assert_eq!(strip_leading_mark("Claude"), "Claude");
        assert_eq!(
            strip_leading_mark("→x"),
            "→x",
            "no space after the mark: not a mark"
        );
        assert_eq!(strip_leading_mark(""), "");
    }

    /// A Session is named by where it runs unless a person named it, and the
    /// runtime's generated name is only the last resort.
    #[gpui::test]
    fn sessions_are_titled_by_worktree_unless_a_person_named_them(cx: &mut gpui::TestAppContext) {
        let root = std::env::temp_dir().join(format!("ultraplexr-title-{}", uuid::Uuid::new_v4()));
        let repo = root.join("ultraplexr");
        std::fs::create_dir_all(repo.join(".git")).expect("git dir");
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").expect("HEAD");

        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = crate::create_surfaces(window, cx);
            UltraplexrDesktop::new(surfaces, cx.focus_handle())
        });
        cx.update(|_, cx| {
            desktop.update(cx, |desktop, cx| {
                let surface_index = desktop.workspace().sessions[0].terminals[0];
                desktop.workspace_mut().sessions[0].name = "shell-1cf389a9".to_owned();

                // Nothing known about where it runs: the generated name stands.
                let session = desktop.workspace().sessions[0].clone();
                assert_eq!(
                    desktop.session_display_title(&session, cx),
                    "shell-1cf389a9"
                );
                assert!(desktop.session_worktree(&session, cx).is_none());

                // The runtime reports where it started: repo and branch.
                desktop.surface_cwds[surface_index] = Some(repo.clone());
                let session = desktop.workspace().sessions[0].clone();
                assert_eq!(
                    desktop.session_display_title(&session, cx),
                    "ultraplexr@main"
                );
                assert_eq!(
                    desktop
                        .session_worktree(&session, cx)
                        .map(|worktree| worktree.label()),
                    Some("ultraplexr@main".to_owned())
                );

                // A name a person chose is never overridden.
                desktop.workspace_mut().sessions[0].name = "release prep".to_owned();
                let session = desktop.workspace().sessions[0].clone();
                assert_eq!(desktop.session_display_title(&session, cx), "release prep");
                assert!(
                    desktop.session_worktree(&session, cx).is_some(),
                    "the place is still shown in the detail line"
                );
            });
        });
        // The derived title must survive a real layout pass.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[gpui::test]
    fn session_management_popover_and_rename_editor_paint(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = crate::create_surfaces(window, cx);
            UltraplexrDesktop::new(surfaces, cx.focus_handle())
        });

        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.toggle_session_menu(0, window, cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.begin_session_rename(0, window, cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        // The anchored terminate confirmation, and the Remove entry a finished
        // Session shows, both have to survive a real layout pass.
        cx.update(|_, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.request_session_termination_at(0, cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                desktop.pending_termination = None;
                desktop.workspace_mut().sessions[0].status = SessionStatus::Closed;
                desktop.toggle_session_menu(0, window, cx);
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn detected_agents_paint_as_clickable_terminal_children(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = crate::create_surfaces(window, cx);
            UltraplexrDesktop::new(surfaces, cx.focus_handle())
        });

        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                assert_eq!(desktop.workspace().sessions[0].terminals, [0, 1]);
                desktop.surface_foreground_processes[0] =
                    Some(ultraplexr_protocol::TerminalForegroundProcess {
                        process_id: 41,
                        executable: "codex".to_owned(),
                    });
                desktop.surface_foreground_processes[1] =
                    Some(ultraplexr_protocol::TerminalForegroundProcess {
                        process_id: 42,
                        executable: "opencode".to_owned(),
                    });
                assert_eq!(
                    desktop.session_primary_agent(&desktop.workspace().sessions[0]),
                    Some(crate::provider_usage::ProviderKind::Codex)
                );
                assert_eq!(
                    desktop.terminal_agent(1),
                    Some(crate::provider_usage::ProviderKind::OpenCode)
                );
                desktop.toggle_session_expansion(0, cx);
                desktop.select_terminal_in_session(0, 1, window, cx);
                assert!(desktop.session_sidebar.expanded_sessions.contains(&0));
                assert_eq!(desktop.workspace().selected_session, 0);
                assert_eq!(desktop.active_surface, Some(1));
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}
