//! Ordered, non-replayed terminal writes behind one bounded input interface.
use crate::{Error, FeedStatus, Mailbox};
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};
use superplexr_client::{
    ClientError, DaemonSession, EventSubscription, TerminalInput, TerminalInputLease,
};
use superplexr_terminal::{GridSize, KeyInput, MAX_PASTE_BYTES};

const MAX_ACTIONS: usize = 128;
const MAX_BYTES: usize = MAX_PASTE_BYTES;

pub enum Command {
    Claim(GridSize),
    Key(KeyInput),
    Paste { text: String, confirmed: bool },
    Resize(GridSize),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnState {
    Released,
    Pending,
    Unconfirmed,
}
impl Command {
    fn bytes(&self) -> usize {
        match self {
            Self::Key(key) => key
                .physical_key
                .len()
                .saturating_add(key.logical_key.len())
                .saturating_add(key.text.as_ref().map_or(0, String::len))
                .saturating_add(64),
            Self::Paste { text, .. } => text.len(),
            _ => 64,
        }
    }
}
#[derive(Clone, Copy)]
struct Stamp {
    generation: u64,
    continuity: u64,
}
struct Job {
    stamp: Stamp,
    command: Command,
}
#[derive(Default)]
struct State {
    stopped: bool,
    generation: u64,
    active: bool,
    controlling: bool,
    claim_pending: bool,
    lease_acquired: bool,
    claim_uncertain: bool,
    input: Option<TerminalInputLease>,
    release_requested: bool,
    queue: VecDeque<Job>,
    bytes: usize,
    message: Option<String>,
    failed: bool,
    subscription: Option<EventSubscription>,
}
impl State {
    fn fence(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.controlling = false;
        self.claim_pending = false;
        if let Some(input) = &self.input {
            input.retire();
        }
        self.queue.clear();
        self.bytes = 0;
    }
}
pub(crate) struct Lane {
    shared: Arc<(Mutex<State>, Condvar)>,
    mailbox: Arc<Mutex<Mailbox>>,
}
impl Lane {
    pub(crate) fn spawn(
        terminal: DaemonSession,
        mailbox: Arc<Mutex<Mailbox>>,
    ) -> std::io::Result<Self> {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = shared.clone();
        let view = mailbox.clone();
        std::thread::Builder::new()
            .name("superplexr-input".into())
            .spawn(move || run(terminal, view, worker))?;
        Ok(Self { shared, mailbox })
    }
    pub(crate) fn submit(&self, command: Command) -> Result<(), Error> {
        let continuity = {
            let mailbox = self.mailbox.lock().map_err(|_| Error::Poisoned)?;
            if mailbox.invalidate_control || mailbox.view.status != FeedStatus::Live {
                return Err(Error::Unavailable);
            }
            mailbox.view.continuity
        };
        let (state, changed) = &*self.shared;
        let mut state = state.lock().map_err(|_| Error::Poisoned)?;
        if state.stopped || state.release_requested {
            return Err(Error::Unavailable);
        }
        let claim = matches!(command, Command::Claim(_));
        if claim {
            if state.claim_uncertain {
                return Err(Error::ClaimUnconfirmed);
            }
            if state.active || state.lease_acquired || !state.queue.is_empty() {
                return Err(Error::ReleasePending);
            }
            state.failed = false;
            state.message = None;
        } else if !state.controlling
            && !(matches!(command, Command::Resize(_)) && state.claim_pending)
        {
            return Err(Error::ObserveOnly);
        }
        // Only adjacent geometry requests can collapse. Never move a resize
        // across a key/paste, or carry one across a claim/continuity fence.
        let generation = state.generation;
        if let Command::Resize(grid) = &command
            && let Some(last) = state.queue.back_mut()
            && last.stamp.generation == generation
            && last.stamp.continuity == continuity
        {
            match &mut last.command {
                Command::Resize(queued) | Command::Claim(queued) => {
                    *queued = *grid;
                    return Ok(());
                }
                _ => {}
            }
        }
        let bytes = command.bytes();
        if state.queue.len() >= MAX_ACTIONS
            || bytes > MAX_BYTES
            || state.bytes.saturating_add(bytes) > MAX_BYTES
        {
            state.fence();
            state.release_requested = true;
            state.failed = true;
            state.message = Some("Input queue exceeded its limit; queued input discarded and Control return requested".into());
            changed.notify_one();
            return Err(Error::InputQueueFull);
        }
        if claim {
            state.claim_pending = true;
        }
        state.queue.push_back(Job {
            stamp: Stamp {
                generation,
                continuity,
            },
            command,
        });
        state.bytes += bytes;
        changed.notify_one();
        Ok(())
    }
    pub(crate) fn controlling(&self) -> bool {
        self.shared
            .0
            .lock()
            .is_ok_and(|state| state.controlling && !state.stopped)
    }
    pub(crate) fn claim_pending(&self) -> bool {
        self.shared
            .0
            .lock()
            .is_ok_and(|state| state.claim_pending && !state.stopped && !state.release_requested)
    }
    pub(crate) fn failed(&self) -> bool {
        self.shared.0.lock().map_or(true, |state| state.failed)
    }
    pub(crate) fn release_pending(&self) -> bool {
        self.shared.0.lock().map_or(true, |state| {
            state.active
                || state.release_requested
                || state.lease_acquired
                || state.claim_uncertain
                || !state.queue.is_empty()
        })
    }
    pub(crate) fn return_state(&self) -> ReturnState {
        let Ok(state) = self.shared.0.lock() else {
            return ReturnState::Unconfirmed;
        };
        if state.active || state.release_requested || !state.queue.is_empty() {
            ReturnState::Pending
        } else if state.lease_acquired || state.claim_uncertain {
            ReturnState::Unconfirmed
        } else {
            ReturnState::Released
        }
    }
    /// Fence local input immediately. True means already free; false means the
    /// single worker still has an admitted write or lease return to finish.
    pub(crate) fn release(&self) -> bool {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        let pending = state.active
            || state.release_requested
            || state.lease_acquired
            || state.claim_uncertain
            || !state.queue.is_empty();
        state.fence();
        if pending {
            state.release_requested = true;
            changed.notify_one();
        }
        !pending
    }
    pub(crate) fn take_message(&self) -> Option<String> {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .message
            .take()
    }
    pub(crate) fn shutdown(&self, subscription: Option<EventSubscription>) {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped && subscription.is_none() {
            return;
        }
        state.fence();
        state.stopped = true;
        state.release_requested = true;
        if subscription.is_some() {
            state.subscription = subscription;
        }
        changed.notify_one();
    }
}
impl Drop for Lane {
    fn drop(&mut self) {
        self.shutdown(None);
    }
}

fn current(
    shared: &Arc<(Mutex<State>, Condvar)>,
    mailbox: &Arc<Mutex<Mailbox>>,
    stamp: &Stamp,
) -> bool {
    let valid = shared.0.lock().is_ok_and(|state| {
        !state.stopped && state.generation == stamp.generation && !state.release_requested
    });
    valid
        && mailbox.lock().is_ok_and(|state| {
            !state.invalidate_control
                && state.view.status == FeedStatus::Live
                && state.view.continuity == stamp.continuity
        })
}

fn run(
    terminal: DaemonSession,
    mailbox: Arc<Mutex<Mailbox>>,
    shared: Arc<(Mutex<State>, Condvar)>,
) {
    let (state, changed) = &*shared;
    loop {
        let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
        guard = changed
            .wait_while(guard, |state| {
                !state.stopped && !state.release_requested && state.queue.is_empty()
            })
            .unwrap_or_else(|error| error.into_inner());
        if guard.release_requested {
            guard.release_requested = false;
            guard.active = true;
            let leased = guard.lease_acquired;
            let input = guard.input.clone();
            drop(guard);
            // Return only the original wire/epoch, even after another read
            // worker has reconnected. A missing handle is not proof of return.
            let result = if leased {
                input
                    .ok_or_else(retired_input_error)
                    .and_then(|input| input.release_control())
            } else {
                Ok(())
            };
            let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
            guard.active = false;
            match result {
                Ok(()) => {
                    guard.lease_acquired = false;
                    guard.input = None;
                    if guard.claim_uncertain {
                        guard.message = Some("Claim outcome unconfirmed; ^] d detach, then reconnect with a new observing view".into());
                    } else if !guard.failed {
                        guard.message = Some("Control returned; observing".into());
                    }
                }
                Err(error) => {
                    guard.failed = true;
                    guard.message = Some(format!("Control return unconfirmed: {error}; explicitly retry return before changing views").chars().take(1024).collect());
                }
            }
            continue;
        }
        if guard.stopped {
            let subscription = guard.subscription.take();
            drop(guard);
            if let Some(subscription) = subscription {
                subscription.cancel();
            }
            return;
        }
        let Some(job) = guard.queue.pop_front() else {
            continue;
        };
        guard.bytes = guard.bytes.saturating_sub(job.command.bytes());
        guard.active = true;
        drop(guard);
        if !current(&shared, &mailbox, &job.stamp) {
            let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
            guard.active = false;
            guard.fence();
            guard.release_requested = true;
            guard.failed = true;
            guard.message = Some("Connection changed; unsent input discarded".into());
            continue;
        }
        let claim = matches!(job.command, Command::Claim(_));
        let paste = matches!(job.command, Command::Paste { .. });
        let stamp = job.stamp;
        let input = state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .input
            .clone();
        let result = match job.command {
            Command::Claim(grid) => terminal.claim_input(false).and_then(|input| {
                {
                    let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
                    guard.lease_acquired = true;
                    guard.input = Some(input.clone());
                    if guard.stopped
                        || guard.generation != stamp.generation
                        || guard.release_requested
                    {
                        input.retire();
                    }
                }
                if current(&shared, &mailbox, &stamp) {
                    input.send(TerminalInput::Resize {
                        grid,
                        cell_width_px: 0,
                        cell_height_px: 0,
                    })
                } else {
                    Ok(())
                }
            }),
            Command::Key(key) => send_input(input, TerminalInput::Key(key)),
            Command::Paste { text, confirmed } => send_input(
                input,
                TerminalInput::Paste {
                    bytes: text.into_bytes(),
                    confirmed,
                },
            ),
            Command::Resize(grid) => send_input(
                input,
                TerminalInput::Resize {
                    grid,
                    cell_width_px: 0,
                    cell_height_px: 0,
                },
            ),
        };
        let valid = current(&shared, &mailbox, &stamp);
        let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
        guard.active = false;
        match result {
            Ok(())
                if valid
                    && !guard.stopped
                    && guard.generation == stamp.generation
                    && !guard.release_requested =>
            {
                if claim {
                    guard.claim_pending = false;
                    guard.controlling = true;
                    guard.message = Some("Control acquired".into());
                } else if paste {
                    guard.message = Some("Paste acknowledged".into());
                }
            }
            Ok(()) => {
                guard.fence();
                guard.release_requested = true;
                guard.failed = true;
                guard.message = Some(
                    "View changed; queued input discarded and Control return requested".into(),
                );
            }
            Err(error) => {
                let denied_claim =
                    claim && !guard.lease_acquired && matches!(&error, ClientError::Remote { .. });
                if claim && !guard.lease_acquired && !denied_claim {
                    guard.claim_uncertain = true;
                }
                guard.fence();
                guard.release_requested = true;
                guard.failed = true;
                let message = if denied_claim {
                    format!("Control not acquired: {error}; observing, retry claim explicitly")
                } else {
                    format!("Write unconfirmed: {error}; input disabled, no write will be replayed")
                };
                guard.message = Some(message.chars().take(1024).collect());
            }
        }
    }
}

fn send_input(lease: Option<TerminalInputLease>, input: TerminalInput) -> Result<(), ClientError> {
    lease.ok_or_else(retired_input_error)?.send(input)
}

fn retired_input_error() -> ClientError {
    ClientError::Io(std::io::Error::new(
        std::io::ErrorKind::ConnectionAborted,
        "TUI input authority retired",
    ))
}
