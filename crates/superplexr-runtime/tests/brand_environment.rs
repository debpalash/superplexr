use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, Instant},
};
use superplexr_core::SessionId;
use superplexr_runtime::{SessionEvent, SessionHandle, SessionSpec};
use superplexr_terminal::GridSize;

#[test]
fn real_shell_receives_branded_identity_and_compatible_agent_aliases() {
    let id = SessionId::new();
    let root = std::env::temp_dir().join(format!("up-brand-{id}"));
    let script = r#"test "$TERM_PROGRAM" = superplexr &&
test "$ULTRAPLEXR_RUN_ID" = current &&
test "$ULTRAPLEXR_SESSION" = saved &&
test -z "${ULTRAPLEXR_AGENT_TOKEN_FD+x}" &&
test "$SUPERPLEXR_RUN_ID" = current && test "$TERMI9NE_RUN_ID" = current &&
test "$SUPERPLEXR_SESSION" = saved && test "$TERMI9NE_SESSION" = saved &&
test -z "${SUPERPLEXR_AGENT_TOKEN_FD+x}" && test -z "${TERMI9NE_AGENT_TOKEN_FD+x}" &&
printf 'brand-compat-ok\n'"#;
    let spec = SessionSpec {
        id,
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_owned(), script.to_owned()],
        cwd: std::env::current_dir().unwrap(),
        environment_delta: BTreeMap::from([
            ("SUPERPLEXR_RUN_ID".to_owned(), Some("current".to_owned())),
            ("TERMI9NE_RUN_ID".to_owned(), Some("stale".to_owned())),
            ("ULTRAPLEXR_SESSION".to_owned(), Some("saved".to_owned())),
            ("SUPERPLEXR_AGENT_TOKEN_FD".to_owned(), None),
            (
                "TERMI9NE_AGENT_TOKEN_FD".to_owned(),
                Some("stale".to_owned()),
            ),
        ]),
        grid: GridSize::new(80, 24).unwrap(),
    };
    let (handle, events) = SessionHandle::spawn_subscribed(spec, &root).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut observed = false;
    let mut exited = false;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match events.recv_timeout(remaining) {
            Ok(SessionEvent::Frame(frame)) => {
                observed |= frame
                    .rows
                    .iter()
                    .any(|row| row.text().contains("brand-compat-ok"));
            }
            Ok(SessionEvent::Exited { .. }) => {
                exited = true;
                break;
            }
            Ok(SessionEvent::Failed { message }) => panic!("PTY failed: {message}"),
            Ok(_) => {}
            Err(_) => break,
        }
    }
    if !exited {
        let _ = handle.kill();
    }
    drop(handle);
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        observed && exited,
        "real shell must verify both namespaces and exit"
    );
}
