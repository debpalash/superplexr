use crate::{
    SessionStatus, WorkspaceView,
    workspace_reconcile::{apply_group, history_placeholder},
    workspace_store::{
        SessionRecord, ViewDismissals, WorkspaceDocument, WorkspaceRecord, WorkspaceStore,
    },
    workspace_tabs::{WorkspaceId, WorkspaceTabs},
};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    process::{Child, Command as ProcessCommand, Stdio},
    time::{Duration, Instant},
};
use ultraplexr_client::ControlClient;

async fn next_observer_event(
    stream: &mut axum::body::BodyDataStream,
    buffer: &mut String,
) -> (String, String) {
    use tokio_stream::StreamExt;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(end) = buffer.find("\n\n") {
                let packet = buffer[..end].to_owned();
                buffer.drain(..end + 2);
                let event = packet
                    .lines()
                    .find_map(|line| line.strip_prefix("event:"))
                    .unwrap_or("")
                    .trim();
                let data = packet
                    .lines()
                    .filter_map(|line| line.strip_prefix("data:"))
                    .map(str::trim_start)
                    .collect::<Vec<_>>()
                    .join("\n");
                if !event.is_empty() {
                    return (event.to_owned(), data);
                }
                continue;
            }
            let bytes = stream
                .next()
                .await
                .expect("feed remains open")
                .expect("body chunk");
            buffer.push_str(std::str::from_utf8(&bytes).expect("SSE UTF8"));
        }
    })
    .await
    .expect("event deadline")
}

