use super::*;
use superplexr_protocol::search_stream::SearchPage;
use tokio_stream::StreamExt;

fn history_terminal(owner: &ControlClient, fixture: &RuntimeFixture) -> DaemonSession {
    let session = owner.start_terminal(TerminalSessionSpec {
        session_id: SessionId::new(), mission_id: None, run_id: None,
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "stty -echo; awk 'BEGIN { for(i=0;i<1200;i++) printf \"needle %04d\\n\",i; print \"READY\" }'; exec cat".into()],
        cwd: fixture.root.clone(), environment_delta: BTreeMap::new(),
        grid: GridSize::new(80, 24).unwrap(),
    }).unwrap();
    wait_text(&session, "READY");
    session
}

async fn start_search(router: &Router, id: SessionId) -> axum::response::Response {
    router
        .clone()
        .oneshot(request(
            &format!("/sessions/{id}/search"),
            "POST",
            json!({"query":"needle","case_sensitive":true,"limit":1000}),
            Some(ORIGIN),
            true,
        ))
        .await
        .unwrap()
}

async fn collect_search(response: axum::response::Response, id: SessionId) -> Vec<Value> {
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let mut buffer = String::new();
    let mut sequence = 1;
    let mut identity = None;
    let mut matches = Vec::new();
    loop {
        let (kind, data) = next_observer_event(&mut body, &mut buffer).await;
        assert_eq!(kind, "search-page", "{data}");
        let page: SearchPage = serde_json::from_str(&data).unwrap();
        assert_eq!(page.session_id, id);
        assert_eq!(page.sequence, sequence);
        assert!(page.matches.len() <= 64);
        assert!(data.len() <= 512 * 1024);
        assert!(page.error.is_none());
        assert_eq!(*identity.get_or_insert(page.search_id), page.search_id);
        sequence += 1;
        matches.extend(
            page.matches
                .iter()
                .map(|found| serde_json::to_value(found).unwrap()),
        );
        if page.complete {
            break;
        }
    }
    assert!(sequence > 2, "stream must deliver multiple pages");
    assert!(
        body.next().await.is_none(),
        "complete page closes HTTP stream"
    );
    matches
}

#[test]
fn browser_search_matches_native_history_without_mutating_terminal() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = history_terminal(&owner, &fixture);
    let id = terminal.id();
    let before = terminal.capture().unwrap();
    let live = terminal.snapshot().unwrap();
    let canonical = terminal.search("needle", true, 1000).unwrap();
    let (_, client) = scoped(&owner, &fixture, id, ShareRole::Observer);
    let gateway = superplexr_observer::Observer::new(client, HOST.into(), KEY.into()).unwrap();
    let router = gateway.clone().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let matches = collect_search(start_search(&router, id).await, id).await;
        assert_eq!(
            matches,
            canonical
                .iter()
                .map(|found| serde_json::to_value(found).unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(matches.len(), 1000);
        let row = matches[0]["line"].as_u64().unwrap();
        let response = router
            .clone()
            .oneshot(request(
                &format!("/sessions/{id}/history-line/{row}"),
                "GET",
                Value::Null,
                None,
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let frame: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(frame["text"].as_str().unwrap().contains("needle 0000"));
        gateway.shutdown();
    });
    let after = terminal.capture().unwrap();
    assert_eq!(after.terminal.process_id, before.terminal.process_id);
    assert_eq!(
        after.terminal.controller_surface_id,
        before.terminal.controller_surface_id
    );
    assert_eq!(terminal.snapshot().unwrap().rows, live.rows);
    terminal.kill().unwrap();
}

#[test]
fn browser_search_enforces_auth_origin_scope_and_query_bounds() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = terminal(&owner, &fixture);
    let hidden = terminal.id();
    let visible = history_terminal(&owner, &fixture);
    let (_, client) = scoped(&owner, &fixture, visible.id(), ShareRole::Observer);
    let gateway = superplexr_observer::Observer::new(client, HOST.into(), KEY.into()).unwrap();
    let router = gateway.clone().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let path = format!("/sessions/{}/search", visible.id());
        for (origin, authorized, status) in [
            (Some(ORIGIN), false, StatusCode::UNAUTHORIZED),
            (None, true, StatusCode::FORBIDDEN),
            (Some("http://foreign.invalid"), true, StatusCode::FORBIDDEN),
        ] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(
                        &path,
                        "POST",
                        json!({"query":"needle","limit":10}),
                        origin,
                        authorized
                    ))
                    .await
                    .unwrap()
                    .status(),
                status
            );
        }
        for query in [
            json!({"query":"","limit":10}),
            json!({"query":"界".repeat(342),"limit":10}),
            json!({"query":"needle","limit":1001}),
            json!({"query":"needle","limit":0}),
        ] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(&path, "POST", query, Some(ORIGIN), true))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        let response = start_search(&router, hidden).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "native admission errors are SSE errors"
        );
        let mut body = response.into_body().into_data_stream();
        let (kind, _) = next_observer_event(&mut body, &mut String::new()).await;
        assert_eq!(kind, "search-error");
        let response = router
            .clone()
            .oneshot(request(
                &format!("/sessions/{hidden}/history-line/0"),
                "GET",
                Value::Null,
                None,
                true,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        gateway.shutdown();
    });
}

