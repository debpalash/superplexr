use super::{LoopbackRelay, RuntimeFixture, next_observer_event};
use axum::{
    Router,
    body::{Body, BodyDataStream, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tower::ServiceExt;
use ultraplexr_client::{ControlClient, DaemonSession};
use ultraplexr_core::SessionId;
use ultraplexr_protocol::{ShareRole, TerminalSessionSpec};
use ultraplexr_terminal::GridSize;

const KEY: &str = "0123456789abcdef0123456789abcdef";
const HOST: &str = "127.0.0.1:7777";
const ORIGIN: &str = "http://127.0.0.1:7777";

#[path = "browser_search_tests.rs"]
mod browser_search_tests;

#[path = "browser_workflow_tests.rs"]
mod browser_workflow_tests;

#[path = "desktop_feed_integration_tests.rs"]
mod desktop_feed_tests;

#[path = "desktop_input_tests.rs"]
mod desktop_input_tests;

#[path = "concurrent_wait_tests.rs"]
mod concurrent_wait_tests;

#[path = "desktop_receive_tests.rs"]
mod desktop_receive_tests;

#[path = "metadata_feed_tests.rs"]
mod metadata_feed_tests;

#[test]
fn browser_control_idle_frames_stay_quiet_and_unresponsive_view_lease_expires() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = terminal(&owner, &fixture);
    let id = terminal.id();
    let (_, client) = scoped(&owner, &fixture, id, ShareRole::Controller);
    let gateway =
        ultraplexr_observer::Observer::with_control(client, HOST.into(), KEY.into()).unwrap();
    let router = gateway.clone().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (view, mut body, _) = attach(&router, id).await;
        terminal.release_control().unwrap();
        let (status, result) =
            command(&router, &view, 1, Value::Null, json!({"type":"claim"})).await;
        assert_eq!(status, StatusCode::OK);
        use tokio_stream::StreamExt;
        assert!(
            tokio::time::timeout(Duration::from_millis(2200), body.next())
                .await
                .is_err(),
            "heartbeat checks must not repaint idle frames"
        );
        tokio::time::timeout(Duration::from_secs(18), async {
            while body.next().await.is_some() {}
        })
        .await
        .expect("missing browser heartbeat closes attachment");
        assert_eq!(
            command(
                &router,
                &view,
                2,
                result["lease"].clone(),
                json!({"type":"paste","text":"expired\n","confirmed":true})
            )
            .await
            .0,
            StatusCode::GONE
        );
        gateway.shutdown();
    });
    terminal.claim_control(true).unwrap();
    terminal.kill().unwrap();
}

fn terminal(owner: &ControlClient, fixture: &RuntimeFixture) -> DaemonSession {
    let terminal = owner
        .start_terminal(TerminalSessionSpec {
            session_id: SessionId::new(),
            mission_id: None,
            run_id: None,
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "stty -echo; printf 'ready\\n'; exec cat".into(),
            ],
            cwd: fixture.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).unwrap(),
        })
        .unwrap();
    wait_text(&terminal, "ready");
    terminal
}

fn wait_text(terminal: &DaemonSession, text: &str) {
    terminal
        .wait(
            ultraplexr_protocol::TerminalWaitCondition::Text {
                query: text.into(),
                case_sensitive: true,
            },
            Duration::from_secs(5),
        )
        .unwrap();
}

fn scoped(
    owner: &ControlClient,
    fixture: &RuntimeFixture,
    id: SessionId,
    role: ShareRole,
) -> (ultraplexr_protocol::ShareSummary, ControlClient) {
    let (share, token) = owner
        .create_share("browser control test", role, vec![], vec![id], 60)
        .unwrap();
    (
        share,
        ControlClient::connect_with_share(fixture.root.join("s"), token).unwrap(),
    )
}

