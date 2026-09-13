use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use superplexr_client::{TerminalInput, TerminalStreamUpdate};
use superplexr_protocol::ServerEvent;

fn until(check: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !check() {
        assert!(
            std::time::Instant::now() < deadline,
            "receive overflow/recovery deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// Dropping on an assertion panic must also release the deliberately stalled
// callback, allowing the native subscription worker to retire normally.
struct ReleaseCallback(mpsc::SyncSender<()>);
impl Drop for ReleaseCallback {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

#[test]
fn native_receive_overflow_releases_control_and_resumes_same_pty_without_stale_input() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = owner
        .start_terminal(TerminalSessionSpec {
            session_id: SessionId::new(),
            mission_id: None,
            run_id: None,
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                concat!(
                    "stty -echo; printf 'ready\\n'; ",
                    "while [ ! -f go ]; do sleep 0.01; done; ",
                    "i=0; while [ $i -lt 400 ]; do printf 'receive-%04d\\n' $i; ",
                    "i=$((i + 1)); sleep 0.005; done; printf 'RX-DONE\\n'; exec cat"
                )
                .into(),
            ],
            cwd: fixture.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).unwrap(),
        })
        .unwrap();
    wait_text(&session, "ready");
    let pid = session.capture().unwrap().terminal.process_id;
    session.release_control().unwrap();
    let (_, guest) = scoped(&owner, &fixture, session.id(), ShareRole::Controller);
    let guest_session = guest.terminal(session.id());
    let old = guest_session.claim_input(false).unwrap();
    let (ready_send, ready) = mpsc::sync_channel(1);
    let (release_send, release) = mpsc::sync_channel(1);
    let unblock = ReleaseCallback(release_send);
    let frames = Arc::new(AtomicUsize::new(0));
    let reconnects = Arc::new(AtomicUsize::new(0));
    let worker_frames = frames.clone();
    let worker_reconnects = reconnects.clone();
    let subscription = guest_session
        .subscribe_events_with_status(move |event| {
            match event {
                TerminalStreamUpdate::Event(ServerEvent::TerminalFrame { .. }) => {
                    if worker_frames.fetch_add(1, Ordering::AcqRel) == 0 {
                        ready_send.send(()).unwrap();
                        let _ = release.recv_timeout(Duration::from_secs(15));
                    }
                }
                TerminalStreamUpdate::Reconnecting => {
                    worker_reconnects.fetch_add(1, Ordering::AcqRel);
                }
                _ => {}
            }
            true
        })
        .unwrap();
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    // The PTY produces independently: owner input here would steal Control and
    // invalidate the assertion that transport overflow released the guest.
    std::fs::write(fixture.root.join("go"), b"go").unwrap();
    until(|| !old.is_current());
    assert_eq!(
        frames.load(Ordering::Acquire),
        1,
        "callback remains stalled"
    );
    assert_eq!(reconnects.load(Ordering::Acquire), 0);
    until(|| {
        session
            .capture()
            .unwrap()
            .terminal
            .controller_surface_id
            .is_none()
    });
    assert!(
        old.send(TerminalInput::Paste {
            bytes: b"STALE-RX-INPUT\n".to_vec(),
            confirmed: true
        })
        .is_err()
    );
    assert_eq!(session.capture().unwrap().terminal.process_id, pid);
    drop(unblock);
    until(|| reconnects.load(Ordering::Acquire) > 0 && frames.load(Ordering::Acquire) >= 2);
    wait_text(&session, "RX-DONE");
    assert_eq!(
        session.capture().unwrap().terminal.controller_surface_id,
        None
    );
    assert!(!old.is_current());
    let fresh = guest_session.claim_input(false).unwrap();
    fresh
        .send(TerminalInput::Paste {
            bytes: b"FRESH-RX-INPUT\n".to_vec(),
            confirmed: true,
        })
        .unwrap();
    wait_text(&session, "FRESH-RX-INPUT");
    assert!(
        session
            .search("STALE-RX-INPUT", true, 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(session.capture().unwrap().terminal.process_id, pid);
    subscription.cancel();
}
