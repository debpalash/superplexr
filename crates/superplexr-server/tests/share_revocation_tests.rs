//! Real daemon/IPC/PTY coverage of authority changing while a request waits.
#[path = "share_revocation_tests/search_stream_tests.rs"]
mod search_stream_tests;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use superplexr_client::{ControlClient, DaemonSession};
use superplexr_core::SessionId;
use superplexr_protocol::{
    ClientRequest, Request, ResponseResult, ServerResponse,
    wire_v3::{FrameKind, Hello, SyncWire},
};
use superplexr_protocol::{ShareRole, ShareSummary, TerminalSessionSpec, TerminalWaitCondition};
use superplexr_terminal::GridSize;
use uuid::Uuid;

struct Process {
    root: PathBuf,
    child: Option<Child>,
}
impl Process {
    fn new() -> Self {
        let root = PathBuf::from("/tmp").join(format!("up-revoke-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("isolated root");
        let mut process = Self { root, child: None };
        process.start();
        process
    }
    fn start(&mut self) {
        let log = fs::File::create(self.root.join("daemon.log")).expect("fixture log");
        self.child = Some(
            Command::new(env!("CARGO_BIN_EXE_superplexr-server"))
                .arg("--socket")
                .arg(self.root.join("s"))
                .arg("--state-dir")
                .arg(self.root.join("state"))
                .stdout(Stdio::from(log.try_clone().expect("log")))
                .stderr(Stdio::from(log))
                .spawn()
                .expect("real daemon"),
        );
        until(|| {
            assert!(
                self.child
                    .as_mut()
                    .expect("daemon")
                    .try_wait()
                    .expect("status")
                    .is_none()
            );
            ControlClient::connect(self.root.join("s")).is_ok()
        });
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            child.wait().expect("reap fixture daemon");
        }
    }
    fn owner(&self) -> ControlClient {
        ControlClient::connect(self.root.join("s")).expect("owner")
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Fixture {
    process: Process,
    owner: ControlClient,
    terminal: DaemonSession,
}
impl Fixture {
    fn new() -> Self {
        let process = Process::new();
        let owner = process.owner();
        let terminal = owner
            .start_terminal(TerminalSessionSpec {
                session_id: SessionId::new(),
                mission_id: None,
                run_id: None,
                program: "/bin/cat".into(),
                args: vec![],
                cwd: process.root.clone(),
                environment_delta: BTreeMap::new(),
                grid: GridSize::new(80, 24).expect("grid"),
            })
            .expect("real PTY");
        terminal
            .paste(b"INITIAL\n".to_vec(), true)
            .expect("initial output");
        terminal
            .wait(text("INITIAL"), Duration::from_secs(3))
            .expect("ready frame");
        Self {
            process,
            owner,
            terminal,
        }
    }
    fn share(&self) -> (ShareSummary, String) {
        self.owner
            .create_share(
                "fixture observer",
                ShareRole::Observer,
                vec![],
                vec![self.terminal.id()],
                60,
            )
            .expect("Share")
    }
    fn observer(&self, token: &str) -> ControlClient {
        ControlClient::connect_with_share(self.process.root.join("s"), token).expect("observer")
    }
    fn waits(&self, count: usize) {
        until(|| {
            self.owner
                .runtime_diagnostics()
                .expect("diagnostics")
                .terminal_waits_active
                == count
        });
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.terminal.kill();
    }
}

fn text(query: &str) -> TerminalWaitCondition {
    TerminalWaitCondition::Text {
        query: query.into(),
        case_sensitive: true,
    }
}
#[track_caller]
fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "condition deadline");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn revocation_discards_in_flight_result_and_releases_wait_without_stopping_session() {
    let fixture = Fixture::new();
    let before = fixture.terminal.capture().expect("identity").terminal;
    let (share, token) = fixture.share();
    let session = fixture.observer(&token).terminal(fixture.terminal.id());
    let (send, receive) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = send.send(session.wait(text("AFTER-REVOKE"), Duration::from_secs(15)));
    });
    fixture.waits(1); // Real admission barrier, not a timing guess.
    fixture
        .owner
        .revoke_share(share.share_id)
        .expect("durable revocation");
    fixture
        .terminal
        .paste(b"AFTER-REVOKE\n".to_vec(), true)
        .expect("owner input");
    let result = receive
        .recv_timeout(Duration::from_secs(3))
        .expect("revoked request retires promptly");
    worker.join().expect("wait caller");
    assert!(
        result.is_err(),
        "revoked caller received the post-revocation capture"
    );
    fixture.waits(0);
    assert!(ControlClient::connect_with_share(fixture.process.root.join("s"), token).is_err());
    let after = fixture
        .terminal
        .wait(text("AFTER-REVOKE"), Duration::from_secs(3))
        .expect("owner remains live")
        .1
        .terminal;
    assert_eq!(after.session_id, before.session_id);
    assert_eq!(after.process_id, before.process_id);
}

#[test]
fn revocation_of_another_share_does_not_cancel_a_valid_wait() {
    let fixture = Fixture::new();
    let (_, token) = fixture.share();
    let (other, _) = fixture.share();
    let session = fixture.observer(&token).terminal(fixture.terminal.id());
    let worker =
        thread::spawn(move || session.wait(text("STILL-AUTHORIZED"), Duration::from_secs(5)));
    fixture.waits(1);
    fixture
        .owner
        .revoke_share(other.share_id)
        .expect("unrelated revocation");
    fixture
        .terminal
        .paste(b"STILL-AUTHORIZED\n".to_vec(), true)
        .expect("owner input");
    worker
        .join()
        .expect("caller")
        .expect("unrelated Share remains authorized");
    fixture.waits(0);
}