#[test]
fn observer_stream_is_event_driven_and_resumes_without_changing_session_identity() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use std::sync::Arc;
    use tokio_stream::StreamExt;
    use tower::ServiceExt;
    use ultraplexr_protocol::ShareRole;
    let daemon = RuntimeFixture::start();
    let owner = daemon.client();
    let id = SessionId::new();
    let terminal = owner
        .start_terminal(TerminalSessionSpec {
            session_id: id,
            mission_id: None,
            run_id: None,
            program: "/bin/cat".into(),
            args: vec![],
            cwd: daemon.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize {
                columns: 80,
                rows: 24,
            },
        })
        .expect("PTY");
    let process_id = owner.list_terminals().expect("initial index")[0].process_id;
    let (share, token) = owner
        .create_share(
            "Reconnect observer",
            ShareRole::Observer,
            vec![],
            vec![id],
            60,
        )
        .expect("Share");
    let relay = Arc::new(LoopbackRelay::new(daemon.root.join("s")));
    let observer =
        ControlClient::connect_with_connector(relay.clone(), Some(token)).expect("observer");
    let gateway = ultraplexr_observer::Observer::new(
        observer,
        "127.0.0.1:7777".into(),
        "0123456789abcdef0123456789abcdef".into(),
    )
    .expect("gateway");
    let router = gateway.clone().router();
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    runtime.block_on(async {
        let request = |path: String| {
            Request::builder()
                .uri(path)
                .header("host", "127.0.0.1:7777")
                .header("authorization", "Bearer 0123456789abcdef0123456789abcdef")
                .body(Body::empty())
                .expect("request")
        };
        let path = format!("/sessions/{id}/events");
        let response = router
            .clone()
            .oneshot(request(path.clone()))
            .await
            .expect("attach");
        assert!(response.status().is_success());
        let mut stream = response.into_body().into_data_stream();
        let mut buffer = String::new();
        assert_eq!(
            next_observer_event(&mut stream, &mut buffer).await.0,
            "frame"
        );
        // No new terminal output means no snapshot traffic (SSE keepalive is 15s).
        assert!(
            tokio::time::timeout(Duration::from_millis(650), stream.next())
                .await
                .is_err(),
            "idle frame polling is gone"
        );
        relay.disconnect();
        assert_eq!(
            next_observer_event(&mut stream, &mut buffer).await.0,
            "reconnecting"
        );
        terminal
            .paste(b"OFFLINE-FIRST-MARKER\n".to_vec(), true)
            .expect("output during outage");
        for line in 0..45 {
            terminal
                .paste(format!("offline-line-{line}\n").into_bytes(), true)
                .expect("retained output");
        }
        relay.resume();
        loop {
            let (event, data) = next_observer_event(&mut stream, &mut buffer).await;
            if event == "frame" {
                let frame: serde_json::Value = serde_json::from_str(&data).expect("frame");
                if frame["text"]
                    .as_str()
                    .expect("text")
                    .contains("offline-line-44")
                {
                    break;
                }
            }
        }
        assert_eq!(
            router
                .clone()
                .oneshot(request(format!("/sessions/{id}/history/100001")))
                .await
                .expect("invalid history offset")
                .status(),
            axum::http::StatusCode::BAD_REQUEST
        );
        let history_response = router
            .clone()
            .oneshot(request(format!("/sessions/{id}/history/100000")))
            .await
            .expect("history");
        assert!(history_response.status().is_success());
        let history = to_bytes(history_response.into_body(), 1024 * 1024)
            .await
            .expect("history body");
        // Locate a marker that has scrolled off-screen using the runtime's scoped
        // search, then prove history was not discarded by client disconnect.
        assert!(
            !owner
                .terminal(id)
                .search("OFFLINE-FIRST-MARKER", true, 10)
                .expect("durable history search")
                .is_empty()
        );
        assert!(
            serde_json::from_slice::<serde_json::Value>(&history).expect("history JSON")["text"]
                .as_str()
                .expect("history text")
                .contains("OFFLINE-FIRST-MARKER"),
            "Observer history must retrieve output retained across the outage"
        );

        // Simulate browser detach/reattach independently of native reconnect.
        drop(stream);
        terminal
            .paste(b"HTTP-DETACHED-MARKER\n".to_vec(), true)
            .expect("output while browser detached");
        let response = router
            .clone()
            .oneshot(request(path))
            .await
            .expect("reattach");
        let mut stream = response.into_body().into_data_stream();
        buffer.clear();
        loop {
            let (event, data) = next_observer_event(&mut stream, &mut buffer).await;
            if event == "frame" && data.contains("HTTP-DETACHED-MARKER") {
                break;
            }
        }
        let sessions = owner.list_terminals().expect("same index");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, id);
        assert_eq!(sessions[0].process_id, process_id);
        owner
            .revoke_share(share.share_id)
            .expect("revoke after reconnect");
        loop {
            let (event, _) = next_observer_event(&mut stream, &mut buffer).await;
            if event == "ended" {
                break;
            }
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .expect("revoked feed closes")
                .is_none()
        );
        gateway.shutdown();
    });
    terminal.kill().expect("cleanup PTY");
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "launches the real desktop twice; build ultraplexr-desktop first"]
fn native_desktop_restart_preserves_personal_state_and_live_sessions() {
    let daemon = RuntimeFixture::start();
    let owner = daemon.client();
    let mission = MissionId::new();
    let hidden = MissionId::new();
    for (id, name) in [(mission, "Visible project"), (hidden, "Dismissed project")] {
        owner
            .create_mission(id, name, Actor::human("test").expect("actor"))
            .expect("mission");
    }
    let id = SessionId::new();
    let terminal = owner
        .start_terminal(TerminalSessionSpec {
            session_id: id,
            mission_id: None,
            run_id: None,
            program: "/bin/cat".into(),
            args: vec![],
            cwd: daemon.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize {
                columns: 80,
                rows: 24,
            },
        })
        .expect("persistent PTY");
    owner
        .dispatch(
            mission,
            Command::StartSession {
                session_id: id,
                name: "Manual task".into(),
                started_by: Actor::human("test").expect("actor"),
            },
        )
        .expect("bind Session");
    let group = owner
        .create_session_group(SessionGroupSpec {
            group_id: SessionGroupId::new(),
            mission_id: Some(mission),
            name: "Manual task".into(),
            session_ids: vec![id],
            position: 0,
            pinned: true,
            detached: false,
        })
        .expect("group");
    let state = daemon.root.join("desktop");
    let store = WorkspaceStore::new(state.join("workspaces.json"));
    let mut document = WorkspaceDocument::new(
        17,
        18,
        Default::default(),
        Default::default(),
        Some(260),
        vec![WorkspaceRecord {
            id: 17,
            mission_id: Some(mission),
            title: "My renamed workspace".into(),
            pinned: true,
            selected_session: 0,
            focus_mode: false,
            sessions: vec![SessionRecord {
                group_id: Some(group.group_id),
                group_version: group.version,
                pinned: true,
                automatic_name: false,
                name: "Manual task".into(),
                actor: "test".into(),
                terminals: vec![id],
                pane_layout: None,
            }],
        }],
    );
    document.dismissals.missions.insert(hidden);
    store
        .save(&document)
        .expect("seed actual persisted presentation");
    let before = owner.list_terminals().expect("before")[0].clone();
    let binary = std::env::var_os("ULTRAPLEXR_DESKTOP_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ultraplexr-desktop")
        });
    assert!(binary.is_file(), "build the real desktop binary first");
    let newly_created = MissionId::new();
    for launch in 1..=2 {
        if launch == 2 {
            owner
                .create_mission(
                    newly_created,
                    "New project",
                    Actor::human("test").expect("actor"),
                )
                .expect("new work created while desktop is closed");
        }
        let log_path = daemon.root.join(format!("desktop-{launch}.log"));
        let log = std::fs::File::create(&log_path).expect("desktop log");
        let mut desktop = ProcessCommand::new(&binary)
            .args(["--connect-only", "--idle-benchmark", "--socket"])
            .arg(daemon.root.join("s"))
            .arg("--state-dir")
            .arg(&state)
            .stdout(Stdio::from(log.try_clone().expect("log")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("real desktop");
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = desktop.try_wait().expect("desktop status") {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = desktop.kill();
                let _ = desktop.wait();
                panic!(
                    "native desktop timed out: {}",
                    std::fs::read_to_string(&log_path).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let log = std::fs::read_to_string(log_path).expect("desktop evidence");
        assert!(status.success(), "desktop failed: {log}");
        assert!(
            log.contains("\"benchmark\":\"desktop_idle\""),
            "real first-render/idle benchmark did not run: {log}"
        );
        println!("native desktop launch {launch}: {log}");
        let restored = store
            .load()
            .expect("actual desktop save")
            .expect("document");
        assert_eq!(restored.active_workspace, 17);
        assert!(restored.dismissals.missions.contains(&hidden));
        assert!(
            !restored
                .workspaces
                .iter()
                .any(|view| view.mission_id == Some(hidden))
        );
        if launch == 2 {
            assert!(
                restored
                    .workspaces
                    .iter()
                    .any(|view| view.mission_id == Some(newly_created)),
                "dismissals must not hide newly created work"
            );
        }
        let view = restored
            .workspaces
            .iter()
            .find(|view| view.id == 17)
            .expect("original view");
        assert_eq!(view.title, "My renamed workspace");
        assert!(view.pinned);
        let session = view
            .sessions
            .iter()
            .find(|session| session.group_id == Some(group.group_id))
            .expect("shared group");
        assert_eq!(session.name, "Manual task");
        assert!(session.pinned);
        assert_eq!(session.terminals, vec![id]);
        let after = owner.list_terminals().expect("runtime after desktop exit");
        assert_eq!(
            after.len(),
            1,
            "desktop reconnect must not spawn another process"
        );
        assert_eq!(after[0].session_id, before.session_id);
        assert_eq!(after[0].process_id, before.process_id);
        assert_eq!(after[0].status, TerminalSessionStatus::Running);
    }
    terminal.kill().expect("fixture cleanup");
}

/// Alternate trusted byte-stream adapter used against the real daemon. The
/// loopback relay is test-only, not a proposed unauthenticated remote endpoint.
struct LoopbackRelay {
    socket: PathBuf,
    connections: std::sync::Mutex<Vec<std::net::TcpStream>>,
    online: std::sync::atomic::AtomicBool,
}
impl LoopbackRelay {
    fn new(socket: PathBuf) -> Self {
        Self {
            socket,
            connections: Default::default(),
            online: std::sync::atomic::AtomicBool::new(true),
        }
    }
    fn disconnect(&self) {
        self.online
            .store(false, std::sync::atomic::Ordering::Release);
        for stream in self.connections.lock().expect("relay sockets").drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
    fn resume(&self) {
        self.online
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
impl ultraplexr_client::transport::Connector for LoopbackRelay {
    fn connect(
        &self,
        client_id: uuid::Uuid,
    ) -> Result<ultraplexr_client::transport::Connection, ultraplexr_client::ClientError> {
        use std::{
            io,
            net::{Shutdown, TcpListener, TcpStream},
            os::unix::net::UnixStream,
        };
        use ultraplexr_protocol::{
            ProtocolError,
            wire_v3::{Hello, SyncWire},
        };
        if !self.online.load(std::sync::atomic::Ordering::Acquire) {
            return Err(
                io::Error::new(io::ErrorKind::ConnectionRefused, "test network offline").into(),
            );
        }
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let tcp = TcpStream::connect(listener.local_addr()?)?;
        let (mut peer, _) = listener.accept()?;
        let mut unix = UnixStream::connect(&self.socket)?;
        self.connections
            .lock()
            .expect("relay sockets")
            .push(tcp.try_clone()?);
        let mut peer_read = peer.try_clone()?;
        let mut unix_write = unix.try_clone()?;
        std::thread::spawn(move || {
            let _ = io::copy(&mut peer_read, &mut unix_write);
            let _ = unix_write.shutdown(Shutdown::Both);
        });
        std::thread::spawn(move || {
            let _ = io::copy(&mut unix, &mut peer);
            let _ = peer.shutdown(Shutdown::Both);
        });
        tcp.set_read_timeout(Some(Duration::from_secs(5)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut wire = SyncWire::new(tcp);
        wire.client_handshake(&Hello::new(client_id, "relay-test", client_id))
            .map_err(ProtocolError::from)?;
        wire.get_ref().set_read_timeout(None)?;
        wire.get_ref().set_write_timeout(None)?;
        let shutdown = wire.get_ref().try_clone()?;
        ultraplexr_client::transport::Connection::from_negotiated(
            wire,
            |stream| Ok((stream.try_clone()?, stream)),
            move || {
                let _ = shutdown.shutdown(Shutdown::Both);
            },
        )
    }
}

#[test]
fn observer_gateway_enforces_scope_revocation_and_no_writes_over_alternate_transport() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    use ultraplexr_protocol::ShareRole;
    let daemon = RuntimeFixture::start();
    let owner = daemon.client();
    let id = SessionId::new();
    let terminal = owner
        .start_terminal(TerminalSessionSpec {
            session_id: id,
            mission_id: None,
            run_id: None,
            program: "/bin/cat".into(),
            args: vec![],
            cwd: daemon.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize {
                columns: 80,
                rows: 24,
            },
        })
        .expect("PTY");
    let (share, token) = owner
        .create_share("Browser test", ShareRole::Observer, vec![], vec![id], 60)
        .expect("observer Share");
    let observer = ControlClient::connect_with_connector(
        Arc::new(LoopbackRelay::new(daemon.root.join("s"))),
        Some(token),
    )
    .expect("observer via TCP relay");
    assert!(observer.is_observer());
    assert!(
        observer.terminal(id).terminate().is_err(),
        "daemon rejects observer writes"
    );
    let access = "0123456789abcdef0123456789abcdef";
    assert!(
        ultraplexr_observer::Observer::new(
            owner.clone(),
            "127.0.0.1:7777".to_owned(),
            access.to_owned()
        )
        .is_err()
    );
    let router = ultraplexr_observer::Observer::new(
        observer,
        "127.0.0.1:7777".to_owned(),
        access.to_owned(),
    )
    .expect("observer gateway")
    .router();
    let runtime = tokio::runtime::Runtime::new().expect("HTTP test runtime");
    runtime.block_on(async {
        let request =
            |path: &str, authenticated: bool, host: &str, origin: Option<&str>, method: &str| {
                let mut request = Request::builder()
                    .uri(path)
                    .method(method)
                    .header("host", host);
                if authenticated {
                    request = request.header("authorization", format!("Bearer {access}"));
                }
                if let Some(origin) = origin {
                    request = request.header("origin", origin);
                }
                request.body(Body::empty()).expect("HTTP request")
            };
        for (path, auth, host, origin, method, expected) in [
            (
                "/sessions",
                false,
                "127.0.0.1:7777",
                None,
                "GET",
                StatusCode::UNAUTHORIZED,
            ),
            (
                "/sessions",
                true,
                "evil.example",
                None,
                "GET",
                StatusCode::FORBIDDEN,
            ),
            (
                "/sessions",
                true,
                "127.0.0.1:7777",
                Some("https://evil.example"),
                "GET",
                StatusCode::FORBIDDEN,
            ),
            (
                "/sessions",
                true,
                "127.0.0.1:7777",
                Some("http://127.0.0.1:7777"),
                "POST",
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                "/terminal-input",
                true,
                "127.0.0.1:7777",
                Some("http://127.0.0.1:7777"),
                "POST",
                StatusCode::NOT_FOUND,
            ),
        ] {
            let response = router
                .clone()
                .oneshot(request(path, auth, host, origin, method))
                .await
                .expect("HTTP response");
            assert_eq!(response.status(), expected, "{method} {path}");
            assert_eq!(response.headers()["cache-control"], "no-store");
        }
        let response = router
            .clone()
            .oneshot(request("/sessions", true, "127.0.0.1:7777", None, "GET"))
            .await
            .expect("list");
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("JSON body");
        let list: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(list["terminals"].as_array().expect("terminals").len(), 1);
        let denied_path = format!("/sessions/{}/events", SessionId::new());
        let denied = router
            .clone()
            .oneshot(request(&denied_path, true, "127.0.0.1:7777", None, "GET"))
            .await
            .expect("out of scope");
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let path = format!("/sessions/{id}/events");
        let response = router
            .clone()
            .oneshot(request(&path, true, "127.0.0.1:7777", None, "GET"))
            .await
            .expect("stream");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["content-type"]
                .to_str()
                .expect("content type")
                .starts_with("text/event-stream")
        );
        // Keep the stream open through revocation. The producer rechecks Share
        // authority at each capture and emits an ended event, then closes.
        owner.revoke_share(share.share_id).expect("revoke");
        let body = tokio::time::timeout(
            Duration::from_secs(5),
            to_bytes(response.into_body(), 256 * 1024),
        )
        .await
        .expect("revoked stream closes")
        .expect("stream body");
        assert!(String::from_utf8_lossy(&body).contains("event: ended"));
        let denied = router
            .oneshot(request("/sessions", true, "127.0.0.1:7777", None, "GET"))
            .await
            .expect("revoked list");
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    });
    terminal.kill().expect("stop fixture terminal");
}
use ultraplexr_core::{Actor, Command, MissionId, SessionId};
use ultraplexr_protocol::{
    SessionGroupChange, SessionGroupEvent, SessionGroupId, SessionGroupSpec, TerminalSessionSpec,
    TerminalSessionStatus,
};
use ultraplexr_terminal::GridSize;

/// Child process entry point; never touches the person's daemon or state.
#[test]
#[ignore = "spawned by the real-daemon lifecycle test"]
fn isolated_daemon() {
    let Some(root) = std::env::var_os("ULTRAPLEXR_LIFECYCLE_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    ultraplexr_server::run_blocking(root.join("s"), root.join("state"), None)
        .expect("isolated daemon");
}

struct RuntimeFixture {
    child: Child,
    root: PathBuf,
}

#[path = "browser_control_tests.rs"]
mod browser_control_tests;
#[path = "history_search_tests.rs"]
mod history_search_tests;
impl RuntimeFixture {
    fn start() -> Self {
        let root = PathBuf::from("/tmp").join(format!("up-lifecycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).expect("isolated root");
        let log = std::fs::File::create(root.join("daemon.log")).expect("daemon log");
        let child = ProcessCommand::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "workspace_lifecycle_tests::isolated_daemon",
                "--ignored",
                "--nocapture",
            ])
            .env("ULTRAPLEXR_LIFECYCLE_TEST_ROOT", &root)
            .stdout(Stdio::from(log.try_clone().expect("log handle")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("daemon child");
        let mut fixture = Self { child, root };
        let deadline = Instant::now() + Duration::from_secs(15);
        while !fixture.root.join("s").exists() {
            assert!(
                fixture.child.try_wait().expect("child status").is_none(),
                "daemon exited: {:?}",
                std::fs::read_to_string(fixture.root.join("daemon.log"))
            );
            assert!(Instant::now() < deadline, "daemon startup timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
        fixture
    }
    fn client(&self) -> ControlClient {
        ControlClient::connect(self.root.join("s")).expect("real client")
    }
}
impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn view(mission_id: MissionId) -> WorkspaceView {
    WorkspaceView {
        sessions: vec![history_placeholder()],
        selected_session: 0,
        focus_mode: false,
        mission_id: Some(mission_id),
        mission: None,
    }
}

#[test]
fn real_daemon_two_clients_reconcile_duplicate_views_and_reload_dismissals() {
    let daemon = RuntimeFixture::start();
    let owner = daemon.client();
    let other = daemon.client();
    let mission = MissionId::new();
    owner
        .create_mission(mission, "Lifecycle", Actor::human("test").expect("actor"))
        .expect("mission");
    let session_id = SessionId::new();
    let terminal = owner
        .start_terminal(TerminalSessionSpec {
            session_id,
            mission_id: None,
            run_id: None,
            program: "/bin/cat".into(),
            args: vec![],
            cwd: daemon.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize {
                columns: 80,
                rows: 24,
            },
        })
        .expect("real PTY");
    owner
        .dispatch(
            mission,
            Command::StartSession {
                session_id,
                name: "shell".to_owned(),
                started_by: Actor::human("test").expect("actor"),
            },
        )
        .expect("bind mission");
    let spec = SessionGroupSpec {
        group_id: SessionGroupId::new(),
        mission_id: Some(mission),
        name: "shell".to_owned(),
        session_ids: vec![session_id],
        position: 0,
        pinned: false,
        detached: false,
    };
    let initial = owner.create_session_group(spec.clone()).expect("group");
    let events = other.subscribe_session_groups(None).expect("subscription");
    let surfaces = HashMap::from([(session_id, 0)]);
    let mut tabs = WorkspaceTabs::new(vec![
        ("Original".to_owned(), view(mission)),
        ("Duplicate".to_owned(), view(mission)),
    ])
    .expect("views");
    let mut dismissals = ViewDismissals::default();
    let mut versions = HashMap::new();
    apply_group(
        &mut tabs,
        &initial,
        &surfaces,
        &dismissals,
        SessionStatus::Idle,
        &mut versions,
    );
    let renamed = owner
        .sync_session_group(
            spec.clone(),
            SessionGroupChange::Rename {
                name: "Task A".to_owned(),
            },
        )
        .expect("rename");
    let pinned = other
        .sync_session_group(spec, SessionGroupChange::SetPinned { pinned: true })
        .expect("pin from second client");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("broadcast");
        if let SessionGroupEvent::GroupChanged { group } = event {
            apply_group(
                &mut tabs,
                &group,
                &surfaces,
                &dismissals,
                SessionStatus::Idle,
                &mut versions,
            );
            if group.version == pinned.version {
                break;
            }
        }
    }
    // Delayed rename must not revert newer pin state in either duplicate.
    apply_group(
        &mut tabs,
        &renamed,
        &surfaces,
        &dismissals,
        SessionStatus::Idle,
        &mut versions,
    );
    for tab in tabs.tabs() {
        assert_eq!(tab.content().sessions.len(), 1);
        let session = &tab.content().sessions[0];
        assert_eq!(session.name, "Task A");
        assert!(session.pinned);
        assert_eq!(session.group_version, pinned.version);
    }

    let detached = other
        .update_session_group(
            pinned.group_id,
            pinned.version,
            SessionGroupChange::SetDetached { detached: true },
        )
        .expect("detach shared group");
    apply_group(
        &mut tabs,
        &detached,
        &surfaces,
        &dismissals,
        SessionStatus::Idle,
        &mut versions,
    );
    apply_group(
        &mut tabs,
        &pinned,
        &surfaces,
        &dismissals,
        SessionStatus::Idle,
        &mut versions,
    );
    assert!(
        tabs.tabs()
            .iter()
            .all(|tab| tab.content().sessions[0].terminals.is_empty()),
        "stale event cannot resurrect a detached group"
    );
    let attached = other
        .update_session_group(
            detached.group_id,
            detached.version,
            SessionGroupChange::SetDetached { detached: false },
        )
        .expect("reattach shared group");
    apply_group(
        &mut tabs,
        &attached,
        &surfaces,
        &dismissals,
        SessionStatus::Idle,
        &mut versions,
    );
    assert!(
        tabs.tabs()
            .iter()
            .all(|tab| tab.content().sessions[0].terminals == vec![0])
    );

    // A local hide survives presentation restart, without stopping the PTY.
    tabs.add("Personal tab".to_owned(), view(MissionId::new()));
    crate::workspace_reconcile::close_view(&mut tabs, &mut dismissals, WorkspaceId::from_raw(1))
        .expect("close duplicate view");
    assert!(
        dismissals.allows_mission(mission),
        "another view is still open"
    );
    crate::workspace_reconcile::close_view(&mut tabs, &mut dismissals, WorkspaceId::from_raw(2))
        .expect("close last view of mission");
    assert!(!dismissals.allows_mission(mission));
    let store = WorkspaceStore::new(daemon.root.join("view/workspaces.json"));
    let mut doc = WorkspaceDocument::new(
        3,
        4,
        Default::default(),
        Default::default(),
        None,
        vec![WorkspaceRecord {
            id: 3,
            mission_id: None,
            title: "Personal tab".to_owned(),
            pinned: true,
            selected_session: 0,
            focus_mode: false,
            sessions: vec![SessionRecord {
                group_id: None,
                group_version: 0,
                pinned: false,
                automatic_name: false,
                name: "mission history".to_owned(),
                actor: String::new(),
                terminals: vec![],
                pane_layout: None,
            }],
        }],
    );
    doc.dismissals = dismissals;
    store.save(&doc).expect("persist view");
    drop(owner);
    drop(other);
    let restarted = daemon.client();
    let loaded = store.load().expect("reload view").expect("saved document");
    assert_eq!(loaded, doc);
    assert!(!loaded.dismissals.allows_mission(mission));
    let fresh = MissionId::new();
    restarted
        .create_mission(fresh, "New CLI task", Actor::human("test").expect("actor"))
        .expect("new mission");
    assert!(crate::should_restore_discovered_mission(
        !loaded.dismissals.allows_mission(fresh),
        false,
        true
    ));
    assert!(
        restarted
            .list_terminals()
            .expect("live terminals")
            .iter()
            .any(|s| s.session_id == session_id && s.status == TerminalSessionStatus::Running)
    );
    let mut reopened = WorkspaceTabs::restore(
        vec![(
            WorkspaceId::from_raw(3),
            "Personal tab".to_owned(),
            true,
            view(fresh),
        )],
        WorkspaceId::from_raw(3),
    )
    .expect("reloaded tab model");
    let shared = restarted
        .list_session_groups(None)
        .expect("persisted runtime groups");
    for group in &shared {
        apply_group(
            &mut reopened,
            group,
            &surfaces,
            &loaded.dismissals,
            SessionStatus::Idle,
            &mut versions,
        );
    }
    assert_eq!(reopened.len(), 1);
    assert!(reopened.active().content().sessions[0].terminals.is_empty());
    assert_eq!(shared[0].name, "Task A");
    assert!(shared[0].pinned);
    terminal.terminate().expect("terminate fixture PTY");
    let deadline = Instant::now() + Duration::from_secs(5);
    while restarted
        .list_terminals()
        .expect("terminal state")
        .iter()
        .any(|s| s.session_id == session_id && s.status == TerminalSessionStatus::Running)
    {
        assert!(Instant::now() < deadline, "terminal did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
    terminal.archive().expect("archive ended terminal");
    assert!(
        !restarted
            .list_terminals()
            .expect("active index")
            .iter()
            .any(|s| s.session_id == session_id)
    );
    assert!(
        restarted
            .list_all_terminals()
            .expect("history index")
            .iter()
            .any(|s| s.session_id == session_id && s.archived)
    );
    terminal.restore().expect("restore archived history");
    assert!(
        restarted
            .list_terminals()
            .expect("restored index")
            .iter()
            .any(|s| s.session_id == session_id)
    );
}
