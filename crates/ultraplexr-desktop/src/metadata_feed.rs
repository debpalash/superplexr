//! One-slot handoff to the desktop executor. The worker pulls directly from
//! the client's bounded wire mailbox, and dropping the feed wakes idle reads.
use std::sync::{Arc, Mutex};
use ultraplexr_client::{
    ClientError, MetadataCancellation, MetadataDelivery, MetadataReceiver, MetadataUpdate,
};

#[derive(Default)]
struct CancellationState {
    stopped: bool,
    receiver: Option<MetadataCancellation>,
}

struct CancelOnDrop(Arc<Mutex<CancellationState>>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let receiver = {
            let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
            state.stopped = true;
            state.receiver.take()
        };
        drop(receiver);
    }
}

pub(super) struct Feed<T> {
    // Dropped before the channel: wake native receive without GUI socket I/O.
    _cancel: CancelOnDrop,
    receive: tokio::sync::mpsc::Receiver<Result<MetadataDelivery<MetadataUpdate<T>>, String>>,
    ended: bool,
}
#[cfg(test)]
pub(super) fn spawn<T: Send + 'static>(events: MetadataReceiver<T>) -> std::io::Result<Feed<T>> {
    spawn_with(move || Ok(events))
}

/// Establish the read-only subscription on its worker, never on the GUI thread.
/// Closing the feed during establishment prevents the resulting receiver from
/// entering its read loop. The in-flight connection uses native client deadlines.
pub(super) fn spawn_with<T: Send + 'static>(
    connect: impl FnOnce() -> Result<MetadataReceiver<T>, ClientError> + Send + 'static,
) -> std::io::Result<Feed<T>> {
    let (send, receive) = tokio::sync::mpsc::channel(1);
    let cancellation = Arc::new(Mutex::new(CancellationState::default()));
    let cancel = CancelOnDrop(Arc::clone(&cancellation));
    std::thread::Builder::new()
        .name("ultraplexr-metadata-feed".into())
        .spawn(move || {
            if send.is_closed() {
                return;
            }
            let events = match connect() {
                Ok(events) => events,
                Err(error) => {
                    let detail = error.to_string().chars().take(1024).collect();
                    let _ = send.blocking_send(Err(detail));
                    return;
                }
            };
            {
                let mut state = cancellation
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if state.stopped {
                    return;
                }
                state.receiver = Some(events.cancellation());
            }
            loop {
                match events.recv_update_delivery() {
                    Ok(event) => {
                        if send.blocking_send(Ok(event)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        // One final bounded handoff, not another retry loop.
                        let detail = error.to_string().chars().take(1024).collect();
                        let _ = send.blocking_send(Err(detail));
                        break;
                    }
                }
            }
        })?;
    Ok(Feed {
        _cancel: cancel,
        receive,
        ended: false,
    })
}
impl<T> Feed<T> {
    pub(super) async fn recv_update(
        &mut self,
    ) -> Option<Result<MetadataDelivery<MetadataUpdate<T>>, String>> {
        if self.ended {
            return None;
        }
        let update = self
            .receive
            .recv()
            .await
            .unwrap_or_else(|| Err("The session-sync worker stopped unexpectedly".to_owned()));
        self.ended = update.is_err();
        Some(update)
    }
    #[cfg(test)]
    pub(super) async fn recv(&mut self) -> Option<MetadataDelivery<T>> {
        loop {
            let update = self.recv_update().await?.ok()?;
            // Tests for item-only handoff can share the production worker.
            let mut item = None;
            let delivery = update.map(|update| match update {
                MetadataUpdate::Item(value) => {
                    item = Some(value);
                }
                MetadataUpdate::Snapshot(_) => {}
            });
            if let Some(item) = item {
                return Some(delivery.map(|()| item));
            }
        }
    }
}
