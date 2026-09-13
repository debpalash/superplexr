//! Bounded, connection-owned terminal waits. Mutating RPCs remain on the
//! ordered connection loop; these observations may finish out of request order.
use super::{
    AppState, ClientAuthority, ServerError, ServerResponse, SharedServerWireWriter, share_request,
    terminal_wait,
};
use std::{net::Shutdown, os::unix::net::UnixStream, sync::Arc, time::Duration};
use tokio::{sync::Semaphore, task::JoinSet};
use superplexr_core::SessionId;
use superplexr_protocol::{ProtocolError, TerminalWaitCondition, wire_v3::FrameKind};
use uuid::Uuid;

const MAX_CONNECTION_WAITS: usize = 8;
const MAX_PROCESS_WAITS: usize = 32;
static SLOTS: Semaphore = Semaphore::const_new(MAX_PROCESS_WAITS);

// Shared with the native event forwarder. Cancelling the async consumer does
// not release admission until that forwarder has actually stopped too.
pub(super) type Permit = Arc<tokio::sync::SemaphorePermit<'static>>;

pub(super) struct Spec {
    pub request_id: Uuid,
    pub session_id: SessionId,
    pub condition: TerminalWaitCondition,
    pub timeout_millis: u64,
}

pub(super) struct Jobs {
    tasks: JoinSet<()>,
    socket: Option<Arc<UnixStream>>,
}
impl Jobs {
    pub(super) fn new(socket: Option<Arc<UnixStream>>) -> Self {
        Self {
            tasks: JoinSet::new(),
            socket,
        }
    }
    pub(super) fn start(
        &mut self,
        spec: Spec,
        authority: ClientAuthority,
        state: Arc<AppState>,
        wire: SharedServerWireWriter,
    ) -> bool {
        while self.tasks.try_join_next().is_some() {}
        if self.tasks.len() >= MAX_CONNECTION_WAITS {
            return false;
        }
        let Ok(permit) = SLOTS.try_acquire() else {
            return false;
        };
        let permit = Arc::new(permit);
        let socket = self.socket.clone();
        // Include task scheduling and response writer backpressure, not only
        // the condition's timeout. A stalled response retires the connection.
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(spec.timeout_millis)
            + Duration::from_secs(2);
        self.tasks.spawn(async move {
            let mut failure = CloseOnDrop(socket.clone());
            let access =
                share_request::Access::new(&authority, &state.shares, &state.share_revocations);
            let operation = access.run(async {
                let response = match terminal_wait(
                    &state,
                    spec.session_id,
                    spec.condition,
                    spec.timeout_millis,
                    Some(permit.clone()),
                )
                .await
                {
                    Ok(body) => ServerResponse::success(spec.request_id, body),
                    Err(error) => {
                        ServerResponse::error(spec.request_id, "request_failed", error.to_string())
                    }
                };
                // A completed condition does not retain a blocked response for
                // the remainder of a potentially hour-long wait deadline.
                tokio::time::timeout(
                    Duration::from_secs(2),
                    write_response(&access, &wire, &socket, &response),
                )
                .await
                .map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "terminal wait response deadline",
                    )
                })?
            });
            if matches!(
                tokio::time::timeout_at(deadline, operation).await,
                Ok(Ok(()))
            ) {
                failure.0 = None;
            }
            // `permit` remains alive through response completion. Any forwarder
            // clone owns its share until the OS thread actually exits.
            drop(permit);
        });
        true
    }
}
impl Drop for Jobs {
    fn drop(&mut self) {
        // Close before aborting children: no writer owner can keep a detached
        // peer alive or append bytes after interrupted response framing.
        if let Some(socket) = &self.socket {
            let _ = socket.shutdown(Shutdown::Both);
        }
        self.tasks.abort_all();
    }
}

struct CloseOnDrop(Option<Arc<UnixStream>>);
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if let Some(socket) = &self.0 {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

async fn write_response(
    access: &share_request::Access<'_>,
    wire: &SharedServerWireWriter,
    socket: &Option<Arc<UnixStream>>,
    response: &ServerResponse,
) -> Result<(), ServerError> {
    let mut writer = wire.lock().await;
    access.revalidate().await?;
    // Declared after the writer guard: cancellation shuts down the socket
    // BEFORE releasing its mutex to a competing response/subscription writer.
    let mut partial = CloseOnDrop(socket.clone());
    writer
        .send_json(FrameKind::Response, 0, response)
        .await
        .map_err(ProtocolError::from)?;
    partial.0 = None;
    Ok(())
}

#[cfg(test)]
#[path = "wait_tasks_tests.rs"]
mod tests;