#[test]
fn persisted_near_expiry_cancels_an_admitted_wait_after_restart() {
    let mut fixture = Fixture::new();
    let (share, token) = fixture.share();
    fixture.process.stop();
    // Model a restart near the end of a valid Share's lifetime without adding
    // a production short-TTL option or a minute-long sleep to the test suite.
    // Only this stopped fixture's private persisted record is adjusted.
    let path = fixture.process.root.join("state/shares.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("stored Share")).expect("JSON");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_micros() as u64;
    stored["shares"][share.share_id.to_string()]["summary"]["expires_at_micros"] =
        (now + 3_000_000).into();
    fs::write(path, serde_json::to_vec(&stored).expect("fixture JSON"))
        .expect("near-expiry fixture");
    fixture.process.start();
    fixture.owner = fixture.process.owner();
    let session = fixture.observer(&token).terminal(fixture.terminal.id());
    let (send, receive) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = send.send(session.wait(
            TerminalWaitCondition::Quiet {
                quiet_millis: 10_000,
            },
            Duration::from_secs(15),
        ));
    });
    fixture.waits(1);
    let result = receive
        .recv_timeout(Duration::from_secs(5))
        .expect("expiry cancels without waiting for quiet completion");
    worker.join().expect("caller");
    assert!(result.is_err());
    fixture.waits(0);
    assert!(ControlClient::connect_with_share(fixture.process.root.join("s"), token).is_err());
}

fn raw_client(fixture: &Fixture, client_id: Uuid) -> SyncWire<std::os::unix::net::UnixStream> {
    let socket = std::os::unix::net::UnixStream::connect(fixture.process.root.join("s"))
        .expect("raw connection");
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("read bound");
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .expect("write bound");
    let mut wire = SyncWire::new(socket);
    wire.client_handshake(&Hello::new(client_id, "authority-fixture", Uuid::new_v4()))
        .expect("handshake");
    wire
}

#[test]
fn spoofed_client_identity_cannot_release_owner_control_on_disconnect() {
    let fixture = Fixture::new();
    let (_, token) = fixture.share();
    let before = fixture.terminal.capture().expect("owner lease").terminal;
    let owner_id = before.controller_client_id.expect("owner controls");
    for token in ["invalid-capability".to_owned(), token] {
        let baseline = fixture
            .owner
            .runtime_diagnostics()
            .expect("baseline")
            .open_connections;
        let mut wire = raw_client(&fixture, owner_id);
        let mut request = ClientRequest::for_client(owner_id, Request::Ping);
        request.share_token = Some(token);
        wire.send_json(FrameKind::Request, 0, &request)
            .expect("request");
        let _: ServerResponse = wire
            .receive_json(FrameKind::Response, 0)
            .expect("admission response");
        drop(wire);
        until(|| {
            fixture
                .owner
                .runtime_diagnostics()
                .expect("connection cleanup")
                .open_connections
                <= baseline
        });
        let after = fixture
            .terminal
            .capture()
            .expect("retained owner lease")
            .terminal;
        assert_eq!(
            after.controller_client_id, before.controller_client_id,
            "untrusted or differently scoped disconnect released owner's lease"
        );
        assert_eq!(after.control_epoch, before.control_epoch);
    }
}

#[test]
fn spoofed_owner_coordinates_cannot_grant_a_shared_controller_its_lease() {
    let fixture = Fixture::new();
    let before = fixture.terminal.capture().expect("owner lease").terminal;
    let owner_id = before.controller_client_id.expect("owner controls");
    let (_, token) = fixture
        .owner
        .create_share(
            "controller fixture",
            ShareRole::Controller,
            vec![],
            vec![fixture.terminal.id()],
            60,
        )
        .expect("Controller Share");
    let mut wire = raw_client(&fixture, owner_id);
    for action in [
        Request::ClaimTerminalControl {
            session_id: fixture.terminal.id(),
            force: false,
        },
        Request::ReleaseTerminalControl {
            session_id: fixture.terminal.id(),
        },
        Request::TerminalPaste {
            session_id: fixture.terminal.id(),
            bytes: b"SPOOFED-INPUT\n".to_vec(),
            confirmed: true,
        },
    ] {
        let mut request = ClientRequest::for_client(owner_id, action);
        request.surface_id = before.controller_surface_id;
        request.control_epoch = Some(before.control_epoch);
        request.share_token = Some(token.clone());
        wire.send_json(FrameKind::Request, 0, &request)
            .expect("request");
        let response: ServerResponse = wire.receive_json(FrameKind::Response, 0).expect("response");
        assert!(
            matches!(response.result, ResponseResult::Error { .. }),
            "Share identity must be part of the Control lease, not just client/surface/epoch"
        );
    }
    let after = fixture.terminal.capture().expect("owner lease unchanged");
    assert_eq!(after.terminal.controller_share_id, None);
    assert_eq!(after.terminal.control_epoch, before.control_epoch);
    assert!(
        !after
            .frame
            .rows
            .iter()
            .any(|row| row.text().contains("SPOOFED-INPUT"))
    );
}
