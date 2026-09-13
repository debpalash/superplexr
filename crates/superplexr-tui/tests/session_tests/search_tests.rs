use super::*;
use superplexr_tui::search::{Update, Worker};

fn history_fixture() -> Fixture {
    let fixture = Fixture::with_program(
        "/bin/sh",
        &[
            "-c",
            "stty -echo; awk 'BEGIN { for (i=0; i<400; i++) printf \"needle 界 é row %04d\\n\", i; print \"TUI-SEARCH-READY\" }'; exec cat",
        ],
    );
    until(|| fixture.text().contains("TUI-SEARCH-READY"));
    fixture
}

fn update(worker: &Worker) -> Update {
    let mut result = None;
    until(|| {
        result = worker.take_update();
        result.is_some()
    });
    result.expect("update")
}

#[test]
fn worker_streams_replaces_backpressured_jobs_and_reads_history_without_control() {
    let fixture = history_fixture();
    let before = fixture.owner.list_terminals().expect("before")[0].clone();
    let grid = fixture.terminal.snapshot().expect("frame");
    let worker = Worker::new().expect("worker");
    worker.search(fixture.terminal.clone(), "needle".into());
    let first = match update(&worker) {
        Update::Page(page) => {
            assert!(!page.complete);
            page.matches[0].clone()
        }
        _ => panic!("expected first incremental page"),
    };
    // Leave a page in the mailbox: replacement must wake the backpressured
    // producer, retire the old native job, and never publish stale results.
    std::thread::sleep(Duration::from_millis(150));
    let started = Instant::now();
    worker.cancel();
    assert!(worker.take_update().is_none());
    worker.search(fixture.terminal.clone(), "TUI-SEARCH-READY".into());
    assert!(started.elapsed() < Duration::from_secs(1));
    let mut matches = Vec::new();
    loop {
        match update(&worker) {
            Update::Page(page) => {
                matches.extend(page.matches);
                if page.complete {
                    break;
                }
            }
            Update::Error(error) => panic!("replacement search: {error}"),
            _ => panic!("unexpected history"),
        }
    }
    assert_eq!(matches.len(), 1);
    assert!(matches[0].preview.contains("TUI-SEARCH-READY"));
    worker.history(fixture.terminal.clone(), first.clone());
    match update(&worker) {
        Update::History(frame) => {
            assert!(frame.rows.iter().any(|row| row.text().contains("row 0000")))
        }
        Update::Error(error) => panic!("history: {error}"),
        _ => panic!("expected history"),
    }
    let mut stale = first;
    stale.preview = "not the original history".into();
    worker.history(fixture.terminal.clone(), stale);
    assert!(matches!(update(&worker), Update::Error(error) if error.contains("History changed")));
    let after = fixture.owner.list_terminals().expect("after")[0].clone();
    assert_eq!(before.process_id, after.process_id);
    assert_eq!(before.controller_surface_id, after.controller_surface_id);
    assert_eq!(
        fixture
            .terminal
            .snapshot()
            .expect("unchanged live grid")
            .rows,
        grid.rows
    );
}

#[test]
fn real_tui_search_edits_unicode_reveals_history_and_keeps_query_out_of_pty() {
    let fixture = history_fixture();
    fixture.terminal.release_control().expect("release");
    let before = fixture.owner.list_terminals().expect("before")[0].clone();
    let mut tui = OuterTerminal::start(&fixture, &["--control"]);
    until(|| tui.saw("Control acquired"));
    tui.send(b"\x1d/");
    until(|| tui.saw("Superplexr / Search"));
    tui.send("needle 界\r".as_bytes());
    until(|| tui.saw("Search complete: 400 results"));
    assert_eq!(fixture.text().matches("TUI-SEARCH-READY").count(), 1);
    tui.send(b"j\r");
    until(|| tui.saw("Search history"));
    // The query/result-navigation keys must not be sent to the controlled cat.
    assert!(!fixture.text().contains("needle 界j"));
    assert_eq!(
        fixture.terminal.history_frame(0).expect("history").rows,
        fixture.terminal.snapshot().expect("live").rows
    );
    tui.send(b"\x1dl");
    until(|| tui.saw("Live output"));
    tui.send(b"\x1d/");
    std::thread::sleep(Duration::from_millis(100));
    tui.send(b"NO-SUCH-QUERY\r");
    until(|| tui.saw("Search complete: 0 results"));
    tui.send(b"/\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f\x7f");
    tui.send(b"\x1b[200~TUI-SEARCH-READY\x1b[201~\r");
    until(|| tui.saw("Search complete: 1 results"));
    tui.send(b"\x03");
    until(|| tui.saw("Search cancelled"));
    tui.send(b"\x1d n"); // Unknown prefix command stays local.
    tui.send(b"\x1dd");
    assert!(tui.wait_exit().success());
    assert!(!fixture.text().contains("NO-SUCH-QUERY"));
    let after = fixture.owner.list_terminals().expect("after")[0].clone();
    assert_eq!(before.process_id, after.process_id);
    assert!(after.controller_surface_id.is_none());
    until(|| tui.saw("\x1b[?1049l"));
}

#[test]
fn real_tui_search_revocation_clears_view_and_detaches_without_killing_host() {
    use std::os::unix::fs::OpenOptionsExt;
    let fixture = history_fixture();
    let (share, token) = fixture
        .owner
        .create_share(
            "search observer",
            ShareRole::Observer,
            vec![],
            vec![fixture.terminal.id()],
            60,
        )
        .expect("share");
    let path = fixture.root.join("search.token");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .expect("private token");
    file.write_all(token.as_bytes()).expect("token");
    let mut tui = OuterTerminal::start(
        &fixture,
        &["--share-token-file", path.to_str().expect("path")],
    );
    until(|| tui.saw("Observing"));
    tui.send(b"\x1d/needle\r");
    until(|| tui.saw("Search complete: 400 results"));
    fixture.owner.revoke_share(share.share_id).expect("revoke");
    assert!(!tui.wait_exit().success());
    until(|| tui.saw("\x1b[?1049l"));
    assert_eq!(
        fixture.owner.list_terminals().expect("live")[0].status,
        TerminalSessionStatus::Running
    );
}

#[test]
fn pending_admission_is_nonblocking_and_only_latest_query_survives() {
    let fixture = history_fixture();
    let worker = Worker::new().expect("worker");
    let signal = |name: &str| {
        assert!(
            Command::new("kill")
                .args([name, &fixture.child.id().to_string()])
                .status()
                .expect("signal private fixture daemon")
                .success()
        );
    };
    // Hold the actual daemon, not a fake SearchStream. Its startup response
    // cannot arrive until CONT, so cancellation must also cover pre-admission.
    signal("-STOP");
    worker.search(fixture.terminal.clone(), "needle".into());
    std::thread::sleep(Duration::from_millis(150));
    assert!(worker.take_update().is_none());
    let started = Instant::now();
    for _ in 0..100 {
        worker.cancel();
        worker.search(fixture.terminal.clone(), "obsolete query".into());
    }
    worker.search(fixture.terminal.clone(), "TUI-SEARCH-READY".into());
    assert!(started.elapsed() < Duration::from_secs(1));
    signal("-CONT");
    let mut matches = Vec::new();
    loop {
        match update(&worker) {
            Update::Page(page) => {
                matches.extend(page.matches);
                if page.complete {
                    break;
                }
            }
            Update::Error(error) => panic!("latest search: {error}"),
            _ => panic!("unexpected history"),
        }
    }
    assert_eq!(matches.len(), 1);
    assert!(matches[0].preview.contains("TUI-SEARCH-READY"));
}
