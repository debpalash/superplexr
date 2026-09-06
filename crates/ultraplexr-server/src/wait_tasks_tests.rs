use super::*;
use crate::{ResponseBody, share_request_tests::Fixture};
use tokio::{io::AsyncReadExt, sync::Mutex};
use ultraplexr_protocol::wire_v3::AsyncWireWriter;

#[tokio::test]
async fn concurrent_wait_cancelled_partial_reply_closes_before_reusing_writer() {
    let fixture = Fixture::new();
    let access = fixture.access();
    let (server, mut peer) = tokio::net::UnixStream::pair().unwrap();
    let server = server.into_std().unwrap();
    let socket = Arc::new(server.try_clone().unwrap());
    let shutdown_socket = Some(socket.clone());
    let (_, writer) = tokio::net::UnixStream::from_std(server)
        .unwrap()
        .into_split();
    let wire = Arc::new(Mutex::new(AsyncWireWriter::new(Box::new(writer) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>)));
    let response = ServerResponse::success(
        Uuid::new_v4(),
        ResponseBody::TerminalSelectionText {
            session_id: SessionId::new(),
            text: Some("x".repeat(8 * 1024 * 1024)),
        },
    );
    let mut sending = Box::pin(access.run(write_response(&access, &wire, &shutdown_socket, &response)));
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            result = &mut sending => panic!("expected socket backpressure: {result:?}"),
            result = peer.readable() => result.unwrap(),
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), sending)
            .await
            .is_err()
    );
    let mut partial = vec![];
    tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut partial))
        .await
        .unwrap()
        .unwrap();
    assert!(!partial.is_empty() && partial.len() < 8 * 1024 * 1024);
    assert!(
        write_response(
            &access,
            &wire,
            &Some(socket.clone()),
            &ServerResponse::success(Uuid::new_v4(), ResponseBody::Pong)
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn concurrent_wait_reply_rechecks_revocation_after_writer_admission() {
    use std::future::Future;
    let fixture = Fixture::new();
    let access = fixture.access();
    let (server, peer) = tokio::net::UnixStream::pair().unwrap();
    let server = server.into_std().unwrap();
    let socket = Arc::new(server.try_clone().unwrap());
    let shutdown_socket = Some(socket.clone());
    let (_, writer) = tokio::net::UnixStream::from_std(server)
        .unwrap()
        .into_split();
    let wire = Arc::new(Mutex::new(AsyncWireWriter::new(Box::new(writer) as Box<dyn tokio::io::AsyncWrite + Unpin + Send>)));
    let held = wire.lock().await;
    let response = ServerResponse::success(Uuid::new_v4(), ResponseBody::Pong);
    let mut sending = Box::pin(access.run(write_response(&access, &wire, &shutdown_socket, &response)));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(sending.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    fixture.revoke(false).await;
    drop(held);
    assert!(sending.await.is_err());
    assert_eq!(
        peer.try_read(&mut [0]).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
