//! One ordered worker per Surface. Queue limits include the in-flight command;
//! retirement is local, drops waiting commands and never waits on socket I/O.
use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Condvar, Mutex},
};
use tokio::sync::watch;
use ultraplexr_client::{TerminalInput, TerminalInputLease};
use ultraplexr_core::SessionId;

pub(crate) const MAX_COMMANDS: usize = 128;
// Preserve the terminal's existing maximum individual paste size.
pub(crate) const MAX_PAYLOAD_BYTES: usize = ultraplexr_terminal::MAX_PASTE_BYTES;

#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub generation: u64,
    pub message: String,
}
#[derive(Default)]
struct State {
    generation: u64,
    lease: Option<TerminalInputLease>,
    commands: VecDeque<TerminalInput>,
    queued_bytes: usize,
    in_flight: Option<usize>,
    closed: bool,
}
struct Shared {
    session: SessionId,
    state: Mutex<State>,
    changed: Condvar,
    report: watch::Sender<Option<Failure>>,
}
impl Shared {
    fn retire(&self, state: &mut State, message: &str) {
        state.generation = state.generation.wrapping_add(1);
        if let Some(lease) = state.lease.take() {
            lease.retire();
        }
        state.commands.clear();
        state.queued_bytes = 0;
        self.report.send_replace(Some(Failure {
            generation: state.generation,
            message: message.chars().take(1024).collect(),
        }));
        self.changed.notify_all();
    }
}
/// Non-owning retirement handle used by the native event callback.
#[derive(Clone)]
pub(crate) struct Gate(Arc<Shared>);
impl Gate {
    pub(crate) fn retire(&self, message: &str) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.0.retire(&mut state, message);
    }
}
pub(crate) struct Queue {
    shared: Arc<Shared>,
}
impl Queue {
    pub(crate) fn new(session: SessionId) -> io::Result<(Self, watch::Receiver<Option<Failure>>)> {
        let (report, receive) = watch::channel(None);
        let shared = Arc::new(Shared {
            session,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            report,
        });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name(format!("ultraplexr-input-{session}"))
            .spawn(move || work(worker))?;
        Ok((Self { shared }, receive))
    }
    pub(crate) fn gate(&self) -> Gate {
        Gate(self.shared.clone())
    }
    pub(crate) fn arm(&self, lease: TerminalInputLease) -> Result<(), &'static str> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.shared
            .retire(&mut state, "Control replaced; old input discarded");
        if state.closed || lease.session_id() != self.shared.session || !lease.is_current() {
            lease.retire();
            return Err("Input authority is unavailable; request Control again");
        }
        state.lease = Some(lease);
        self.shared.report.send_replace(None);
        self.shared.changed.notify_all();
        Ok(())
    }
    pub(crate) fn is_armed(&self) -> bool {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        !state.closed
            && state
                .lease
                .as_ref()
                .is_some_and(TerminalInputLease::is_current)
    }
    pub(crate) fn current_failure(&self, failure: &Failure) -> bool {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.generation == failure.generation && state.lease.is_none()
    }
    pub(crate) fn synchronize_control_owner(
        &self,
        controller_surface_id: Option<uuid::Uuid>,
        epoch: u64,
    ) -> bool {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(lease) = &state.lease {
            lease.observe_control(controller_surface_id, epoch);
            if !state.closed && lease.is_current() {
                return true;
            }
        }
        self.shared
            .retire(&mut state, "Control changed; pending input discarded");
        false
    }
    pub(crate) fn send(&self, command: TerminalInput) -> Result<(), &'static str> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let error = if state.closed
            || !state
                .lease
                .as_ref()
                .is_some_and(TerminalInputLease::is_current)
        {
            Some("Input retired; request Control again")
        } else if state.commands.len() + usize::from(state.in_flight.is_some()) >= MAX_COMMANDS
            || state
                .queued_bytes
                .saturating_add(state.in_flight.unwrap_or(0))
                .saturating_add(command.retained_bytes())
                > MAX_PAYLOAD_BYTES
        {
            Some("Input queue limit reached; pending input discarded. Request Control again")
        } else {
            None
        };
        if let Some(error) = error {
            self.shared.retire(&mut state, error);
            return Err(error);
        }
        state.queued_bytes += command.retained_bytes();
        state.commands.push_back(command);
        self.shared.changed.notify_one();
        Ok(())
    }
}
impl Drop for Queue {
    fn drop(&mut self) {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.closed = true;
        self.shared
            .retire(&mut state, "Surface detached; pending input discarded");
    }
}
fn work(shared: Arc<Shared>) {
    loop {
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while !state.closed && state.commands.is_empty() {
            state = shared
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        if state.closed {
            return;
        }
        let Some(command) = state.commands.pop_front() else {
            continue;
        };
        let Some(lease) = state.lease.clone() else {
            continue;
        };
        state.queued_bytes -= command.retained_bytes();
        state.in_flight = Some(command.retained_bytes());
        let generation = state.generation;
        drop(state);
        let result = lease.send(command);
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.in_flight = None;
        if state.generation == generation
            && let Err(error) = result
        {
            shared.retire(&mut state, &format!("Input delivery uncertain or rejected; pending input discarded. Request Control again: {error}"));
        }
    }
}
