//! Pull-driven metadata: no decoding relay thread or secondary native FIFO.
//! Async frontends use a bounded handoff of connection-pinned deliveries.
use super::*;
use std::{cell::RefCell, sync::Condvar};
use ultraplexr_protocol::collection_stream::{FEATURE, SnapshotMarker};

/// Ordered snapshot delimiters and ordinary collection changes. Only a matched
/// end proves complete membership; a disconnected/partial snapshot does not.
pub enum MetadataUpdate<T> {
    Snapshot(SnapshotMarker),
    Item(T),
}

enum SnapshotState {
    Legacy,
    Initial,
    Collecting(Uuid),
    Live,
    Failed,
}
impl SnapshotState {
    fn for_stream(stream: &MultiplexedSubscription) -> Self {
        if stream
            .cancel
            .wire
            .features
            .iter()
            .any(|feature| feature == FEATURE)
        {
            Self::Initial
        } else {
            Self::Legacy
        }
    }
}

/// Consumer-side reconciliation stores only IDs for the in-progress snapshot,
/// not another copy of decoded records. Memory follows collection membership,
/// never the lifetime number of events. Use one tracker per ordered feed.
pub struct CollectionTracker<K> {
    pending: Option<(Uuid, std::collections::HashSet<K>)>,
    identity: Option<(std::sync::Weak<MultiplexedWire>, u32)>,
}
impl<K> Default for CollectionTracker<K> {
    fn default() -> Self {
        Self {
            pending: None,
            identity: None,
        }
    }
}
pub enum CollectionChange<K, T> {
    Item(T),
    Complete(std::collections::HashSet<K>),
}
impl<K: Eq + std::hash::Hash> CollectionTracker<K> {
    /// True until a negotiated snapshot is complete or abandoned. Legacy
    /// item-only streams never enter this state and cannot prove membership.
    pub fn snapshot_in_progress(&self) -> bool {
        self.pending.is_some()
    }

    /// Check connection identity at the final handoff, then track membership.
    /// An end without its begin (e.g. a discarded stale delivery) cannot prune.
    pub fn accept<T>(
        &mut self,
        delivery: MetadataDelivery<MetadataUpdate<T>>,
        key: impl FnOnce(&T) -> Option<K>,
    ) -> Option<CollectionChange<K, T>> {
        let same_stream = self.identity.as_ref().is_some_and(|(wire, id)| {
            *id == delivery.stream_id && wire.ptr_eq(&Arc::downgrade(&delivery.wire))
        });
        if !same_stream {
            self.pending = None;
            self.identity = Some((Arc::downgrade(&delivery.wire), delivery.stream_id));
        }
        let Some(update) = delivery.into_current() else {
            self.pending = None;
            return None;
        };
        match update {
            MetadataUpdate::Snapshot(SnapshotMarker::SnapshotBegin { generation }) => {
                self.pending = Some((generation, Default::default()));
                None
            }
            MetadataUpdate::Snapshot(SnapshotMarker::SnapshotEnd { generation }) => {
                let (expected, seen) = self.pending.take()?;
                (expected == generation).then_some(CollectionChange::Complete(seen))
            }
            MetadataUpdate::Item(item) => {
                if let Some((_, seen)) = &mut self.pending
                    && let Some(id) = key(&item)
                {
                    seen.insert(id);
                }
                Some(CollectionChange::Item(item))
            }
        }
    }
}

struct State {
    cancelled: bool,
    current: Option<SubscriptionCancel>,
}
struct Lifetime {
    state: Mutex<State>,
    changed: Condvar,
}
impl Lifetime {
    fn cancel(&self) {
        let current = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.cancelled = true;
            state.current.take()
        };
        if let Some(current) = current {
            // Local wakeup only. The receiver's worker owns the eventual
            // Unsubscribe write when its MultiplexedSubscription is dropped.
            current.wire.streams.cancel(current.stream_id);
        }
        self.changed.notify_all();
    }
    fn install(&self, cancel: SubscriptionCancel) -> Result<(), ClientError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?;
        if state.cancelled {
            return Err(disconnected_error("metadata receiver cancelled"));
        }
        state.current = Some(cancel);
        Ok(())
    }
    fn pause(&self, delay: Duration) -> Result<(), ClientError> {
        let state = self
            .state
            .lock()
            .map_err(|_| ClientError::ControlPoisoned)?;
        let (state, _) = self
            .changed
            .wait_timeout_while(state, delay, |state| !state.cancelled)
            .map_err(|_| ClientError::ControlPoisoned)?;
        if state.cancelled {
            Err(disconnected_error("metadata receiver cancelled"))
        } else {
            Ok(())
        }
    }
}

