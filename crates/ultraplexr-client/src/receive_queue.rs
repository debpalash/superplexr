//! Ordered subscription mailboxes registered at ACK dispatch, with bounded
//! queued payload capacity. Overflow retires the wire rather than dropping deltas.
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use ultraplexr_protocol::wire_v3::{FrameKind, ReceivedFrame};

const STREAM_FRAMES: usize = 128;
pub(super) const STREAM_BYTES: usize = 8 * 1024 * 1024;
const WIRE_BYTES: usize = 32 * 1024 * 1024;
const WIRE_STREAMS: usize = 256;

#[derive(Debug)]
pub(super) enum PushError {
    Transient(&'static str),
    Oversized(usize),
}

pub(super) struct Budget {
    bytes: AtomicUsize,
    frames: AtomicUsize,
    streams: AtomicUsize,
    max_bytes: usize,
    max_frames: usize,
    max_streams: usize,
}
impl Budget {
    fn new(max_bytes: usize, max_frames: usize, max_streams: usize) -> Self {
        Self {
            bytes: AtomicUsize::new(0),
            frames: AtomicUsize::new(0),
            streams: AtomicUsize::new(0),
            max_bytes,
            max_frames,
            max_streams,
        }
    }
    pub(super) fn process() -> Arc<Self> {
        static BUDGET: OnceLock<Arc<Budget>> = OnceLock::new();
        BUDGET
            .get_or_init(|| Arc::new(Self::new(64 * 1024 * 1024, 8192, 1024)))
            .clone()
    }
}
fn reserve(count: &AtomicUsize, amount: usize, limit: usize) -> bool {
    count
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(amount).filter(|next| *next <= limit)
        })
        .is_ok()
}
struct PayloadCharge {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for PayloadCharge {
    fn drop(&mut self) {
        self.budget.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        self.budget.frames.fetch_sub(1, Ordering::AcqRel);
    }
}
struct StreamCharge(Arc<Budget>);
impl Drop for StreamCharge {
    fn drop(&mut self) {
        self.0.streams.fetch_sub(1, Ordering::AcqRel);
    }
}
struct Envelope {
    frame: ReceivedFrame,
    _charge: PayloadCharge,
}
struct Slot {
    frames: VecDeque<Envelope>,
    bytes: usize,
    attached: bool,
    retired: bool,
    ended: bool,
    _charge: StreamCharge,
}
#[derive(Default)]
struct State {
    slots: HashMap<u32, Slot>,
    bytes: usize,
    closed: Option<&'static str>,
}
struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    budget: Arc<Budget>,
}
#[derive(Clone)]
pub(super) struct Streams(Arc<Shared>);
impl Streams {
    pub(super) fn new(budget: Arc<Budget>) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State::default()),
            ready: Condvar::new(),
            budget,
        }))
    }
    pub(super) fn announce(&self, id: u32) -> Result<(), &'static str> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.closed.is_some() || id == 0 || state.slots.contains_key(&id) {
            return Err("invalid or duplicate subscription ACK");
        }
        if state.slots.len() >= WIRE_STREAMS
            || !reserve(&self.0.budget.streams, 1, self.0.budget.max_streams)
        {
            return Err("native subscription count limit reached");
        }
        state.slots.insert(
            id,
            Slot {
                frames: VecDeque::new(),
                bytes: 0,
                attached: false,
                retired: false,
                ended: false,
                _charge: StreamCharge(self.0.budget.clone()),
            },
        );
        Ok(())
    }
    pub(super) fn attach(&self, id: u32) -> Result<Receiver, &'static str> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(error) = state.closed {
            return Err(error);
        }
        let slot = state
            .slots
            .get_mut(&id)
            .ok_or("subscription has no acknowledged mailbox")?;
        if slot.attached || slot.retired {
            return Err("subscription receiver already attached or retired");
        }
        slot.attached = true;
        Ok(Receiver {
            streams: self.clone(),
            id,
        })
    }
    pub(super) fn push(&self, frame: ReceivedFrame) -> Result<(), PushError> {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(error) = state.closed {
            return Err(PushError::Transient(error));
        }
        let bytes = frame.payload.capacity();
        let total = state.bytes.saturating_add(bytes);
        let Some(slot) = state.slots.get_mut(&frame.header.stream_id) else {
            // A denied subscription can end without an acceptance/mailbox.
            return if frame.header.kind == FrameKind::Close && frame.header.stream_id != 0 {
                Ok(())
            } else {
                Err(PushError::Transient(
                    "event arrived before subscription acceptance",
                ))
            };
        };
        if slot.retired {
            return Ok(());
        }
        if slot.ended {
            return Err(PushError::Transient("event arrived after subscription end"));
        }
        if bytes > STREAM_BYTES {
            return Err(PushError::Oversized(bytes));
        }
        if slot.frames.len() >= STREAM_FRAMES
            || slot.bytes.saturating_add(bytes) > STREAM_BYTES
            || total > WIRE_BYTES
        {
            return Err(PushError::Transient(
                "native subscription receive queue limit reached",
            ));
        }
        let budget = &self.0.budget;
        if !reserve(&budget.bytes, bytes, budget.max_bytes) {
            return Err(PushError::Transient(
                "native process receive byte limit reached",
            ));
        }
        if !reserve(&budget.frames, 1, budget.max_frames) {
            budget.bytes.fetch_sub(bytes, Ordering::AcqRel);
            return Err(PushError::Transient(
                "native process receive frame limit reached",
            ));
        }
        slot.ended = frame.header.kind == FrameKind::Close;
        slot.bytes += bytes;
        slot.frames.push_back(Envelope {
            frame,
            _charge: PayloadCharge {
                budget: budget.clone(),
                bytes,
            },
        });
        state.bytes = total;
        self.0.ready.notify_all();
        Ok(())
    }
    pub(super) fn cancel(&self, id: u32) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(slot) = state.slots.get_mut(&id) {
            let bytes = slot.bytes;
            slot.frames.clear();
            slot.bytes = 0;
            slot.retired = true;
            state.bytes -= bytes;
        }
        // Keep a bounded empty tombstone only until the Unsubscribe ACK. This
        // discards in-flight events without rebuilding an orphan early queue.
        self.0.ready.notify_all();
    }
    pub(super) fn finish(&self, id: u32) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(slot) = state.slots.remove(&id) {
            state.bytes -= slot.bytes;
        }
        self.0.ready.notify_all();
    }
    pub(super) fn close(&self, reason: &'static str) {
        let mut state = self
            .0
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.closed = Some(reason);
        state.slots.clear();
        state.bytes = 0;
        self.0.ready.notify_all();
    }
    pub(super) fn is_open(&self, id: u32) -> bool {
        self.0.state.lock().is_ok_and(|state| {
            state.closed.is_none()
                && state
                    .slots
                    .get(&id)
                    .is_some_and(|slot| !slot.retired && !slot.ended)
        })
    }
}
pub(super) struct Receiver {
    streams: Streams,
    id: u32,
}
impl Receiver {
    pub(super) fn recv(&self) -> Result<ReceivedFrame, &'static str> {
        self.recv_until(None)?
            .ok_or("subscription receive timed out")
    }

    pub(super) fn recv_until(
        &self,
        deadline: Option<std::time::Instant>,
    ) -> Result<Option<ReceivedFrame>, &'static str> {
        let shared = &self.streams.0;
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(error) = state.closed {
                return Err(error);
            }
            let slot = state.slots.get_mut(&self.id).ok_or("subscription ended")?;
            if slot.retired {
                return Err("subscription retired");
            }
            if let Some(envelope) = slot.frames.pop_front() {
                slot.bytes -= envelope._charge.bytes;
                state.bytes -= envelope._charge.bytes;
                return Ok(Some(envelope.frame));
            }
            if slot.ended {
                return Err("subscription ended");
            }
            state = if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Ok(None);
                }
                shared
                    .ready
                    .wait_timeout(state, remaining)
                    .unwrap_or_else(|error| error.into_inner())
                    .0
            } else {
                shared
                    .ready
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner())
            };
        }
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.streams.cancel(self.id);
    }
}

#[cfg(test)]
#[path = "receive_queue_tests.rs"]
mod tests;
