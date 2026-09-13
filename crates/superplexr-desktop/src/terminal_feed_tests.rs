use super::*;

#[test]
fn oversized_terminal_record_clears_stale_frame_and_reports_resource_limit() {
    let id = SessionId::new();
    let (send, mut receive) = channel(id);
    assert!(!send.publish(TerminalStreamUpdate::ReceiveLimited));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let batch = runtime.block_on(receive.recv()).unwrap();
    assert!(batch.rejected && batch.continuity_lost);
    assert!(batch.frame.is_none());
    assert_eq!(batch.error, Some(superplexr_client::RECEIVE_LIMIT_MESSAGE));
    assert!(!send.publish(TerminalStreamUpdate::Reconnecting));
}
use superplexr_protocol::FrameDelta;
use superplexr_terminal::{GridSize, PasteConfirmation, PasteRisk, TerminalAction, TerminalModel};

fn initial() -> FullFrame {
    TerminalModel::new(GridSize::new(20, 4).unwrap())
        .unwrap()
        .frame()
        .unwrap()
}
fn frame(id: SessionId, frame: FullFrame) -> TerminalStreamUpdate {
    TerminalStreamUpdate::Event(ServerEvent::TerminalFrame {
        session_id: id,
        frame: Box::new(frame),
    })
}

#[test]
fn desktop_feed_applies_all_deltas_but_retains_one_wakeup_and_latest_frame() {
    let id = SessionId::new();
    let (send, mut receive) = channel(id);
    let mut model = TerminalModel::new(GridSize::new(20, 4).unwrap()).unwrap();
    let mut previous = model.frame().unwrap();
    assert!(send.publish(frame(id, previous.clone())));
    // A UI that does not drain during thousands of events cannot build a FIFO.
    for i in 0..2000 {
        model
            .advance(TerminalAction::Output(format!("\r\nrow-{i} 界").as_bytes()))
            .unwrap();
        let next = model.frame().unwrap();
        let delta = FrameDelta::between(&previous, &next).unwrap();
        assert!(
            send.publish(TerminalStreamUpdate::Event(ServerEvent::TerminalDelta {
                session_id: id,
                delta: Box::new(delta)
            }))
        );
        previous = next;
    }
    assert_eq!(receive.wake.len(), 1);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let batch = receive.recv().await.unwrap();
        assert_eq!(batch.frame.as_deref(), Some(&previous));
        assert!(batch.notices.iter().all(Option::is_none));
        assert!(batch.is_current());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), receive.recv())
                .await
                .is_err()
        );
    });
    drop(receive);
    assert!(!send.publish(frame(id, previous)));
}

#[test]
fn desktop_feed_reconnect_is_sticky_and_revocation_invalidates_already_taken_frames() {
    let id = SessionId::new();
    let (send, mut receive) = channel(id);
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        assert!(send.publish(frame(id, initial())));
        let stale = receive.recv().await.unwrap();
        assert!(send.publish(TerminalStreamUpdate::Reconnecting));
        assert!(send.publish(frame(id, initial())));
        assert!(!stale.is_current());
        let resumed = receive.recv().await.unwrap();
        assert!(
            resumed.continuity_lost,
            "a fast full snapshot cannot erase the disconnect"
        );
        assert!(!resumed.rejected);
        assert!(resumed.frame.is_some());
        assert!(!send.publish(TerminalStreamUpdate::Rejected));
        assert!(!resumed.is_current());
        assert!(!send.publish(frame(id, initial())));
        let rejected = receive.recv().await.unwrap();
        assert!(rejected.rejected);
        assert!(rejected.frame.is_none());
        assert!(rejected.notices.iter().all(Option::is_none));
    });
}

#[test]
fn desktop_feed_coalesces_notices_without_losing_exit_or_reusing_paste() {
    let id = SessionId::new();
    let (send, mut receive) = channel(id);
    for i in 0..1000 {
        assert!(
            send.publish(TerminalStreamUpdate::Event(ServerEvent::TerminalBell {
                session_id: id,
                count: 1
            }))
        );
        assert!(send.publish(TerminalStreamUpdate::Event(
            ServerEvent::PasteConfirmation {
                session_id: id,
                confirmation: PasteConfirmation {
                    bytes: format!("command-{i}\n").into_bytes(),
                    risk: PasteRisk::MultilineOrEscape
                }
            }
        )));
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let batch = receive.recv().await.unwrap();
        assert!(matches!(batch.notices[0], Some(ServerEvent::TerminalBell { count:1000, .. })));
        assert!(matches!(&batch.notices[1], Some(ServerEvent::PasteConfirmation { confirmation, .. }) if confirmation.bytes == b"command-999\n"));
        assert!(send.publish(TerminalStreamUpdate::Event(ServerEvent::TerminalTerminationEscalationRequired { session_id:id })));
        assert!(!send.publish(TerminalStreamUpdate::Event(ServerEvent::TerminalExited { session_id:id, code:0, signal:None, success:true })));
        let batch = receive.recv().await.unwrap();
        assert!(batch.notices[..3].iter().all(Option::is_none));
        assert!(matches!(batch.notices[3], Some(ServerEvent::TerminalExited { success:true, .. })));
    });
}

#[test]
fn desktop_feed_rejects_wrong_identity_and_invalid_delta_without_painting_partial_state() {
    for wrong_identity in [false, true] {
        let id = SessionId::new();
        let (send, mut receive) = channel(id);
        assert!(send.publish(frame(id, initial())));
        if wrong_identity {
            assert!(!send.publish(frame(SessionId::new(), initial())));
        } else {
            let base = initial();
            let mut next = base.clone();
            next.sequence += 1;
            let mut delta = FrameDelta::between(&base, &next).unwrap();
            delta.base_sequence += 42;
            assert!(
                !send.publish(TerminalStreamUpdate::Event(ServerEvent::TerminalDelta {
                    session_id: id,
                    delta: Box::new(delta)
                }))
            );
        }
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let batch = receive.recv().await.unwrap();
            assert!(batch.rejected);
            assert!(batch.error.is_some());
            assert!(batch.frame.is_none());
        });
    }
}
