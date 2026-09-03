use std::{collections::HashSet, time::Duration};

#[cfg(not(test))]
use crate::theme::SIGNAL;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, CursorStyle, FocusHandle, FontWeight,
    IntoElement, KeyDownEvent, MouseButton, MouseMoveEvent, ScrollHandle, Window, accesskit::Role,
    deferred, div, prelude::*, px, relative, svg,
};
#[cfg(not(test))]
use termi9ne_core::{AttentionKind, SessionId, SignalId};

use crate::{
    SessionStatus, SessionView, Termi9neDesktop,
    app_zoom::ui_size,
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
const MAX_SESSION_NAME_CHARS: usize = 64;

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
    history_hits: Vec<HistoryHit>,
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
            history_hits: Vec::new(),
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
        self.menu_open = None;
        self.rename = None;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SessionRenameDraft {
    index: usize,
    value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HistoryHit {
    session_index: usize,
    surface_index: usize,
    line: usize,
    column: usize,
    preview: String,
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
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(PANEL))
        .child(
            svg()
                .path(kind.icon_path())
                .size(px((size - 5.0).max(8.0)))
                .text_color(rgb(token)),
        )
        .into_any_element()
}

impl Termi9neDesktop {
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
    fn archived_terminal_tray(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let count = self.archived_terminals.len() + self.detached_groups.len();
        if count == 0 || self.shared_mode {
            return None;
        }
        let open = self.archive_drawer_open;
        let archived = self.archived_terminals.clone();
        let detached = self.detached_groups.clone();
        Some(
            div()
                .border_b_1()
                .border_color(rgb(HAIRLINE))
                .child(
                    div()
                        .id("archived-session-toggle")
                        .role(Role::Button)
                        .aria_label("Detached and archived sessions")
                        .aria_expanded(open)
                        .h(px(28.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(TRACE))
                        .hover(|row| row.bg(rgb(PANEL)).text_color(rgb(CHALK)))
                        .on_click(cx.listener(|desktop, _, _, cx| {
                            desktop.toggle_archive_drawer(cx);
                        }))
                        .child(if open { "⌄" } else { "›" })
                        .child(div().ml_1().child("Detached & archive"))
                        .child(div().ml_auto().child(count.to_string())),
                )
                .when(open, |tray| {
                    tray.child(
                        div()
                            .id("archived-session-list")
                            .max_h(px(148.0))
                            .overflow_y_scroll()
                            .pb_1()
                            .children(detached.into_iter().map(|group| {
                                let group_id = group.group_id;
                                let version = group.version;
                                div()
                                    .id(format!("reattach-group-{group_id}"))
                                    .role(Role::Button)
                                    .aria_label(format!("Reattach session group {}", group.name))
                                    .h(px(30.0))
                                    .mx_1()
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .font_family(UI_FONT)
                                    .text_xs()
                                    .text_color(rgb(TRACE))
                                    .hover(|row| row.bg(rgb(PANEL)).text_color(rgb(CHALK)))
                                    .on_click(cx.listener(move |desktop, _, _, cx| {
                                        desktop.restore_detached_group(group_id, version, cx);
                                    }))
                                    .child(
                                        div()
                                            .max_w(px(132.0))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .font_family(PRODUCT_FONT)
                                            .text_color(rgb(CHALK))
                                            .child(group.name),
                                    )
                                    .child(div().ml_auto().child("Reattach"))
                            }))
                            .children(archived.into_iter().map(|terminal| {
                                let session_id = terminal.session_id;
                                let kind = if terminal.mission_id.is_some() {
                                    "agent history"
                                } else {
                                    "terminal history"
                                };
                                div()
                                    .id(format!("restore-terminal-{session_id}"))
                                    .role(Role::Button)
                                    .aria_label(format!("Restore archived {kind}"))
                                    .h(px(30.0))
                                    .mx_1()
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded(px(4.0))
                                    .cursor_pointer()
                                    .font_family(UI_FONT)
                                    .text_xs()
                                    .text_color(rgb(TRACE))
                                    .hover(|row| row.bg(rgb(PANEL)).text_color(rgb(CHALK)))
                                    .on_click(cx.listener(move |desktop, _, _, cx| {
                                        desktop.restore_archived_terminal(session_id, cx);
                                    }))
                                    .child(
                                        div()
                                            .font_family(PRODUCT_FONT)
                                            .text_color(rgb(CHALK))
                                            .child(crate::short_id(session_id)),
                                    )
                                    .child(div().ml_2().child(kind))
                                    .child(div().ml_auto().child("Restore"))
                            })),
                    )
                })
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
                .rounded(px(5.0))
                .border_1()
                .border_color(rgb(SIGNAL).alpha(0.5))
                .bg(rgb(PANEL))
                .overflow_hidden()
                .child(
                    div()
                        .h(px(27.0))
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
                        .min_h(px(44.0))
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
                        .child(div().size(px(7.0)).rounded_full().bg(rgb(SIGNAL)))
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
                renamed_group = Some(session.group_id);
            }
            self.persist_workspaces();
        }
        #[cfg(not(test))]
        if let Some(group_id) = renamed_group {
            self.sync_session_group(
                group_id,
                Some(termi9ne_protocol::SessionGroupChange::Rename {
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
        open_upward: bool,
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
            .absolute()
            .right(px(4.0))
            .when(!open_upward, |popover| popover.top(px(29.0)))
            .when(open_upward, |popover| popover.bottom(px(29.0)))
            .w(px(228.0))
            .p_2()
            .flex()
            .flex_col()
            .gap_2()
            .rounded(px(5.0))
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
                            .h(px(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .rounded(px(3.0))
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
                            .h(px(24.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .rounded(px(3.0))
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
        self.archive_session(index, cx);
        #[cfg(not(test))]
        if let Some(session) = self.workspace().sessions.get(index) {
            let group_id = session.group_id;
            self.sync_session_group(
                group_id,
                Some(termi9ne_protocol::SessionGroupChange::SetDetached { detached: true }),
                cx,
            );
        }
        #[cfg(test)]
        {
            let workspace = self.workspace_mut();
            if index < workspace.sessions.len() {
                workspace.sessions.remove(index);
                workspace.selected_session = workspace
                    .selected_session
                    .min(workspace.sessions.len().saturating_sub(1));
            }
        }
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
                Some(termi9ne_protocol::SessionGroupChange::SetPinned { pinned }),
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
                Some(termi9ne_protocol::SessionGroupChange::SetDetached { detached: true }),
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
        self.session_sidebar.history_hits.clear();
        self.focus_sidebar_search(window, cx);
    }

    fn refresh_history_search(&mut self, cx: &mut Context<Self>) {
        let query = self.session_sidebar.query.trim().to_owned();
        if query.chars().count() < 2 {
            self.session_sidebar.history_hits.clear();
            return;
        }
        let targets = self
            .workspace()
            .sessions
            .iter()
            .enumerate()
            .flat_map(|(session_index, session)| {
                session
                    .terminals
                    .iter()
                    .copied()
                    .map(move |surface_index| (session_index, surface_index))
            })
            .collect::<Vec<_>>();
        let mut hits = Vec::new();
        for (session_index, surface_index) in targets {
            let Some(surface) = self.surfaces.get(surface_index) else {
                continue;
            };
            let Ok(matches) = surface.read(cx).search(&query, 8) else {
                continue;
            };
            hits.extend(matches.into_iter().map(|found| HistoryHit {
                session_index,
                surface_index,
                line: found.line,
                column: found.column,
                preview: found.preview,
            }));
            if hits.len() >= 24 {
                hits.truncate(24);
                break;
            }
        }
        self.session_sidebar.history_hits = hits;
    }

    fn reveal_history_hit(&mut self, hit: HistoryHit, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace_mut().selected_session = hit.session_index;
        self.active_surface = Some(hit.surface_index);
        if let Some(surface) = self.surfaces.get(hit.surface_index) {
            surface.update(cx, |surface, cx| {
                surface.reveal_history_line(hit.line, cx);
            });
            window.focus(&surface.read(cx).focus_handle_owned(), cx);
        }
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
                if self.session_sidebar.query.is_empty() {
                    self.session_sidebar.search_open = false;
                    self.focus_active_terminal(window, cx);
                } else {
                    self.session_sidebar.query.clear();
                    self.session_sidebar.history_hits.clear();
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
        if self.session_sidebar.history_hits.is_empty() {
            return None;
        }
        Some(
            div()
                .mx_2()
                .mb_2()
                .rounded(px(5.0))
                .border_1()
                .border_color(rgb(HAIRLINE))
                .bg(rgb(PANEL))
                .overflow_hidden()
                .child(
                    div()
                        .h(px(26.0))
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
                                .child(self.session_sidebar.history_hits.len().to_string()),
                        ),
                )
                .children(
                    self.session_sidebar
                        .history_hits
                        .iter()
                        .take(8)
                        .cloned()
                        .enumerate()
                        .map(|(index, hit)| {
                            let session_name =
                                self.workspace().sessions[hit.session_index].name.clone();
                            let selected_hit = hit.clone();
                            div()
                                .id(("terminal-history-hit", index))
                                .h(px(42.0))
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
                                        .child(hit.preview),
                                )
                                .child(
                                    div()
                                        .font_family(UI_FONT)
                                        .text_size(ui_size(9.0))
                                        .text_color(rgb(TRACE))
                                        .child(format!(
                                            "{session_name} · line {}:{}",
                                            hit.line + 1,
                                            hit.column + 1
                                        )),
                                )
                        }),
                )
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
                Some(termi9ne_protocol::TerminalSessionStatus::Running) => SessionStatus::Idle,
                Some(termi9ne_protocol::TerminalSessionStatus::Failed) => SessionStatus::Terminated,
                Some(termi9ne_protocol::TerminalSessionStatus::Exited) | None => {
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
        if let Some(agent) = agent {
            return agent.label().to_owned();
        }
        self.surfaces
            .get(surface_index)
            .and_then(|surface| surface.read(cx).terminal_title().map(str::to_owned))
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| format!("Terminal {}", surface_index + 1))
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
            .size(px(20.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .rounded(px(5.0))
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
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .size(px(5.0))
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
                            .size(px(18.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
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
                    .h(px(TERMINAL_CHILD_HEIGHT))
                    .ml(px(26.0))
                    .mr_1()
                    .pl(px(7.0))
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .cursor_pointer()
                    .rounded(px(5.0))
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
                    .child(div().size(px(5.0)).rounded_full().bg(rgb(status.color())))
            })
            .collect::<Vec<_>>();
        let animation_id = format!("session-children-{session_index}-{expanded}");
        div()
            .relative()
            .h(px(if expanded { target_height } else { 0.0 }))
            .overflow_hidden()
            .opacity(if expanded { 1.0 } else { 0.0 })
            .when(!expanded, |children| children.invisible())
            .child(
                div()
                    .absolute()
                    .left(px(17.0))
                    .top(px(1.0))
                    .bottom(px(5.0))
                    .w(px(1.0))
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
                    children.h(px(target_height * progress)).opacity(progress)
                },
            )
            .into_any_element()
    }

    fn expanded_session_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let session = &self.workspace().sessions[index];
        let selected = self.workspace().selected_session == index;
        let status_color = session.status.color();
        let name = session.name.clone();
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
        let status_detail = format!(
            "{identity} · {} · {terminal_count} terminal{}",
            status_word(session.status),
            if terminal_count == 1 { "" } else { "s" }
        );
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
            session.name,
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
        let session_count = self.workspace().sessions.len();
        let remaining_below = session_count.saturating_sub(index.saturating_add(1));
        let open_menu_upward = index >= 4 && remaining_below < 4;
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
            .role(Role::Tab)
            .aria_label(accessibility_label)
            .aria_selected(selected)
            .aria_expanded(expanded)
            .group("session-row")
            .relative()
            .h(px(SESSION_ROW_HEIGHT))
            .mx_1()
            .flex()
            .items_center()
            .min_w_0()
            .cursor_pointer()
            .rounded(px(5.0))
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
                        .top(px(6.0))
                        .bottom(px(6.0))
                        .w(px(2.0))
                        .rounded(px(1.0))
                        .bg(rgb(RELAY)),
                )
            })
            .child(
                div()
                    .ml(px(7.0))
                    .mr(px(7.0))
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .flex_none()
                    .child(
                        div()
                            .w(px(8.0))
                            .font_family(UI_FONT)
                            .text_size(ui_size(9.0))
                            .text_color(rgb(if expanded { CHALK } else { TRACE }))
                            .child(if expanded { "⌄" } else { "›" }),
                    )
                    .child(primary_agent.map_or_else(
                        || {
                            div()
                                .relative()
                                .size(px(16.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .bg(rgb(PANEL))
                                .font_family(UI_FONT)
                                .text_size(ui_size(9.0))
                                .text_color(rgb(TRACE))
                                .child("›_")
                                .child(
                                    div()
                                        .absolute()
                                        .right(px(-1.0))
                                        .bottom(px(-1.0))
                                        .size(px(5.0))
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
                                        .right(px(-1.0))
                                        .bottom(px(-1.0))
                                        .size(px(5.0))
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
                    .min_w(px(56.0))
                    .overflow_hidden()
                    .when(!renaming, |name_row| {
                        name_row.child(
                            div()
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
                                .h(px(24.0))
                                .px_1()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .overflow_hidden()
                                .rounded(px(3.0))
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
                                        .ml(px(1.0))
                                        .w(px(1.0))
                                        .h(px(14.0))
                                        .flex_none()
                                        .bg(rgb(RELAY)),
                                ),
                        )
                    })
                    .when(!renaming, |name_row| {
                        name_row.child(
                            div()
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
                    .gap(px(2.0))
                    .children(terminal_quick_switches)
                    .when(hidden_terminal_count > 0, |marks| {
                        marks.child(
                            div()
                                .ml(px(1.0))
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
                        .size(px(22.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(px(3.0))
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
                row.child(
                    deferred(self.session_termination_popover(index, open_menu_upward, cx))
                        .with_priority(2),
                )
            })
            .when(menu_open, |row| {
                row.child(
                    deferred(
                        div()
                            .id(("session-menu", index))
                            .absolute()
                            .right(px(4.0))
                            .when(!open_menu_upward, |menu| menu.top(px(29.0)))
                            .when(open_menu_upward, |menu| menu.bottom(px(29.0)))
                            .w(px(196.0))
                            .py_1()
                            .flex()
                            .flex_col()
                            .rounded(px(5.0))
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
                                    .role(Role::MenuItem)
                                    .aria_label("Rename session")
                                    .h(px(30.0))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .rounded(px(3.0))
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
                                            .w(px(22.0))
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
                                    .h(px(30.0))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .rounded(px(3.0))
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
                                            .w(px(22.0))
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
                                    .h(px(30.0))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .rounded(px(3.0))
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
                                            .w(px(22.0))
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
                                        .h(px(30.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .rounded(px(3.0))
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
                                                .w(px(22.0))
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
                                        .h(px(30.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .child(
                                            div()
                                                .w(px(22.0))
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
                                                            .size(px(22.0))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .cursor_pointer()
                                                            .rounded(px(3.0))
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
                                                                |_, _, cx| cx.stop_propagation(),
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
                                                            .size(px(22.0))
                                                            .flex()
                                                            .items_center()
                                                            .justify_center()
                                                            .cursor_pointer()
                                                            .rounded(px(3.0))
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
                                                                |_, _, cx| cx.stop_propagation(),
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
                                        .h(px(31.0))
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
                                                .w(px(22.0))
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
                                        .h(px(31.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .mt_1()
                                        .border_t_1()
                                        .border_color(rgb(HAIRLINE))
                                        .text_color(rgb(TRACE))
                                        .hover(|item| item.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(move |desktop, _, _, cx| {
                                            cx.stop_propagation();
                                            desktop.archive_session_from_menu(index, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(px(22.0))
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
                                        .h(px(31.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .rounded(px(3.0))
                                        .text_color(rgb(TRACE))
                                        .hover(|item| item.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                            cx.stop_propagation();
                                        })
                                        .on_click(cx.listener(move |desktop, _, _, cx| {
                                            cx.stop_propagation();
                                            desktop.remove_session_from_menu(index, cx);
                                        }))
                                        .child(
                                            div()
                                                .w(px(22.0))
                                                .flex_none()
                                                .font_family(UI_FONT)
                                                .child("✕"),
                                        )
                                        .child("Remove"),
                                )
                            }),
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
        let accessibility_label = format!(
            "{}, {}, {} terminal{}",
            session.name,
            session.status.label(&session.actor),
            terminal_count,
            if terminal_count == 1 { "" } else { "s" }
        );

        div()
            .id(("collapsed-session-row", index))
            .role(Role::Tab)
            .aria_label(accessibility_label)
            .aria_selected(selected)
            .relative()
            .h(px(32.0))
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
                        .top(px(7.0))
                        .bottom(px(7.0))
                        .w(px(2.0))
                        .bg(rgb(RELAY)),
                )
            })
            .child(
                div()
                    .relative()
                    .size(px(18.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
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
                            .right(px(-1.0))
                            .bottom(px(-1.0))
                            .size(px(5.0))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(DECK))
                            .bg(rgb(status_color)),
                    )
                    .when(terminal_count > 1, |icon| {
                        icon.child(
                            div()
                                .absolute()
                                .right(px(-5.0))
                                .top(px(-5.0))
                                .min_w(px(12.0))
                                .h(px(12.0))
                                .px(px(2.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(6.0))
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
            .h(px(28.0))
            .mx_2()
            .mb(px(3.0))
            .px_2()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .cursor_text()
            .rounded(px(4.0))
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
                        .size(px(18.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .rounded(px(4.0))
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
            f32::from(window.viewport_size().width),
            self.sidebar_open,
            self.sidebar_custom_width,
        );
        let sessions = &self.workspace().sessions;
        let session_count = sessions.len();
        let visible = visible_indices(sessions, &self.session_sidebar.query);
        let no_matches = visible.is_empty() && !self.session_sidebar.query.is_empty();

        div()
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
            .when(self.sidebar_open, |sidebar| {
                match self.sidebar_custom_width {
                    Some(width) => sidebar.w(px(width)),
                    None => sidebar
                        .w(relative(EXPANDED_WIDTH_RATIO))
                        .min_w(px(EXPANDED_MIN_WIDTH))
                        .max_w(px(EXPANDED_MAX_WIDTH)),
                }
            })
            .when(!self.sidebar_open, |sidebar| sidebar.w(px(COLLAPSED_WIDTH)))
            .child(
                div()
                    .id(if self.sidebar_open {
                        "sidebar-header"
                    } else {
                        "collapse-sidebar"
                    })
                    .h(px(36.0))
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
                                    .ml(px(6.0))
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
                                    .size(px(22.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .rounded(px(4.0))
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
                                        .size(px(22.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .rounded(px(4.0))
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
                                    .ml(px(2.0))
                                    .size(px(22.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .rounded(px(4.0))
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
                    .children({
                        #[cfg(not(test))]
                        {
                            self.archived_terminal_tray(cx)
                        }
                        #[cfg(test)]
                        {
                            None::<AnyElement>
                        }
                    })
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
                                    .pt(px(2.0))
                                    .pb_2()
                                    .children(
                                        visible
                                            .iter()
                                            .copied()
                                            .map(|index| self.expanded_session_row(index, cx)),
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
                        .h(px(32.0))
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
            Termi9neDesktop::new(surfaces, cx.focus_handle())
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
            Termi9neDesktop::new(surfaces, cx.focus_handle())
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
            Termi9neDesktop::new(surfaces, cx.focus_handle())
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

    #[gpui::test]
    fn session_management_popover_and_rename_editor_paint(cx: &mut gpui::TestAppContext) {
        let (desktop, cx) = cx.add_window_view(|window, cx| {
            let surfaces = crate::create_surfaces(window, cx);
            Termi9neDesktop::new(surfaces, cx.focus_handle())
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
            Termi9neDesktop::new(surfaces, cx.focus_handle())
        });

        cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                assert_eq!(desktop.workspace().sessions[0].terminals, [0, 1]);
                desktop.surface_foreground_processes[0] =
                    Some(termi9ne_protocol::TerminalForegroundProcess {
                        process_id: 41,
                        executable: "codex".to_owned(),
                    });
                desktop.surface_foreground_processes[1] =
                    Some(termi9ne_protocol::TerminalForegroundProcess {
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
