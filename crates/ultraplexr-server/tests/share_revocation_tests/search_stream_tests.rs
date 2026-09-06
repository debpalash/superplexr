use super::*;
use ultraplexr_protocol::{
    ResponseBody,
    search_stream::{MAX_PAGE_BYTES, SearchPage},
};

fn legacy(fixture: &Fixture) -> Vec<ultraplexr_terminal::SearchMatch> {
    match fixture
        .owner
        .request(Request::TerminalSearch {
            session_id: fixture.terminal.id(),
            query: "ROW".into(),
            case_sensitive: true,
            limit: 1000,
        })
        .expect("legacy collection")
    {
        ResponseBody::TerminalSearchResults { matches, .. } => matches,
        _ => panic!("legacy search response"),
    }
}

fn history() -> Fixture {
    let process = Process::new();
    let owner = process.owner();
    // Generate history as PTY output, not thousands of lines sent through the
    // kernel's bounded canonical input/echo queues.
    let terminal = owner.start_terminal(TerminalSessionSpec {
        session_id: SessionId::new(), mission_id: None, run_id: None,
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "stty -echo; awk 'BEGIN { for(i=0;i<1200;i++) printf \"ROW-%06d\\n\",i }'; printf 'SEARCH-READY\\n'; exec /bin/cat".into()],
        cwd: process.root.clone(), environment_delta: BTreeMap::new(),
        grid: GridSize::new(80, 24).expect("grid"),
    }).expect("history PTY");
    let fixture = Fixture {
        process,
        owner,
        terminal,
    };
    fixture
        .terminal
        .wait(text("SEARCH-READY"), Duration::from_secs(3))
        .expect("history ready");
    fixture
}

#[test]
fn pages_match_canonical_search_and_cancel_without_replacing_the_terminal() {
    let fixture = history();
    let before = fixture.terminal.capture().expect("identity").terminal;
    let expected = legacy(&fixture);
    let (_, token) = fixture.share();
    let observer = fixture.observer(&token);
    let session = observer.terminal(fixture.terminal.id());
    let mut pages = session
        .search_pages("ROW", true, 1000)
        .expect("negotiated stream");
    let first = pages.next_page().expect("first page").expect("page");
    assert!(!first.complete);
    let mut actual = first.matches;
    while let Some(page) = pages.next_page().expect("next page") {
        assert!(page.matches.len() <= 64);
        assert!(serde_json::to_vec(&page).expect("encoded page").len() <= MAX_PAGE_BYTES);
        actual.extend(page.matches);
    }
    assert_eq!(actual, expected);
    drop(pages);

    let mut paused = session
        .search_pages("ROW", true, 1000)
        .expect("second search");
    assert!(!paused.next_page().expect("first").expect("page").complete);
    let cancellation = paused.cancellation();
    cancellation.cancel();
    assert!(paused.next_page().is_err());
    // Ping on the same connection orders after the cancel. A separately
    // connected client cannot serve as a teardown barrier for this stream.
    assert!(matches!(
        observer.request(Request::Ping).expect("cancel barrier"),
        ResponseBody::Pong
    ));
    let mut next = session.search_pages("ROW", true, 1).expect("readmission");
    assert!(
        next.next_page()
            .expect("replacement")
            .expect("complete page")
            .complete
    );
    fixture
        .terminal
        .paste(b"AFTER-SEARCH-CANCEL\n".to_vec(), true)
        .expect("owner input");
    let after = fixture
        .terminal
        .wait(text("AFTER-SEARCH-CANCEL"), Duration::from_secs(3))
        .expect("input works")
        .1
        .terminal;
    assert_eq!(before.session_id, after.session_id);
    assert_eq!(before.process_id, after.process_id);
}

fn raw(fixture: &Fixture, token: &str) -> (SyncWire<std::os::unix::net::UnixStream>, Uuid, String) {
    let socket =
        std::os::unix::net::UnixStream::connect(fixture.process.root.join("s")).expect("socket");
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    let mut wire = SyncWire::new(socket);
    let client = Uuid::new_v4();
    wire.client_handshake(&Hello::new(client, "search-test", client))
        .expect("handshake");
    (wire, client, token.into())
}
fn send(
    raw: &mut (SyncWire<std::os::unix::net::UnixStream>, Uuid, String),
    action: Request,
) -> Uuid {
    let mut request = ClientRequest::for_client(raw.1, action);
    request.share_token = Some(raw.2.clone());
    raw.0
        .send_json(FrameKind::Request, 0, &request)
        .expect("request");
    request.request_id
}
fn start(
    raw: &mut (SyncWire<std::os::unix::net::UnixStream>, Uuid, String),
    id: SessionId,
) -> (Uuid, u32, SearchPage) {
    let search_id = Uuid::new_v4();
    send(
        raw,
        Request::TerminalSearchStart {
            search_id,
            session_id: id,
            query: "ROW".into(),
            case_sensitive: true,
            limit: 1000,
        },
    );
    let response: ServerResponse = raw
        .0
        .receive_json(FrameKind::Response, 0)
        .expect("acceptance");
    let ResponseResult::Success { body } = response.result else {
        panic!("start denied")
    };
    let ResponseBody::TerminalSearchStarted { stream_id, .. } = *body else {
        panic!("wrong response")
    };
    let page = raw
        .0
        .receive_json(FrameKind::SearchPage, stream_id)
        .expect("first page");
    (search_id, stream_id, page)
}

