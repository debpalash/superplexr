use std::time::Duration;
#[cfg(not(test))]
use std::time::Instant;
use std::{ops::Range, sync::Arc};

#[cfg(not(test))]
use crate::terminal_input::Queue as InputQueue;
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
use superplexr_client::{TerminalInput as QueuedInput, TerminalStreamUpdate};
use superplexr_protocol::{ServerEvent, TerminalSessionStatus};
#[cfg(test)]
use superplexr_terminal::TerminalError;
use superplexr_terminal::{
    FullFrame, GridSize, HistoryViewport, KeyAction, KeyInput, KeyModifiers,
    MouseAction as TerminalMouseAction, MouseButton as TerminalMouseButton, MouseInput,
    PasteConfirmation, SelectionPoint, TerminalAction, TerminalEffects, TerminalModel,
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
#[path = "terminal_surface_search_tests.rs"]
mod search_tests;

#[cfg(test)]
const FIXTURE_OUTPUT: &[u8] = b"\x1b[2J\x1b[H$ superplexr verify --platform native\r\n\x1b[1;34mghostty\x1b[0m  terminal state pinned\r\n\x1b[1;32minput\x1b[0m    keyboard, IME, focus and paste connected\r\n\x1b[1;33mnext\x1b[0m     connect a real PTY";

/// GPUI-owned interactive surface around the backend-neutral terminal module.
pub(crate) struct TerminalSurface {
    element_id: SharedString,
    grid_id: SharedString,
    model: Option<TerminalModel>,
    session: Option<DaemonSession>,
    frame: Arc<FullFrame>,
    /// Latest live frame while a read-only search viewport is displayed.
    search_live_frame: Option<Arc<FullFrame>>,
    #[cfg(not(test))]
    catalog_labels: Option<CatalogLabels>,
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
    presented: bool,
    event_generation: u64,
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
struct CatalogLabels {
    sequence: u64,
    title: Option<String>,
    directory: Option<String>,
}

#[cfg(not(test))]
#[derive(Clone, Debug)]
pub(crate) enum TerminalSurfaceEvent {
    SearchInvalidated,
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
    /// Reconciles the daemon's durable process state into this rendered surface.
    /// Restored sessions do not replay their exit event, so metadata is the
    /// authoritative source after an app launch or reconnect.
    pub(crate) fn synchronize_process_status(
        &mut self,
        status: TerminalSessionStatus,
        cx: &mut Context<Self>,
    ) {
        let success = match status {
            TerminalSessionStatus::Running => return,
            TerminalSessionStatus::Exited => true,
            TerminalSessionStatus::Failed => false,
        };
        self.writable = false;
        self.historical = true;
        self.pending_paste = None;
        self.composition.clear();
        self.ended = Some(success);
        self.last_error = None;
        self.last_encoded = if success {
            "completed · terminal history retained".to_owned()
        } else {
            "failed · terminal history retained".to_owned()
        };
        cx.notify();
    }

    #[cfg(not(test))]
    pub(crate) fn synchronize_catalog_labels(
        &mut self,
        summary: &superplexr_protocol::TerminalSessionSummary,
    ) {
        let labels = self.catalog_labels.get_or_insert(CatalogLabels {
            sequence: summary.latest_sequence,
            title: None,
            directory: None,
        });
        labels.sequence = summary.latest_sequence;
        labels.title.clone_from(&summary.display_title);
        labels.directory.clone_from(&summary.display_directory);
    }

    /// Presentation only. Never use catalog directories for shell launch or
    /// filesystem access: they are sanitized and may have been truncated.
    #[cfg(not(test))]
    pub(crate) fn session_display_metadata(&self) -> (Option<&str>, Option<&str>) {
        let frame = self.search_live_frame.as_ref().unwrap_or(&self.frame);
        if let Some(labels) = &self.catalog_labels
            && labels.sequence > frame.sequence
        {
            (labels.title.as_deref(), labels.directory.as_deref())
        } else {
            (frame.title.as_deref(), frame.current_directory.as_deref())
        }
    }

    pub(crate) fn terminal_title(&self) -> Option<&str> {
        self.search_live_frame
            .as_ref()
            .unwrap_or(&self.frame)
            .title
            .as_deref()
    }

    /// True once the process exited or the session failed; input is refused.
    pub(crate) fn has_ended(&self) -> bool {
        self.ended.is_some()
    }

    #[cfg(not(test))]
    pub(crate) fn visible_text(&self) -> String {
        self.frame
            .rows
            .iter()
            .map(|row| row.text())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_owned()
    }

    #[cfg(not(test))]
    pub(crate) fn current_directory(&self) -> Option<&str> {
        self.search_live_frame
            .as_ref()
            .unwrap_or(&self.frame)
            .current_directory
            .as_deref()
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
            search_live_frame: None,
            #[cfg(not(test))]
            catalog_labels: None,
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
            presented: false,
            event_generation: 0,
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
        let input_queue = if historical {
            None
        } else {
            Some(Self::start_input_queue(session.id(), cx)?)
        };
        let writable = input_queue.as_ref().is_some_and(|queue| {
            session
                .claim_input(false)
                .ok()
                .is_some_and(|lease| queue.arm(lease).is_ok())
        });
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
        Ok(Self {
            element_id: id.into(),
            grid_id: format!("{id}-grid").into(),
            model: None,
            session: Some(session),
            frame: Arc::new(frame),
            search_live_frame: None,
            #[cfg(not(test))]
            catalog_labels: None,
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
            event_generation: 0,
            last_activity_emitted: None,
            event_subscription: None,
            input_queue,
            _subscriptions: vec![focus_in, focus_out],
        })
    }

    #[cfg(not(test))]
    pub(crate) fn set_presented(&mut self, presented: bool, cx: &mut Context<Self>) {
        if self.presented == presented {
            return;
        }
        self.presented = presented;
        // A cancelled subscription may already have scheduled a GPUI update.
        // Hiding and reattaching must not let that old task paint this Surface.
        self.event_generation = self.event_generation.wrapping_add(1);
        if presented {
            if let Err(error) = self.start_event_subscription(cx) {
                self.last_error = Some(format!("terminal attach failed: {error}"));
            }
            cx.notify();
        } else {
            self.retire_input("Surface hidden; request Control to resume input", cx);
            if let Some(subscription) = self.event_subscription.take() {
                subscription.cancel();
            }
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
        let generation = self.event_generation;
        let (event_send, mut event_receive) = crate::terminal_feed::channel(session.id());
        let input_gate = self.input_queue.as_ref().map(InputQueue::gate);
        let subscription = session.subscribe_events_with_status(move |event| {
            if matches!(
                &event,
                TerminalStreamUpdate::Reconnecting
                    | TerminalStreamUpdate::Rejected
                    | TerminalStreamUpdate::ReceiveLimited
                    | TerminalStreamUpdate::Event(
                        ServerEvent::TerminalExited { .. } | ServerEvent::TerminalFailed { .. }
                    )
            ) && let Some(gate) = &input_gate
            {
                gate.retire("Terminal connection or authority changed; request Control again");
            }
            event_send.publish(event)
        })?;
        cx.spawn(async move |this, cx| {
            while let Some(batch) = event_receive.recv().await {
                if this
                    .update(cx, |surface, cx| {
                        surface.accept_feed_batch(generation, batch, cx)
                    })
                    .ok()
                    != Some(true)
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

    pub(crate) fn daemon_session(&self) -> Option<DaemonSession> {
        self.session.clone()
    }

    pub(crate) fn show_search_history(&mut self, frame: Arc<FullFrame>, cx: &mut Context<Self>) {
        self.retire_input(
            "Read-only history; request Control after returning to live",
            cx,
        );
        if self.search_live_frame.is_none() {
            self.search_live_frame = Some(self.frame.clone());
        }
        self.frame = frame;
        Arc::make_mut(&mut self.frame).cursor = None;
        Arc::make_mut(&mut self.frame).mouse_tracking = false;
        self.selection = None;
        self.pending_paste = None;
        self.composition.clear();
        cx.notify();
    }

    fn close_search_history(&mut self, cx: &mut Context<Self>) {
        if let Some(frame) = self.search_live_frame.take() {
            self.frame = frame;
            self.selection = None;
            cx.notify();
        }
    }

    fn invalidate_search_history(&mut self, rejected: bool, cx: &mut Context<Self>) {
        self.close_search_history(cx);
        self.retire_input("Terminal connection changed; request Control again", cx);
        self.pending_paste = None;
        self.composition.clear();
        self.selection = None;
        if rejected {
            let frame = Arc::make_mut(&mut self.frame);
            frame.rows.clear();
            frame.cursor = None;
        }
        self.last_error = Some(
            if rejected {
                "Session access ended"
            } else {
                "Reconnecting; search cancelled"
            }
            .into(),
        );
        cx.notify();
    }

    #[cfg(not(test))]
    pub(crate) fn mark_unavailable(&mut self, cx: &mut Context<Self>) {
        self.catalog_labels = None;
        self.set_presented(false, cx);
        self.invalidate_search_history(true, cx);
        self.last_error = Some("Session is no longer in the available collection".into());
        cx.emit(TerminalSurfaceEvent::SearchInvalidated);
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
        #[cfg(not(test))]
        let result = session.claim_input(false).and_then(|lease| {
            self.input_queue
                .as_ref()
                .ok_or_else(|| {
                    superplexr_client::ClientError::Io(std::io::Error::other(
                        "input worker unavailable",
                    ))
                })?
                .arm(lease)
                .map_err(|message| {
                    superplexr_client::ClientError::Io(std::io::Error::other(message))
                })
        });
        #[cfg(test)]
        let result = session.claim_control(false);
        match result {
            Ok(()) => {
                self.writable = true;
                self.last_error = None;
                self.last_encoded = "terminal control acquired".to_owned();
            }
            Err(error) => self.retire_input(&format!("control unavailable: {error}"), cx),
        }
        cx.notify();
    }

    #[cfg(not(test))]
    pub(crate) fn synchronize_control_owner(
        &mut self,
        controller_surface_id: Option<uuid::Uuid>,
        control_epoch: u64,
        cx: &mut Context<Self>,
    ) {
        // Metadata can retire input, but only an explicit claim can re-arm it.
        self.writable = self.search_live_frame.is_none()
            && self.input_queue.as_ref().is_some_and(|queue| {
                queue.synchronize_control_owner(controller_surface_id, control_epoch)
            });
        cx.notify();
    }

    pub(crate) fn resize_to(
        &mut self,
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
        cx: &mut Context<Self>,
    ) {
        if self.search_live_frame.is_some() || grid == self.requested_grid {
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
        if self.search_live_frame.is_some() && event.keystroke.key == "escape" {
            self.close_search_history(cx);
            cx.stop_propagation();
            return;
        }
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

        if self.search_live_frame.is_some() {
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
        if self.search_live_frame.is_some() {
            cx.stop_propagation();
            return;
        }
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
        if self.search_live_frame.is_some() {
            return false;
        }
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
                // Reject before cloning potentially large clipboard payloads.
                if !self.input_queue.as_ref().is_some_and(InputQueue::is_armed) {
                    self.retire_input("Input retired; request Control again", cx);
                    return false;
                }
                if let TerminalAction::Paste { bytes, .. } = &action
                    && bytes.len() > crate::terminal_input::MAX_PAYLOAD_BYTES
                {
                    self.retire_input("Paste exceeds the 8 MiB input limit", cx);
                    return false;
                }
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
                let result = self
                    .input_queue
                    .as_ref()
                    .ok_or("terminal input worker unavailable")
                    .and_then(|queue| queue.send(queued));
                if let Err(error) = result {
                    self.retire_input(error, cx);
                }
                return result.is_ok();
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

    fn accept_feed_batch(
        &mut self,
        generation: u64,
        batch: crate::terminal_feed::Batch,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.presented || self.event_generation != generation {
            return false;
        }
        // A discontinuity may arrive while this batch waits on the UI executor.
        // Skip stale pixels, but keep receiving the pending invalidation notice.
        if !batch.is_current() {
            return true;
        }
        if batch.continuity_lost {
            self.invalidate_search_history(batch.rejected, cx);
            if let Some(error) = batch.error {
                self.last_error = Some(error.into());
            }
            #[cfg(not(test))]
            cx.emit(TerminalSurfaceEvent::SearchInvalidated);
        }
        if let Some(frame) = batch.frame {
            *self.search_live_frame.as_mut().unwrap_or(&mut self.frame) = frame;
            #[cfg(not(test))]
            self.emit_activity_heartbeat(cx);
        }
        for event in batch.notices.into_iter().flatten() {
            self.accept_runtime_notice(event, cx);
        }
        cx.notify();
        true
    }

    fn accept_runtime_notice(&mut self, event: ServerEvent, cx: &mut Context<Self>) {
        match event {
            ServerEvent::TerminalFrame { .. } | ServerEvent::TerminalDelta { .. } => {}
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
                self.retire_input("Terminal exited; pending input discarded", cx);
                self.force_kill_available = false;
                self.writable = false;
                self.pending_paste = None;
                self.composition.clear();
                self.historical = true;
                self.history_viewport = HistoryViewport::RowsBeforeBottom(0);
                self.last_encoded = if success {
                    format!("exited ({code})")
                } else {
                    format!("failed ({code})")
                };
                self.ended = Some(success);
                self.last_error = None;
                #[cfg(not(test))]
                cx.emit(TerminalSurfaceEvent::Exited { success });
            }
            ServerEvent::TerminalFailed { message, .. } => {
                self.retire_input("Terminal failed; pending input discarded", cx);
                self.force_kill_available = false;
                self.writable = false;
                self.pending_paste = None;
                self.composition.clear();
                self.historical = true;
                self.history_viewport = HistoryViewport::RowsBeforeBottom(0);
                self.last_encoded = "failed".to_owned();
                self.last_error = Some(message);
                self.ended = Some(false);
                #[cfg(not(test))]
                cx.emit(TerminalSurfaceEvent::Failed);
            }
        }
        cx.notify();
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

impl TerminalSurface {
    fn retire_input(&mut self, message: &str, cx: &mut Context<Self>) {
        #[cfg(not(test))]
        if let Some(queue) = &self.input_queue {
            queue.gate().retire(message);
        }
        self.writable = false;
        self.pending_paste = None;
        self.composition.clear();
        self.last_error = Some(message.to_owned());
        cx.notify();
    }

    #[cfg(not(test))]
    fn start_input_queue(
        session: superplexr_core::SessionId,
        cx: &mut Context<Self>,
    ) -> Result<InputQueue, superplexr_client::ClientError> {
        let (queue, mut reports) = InputQueue::new(session)?;
        cx.spawn(async move |this, cx| {
            while reports.changed().await.is_ok() {
                let failure = { reports.borrow_and_update().clone() };
                let Some(failure) = failure else {
                    continue;
                };
                let updated = this.update(cx, |surface, cx| {
                    if surface
                        .input_queue
                        .as_ref()
                        .is_some_and(|queue| queue.current_failure(&failure))
                    {
                        surface.writable = false;
                        surface.pending_paste = None;
                        surface.composition.clear();
                        if !surface.has_ended() {
                            surface.last_error = Some(failure.message);
                        }
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        })
        .detach();
        Ok(queue)
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
        .size(ui_size(20.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(ui_size(3.0))
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
            .when(self.search_live_frame.is_some(), |element| {
                element.child(
                    div()
                        .id("close-search-history")
                        .debug_selector(|| "close-search-history".to_owned())
                        .role(gpui::accesskit::Role::Button)
                        .aria_label("Close read-only search history")
                        .px_2()
                        .py_1()
                        .bg(rgb(PANEL))
                        .text_color(rgb(RELAY))
                        .text_xs()
                        .cursor_pointer()
                        .child("Search history · read-only · Esc or click to return")
                        .on_click(
                            cx.listener(|surface, _, _, cx| surface.close_search_history(cx)),
                        ),
                )
            })
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
                            .px(ui_size(10.0))
                            .py(ui_size(6.0))
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
                            .h(ui_size(24.0))
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
                        .h(ui_size(26.0))
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
            .when(!self.writable && self.ended.is_none(), |element| {
                element.child(
                    div()
                        .h(ui_size(26.0))
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
                                .debug_selector(|| "request-terminal-control".to_owned())
                                .ml_auto()
                                .h(ui_size(20.0))
                                .px_2()
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .rounded(ui_size(4.0))
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
                        .h(ui_size(24.0))
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
    use crate::selection::Selection;
    use crate::terminal_element::TerminalElement;
    use gpui::{AnyWindowHandle, Keystroke, TestAppContext};

    #[test]
    fn surface_selection_formats_forward_reverse_and_rectangular_marks() {
        let mut model = TerminalModel::new(
            GridSize::new(8, 3).expect("selection fixture grid should be valid"),
        )
        .expect("selection fixture terminal should initialize");
        model
            .advance(TerminalAction::Output(b"alpha\r\nbravo\r\ncharlie"))
            .expect("selection fixture output should parse");
        let frame = model.frame().expect("selection fixture should render");

        let forward = Selection {
            anchor: SelectionPoint { column: 1, row: 0 },
            head: SelectionPoint { column: 2, row: 1 },
            rectangle: false,
        };
        let reverse = Selection {
            anchor: forward.head,
            head: forward.anchor,
            rectangle: false,
        };
        assert_eq!(forward.text(&frame.rows, frame.grid.columns), "lpha\nbra");
        assert_eq!(
            reverse.text(&frame.rows, frame.grid.columns),
            forward.text(&frame.rows, frame.grid.columns)
        );

        let rectangle = Selection {
            anchor: SelectionPoint { column: 1, row: 0 },
            head: SelectionPoint { column: 3, row: 2 },
            rectangle: true,
        };
        assert_eq!(
            rectangle.text(&frame.rows, frame.grid.columns),
            "lph\nrav\nhar"
        );
    }

    #[test]
    fn a_click_without_a_drag_does_not_create_copyable_text() {
        let point = SelectionPoint { column: 2, row: 0 };
        let click = Selection {
            anchor: point,
            head: point,
            rectangle: false,
        };
        assert!(click.is_empty());
    }

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

    #[gpui::test]
    fn restored_exited_terminal_is_a_finished_surface(cx: &mut TestAppContext) {
        let (surface, cx) = cx.add_window_view(|window, cx| {
            TerminalSurface::new(window, cx).expect("terminal fixture should initialize")
        });
        cx.update(|_, cx| {
            surface.update(cx, |surface, cx| {
                surface.writable = false;
                surface.historical = true;
                surface.synchronize_process_status(
                    superplexr_protocol::TerminalSessionStatus::Exited,
                    cx,
                );
                surface.last_encoded = "retained history".to_owned();
                cx.notify();
            });
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(surface.read_with(cx, |surface, _| surface.has_ended()));
        assert!(
            cx.debug_bounds("request-terminal-control").is_none(),
            "an exited terminal cannot grant control and must not offer the action"
        );
    }
}
