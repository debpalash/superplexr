//! A browser view owns one Surface, never the gateway's general authority.
//! Commands are ordered, lease-bound and never replayed after an uncertain result.
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use ultraplexr_client::{ClientError, DaemonSession, TerminalInput, TerminalInputLease};
use ultraplexr_terminal::{GridSize, KeyInput};
use uuid::Uuid;

use crate::Observer;

#[derive(Default)]
pub(crate) struct Views(Mutex<HashMap<Uuid, Arc<Surface>>>);

impl Views {
    pub(crate) fn insert(&self, terminal: DaemonSession) -> Result<Arc<Surface>, StatusCode> {
        let surface = Arc::new(Surface {
            id: Uuid::new_v4(),
            terminal,
            alive: AtomicBool::new(true),
            input: Mutex::new(InputAuthority {
                lease: None,
                touched: Instant::now(),
            }),
            state: Mutex::new(LeaseState {
                nonce: None,
                sequence: 0,
            }),
        });
        self.0
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .insert(surface.id, surface.clone());
        Ok(surface)
    }

    fn get(&self, id: Uuid) -> Result<Arc<Surface>, StatusCode> {
        self.0
            .lock()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            .get(&id)
            .cloned()
            .ok_or(StatusCode::GONE)
    }
}

struct LeaseState {
    nonce: Option<Uuid>,
    sequence: u64,
}

struct InputAuthority {
    lease: Option<TerminalInputLease>,
    touched: Instant,
}
impl InputAuthority {
    fn expired(&self) -> bool {
        self.lease.is_some() && self.touched.elapsed() >= Duration::from_secs(15)
    }
}

pub(crate) struct Surface {
    pub(crate) id: Uuid,
    terminal: DaemonSession,
    alive: AtomicBool,
    // Never held during an RPC. Detach can retire input while the command
    // state lock is occupied by a claim, heartbeat, or socket write.
    input: Mutex<InputAuthority>,
    state: Mutex<LeaseState>,
}

