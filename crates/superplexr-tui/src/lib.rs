//! Optional focused-session client. No PTY ownership or session database.
pub mod attach_worker;
pub mod group_actions;
pub mod input;
pub mod input_lane;
pub mod navigator;
pub mod navigator_worker;
pub mod render;
pub mod search;
pub mod workflow;
pub mod workspace;

use std::sync::{Arc, Mutex};
use superplexr_client::{
    ClientError, ControlClient, DaemonSession, EventSubscription, TerminalStreamUpdate,
};
use superplexr_core::SessionId;
use superplexr_protocol::ServerEvent;
use superplexr_terminal::{FullFrame, GridSize, KeyInput};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Two panes are already open; close a pane or replace the focused Session")]
    PaneLimit,
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("Observe mode: explicitly request Control first")]
    ObserveOnly,
    #[error("Control return pending; input stays disabled until acknowledged")]
    ReleasePending,
    #[error("Input queue limit reached; queued input was discarded and Control return requested")]
    InputQueueFull,
    #[error("This view uses queued input; synchronous writes are disabled")]
    QueuedInput,
    #[error(
        "Control claim was not confirmed; detach (^] d) and reconnect with a new observing view"
    )]
    ClaimUnconfirmed,
    #[error("Feed unavailable; input is disabled")]
    Unavailable,
    #[error("Frame mailbox poisoned")]
    Poisoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedStatus {
    Live,
    Reconnecting,
    Ended,
    Rejected,
    ReceiveLimited,
}

#[derive(Clone)]
pub struct View {
    pub frame: Option<Arc<FullFrame>>,
    pub status: FeedStatus,
    /// Changes even when a reconnect and its replacement frame are coalesced.
    pub continuity: u64,
}

struct Mailbox {
    view: View,
    dirty: bool,
    // Sticky across coalesced reconnect+frame events: input must remain opt-in.
    invalidate_control: bool,
}

/// Owns one view/subscription, never the remote process. Drop cancels the feed
/// and best-effort returns its own Control lease, without killing the Session.
pub struct FocusedSession {
    terminal: DaemonSession,
    subscription: Option<EventSubscription>,
    mailbox: Arc<Mutex<Mailbox>>,
    controlling: bool,
    lease_acquired: bool,
    input_lane: Option<input_lane::Lane>,
}

impl FocusedSession {
    pub fn attach(client: &ControlClient, id: SessionId) -> Result<Self, Error> {
        let terminal = client.terminal(id);
        let mailbox = Arc::new(Mutex::new(Mailbox {
            view: View {
                frame: None,
                status: FeedStatus::Reconnecting,
                continuity: 0,
            },
            dirty: false,
            invalidate_control: false,
        }));
        let publish = mailbox.clone();
        let subscription = terminal.subscribe_frames_with(move |update| {
            let Ok(mut state) = publish.lock() else {
                return false;
            };
            match update {
                TerminalStreamUpdate::Event(ServerEvent::TerminalFrame { frame, .. }) => {
                    state.view.frame = Some(Arc::from(frame));
                    state.view.status = FeedStatus::Live;
                }
                TerminalStreamUpdate::Event(
                    ServerEvent::TerminalExited { .. } | ServerEvent::TerminalFailed { .. },
                ) => {
                    state.view.status = FeedStatus::Ended;
                    state.invalidate_control = true;
                }
                TerminalStreamUpdate::Reconnecting => {
                    state.view.continuity = state.view.continuity.wrapping_add(1);
                    state.view.status = FeedStatus::Reconnecting;
                    state.invalidate_control = true;
                }
                TerminalStreamUpdate::Rejected => {
                    state.view.frame = None;
                    state.view.status = FeedStatus::Rejected;
                    state.invalidate_control = true;
                }
                TerminalStreamUpdate::ReceiveLimited => {
                    state.view.frame = None;
                    state.view.status = FeedStatus::ReceiveLimited;
                    state.view.continuity = state.view.continuity.wrapping_add(1);
                    state.invalidate_control = true;
                }
                _ => return true,
            }
            state.dirty = true;
            true
        })?;
        Ok(Self {
            terminal,
            subscription: Some(subscription),
            mailbox,
            controlling: false,
            lease_acquired: false,
            input_lane: None,
        })
    }

    /// Latest-state coalescing keeps this client's display queue bounded.
    pub fn take_update(&mut self) -> Result<Option<View>, Error> {
        let mut state = self.mailbox.lock().map_err(|_| Error::Poisoned)?;
        if state.invalidate_control {
            self.controlling = false;
            if let Some(lane) = &self.input_lane {
                lane.release();
            }
            state.invalidate_control = false;
        }
        if !state.dirty {
            return Ok(None);
        }
        state.dirty = false;
        Ok(Some(state.view.clone()))
    }

