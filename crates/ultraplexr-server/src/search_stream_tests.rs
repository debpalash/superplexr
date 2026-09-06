use super::*;
use crate::{ShareRole, share_store::ShareStore};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, broadcast},
};
use ultraplexr_protocol::wire_v3::AsyncWireWriter;

struct Store {
    root: std::path::PathBuf,
    shares: Mutex<ShareStore>,
    authority: ClientAuthority,
}
impl Store {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("up-search-write-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).expect("root");
        let mut shares = ShareStore::open(root.join("shares.json")).expect("shares");
        let (share, _) = shares
            .create(
                "test".into(),
                ShareRole::Observer,
                vec![],
                vec![SessionId::new()],
                60,
            )
            .expect("share");
        Self {
            root,
            shares: Mutex::new(shares),
            authority: ClientAuthority::Shared(share),
        }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn cancelling_a_partial_search_frame_shuts_down_the_shared_socket() {
    let fixture = Store::new();
    let events = broadcast::channel(2).0;
    let access = share_request::Access::new(&fixture.authority, &fixture.shares, &events);
    let (server, mut peer) = tokio::net::UnixStream::pair().expect("pair");
    let server = server.into_std().expect("std socket");
    let socket = Arc::new(server.try_clone().expect("shutdown handle"));
    let (_, write) = tokio::net::UnixStream::from_std(server)
        .expect("async socket")
        .into_split();
    let wire = Arc::new(Mutex::new(AsyncWireWriter::new(write)));
    let writer = Writer {
        wire: wire.clone(),
        socket,
        stream_id: 1,
        access: &access,
    };
    let payload = "x".repeat(8 * 1024 * 1024);
    let mut send = Box::pin(writer.send(FrameKind::SearchPage, 1, &payload));
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            result = &mut send => panic!("large write should be backpressured: {result:?}"),
            result = peer.readable() => result.expect("partial bytes"),
        }
    })
    .await
    .expect("write starts");
    drop(send); // Simulates cancelling just this search task, not the connection.
    let mut partial = vec![];
    tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut partial))
        .await
        .expect("shutdown despite other writer owners")
        .expect("partial read");
    assert!(!partial.is_empty());
    assert!(partial.len() < payload.len());
    assert!(
        writer
            .send(FrameKind::Response, 0, &"must not follow partial frame")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn search_writer_rechecks_revocation_after_waiting_for_the_writer() {
    let fixture = Store::new();
    let events = broadcast::channel(2).0;
    let access = share_request::Access::new(&fixture.authority, &fixture.shares, &events);
    let (server, peer) = tokio::net::UnixStream::pair().expect("pair");
    let server = server.into_std().expect("std socket");
    let socket = Arc::new(server.try_clone().expect("shutdown handle"));
    let (_, write) = tokio::net::UnixStream::from_std(server)
        .expect("async socket")
        .into_split();
    let wire = Arc::new(Mutex::new(AsyncWireWriter::new(write)));
    let writer = Writer {
        wire: wire.clone(),
        socket,
        stream_id: 1,
        access: &access,
    };
    let held = wire.lock().await;
    let mut send = Box::pin(writer.send(FrameKind::SearchPage, 1, &"sensitive"));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(send.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    fixture
        .shares
        .lock()
        .await
        .revoke(fixture.authority.identity().expect("share ID"))
        .expect("durable revocation without event");
    drop(held);
    assert!(send.await.is_err());
    assert_eq!(
        peer.try_read(&mut [0])
            .expect_err("no bytes admitted")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}