#[test]
fn paused_stream_is_backpressured_and_another_connection_cannot_ack_or_cancel_it() {
    let fixture = history();
    let (_, token) = fixture.share();
    let mut wire = raw(&fixture, &token);
    let (search_id, stream, page) = start(&mut wire, fixture.terminal.id());
    assert!(!page.complete);
    // A ping response is next, not another search page: no ACK, no publication.
    let ping = send(&mut wire, Request::Ping);
    let response: ServerResponse = wire
        .0
        .receive_json(FrameKind::Response, 0)
        .expect("ping without draining search");
    assert_eq!(response.request_id, ping);
    let other = fixture.observer(&token);
    assert!(
        other
            .request(Request::TerminalSearchAck {
                search_id,
                sequence: page.sequence
            })
            .is_err()
    );
    other
        .request(Request::TerminalSearchCancel { search_id })
        .expect("foreign cancel is inert");
    send(
        &mut wire,
        Request::TerminalSearchAck {
            search_id,
            sequence: page.sequence,
        },
    );
    // The ACK receipt and next page may interleave on independently sequenced streams.
    let mut next = None;
    for _ in 0..2 {
        let frame = wire.0.receive().expect("ack or page");
        if frame.header.kind == FrameKind::SearchPage {
            assert_eq!(frame.header.stream_id, stream);
            next = Some(serde_json::from_slice::<SearchPage>(&frame.payload).expect("page"));
        }
    }
    assert_eq!(next.expect("still active").sequence, 2);
    // Duplicate ACK cancels this search, never advances its credit again.
    send(
        &mut wire,
        Request::TerminalSearchAck {
            search_id,
            sequence: 1,
        },
    );
    let response: ServerResponse = wire
        .0
        .receive_json(FrameKind::Response, 0)
        .expect("stale ACK response");
    assert!(matches!(response.result, ResponseResult::Error { .. }));
    let (_, _, replacement) = start(&mut wire, fixture.terminal.id());
    assert_eq!(replacement.sequence, 1);
}

#[test]
fn revocation_and_disconnect_cancel_native_jobs_without_a_consumer_ack() {
    let fixture = history();
    let (share, token) = fixture.share();
    let mut wire = raw(&fixture, &token);
    start(&mut wire, fixture.terminal.id());
    fixture.owner.revoke_share(share.share_id).expect("revoke");
    assert!(
        wire.0.receive().is_err(),
        "revoked paused stream must close"
    );
    until(|| fixture.terminal.search("ROW", true, 1).is_ok());
    let (_, token) = fixture.share();
    let mut wire = raw(&fixture, &token);
    start(&mut wire, fixture.terminal.id());
    drop(wire);
    until(|| fixture.terminal.search("ROW", true, 1).is_ok());
}

#[test]
fn closed_session_uses_the_same_wire_pages_and_oversized_queries_are_refused() {
    let fixture = history();
    assert!(
        fixture
            .terminal
            .search_pages("x".repeat(1025), true, 1)
            .is_err()
    );
    fixture.terminal.kill().expect("close PTY");
    fixture
        .terminal
        .wait(TerminalWaitCondition::Exit, Duration::from_secs(3))
        .expect("exited");
    let expected = legacy(&fixture);
    let mut pages = fixture
        .terminal
        .search_pages("ROW", true, 1000)
        .expect("archived stream");
    let mut actual = vec![];
    while let Some(page) = pages.next_page().expect("archive page") {
        actual.extend(page.matches);
    }
    assert_eq!(actual, expected);
}

#[test]
fn negotiated_absence_uses_legacy_collection_without_sending_unknown_requests() {
    use ultraplexr_client::transport::{Connection, Connector};
    struct WithoutSearch(PathBuf);
    impl Connector for WithoutSearch {
        fn connect(&self, client_id: Uuid) -> Result<Connection, ultraplexr_client::ClientError> {
            let socket = std::os::unix::net::UnixStream::connect(&self.0)?;
            socket.set_read_timeout(Some(Duration::from_secs(3)))?;
            let mut wire = SyncWire::new(socket);
            let mut hello = Hello::new(client_id, "no-search-feature", client_id);
            hello
                .features
                .retain(|feature| feature != ultraplexr_protocol::search_stream::FEATURE);
            wire.client_handshake(&hello)
                .map_err(ultraplexr_protocol::ProtocolError::from)?;
            wire.get_ref().set_read_timeout(None)?;
            let shutdown = wire.get_ref().try_clone()?;
            Connection::from_negotiated(
                wire,
                |socket| Ok((socket.try_clone()?, socket)),
                move || {
                    let _ = shutdown.shutdown(std::net::Shutdown::Both);
                },
            )
        }
    }
    let fixture = history();
    let client = ControlClient::connect_with_connector(
        std::sync::Arc::new(WithoutSearch(fixture.process.root.join("s"))),
        None,
    )
    .expect("legacy feature profile");
    let session = client.terminal(fixture.terminal.id());
    assert!(
        matches!(session.search_pages("ROW", true, 1), Err(ultraplexr_client::ClientError::Remote { code, .. }) if code == "unsupported_search_stream")
    );
    assert_eq!(
        session.search("ROW", true, 1000).expect("legacy fallback"),
        legacy(&fixture)
    );
    assert!(matches!(
        client
            .request(Request::Ping)
            .expect("connection still usable"),
        ResponseBody::Pong
    ));
}

