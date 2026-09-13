//! Request-owned bridge to canonical search pages. HTTP and native queues are
//! bounded independently; disconnected browsers cannot accumulate workers.
use super::*;
use serde::Deserialize;
use std::{sync::Mutex, time::Duration};
use superplexr_client::{DaemonSession, SearchCancel};
use superplexr_protocol::search_stream::SearchPage;
use tokio::sync::OwnedSemaphorePermit;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Query {
    query: String,
    #[serde(default)]
    case_sensitive: bool,
    limit: usize,
}
#[derive(Default)]
struct Lifetime {
    stopped: bool,
    cancel: Option<SearchCancel>,
}
type Shared = Arc<Mutex<Lifetime>>;

pub(super) async fn start(
    State(state): State<Observer>,
    Path(id): Path<SessionId>,
    Json(query): Json<Query>,
) -> Result<Response, StatusCode> {
    if query.query.is_empty() || query.query.len() > 1024 || !(1..=1000).contains(&query.limit) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if *state.shutdown.borrow() {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    let permit = Arc::new(
        state
            .searches
            .clone()
            .try_acquire_owned()
            .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?,
    );
    let lifetime: Shared = Arc::default();
    let (pages, mut receive) = mpsc::channel(1);
    let native_lifetime = lifetime.clone();
    let native_permit = permit.clone();
    let terminal = state.client.terminal(id);
    tokio::task::spawn_blocking(move || {
        let _permit = native_permit;
        if let Err(error) = produce(terminal, query, &native_lifetime, &pages) {
            let _ = pages.blocking_send(Err(error.to_string()));
        }
    });
    let (send, body) = mpsc::channel::<Result<Event, Infallible>>(1);
    let mut shutdown = state.shutdown.subscribe();
    tokio::spawn(async move {
        let deadline = tokio::time::sleep(Duration::from_secs(25));
        tokio::pin!(deadline);
        let invalidated = connection_lost(lifetime.clone());
        tokio::pin!(invalidated);
        let mut reason = None;
        loop {
            if *shutdown.borrow() {
                break;
            }
            let page = tokio::select! {
                biased;
                _ = send.closed() => break,
                _ = shutdown.changed() => break,
                _ = &mut invalidated => { reason = Some("Search connection ended; run a new search"); break; }
                _ = &mut deadline => { reason = Some("Search timed out; narrow the query"); break; }
                page = receive.recv() => page,
            };
            let (event, complete) = match page {
                Some(Ok(page)) => {
                    let complete = page.complete;
                    match Event::default().event("search-page").json_data(page) {
                        Ok(event) => (event, complete),
                        Err(_) => {
                            reason = Some("Search page could not be encoded");
                            break;
                        }
                    }
                }
                Some(Err(error)) => (Event::default().event("search-error").data(error), true),
                None => {
                    reason = Some("Search ended without a complete page");
                    break;
                }
            };
            tokio::select! {
                biased;
                _ = send.closed() => break,
                _ = shutdown.changed() => break,
                _ = &mut invalidated => { reason = Some("Search connection ended; run a new search"); break; }
                _ = &mut deadline => { reason = Some("Search timed out; narrow the query"); break; }
                result = send.send(Ok(event)) => if result.is_err() { break; },
            }
            if complete {
                break;
            }
        }
        // Drop the relay's receiver first, waking a producer blocked on HTTP
        // backpressure. Pre-admission cancellation is recorded before taking
        // the handle; late admission sees stopped and drops on its own worker.
        drop(receive);
        let cancel = {
            let mut lifetime = lifetime.lock().unwrap_or_else(|error| error.into_inner());
            lifetime.stopped = true;
            lifetime.cancel.take()
        };
        if let Some(cancel) = cancel {
            let cancel_permit = permit.clone();
            tokio::task::spawn_blocking(move || {
                let _permit = cancel_permit;
                cancel.cancel();
            });
        }
        if let Some(reason) = reason {
            let _ = tokio::time::timeout(
                Duration::from_secs(1),
                send.send(Ok(Event::default().event("search-error").data(reason))),
            )
            .await;
        }
    });
    Ok(Sse::new(ReceiverStream::new(body)).into_response())
}

fn produce(
    terminal: DaemonSession,
    query: Query,
    lifetime: &Shared,
    pages: &mpsc::Sender<Result<SearchPage, String>>,
) -> Result<(), ClientError> {
    if lifetime
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .stopped
    {
        return Ok(());
    }
    let mut stream = terminal.search_pages(query.query, query.case_sensitive, query.limit)?;
    {
        let mut state = lifetime.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped {
            return Ok(());
        }
        state.cancel = Some(stream.cancellation());
    }
    while let Some(page) = stream.next_page()? {
        let complete = page.complete;
        if pages.blocking_send(Ok(page)).is_err() || complete {
            break;
        }
    }
    Ok(())
}

async fn connection_lost(lifetime: Shared) {
    // Only active search relays use this timer. It also interrupts a relay
    // waiting to send to a browser that has stopped consuming HTTP data.
    let mut timer = tokio::time::interval(Duration::from_millis(100));
    loop {
        timer.tick().await;
        if lifetime
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .cancel
            .as_ref()
            .is_some_and(SearchCancel::connection_closed)
        {
            return;
        }
    }
}

pub(super) async fn history_line(
    State(state): State<Observer>,
    Path((id, row)): Path<(SessionId, u32)>,
) -> Result<Json<Snapshot>, StatusCode> {
    if row > 100_000 + u32::from(superplexr_terminal::MAX_ROWS) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let permit: OwnedSemaphorePermit = state
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let terminal = state.client.terminal(id);
        let status = terminal.capture().map_err(client_status)?.terminal.status;
        let frame = terminal.history_line(row).map_err(client_status)?;
        Ok(Json(Snapshot::from_frame(&frame, status)))
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
}