    pub fn controlling(&self) -> bool {
        self.input_lane
            .as_ref()
            .map_or(self.controlling, input_lane::Lane::controlling)
    }
    pub fn claim_pending(&self) -> bool {
        self.input_lane
            .as_ref()
            .is_some_and(input_lane::Lane::claim_pending)
    }

    /// Includes locally disabled input whose remote lease return is unconfirmed.
    pub fn release_pending(&self) -> bool {
        self.input_lane
            .as_ref()
            .map_or(self.lease_acquired, input_lane::Lane::release_pending)
    }
    /// Opaque local Surface identity, including duplicated views of one Session.
    pub fn surface_identity(&self) -> [u8; 16] {
        *self.terminal.surface_id().as_bytes()
    }
    /// Inspect a requested return without issuing or retrying any remote write.
    pub fn return_state(&self) -> input_lane::ReturnState {
        self.input_lane.as_ref().map_or_else(
            || {
                if self.lease_acquired {
                    input_lane::ReturnState::Unconfirmed
                } else {
                    input_lane::ReturnState::Released
                }
            },
            input_lane::Lane::return_state,
        )
    }

    /// Opt into the interactive TUI's bounded worker. Success means queued,
    /// never a remote acknowledgement. Synchronous write methods then refuse
    /// mixing ordering models; release remains a local fence/pending check.
    pub fn queue_input(&mut self, command: input_lane::Command) -> Result<(), Error> {
        if self.input_lane.is_none() {
            if self.lease_acquired {
                return Err(Error::ReleasePending);
            }
            self.input_lane = Some(
                input_lane::Lane::spawn(self.terminal.clone(), self.mailbox.clone())
                    .map_err(ClientError::Io)?,
            );
        }
        self.input_lane
            .as_ref()
            .ok_or(Error::Unavailable)?
            .submit(command)
    }

    pub fn take_input_message(&self) -> Option<String> {
        self.input_lane
            .as_ref()
            .and_then(input_lane::Lane::take_message)
    }
    pub fn input_failed(&self) -> bool {
        self.input_lane
            .as_ref()
            .is_some_and(input_lane::Lane::failed)
    }

    pub fn claim_control(&mut self) -> Result<(), Error> {
        if self.input_lane.is_some() {
            return Err(Error::QueuedInput);
        }
        if self
            .mailbox
            .lock()
            .map_err(|_| Error::Poisoned)?
            .view
            .status
            != FeedStatus::Live
        {
            return Err(Error::Unavailable);
        }
        self.terminal.claim_control(false)?;
        self.controlling = true;
        self.lease_acquired = true;
        Ok(())
    }

    pub fn release_control(&mut self) -> Result<(), Error> {
        self.controlling = false;
        if let Some(lane) = &self.input_lane {
            return if lane.release() {
                Ok(())
            } else {
                Err(Error::ReleasePending)
            };
        }
        if self.lease_acquired {
            self.terminal.release_control()?;
            self.lease_acquired = false;
        }
        Ok(())
    }

    fn can_input(&self) -> Result<(), Error> {
        if self.input_lane.is_some() {
            return Err(Error::QueuedInput);
        }
        if !self.controlling {
            return Err(Error::ObserveOnly);
        }
        let state = self.mailbox.lock().map_err(|_| Error::Poisoned)?;
        if state.invalidate_control || state.view.status != FeedStatus::Live {
            return Err(Error::Unavailable);
        }
        Ok(())
    }

    fn input_result(&mut self, result: Result<(), ClientError>) -> Result<(), Error> {
        if result.is_err() {
            self.controlling = false;
        }
        result.map_err(Error::from)
    }

    pub fn send_key(&mut self, key: KeyInput) -> Result<(), Error> {
        self.can_input()?;
        let result = self.terminal.send_key(key);
        self.input_result(result)
    }

    /// Call only after explicit user confirmation for multiline/escape paste.
    pub fn paste(&mut self, text: String, confirmed: bool) -> Result<(), Error> {
        self.can_input()?;
        let result = self.terminal.paste(text.into_bytes(), confirmed);
        self.input_result(result)
    }

    pub fn resize(&mut self, grid: GridSize) -> Result<(), Error> {
        self.can_input()?;
        let result = self.terminal.resize(grid, 0, 0);
        self.input_result(result)
    }

    pub fn history(&self, offset: u32) -> Result<Arc<FullFrame>, Error> {
        Ok(Arc::new(self.terminal.history_frame(offset.min(100_000))?))
    }
}

impl Drop for FocusedSession {
    fn drop(&mut self) {
        if let Some(lane) = &self.input_lane {
            lane.shutdown(self.subscription.take());
            return;
        }
        if let Some(subscription) = self.subscription.take() {
            subscription.cancel();
        }
        if self.lease_acquired {
            let _ = self.terminal.release_control();
        }
    }
}
