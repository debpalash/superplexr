//! Connection-owned, acknowledged native searches. No unbounded result relay.
use super::{
    AppState, ClientAuthority, RequestError, ResponseBody, ServerError, ServerResponse,
    SharedServerWireWriter, history_search, share_request, terminal_record,
};
use std::{
    collections::HashMap,
    net::Shutdown,
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use ultraplexr_core::SessionId;
use ultraplexr_protocol::{
    ProtocolError,
    search_stream::{
        MAX_CONNECTION_SEARCHES, MAX_PAGE_BYTES, MAX_PAGE_MATCHES, MAX_PROCESS_SEARCHES, SearchPage,
    },
    wire_v3::FrameKind,
};
use uuid::Uuid;

// A connection cap alone can be bypassed by opening more connections. Include
// replay/admission and writer backpressure in the process-wide active-job cap.
static SEARCH_SLOTS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(MAX_PROCESS_SEARCHES);

#[cfg(test)]
#[path = "search_stream_tests.rs"]
mod tests;

pub(super) struct Job {
    task: tokio::task::JoinHandle<()>,
    ack: mpsc::Sender<u64>,
    expected: Arc<AtomicU64>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
pub(super) struct Jobs(HashMap<Uuid, Job>);
impl Jobs {
    pub(super) fn admit(&mut self, id: Uuid) -> bool {
        self.0.retain(|_, job| !job.task.is_finished());
        !self.0.contains_key(&id) && self.0.len() < MAX_CONNECTION_SEARCHES
    }
    pub(super) async fn cancel(&mut self, id: Uuid) {
        if let Some(mut job) = self.0.remove(&id) {
            job.task.abort();
            // Await teardown so an acknowledged cancel also releases native
            // admission before another search on this connection starts.
            let _ = (&mut job.task).await;
        }
    }
    pub(super) async fn acknowledge(&mut self, id: Uuid, sequence: u64) -> bool {
        let valid = self.0.get(&id).is_some_and(|job| {
            sequence != 0
                && job
                    .expected
                    .compare_exchange(sequence, 0, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                && job.ack.try_send(sequence).is_ok()
        });
        if !valid {
            self.cancel(id).await;
        }
        valid
    }
    pub(super) fn start(
        &mut self,
        spec: Spec,
        authority: ClientAuthority,
        state: Arc<AppState>,
        wire: SharedServerWireWriter,
        socket: Option<Arc<UnixStream>>,
    ) -> bool {
        let Ok(permit) = SEARCH_SLOTS.try_acquire() else {
            return false;
        };
        let (ack, receive) = mpsc::channel(1);
        let expected = Arc::new(AtomicU64::new(0));
        let id = spec.search_id;
        let task_expected = expected.clone();
        let task = tokio::spawn(async move {
            let _permit = permit;
            let access =
                share_request::Access::new(&authority, &state.shares, &state.share_revocations);
            let writer = Writer {
                wire,
                socket,
                stream_id: spec.stream_id,
                access: &access,
            };
            let guarded = access.run(async {
                writer
                    .send(
                        FrameKind::Response,
                        0,
                        &ServerResponse::success(
                            spec.request_id,
                            ResponseBody::TerminalSearchStarted {
                                search_id: id,
                                stream_id: spec.stream_id,
                            },
                        ),
                    )
                    .await?;
                let mut sequence = 1;
                let result = tokio::time::timeout(
                    Duration::from_secs(15),
                    produce(
                        &spec,
                        &state,
                        &writer,
                        receive,
                        task_expected,
                        &mut sequence,
                    ),
                )
                .await;
                let error = match result {
                    Ok(Ok(())) => return Ok(()),
                    Ok(Err(error)) => error.to_string(),
                    Err(_) => "search stream deadline exceeded".into(),
                };
                let page = SearchPage {
                    search_id: id,
                    session_id: spec.session_id,
                    sequence,
                    matches: vec![],
                    complete: false,
                    error: Some(error.chars().take(256).collect()),
                };
                // Errors must not keep a non-reading socket alive indefinitely.
                tokio::time::timeout(
                    Duration::from_secs(1),
                    writer.send(FrameKind::SearchPage, spec.stream_id, &page),
                )
                .await
                .map_err(|_| {
                    ServerError::Request(RequestError::HistoryWorker(
                        "search error delivery deadline".into(),
                    ))
                })??;
                Ok(())
            });
            // Include acceptance writer queueing in the lifetime bound too.
            let result = tokio::time::timeout(Duration::from_secs(17), guarded).await;
            if !matches!(result, Ok(Ok(())))
                && let Some(socket) = &writer.socket
            {
                let _ = socket.shutdown(Shutdown::Both);
            }
        });
        self.0.insert(
            id,
            Job {
                task,
                ack,
                expected,
            },
        );
        true
    }
}

pub(super) struct Spec {
    pub request_id: Uuid,
    pub search_id: Uuid,
    pub session_id: SessionId,
    pub stream_id: u32,
    pub query: String,
    pub case_sensitive: bool,
    pub limit: usize,
}

struct Writer<'a> {
    wire: SharedServerWireWriter,
    socket: Option<Arc<UnixStream>>,
    stream_id: u32,
    access: &'a share_request::Access<'a>,
}
struct PartialWrite(Option<Arc<UnixStream>>);
impl Drop for PartialWrite {
    fn drop(&mut self) {
        if let Some(socket) = &self.0 {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}
impl Writer<'_> {
    async fn send<T: serde::Serialize>(
        &self,
        kind: FrameKind,
        stream_id: u32,
        value: &T,
    ) -> Result<(), ServerError> {
        let mut wire = self.wire.lock().await;
        self.access.revalidate().await?;
        let mut interrupted = PartialWrite(self.socket.clone());
        wire.send_json(kind, stream_id, value)
            .await
            .map_err(ProtocolError::from)?;
        interrupted.0 = None;
        Ok(())
    }
}

async fn produce(
    spec: &Spec,
    state: &AppState,
    writer: &Writer<'_>,
    mut ack: mpsc::Receiver<u64>,
    expected: Arc<AtomicU64>,
    sequence: &mut u64,
) -> Result<(), ServerError> {
    let record = terminal_record(state, spec.session_id)?;
    let mut source = history_search::Pages::start(
        &record,
        state.terminal_state_dir.clone(),
        spec.session_id,
        spec.query.clone(),
        spec.case_sensitive,
        spec.limit,
    )
    .await?;
    loop {
        let batch = source.next().await?;
        let complete = batch.complete;
        let mut remaining = batch.matches.into_iter().peekable();
        loop {
            let mut page = SearchPage {
                search_id: spec.search_id,
                session_id: spec.session_id,
                sequence: *sequence,
                matches: vec![],
                complete: false,
                error: None,
            };
            let mut bytes = 256_usize; // Envelope, counters and separators.
            while let Some(next) = remaining.peek() {
                // JSON escaping costs at most six bytes per input byte. Native
                // previews are <=64 KiB, so even one full row always fits.
                let cost = next.preview.len().saturating_mul(6).saturating_add(128);
                if bytes + cost > MAX_PAGE_BYTES || page.matches.len() == MAX_PAGE_MATCHES {
                    break;
                }
                bytes += cost;
                page.matches.push(remaining.next().expect("peeked match"));
            }
            if page.matches.is_empty() && remaining.peek().is_some() {
                return Err(RequestError::HistoryWorker(
                    "search preview exceeds wire page budget".into(),
                )
                .into());
            }
            page.complete = complete && remaining.peek().is_none();
            if !page.complete {
                expected.store(*sequence, Ordering::Release);
            }
            writer
                .send(FrameKind::SearchPage, writer.stream_id, &page)
                .await?;
            *sequence += 1;
            if page.complete {
                return Ok(());
            }
            if ack.recv().await != Some(page.sequence) {
                return Err(
                    RequestError::HistoryWorker("search acknowledgement missing".into()).into(),
                );
            }
            if remaining.peek().is_none() {
                break;
            }
        }
    }
}
