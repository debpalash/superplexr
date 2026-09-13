use super::*;
use crate::terminal_input::{MAX_COMMANDS, MAX_PAYLOAD_BYTES, Queue};
use superplexr_client::TerminalInput;

fn paste(text: &str) -> TerminalInput {
    TerminalInput::Paste {
        bytes: text.as_bytes().to_vec(),
        confirmed: true,
    }
}
fn until(check: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !check() {
        assert!(
            std::time::Instant::now() < deadline,
            "input retirement deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct PausedDaemon(i32);
impl PausedDaemon {
    fn new(fixture: &RuntimeFixture) -> Self {
        let pid = i32::try_from(fixture.child.id()).unwrap();
        // SAFETY: positive PID belongs to this test's live isolated daemon.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
        Self(pid)
    }
}
impl Drop for PausedDaemon {
    fn drop(&mut self) {
        // SAFETY: the fixture outlives this guard; resume even on assertion panic.
        unsafe {
            libc::kill(self.0, libc::SIGCONT);
        }
    }
}

#[test]
fn bounded_input_pins_epoch_and_never_revives_after_control_reacquisition() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let pid = session.capture().unwrap().terminal.process_id;
    let old = session.claim_input(false).unwrap();
    old.send(paste("FIRST 界\n")).unwrap();
    wait_text(&session, "FIRST 界");
    let other = owner.terminal(session.id());
    let takeover = other.claim_input(true).unwrap();
    takeover.send(paste("OTHER\n")).unwrap();
    assert!(old.send(paste("STALE-AUTHORITY\n")).is_err());
    let fresh = session.claim_input(true).unwrap();
    assert!(!old.is_current());
    assert!(old.send(paste("MUST-NOT-REVIVE\n")).is_err());
    fresh.send(paste("FRESH\n")).unwrap();
    wait_text(&session, "FRESH");
    assert!(
        session
            .search("STALE-AUTHORITY", true, 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        session
            .search("MUST-NOT-REVIVE", true, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(session.capture().unwrap().terminal.process_id, pid);
}

#[test]
fn bounded_input_count_and_byte_overflow_discard_pending_work_and_require_explicit_arm() {
    for byte_limit in [false, true] {
        let fixture = RuntimeFixture::start();
        let owner = fixture.client();
        let session = terminal(&owner, &fixture);
        let (queue, reports) = Queue::new(session.id()).unwrap();
        queue.arm(session.claim_input(false).unwrap()).unwrap();
        let paused = PausedDaemon::new(&fixture);
        queue.send(TerminalInput::Focus(false)).unwrap();
        if byte_limit {
            queue
                .send(TerminalInput::Paste {
                    bytes: vec![b'x'; MAX_PAYLOAD_BYTES],
                    confirmed: true,
                })
                .unwrap();
        } else {
            for _ in 1..MAX_COMMANDS {
                queue.send(TerminalInput::Focus(false)).unwrap();
            }
        }
        assert!(queue.send(paste("NEVER-SEND\n")).is_err());
        assert!(!queue.is_armed());
        let failure = reports.borrow().clone().unwrap();
        assert!(failure.message.contains("limit reached"));
        assert!(queue.current_failure(&failure));
        drop(paused);
        queue.arm(session.claim_input(false).unwrap()).unwrap();
        assert!(
            !queue.current_failure(&failure),
            "an old report cannot poison explicit new Control"
        );
        queue.send(paste("AFTER-ARM\n")).unwrap();
        wait_text(&session, "AFTER-ARM");
        assert!(session.search("NEVER-SEND", true, 1).unwrap().is_empty());
        assert!(
            session.search("xxxxxxxx", true, 1).unwrap().is_empty(),
            "large queued paste was discarded"
        );
    }
}

#[test]
fn bounded_input_metadata_ignores_pre_claim_updates_and_never_rearms_retired_input() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let (queue, _) = Queue::new(session.id()).unwrap();
    queue.arm(session.claim_input(false).unwrap()).unwrap();
    let epoch = session.control_epoch().unwrap();
    assert!(queue.synchronize_control_owner(None, epoch - 1));
    assert!(queue.synchronize_control_owner(Some(session.surface_id()), epoch));
    queue.send(paste("STALE-METADATA-IGNORED\n")).unwrap();
    wait_text(&session, "STALE-METADATA-IGNORED");
    session.release_control().unwrap();
    let released = session.control_epoch().unwrap();
    assert!(!queue.synchronize_control_owner(None, released));
    assert!(!queue.synchronize_control_owner(Some(session.surface_id()), epoch));
    assert!(queue.send(paste("METADATA-MUST-NOT-REARM\n")).is_err());
    queue.arm(session.claim_input(false).unwrap()).unwrap();
    assert!(queue.synchronize_control_owner(None, released));
    queue.send(paste("EXPLICIT-AFTER-METADATA\n")).unwrap();
    wait_text(&session, "EXPLICIT-AFTER-METADATA");
    assert!(
        session
            .search("METADATA-MUST-NOT-REARM", true, 1)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn bounded_input_counts_reserved_capacity_even_for_empty_payloads() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let (queue, reports) = Queue::new(session.id()).unwrap();
    queue.arm(session.claim_input(false).unwrap()).unwrap();
    let bytes = Vec::with_capacity(MAX_PAYLOAD_BYTES + 1);
    assert!(bytes.is_empty());
    assert!(
        queue
            .send(TerminalInput::Paste {
                bytes,
                confirmed: true
            })
            .is_err()
    );
    assert!(!queue.is_armed());
    assert!(
        reports
            .borrow()
            .as_ref()
            .unwrap()
            .message
            .contains("limit reached")
    );
    queue.arm(session.claim_input(false).unwrap()).unwrap();
    queue.send(paste("CAPACITY-REARM\n")).unwrap();
    wait_text(&session, "CAPACITY-REARM");
}

#[test]
fn bounded_input_disconnect_does_not_move_old_lease_to_repaired_transport() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    session.release_control().unwrap();
    let (_, token) = owner
        .create_share(
            "input reconnect",
            ShareRole::Controller,
            vec![],
            vec![session.id()],
            60,
        )
        .unwrap();
    let relay = Arc::new(LoopbackRelay::new(fixture.root.join("s")));
    let client = ControlClient::connect_with_connector(relay.clone(), Some(token)).unwrap();
    let remote = client.terminal(session.id());
    let lease = remote.claim_input(false).unwrap();
    let (queue, reports) = Queue::new(session.id()).unwrap();
    queue.arm(lease.clone()).unwrap();
    let paused = PausedDaemon::new(&fixture);
    queue.send(paste("MAY-HAVE-BEEN-ADMITTED\n")).unwrap();
    queue.send(paste("QUEUED-MUST-NOT-REPLAY\n")).unwrap();
    relay.disconnect();
    until(|| !lease.is_current());
    until(|| reports.borrow().is_some());
    assert!(!queue.is_armed());
    drop(paused);
    relay.resume();
    // Observation repairs the transport. It must not restore old input authority.
    let _ = remote.snapshot();
    remote.snapshot().unwrap();
    assert!(queue.send(paste("NO-AUTO-ARM\n")).is_err());
    assert!(lease.send(paste("OLD-LEASE\n")).is_err());
    queue.arm(remote.claim_input(false).unwrap()).unwrap();
    queue.send(paste("RECONNECTED-EXPLICITLY\n")).unwrap();
    wait_text(&session, "RECONNECTED-EXPLICITLY");
    for marker in ["QUEUED-MUST-NOT-REPLAY", "NO-AUTO-ARM", "OLD-LEASE"] {
        assert!(
            session.search(marker, true, 1).unwrap().is_empty(),
            "{marker}"
        );
    }
}

#[test]
fn bounded_input_gate_and_drop_discard_waiting_commands_without_stopping_terminal() {
    for detach in [false, true] {
        let fixture = RuntimeFixture::start();
        let owner = fixture.client();
        let session = terminal(&owner, &fixture);
        let pid = session.capture().unwrap().terminal.process_id;
        let (queue, _) = Queue::new(session.id()).unwrap();
        let lease = session.claim_input(false).unwrap();
        queue.arm(lease.clone()).unwrap();
        let paused = PausedDaemon::new(&fixture);
        queue.send(TerminalInput::Focus(false)).unwrap();
        queue.send(paste("DISCARDED-ON-RETIRE\n")).unwrap();
        if detach {
            drop(queue);
        } else {
            queue.gate().retire("Read-only history");
        }
        assert!(!lease.is_current());
        drop(paused);
        session.paste(b"OWNER-ACTIVE\n".to_vec(), true).unwrap();
        wait_text(&session, "OWNER-ACTIVE");
        assert!(
            session
                .search("DISCARDED-ON-RETIRE", true, 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(session.capture().unwrap().terminal.process_id, pid);
    }
}

#[test]
fn bounded_input_respects_observer_scope_and_revoked_controller_share() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let (_, observer) = scoped(&owner, &fixture, session.id(), ShareRole::Observer);
    assert!(observer.terminal(session.id()).claim_input(false).is_err());
    session.release_control().unwrap();
    let (share, controller) = scoped(&owner, &fixture, session.id(), ShareRole::Controller);
    let remote = controller.terminal(session.id());
    let lease = remote.claim_input(false).unwrap();
    let (queue, reports) = Queue::new(session.id()).unwrap();
    queue.arm(lease).unwrap();
    queue.send(paste("AUTHORIZED\n")).unwrap();
    wait_text(&session, "AUTHORIZED");
    owner.revoke_share(share.share_id).unwrap();
    let _ = queue.send(paste("REVOKED-INPUT\n"));
    until(|| reports.borrow().is_some());
    assert!(!queue.is_armed());
    session.claim_control(true).unwrap();
    session
        .paste(b"OWNER-AFTER-REVOKE\n".to_vec(), true)
        .unwrap();
    wait_text(&session, "OWNER-AFTER-REVOKE");
    assert!(session.search("REVOKED-INPUT", true, 1).unwrap().is_empty());
}
