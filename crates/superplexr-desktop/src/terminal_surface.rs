use std::time::Duration;
#[cfg(not(test))]
use std::time::Instant;
use std::{ops::Range, sync::Arc};

#[cfg(not(test))]
use gpui::EventEmitter;
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, EntityInputHandler, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollWheelEvent, SharedString, Subscription,
    UTF16Selection, Window, div, point, prelude::*, px, size,
};
use superplexr_client::DaemonSession;
#[cfg(not(test))]
use superplexr_client::EventSubscription;
#[cfg(not(test))]
use superplexr_protocol::ServerEvent;
#[cfg(test)]
use superplexr_terminal::TerminalError;
use superplexr_terminal::{
    FullFrame, GridSize, HistoryViewport, KeyAction, KeyInput, KeyModifiers,
    MouseAction as TerminalMouseAction, MouseButton as TerminalMouseButton, MouseInput,
    PasteConfirmation, SearchMatch, SelectionPoint, TerminalAction, TerminalEffects, TerminalModel,
    ViewportScroll,
};

use crate::{
    app_zoom::ui_size,
    overlay_scrollbar::{
        AutoHideScrollbar, accumulate_terminal_scroll_rows, terminal_history_thumb,
    },
    terminal_element::{TerminalElement, terminal_color},
    theme::{ACTIVE, CHALK, FAULT, HAIRLINE, PANEL, RELAY, SIGNAL, TRACE, UI_FONT, rgb},
};

#[cfg(test)]
const FIXTURE_OUTPUT: &[u8] = b"\x1b[2J\x1b[H$ superplexr verify --platform native\r\n\x1b[1;34mghostty\x1b[0m  terminal state pinned\r\n\x1b[1;32minput\x1b[0m    keyboard, IME, focus and paste connected\r\n\x1b[1;33mnext\x1b[0m     connect a real PTY";

/// GPUI-owned interactive surface around the backend-neutral terminal module.
pub(crate) struct TerminalSurface {
    element_id: SharedString,
    grid_id: SharedString,
    model: Option<TerminalModel>,
    session: Option<DaemonSession>,
    frame: Arc<FullFrame>,
    requested_grid: GridSize,
    geometry: Option<TerminalGeometry>,
    selection_anchor: Option<SelectionPoint>,
    /// The viewer's selection over the visible grid. Never a request: it
    /// works on terminals this surface cannot control, and on finished ones.
    selection: Option<crate::selection::Selection>,
    focus_handle: FocusHandle,
    composition: String,
    caps_lock: bool,
    pending_paste: Option<PasteConfirmation>,
    last_encoded: String,
    last_error: Option<String>,
    /// `Some(success)` once the process has exited or the session failed.
    ended: Option<bool>,
    writable: bool,
    historical: bool,
    history_viewport: HistoryViewport,
    scroll_rows_before_bottom: u32,
    scroll_row_remainder: f32,
    scrollbar: AutoHideScrollbar,
    force_kill_available: bool,
    #[cfg(not(test))]
    presented: bool,
    #[cfg(not(test))]
    last_activity_emitted: Option<Instant>,
    #[cfg(not(test))]
    event_subscription: Option<EventSubscription>,
    /// Ordered off-thread sender for daemon input; keeps keystrokes from
    /// blocking the render loop on socket round trips.
    #[cfg(not(test))]
    input_queue: Option<InputQueue>,
    _subscriptions: Vec<Subscription>,
}

#[cfg(not(test))]
#[derive(Clone, Debug)]
pub(crate) enum TerminalSurfaceEvent {
    Activity,
    Exited {
        success: bool,
    },
    Failed,
    /// Keyboard focus landed on this surface, so it is the input target.
    Focused,
    /// The person asked for a fresh shell in place of this ended one.
    RestartRequested,
    /// The person dismissed this ended terminal.
    CloseRequested,
}

#[derive(Clone, Copy)]
struct TerminalGeometry {
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
}

impl TerminalSurface {
    pub(crate) fn terminal_title(&self) -> Option<&str> {
        self.frame.title.as_deref()
    }

    /// True once the process exited or the session failed; input is refused.
    pub(crate) fn has_ended(&self) -> bool {
        self.ended.is_some()
    }

