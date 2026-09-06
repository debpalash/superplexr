use super::*;
use tokio::io::AsyncReadExt;

fn pair() -> (SubscriptionWriter, tokio::net::UnixStream) {
    let (server, peer) = tokio::net::UnixStream::pair().unwrap();
    let server = server.into_std().unwrap();
    let shutdown = Arc::new(server.try_clone().unwrap());
    let (_, writer) = tokio::net::UnixStream::from_std(server)
        .unwrap()
        .into_split();
    (
        SubscriptionWriter {
            wire: Arc::new(Mutex::new(AsyncWireWriter::new(writer))),
            shutdown,
            stream_id: 1,
        },
        peer,
    )
}

#[tokio::test]
async fn native_receive_abort_partial_subscription_frame_closes_before_writer_reuse() {
    let (writer, mut peer) = pair();
    let worker = writer.clone();
    let sending = tokio::spawn(async move { worker.event(&"x".repeat(8 * 1024 * 1024)).await });
    tokio::time::timeout(Duration::from_secs(2), peer.readable())
        .await
        .unwrap()
        .unwrap();
    assert!(!sending.is_finished(), "real socket must be backpressured");
    sending.abort();
    assert!(sending.await.unwrap_err().is_cancelled());
    let mut partial = vec![];
    tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut partial))
        .await
        .unwrap()
        .unwrap();
    assert!(!partial.is_empty() && partial.len() < 8 * 1024 * 1024);
    assert!(
        writer
            .response(&ServerResponse::success(
                uuid::Uuid::new_v4(),
                ResponseBody::Pong
            ))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn native_receive_abort_before_writer_admission_preserves_healthy_connection() {
    use std::future::Future;
    let (writer, mut peer) = pair();
    let held = writer.wire.lock().await;
    let mut sending = Box::pin(writer.event(&42));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(sending.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(sending);
    drop(held);
    writer.event(&43).await.unwrap();
    let mut bytes = [0; 32];
    let count = tokio::time::timeout(Duration::from_secs(1), peer.read(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(count > 0);
    writer.event(&44).await.unwrap();
}
