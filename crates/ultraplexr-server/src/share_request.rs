//! Revocable ordinary RPCs, including writer admission and response I/O.
use super::{
    AppState, ClientAuthority, ServerError, ServerResponse, SharedServerWireWriter,
    release_client_controllers,
    share_store::{ShareError, ShareStore},
};
use std::{collections::HashMap, future::Future, sync::Arc};
use tokio::sync::{Mutex, broadcast};
use ultraplexr_protocol::wire_v3::FrameKind;
use uuid::Uuid;

/// Every connection exit (including protocol errors and cancellation) retires
/// its streams and Control leases. No child task may keep its writer alive.
pub(super) struct ConnectionTasks {
    state: Arc<AppState>,
    pub(super) client_id: Option<Uuid>,
    // None means no authenticated identity yet; Some(None) is the owner.
    pub(super) authority: Option<Option<Uuid>>,
    pub(super) subscriptions: HashMap<u32, tokio::task::JoinHandle<()>>,
}
impl ConnectionTasks {
    pub(super) fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            client_id: None,
            authority: None,
            subscriptions: HashMap::new(),
        }
    }
}
impl Drop for ConnectionTasks {
    fn drop(&mut self) {
        for subscription in self.subscriptions.values() {
            subscription.abort();
        }
        if let (Some(client_id), Some(share_id)) = (self.client_id, self.authority) {
            release_client_controllers(&self.state, client_id, share_id);
        }
    }
}

/// The operation includes computation, writer queueing and response writing.
/// On cancellation the caller MUST terminate the connection: a partially
/// written frame cannot safely be followed by another message on that stream.
/// Already delivered bytes and effects admitted before revocation are not undone.
pub(super) struct Access<'a> {
    authority: &'a ClientAuthority,
    shares: &'a Mutex<ShareStore>,
    revocations: &'a broadcast::Sender<Uuid>,
}
impl<'a> Access<'a> {
    pub(super) fn new(
        authority: &'a ClientAuthority,
        shares: &'a Mutex<ShareStore>,
        revocations: &'a broadcast::Sender<Uuid>,
    ) -> Self {
        Self {
            authority,
            shares,
            revocations,
        }
    }
    pub(super) async fn run<T>(
        &self,
        operation: impl Future<Output = Result<T, ServerError>>,
    ) -> Result<T, ServerError> {
        let ClientAuthority::Shared(share) = self.authority else {
            return operation.await;
        };
        // Subscribe before the authoritative check so revocation cannot fall
        // between admission and event registration.
        let mut revocations = self.revocations.subscribe();
        let remaining = self
            .shares
            .lock()
            .await
            .remaining_lifetime(share.share_id)?;
        let expiry = tokio::time::sleep(remaining);
        tokio::pin!(expiry, operation);
        loop {
            tokio::select! {
                biased;
                _ = &mut expiry => return Err(ShareError::Unauthorized.into()),
                event = revocations.recv() => match event {
                    Ok(id) if id != share.share_id => {},
                    // Unknown missed events or a closed revocation source fail closed.
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_) | broadcast::error::RecvError::Closed) => {
                        return Err(ShareError::Unauthorized.into());
                    }
                },
                result = &mut operation => return result,
            }
        }
    }

    /// Revalidate after acquiring the writer, not merely before awaiting work.
    /// Called inside `run`, which also cancels stalled response I/O on revocation.
    pub(super) async fn write_response(
        &self,
        writer: &SharedServerWireWriter,
        response: &ServerResponse,
    ) -> Result<(), ServerError> {
        let mut writer = writer.lock().await;
        self.revalidate().await?;
        writer
            .send_json(FrameKind::Response, 0, response)
            .await
            .map_err(ultraplexr_protocol::ProtocolError::from)?;
        Ok(())
    }

    /// Used after writer admission by other guarded stream formats too.
    pub(super) async fn revalidate(&self) -> Result<(), ServerError> {
        if let ClientAuthority::Shared(share) = self.authority {
            self.shares
                .lock()
                .await
                .remaining_lifetime(share.share_id)?;
        }
        Ok(())
    }
}