/// A local, nonblocking cancellation guard. Dropping it wakes a blocked native
/// receive or reconnect backoff, without a socket write on the dropping thread.
/// Keep it with the async consumer when a worker owns the receiver.
pub struct MetadataCancellation(Arc<Lifetime>);
impl Drop for MetadataCancellation {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// A decoded item pinned to its exact logical subscription and connection.
/// Bounded frontend queues must retain this wrapper until the point of use.
pub struct MetadataDelivery<T> {
    value: T,
    wire: Arc<MultiplexedWire>,
    stream_id: u32,
    lifetime: Arc<Lifetime>,
}
impl<T> MetadataDelivery<T> {
    /// Drop data queued before cancellation, disconnect or replacement. This
    /// is a local admission check, not a durable Share revalidation RPC.
    pub fn into_current(self) -> Option<T> {
        self.is_current().then_some(self.value)
    }
    /// Observe invalidation without consuming or cloning the retained value.
    pub fn is_current(&self) -> bool {
        self.lifetime.state.lock().is_ok_and(|state| {
            !state.cancelled
                && !self.wire.closed.load(Ordering::Acquire)
                && self.wire.streams.is_open(self.stream_id)
                && state.current.as_ref().is_some_and(|current| {
                    current.stream_id == self.stream_id && Arc::ptr_eq(&current.wire, &self.wire)
                })
        })
    }
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> MetadataDelivery<U> {
        MetadataDelivery {
            value: map(self.value),
            wire: self.wire,
            stream_id: self.stream_id,
            lifetime: self.lifetime,
        }
    }
}

/// Single-consumer blocking metadata stream. Reading directly drains the
/// bounded wire mailbox: there is no native relay thread or decoded FIFO.
/// `recv`/`recv_delivery` reconnect read-only after transient transport loss;
/// The item-only convenience methods omit snapshot markers. Collection clients
/// must use `recv_update_delivery` and reconcile only at a complete snapshot.
pub struct MetadataReceiver<T> {
    stream: RefCell<MultiplexedSubscription>,
    reconnect: Box<dyn Fn() -> Result<MultiplexedSubscription, ClientError> + Send>,
    decode: fn(&[u8]) -> Result<T, serde_json::Error>,
    lifetime: Arc<Lifetime>,
    snapshot: RefCell<SnapshotState>,
}
impl<T> MetadataReceiver<T> {
    pub(super) fn new(
        stream: MultiplexedSubscription,
        reconnect: impl Fn() -> Result<MultiplexedSubscription, ClientError> + Send + 'static,
        decode: fn(&[u8]) -> Result<T, serde_json::Error>,
    ) -> Self {
        let lifetime = Arc::new(Lifetime {
            state: Mutex::new(State {
                cancelled: false,
                current: Some(stream.cancel.clone()),
            }),
            changed: Condvar::new(),
        });
        let snapshot = RefCell::new(SnapshotState::for_stream(&stream));
        Self {
            stream: RefCell::new(stream),
            reconnect: Box::new(reconnect),
            decode,
            lifetime,
            snapshot,
        }
    }
    pub fn cancellation(&self) -> MetadataCancellation {
        MetadataCancellation(self.lifetime.clone())
    }

    /// Whether the current wire can prove complete collection membership.
    /// Recheck after a receive if reconnect may negotiate another peer.
    pub fn supports_collection_snapshots(&self) -> bool {
        !matches!(*self.snapshot.borrow(), SnapshotState::Legacy)
    }

