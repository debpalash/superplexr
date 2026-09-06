//! Bounded native-event to GPUI boundary. Apply every ordered delta, but retain
//! only the latest canonical frame and one slot per kind of UI notice. A unit
//! wakeup, not a frame/event FIFO, crosses onto the render executor.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::mpsc;
use ultraplexr_client::TerminalStreamUpdate;
use ultraplexr_core::SessionId;
use ultraplexr_protocol::ServerEvent;
use ultraplexr_terminal::FullFrame;

#[derive(Default)]
pub(crate) struct Batch {
    pub frame: Option<Arc<FullFrame>>,
    pub continuity_lost: bool,
    pub rejected: bool,
    pub error: Option<&'static str>,
    // Bell, paste confirmation, escalation, outcome. Never an ordered event log.
    pub notices: [Option<ServerEvent>; 4],
    pub validity: Option<(Arc<AtomicU64>, u64)>,
}
impl Batch {
    pub(crate) fn is_current(&self) -> bool {
        self.validity
            .as_ref()
            .is_none_or(|(epoch, expected)| epoch.load(Ordering::Acquire) == *expected)
    }
}
#[derive(Default)]
struct State {
    frame: Option<Arc<FullFrame>>,
    frame_dirty: bool,
    pending: Batch,
    ended: bool,
    continuity: Arc<AtomicU64>,
}
pub(crate) struct Sender {
    session: SessionId,
    state: Arc<Mutex<State>>,
    wake: mpsc::Sender<()>,
}
pub(crate) struct Receiver {
    state: Arc<Mutex<State>>,
    wake: mpsc::Receiver<()>,
}

pub(crate) fn channel(session: SessionId) -> (Sender, Receiver) {
    let state = Arc::new(Mutex::new(State::default()));
    let (send, receive) = mpsc::channel(1);
    (
        Sender {
            session,
            state: state.clone(),
            wake: send,
        },
        Receiver {
            state,
            wake: receive,
        },
    )
}

impl State {
    fn invalidate(&mut self, rejected: bool, error: Option<&'static str>) {
        self.continuity.fetch_add(1, Ordering::AcqRel);
        self.frame = None;
        self.frame_dirty = false;
        self.pending = Batch {
            continuity_lost: true,
            rejected,
            error,
            ..Batch::default()
        };
        self.ended = rejected;
    }
    fn accept(&mut self, session: SessionId, update: TerminalStreamUpdate) {
        match update {
            TerminalStreamUpdate::Reconnecting => self.invalidate(false, None),
            TerminalStreamUpdate::Rejected => self.invalidate(true, None),
            TerminalStreamUpdate::ReceiveLimited => {
                self.invalidate(true, Some(ultraplexr_client::RECEIVE_LIMIT_MESSAGE))
            }
            TerminalStreamUpdate::Event(event) => {
                let id = match &event {
                    ServerEvent::TerminalFrame { session_id, .. }
                    | ServerEvent::TerminalDelta { session_id, .. }
                    | ServerEvent::TerminalBell { session_id, .. }
                    | ServerEvent::PasteConfirmation { session_id, .. }
                    | ServerEvent::TerminalTerminationEscalationRequired { session_id }
                    | ServerEvent::TerminalExited { session_id, .. }
                    | ServerEvent::TerminalFailed { session_id, .. } => *session_id,
                };
                if id != session {
                    self.invalidate(true, Some("Terminal identity changed; reattach"));
                    return;
                }
                match event {
                    ServerEvent::TerminalFrame { frame, .. } => {
                        if self
                            .frame
                            .as_ref()
                            .is_some_and(|current| current.sequence > frame.sequence)
                        {
                            self.invalidate(true, Some("Terminal frame went backwards; reattach"));
                            return;
                        }
                        self.frame = Some(Arc::from(frame));
                        self.frame_dirty = true;
                    }
                    ServerEvent::TerminalDelta { delta, .. } => {
                        if !self
                            .frame
                            .as_mut()
                            .is_some_and(|frame| delta.apply_to(Arc::make_mut(frame)).is_ok())
                        {
                            self.invalidate(true, Some("Terminal delta lost its base; reattach"));
                            return;
                        }
                        self.frame_dirty = true;
                    }
                    ServerEvent::TerminalBell { count, .. } => {
                        let previous = match self.pending.notices[0].take() {
                            Some(ServerEvent::TerminalBell { count, .. }) => count,
                            _ => 0,
                        };
                        self.pending.notices[0] = Some(ServerEvent::TerminalBell {
                            session_id: session,
                            count: previous.saturating_add(count),
                        });
                    }
                    event @ ServerEvent::PasteConfirmation { .. } => {
                        self.pending.notices[1] = Some(event)
                    }
                    event @ ServerEvent::TerminalTerminationEscalationRequired { .. } => {
                        self.pending.notices[2] = Some(event)
                    }
                    event @ (ServerEvent::TerminalExited { .. }
                    | ServerEvent::TerminalFailed { .. }) => {
                        self.pending.notices[1] = None;
                        self.pending.notices[2] = None;
                        self.pending.notices[3] = Some(event);
                        self.ended = true;
                    }
                }
            }
        }
    }
}
impl Sender {
    /// Runs on the existing native subscription thread, never blocks for GPUI.
    /// False retires this subscription after the final notice has been queued.
    pub(crate) fn publish(&self, update: TerminalStreamUpdate) -> bool {
        if self.wake.is_closed() {
            return false;
        }
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.ended {
            return false;
        }
        state.accept(self.session, update);
        let continuing = !state.ended;
        drop(state);
        match self.wake.try_send(()) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => continuing,
            Err(mpsc::error::TrySendError::Closed(())) => false,
        }
    }
}
impl Receiver {
    pub(crate) async fn recv(&mut self) -> Option<Batch> {
        self.wake.recv().await?;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let mut batch = std::mem::take(&mut state.pending);
        batch.validity = Some((
            state.continuity.clone(),
            state.continuity.load(Ordering::Acquire),
        ));
        if state.frame_dirty {
            batch.frame = state.frame.clone();
            state.frame_dirty = false;
        }
        Some(batch)
    }
}

#[cfg(test)]
#[path = "terminal_feed_tests.rs"]
mod tests;
