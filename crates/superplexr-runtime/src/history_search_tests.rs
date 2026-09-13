use super::*;
use std::{future::Future, pin::pin, sync::atomic::AtomicUsize, task::Wake};

#[derive(Default)]
struct Notification(AtomicUsize);
impl Wake for Notification {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn async_page_replaces_abandoned_waiter_and_wakes_on_send() {
    let (send, mut search) = channel(SearchCancellation::default());
    let old = Arc::new(Notification::default());
    let new = Arc::new(Notification::default());
    let old_waker = Waker::from(old.clone());
    let new_waker = Waker::from(new.clone());
    {
        let mut page = pin!(search.next_page_async());
        assert!(
            page.as_mut()
                .poll(&mut Context::from_waker(&old_waker))
                .is_pending()
        );
    }
    let mut page = pin!(search.next_page_async());
    assert!(
        page.as_mut()
            .poll(&mut Context::from_waker(&new_waker))
            .is_pending()
    );
    send.try_send(Ok(SearchBatch {
        matches: vec![],
        complete: true,
        rows_scanned: 0,
    }))
    .expect("send");
    assert_eq!(old.0.load(Ordering::SeqCst), 0);
    assert_eq!(new.0.load(Ordering::SeqCst), 1);
    assert!(
        matches!(page.as_mut().poll(&mut Context::from_waker(&new_waker)), Poll::Ready(Ok(batch)) if batch.complete)
    );
}

#[test]
fn sender_teardown_wakes_pending_page_and_reports_incomplete() {
    let (send, mut search) = channel(SearchCancellation::default());
    let notification = Arc::new(Notification::default());
    let waker = Waker::from(notification.clone());
    let mut page = pin!(search.next_page_async());
    assert!(
        page.as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    drop(send);
    assert_eq!(notification.0.load(Ordering::SeqCst), 1);
    assert!(matches!(
        page.as_mut().poll(&mut Context::from_waker(&waker)),
        Poll::Ready(Err(RuntimeError::SearchIncomplete))
    ));
}

#[test]
fn cancelled_search_refuses_buffered_pages_and_drop_cancels_token() {
    let (send, mut search) = channel(SearchCancellation::default());
    let token = search.cancellation();
    send.try_send(Ok(SearchBatch {
        matches: vec![],
        complete: true,
        rows_scanned: 0,
    }))
    .expect("send");
    token.cancel();
    let waker = Waker::noop();
    assert!(matches!(
        pin!(search.next_page_async()).poll(&mut Context::from_waker(waker)),
        Poll::Ready(Err(RuntimeError::SearchIncomplete))
    ));
    let (_send, search) = channel(SearchCancellation::default());
    let token = search.cancellation();
    assert!(!token.is_cancelled());
    drop(search);
    assert!(token.is_cancelled());
}

#[test]
fn racing_send_or_disconnect_never_strands_an_async_receiver() {
    struct Notify(mpsc::Sender<()>);
    impl Wake for Notify {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }
    for disconnect in [false, true] {
        for _ in 0..100 {
            let (sender, mut search) = channel(SearchCancellation::default());
            let (notify, notifications) = mpsc::channel();
            let waker = Waker::from(Arc::new(Notify(notify)));
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let producer_barrier = barrier.clone();
            let producer = std::thread::spawn(move || {
                producer_barrier.wait();
                if !disconnect {
                    sender
                        .try_send(Ok(SearchBatch {
                            matches: vec![],
                            complete: true,
                            rows_scanned: 0,
                        }))
                        .expect("publish page");
                }
                drop(sender);
            });
            barrier.wait();
            let mut future = pin!(search.next_page_async());
            let result = loop {
                if let Poll::Ready(result) = future.as_mut().poll(&mut Context::from_waker(&waker))
                {
                    break result;
                }
                // Never use a timeout as a reason to repoll: that could hide a
                // lost registration wakeup while a real executor hangs forever.
                notifications
                    .recv_timeout(Duration::from_secs(1))
                    .expect("producer must wake pending receiver");
            };
            if disconnect {
                assert!(matches!(result, Err(RuntimeError::SearchIncomplete)));
            } else {
                assert!(result.expect("delivered page").complete);
            }
            producer.join().expect("producer");
        }
    }
}