    /// Receive an item for immediate use. Async queues should instead use
    /// `recv_delivery` and call `into_current` after their final handoff.
    pub fn recv(&self) -> Result<T, ClientError> {
        loop {
            if let Some(value) = self.recv_delivery()?.into_current() {
                return Ok(value);
            }
        }
    }
    /// One bounded wait on the current subscription. Unlike `recv`, this does
    /// not perform a potentially five-second transport handshake on timeout
    /// or disconnection; callers receive the error and choose whether to retry.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, ClientError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| disconnected_error("invalid metadata receive timeout"))?;
        loop {
            if let Some(MetadataUpdate::Item(item)) = self.read(Some(deadline))?.into_current() {
                return Ok(item);
            }
        }
    }
    pub fn recv_delivery(&self) -> Result<MetadataDelivery<T>, ClientError> {
        loop {
            let update = self.recv_update_delivery()?;
            if matches!(&update.value, MetadataUpdate::Item(_)) {
                return Ok(update.map(|update| match update {
                    MetadataUpdate::Item(item) => item,
                    MetadataUpdate::Snapshot(_) => unreachable!("item checked above"),
                }));
            }
        }
    }
    /// Includes negotiated snapshot markers. No full collection is buffered.
    pub fn recv_update_delivery(&self) -> Result<MetadataDelivery<MetadataUpdate<T>>, ClientError> {
        let mut delay = Duration::from_millis(50);
        loop {
            match self.read(None) {
                Ok(delivery) => return Ok(delivery),
                Err(ClientError::Protocol(error)) => return Err(error.into()),
                Err(error @ ClientError::ReceiveLimit { .. }) => return Err(error),
                Err(error @ ClientError::MetadataSnapshot(_)) => return Err(error),
                Err(_) => {
                    {
                        let mut state = self
                            .lifetime
                            .state
                            .lock()
                            .map_err(|_| ClientError::ControlPoisoned)?;
                        state.current = None;
                    }
                    loop {
                        self.lifetime.pause(delay)?;
                        match (self.reconnect)() {
                            Ok(stream) => {
                                self.lifetime.install(stream.cancel.clone())?;
                                *self.snapshot.borrow_mut() = SnapshotState::for_stream(&stream);
                                *self.stream.borrow_mut() = stream;
                                break;
                            }
                            Err(
                                error @ (ClientError::Remote { .. }
                                | ClientError::Version { .. }
                                | ClientError::ReceiveLimit { .. }),
                            ) => return Err(error),
                            Err(_) => delay = (delay * 2).min(Duration::from_secs(2)),
                        }
                    }
                }
            }
        }
    }
    fn read(
        &self,
        deadline: Option<Instant>,
    ) -> Result<MetadataDelivery<MetadataUpdate<T>>, ClientError> {
        {
            let state = self
                .lifetime
                .state
                .lock()
                .map_err(|_| ClientError::ControlPoisoned)?;
            if state.cancelled {
                return Err(disconnected_error("metadata receiver cancelled"));
            }
        }
        let stream = self.stream.borrow();
        if matches!(*self.snapshot.borrow(), SnapshotState::Failed) {
            return Err(ClientError::MetadataSnapshot("receiver permanently failed"));
        }
        let frame = stream
            .receive
            .recv_until(deadline)
            .map_err(|reason| stream.cancel.wire.receive_error(reason))?
            .ok_or_else(|| {
                ClientError::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "metadata receive timed out",
                ))
            })?;
        if frame.header.kind == FrameKind::Close {
            return Err(disconnected_error("metadata subscription ended"));
        }
        if !matches!(
            frame.header.kind,
            FrameKind::EventBatch | FrameKind::MetadataSnapshot
        ) || frame.header.stream_id != stream.stream_id
        {
            stream.cancel.wire.retire("invalid metadata frame");
            return Err(disconnected_error("invalid metadata frame"));
        }
        let value = if frame.header.kind == FrameKind::MetadataSnapshot {
            let marker: SnapshotMarker =
                serde_json::from_slice(&frame.payload).map_err(|error| {
                    *self.snapshot.borrow_mut() = SnapshotState::Failed;
                    stream
                        .cancel
                        .wire
                        .retire("malformed metadata snapshot marker");
                    ProtocolError::from(error)
                })?;
            let mut state = self.snapshot.borrow_mut();
            match (&*state, marker) {
                (
                    SnapshotState::Initial | SnapshotState::Live,
                    SnapshotMarker::SnapshotBegin { generation },
                ) => {
                    *state = SnapshotState::Collecting(generation);
                }
                (
                    SnapshotState::Collecting(expected),
                    SnapshotMarker::SnapshotEnd { generation },
                ) if *expected == generation => {
                    *state = SnapshotState::Live;
                }
                _ => {
                    *state = SnapshotState::Failed;
                    stream
                        .cancel
                        .wire
                        .retire("invalid metadata snapshot ordering");
                    return Err(ClientError::MetadataSnapshot(
                        "unnegotiated, nested or mismatched marker",
                    ));
                }
            }
            MetadataUpdate::Snapshot(marker)
        } else {
            if matches!(*self.snapshot.borrow(), SnapshotState::Initial) {
                *self.snapshot.borrow_mut() = SnapshotState::Failed;
                stream
                    .cancel
                    .wire
                    .retire("metadata item before snapshot begin");
                return Err(ClientError::MetadataSnapshot("item before snapshot begin"));
            }
            MetadataUpdate::Item((self.decode)(&frame.payload).map_err(|error| {
                *self.snapshot.borrow_mut() = SnapshotState::Failed;
                stream.cancel.wire.retire("malformed metadata record");
                ProtocolError::from(error)
            })?)
        };
        Ok(MetadataDelivery {
            value,
            wire: stream.cancel.wire.clone(),
            stream_id: stream.stream_id,
            lifetime: self.lifetime.clone(),
        })
    }
}
impl<T> Drop for MetadataReceiver<T> {
    fn drop(&mut self) {
        self.lifetime.cancel();
    }
}
