//! One lazy navigation worker, with one replaceable request and result slot.
//! Dropping this owner never joins a stalled remote request on the input thread.
use crate::navigator::{self, Entry, Tab};
use crate::workflow::{self, CatalogPage, Inspection};
use std::sync::{Arc, Condvar, Mutex};
use ultraplexr_client::{ClientError, ControlClient};
use ultraplexr_core::MissionId;
use ultraplexr_protocol::SessionGroupId;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Navigation(Tab, Option<MissionId>, Option<SessionGroupId>),
    Inspection(Inspection),
    Catalog(CatalogPage),
}

#[derive(Clone, Copy)]
struct Request {
    generation: u64,
    scope: Scope,
}

#[derive(Default)]
struct State {
    stopped: bool,
    generation: u64,
    target: Option<Scope>,
    pending: Option<Request>,
    active: bool,
    result: Option<Result<Vec<Entry>, ClientError>>,
}

pub struct Worker {
    shared: Arc<(Mutex<State>, Condvar)>,
}

impl Worker {
    pub fn spawn(client: ControlClient) -> std::io::Result<Self> {
        let shared = Arc::new((Mutex::new(State::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("ultraplexr-navigation".into())
            .spawn(move || {
                let (state, changed) = &*worker;
                loop {
                    let request = {
                        let guard = state.lock().unwrap_or_else(|error| error.into_inner());
                        let mut guard = changed
                            .wait_while(guard, |state| !state.stopped && state.pending.is_none())
                            .unwrap_or_else(|error| error.into_inner());
                        if guard.stopped {
                            return;
                        }
                        let Some(request) = guard.pending.take() else {
                            continue;
                        };
                        guard.active = true;
                        request
                    };
                    let cancelled = || {
                        let guard = state.lock().unwrap_or_else(|error| error.into_inner());
                        guard.stopped || guard.generation != request.generation
                    };
                    let result = match request.scope {
                        Scope::Inspection(inspection) => {
                            workflow::load(&client, inspection, &cancelled)
                        }
                        Scope::Catalog(page) => workflow::load_catalog(&client, page, &cancelled),
                        Scope::Navigation(tab, mission, group) => {
                            navigator::load(&client, tab, mission, group, &cancelled)
                        }
                    };
                    let mut guard = state.lock().unwrap_or_else(|error| error.into_inner());
                    guard.active = false;
                    if !guard.stopped && guard.generation == request.generation {
                        guard.result = match result {
                            Ok(Some(entries)) => Some(Ok(entries)),
                            Err(error) => Some(Err(error)),
                            Ok(None) => None,
                        };
                    }
                }
            })?;
        Ok(Self { shared })
    }

    /// Returns true when existing rows belong to a different navigation scope.
    pub fn request(
        &self,
        tab: Tab,
        mission: Option<MissionId>,
        group: Option<SessionGroupId>,
    ) -> bool {
        self.request_target(Scope::Navigation(tab, mission, group))
    }

    pub fn inspect(&self, inspection: Inspection) -> bool {
        self.request_target(Scope::Inspection(inspection))
    }

    pub fn catalog(&self, page: CatalogPage) -> bool {
        self.request_target(Scope::Catalog(page))
    }

    fn request_target(&self, scope: Scope) -> bool {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        let scope_changed = state.target != Some(scope);
        state.target = Some(scope);
        state.generation = state.generation.wrapping_add(1);
        state.pending = Some(Request {
            generation: state.generation,
            scope,
        });
        state.result = None;
        changed.notify_one();
        scope_changed
    }

    pub fn busy(&self) -> bool {
        let state = self
            .shared
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.active || state.pending.is_some() || state.result.is_some()
    }

    pub fn take(&self) -> Option<Result<Vec<Entry>, ClientError>> {
        self.shared
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .result
            .take()
    }

    pub fn cancel(&self) {
        let mut state = self
            .shared
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.target.take().is_some() {
            state.generation = state.generation.wrapping_add(1);
        }
        state.pending = None;
        state.result = None;
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (state, changed) = &*self.shared;
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.stopped = true;
        state.pending = None;
        state.result = None;
        changed.notify_one();
    }
}
