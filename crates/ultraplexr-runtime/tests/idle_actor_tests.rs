use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};
use ultraplexr_core::SessionId;
use ultraplexr_runtime::{SessionEvent, SessionHandle, SessionSpec};
use ultraplexr_terminal::GridSize;

struct Fixture {
    handle: SessionHandle,
    events: Receiver<SessionEvent>,
    root: PathBuf,
}
impl Fixture {
    fn new(script: &str) -> Self {
        let id = SessionId::new();
        let root = std::env::temp_dir().join(format!("up-idle-{id}"));
        let spec = SessionSpec {
            id,
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            cwd: std::env::current_dir().expect("cwd"),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).expect("grid"),
        };
        let (handle, events) = SessionHandle::spawn_subscribed(spec, &root).expect("real PTY");
        Self {
            handle,
            events,
            root,
        }
    }
    fn until(&self, timeout: Duration, predicate: impl Fn(&SessionEvent) -> bool) {
        let end = Instant::now() + timeout;
        loop {
            let event = self
                .events
                .recv_timeout(end.saturating_duration_since(Instant::now()))
                .expect("expected actor event before deadline");
            assert!(!matches!(&event, SessionEvent::Failed { .. }), "{event:?}");
            if predicate(&event) {
                return;
            }
        }
    }
    fn text(&self, needle: &str) {
        self.until(Duration::from_secs(3), |event| matches!(event, SessionEvent::Frame(frame) if frame.rows.iter().any(|row| row.text().contains(needle))));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.handle.kill();
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            match self.events.recv_timeout(Duration::from_millis(50)) {
                Ok(SessionEvent::Exited(_)) | Err(RecvTimeoutError::Disconnected) => break,
                _ => {}
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn quiet_actor_wakes_for_input_resize_reattach_and_final_output_without_poll_delay() {
    let mut fixture = Fixture::new(
        "stty -echo; printf 'READY\\n'; while IFS= read -r line; do case \"$line\" in size) stty size;; quit) printf 'FINAL\\n'; exit 7;; *) printf 'ack:%s\\n' \"$line\";; esac; done",
    );
    fixture.text("READY");
    let id = fixture.handle.id();
    let pid = fixture.handle.process_id();
    let frame = fixture.handle.snapshot().expect("initial snapshot");
    // Wait through idle-delay/maintenance without manufacturing terminal output.
    let end = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < end {
        match fixture
            .events
            .recv_timeout(end.saturating_duration_since(Instant::now()))
        {
            Ok(SessionEvent::Frame(next)) => assert_eq!(
                next, frame,
                "idle maintenance must not publish changed frames"
            ),
            Ok(SessionEvent::ForegroundProcessChanged { .. }) | Err(RecvTimeoutError::Timeout) => {}
            other => panic!("unexpected idle event: {other:?}"),
        }
    }
    let started = Instant::now();
    for i in 0..10 {
        let marker = format!("echo-{i}");
        fixture
            .handle
            .paste(format!("{marker}\n").into_bytes(), true)
            .expect("input wakes actor");
        fixture.text(&format!("ack:{marker}"));
        fixture.handle.snapshot().expect("snapshot wakes actor");
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "commands must wake recv_timeout, not wait for periodic polling"
    );
    fixture.events = fixture.handle.subscribe().expect("reattach");
    assert_eq!(fixture.handle.id(), id);
    assert_eq!(fixture.handle.process_id(), pid);
    fixture
        .handle
        .resize(GridSize::new(100, 30).expect("resize"), 8, 16)
        .expect("resize wakes actor");
    fixture
        .handle
        .paste(b"size\n".to_vec(), true)
        .expect("query kernel grid");
    fixture.text("30 100");
    fixture
        .handle
        .paste(b"quit\n".to_vec(), true)
        .expect("finish");
    fixture.text("FINAL");
    fixture.until(
        Duration::from_secs(2),
        |event| matches!(event, SessionEvent::Exited(exit) if exit.code == 7),
    );
}

#[test]
fn idle_termination_keeps_hangup_term_and_force_escalation_deadlines() {
    let fixture =
        Fixture::new("trap '' HUP TERM; printf 'READY\\n'; while :; do read -r line; done");
    fixture.text("READY");
    let started = Instant::now();
    fixture.handle.terminate().expect("hangup");
    fixture.until(Duration::from_secs(7), |event| {
        matches!(event, SessionEvent::TerminationEscalationRequired)
    });
    assert!(
        started.elapsed() >= Duration::from_secs(5),
        "must not shorten the two grace periods"
    );
    fixture.handle.kill().expect("explicit force");
    fixture.until(Duration::from_secs(2), |event| {
        matches!(event, SessionEvent::Exited(_))
    });
}

fn history_fixture() -> Fixture {
    let fixture = Fixture::new(
        "stty -echo; awk 'BEGIN { for (i=0;i<20000;i++) printf \"ROW-%06d retained history\\n\",i }'; printf 'READY\\n'; while IFS= read -r line; do printf 'ack:%s\\n' \"$line\"; done",
    );
    fixture.text("READY");
    fixture
}

#[test]
fn slow_search_consumers_do_not_block_input_snapshots_or_cancellation() {
    let fixture = history_fixture();
    let search = fixture
        .handle
        .start_search("ROW".into(), true, 1000)
        .expect("stream search");
    let page = search
        .next_page(Duration::from_secs(1))
        .expect("first page")
        .expect("ready page");
    assert!(!page.complete);
    assert!(!page.matches.is_empty());
    assert!(page.matches.len() <= 64);
    // Stop draining: the four-page queue fills while actor input remains live.
    let start = Instant::now();
    for n in 0..8 {
        fixture
            .handle
            .paste(format!("during-search-{n}\n").into_bytes(), true)
            .expect("input");
        fixture.text(&format!("ack:during-search-{n}"));
        fixture
            .handle
            .snapshot()
            .expect("snapshot during pending search");
    }
    assert!(start.elapsed() < Duration::from_secs(2));
    let busy = fixture
        .handle
        .start_search("ROW".into(), true, 1)
        .expect("bounded admission");
    assert!(
        busy.next_page(Duration::from_secs(1)).is_err(),
        "one active job per Session"
    );
    drop(search);
    fixture
        .handle
        .snapshot()
        .expect("cancellation ordering barrier");
    let matches = fixture
        .handle
        .search("ROW-019999".into(), true, 1)
        .expect("new search after cancel");
    assert_eq!(matches.len(), 1);
    fixture
        .handle
        .paste(b"after-search\n".to_vec(), true)
        .expect("input after complete");
    fixture.text("ack:after-search");
}

#[test]
fn abandoned_result_queues_expire_without_false_completion_or_session_loss() {
    let fixture = history_fixture();
    let search = fixture
        .handle
        .start_search("ROW".into(), true, 1000)
        .expect("stream search");
    fixture.handle.snapshot().expect("accepted work");
    std::thread::sleep(Duration::from_millis(5200));
    let mut pages = 0;
    loop {
        match search.next_page(Duration::from_secs(1)) {
            Ok(Some(page)) => {
                assert!(!page.complete);
                pages += 1;
            }
            Err(ultraplexr_runtime::RuntimeError::SearchIncomplete) => break,
            Err(error) => panic!("a full expired queue is not a stopped actor: {error}"),
            Ok(None) => panic!("expired job must retire its result stream"),
        }
    }
    assert!(pages <= 4);
    fixture.handle.snapshot().expect("Session remains live");
    assert_eq!(
        fixture
            .handle
            .search("ROW-000000".into(), true, 1)
            .expect("search after expiry")
            .len(),
        1
    );
}

#[test]
fn terminal_exit_is_not_delayed_by_a_paused_search() {
    let fixture = history_fixture();
    let search = fixture
        .handle
        .start_search("ROW".into(), true, 1000)
        .expect("stream search");
    assert!(
        search
            .next_page(Duration::from_secs(1))
            .expect("first page")
            .is_some()
    );
    fixture.handle.kill().expect("kill during search");
    fixture.until(Duration::from_secs(2), |event| {
        matches!(event, SessionEvent::Exited(_))
    });
}

// Wake-driven executor for the runtime's executor-independent page API. A
// timeout fails instead of repolling, so missed notifications cannot pass.
fn await_page(search: &mut ultraplexr_runtime::SessionSearch) -> ultraplexr_terminal::SearchBatch {
    use std::{
        future::Future,
        pin::pin,
        sync::Arc,
        task::{Context, Wake, Waker},
    };
    struct Notify(std::sync::mpsc::Sender<()>);
    impl Wake for Notify {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }
    let (send, receive) = std::sync::mpsc::channel();
    let waker = Waker::from(Arc::new(Notify(send)));
    let mut future = pin!(search.next_page_async());
    loop {
        if let std::task::Poll::Ready(result) =
            future.as_mut().poll(&mut Context::from_waker(&waker))
        {
            return result.expect("native page");
        }
        receive
            .recv_timeout(Duration::from_secs(2))
            .expect("page must notify, not require timer polling");
    }
}

#[test]
fn async_native_pages_cancel_and_readmit_without_replacing_the_pty() {
    let fixture = history_fixture();
    let id = fixture.handle.id();
    let pid = fixture.handle.process_id();
    let mut search = fixture
        .handle
        .start_search("ROW".into(), true, 1000)
        .expect("search");
    assert!(!await_page(&mut search).complete);
    let cancelled = search.cancellation();
    cancelled.cancel();
    fixture.handle.snapshot().expect("actor barrier");
    // Keep the old receiver alive: token cancellation, not receiver teardown,
    // must release native admission and tracked references.
    let mut replacement = fixture
        .handle
        .start_search("ROW".into(), true, 1000)
        .expect("replacement");
    let mut count = 0;
    loop {
        let page = await_page(&mut replacement);
        assert!(page.matches.len() <= 64);
        count += page.matches.len();
        if page.complete {
            break;
        }
    }
    assert_eq!(count, 1000);
    assert_eq!(fixture.handle.id(), id);
    assert_eq!(fixture.handle.process_id(), pid);
    fixture
        .handle
        .paste(b"after-async-search\n".to_vec(), true)
        .expect("input");
    fixture.text("ack:after-async-search");
    assert!(matches!(
        fixture
            .handle
            .start_search_with_cancellation("ROW".into(), true, 1, cancelled),
        Err(ultraplexr_runtime::RuntimeError::SearchIncomplete)
    ));
}
