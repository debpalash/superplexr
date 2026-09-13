//! One I/O worker and one cancellation writer per frontend owner, never per query. The
//! latest pending command replaces older commands. No socket I/O under the
//! mailbox lock, no thread joins or network writes on the input loop.
use crate::{DaemonSession, SearchCancel};
use std::{
    io,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread,
};
use superplexr_protocol::search_stream::SearchPage;
use superplexr_terminal::{FullFrame, SearchMatch};

pub enum Update {
    Page(SearchPage),
    History(Arc<FullFrame>),
    Error(String),
}
enum Job {
    Search(DaemonSession, String, usize),
    History(DaemonSession, SearchMatch),
    HistoryOffset(DaemonSession, u32),
}
#[derive(Default)]
struct State {
    generation: u64,
    command: Option<Job>,
    update: Option<Update>,
    cancel: Option<(u64, SearchCancel)>,
    cancelling: bool,
    closed: bool,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
    fn wait<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed
            .wait(state)
            .unwrap_or_else(|error| error.into_inner())
    }
    fn current(&self, generation: u64) -> bool {
        let state = self.lock();
        !state.closed && state.generation == generation
    }
    fn publish(&self, generation: u64, update: Update) -> bool {
        let mut state = self.lock();
        while !state.closed && state.generation == generation && state.update.is_some() {
            state = self.wait(state);
        }
        if state.closed || state.generation != generation {
            return false;
        }
        state.update = Some(update);
        true
    }
}

/// Bounded, generation-scoped background search and history reads. Dropping or
/// cancelling discards queued output immediately; admitted I/O retires on the
/// workers. An already admitted history RPC has no wire cancellation operation.
pub struct Worker {
    shared: Arc<Shared>,
}
impl Worker {
    pub fn new() -> io::Result<Self> {
        let worker = Self {
            shared: Arc::default(),
        };
        let shared = worker.shared.clone();
        thread::Builder::new()
            .name("history-search-cancel".into())
            .spawn(move || cancel_loop(shared))?;
        let shared = worker.shared.clone();
        thread::Builder::new()
            .name("history-search".into())
            .spawn(move || work_loop(shared))?;
        Ok(worker)
    }
    fn replace(&self, command: Option<Job>) {
        let mut state = self.shared.lock();
        state.generation = state.generation.wrapping_add(1);
        state.command = command;
        state.update = None;
        self.shared.changed.notify_all();
    }
    pub fn search(&self, session: DaemonSession, query: String) {
        self.search_with_limit(session, query, 1000);
    }
    /// Search one session, clamping the requested match limit to 1..=1000.
    pub fn search_with_limit(&self, session: DaemonSession, query: String, limit: usize) {
        self.replace(Some(Job::Search(session, query, limit.clamp(1, 1000))));
    }
    pub fn history(&self, session: DaemonSession, found: SearchMatch) {
        self.replace(Some(Job::History(session, found)));
    }
    /// Read a retained page relative to the current bottom. This is a local
    /// view request, not a shared viewport mutation or stable history bookmark.
    pub fn history_offset(&self, session: DaemonSession, offset: u32) {
        self.replace(Some(Job::HistoryOffset(session, offset.min(100_000))));
    }
    pub fn cancel(&self) {
        self.replace(None);
    }
    pub fn take_update(&self) -> Option<Update> {
        let update = self.shared.lock().update.take();
        self.shared.changed.notify_all();
        update
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.closed = true;
        state.command = None;
        state.update = None;
        self.shared.changed.notify_all();
    }
}

fn cancel_loop(shared: Arc<Shared>) {
    loop {
        let mut state = shared.lock();
        while !state.closed
            && !state
                .cancel
                .as_ref()
                .is_some_and(|(generation, _)| *generation != state.generation)
        {
            state = shared.wait(state);
        }
        if let Some((_, cancel)) = state.cancel.take() {
            state.cancelling = true;
            drop(state);
            cancel.cancel();
            state = shared.lock();
            state.cancelling = false;
            shared.changed.notify_all();
        }
        if state.closed {
            return;
        }
    }
}
fn work_loop(shared: Arc<Shared>) {
    loop {
        let mut state = shared.lock();
        while !state.closed && (state.command.is_none() || state.cancelling) {
            state = shared.wait(state);
        }
        if state.closed {
            return;
        }
        let generation = state.generation;
        let Some(job) = state.command.take() else {
            continue;
        };
        drop(state);
        match job {
            Job::Search(session, query, limit) => {
                if let Err(error) = search(&shared, generation, session, query, limit) {
                    shared.publish(generation, Update::Error(error.to_string()));
                }
                let mut state = shared.lock();
                if state
                    .cancel
                    .as_ref()
                    .is_some_and(|(id, _)| *id == generation)
                {
                    state.cancel = None;
                }
            }
            Job::History(session, found) => {
                if !shared.current(generation) {
                    continue;
                }
                let result = u32::try_from(found.line)
                    .map_err(|_| "History row is out of range; search again".to_owned())
                    .and_then(|row| session.history_line(row).map_err(|error| error.to_string()))
                    .and_then(|frame| {
                        // Physical row offsets are not stable across pruning/reflow.
                        // Refuse an obviously stale target; this is not epoch proof.
                        if frame
                            .rows
                            .iter()
                            .any(|row| row.text().trim_end() == found.preview.trim_end())
                        {
                            Ok(Arc::new(frame))
                        } else {
                            Err("History changed; search again to locate this result".into())
                        }
                    });
                shared.publish(
                    generation,
                    match result {
                        Ok(frame) => Update::History(frame),
                        Err(error) => Update::Error(error),
                    },
                );
            }
            Job::HistoryOffset(session, offset) => {
                if !shared.current(generation) {
                    continue;
                }
                let update = match session.history_frame(offset) {
                    Ok(frame) => Update::History(Arc::new(frame)),
                    Err(error) => Update::Error(error.to_string()),
                };
                shared.publish(generation, update);
            }
        }
    }
}
fn search(
    shared: &Shared,
    generation: u64,
    session: DaemonSession,
    query: String,
    limit: usize,
) -> Result<(), crate::ClientError> {
    if !shared.current(generation) {
        return Ok(());
    }
    let mut stream = session.search_pages(query, false, limit)?;
    {
        let mut state = shared.lock();
        // Cancellation may precede admission. Retire on this worker in that case.
        if state.closed || state.generation != generation {
            drop(state);
            return Ok(());
        }
        state.cancel = Some((generation, stream.cancellation()));
        shared.changed.notify_all();
    }
    while shared.current(generation) {
        let Some(page) = stream.next_page()? else {
            break;
        };
        let complete = page.complete;
        if !shared.publish(generation, Update::Page(page)) || complete {
            break;
        }
    }
    Ok(())
}
