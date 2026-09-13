//! Await native search pages directly; cancel live admission and replay work
//! when the request is dropped. Canonical native objects stay on their owner.
use super::{
    ReplayBudget, RequestError, SessionId, TerminalRecord, replay_terminal_model_for_search,
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use superplexr_runtime::{RuntimeError, SearchCancellation, SessionHandle};
use superplexr_terminal::{SearchBatch, SearchMatch};

#[cfg(test)]
#[path = "history_search_tests.rs"]
mod tests;

const LIVE_DEADLINE: Duration = Duration::from_secs(5);
const REPLAY_DEADLINE: Duration = Duration::from_secs(15);

pub(super) struct CancelOnDrop(pub(super) SearchCancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// One live or archived page source, with fallback allowed only before any
/// result has been published and after proving that the live actor stopped.
pub(super) struct Pages {
    live: Option<(SessionHandle, superplexr_runtime::SessionSearch)>,
    replay: Option<ReplaySearch>,
    fallback: Option<(PathBuf, SessionId, String, bool, usize)>,
    _cancellation: CancelOnDrop,
}
impl Pages {
    pub(super) async fn start(
        record: &TerminalRecord,
        state_dir: PathBuf,
        session_id: SessionId,
        query: String,
        case_sensitive: bool,
        limit: usize,
    ) -> Result<Self, RequestError> {
        if query.len() > 1024 {
            return Err(RequestError::SearchQueryTooLong);
        }
        let cancellation = CancelOnDrop(SearchCancellation::default());
        let live = if let Ok(handle) = record.live_handle(session_id) {
            let handle = handle.clone();
            let owner = handle.clone();
            let query = query.clone();
            let token = cancellation.0.clone();
            match tokio::task::spawn_blocking(move || {
                owner.start_search_with_cancellation(query, case_sensitive, limit, token)
            })
            .await
            .map_err(|error| RequestError::HistoryWorker(error.to_string()))?
            {
                Ok(search) => Some((handle, search)),
                Err(RuntimeError::ActorStopped) => None,
                Err(error) => return Err(error.into()),
            }
        } else {
            None
        };
        let mut pages = Self {
            live,
            replay: None,
            fallback: Some((state_dir, session_id, query, case_sensitive, limit)),
            _cancellation: cancellation,
        };
        if pages.live.is_none() {
            pages.start_replay();
        }
        Ok(pages)
    }
    fn start_replay(&mut self) {
        if let Some((root, id, query, case, limit)) = self.fallback.take() {
            self.live = None;
            self.replay = Some(ReplaySearch::start(root, id, query, case, limit));
        }
    }
    pub(super) async fn next(&mut self) -> Result<SearchBatch, RequestError> {
        if let Some((handle, search)) = &mut self.live {
            let result = search.next_page_async().await;
            if matches!(result, Err(RuntimeError::SearchIncomplete)) && self.fallback.is_some() {
                let handle = handle.clone();
                if matches!(
                    tokio::task::spawn_blocking(move || handle.snapshot()).await,
                    Ok(Err(RuntimeError::ActorStopped))
                ) {
                    self.start_replay();
                } else {
                    return result.map_err(Into::into);
                }
            } else {
                if result.is_ok() {
                    self.fallback = None;
                }
                return result.map_err(Into::into);
            }
        }
        self.replay
            .as_mut()
            .ok_or(RequestError::Runtime(RuntimeError::SearchIncomplete))?
            .next_page()
            .await
    }
}

pub(super) async fn collect(
    record: &TerminalRecord,
    state_dir: PathBuf,
    session_id: SessionId,
    query: String,
    case_sensitive: bool,
    limit: usize,
) -> Result<Vec<SearchMatch>, RequestError> {
    if query.len() > 1024 {
        return Err(RequestError::SearchQueryTooLong);
    }
    let limit = limit.min(1000);
    let live = match record.live_handle(session_id) {
        Ok(handle) => {
            let result = collect_live(handle.clone(), query.clone(), case_sensitive, limit).await;
            if matches!(
                result,
                Err(RequestError::Runtime(RuntimeError::SearchIncomplete))
            ) {
                // An expired job is not a dead actor. Prove actor death before
                // replacing a failed one-shot collection with journal replay.
                let handle = handle.clone();
                if matches!(
                    tokio::task::spawn_blocking(move || handle.snapshot()).await,
                    Ok(Err(RuntimeError::ActorStopped))
                ) {
                    Err(RequestError::Runtime(RuntimeError::ActorStopped))
                } else {
                    result
                }
            } else {
                result
            }
        }
        Err(error) => Err(error),
    };
    match live {
        Ok(matches) => Ok(matches),
        Err(RequestError::TerminalNotRunning(_))
        | Err(RequestError::Runtime(RuntimeError::ActorStopped)) => {
            let mut replay =
                ReplaySearch::start(state_dir, session_id, query, case_sensitive, limit);
            tokio::time::timeout(REPLAY_DEADLINE, async {
                let mut matches = Vec::new();
                loop {
                    let page = replay.next_page().await?;
                    matches.extend(page.matches);
                    if page.complete {
                        return Ok(matches);
                    }
                }
            })
            .await
            .map_err(|_| RequestError::HistoryWorker("history search deadline exceeded".into()))?
        }
        Err(error) => Err(error),
    }
}

async fn collect_live(
    handle: SessionHandle,
    query: String,
    case_sensitive: bool,
    limit: usize,
) -> Result<Vec<SearchMatch>, RequestError> {
    let cancellation = CancelOnDrop(SearchCancellation::default());
    let token = cancellation.0.clone();
    tokio::time::timeout(LIVE_DEADLINE, async {
        // Only bounded command-queue admission uses a blocking worker. The
        // result is a Send receiver, never a native terminal/tracked reference.
        let mut search = tokio::task::spawn_blocking(move || {
            handle.start_search_with_cancellation(query, case_sensitive, limit, token)
        })
        .await
        .map_err(|error| RequestError::HistoryWorker(error.to_string()))??;
        let mut matches = Vec::new();
        loop {
            let page = search.next_page_async().await?;
            matches.extend(page.matches);
            if page.complete {
                return Ok(matches);
            }
        }
    })
    .await
    .map_err(|_| RequestError::Runtime(RuntimeError::ActorTimeout))?
}

/// Archived pages have one queued batch. The worker owns its replay model;
/// dropping this receiver cancels parsing/scanning and unblocks a full queue.
pub(super) struct ReplaySearch {
    receive: tokio::sync::mpsc::Receiver<Result<SearchBatch, RequestError>>,
    _cancellation: CancelOnDrop,
    _worker: tokio::task::JoinHandle<()>,
}
impl ReplaySearch {
    pub(super) fn start(
        state_dir: PathBuf,
        session_id: SessionId,
        query: String,
        case_sensitive: bool,
        limit: usize,
    ) -> Self {
        let cancellation = CancelOnDrop(SearchCancellation::default());
        let token = cancellation.0.clone();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let worker = tokio::task::spawn_blocking(move || {
            let run = || -> Result<(), RequestError> {
                let started = Instant::now();
                let model = replay_terminal_model_for_search(
                    &state_dir,
                    session_id,
                    ReplayBudget::HISTORY,
                    &token,
                )?;
                let mut scan = model.begin_search(&query, case_sensitive, limit.min(1000))?;
                loop {
                    if token.is_cancelled() || send.is_closed() {
                        return Ok(());
                    }
                    if started.elapsed() >= REPLAY_DEADLINE {
                        return Err(RequestError::HistoryWorker(
                            "history search deadline exceeded".into(),
                        ));
                    }
                    let page = model.search_step(&mut scan)?;
                    let complete = page.complete;
                    if (complete || !page.matches.is_empty())
                        && send.blocking_send(Ok(page)).is_err()
                    {
                        return Ok(());
                    }
                    if complete {
                        return Ok(());
                    }
                }
            };
            if let Err(error) = run() {
                let _ = send.blocking_send(Err(error));
            }
        });
        Self {
            receive,
            _cancellation: cancellation,
            _worker: worker,
        }
    }
    pub(super) async fn next_page(&mut self) -> Result<SearchBatch, RequestError> {
        self.receive
            .recv()
            .await
            .ok_or(RequestError::Runtime(RuntimeError::SearchIncomplete))?
    }
}
