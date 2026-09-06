use super::*;
use std::{collections::BTreeMap, future::Future};
use ultraplexr_terminal::GridSize;

#[tokio::test]
async fn exit_wait_awaits_the_authoritative_projection_after_native_exit() {
    let id = SessionId::new();
    let root = std::env::temp_dir().join(format!("up-exit-projection-{id}"));
    let (handle, events) = SessionHandle::spawn_subscribed(
        SessionSpec {
            id,
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "read -r done".into()],
            cwd: std::env::current_dir().expect("cwd"),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).expect("grid"),
        },
        &root,
    )
    .expect("real PTY");
    struct Cleanup(SessionHandle, PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let _cleanup = Cleanup(handle.clone(), root);
    let frame = handle.snapshot().expect("initial frame");
    let record = TerminalRecord {
        handle: Some(handle.clone()),
        viewers: Arc::new(std::sync::Mutex::new(Vec::new())),
        projection: Arc::new(RwLock::new(TerminalProjection {
            summary: TerminalSessionSummary {
                session_id: id,
                mission_id: None,
                run_id: None,
                process_id: handle.process_id(),
                foreground_process: None,
                display_title: None,
                display_directory: None,
                tty_name: None,
                status: TerminalSessionStatus::Running,
                archived: false,
                latest_sequence: frame.sequence,
                controller_client_id: None,
                controller_surface_id: None,
                controller_share_id: None,
                control_epoch: 0,
                control_offer: None,
                control_requests: Vec::new(),
                cwd: None,
            },
            frame: Some(frame),
            final_event: None,
        })),
    };
    let updates = broadcast::channel(2).0;
    let mut wait = Box::pin(terminal_wait_from_record(
        record.clone(),
        &updates,
        id,
        TerminalWaitCondition::Exit,
        Instant::now(),
        Duration::from_secs(2),
        None,
    ));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(wait.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    handle.kill().expect("native exit");
    tokio::task::spawn_blocking(move || {
        loop {
            let event = events
                .recv_timeout(Duration::from_secs(1))
                .expect("native exit delivered");
            if is_terminal_event(&event) {
                break;
            }
        }
    })
    .await
    .expect("native event observer");
    // Deliberately delay only projection publication, as the real observer
    // thread can be scheduled after the wait's native event forwarder.
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut wait)
            .await
            .is_err(),
        "native exit must not reject the wait while its authoritative summary is still running"
    );
    {
        let mut projection = record.projection.write().expect("projection");
        projection.summary.status = TerminalSessionStatus::Exited;
        let _ = updates.send(projection.summary.clone());
    }
    let result = wait.await.expect("published exit satisfies wait");
    assert!(
        matches!(result, ResponseBody::TerminalWaitSatisfied { capture, .. } if capture.terminal.status == TerminalSessionStatus::Exited)
    );
}
