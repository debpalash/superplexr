use super::*;
use tokio_stream::StreamExt;
use ultraplexr_protocol::{
    SessionGroupChange, SessionGroupEvent, SessionGroupId, SessionGroupSpec,
};

fn until(check: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !check() {
        assert!(
            std::time::Instant::now() < deadline,
            "metadata lifecycle deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn oversized_mission_stops_retry_releases_control_and_allows_explicit_terminal_recovery() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let pid = session.capture().unwrap().terminal.process_id;
    session.release_control().unwrap();
    let mission_id = ultraplexr_core::MissionId::new();
    let actor = ultraplexr_core::Actor::human("oversize-fixture").unwrap();
    owner
        .create_mission(mission_id, "small initial snapshot", actor.clone())
        .unwrap();
    let (_, token) = owner
        .create_share(
            "oversize shared wire",
            ShareRole::Controller,
            vec![mission_id],
            vec![session.id()],
            60,
        )
        .unwrap();
    let source = ControlClient::connect_with_share(fixture.root.join("s"), token).unwrap();
    let source_session = source.terminal(session.id());
    let old = source_session.claim_input(false).unwrap();
    let receiver = source.subscribe_missions().unwrap();
    assert_eq!(receiver.recv().unwrap().id, mission_id);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (publish, mut desktop) = crate::terminal_feed::channel(session.id());
    let native = source_session
        .subscribe_events_with_status(move |event| publish.publish(event))
        .unwrap();
    let held = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), desktop.recv())
            .await
            .unwrap()
            .unwrap()
    });
    let gateway =
        ultraplexr_observer::Observer::with_control(source.clone(), HOST.into(), KEY.into())
            .unwrap();
    let (_, mut browser, mut buffer) =
        runtime.block_on(attach(&gateway.clone().router(), session.id()));
    owner
        .dispatch(
            mission_id,
            ultraplexr_core::Command::StartSession {
                session_id: session.id(),
                name: "m".repeat(9 * 1024 * 1024),
                started_by: actor,
            },
        )
        .unwrap();
    for _ in 0..3 {
        assert!(
            matches!(receiver.recv_delivery(), Err(ultraplexr_client::ClientError::ReceiveLimit { bytes, limit }) if bytes > limit && limit == 8 * 1024 * 1024)
        );
    }
    until(|| !old.is_current());
    let stopped = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), desktop.recv())
            .await
            .unwrap()
            .unwrap()
    });
    assert!(!held.is_current());
    assert!(stopped.rejected && stopped.continuity_lost && stopped.frame.is_none());
    assert_eq!(
        stopped.error,
        Some(ultraplexr_client::RECEIVE_LIMIT_MESSAGE)
    );
    runtime.block_on(async {
        loop {
            let (kind, message) = next_observer_event(&mut browser, &mut buffer).await;
            if kind == "ended" {
                assert_eq!(message, ultraplexr_client::RECEIVE_LIMIT_MESSAGE);
                assert!(
                    tokio::time::timeout(Duration::from_secs(3), browser.next())
                        .await
                        .expect("ended feed must close promptly")
                        .is_none(),
                    "ended feed must close"
                );
                break;
            }
            assert_ne!(
                kind, "reconnecting",
                "an intrinsic record limit must not reconnect"
            );
        }
    });
    gateway.shutdown();
    native.cancel();
    // Drain async view cleanup before checking for an accidental reconnect.
    runtime.shutdown_timeout(Duration::from_secs(3));
    assert_eq!(owner.runtime_diagnostics().unwrap().open_connections, 1);
    assert_eq!(
        session.capture().unwrap().terminal.controller_surface_id,
        None
    );
    let fresh = source_session.claim_input(false).unwrap();
    fresh
        .send(ultraplexr_client::TerminalInput::Paste {
            bytes: b"AFTER-OVERSIZED-RECORD\n".to_vec(),
            confirmed: true,
        })
        .unwrap();
    wait_text(&session, "AFTER-OVERSIZED-RECORD");
    assert_eq!(session.capture().unwrap().terminal.process_id, pid);
    assert!(!old.is_current());
    assert!(
        matches!(
            receiver.recv_delivery(),
            Err(ultraplexr_client::ClientError::ReceiveLimit { .. })
        ),
        "an old receiver cannot migrate onto explicit new Control"
    );
}

