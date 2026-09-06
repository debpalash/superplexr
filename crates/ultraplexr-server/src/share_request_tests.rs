use super::{
    ClientAuthority, ResponseBody, ServerError, ServerResponse, ShareRole, forward_wait_events,
    share_request::Access,
    share_store::{ShareError, ShareStore},
};
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
    io::AsyncReadExt,
    net::UnixStream,
    sync::{Mutex, Notify, broadcast},
};
use ultraplexr_core::SessionId;
use ultraplexr_protocol::wire_v3::AsyncWireWriter;
use uuid::Uuid;

pub(super) struct Fixture {
    root: PathBuf,
    authority: ClientAuthority,
    shares: Mutex<ShareStore>,
    events: broadcast::Sender<Uuid>,
}
impl Fixture {
    pub(super) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("up-share-access-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("fixture root");
        let mut shares = ShareStore::open(root.join("shares.json")).expect("store");
        let (share, _) = shares
            .create(
                "fixture".into(),
                ShareRole::Observer,
                vec![],
                vec![SessionId::new()],
                60,
            )
            .expect("Share");
        Self {
            root,
            authority: ClientAuthority::Shared(share),
            shares: Mutex::new(shares),
            events: broadcast::channel(2).0,
        }
    }
    pub(super) fn access(&self) -> Access<'_> {
        Access::new(&self.authority, &self.shares, &self.events)
    }
    pub(super) async fn revoke(&self, notify: bool) {
        let id = self.authority.identity().expect("Shared");
        self.shares.lock().await.revoke(id).expect("revoke");
        if notify {
            let _ = self.events.send(id);
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn denied<T>(result: Result<T, ServerError>) {
    assert!(matches!(
        result,
        Err(ServerError::Share(ShareError::Unauthorized))
    ));
}

#[tokio::test]
async fn stale_identity_cannot_start_work_even_if_revocation_event_was_missed() {
    let fixture = Fixture::new();
    fixture.revoke(false).await;
    let access = fixture.access();
    denied(
        access
            .run(async {
                panic!("revoked operation was polled");
                #[allow(unreachable_code)]
                Ok(())
            })
            .await,
    );
}

#[tokio::test]
async fn lagged_revocation_source_cancels_instead_of_guessing_authority() {
    let fixture = Fixture::new();
    let access = fixture.access();
    let started = Notify::new();
    let operation = access.run(async {
        started.notify_one();
        std::future::pending::<Result<(), ServerError>>().await
    });
    let revoke = async {
        started.notified().await;
        // No await in this burst: the receiver must observe a lost interval.
        for _ in 0..8 {
            fixture.events.send(Uuid::new_v4()).expect("listener");
        }
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(operation, revoke)
    })
    .await
    .expect("bounded cancel");
    denied(result);
}

#[tokio::test]
async fn writer_queue_rechecks_durable_revocation_without_a_notification() {
    let fixture = Fixture::new();
    let access = fixture.access();
    let (server, peer) = UnixStream::pair().expect("real wire");
    let (_read, write) = server.into_split();
    let writer = Arc::new(Mutex::new(AsyncWireWriter::new(Box::new(write) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>)));
    let held = writer.lock().await;
    let queued = Notify::new();
    let response = ServerResponse::success(
        Uuid::new_v4(),
        ResponseBody::TerminalSelectionText {
            session_id: SessionId::new(),
            text: Some("must not leave writer queue".into()),
        },
    );
    let operation = access.run(async {
        queued.notify_one();
        access.write_response(&writer, &response).await
    });
    let revoke = async {
        queued.notified().await;
        fixture.revoke(false).await;
        drop(held);
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(operation, revoke)
    })
    .await
    .expect("writer retires");
    denied(result);
    assert_eq!(
        peer.try_read(&mut [0; 1])
            .expect_err("no payload admitted")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[tokio::test]
async fn revocation_interrupts_a_partially_written_response() {
    let fixture = Fixture::new();
    let access = fixture.access();
    let (server, mut peer) = UnixStream::pair().expect("real wire");
    let (_read, write) = server.into_split();
    let writer = Arc::new(Mutex::new(AsyncWireWriter::new(Box::new(write) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>)));
    // Larger than normal Unix socket buffers; no peer drain until revocation.
    let response = ServerResponse::success(
        Uuid::new_v4(),
        ResponseBody::TerminalSelectionText {
            session_id: SessionId::new(),
            text: Some("x".repeat(8 * 1024 * 1024)),
        },
    );
    let operation = access.run(access.write_response(&writer, &response));
    let revoke = async {
        peer.readable().await.expect("response started");
        fixture.revoke(true).await;
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(operation, revoke)
    })
    .await
    .expect("blocked write cancels");
    denied(result);
    // As in handle_connection, never reuse a writer after interrupted framing.
    drop(writer);
    let mut partial = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut partial))
        .await
        .expect("closed writer")
        .expect("partial wire");
    assert!(!partial.is_empty());
    assert!(
        partial.len() < 8 * 1024 * 1024,
        "not a complete sensitive response"
    );
}

#[test]
fn cancelled_wait_forwarder_exits_even_when_its_source_stays_silent() {
    let (source, events) = std::sync::mpsc::channel();
    let (send, mut receive) = tokio::sync::mpsc::channel(1);
    let worker = std::thread::spawn(move || forward_wait_events(events, send));
    source
        .send(super::SessionEvent::ForegroundProcessChanged { process_id: 1 })
        .expect("live source");
    assert!(matches!(
        receive.blocking_recv(),
        Some(super::SessionEvent::ForegroundProcessChanged { process_id: 1 })
    ));
    // Exercise cancellation of an already running, now silent forwarder,
    // rather than only a consumer that disappeared before the worker started.
    std::thread::sleep(Duration::from_millis(50));
    assert!(!worker.is_finished());
    drop(receive);
    let end = std::time::Instant::now() + Duration::from_secs(1);
    while !worker.is_finished() {
        assert!(
            std::time::Instant::now() < end,
            "cancelled forwarding thread leaked"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    worker.join().expect("forwarder exited");
    assert!(
        source
            .send(super::SessionEvent::ForegroundProcessChanged { process_id: 2 })
            .is_err()
    );
}
