//! Input authority pinned to the exact acknowledged connection and Control
//! epoch. No retry, connection repair or implicit Control acquisition on send.
use super::*;

#[derive(Debug)]
pub enum TerminalInput {
    Key(KeyInput),
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
    Mouse(MouseInput),
    Resize {
        grid: GridSize,
        cell_width_px: u32,
        cell_height_px: u32,
    },
}
impl TerminalInput {
    /// Allocated capacity of owned input payloads; excludes fixed enum/queue overhead
    /// and the temporary encoded wire representation.
    pub fn retained_bytes(&self) -> usize {
        match self {
            Self::Paste { bytes, .. } => bytes.capacity(),
            Self::Key(key) => key
                .physical_key
                .capacity()
                .saturating_add(key.logical_key.capacity())
                .saturating_add(key.text.as_ref().map_or(0, String::capacity)),
            _ => 0,
        }
    }
    fn request(self, session_id: SessionId) -> Request {
        match self {
            Self::Key(input) => Request::TerminalKey { session_id, input },
            Self::Paste { bytes, confirmed } => Request::TerminalPaste {
                session_id,
                bytes,
                confirmed,
            },
            Self::Focus(focused) => Request::TerminalFocus {
                session_id,
                focused,
            },
            Self::Scroll(scroll) => Request::TerminalScroll { session_id, scroll },
            Self::Select {
                anchor,
                head,
                rectangle,
            } => Request::TerminalSelect {
                session_id,
                anchor,
                head,
                rectangle,
            },
            Self::ClearSelection => Request::TerminalClearSelection { session_id },
            Self::Mouse(input) => Request::TerminalMouse { session_id, input },
            Self::Resize {
                grid,
                cell_width_px,
                cell_height_px,
            } => Request::TerminalResize {
                session_id,
                grid,
                cell_width_px,
                cell_height_px,
            },
        }
    }
}

/// Clones share retirement, but never pick up a subsequent connection or epoch.
/// Retire is local/nonblocking. Already admitted socket writes cannot be undone.
#[derive(Clone)]
pub struct TerminalInputLease {
    session: DaemonSession,
    wire: Arc<MultiplexedWire>,
    epoch: u64,
    active: Arc<AtomicBool>,
}
impl TerminalInputLease {
    pub fn session_id(&self) -> SessionId {
        self.session.id()
    }
    pub fn retire(&self) {
        self.active.store(false, Ordering::Release);
    }
    /// Disable input locally, then return this exact acknowledged authority.
    /// Run on a worker: this waits for an ACK, but never reconnects or uses a
    /// newer epoch. A failed return remains unconfirmed; retry is caller-driven.
    /// Retirement does not prevent returning an already fenced lease.
    pub fn release_control(&self) -> Result<(), ClientError> {
        self.retire();
        let mut request = self
            .session
            .control
            .client_request(Request::ReleaseTerminalControl {
                session_id: self.session.id(),
            });
        request.surface_id = Some(self.session.surface_id());
        request.control_epoch = Some(self.epoch);
        let response = self.wire.exchange(&request)?;
        match decode_response(&request, response)? {
            ResponseBody::TerminalControlChanged {
                session_id,
                control_epoch,
            } if session_id == self.session.id()
                && control_epoch != 0
                && control_epoch != self.epoch =>
            {
                // A delayed return must not overwrite another acquisition's
                // local epoch. The ACK confirms only this lease's return.
                let _ = self.session.control_epoch.compare_exchange(
                    self.epoch,
                    control_epoch,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                Ok(())
            }
            body => Err(ClientError::UnexpectedResponse(Box::new(body))),
        }
    }
    /// Apply an authoritative ownership notification without acquiring Control.
    /// Notifications older than this lease's ACK cannot retire it; current or
    /// newer mismatches retire permanently, even if later metadata names us.
    pub fn observe_control(&self, controller_surface_id: Option<Uuid>, epoch: u64) {
        if epoch >= self.epoch
            && (epoch != self.epoch || controller_surface_id != Some(self.session.surface_id()))
        {
            self.retire();
        }
    }
    pub fn is_current(&self) -> bool {
        self.active.load(Ordering::Acquire)
            && !self.wire.closed.load(Ordering::Acquire)
            && self.session.control_epoch() == Some(self.epoch)
            && self
                .session
                .control
                .current_wire()
                .is_ok_and(|current| Arc::ptr_eq(&current, &self.wire))
    }
    /// Send once on a worker. Any error retires this lease, including uncertain
    /// replies. The caller must discard pending input and explicitly reclaim.
    pub fn send(&self, input: TerminalInput) -> Result<(), ClientError> {
        let mut request = self
            .session
            .control
            .client_request(input.request(self.session.id()));
        request.surface_id = Some(self.session.surface_id());
        request.control_epoch = Some(self.epoch);
        let result = self
            .wire
            .exchange_checked(&request, || self.is_current())
            .and_then(|response| decode_response(&request, response))
            .and_then(|body| match body {
                ResponseBody::TerminalCommandAccepted { .. } => Ok(()),
                body => Err(ClientError::UnexpectedResponse(Box::new(body))),
            });
        if result.is_err() {
            self.retire();
        }
        result
    }
}
impl DaemonSession {
    /// Explicitly claim Control and return input authority tied to that exact
    /// ACK/transport. Reconnection can occur before this claim, never on send.
    pub fn claim_input(&self, force: bool) -> Result<TerminalInputLease, ClientError> {
        let wire = self.control.subscription_wire()?;
        let mut request = self.control.client_request(Request::ClaimTerminalControl {
            session_id: self.id(),
            force,
        });
        request.surface_id = Some(self.surface_id());
        request.control_epoch = self.control_epoch();
        let response = wire
            .exchange(&request)
            .and_then(|response| decode_response(&request, response));
        let body = match response {
            Ok(body) => body,
            Err(error @ ClientError::Remote { .. }) => return Err(error),
            Err(error) => {
                // A missing ACK may hide a successful acquisition. There is no
                // known epoch with which to safely retry input/return. Retiring
                // the original wire invokes authenticated disconnect cleanup;
                // never replay the claim on a replacement connection.
                wire.retire("Control claim acknowledgement unconfirmed");
                return Err(error);
            }
        };
        let epoch = match body {
            ResponseBody::TerminalControlChanged {
                session_id,
                control_epoch,
            } if session_id == self.id() && control_epoch != 0 => control_epoch,
            body => {
                wire.retire("invalid Control claim acknowledgement");
                return Err(ClientError::UnexpectedResponse(Box::new(body)));
            }
        };
        self.control_epoch.store(epoch, Ordering::Release);
        let lease = TerminalInputLease {
            session: self.clone(),
            wire,
            epoch,
            active: Arc::new(AtomicBool::new(true)),
        };
        if epoch == 0 || !lease.is_current() {
            lease.wire.retire("Control changed during acquisition");
            return Err(disconnected_error("Control changed during acquisition"));
        }
        Ok(lease)
    }
}