fn request(
    path: &str,
    method: &str,
    body: Value,
    origin: Option<&str>,
    authorized: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(path)
        .method(method)
        .header("host", HOST)
        .header("content-type", "application/json");
    if authorized {
        builder = builder.header("authorization", format!("Bearer {KEY}"));
    }
    if let Some(origin) = origin {
        builder = builder.header("origin", origin);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn attach(router: &Router, id: SessionId) -> (String, BodyDataStream, String) {
    let response = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = router
                .clone()
                .oneshot(request(
                    &format!("/sessions/{id}/events"),
                    "GET",
                    Value::Null,
                    None,
                    true,
                ))
                .await
                .unwrap();
            if response.status() != StatusCode::SERVICE_UNAVAILABLE {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("read-only attachment may retry after native repair");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let mut buffer = String::new();
    let (kind, id) = next_observer_event(&mut body, &mut buffer).await;
    assert_eq!(kind, "surface");
    assert_eq!(next_observer_event(&mut body, &mut buffer).await.0, "frame");
    (id, body, buffer)
}

async fn command(
    router: &Router,
    view: &str,
    sequence: u64,
    lease: Value,
    action: Value,
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(request(
            &format!("/views/{view}/commands"),
            "POST",
            json!({"sequence":sequence,"lease":lease,"action":action}),
            Some(ORIGIN),
            true,
        ))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 100_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[test]
fn browser_views_have_independent_control_ordering_and_cannot_force_or_replay_input() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = terminal(&owner, &fixture);
    let id = terminal.id();
    let pid = terminal.capture().unwrap().terminal.process_id;
    let (_, client) = scoped(&owner, &fixture, id, ShareRole::Controller);
    let gateway =
        ultraplexr_observer::Observer::with_control(client, HOST.into(), KEY.into()).unwrap();
    let router = gateway.clone().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (a, body_a, _) = attach(&router, id).await;
        let (b, body_b, _) = attach(&router, id).await;
        assert_ne!(a, b);
        assert_eq!(
            command(&router, &a, 1, Value::Null, json!({"type":"claim"}))
                .await
                .0,
            StatusCode::CONFLICT,
            "cannot steal owner's control"
        );
        terminal.release_control().unwrap();
        let (status, result) = command(&router, &a, 2, Value::Null, json!({"type":"claim"})).await;
        assert_eq!(status, StatusCode::OK);
        let lease_a = result["lease"].clone();
        assert_eq!(
            command(&router, &b, 1, Value::Null, json!({"type":"claim"}))
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            command(
                &router,
                &b,
                2,
                lease_a.clone(),
                json!({"type":"paste","text":"stolen\\n","confirmed":true})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            command(&router, &a, 3, lease_a.clone(), json!({"type":"release"}))
                .await
                .0,
            StatusCode::OK
        );
        let (status, result) = command(&router, &b, 3, Value::Null, json!({"type":"claim"})).await;
        assert_eq!(status, StatusCode::OK);
        let lease_b = result["lease"].clone();
        let action = json!({"type":"paste","text":"one-execution\n","confirmed":true});
        assert_eq!(
            command(&router, &b, 4, lease_b.clone(), action.clone())
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            command(&router, &b, 4, lease_b.clone(), action).await.0,
            StatusCode::CONFLICT
        );
        wait_text(&terminal, "one-execution");
        let text = terminal
            .snapshot()
            .unwrap()
            .rows
            .iter()
            .map(|r| r.text())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text.matches("one-execution").count(), 1);
        assert!(!text.contains("stolen"));
        assert_eq!(
            command(
                &router,
                &a,
                4,
                lease_a,
                json!({"type":"paste","text":"stale\n","confirmed":true})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            command(
                &router,
                &b,
                5,
                lease_b.clone(),
                json!({"type":"resize","columns":100,"rows":30})
            )
            .await
            .0,
            StatusCode::OK
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while terminal.snapshot().unwrap().grid != GridSize::new(100, 30).unwrap() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("resize reaches the canonical frame");
        drop(body_a);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            command(&router, &b, 6, lease_b.clone(), json!({"type":"heartbeat"}))
                .await
                .0,
            StatusCode::OK,
            "closing another view cannot release B"
        );
        terminal.claim_control(true).unwrap();
        assert_eq!(
            command(
                &router,
                &b,
                7,
                lease_b,
                json!({"type":"paste","text":"after-owner-takeover\n","confirmed":true})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        drop(body_b);
        gateway.shutdown();
    });
    assert_eq!(terminal.capture().unwrap().terminal.process_id, pid);
    terminal.claim_control(true).unwrap();
    terminal.kill().unwrap();
}

#[test]
fn browser_control_fails_closed_for_observers_origins_payloads_paste_and_scope() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = terminal(&owner, &fixture);
    let id = terminal.id();
    let (_, observer) = scoped(&owner, &fixture, id, ShareRole::Observer);
    let (_, controller) = scoped(&owner, &fixture, id, ShareRole::Controller);
    assert!(
        ultraplexr_observer::Observer::with_control(owner.clone(), HOST.into(), KEY.into())
            .is_err()
    );
    assert!(
        ultraplexr_observer::Observer::with_control(observer.clone(), HOST.into(), KEY.into())
            .is_err()
    );
    assert!(
        ultraplexr_observer::Observer::new(controller.clone(), HOST.into(), KEY.into()).is_err()
    );
    let readonly = ultraplexr_observer::Observer::new(observer, HOST.into(), KEY.into())
        .unwrap()
        .router();
    let gateway =
        ultraplexr_observer::Observer::with_control(controller, HOST.into(), KEY.into()).unwrap();
    let router = gateway.clone().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (view, body, _) = attach(&router, id).await;
        let path = format!("/views/{view}/commands");
        let claim = json!({"sequence":1,"lease":null,"action":{"type":"claim"}});
        for (origin, authorized, status) in [
            (None, true, StatusCode::FORBIDDEN),
            (Some("https://evil.example"), true, StatusCode::FORBIDDEN),
            (Some(ORIGIN), false, StatusCode::UNAUTHORIZED),
        ] {
            assert_eq!(
                router
                    .clone()
                    .oneshot(request(&path, "POST", claim.clone(), origin, authorized))
                    .await
                    .unwrap()
                    .status(),
                status
            );
        }
        assert_eq!(
            readonly
                .oneshot(request(&path, "POST", claim, Some(ORIGIN), true))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let unknown = router
            .clone()
            .oneshot(request(
                &format!("/sessions/{}/events", SessionId::new()),
                "GET",
                Value::Null,
                None,
                true,
            ))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
        terminal.release_control().unwrap();
        let (_, result) = command(&router, &view, 1, Value::Null, json!({"type":"claim"})).await;
        let lease = result["lease"].clone();
        assert!(lease.is_string());
        assert_eq!(
            command(
                &router,
                &view,
                2,
                lease.clone(),
                json!({"type":"paste","text":"dangerous\n"})
            )
            .await
            .0,
            StatusCode::PRECONDITION_REQUIRED
        );
        assert!(
            !terminal
                .snapshot()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.text().contains("dangerous"))
        );
        assert_eq!(
            command(
                &router,
                &view,
                3,
                lease.clone(),
                json!({"type":"resize","columns":401,"rows":24})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            command(
                &router,
                &view,
                4,
                lease.clone(),
                json!({"type":"paste","text":"x".repeat(16_385),"confirmed":true})
            )
            .await
            .0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            command(
                &router,
                &view,
                5,
                lease.clone(),
                json!({"type":"claim","force":true})
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            command(&router, &view, 5, lease.clone(), json!({"type":"kill"}))
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            command(
                &router,
                &view,
                5,
                lease,
                json!({"type":"paste","text":"confirmed\n","confirmed":true})
            )
            .await
            .0,
            StatusCode::OK
        );
        wait_text(&terminal, "confirmed");
        drop(body);
        gateway.shutdown();
    });
    terminal.claim_control(true).unwrap();
    terminal.kill().unwrap();
}

#[test]
fn browser_detach_native_disconnect_and_revocation_retire_input_identity() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let terminal = terminal(&owner, &fixture);
    let id = terminal.id();
    let pid = terminal.capture().unwrap().terminal.process_id;
    let (share, token) = owner
        .create_share(
            "browser reconnect",
            ShareRole::Controller,
            vec![],
            vec![id],
            60,
        )
        .unwrap();
    let relay = Arc::new(LoopbackRelay::new(fixture.root.join("s")));
    let client = ControlClient::connect_with_connector(relay.clone(), Some(token)).unwrap();
    let gateway =
        ultraplexr_observer::Observer::with_control(client, HOST.into(), KEY.into()).unwrap();
    let router = gateway.clone().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (first, body, _) = attach(&router, id).await;
        terminal.release_control().unwrap();
        let (_, result) = command(&router, &first, 1, Value::Null, json!({"type":"claim"})).await;
        let old = result["lease"].clone();
        drop(body);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if terminal
                    .capture()
                    .unwrap()
                    .terminal
                    .controller_surface_id
                    .is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            command(
                &router,
                &first,
                2,
                old,
                json!({"type":"paste","text":"detached\n","confirmed":true})
            )
            .await
            .0,
            StatusCode::GONE
        );
        let (second, mut body, _) = attach(&router, id).await;
        let (_, result) = command(&router, &second, 1, Value::Null, json!({"type":"claim"})).await;
        let old = result["lease"].clone();
        relay.disconnect();
        use tokio_stream::StreamExt;
        tokio::time::timeout(Duration::from_secs(3), async {
            while body.next().await.is_some() {}
        })
        .await
        .unwrap();
        assert_eq!(
            command(
                &router,
                &second,
                2,
                old.clone(),
                json!({"type":"paste","text":"offline\n","confirmed":true})
            )
            .await
            .0,
            StatusCode::GONE
        );
        relay.resume();
        let (third, body, _) = attach(&router, id).await;
        assert_ne!(second, third);
        assert_eq!(
            command(
                &router,
                &third,
                1,
                old,
                json!({"type":"paste","text":"replayed\n","confirmed":true})
            )
            .await
            .0,
            StatusCode::CONFLICT
        );
        let (_, result) = command(&router, &third, 2, Value::Null, json!({"type":"claim"})).await;
        let lease = result["lease"].clone();
        assert!(lease.is_string());
        owner.revoke_share(share.share_id).unwrap();
        let status = command(
            &router,
            &third,
            3,
            lease,
            json!({"type":"paste","text":"revoked\n","confirmed":true}),
        )
        .await
        .0;
        assert!([StatusCode::GONE, StatusCode::CONFLICT].contains(&status));
        drop(body);
        gateway.shutdown();
    });
    let capture = terminal.capture().unwrap();
    assert_eq!(capture.terminal.process_id, pid);
    let text = capture
        .frame
        .rows
        .iter()
        .map(|r| r.text())
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["detached", "offline", "replayed", "revoked"] {
        assert!(!text.contains(forbidden));
    }
    terminal.claim_control(true).unwrap();
    terminal.kill().unwrap();
}
