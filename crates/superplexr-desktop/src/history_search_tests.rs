use super::*;
use crate::history_search::{Search, Target};
use superplexr_client::{DaemonSession, TerminalStreamUpdate};
use superplexr_protocol::{SessionGroupId, ShareRole};

fn terminal(daemon: &RuntimeFixture, owner: &ControlClient, marker: &str) -> DaemonSession {
    let terminal = owner.start_terminal(TerminalSessionSpec {
        session_id: SessionId::new(), mission_id: None, run_id: None,
        program: "/bin/sh".into(),
        args: vec!["-c".into(), format!("stty -echo; awk 'BEGIN {{ for (i=0;i<80;i++) printf \"needle {marker} %04d\\n\",i; print \"READY-{marker}\" }}'; exec cat")],
        cwd: daemon.root.clone(), environment_delta: BTreeMap::new(),
        grid: GridSize { columns: 80, rows: 24 },
    }).expect("terminal");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !terminal
        .snapshot()
        .expect("frame")
        .rows
        .iter()
        .any(|row| row.text().contains(&format!("READY-{marker}")))
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    terminal
}
fn target(session: &DaemonSession, index: usize) -> Target {
    Target {
        group_id: SessionGroupId::new(),
        surface_index: index,
        session: session.clone(),
    }
}
fn finish(search: &mut Search) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while search.running {
        search.poll();
        assert!(
            Instant::now() < deadline,
            "search deadline: {}",
            search.note
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn desktop_search_streams_across_terminals_and_reveals_without_shared_scroll() {
    let daemon = RuntimeFixture::start();
    let owner = daemon.client();
    let sessions: Vec<_> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|name| terminal(&daemon, &owner, name))
        .collect();
    let before = owner.list_terminals().expect("before");
    let live = sessions[0].snapshot().expect("frame");
    let workspace = WorkspaceId::from_raw(42);
    let mut search = Search::new(workspace).expect("search");
    search.start(
        workspace,
        "needle".into(),
        sessions
            .iter()
            .enumerate()
            .map(|(i, s)| target(s, i))
            .collect(),
    );
    let deadline = Instant::now() + Duration::from_secs(8);
    while search.hits.is_empty() {
        search.poll();
        assert!(Instant::now() < deadline, "{}", search.note);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        search.running,
        "first terminal's results precede overall completion"
    );
    assert!((1..=8).contains(&search.hits.len()));
    finish(&mut search);
    assert_eq!(search.hits.len(), 24);
    assert!(search.note.contains("limit"));
    for session in &sessions {
        assert_eq!(
            search
                .hits
                .iter()
                .filter(|hit| hit.session_id == session.id())
                .count(),
            8
        );
    }
    search.reveal(search.hits[0].clone(), sessions[0].clone());
    finish(&mut search);
    let (hit, frame) = search.preview.take().expect("read-only viewport");
    assert_eq!(hit.session_id, sessions[0].id());
    assert!(
        frame
            .rows
            .iter()
            .any(|row| row.text().contains("alpha 0000"))
    );
    assert_eq!(
        sessions[0].snapshot().expect("live unchanged").rows,
        live.rows
    );
    let after = owner.list_terminals().expect("after");
    for old in before {
        let new = after
            .iter()
            .find(|session| session.session_id == old.session_id)
            .expect("same session");
        assert_eq!(old.process_id, new.process_id);
        assert_eq!(old.controller_surface_id, new.controller_surface_id);
    }
}

#[test]
fn desktop_search_replacement_and_revoked_scope_cannot_publish_old_results() {
    let daemon = RuntimeFixture::start();
    let owner = daemon.client();
    let session = terminal(&daemon, &owner, "replacement");
    let first = WorkspaceId::from_raw(1);
    let second = WorkspaceId::from_raw(2);
    let mut search = Search::new(first).expect("search");
    search.start(first, "needle".into(), vec![target(&session, 0)]);
    search.start(
        second,
        "READY-replacement".into(),
        vec![target(&session, 12)],
    );
    finish(&mut search);
    assert_eq!(search.workspace, second);
    assert_eq!(search.hits.len(), 1);
    assert_eq!(search.hits[0].surface_index, 12);
    assert!(search.hits[0].found.preview.contains("READY-replacement"));
    let (share, token) = owner
        .create_share(
            "desktop search",
            ShareRole::Observer,
            vec![],
            vec![session.id()],
            60,
        )
        .expect("share");
    let observer =
        ControlClient::connect_with_share(daemon.root.join("s"), token).expect("observer");
    let scoped = observer.terminal(session.id());
    let (send, receive) = std::sync::mpsc::channel();
    let _subscription = scoped
        .subscribe_events_with_status(move |event| send.send(event).is_ok())
        .expect("desktop event interface");
    assert!(matches!(
        receive.recv_timeout(Duration::from_secs(5)).expect("frame"),
        TerminalStreamUpdate::Event(_)
    ));
    owner.revoke_share(share.share_id).expect("revoke");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if matches!(
            receive.recv_timeout(Duration::from_secs(1)),
            Ok(TerminalStreamUpdate::Rejected)
        ) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "explicit rejection reaches desktop interface"
        );
    }
    search.start(second, "needle".into(), vec![target(&scoped, 0)]);
    finish(&mut search);
    assert!(search.hits.is_empty());
    assert!(search.note.contains("failed"));
    assert_eq!(owner.list_terminals().expect("retained").len(), 1);
}
