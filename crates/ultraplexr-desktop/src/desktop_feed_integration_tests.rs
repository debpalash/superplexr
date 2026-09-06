use super::*;
use crate::terminal_feed;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use ultraplexr_client::TerminalStreamUpdate;
use ultraplexr_protocol::ServerEvent;

fn until(check: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !check() {
        assert!(
            std::time::Instant::now() < deadline,
            "native subscription deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn desktop_feed_stalled_consumer_keeps_latest_native_frame_and_terminal_exit() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = terminal(&owner, &fixture);
    let id = terminal.id();
    let before = terminal.capture().unwrap().terminal;
    let (_, client) = scoped(&owner, &fixture, id, ShareRole::Observer);
    let (send, mut receive) = terminal_feed::channel(id);
    let seen = Arc::new(AtomicU64::new(0));
    let seen_worker = seen.clone();
    let subscription = client
        .terminal(id)
        .subscribe_events_with_status(move |event| {
            let sequence = match &event {
                TerminalStreamUpdate::Event(ServerEvent::TerminalFrame { frame, .. }) => {
                    frame.sequence
                }
                TerminalStreamUpdate::Event(ServerEvent::TerminalDelta { delta, .. }) => {
                    delta.sequence
                }
                _ => 0,
            };
            let keep = send.publish(event);
            seen_worker.fetch_max(sequence, Ordering::AcqRel);
            keep
        })
        .unwrap();
    until(|| seen.load(Ordering::Acquire) > 0);
    // The render-side receiver deliberately does not drain while real PTY
    // output and native subscription processing continue.
    for i in 0..200 {
        terminal
            .paste(format!("stream-{i:04} 界\n").into_bytes(), true)
            .unwrap();
    }
    wait_text(&terminal, "stream-0199");
    let live = terminal.snapshot().unwrap();
    until(|| seen.load(Ordering::Acquire) >= live.sequence);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let batch = tokio::time::timeout(Duration::from_secs(2), receive.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(batch.is_current());
        assert_eq!(batch.frame.unwrap().as_ref(), &live);
        assert!(!batch.continuity_lost);
        assert!(
            tokio::time::timeout(Duration::from_millis(80), receive.recv())
                .await
                .is_err(),
            "no frame FIFO remains to drain"
        );
    });
    let after = terminal.capture().unwrap().terminal;
    assert_eq!(before.process_id, after.process_id);
    assert_eq!(before.controller_surface_id, after.controller_surface_id);
    terminal.kill().unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let batch = receive.recv().await.expect("exit remains observable");
                if matches!(
                    batch.notices[3],
                    Some(ServerEvent::TerminalExited { .. } | ServerEvent::TerminalFailed { .. })
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();
    });
    subscription.cancel();
}

#[test]
fn desktop_feed_keeps_reconnect_and_revocation_when_ui_does_not_drain() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = terminal(&owner, &fixture);
    let id = terminal.id();
    let pid = terminal.capture().unwrap().terminal.process_id;
    let (share, token) = owner
        .create_share("desktop feed", ShareRole::Observer, vec![], vec![id], 60)
        .unwrap();
    let relay = Arc::new(LoopbackRelay::new(fixture.root.join("s")));
    let client = ControlClient::connect_with_connector(relay.clone(), Some(token)).unwrap();
    let (send, mut receive) = terminal_feed::channel(id);
    let seen = Arc::new(AtomicU64::new(0));
    let lost = Arc::new(AtomicBool::new(false));
    let denied = Arc::new(AtomicBool::new(false));
    let (worker_seen, worker_lost, worker_denied) = (seen.clone(), lost.clone(), denied.clone());
    let subscription = client
        .terminal(id)
        .subscribe_events_with_status(move |event| {
            let (sequence, disconnected, rejected) = match &event {
                TerminalStreamUpdate::Event(ServerEvent::TerminalFrame { frame, .. }) => {
                    (frame.sequence, false, false)
                }
                TerminalStreamUpdate::Event(ServerEvent::TerminalDelta { delta, .. }) => {
                    (delta.sequence, false, false)
                }
                TerminalStreamUpdate::Reconnecting => (0, true, false),
                TerminalStreamUpdate::Rejected => (0, false, true),
                _ => (0, false, false),
            };
            let keep = send.publish(event);
            worker_seen.fetch_max(sequence, Ordering::AcqRel);
            if disconnected {
                worker_lost.store(true, Ordering::Release);
            }
            if rejected {
                worker_denied.store(true, Ordering::Release);
            }
            keep
        })
        .unwrap();
    until(|| seen.load(Ordering::Acquire) > 0);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let stale = runtime.block_on(receive.recv()).unwrap();
    relay.disconnect();
    until(|| lost.load(Ordering::Acquire));
    assert!(
        !stale.is_current(),
        "already scheduled old UI frames are invalidated"
    );
    terminal
        .paste(b"AFTER-DISCONNECT\n".to_vec(), true)
        .unwrap();
    wait_text(&terminal, "AFTER-DISCONNECT");
    let live = terminal.snapshot().unwrap();
    relay.resume();
    until(|| seen.load(Ordering::Acquire) >= live.sequence);
    let resumed = runtime.block_on(receive.recv()).unwrap();
    assert!(resumed.continuity_lost);
    assert!(!resumed.rejected);
    assert_eq!(resumed.frame.as_deref(), Some(&live));
    owner.revoke_share(share.share_id).unwrap();
    until(|| denied.load(Ordering::Acquire));
    assert!(!resumed.is_current());
    let rejected = runtime.block_on(receive.recv()).unwrap();
    assert!(rejected.rejected);
    assert!(rejected.frame.is_none());
    assert!(rejected.notices.iter().all(Option::is_none));
    assert_eq!(terminal.capture().unwrap().terminal.process_id, pid);
    terminal
        .paste(b"OWNER-STILL-WORKS\n".to_vec(), true)
        .unwrap();
    wait_text(&terminal, "OWNER-STILL-WORKS");
    subscription.cancel();
}
