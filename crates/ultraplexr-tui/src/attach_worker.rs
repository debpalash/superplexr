//! One reusable, bounded observing-attachment worker. Network subscription and
//! retired-pane destruction never run in `finish` on the terminal input thread.
use crate::{
    Error,
    workspace::{Open, Pane, Workspace},
};
use std::sync::{Arc, Condvar, Mutex};
use ultraplexr_client::ControlClient;
use ultraplexr_core::SessionId;

struct Request {
    generation: u64,
    id: SessionId,
    mode: Open,
}
struct ResultSlot {
    generation: u64,
    mode: Open,
    result: Result<Pane, Error>,
}
#[derive(Default)]
struct State {
    stopped: bool,
    generation: u64,
    active: bool,
    request: Option<Request>,
    result: Option<ResultSlot>,
    retired: Option<Pane>,
}
pub struct Worker {
    shared: Arc<(Mutex<State>, Condvar)>,
}

impl Worker {
    pub fn spawn(client: ControlClient) -> std::io::Result<Self> {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("ultraplexr-attach".into())
            .spawn(move || {
                let (state, changed) = &*worker;
                loop {
                    let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
                    guard = changed
                        .wait_while(guard, |state| {
                            !state.stopped
                                && state.request.is_none()
                                && state.retired.is_none()
                                && !state
                                    .result
                                    .as_ref()
                                    .is_some_and(|result| result.generation != state.generation)
                        })
                        .unwrap_or_else(|error| error.into_inner());
                    if guard.stopped {
                        let result = guard.result.take();
                        let retired = guard.retired.take();
                        drop(guard);
                        drop(result);
                        drop(retired);
                        return;
                    }
                    if let Some(retired) = guard.retired.take() {
                        guard.active = true;
                        drop(guard);
                        drop(retired);
                        state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .active = false;
                        continue;
                    }
                    if guard
                        .result
                        .as_ref()
                        .is_some_and(|result| result.generation != guard.generation)
                    {
                        let result = guard.result.take();
                        guard.active = true;
                        drop(guard);
                        drop(result);
                        state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .active = false;
                        continue;
                    }
                    let Some(request) = guard.request.take() else {
                        continue;
                    };
                    guard.active = true;
                    drop(guard);
                    let result = Pane::attach(&client, request.id);
                    let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
                    if guard.stopped || guard.generation != request.generation {
                        // Keep admission occupied during network cleanup, but never
                        // hold the UI's state mutex while dropping a subscription.
                        drop(guard);
                        drop(result);
                        state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .active = false;
                    } else {
                        guard.active = false;
                        guard.result = Some(ResultSlot {
                            generation: request.generation,
                            mode: request.mode,
                            result,
                        });
                    }
                }
            })?;
        Ok(Self { shared })
    }

    pub fn busy(&self) -> bool {
        let state = self
            .shared
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active || state.request.is_some() || state.result.is_some() || state.retired.is_some()
    }

    /// At most one admitted attachment, result, or retiring pane. Repeated
    /// Enter does not grow a queue or open extra subscriptions.
    pub fn request(&self, id: SessionId, mode: Open) -> bool {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped
            || state.active
            || state.request.is_some()
            || state.result.is_some()
            || state.retired.is_some()
        {
            return false;
        }
        state.generation = state.generation.wrapping_add(1);
        state.request = Some(Request {
            generation: state.generation,
            id,
            mode,
        });
        changed.notify_one();
        true
    }

    /// This performs only local workspace changes. Pane destruction is handed
    /// back to this worker in the same lock scope as result consumption.
    pub fn finish(&self, workspace: &mut Workspace) -> Option<Result<(), Error>> {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped
            || state
                .result
                .as_ref()
                .is_none_or(|result| result.generation != state.generation)
        {
            return None;
        }
        let result = state.result.take()?;
        let outcome = match result.result {
            Err(error) => Err(error),
            Ok(pane) => match workspace.install_prepared(pane, result.mode) {
                Ok(retired) => {
                    state.retired = retired;
                    Ok(())
                }
                Err((error, pane)) => {
                    state.retired = Some(pane);
                    Err(error)
                }
            },
        };
        changed.notify_one();
        Some(outcome)
    }

    pub fn cancel(&self) {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.generation = state.generation.wrapping_add(1);
        state.request = None;
        // Leave a completed Pane for the worker to destroy, never this caller.
        changed.notify_one();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.stopped = true;
        state.request = None;
        changed.notify_one();
    }
}
