//! One bounded, cancellable history job per Session actor.
use super::RuntimeError;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError},
    },
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};
use superplexr_terminal::{HistorySearch, SearchBatch, TerminalModel};

#[cfg(test)]
#[path = "history_search_tests.rs"]
mod tests;

pub(super) type PageResult = Result<SearchBatch, String>;

/// Cloneable cancellation for one search, including admission and replay work.
/// Cancellation is irreversible; use a fresh value for a new independent job.
#[derive(Clone, Debug, Default)]
pub struct SearchCancellation(Arc<AtomicBool>);
impl SearchCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Default)]
struct PageWake(Mutex<Option<Waker>>);
impl PageWake {
    fn register(&self, waker: &Waker) -> Result<(), RuntimeError> {
        let next = waker.clone();
        let mut slot = self
            .0
            .lock()
            .map_err(|_| RuntimeError::Pty("search wake lock poisoned".into()))?;
        let previous = if slot
            .as_ref()
            .is_none_or(|current| !current.will_wake(waker))
        {
            slot.replace(next)
        } else {
            None
        };
        drop(slot);
        drop(previous);
        Ok(())
    }
    fn take(&self) -> Option<Waker> {
        self.0.lock().ok()?.take()
    }
    fn wake(&self) {
        // Waker implementations may reenter: never invoke one under our lock.
        if let Some(waker) = self.take() {
            waker.wake();
        }
    }
}

#[derive(Debug)]
pub(super) struct PageSender {
    send: Option<SyncSender<PageResult>>,
    wake: Arc<PageWake>,
}
impl PageSender {
    pub(super) fn try_send(&self, page: PageResult) -> Result<(), TrySendError<PageResult>> {
        let result = self
            .send
            .as_ref()
            .expect("sender lives until Drop")
            .try_send(page);
        if result.is_ok() {
            self.wake.wake();
        }
        result
    }
}
impl Drop for PageSender {
    fn drop(&mut self) {
        // Publish disconnect before waking, including queued-command teardown.
        drop(self.send.take());
        self.wake.wake();
    }
}

pub(super) fn channel(cancelled: SearchCancellation) -> (PageSender, SessionSearch) {
    let (send, receive) = mpsc::sync_channel(4);
    let wake = Arc::new(PageWake::default());
    (
        PageSender {
            send: Some(send),
            wake: Arc::clone(&wake),
        },
        SessionSearch {
            receive,
            cancelled,
            wake,
        },
    )
}

/// A single-consumer stream of bounded history result pages. Dropping it
/// cancels actor work and releases its tracked history references. `None` from
/// `next_page` means the caller's wait elapsed, not end of results; only a page
/// with `complete` set ends a successful search. The actor has a five-second
/// overall deadline, including time spent waiting for a slow consumer.
#[derive(Debug)]
pub struct SessionSearch {
    receive: Receiver<PageResult>,
    cancelled: SearchCancellation,
    wake: Arc<PageWake>,
}
impl SessionSearch {
    pub fn next_page(&self, timeout: Duration) -> Result<Option<SearchBatch>, RuntimeError> {
        if self.cancelled.is_cancelled() {
            return Err(RuntimeError::SearchIncomplete);
        }
        match self.receive.recv_timeout(timeout) {
            Ok(page) => page.map(Some).map_err(RuntimeError::Pty),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(RuntimeError::SearchIncomplete),
        }
    }

    /// Await a page without a forwarding thread, executor dependency, or polling
    /// timer. Dropping this future alone leaves the search resumable; dropping
    /// the SessionSearch cancels the job. Only `complete` marks successful EOF.
    pub async fn next_page_async(&mut self) -> Result<SearchBatch, RuntimeError> {
        std::future::poll_fn(|context| self.poll_page(context)).await
    }

    pub fn cancellation(&self) -> SearchCancellation {
        self.cancelled.clone()
    }

    fn poll_page(&mut self, context: &mut Context<'_>) -> Poll<Result<SearchBatch, RuntimeError>> {
        if self.cancelled.is_cancelled() {
            return Poll::Ready(Err(RuntimeError::SearchIncomplete));
        }
        match self.receive.try_recv() {
            Ok(page) => return Poll::Ready(page.map_err(RuntimeError::Pty)),
            Err(TryRecvError::Disconnected) => {
                return Poll::Ready(Err(RuntimeError::SearchIncomplete));
            }
            Err(TryRecvError::Empty) => {}
        }
        if let Err(error) = self.wake.register(context.waker()) {
            return Poll::Ready(Err(error));
        }
        // Recheck after registration: a send/drop racing with registration must
        // either be observed here or notify this waker, never strand the future.
        let result = match self.receive.try_recv() {
            Ok(page) => Poll::Ready(page.map_err(RuntimeError::Pty)),
            Err(TryRecvError::Disconnected) => Poll::Ready(Err(RuntimeError::SearchIncomplete)),
            Err(TryRecvError::Empty) => Poll::Pending,
        };
        if result.is_ready() {
            self.wake.take();
        }
        result
    }
}
impl Drop for SessionSearch {
    fn drop(&mut self) {
        self.cancelled.cancel();
        self.wake.take();
    }
}

pub(super) struct SearchWork {
    scan: HistorySearch,
    reply: PageSender,
    cancelled: SearchCancellation,
    deadline: Instant,
    pending: Option<PageResult>,
}
impl SearchWork {
    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.is_cancelled()
    }

    pub(super) fn new(
        scan: HistorySearch,
        reply: PageSender,
        cancelled: SearchCancellation,
        deadline: Instant,
    ) -> Self {
        Self {
            scan,
            reply,
            cancelled,
            deadline,
            pending: None,
        }
    }

    /// None retires work. A zero delay yields to queued actor messages before
    /// scanning again; a bounded wait handles full result queues without spin.
    pub(super) fn poll(&mut self, model: &TerminalModel, now: Instant) -> Option<Duration> {
        if self.cancelled.is_cancelled() {
            return None;
        }
        if now >= self.deadline {
            // A full receiver will observe disconnect after draining already
            // delivered pages. It never receives a false completion marker.
            let _ = self
                .reply
                .try_send(Err("history search deadline exceeded".into()));
            return None;
        }
        if self.pending.is_none() {
            let result = model
                .search_step(&mut self.scan)
                .map_err(|error| error.to_string());
            if matches!(&result, Ok(page) if !page.complete && page.matches.is_empty()) {
                return Some(Duration::ZERO);
            }
            self.pending = Some(result);
        }
        let page = self.pending.take().expect("pending page prepared above");
        let finished = !page.as_ref().is_ok_and(|page| !page.complete);
        match self.reply.try_send(page) {
            Ok(()) if finished => None,
            Ok(()) => Some(Duration::ZERO),
            Err(TrySendError::Full(page)) => {
                self.pending = Some(page);
                Some(Duration::from_millis(10).min(self.deadline.saturating_duration_since(now)))
            }
            Err(TrySendError::Disconnected(_)) => None,
        }
    }
}