#[test]
fn metadata_slow_desktop_handoff_invalidates_stale_data_and_recovers_without_control() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let pid = session.capture().unwrap().terminal.process_id;
    let group_id = SessionGroupId::new();
    let group = owner
        .create_session_group(SessionGroupSpec {
            group_id,
            mission_id: None,
            name: "metadata-initial".into(),
            session_ids: vec![session.id()],
            position: 0,
            pinned: false,
            detached: false,
        })
        .unwrap();
    session.release_control().unwrap();
    let source = fixture.client();
    let old_input = source.terminal(session.id()).claim_input(false).unwrap();
    let mut feed =
        crate::metadata_feed::spawn(source.subscribe_session_groups(None).unwrap()).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let held = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(3), feed.recv())
            .await
            .unwrap()
            .unwrap()
    });
    let mut version = group.version;
    for n in 0..300 {
        version = owner
            .update_session_group(
                group_id,
                version,
                SessionGroupChange::Rename {
                    name: format!("metadata-{n:04}"),
                },
            )
            .unwrap()
            .version;
    }
    until(|| !old_input.is_current());
    until(|| owner.runtime_diagnostics().unwrap().open_connections == 1);
    assert!(
        held.into_current().is_none(),
        "decoded data held before overflow must be rejected"
    );
    assert_eq!(
        session.capture().unwrap().terminal.controller_surface_id,
        None
    );
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let delivery = feed.recv().await.unwrap();
                if let Some(SessionGroupEvent::GroupChanged { group }) = delivery.into_current()
                    && group.group_id == group_id
                    && group.version == version
                {
                    assert_eq!(group.name, "metadata-0299");
                    break;
                }
            }
        })
        .await
        .unwrap();
    });
    assert!(!old_input.is_current());
    assert_eq!(
        session.capture().unwrap().terminal.controller_surface_id,
        None
    );
    assert_eq!(session.capture().unwrap().terminal.process_id, pid);
    drop(feed);
}

#[test]
fn metadata_cancellation_wakes_idle_receive_without_closing_shared_control_connection() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let source = fixture.client();
    let receiver = source.subscribe_terminals().unwrap();
    let cancel = receiver.cancellation();
    let (send, done) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let rejected = receiver.recv().is_err();
        drop(receiver);
        send.send(rejected).unwrap();
    });
    until(|| {
        owner
            .runtime_diagnostics()
            .unwrap()
            .terminal_index_subscribers
            == 1
    });
    drop(cancel);
    assert!(done.recv_timeout(Duration::from_secs(2)).unwrap());
    worker.join().unwrap();
    until(|| {
        owner
            .runtime_diagnostics()
            .unwrap()
            .terminal_index_subscribers
            == 0
    });
    assert_eq!(owner.runtime_diagnostics().unwrap().open_connections, 2);
    source.list_terminals().unwrap();
}

#[test]
fn metadata_timeout_preserves_subscription_and_desktop_drop_unsubscribes_idle_feed() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let source = fixture.client();
    let receiver = source.subscribe_terminals().unwrap();
    assert!(
        matches!(receiver.recv_timeout(Duration::from_millis(20)), Err(ultraplexr_client::ClientError::Io(error)) if error.kind() == std::io::ErrorKind::TimedOut)
    );
    assert_eq!(
        owner
            .runtime_diagnostics()
            .unwrap()
            .terminal_index_subscribers,
        1
    );
    let feed = crate::metadata_feed::spawn(receiver).unwrap();
    drop(feed);
    until(|| {
        owner
            .runtime_diagnostics()
            .unwrap()
            .terminal_index_subscribers
            == 0
    });
    assert_eq!(owner.runtime_diagnostics().unwrap().open_connections, 2);
}

#[test]
fn metadata_revoked_share_does_not_reconnect_forever_or_deliver_held_items() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let (share, source) = scoped(&owner, &fixture, session.id(), ShareRole::Observer);
    let receiver = source.subscribe_terminals().unwrap();
    let held = receiver.recv_delivery().unwrap();
    owner.revoke_share(share.share_id).unwrap();
    until(|| !held.is_current());
    let (send, done) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let mut attempts = 0;
        loop {
            match receiver.recv_delivery() {
                Ok(item) => {
                    let _ = item.into_current();
                    attempts += 1;
                    assert!(attempts < 20);
                }
                Err(error) => {
                    send.send(error.to_string()).unwrap();
                    break;
                }
            }
        }
    });
    let error = done.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(error.contains("share"), "{error}");
    assert!(held.into_current().is_none());
    worker.join().unwrap();
    assert_eq!(
        session.capture().unwrap().terminal.process_id,
        owner.list_terminals().unwrap()[0].process_id
    );
}