#[test]
fn dense_unicode_previews_are_partitioned_without_truncation_or_missing_matches() {
    let fixture = Fixture::new();
    // 80 grapheme cells, each with 100 combining marks: one physical row is
    // much larger than its grid width. This forces byte, not only count, pages.
    let row = format!("x{}", "\u{301}".repeat(100)).repeat(80);
    let session = fixture
        .owner
        .start_terminal(TerminalSessionSpec {
            session_id: SessionId::new(),
            mission_id: None,
            run_id: None,
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "printf '%s\\n' \"$1\"; printf 'DENSE-READY\\n'; read -r done".into(),
                "fixture".into(),
                row,
            ],
            cwd: fixture.process.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).expect("grid"),
        })
        .expect("dense PTY");
    struct Kill(DaemonSession);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
        }
    }
    let session = Kill(session);
    session
        .0
        .wait(text("DENSE-READY"), Duration::from_secs(3))
        .expect("dense history");
    let expected = match fixture
        .owner
        .request(Request::TerminalSearch {
            session_id: session.0.id(),
            query: "x".into(),
            case_sensitive: true,
            limit: 1000,
        })
        .expect("canonical")
    {
        ResponseBody::TerminalSearchResults { matches, .. } => matches,
        _ => panic!("canonical result"),
    };
    assert!(
        expected.iter().any(|hit| hit.preview.len() > 8192),
        "must exercise large native previews"
    );
    let mut stream = session
        .0
        .search_pages("x", true, 1000)
        .expect("dense pages");
    let mut matches = vec![];
    let mut pages = 0;
    while let Some(page) = stream.next_page().expect("byte-bounded page") {
        assert!(serde_json::to_vec(&page).expect("wire JSON").len() <= MAX_PAGE_BYTES);
        matches.extend(page.matches);
        pages += 1;
    }
    assert_eq!(matches, expected);
    assert!(pages > 2, "byte budget must split the native 64-match page");
}

#[test]
fn stream_slots_are_bounded_across_connections_and_released_by_cancellation() {
    let fixture = history();
    struct Sessions(Vec<DaemonSession>);
    impl Drop for Sessions {
        fn drop(&mut self) {
            for session in &self.0 {
                let _ = session.kill();
            }
        }
    }
    let mut sessions = Sessions(vec![]);
    for _ in 0..8 {
        let session = fixture.owner.start_terminal(TerminalSessionSpec {
            session_id: SessionId::new(), mission_id: None, run_id: None,
            program: "/bin/sh".into(), args: vec!["-c".into(), "awk 'BEGIN { for(i=0;i<1200;i++) print \"ROW\" }'; printf 'READY\\n'; read -r done".into()],
            cwd: fixture.process.root.clone(), environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).expect("grid"),
        }).expect("quota PTY");
        session
            .wait(text("READY"), Duration::from_secs(3))
            .expect("ready");
        sessions.0.push(session);
    }
    let second = fixture.process.owner();
    let third = fixture.process.owner();
    let mut streams = vec![];
    for (index, session) in sessions.0.iter().enumerate() {
        let owner = if index < 4 { &fixture.owner } else { &second };
        let mut stream = owner
            .terminal(session.id())
            .search_pages("ROW", true, 1000)
            .expect("admitted quota slot");
        assert!(!stream.next_page().expect("first").expect("page").complete);
        streams.push(stream);
    }
    assert!(
        fixture.terminal.search_pages("ROW", true, 1000).is_err(),
        "client enforces connection cap"
    );
    assert!(
        matches!(third.terminal(fixture.terminal.id()).search_pages("ROW", true, 1000), Err(ultraplexr_client::ClientError::Remote { code, .. }) if code == "search_process_limit")
    );
    streams.remove(0).cancellation().cancel();
    fixture
        .owner
        .request(Request::Ping)
        .expect("cancel processed");
    let mut admitted = third
        .terminal(fixture.terminal.id())
        .search_pages("ROW", true, 1000)
        .expect("released process slot");
    assert!(
        !admitted
            .next_page()
            .expect("first after release")
            .expect("page")
            .complete
    );
}