impl Surface {
    // Atomic invalidation cannot wait behind an in-flight runtime request.
    pub(crate) fn invalidate(&self) {
        self.alive.store(false, Ordering::Release);
        if let Some(input) = self
            .input
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .lease
            .as_ref()
        {
            input.retire();
        }
    }
    pub(crate) fn available(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub(crate) fn expired(&self) -> bool {
        self.input
            .lock()
            .map_or(true, |authority| authority.expired())
    }

    fn input(&self) -> Option<TerminalInputLease> {
        self.input
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .lease
            .clone()
    }

    fn apply(&self, request: CommandRequest) -> Result<serde_json::Value, StatusCode> {
        let mut state = self.state.try_lock().map_err(|_| StatusCode::CONFLICT)?;
        if !self.available() {
            return Err(StatusCode::GONE);
        }
        if request.sequence != state.sequence + 1 || request.sequence > 9_007_199_254_740_991 {
            return Err(StatusCode::CONFLICT);
        }
        // Consume even a rejected command; duplicate input can never execute twice.
        state.sequence = request.sequence;
        match request.action {
            Action::Claim {} => {
                // Do not replace known authority with a new browser nonce.
                // It must first be returned, or this attachment must end.
                if self.input().is_some() {
                    return Err(StatusCode::CONFLICT);
                }
                state.nonce = None;
                let input = match self.terminal.claim_input(false) {
                    Ok(input) => input,
                    Err(ClientError::Remote { .. }) => return Err(StatusCode::CONFLICT),
                    Err(_) => {
                        self.invalidate();
                        return Err(StatusCode::GONE);
                    }
                };
                {
                    let mut authority =
                        self.input.lock().unwrap_or_else(|error| error.into_inner());
                    // Retain even a late claim for exact-lease cleanup. Guard
                    // cleanup waits on state, so it cannot miss this handle.
                    if !self.available() {
                        input.retire();
                    }
                    authority.lease = Some(input);
                    authority.touched = Instant::now();
                }
                if !self.available() {
                    return Err(StatusCode::GONE);
                }
                state.nonce = Some(Uuid::new_v4());
            }
            action => {
                if state.nonce.is_none() || state.nonce != request.lease {
                    return Err(StatusCode::CONFLICT);
                }
                if self.expired() {
                    self.invalidate();
                    return Err(StatusCode::GONE);
                }
                let Some(input) = self.input() else {
                    state.nonce = None;
                    self.invalidate();
                    return Err(StatusCode::GONE);
                };
                if !input.is_current() {
                    state.nonce = None;
                    self.invalidate();
                    return Err(StatusCode::GONE);
                }
                let result = match action {
                    Action::Claim {} => unreachable!(),
                    Action::Release {} => {
                        state.nonce = None;
                        input.release_control().map(|()| {
                            let _ = self
                                .input
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .lease
                                .take();
                        })
                    }
                    Action::Heartbeat {} => {
                        // Runtime authority is checked, including revocation and takeover.
                        let capture = match self.terminal.capture() {
                            Ok(capture) => capture,
                            Err(_) => {
                                state.nonce = None;
                                self.invalidate();
                                return Err(StatusCode::FORBIDDEN);
                            }
                        };
                        input.observe_control(
                            capture.terminal.controller_surface_id,
                            capture.terminal.control_epoch,
                        );
                        if !input.is_current() {
                            state.nonce = None;
                            self.invalidate();
                            return Err(StatusCode::CONFLICT);
                        }
                        Ok(())
                    }
                    Action::Key { input: key } => {
                        if key.physical_key.len() > 64
                            || key.logical_key.len() > 64
                            || key.text.as_ref().is_some_and(|text| {
                                text.len() > 256 || text.chars().any(char::is_control)
                            })
                        {
                            return Err(StatusCode::BAD_REQUEST);
                        }
                        input.send(TerminalInput::Key(key))
                    }
                    Action::Paste { text, confirmed } => {
                        if text.len() > 16_384 {
                            return Err(StatusCode::PAYLOAD_TOO_LARGE);
                        }
                        if !confirmed && text.chars().any(char::is_control) {
                            return Err(StatusCode::PRECONDITION_REQUIRED);
                        }
                        input.send(TerminalInput::Paste {
                            bytes: text.into_bytes(),
                            confirmed,
                        })
                    }
                    Action::Resize { columns, rows } => {
                        if columns > 400 || rows > 200 {
                            return Err(StatusCode::BAD_REQUEST);
                        }
                        let grid =
                            GridSize::new(columns, rows).map_err(|_| StatusCode::BAD_REQUEST)?;
                        input.send(TerminalInput::Resize {
                            grid,
                            cell_width_px: 0,
                            cell_height_px: 0,
                        })
                    }
                };
                if result.is_err() {
                    state.nonce = None;
                    self.invalidate();
                    return Err(StatusCode::CONFLICT);
                }
            }
        }
        if !self.available() {
            state.nonce = None;
            return Err(StatusCode::GONE);
        }
        // Do not let a slow acknowledged command revive an expired view in
        // the gap before the SSE watchdog's next tick.
        let fresh = {
            let mut authority = self.input.lock().unwrap_or_else(|error| error.into_inner());
            if authority.expired() {
                false
            } else {
                authority.touched = Instant::now();
                true
            }
        };
        if !fresh {
            state.nonce = None;
            self.invalidate();
            return Err(StatusCode::GONE);
        }
        Ok(serde_json::json!({"sequence": state.sequence, "lease": state.nonce}))
    }
}

// The SSE subscription owns this guard. Dropping an idle HTTP body still
// retires the view and returns its runtime lease without killing the Session.
pub(crate) struct ViewGuard {
    pub(crate) surface: Arc<Surface>,
    pub(crate) views: Arc<Views>,
    // Remains admitted through late claim/return cleanup, not just HTTP life.
    pub(crate) permit: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl Drop for ViewGuard {
    fn drop(&mut self) {
        self.surface.invalidate();
        if let Ok(mut views) = self.views.0.lock() {
            views.remove(&self.surface.id);
        }
        let surface = self.surface.clone();
        let permit = self.permit.take();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut state = surface
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.nonce = None;
            let input = surface
                .input
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .lease
                .take();
            if let Some(input) = input {
                let _ = input.release_control();
            }
        });
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandRequest {
    sequence: u64,
    lease: Option<Uuid>,
    action: Action,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Claim {},
    Release {},
    Heartbeat {},
    Key {
        input: KeyInput,
    },
    Paste {
        text: String,
        #[serde(default)]
        confirmed: bool,
    },
    Resize {
        columns: u16,
        rows: u16,
    },
}

pub(crate) async fn command(
    State(observer): State<Observer>,
    Path(id): Path<Uuid>,
    Json(request): Json<CommandRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !observer.allow_control {
        return Err(StatusCode::FORBIDDEN);
    }
    let permit = observer
        .commands
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let surface = observer.views.get(id)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        surface.apply(request).map(Json)
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
}