    #[cfg(not(test))]
    pub(crate) fn current_directory(&self) -> Option<&str> {
        self.frame.current_directory.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Result<Self, TerminalError> {
        Self::fixture("ghostty-fixture", FIXTURE_OUTPUT, window, cx)
    }

    #[cfg(test)]
    pub(crate) fn fixture(
        id: &str,
        output: &[u8],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, TerminalError> {
        let mut model = TerminalModel::new(GridSize::new(72, 12)?)?;
        model.advance(TerminalAction::Output(output))?;
        let frame = model.frame()?;
        let focus_handle = cx.focus_handle();
        let focus_in = cx.on_focus_in(&focus_handle, window, |surface, _window, cx| {
            surface.apply(TerminalAction::Focus { focused: true }, cx);
        });
        let focus_out = cx.on_focus_out(&focus_handle, window, |surface, _event, _window, cx| {
            surface.apply(TerminalAction::Focus { focused: false }, cx);
            surface.composition.clear();
            cx.notify();
        });

        Ok(Self {
            element_id: id.into(),
            grid_id: format!("{id}-grid").into(),
            model: Some(model),
            session: None,
            frame: Arc::new(frame),
            requested_grid: GridSize::new(72, 12)?,
            geometry: None,
            selection_anchor: None,
            selection: None,
            focus_handle,
            composition: String::new(),
            caps_lock: false,
            pending_paste: None,
            last_encoded: "focus the grid and type".to_owned(),
            last_error: None,
            ended: None,
            writable: true,
            historical: false,
            history_viewport: HistoryViewport::RowsBeforeBottom(0),
            scroll_rows_before_bottom: 0,
            scroll_row_remainder: 0.0,
            scrollbar: AutoHideScrollbar::default(),
            force_kill_available: false,
            #[cfg(not(test))]
            last_activity_emitted: None,
            #[cfg(not(test))]
            event_subscription: None,
            #[cfg(not(test))]
            input_queue: None,
            _subscriptions: vec![focus_in, focus_out],
        })
    }

    #[cfg(not(test))]
    pub(crate) fn live(
        id: &str,
        session: DaemonSession,
        historical: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Self, superplexr_client::ClientError> {
        let writable = !historical && session.claim_control(false).is_ok();
        let frame = if historical {
            session.history_frame(0)?
        } else {
            session.snapshot()?
        };
        let focus_handle = cx.focus_handle();
        let focus_in = cx.on_focus_in(&focus_handle, window, |surface, _window, cx| {
            surface.apply(TerminalAction::Focus { focused: true }, cx);
            cx.emit(TerminalSurfaceEvent::Focused);
        });
        let focus_out = cx.on_focus_out(&focus_handle, window, |surface, _event, _window, cx| {
            surface.apply(TerminalAction::Focus { focused: false }, cx);
            surface.composition.clear();
            cx.notify();
        });
        let requested_grid = frame.grid;
        let input_queue = InputQueue::start(session.clone(), cx);
        Ok(Self {
            element_id: id.into(),
            grid_id: format!("{id}-grid").into(),
            model: None,
            session: Some(session),
            frame: Arc::new(frame),
            requested_grid,
            geometry: None,
            selection_anchor: None,
            selection: None,
            focus_handle,
            composition: String::new(),
            caps_lock: false,
            pending_paste: None,
            last_encoded: if writable {
                "live PTY connected".to_owned()
            } else if historical {
                "retained history · scroll to navigate".to_owned()
            } else {
                "observer mode".to_owned()
            },
            last_error: None,
            ended: None,
            writable,
            historical,
            history_viewport: HistoryViewport::RowsBeforeBottom(0),
            scroll_rows_before_bottom: 0,
            scroll_row_remainder: 0.0,
            scrollbar: AutoHideScrollbar::default(),
            force_kill_available: false,
            presented: false,
            last_activity_emitted: None,
            event_subscription: None,
            input_queue: Some(input_queue),
            _subscriptions: vec![focus_in, focus_out],
        })
    }

    #[cfg(not(test))]
    pub(crate) fn set_presented(&mut self, presented: bool, cx: &mut Context<Self>) {
        if self.presented == presented {
            return;
        }
        self.presented = presented;
        if presented {
            if let Err(error) = self.start_event_subscription(cx) {
                self.last_error = Some(format!("terminal attach failed: {error}"));
            }
            cx.notify();
        } else if let Some(subscription) = self.event_subscription.take() {
            subscription.cancel();
        }
    }

    #[cfg(not(test))]
    fn start_event_subscription(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<(), superplexr_client::ClientError> {
        if self.historical || self.event_subscription.is_some() {
            return Ok(());
        }
        let Some(session) = self.session.clone() else {
            return Ok(());
        };
        let (event_send, mut event_receive) = tokio::sync::mpsc::unbounded_channel();
        let subscription = session.subscribe_with(move |event| event_send.send(event).is_ok())?;
        cx.spawn(async move |this, cx| {
            while let Some(event) = event_receive.recv().await {
                let mut events = Vec::with_capacity(16);
                events.push(event);
                while events.len() < 256 {
                    let Ok(event) = event_receive.try_recv() else {
                        break;
                    };
                    events.push(event);
                }
                if this
                    .update(cx, |surface, cx| {
                        for event in events {
                            surface.accept_runtime_event(event, cx);
                        }
                        if surface.presented {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        self.event_subscription = Some(subscription);
        Ok(())
    }

    pub(crate) fn focus_handle_owned(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub(crate) fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchMatch>, String> {
        if let Some(session) = &self.session {
            return session
                .search(query, false, limit)
                .map_err(|error| error.to_string());
        }
        self.model
            .as_ref()
            .expect("fixture surface must own a terminal model")
            .search(query, false, limit)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn reveal_history_line(&mut self, line: usize, cx: &mut Context<Self>) {
        if self.historical {
            let row = u32::try_from(line).unwrap_or(100_000).min(100_000);
            self.load_history_viewport(HistoryViewport::RowFromTop(row), cx);
            return;
        }
        if !self.apply(TerminalAction::Scroll(ViewportScroll::Top), cx) {
            return;
        }
        let delta = i32::try_from(line).unwrap_or(i32::MAX);
        self.apply(TerminalAction::Scroll(ViewportScroll::Delta(delta)), cx);
    }

    pub(crate) fn run_command(&mut self, command: &str, cx: &mut Context<Self>) {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\n');
        self.apply(
            TerminalAction::Paste {
                bytes: &bytes,
                confirmed: true,
            },
            cx,
        );
    }

    /// Insert text at the cursor without executing it. Used to hand evidence
    /// to an agent already running in this terminal.
    pub(crate) fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let bytes = text.as_bytes().to_vec();
        self.apply(
            TerminalAction::Paste {
                bytes: &bytes,
                confirmed: true,
            },
            cx,
        );
    }

    pub(crate) fn terminate(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            let result = if self.force_kill_available {
                session.kill()
            } else {
                session.terminate()
            };
            if let Err(error) = result {
                self.last_error = Some(error.to_string());
            } else {
                self.last_encoded = if self.force_kill_available {
                    "force kill sent".to_owned()
                } else {
                    "graded termination started".to_owned()
                };
            }
        } else {
            self.last_encoded = "fixture terminated".to_owned();
        }
        cx.notify();
    }

    pub(crate) fn interrupt(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            match session.interrupt() {
                Ok(()) => self.last_encoded = "SIGINT sent to foreground job".to_owned(),
                Err(error) => self.last_error = Some(error.to_string()),
            }
        } else {
            self.last_encoded = "fixture interrupt".to_owned();
        }
        cx.notify();
    }

    pub(crate) const fn requires_force_kill(&self) -> bool {
        self.force_kill_available
    }

    fn request_control(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        match session.claim_control(false) {
            Ok(()) => {
                self.writable = true;
                self.last_error = None;
                self.last_encoded = "terminal control acquired".to_owned();
            }
            Err(error) => self.last_error = Some(format!("control unavailable: {error}")),
        }
        cx.notify();
    }

    #[cfg(not(test))]
    pub(crate) fn synchronize_control_owner(
        &mut self,
        controller_surface_id: Option<uuid::Uuid>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = &self.session else {
            return;
        };
        if controller_surface_id == Some(session.surface_id()) {
            self.writable = true;
            self.last_error = None;
        } else {
            self.writable = false;
            if controller_surface_id.is_none() {
                self.request_control(cx);
                return;
            }
        }
        cx.notify();
    }

    pub(crate) fn resize_to(
        &mut self,
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
        cx: &mut Context<Self>,
    ) {
        if grid == self.requested_grid {
            return;
        }
        self.requested_grid = grid;
        self.apply(
            TerminalAction::Resize {
                grid,
                cell_width_px,
                cell_height_px,
            },
            cx,
        );
    }

    pub(crate) fn set_geometry(
        &mut self,
        bounds: Bounds<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
    ) {
        self.geometry = Some(TerminalGeometry {
            bounds,
            cell_width,
            line_height,
        });
    }

    pub(crate) fn cached_cell_width(&self, line_height: Pixels) -> Option<Pixels> {
        self.geometry
            .filter(|geometry| geometry.line_height == line_height)
            .map(|geometry| geometry.cell_width)
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        let copy_shortcut = event.keystroke.key == "c"
            && (modifiers.platform || (modifiers.control && modifiers.shift));
        if copy_shortcut {
            self.copy_visible(cx);
            cx.stop_propagation();
            return;
        }
        let paste_shortcut = event.keystroke.key == "v"
            && (modifiers.platform || (modifiers.control && modifiers.shift));
        if paste_shortcut {
            let confirm = modifiers.shift && self.pending_paste.is_some();
            self.paste_from_clipboard(confirm, cx);
            cx.stop_propagation();
            return;
        }

        if event.keystroke.is_ime_in_progress() {
            return;
        }
        // Not stopped, so it propagates to the desktop's key bindings.
        if is_desktop_shortcut(&event.keystroke) {
            return;
        }

        let input = key_input(
            &event.keystroke,
            if event.is_held {
                KeyAction::Repeat
            } else {
                KeyAction::Press
            },
            self.caps_lock,
        );
        if self.apply(TerminalAction::EncodeKey(&input), cx) {
            cx.stop_propagation();
        }
        window.invalidate_character_coordinates();
    }

    fn key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if is_desktop_shortcut(&event.keystroke) {
            return;
        }
        let input = key_input(&event.keystroke, KeyAction::Release, self.caps_lock);
        if self.apply(TerminalAction::EncodeKey(&input), cx) {
            cx.stop_propagation();
        }
    }

    pub(crate) fn paste_from_clipboard(&mut self, confirmed: bool, cx: &mut Context<Self>) {
        let bytes = if confirmed {
            self.pending_paste
                .as_ref()
                .map(|pending| pending.bytes.clone())
        } else {
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .map(String::into_bytes)
        };
        if let Some(bytes) = bytes {
            self.apply(
                TerminalAction::Paste {
                    bytes: &bytes,
                    confirmed,
                },
                cx,
            );
        }
    }

    #[cfg(not(test))]
    pub(crate) fn start_output_benchmark(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let command = b"i=0; while [ $i -lt 620 ]; do printf '\\033[H\\033[38;5;75msuperplexr frame %04d\\033[0m\\n' $i; j=0; while [ $j -lt 80 ]; do printf 'agent output '; j=$((j+1)); done; i=$((i+1)); sleep 0.008; done\n";
        if let Err(error) = session.paste(command.to_vec(), true) {
            self.last_error = Some(format!("renderer benchmark input failed: {error}"));
        }
        cx.notify();
    }

    pub(crate) fn copy_visible(&mut self, cx: &mut Context<Self>) {
        let visible = self
            .frame
            .rows
            .iter()
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_owned();
        let selected = self.selected_text();
        let copied_selection = selected.is_some();
        let text = selected
            .or_else(|| {
                self.session
                    .as_ref()
                    .and_then(|session| session.selection_text().ok().flatten())
                    .filter(|selection| !selection.is_empty())
            })
            .unwrap_or(visible);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.last_encoded = if copied_selection {
            "selection copied to clipboard".to_owned()
        } else {
            "visible terminal copied to clipboard".to_owned()
        };
        cx.notify();
    }

    /// The viewer's selected text, when there is any.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let selection = self.selection.filter(|selection| !selection.is_empty())?;
        let text = selection.text(&self.frame.rows, self.frame.grid.columns);
        (!text.is_empty()).then_some(text)
    }

    fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.caps_lock = event.capslock.on;
    }

    fn reveal_scrollbar(&mut self, cx: &mut Context<Self>) {
        let generation = self.scrollbar.reveal();
        cx.notify();
        self.schedule_scrollbar_hide(generation, cx);
    }

    fn schedule_scrollbar_hide(&mut self, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |surface, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            let _ = surface.update(cx, |surface, cx| {
                if surface.scrollbar.hide_if_current(generation) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn scroll_indicator(&self) -> Option<impl IntoElement> {
        if !self.scrollbar.visible() {
            return None;
        }
        let track_height = (self
            .geometry
            .map_or(f32::from(self.frame.grid.rows) * 18.0, |geometry| {
                f32::from(geometry.bounds.size.height)
            })
            - 8.0)
            .max(0.0);
        let thumb = terminal_history_thumb(track_height, self.scroll_rows_before_bottom);
        Some(
            div()
                .id("terminal-scrollbar")
                .debug_selector(|| "terminal-scrollbar".to_owned())
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
                        .bg(rgb(TRACE).alpha(0.82)),
                ),
        )
    }

    fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pixels = event.delta.pixel_delta(window.line_height()).y;
        let delta = accumulate_terminal_scroll_rows(
            f32::from(pixels),
            f32::from(window.line_height()),
            &mut self.scroll_row_remainder,
        );
        if self.historical {
            if delta != 0 {
                let amount = delta.unsigned_abs();
                let viewport = match self.history_viewport {
                    HistoryViewport::RowsBeforeBottom(row) => {
                        HistoryViewport::RowsBeforeBottom(if delta < 0 {
                            row.saturating_add(amount).min(100_000)
                        } else {
                            row.saturating_sub(amount)
                        })
                    }
                    HistoryViewport::RowFromTop(row) => HistoryViewport::RowFromTop(if delta < 0 {
                        row.saturating_sub(amount)
                    } else {
                        row.saturating_add(amount).min(100_000)
                    }),
                };
                if self.load_history_viewport(viewport, cx) {
                    // The selection is in viewport rows; scrolling moves them.
                    self.selection = None;
                    self.scroll_rows_before_bottom = match viewport {
                        HistoryViewport::RowsBeforeBottom(row)
                        | HistoryViewport::RowFromTop(row) => row,
                    };
                    self.reveal_scrollbar(cx);
                    cx.stop_propagation();
                }
            }
            return;
        }
        if self.frame.mouse_tracking {
            let button = if pixels < px(0.0) {
                TerminalMouseButton::ScrollUp
            } else {
                TerminalMouseButton::ScrollDown
            };
            if let Some(input) = self.mouse_input(
                TerminalMouseAction::Press,
                Some(button),
                event.position,
                event.modifiers,
                false,
            ) {
                self.apply(TerminalAction::Mouse(&input), cx);
                cx.stop_propagation();
            }
            return;
        }
        if delta != 0 && self.apply(TerminalAction::Scroll(ViewportScroll::Delta(delta)), cx) {
            let amount = delta.unsigned_abs();
            self.selection = None;
            self.scroll_rows_before_bottom = if delta < 0 {
                self.scroll_rows_before_bottom
                    .saturating_add(amount)
                    .min(100_000)
            } else {
                self.scroll_rows_before_bottom.saturating_sub(amount)
            };
            self.reveal_scrollbar(cx);
            cx.stop_propagation();
        }
    }

    fn load_history_viewport(&mut self, viewport: HistoryViewport, cx: &mut Context<Self>) -> bool {
        let Some(session) = &self.session else {
            return false;
        };
        let result = match viewport {
            HistoryViewport::RowsBeforeBottom(row) => session.history_frame(row),
            HistoryViewport::RowFromTop(row) => session.history_line(row),
        };
        match result {
            Ok(frame) => {
                if frame.grid != self.frame.grid {
                    self.selection = None;
                }
                self.frame = Arc::new(frame);
                self.history_viewport = viewport;
                self.last_error = None;
                self.last_encoded = "retained history".to_owned();
                cx.notify();
                true
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                cx.notify();
                false
            }
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let Some(point) = self.selection_point(event.position) else {
            return;
        };
        if (event.modifiers.platform || event.modifiers.control)
            && let Some(uri) = self.hyperlink_at(point)
            && matches!(uri.split_once(':'), Some(("http" | "https", _)))
        {
            cx.open_url(&uri);
            cx.stop_propagation();
            return;
        }
        if self.frame.mouse_tracking
            && !event.modifiers.shift
            && let Some(input) = self.mouse_input(
                TerminalMouseAction::Press,
                terminal_mouse_button(event.button),
                event.position,
                event.modifiers,
                true,
            )
        {
            self.apply(TerminalAction::Mouse(&input), cx);
            cx.stop_propagation();
            return;
        }
        self.selection_anchor = Some(point);
        self.selection = Some(crate::selection::Selection {
            anchor: point,
            head: point,
            rectangle: event.modifiers.alt,
        });
        cx.notify();
        cx.stop_propagation();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let near_edge = self.geometry.is_some_and(|geometry| {
            geometry.bounds.contains(&event.position)
                && event.position.x >= geometry.bounds.right() - px(12.0)
        });
        if near_edge {
            if self.scrollbar.enter_edge() {
                cx.notify();
            }
        } else if let Some(generation) = self.scrollbar.leave_edge() {
            self.schedule_scrollbar_hide(generation, cx);
        }
        if self.frame.mouse_tracking
            && !event.modifiers.shift
            && let Some(input) = self.mouse_input(
                TerminalMouseAction::Motion,
                None,
                event.position,
                event.modifiers,
                event.dragging(),
            )
        {
            self.apply(TerminalAction::Mouse(&input), cx);
            cx.stop_propagation();
            return;
        }
        let Some(anchor) = self.selection_anchor.filter(|_| event.dragging()) else {
            return;
        };
        let Some(head) = self.selection_point(event.position) else {
            return;
        };
        self.selection = Some(crate::selection::Selection {
            anchor,
            head,
            rectangle: event.modifiers.alt,
        });
        cx.notify();
        cx.stop_propagation();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.frame.mouse_tracking
            && !event.modifiers.shift
            && let Some(input) = self.mouse_input(
                TerminalMouseAction::Release,
                terminal_mouse_button(event.button),
                event.position,
                event.modifiers,
                false,
            )
        {
            self.apply(TerminalAction::Mouse(&input), cx);
            cx.stop_propagation();
        }
        self.selection_anchor = None;
        // A click that never became a drag clears whatever was selected.
        if self.selection.is_some_and(|selection| selection.is_empty()) {
            self.selection = None;
            cx.notify();
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn mouse_input(
        &self,
        action: TerminalMouseAction,
        button: Option<TerminalMouseButton>,
        position: gpui::Point<Pixels>,
        modifiers: gpui::Modifiers,
        any_button_pressed: bool,
    ) -> Option<MouseInput> {
        let geometry = self.geometry?;
        let width = f32::from(geometry.bounds.size.width).max(1.0);
        let height = f32::from(geometry.bounds.size.height).max(1.0);
        Some(MouseInput {
            action,
            button,
            modifiers: KeyModifiers {
                shift: modifiers.shift,
                alt: modifiers.alt,
                control: modifiers.control,
                super_key: modifiers.platform,
                caps_lock: self.caps_lock,
                num_lock: false,
            },
            x_px: f32::from(position.x - geometry.bounds.left()).clamp(0.0, width - 1.0) as u32,
            y_px: f32::from(position.y - geometry.bounds.top()).clamp(0.0, height - 1.0) as u32,
            screen_width_px: width as u32,
            screen_height_px: height as u32,
            cell_width_px: f32::from(geometry.cell_width).max(1.0) as u32,
            cell_height_px: f32::from(geometry.line_height).max(1.0) as u32,
            any_button_pressed,
        })
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn selection_point(&self, position: gpui::Point<Pixels>) -> Option<SelectionPoint> {
        let geometry = self.geometry?;
        let column = ((f32::from(position.x - geometry.bounds.left())
            / f32::from(geometry.cell_width))
        .floor()
        .clamp(0.0, f32::from(self.frame.grid.columns.saturating_sub(1))))
            as u16;
        let row = ((f32::from(position.y - geometry.bounds.top())
            / f32::from(geometry.line_height))
        .floor()
        .clamp(0.0, f32::from(self.frame.grid.rows.saturating_sub(1)))) as u16;
        Some(SelectionPoint { column, row })
    }

    fn hyperlink_at(&self, point: SelectionPoint) -> Option<String> {
        self.frame
            .rows
            .get(usize::from(point.row))?
            .cells
            .get(usize::from(point.column))?
            .hyperlink
            .clone()
    }

    fn apply(&mut self, action: TerminalAction<'_>, cx: &mut Context<Self>) -> bool {
        if let Some(session) = &self.session {
            if !self.writable {
                if !matches!(action, TerminalAction::Focus { .. }) {
                    self.last_error = Some("observer mode; request control to interact".to_owned());
                    cx.notify();
                }
                return false;
            }
            #[cfg(not(test))]
            {
                let _ = session;
                let queued = match action {
                    TerminalAction::EncodeKey(input) => QueuedInput::Key(input.clone()),
                    TerminalAction::Paste { bytes, confirmed } => QueuedInput::Paste {
                        bytes: bytes.to_vec(),
                        confirmed,
                    },
                    TerminalAction::Focus { focused } => QueuedInput::Focus(focused),
                    TerminalAction::Scroll(scroll) => QueuedInput::Scroll(scroll),
                    TerminalAction::Select {
                        anchor,
                        head,
                        rectangle,
                    } => QueuedInput::Select {
                        anchor,
                        head,
                        rectangle,
                    },
                    TerminalAction::ClearSelection => QueuedInput::ClearSelection,
                    TerminalAction::Mouse(input) => QueuedInput::Mouse(*input),
                    TerminalAction::Resize {
                        grid,
                        cell_width_px,
                        cell_height_px,
                    } => QueuedInput::Resize {
                        grid,
                        cell_width_px,
                        cell_height_px,
                    },
                    TerminalAction::Output(_) => return false,
                };
                let accepted = self
                    .input_queue
                    .as_ref()
                    .is_some_and(|queue| queue.send(queued));
                if !accepted {
                    self.last_error = Some("terminal input worker stopped".to_owned());
                    cx.notify();
                }
                return accepted;
            }
            #[cfg(test)]
            {
                let _ = (session, cx);
                return false;
            }
        }

        match self
            .model
            .as_mut()
            .expect("fixture surfaces retain their terminal model")
            .advance(action)
        {
            Ok(effects) => {
                let wrote = !effects.pty_writes.is_empty();
                self.consume_effects(effects);
                self.last_error = None;
                cx.notify();
                wrote
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                cx.notify();
                false
            }
        }
    }

    fn consume_effects(&mut self, effects: TerminalEffects) {
        if let Some(write) = effects.pty_writes.last() {
            self.last_encoded = escaped_bytes(write);
            self.pending_paste = None;
        }
        if let Some(confirmation) = effects.paste_confirmations.into_iter().next() {
            self.pending_paste = Some(confirmation);
            self.last_encoded = "paste blocked pending confirmation".to_owned();
        }
    }

    #[cfg(not(test))]
    fn accept_runtime_event(&mut self, event: ServerEvent, cx: &mut Context<Self>) {
        match event {
            ServerEvent::TerminalFrame { frame, .. } => {
                if frame.sequence >= self.frame.sequence {
                    if frame.grid != self.frame.grid {
                        self.selection = None;
                    }
                    self.frame = Arc::from(frame);
                    self.emit_activity_heartbeat(cx);
                }
            }
            ServerEvent::TerminalDelta { delta, .. } => {
                match delta.apply_to(Arc::make_mut(&mut self.frame)) {
                    Ok(()) => self.emit_activity_heartbeat(cx),
                    Err(error) => {
                        self.last_error = Some(format!("terminal resync required: {error}"))
                    }
                }
            }
            ServerEvent::TerminalBell { count, .. } => {
                self.last_encoded = format!("bell ×{count}");
            }
            ServerEvent::PasteConfirmation { confirmation, .. } => {
                self.pending_paste = Some(confirmation);
                self.last_encoded = "paste blocked pending confirmation".to_owned();
            }
            ServerEvent::TerminalTerminationEscalationRequired { .. } => {
                self.force_kill_available = true;
                self.last_error =
                    Some("process ignored SIGHUP and SIGTERM; force kill is available".to_owned());
            }
            ServerEvent::TerminalExited { code, success, .. } => {
                self.force_kill_available = false;
                self.writable = false;
                self.historical = true;
                self.history_viewport = HistoryViewport::RowsBeforeBottom(0);
                self.last_encoded = if success {
                    format!("exited ({code})")
                } else {
                    format!("failed ({code})")
                };
                self.ended = Some(success);
                self.last_error = None;
                cx.emit(TerminalSurfaceEvent::Exited { success });
            }
            ServerEvent::TerminalFailed { message, .. } => {
                self.force_kill_available = false;
                self.writable = false;
                self.historical = true;
                self.history_viewport = HistoryViewport::RowsBeforeBottom(0);
                self.last_encoded = "failed".to_owned();
                self.last_error = Some(message);
                self.ended = Some(false);
                cx.emit(TerminalSurfaceEvent::Failed);
            }
        }
    }

    #[cfg(not(test))]
    fn emit_activity_heartbeat(&mut self, cx: &mut Context<Self>) {
        const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(200);
        let now = Instant::now();
        if self
            .last_activity_emitted
            .is_none_or(|last| now.duration_since(last) >= HEARTBEAT_INTERVAL)
        {
            self.last_activity_emitted = Some(now);
            cx.emit(TerminalSurfaceEvent::Activity);
        }
    }
}

#[cfg(not(test))]
impl EventEmitter<TerminalSurfaceEvent> for TerminalSurface {}

/// Owned copy of one daemon-bound input, so it can cross to the worker.
#[cfg(not(test))]
enum QueuedInput {
    Key(superplexr_terminal::KeyInput),
    Paste {
        bytes: Vec<u8>,
        confirmed: bool,
    },
    Focus(bool),
    Scroll(ViewportScroll),
    Select {
        anchor: SelectionPoint,
        head: SelectionPoint,
        rectangle: bool,
    },
    ClearSelection,
    Mouse(superplexr_terminal::MouseInput),
    Resize {
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
    },
}

/// One FIFO worker thread per live terminal. Input order is preserved, the
/// UI thread never waits on the control socket, and failures come back to
/// the surface as `last_error` transitions only.
#[cfg(not(test))]
struct InputQueue {
    send: std::sync::mpsc::Sender<QueuedInput>,
}

#[cfg(not(test))]
impl InputQueue {
    fn start(session: DaemonSession, cx: &mut Context<TerminalSurface>) -> Self {
        let (send, receive) = std::sync::mpsc::channel::<QueuedInput>();
        let (report, mut reports) = tokio::sync::mpsc::unbounded_channel::<Result<(), String>>();
        let spawned = std::thread::Builder::new()
            .name(format!("superplexr-input-{}", session.id()))
            .spawn(move || {
                let mut failing = false;
                while let Ok(input) = receive.recv() {
                    let result = match input {
                        QueuedInput::Key(input) => session.send_key(input),
                        QueuedInput::Paste { bytes, confirmed } => session.paste(bytes, confirmed),
                        QueuedInput::Focus(focused) => session.focus(focused),
                        QueuedInput::Scroll(scroll) => session.scroll(scroll),
                        QueuedInput::Select {
                            anchor,
                            head,
                            rectangle,
                        } => session.select(anchor, head, rectangle),
                        QueuedInput::ClearSelection => session.clear_selection(),
                        QueuedInput::Mouse(input) => session.mouse(input),
                        QueuedInput::Resize {
                            grid,
                            cell_width_px,
                            cell_height_px,
                        } => session.resize(grid, cell_width_px, cell_height_px),
                    };
                    // Only transitions reach the UI thread: an error, or the
                    // first success after one.
                    let transition = match result {
                        Ok(()) if failing => {
                            failing = false;
                            Some(Ok(()))
                        }
                        Ok(()) => None,
                        Err(error) => {
                            failing = true;
                            Some(Err(error.to_string()))
                        }
                    };
                    if let Some(transition) = transition
                        && report.send(transition).is_err()
                    {
                        break;
                    }
                }
            });
        if let Err(error) = spawned {
            eprintln!("terminal input worker failed to start: {error}");
        }
        cx.spawn(async move |this, cx| {
            while let Some(result) = reports.recv().await {
                let updated = this.update(cx, |surface, cx| {
                    match result {
                        Ok(()) => surface.last_error = None,
                        Err(error) => surface.last_error = Some(error),
                    }
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self { send }
    }

    fn send(&self, input: QueuedInput) -> bool {
        self.send.send(input).is_ok()
    }
}

#[cfg(not(test))]
fn ended_button(
    id: &'static str,
    label: &'static str,
    glyph: &'static str,
    token: crate::theme::ColorToken,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .role(gpui::accesskit::Role::Button)
        .aria_label(label)
        .size(px(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .cursor_pointer()
        .text_color(rgb(token))
        .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
        .on_click(on_click)
        .child(glyph)
        .into_any_element()
}

impl Focusable for TerminalSurface {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl gpui::Render for TerminalSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let background = terminal_color(self.frame.default_background);

        div()
            .id(self.element_id.clone())
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .flex_1()
            .min_h_0()
            .border_1()
            .border_color(rgb(if focused { RELAY } else { HAIRLINE }))
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .bg(background)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .px(px(10.0))
                            .py(px(6.0))
                            .bg(background)
                            .font_family(UI_FONT)
                            .text_xs()
                            .line_height(ui_size(18.0))
                            .child(TerminalElement::new(
                                self.grid_id.clone(),
                                self.frame.clone(),
                                cx.entity(),
                                self.composition.clone(),
                                self.selection,
                            )),
                    )
                    .children(self.scroll_indicator()),
            )
            .when_some(
                self.last_error.clone().filter(|_| self.ended.is_none()),
                |element, error| {
                    element.child(
                        div()
                            .h(px(24.0))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .bg(rgb(PANEL))
                            .border_t_1()
                            .border_color(rgb(HAIRLINE))
                            .font_family(UI_FONT)
                            .text_xs()
                            .text_color(rgb(SIGNAL))
                            .child("△")
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(error),
                            ),
                    )
                },
            )
            .when_some(self.ended, |element, success| {
                let detail = if self.last_error.is_some() && !success {
                    self.last_error.clone().unwrap_or_default()
                } else {
                    self.last_encoded.clone()
                };
                element.child(
                    div()
                        .h(px(26.0))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .bg(rgb(PANEL))
                        .border_t_1()
                        .border_color(rgb(HAIRLINE))
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(TRACE))
                        .child(
                            div()
                                .text_color(rgb(if success { TRACE } else { FAULT }))
                                .child(if success { "○" } else { "△" }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .child(detail),
                        )
                        .children({
                            #[cfg(not(test))]
                            {
                                Some(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(ended_button(
                                            "terminal-restart",
                                            "Restart shell",
                                            "↻",
                                            RELAY,
                                            cx.listener(|_, _, _, cx| {
                                                cx.emit(TerminalSurfaceEvent::RestartRequested);
                                            }),
                                        ))
                                        .child(ended_button(
                                            "terminal-close",
                                            "Close terminal",
                                            "×",
                                            TRACE,
                                            cx.listener(|_, _, _, cx| {
                                                cx.emit(TerminalSurfaceEvent::CloseRequested);
                                            }),
                                        ))
                                        .into_any_element(),
                                )
                            }
                            #[cfg(test)]
                            {
                                None::<AnyElement>
                            }
                        }),
                )
            })
            .when(!self.writable, |element| {
                element.child(
                    div()
                        .h(px(26.0))
                        .flex()
                        .items_center()
                        .px_2()
                        .bg(rgb(PANEL))
                        .border_t_1()
                        .border_color(rgb(HAIRLINE))
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(TRACE))
                        .child("Observer · input and resize locked")
                        .child(
                            div()
                                .id("request-terminal-control")
                                .ml_auto()
                                .h(px(20.0))
                                .px_2()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .rounded(px(4.0))
                                .bg(rgb(ACTIVE))
                                .text_color(rgb(RELAY))
                                .on_click(cx.listener(|surface, _, _, cx| {
                                    surface.request_control(cx);
                                }))
                                .child("Request control"),
                        ),
                )
            })
            .when(self.pending_paste.is_some(), |element| {
                element.child(
                    div()
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .px_2()
                        .bg(rgb(PANEL))
                        .border_t_1()
                        .border_color(rgb(HAIRLINE))
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(SIGNAL))
                        .child("multiline paste blocked")
                        .child(
                            div()
                                .ml_auto()
                                .text_color(rgb(CHALK))
                                .child("Shift+Paste to send"),
                        ),
                )
            })
    }
}

impl EntityInputHandler for TerminalSurface {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let byte_range = utf16_range_to_bytes(&self.composition, range)?;
        *adjusted_range = Some(bytes_range_to_utf16(&self.composition, &byte_range));
        Some(self.composition[byte_range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.composition.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        (!self.composition.is_empty()).then(|| 0..self.composition.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.composition.clear();
        cx.notify();
    }

    fn paste(&mut self, item: ClipboardItem, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text() {
            self.apply(
                TerminalAction::Paste {
                    bytes: text.as_bytes(),
                    confirmed: false,
                },
                cx,
            );
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition.clear();
        if !text.is_empty() {
            let input = KeyInput {
                physical_key: "unidentified".to_owned(),
                logical_key: text.to_owned(),
                text: Some(text.to_owned()),
                modifiers: KeyModifiers::default(),
                consumed_modifiers: KeyModifiers::default(),
                action: KeyAction::Press,
                composing: false,
                unshifted_codepoint: text.chars().next(),
            };
            self.apply(TerminalAction::EncodeKey(&input), cx);
        } else {
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composition.clear();
        self.composition.push_str(new_text);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let cursor = self.frame.cursor?;
        let cell_width = bounds.size.width / f32::from(self.frame.grid.columns);
        let cell_height = bounds.size.height / f32::from(self.frame.grid.rows);
        Some(Bounds::new(
            point(
                bounds.left() + cell_width * f32::from(cursor.column),
                bounds.top() + cell_height * f32::from(cursor.row),
            ),
            size(cell_width.max(px(1.0)), cell_height.max(px(1.0))),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.composition.encode_utf16().count())
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.composition.encode_utf16().count())
    }
}

fn key_input(keystroke: &gpui::Keystroke, action: KeyAction, caps_lock: bool) -> KeyInput {
    let text = keystroke.key_char.as_deref().filter(|text| {
        text.chars().all(|character| {
            !character.is_control() && !(('\u{f700}'..='\u{f8ff}').contains(&character))
        })
    });
    KeyInput {
        physical_key: keystroke.key.clone(),
        logical_key: keystroke
            .key_char
            .clone()
            .unwrap_or_else(|| keystroke.key.clone()),
        text: text.map(str::to_owned),
        modifiers: KeyModifiers {
            shift: keystroke.modifiers.shift,
            alt: keystroke.modifiers.alt,
            control: keystroke.modifiers.control,
            super_key: keystroke.modifiers.platform,
            caps_lock,
            num_lock: false,
        },
        consumed_modifiers: KeyModifiers::default(),
        action,
        composing: false,
        unshifted_codepoint: (keystroke.key.chars().count() == 1)
            .then(|| keystroke.key.chars().next())
            .flatten(),
    }
}

fn terminal_mouse_button(button: MouseButton) -> Option<TerminalMouseButton> {
    match button {
        MouseButton::Left => Some(TerminalMouseButton::Left),
        MouseButton::Middle => Some(TerminalMouseButton::Middle),
        MouseButton::Right => Some(TerminalMouseButton::Right),
        MouseButton::Navigate(_) => None,
    }
}

/// Keys the desktop owns, which must reach its key bindings rather than the
/// terminal.
///
/// No macOS terminal sends a ⌘ combination to the shell, and Ctrl+Tab
/// switches workspaces here. Forwarding them anyway meant the bindings never
/// fired, and in a terminal left in Kitty keyboard mode each press and
/// release became a sequence a plain shell typed out as text.
fn is_desktop_shortcut(keystroke: &gpui::Keystroke) -> bool {
    keystroke.modifiers.platform || (keystroke.modifiers.control && keystroke.key == "tab")
}

fn escaped_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| match byte {
            0x1b => "\\e".to_owned(),
            0x00..=0x1f | 0x7f => format!("\\x{byte:02x}"),
            _ if byte.is_ascii() => char::from(*byte).to_string(),
            _ => format!("\\x{byte:02x}"),
        })
        .collect()
}

fn utf16_range_to_bytes(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end || range.end > text.encode_utf16().count() {
        return None;
    }
    Some(utf16_offset_to_byte(text, range.start)?..utf16_offset_to_byte(text, range.end)?)
}

fn utf16_offset_to_byte(text: &str, offset: usize) -> Option<usize> {
    if offset == text.encode_utf16().count() {
        return Some(text.len());
    }
    text.char_indices()
        .scan(0, |utf16, (byte, character)| {
            let current = *utf16;
            *utf16 += character.len_utf16();
            Some((byte, current))
        })
        .find_map(|(byte, current)| (current == offset).then_some(byte))
}

fn bytes_range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    text[..range.start].encode_utf16().count()..text[..range.end].encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_element::TerminalElement;
    use gpui::{AnyWindowHandle, Keystroke, TestAppContext};

    #[test]
    fn adapter_preserves_key_identity_text_modifiers_and_lifecycle() {
        let keystroke = gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
            key: "a".to_owned(),
            key_char: Some("A".to_owned()),
        };

        let input = key_input(&keystroke, KeyAction::Repeat, true);

        assert_eq!(input.physical_key, "a");
        assert_eq!(input.logical_key, "A");
        assert_eq!(input.text.as_deref(), Some("A"));
        assert!(input.modifiers.control);
        assert!(input.modifiers.shift);
        assert!(input.modifiers.caps_lock);
        assert_eq!(input.action, KeyAction::Repeat);
        assert_eq!(input.unshifted_codepoint, Some('a'));
    }

    #[test]
    fn utf16_ranges_round_trip_for_composed_unicode() {
        let text = "a界😀z";
        let bytes = utf16_range_to_bytes(text, 1..4).expect("range should be valid");

        assert_eq!(&text[bytes.clone()], "界😀");
        assert_eq!(bytes_range_to_utf16(text, &bytes), 1..4);
        assert!(utf16_range_to_bytes(text, 3..4).is_none());
    }

    #[gpui::test]
    fn terminal_surface_paints_at_two_hundred_percent_scale(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| {
            TerminalSurface::new(window, cx).expect("terminal fixture should initialize")
        });

        window
            .update(cx, |_surface, window, _cx| {
                window.set_scale_factor(2.0);
            })
            .expect("test window should remain available");
        cx.update_window(AnyWindowHandle::from(window), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(window.scale_factor(), 2.0);
        })
        .expect("scaled test window should paint");
    }

    #[gpui::test]
    fn terminal_scrollbar_is_an_auto_hidden_overlay(cx: &mut TestAppContext) {
        let (surface, cx) = cx.add_window_view(|window, cx| {
            TerminalSurface::new(window, cx).expect("terminal fixture should initialize")
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("terminal-scrollbar").is_none());

        cx.update(|_, cx| {
            surface.update(cx, |surface, cx| {
                surface.scrollbar.reveal();
                cx.notify();
            });
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        assert!(cx.debug_bounds("terminal-scrollbar").is_some());
    }

    /// A key the desktop owns must never be encoded for the terminal, so the
    /// binding behind it can fire. Ctrl+Tab forwarded into a Kitty-mode shell
    /// is exactly how `9;5:3u` ended up typed in front of a command.
    #[gpui::test]
    fn desktop_shortcuts_are_not_forwarded_to_the_terminal(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| {
            TerminalSurface::new(window, cx).expect("terminal fixture should initialize")
        });
        cx.run_until_parked();
        window
            .update(cx, |surface, window, cx| {
                // Kitty keyboard mode, the state in which Ctrl+Tab and ⌘ keys
                // would otherwise be encoded as `CSI … u` sequences.
                surface.apply(TerminalAction::Output(b"\x1b[>3u"), cx);
                let untouched = surface.last_encoded.clone();
                for shortcut in [
                    "ctrl-tab",
                    "ctrl-shift-tab",
                    "cmd-t",
                    "cmd-1",
                    "cmd-shift-f",
                ] {
                    surface.key_down(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse(shortcut).expect("valid shortcut"),
                            is_held: false,
                            prefer_character_input: false,
                        },
                        window,
                        cx,
                    );
                    surface.key_up(
                        &gpui::KeyUpEvent {
                            keystroke: Keystroke::parse(shortcut).expect("valid shortcut"),
                        },
                        window,
                        cx,
                    );
                    assert_eq!(
                        surface.last_encoded, untouched,
                        "{shortcut} must not reach the terminal"
                    );
                }
                // An ordinary key still does. A platform-dispatched key
                // carries its character; a parsed one does not.
                let mut plain = Keystroke::parse("a").expect("valid key");
                plain.key_char = Some("a".to_owned());
                surface.key_down(
                    &KeyDownEvent {
                        keystroke: plain,
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(surface.last_encoded, "a", "a plain key is still encoded");
            })
            .expect("test window should remain available");
    }

    /// Selecting and copying must work on a terminal this surface cannot
    /// control: one an agent spawned, one being observed, or one that has
    /// finished. It used to be a daemon request that needed the controller,
    /// so on exactly those terminals a drag did nothing at all.
    #[gpui::test]
    fn selection_and_copy_work_without_controlling_the_terminal(cx: &mut TestAppContext) {
        use gpui::{
            Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, point, px,
        };

        let window = cx.add_window(|window, cx| {
            TerminalSurface::new(window, cx).expect("terminal fixture should initialize")
        });
        let any_window = AnyWindowHandle::from(window);
        cx.run_until_parked();
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("test window should draw so the grid geometry is known");

        // Expected text comes from the frame itself, so this checks the
        // pixel-to-cell mapping rather than assuming the fixture's content.
        let expected = window
            .update(cx, |surface, _, _| {
                let first = surface.frame.rows[0]
                    .text()
                    .chars()
                    .skip(6)
                    .collect::<String>();
                let second = surface.frame.rows[1]
                    .text()
                    .chars()
                    .take(6)
                    .collect::<String>();
                format!("{}\n{second}", first.trim_end())
            })
            .expect("test window should remain available");
        assert!(
            expected.len() > 2,
            "fixture rows should hold text: {expected:?}"
        );

        window
            .update(cx, |surface, window, cx| {
                // The case that was broken: not the controller.
                surface.writable = false;
                let geometry = surface.geometry.expect("drawn once, so geometry is known");
                let at = |row: f32, column: f32| {
                    point(
                        geometry.bounds.left() + geometry.cell_width * column + px(1.0),
                        geometry.bounds.top() + geometry.line_height * row + px(1.0),
                    )
                };
                surface.mouse_down(
                    &MouseDownEvent {
                        button: MouseButton::Left,
                        position: at(0.0, 6.0),
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    },
                    window,
                    cx,
                );
                surface.mouse_move(
                    &MouseMoveEvent {
                        position: at(1.0, 5.0),
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    },
                    window,
                    cx,
                );
                surface.mouse_up(
                    &MouseUpEvent {
                        button: MouseButton::Left,
                        position: at(1.0, 5.0),
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    },
                    window,
                    cx,
                );
                assert_eq!(surface.selected_text().as_deref(), Some(expected.as_str()));
                assert!(
                    surface.last_error.is_none(),
                    "selecting must never trip the observer-mode refusal"
                );
                surface.copy_visible(cx);
                assert_eq!(surface.last_encoded, "selection copied to clipboard");

                // A plain click clears it.
                surface.mouse_down(
                    &MouseDownEvent {
                        button: MouseButton::Left,
                        position: at(0.0, 0.0),
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    },
                    window,
                    cx,
                );
                surface.mouse_up(
                    &MouseUpEvent {
                        button: MouseButton::Left,
                        position: at(0.0, 0.0),
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    },
                    window,
                    cx,
                );
                assert!(
                    surface.selection.is_none(),
                    "a click without a drag clears the selection"
                );
            })
            .expect("test window should remain available");
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(expected.as_str()));
        // The highlight must survive a real layout pass.
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("test window should draw with a selection");
    }

    #[gpui::test]
    fn terminal_surface_handlers_route_input_and_clipboard(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| {
            TerminalSurface::new(window, cx).expect("terminal fixture should initialize")
        });
        let any_window = AnyWindowHandle::from(window);
        cx.run_until_parked();
        window
            .update(cx, |_, window, _| window.activate_window())
            .expect("test window should activate");
        cx.run_until_parked();

        window
            .update(cx, |surface, _, cx| {
                surface.apply(TerminalAction::Output(b"\x1b[?1004h\x1b[>3u"), cx);
            })
            .expect("test window should remain available");
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("test window should draw before focusing");
        window
            .update(cx, |surface, window, cx| {
                window.focus(&surface.focus_handle_owned(), cx);
            })
            .expect("test window should accept focus");
        cx.run_until_parked();
        cx.update_window(any_window, |_, window, cx| {
            window.draw(cx).clear(cx);
        })
        .expect("focused test window should draw");
        window
            .update(cx, |surface, window, _| {
                assert!(surface.focus_handle.is_focused(window));
                assert_eq!(surface.last_encoded, "\\e[I");
            })
            .expect("terminal focus should be observable");
        cx.simulate_keystrokes(any_window, "a");
        window
            .update(cx, |surface, _, _| {
                assert_eq!(surface.last_encoded, "a");
            })
            .expect("a platform-dispatched key press should reach the focused terminal surface");

        window
            .update(cx, |surface, window, cx| {
                surface.key_up(
                    &KeyUpEvent {
                        keystroke: Keystroke::parse("a").expect("valid key fixture"),
                    },
                    window,
                    cx,
                );
            })
            .expect("key release should dispatch");
        window
            .update(cx, |surface, _, _| {
                assert!(surface.last_encoded.contains('u'));
                assert_ne!(surface.last_encoded, "a");
            })
            .expect("Kitty key release should reach the terminal surface");

        window
            .update(cx, |surface, window, cx| {
                surface.key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("cmd-c").expect("valid copy shortcut"),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .expect("copy shortcut should dispatch");
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .expect("copy shortcut should populate the clipboard");
        assert!(copied.contains("ghostty  terminal state pinned"));

        cx.write_to_clipboard(ClipboardItem::new_string("safe paste".to_owned()));
        window
            .update(cx, |surface, window, cx| {
                surface.key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("cmd-v").expect("valid paste shortcut"),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .expect("paste shortcut should dispatch");
        window
            .update(cx, |surface, _, _| {
                assert_eq!(surface.last_encoded, "safe paste");
                assert!(surface.pending_paste.is_none());
            })
            .expect("safe paste should reach the terminal surface");

        let unsafe_paste = "first line\nsecond line";
        cx.write_to_clipboard(ClipboardItem::new_string(unsafe_paste.to_owned()));
        window
            .update(cx, |surface, window, cx| {
                surface.key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("cmd-v").expect("valid paste shortcut"),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .expect("unsafe paste shortcut should dispatch");
        window
            .update(cx, |surface, _, _| {
                assert!(surface.pending_paste.is_some());
                assert_eq!(surface.last_encoded, "paste blocked pending confirmation");
            })
            .expect("unsafe paste should stop for confirmation");
        window
            .update(cx, |surface, window, cx| {
                surface.key_down(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("cmd-shift-v")
                            .expect("valid confirmation shortcut"),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
            })
            .expect("paste confirmation shortcut should dispatch");
        window
            .update(cx, |surface, _, _| {
                assert_eq!(surface.last_encoded, "first line\\x0dsecond line");
                assert!(surface.pending_paste.is_none());
            })
            .expect("confirmed paste should reach the terminal surface");

        window
            .update(cx, |surface, window, cx| {
                surface.replace_and_mark_text_in_range(None, "日本", None, window, cx);
                assert_eq!(surface.composition, "日本");
                surface.replace_text_in_range(None, "日本語", window, cx);
                assert!(surface.composition.is_empty());
                assert_eq!(surface.last_encoded, escaped_bytes("日本語".as_bytes()));
                window.disable_focus(cx);
            })
            .expect("IME commit and focus loss should dispatch");
        cx.run_until_parked();
        window
            .update(cx, |surface, _, _| {
                assert_eq!(surface.last_encoded, "\\e[O");
            })
            .expect("focus-loss report should be observable");
    }

    #[gpui::test]
    fn terminal_element_exposes_one_meaningful_accessibility_node(cx: &mut TestAppContext) {
        let window = cx.add_window(|window, cx| {
            TerminalSurface::new(window, cx).expect("terminal fixture should initialize")
        });
        let surface = window
            .entity(cx)
            .expect("test window should expose its root entity");
        let frame = window
            .read_with(cx, |surface, _| surface.frame.clone())
            .expect("test window should expose its terminal frame");
        let element = TerminalElement::new("a11y-grid", frame, surface, String::new(), None);
        let mut node = gpui::accesskit::Node::new(gpui::accesskit::Role::Terminal);

        assert_eq!(element.a11y_role(), Some(gpui::accesskit::Role::Terminal));
        element.write_a11y_info(&mut node);
        assert_eq!(node.label(), Some("Terminal output, 72 columns by 12 rows"));
        assert!(
            node.value()
                .is_some_and(|value| value.contains("ghostty  terminal state pinned"))
        );
    }
}
