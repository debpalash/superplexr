//! Ordered collection updates with one-slot handoffs and detach cancellation.
use super::*;
use superplexr_client::{MetadataDelivery, MetadataUpdate};
use superplexr_protocol::TerminalSessionSummary;

#[derive(Serialize)]
struct Entry {
    session_id: SessionId,
    status: TerminalSessionStatus,
    archived: bool,
    executable: Option<String>,
    title: Option<String>,
    directory: Option<String>,
}

impl From<TerminalSessionSummary> for Entry {
    fn from(terminal: TerminalSessionSummary) -> Self {
        Self {
            session_id: terminal.session_id,
            status: terminal.status,
            archived: terminal.archived,
            title: terminal
                .display_title
                .map(|title| title.chars().take(512).collect()),
            directory: terminal
                .display_directory
                .map(|directory| directory.chars().take(2048).collect()),
            executable: terminal.foreground_process.map(|process| {
                process
                    .executable
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(256)
                    .collect()
            }),
        }
    }
}

pub(super) async fn events(State(state): State<Observer>) -> Result<Response, StatusCode> {
    let permit = state
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::TOO_MANY_REQUESTS)?;
    let client = state.client.clone();
    // Keep the permit with admitted native work even if HTTP disappears during
    // connection setup. Dropping an abandoned result retires its receiver.
    let (native, permit) = tokio::task::spawn_blocking(move || {
        client
            .subscribe_terminals()
            .map(|receiver| (receiver, permit))
            .map_err(client_status)
    })
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)??;
    let cancellation = native.cancellation();
    let (publish, mut updates) =
        mpsc::channel::<Result<MetadataDelivery<MetadataUpdate<Entry>>, &'static str>>(1);
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        loop {
            if publish.is_closed() {
                break;
            }
            if !native.supports_collection_snapshots() {
                let _ = publish.blocking_send(Err("Session listing requires a runtime with collection snapshots; update the runtime."));
                break;
            }
            let update = native.recv_update_delivery();
            if !native.supports_collection_snapshots() {
                let _ = publish.blocking_send(Err(
                    "The reconnected runtime does not support collection snapshots.",
                ));
                break;
            }
            let update = match update {
                Ok(delivery) => Ok(delivery.map(|update| match update {
                    MetadataUpdate::Snapshot(marker) => MetadataUpdate::Snapshot(marker),
                    MetadataUpdate::Item(terminal) => MetadataUpdate::Item(Entry::from(terminal)),
                })),
                Err(ClientError::ReceiveLimit { .. }) => {
                    Err(superplexr_client::RECEIVE_LIMIT_MESSAGE)
                }
                Err(ClientError::Remote { .. }) => Err("Session-list access ended."),
                Err(_) => Err("Session-list subscription ended; reconnect explicitly."),
            };
            let ended = update.is_err();
            if publish.blocking_send(update).is_err() || ended {
                break;
            }
        }
    });
    let (sender, receiver) = mpsc::channel::<Result<Event, Infallible>>(1);
    let mut shutdown = state.shutdown.subscribe();
    tokio::spawn(async move {
        let _cancellation = cancellation;
        if *shutdown.borrow() {
            return;
        }
        let configuration = Event::default()
            .event("configuration")
            .data(if state.allow_control {
                "controller"
            } else {
                "observer"
            });
        tokio::select! {
            _ = shutdown.changed() => return,
            result = sender.send(Ok(configuration)) => if result.is_err() { return; }
        }
        let features = Event::default()
            .event("features")
            .data(if state.workflow_read {
                "{\"workflow_read\":true}"
            } else {
                "{\"workflow_read\":false}"
            });
        tokio::select! {
            _ = shutdown.changed() => return,
            result = sender.send(Ok(features)) => if result.is_err() { return; }
        }
        loop {
            let update = tokio::select! {
                _ = shutdown.changed() => return,
                _ = sender.closed() => return,
                update = updates.recv() => match update { Some(update) => update, None => return },
            };
            // Wait for HTTP capacity BEFORE admitting a connection-pinned item;
            // backpressure must not turn a stale native delivery into new data.
            let slot = tokio::select! {
                _ = shutdown.changed() => return,
                slot = sender.reserve() => match slot { Ok(slot) => slot, Err(_) => return },
            };
            let event = match update {
                Err(reason) => {
                    slot.send(Ok(Event::default().event("ended").data(reason)));
                    return;
                }
                Ok(delivery) => match delivery.into_current() {
                    None => continue,
                    Some(MetadataUpdate::Snapshot(marker)) => {
                        Event::default().event("collection").json_data(marker)
                    }
                    Some(MetadataUpdate::Item(entry)) => {
                        Event::default().event("session").json_data(entry)
                    }
                },
            };
            match event {
                Ok(event) => {
                    slot.send(Ok(event));
                }
                Err(_) => {
                    slot.send(Ok(Event::default()
                        .event("ended")
                        .data("Unable to encode session metadata.")));
                    return;
                }
            }
        }
    });
    Ok(Sse::new(ReceiverStream::new(receiver))
        .keep_alive(KeepAlive::default())
        .into_response())
}