#[test]
fn browser_search_body_drop_releases_bounded_capacity_and_preserves_processes() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let sessions: Vec<_> = (0..5).map(|_| history_terminal(&owner, &fixture)).collect();
    let (_, token) = owner
        .create_share(
            "search capacity",
            ShareRole::Observer,
            vec![],
            sessions.iter().map(DaemonSession::id).collect(),
            60,
        )
        .unwrap();
    let client = ControlClient::connect_with_share(fixture.root.join("s"), token).unwrap();
    let gateway = superplexr_observer::Observer::new(client, HOST.into(), KEY.into()).unwrap();
    let router = gateway.clone().router();
    let before = owner.list_terminals().unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut held = Vec::new();
        for session in &sessions[..4] {
            let response = start_search(&router, session.id()).await;
            assert_eq!(response.status(), StatusCode::OK);
            let mut body = response.into_body().into_data_stream();
            assert_eq!(
                next_observer_event(&mut body, &mut String::new()).await.0,
                "search-page"
            );
            held.push(body);
        }
        assert_eq!(
            start_search(&router, sessions[4].id()).await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        drop(held);
        let response = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let response = start_search(&router, sessions[4].id()).await;
                if response.status() != StatusCode::TOO_MANY_REQUESTS {
                    break response;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("HTTP detach retires workers and returns search capacity");
        assert_eq!(collect_search(response, sessions[4].id()).await.len(), 1000);
        gateway.shutdown();
        assert_eq!(
            start_search(&router, sessions[0].id()).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    });
    for old in before {
        let new = owner
            .list_terminals()
            .unwrap()
            .into_iter()
            .find(|session| session.session_id == old.session_id)
            .unwrap();
        assert_eq!(new.process_id, old.process_id);
        assert_eq!(new.controller_surface_id, old.controller_surface_id);
    }
}

#[test]
fn browser_search_revocation_ends_backpressured_stream_without_complete_results() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = history_terminal(&owner, &fixture);
    let id = terminal.id();
    let pid = terminal.capture().unwrap().terminal.process_id;
    let (share, client) = scoped(&owner, &fixture, id, ShareRole::Observer);
    let gateway = superplexr_observer::Observer::new(client, HOST.into(), KEY.into()).unwrap();
    let router = gateway.clone().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let response = start_search(&router, id).await;
        let mut body = response.into_body().into_data_stream();
        let mut buffer = String::new();
        assert_eq!(
            next_observer_event(&mut body, &mut buffer).await.0,
            "search-page"
        );
        // Stop consuming long enough to fill the small HTTP/native relay queues.
        tokio::time::sleep(Duration::from_millis(150)).await;
        owner.revoke_share(share.share_id).unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(chunk) = body.next().await {
                buffer.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
            }
        })
        .await
        .expect("revocation retires blocked relay");
        assert!(
            !buffer.contains("\"complete\":true"),
            "partial bytes cannot become successful completion"
        );
        gateway.shutdown();
    });
    assert_eq!(terminal.capture().unwrap().terminal.process_id, pid);
    terminal.paste(b"still-alive\n".to_vec(), true).unwrap();
    wait_text(&terminal, "still-alive");
}
