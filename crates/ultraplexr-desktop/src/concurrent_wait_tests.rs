use super::*;
use std::os::unix::net::UnixStream;
use ultraplexr_protocol::{
    ClientRequest, Request as Rpc, ResponseBody, ResponseResult, ServerResponse,
    TerminalWaitCondition,
    wire_v3::{FrameKind, Hello, SyncWire},
};
use uuid::Uuid;

fn body(response: &ServerResponse) -> &ResponseBody {
    match &response.result {
        ResponseResult::Success { body } => body,
        result => panic!("unexpected RPC error: {result:?}"),
    }
}

fn wait_active(owner: &ControlClient, expected: usize) {
    let end = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let actual = owner.runtime_diagnostics().unwrap().terminal_waits_active;
        if actual == expected {
            return;
        }
        assert!(
            std::time::Instant::now() < end,
            "active waits: {actual}, expected {expected}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn exit_wait(session_id: SessionId) -> Rpc {
    Rpc::TerminalWait {
        session_id,
        condition: TerminalWaitCondition::Exit,
        timeout_millis: 30_000,
    }
}

fn capacity(response: &ServerResponse, id: Uuid) {
    assert_eq!(response.request_id, id);
    assert!(
        matches!(&response.result, ResponseResult::Error { code, .. } if code == "wait_capacity")
    );
}

struct Peer {
    wire: SyncWire<UnixStream>,
    client: Uuid,
    surface: Uuid,
    epoch: Option<u64>,
    token: Option<String>,
}
impl Peer {
    fn new(fixture: &RuntimeFixture, token: Option<String>) -> Self {
        let socket = UnixStream::connect(fixture.root.join("s")).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut wire = SyncWire::new(socket);
        let client = Uuid::new_v4();
        wire.client_handshake(&Hello::new(client, "wait-regression", client))
            .unwrap();
        Self {
            wire,
            client,
            surface: Uuid::new_v4(),
            epoch: None,
            token,
        }
    }
    fn send(&mut self, action: Rpc) -> Uuid {
        let mut request = ClientRequest::for_client(self.client, action);
        request.surface_id = Some(self.surface);
        request.control_epoch = self.epoch;
        request.share_token = self.token.clone();
        self.wire
            .send_json(FrameKind::Request, 0, &request)
            .unwrap();
        request.request_id
    }
    fn receive(&mut self) -> ServerResponse {
        self.wire
            .receive_json(FrameKind::Response, 0)
            .expect("response without head-of-line blocking")
    }
    fn claim(&mut self, session_id: SessionId) {
        let id = self.send(Rpc::ClaimTerminalControl {
            session_id,
            force: false,
        });
        let response = self.receive();
        assert_eq!(response.request_id, id);
        let ResponseBody::TerminalControlChanged { control_epoch, .. } = body(&response) else {
            panic!("Control ACK")
        };
        self.epoch = Some(*control_epoch);
    }
}

#[test]
fn concurrent_wait_allows_same_wire_input_and_correlates_out_of_order_responses() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let pid = session.capture().unwrap().terminal.process_id;
    session.release_control().unwrap();
    let mut peer = Peer::new(&fixture, None);
    peer.claim(session.id());
    let wait = peer.send(Rpc::TerminalWait {
        session_id: session.id(),
        condition: TerminalWaitCondition::Text {
            query: "SAME-WIRE-INPUT 界".into(),
            case_sensitive: true,
        },
        timeout_millis: 30_000,
    });
    let ping = peer.send(Rpc::Ping);
    let response = peer.receive();
    assert_eq!(
        response.request_id, ping,
        "Ping must not wait for the earlier wait RPC"
    );
    let input = peer.send(Rpc::TerminalPaste {
        session_id: session.id(),
        bytes: "SAME-WIRE-INPUT 界\n".as_bytes().to_vec(),
        confirmed: true,
    });
    let mut responses = [peer.receive(), peer.receive()];
    responses.sort_by_key(|response| response.request_id);
    let waited = responses
        .iter()
        .find(|response| response.request_id == wait)
        .unwrap();
    assert!(
        matches!(body(waited), ResponseBody::TerminalWaitSatisfied { capture, .. } if capture.terminal.process_id == pid)
    );
    assert!(matches!(
        body(
            responses
                .iter()
                .find(|response| response.request_id == input)
                .unwrap()
        ),
        ResponseBody::TerminalCommandAccepted { .. }
    ));
}

#[test]
fn concurrent_wait_keeps_control_release_ahead_of_later_input() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    session.release_control().unwrap();
    let mut peer = Peer::new(&fixture, None);
    peer.claim(session.id());
    peer.send(exit_wait(session.id()));
    wait_active(&owner, 1);
    let release = peer.send(Rpc::ReleaseTerminalControl {
        session_id: session.id(),
    });
    let stale = peer.send(Rpc::TerminalPaste {
        session_id: session.id(),
        bytes: b"STALE-AFTER-RELEASE\n".to_vec(),
        confirmed: true,
    });
    let released = peer.receive();
    assert_eq!(released.request_id, release);
    assert!(matches!(
        body(&released),
        ResponseBody::TerminalControlChanged { .. }
    ));
    let rejected = peer.receive();
    assert_eq!(rejected.request_id, stale);
    assert!(matches!(rejected.result, ResponseResult::Error { .. }));
    peer.claim(session.id());
    let fresh = peer.send(Rpc::TerminalPaste {
        session_id: session.id(),
        bytes: b"FRESH-AFTER-RELEASE\n".to_vec(),
        confirmed: true,
    });
    let response = peer.receive();
    assert_eq!(response.request_id, fresh);
    assert!(matches!(
        body(&response),
        ResponseBody::TerminalCommandAccepted { .. }
    ));
    wait_text(&session, "FRESH-AFTER-RELEASE");
    assert!(
        session
            .search("STALE-AFTER-RELEASE", true, 1)
            .unwrap()
            .is_empty()
    );
    drop(peer);
    wait_active(&owner, 0);
}

#[test]
fn concurrent_wait_connection_limit_preserves_ping_and_exit_then_readmission() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let mut peer = Peer::new(&fixture, None);
    let waits: Vec<_> = (0..8).map(|_| peer.send(exit_wait(session.id()))).collect();
    wait_active(&owner, 8);
    let overflow = peer.send(exit_wait(session.id()));
    capacity(&peer.receive(), overflow);
    let ping = peer.send(Rpc::Ping);
    assert_eq!(peer.receive().request_id, ping);
    session.kill().unwrap();
    let mut completed = std::collections::HashSet::new();
    for _ in 0..8 {
        let response = peer.receive();
        assert!(waits.contains(&response.request_id));
        assert!(completed.insert(response.request_id));
        assert!(matches!(
            body(&response),
            ResponseBody::TerminalWaitSatisfied { .. }
        ));
    }
    wait_active(&owner, 0);
    let again = peer.send(exit_wait(session.id()));
    let response = peer.receive();
    assert_eq!(response.request_id, again);
    assert!(matches!(
        body(&response),
        ResponseBody::TerminalWaitSatisfied { .. }
    ));
}

#[test]
fn concurrent_wait_process_limit_cannot_be_bypassed_by_more_connections() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    // No readiness wait: even after its response, its native forwarder may
    // correctly retain a permit until the thread exits. Measure a clean lane.
    let session = owner
        .start_terminal(TerminalSessionSpec {
            session_id: SessionId::new(),
            mission_id: None,
            run_id: None,
            program: "/bin/cat".into(),
            args: vec![],
            cwd: fixture.root.clone(),
            environment_delta: BTreeMap::new(),
            grid: GridSize::new(80, 24).unwrap(),
        })
        .unwrap();
    let mut peers: Vec<_> = (0..4).map(|_| Peer::new(&fixture, None)).collect();
    for peer in &mut peers {
        for _ in 0..8 {
            peer.send(exit_wait(session.id()));
        }
    }
    wait_active(&owner, 32);
    let mut extra = Peer::new(&fixture, None);
    let overflow = extra.send(exit_wait(session.id()));
    capacity(&extra.receive(), overflow);
    drop(peers);
    wait_active(&owner, 0);
    extra.send(exit_wait(session.id()));
    let ping = extra.send(Rpc::Ping);
    assert_eq!(extra.receive().request_id, ping);
    wait_active(&owner, 1);
    drop(extra);
    wait_active(&owner, 0);
    assert_eq!(
        session.capture().unwrap().terminal.status,
        ultraplexr_protocol::TerminalSessionStatus::Running,
    );
}

#[test]
fn concurrent_wait_disconnect_and_revocation_retire_active_observation_without_killing_pty() {
    for revoke in [false, true] {
        let fixture = RuntimeFixture::start();
        let owner = fixture.client();
        let session = terminal(&owner, &fixture);
        let pid = session.capture().unwrap().terminal.process_id;
        let (share, token) = owner
            .create_share(
                "wait observer",
                ShareRole::Observer,
                vec![],
                vec![session.id()],
                60,
            )
            .unwrap();
        let mut peer = Peer::new(&fixture, Some(token));
        peer.send(Rpc::TerminalWait {
            session_id: session.id(),
            condition: TerminalWaitCondition::Text {
                query: "DO-NOT-DELIVER-AFTER-RETIRE".into(),
                case_sensitive: true,
            },
            timeout_millis: 30_000,
        });
        wait_active(&owner, 1);
        if revoke {
            owner.revoke_share(share.share_id).unwrap();
            assert!(
                peer.wire
                    .receive_json::<ServerResponse>(FrameKind::Response, 0)
                    .is_err()
            );
        }
        drop(peer);
        wait_active(&owner, 0);
        session
            .paste(b"OWNER-STILL-ACTIVE\n".to_vec(), true)
            .unwrap();
        wait_text(&session, "OWNER-STILL-ACTIVE");
        assert_eq!(session.capture().unwrap().terminal.process_id, pid);
    }
}

#[test]
fn concurrent_wait_invalid_scope_and_timeout_do_not_poison_following_requests() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let session = terminal(&owner, &fixture);
    let outside = terminal(&owner, &fixture);
    let (_, token) = owner
        .create_share(
            "wait scope",
            ShareRole::Observer,
            vec![],
            vec![session.id()],
            60,
        )
        .unwrap();
    let mut peer = Peer::new(&fixture, Some(token));
    let denied = peer.send(exit_wait(outside.id()));
    let response = peer.receive();
    assert_eq!(response.request_id, denied);
    assert!(
        matches!(response.result, ResponseResult::Error { ref code, .. } if code == "share_request_denied")
    );
    for timeout_millis in [0, 3_600_001] {
        let invalid = peer.send(Rpc::TerminalWait {
            session_id: session.id(),
            condition: TerminalWaitCondition::Exit,
            timeout_millis,
        });
        let response = peer.receive();
        assert_eq!(response.request_id, invalid);
        assert!(matches!(response.result, ResponseResult::Error { .. }));
    }
    let timeout = peer.send(Rpc::TerminalWait {
        session_id: session.id(),
        condition: TerminalWaitCondition::Exit,
        timeout_millis: 50,
    });
    let response = peer.receive();
    assert_eq!(response.request_id, timeout);
    assert!(
        matches!(response.result, ResponseResult::Error { ref message, .. } if message.contains("timed out"))
    );
    let ping = peer.send(Rpc::Ping);
    assert_eq!(peer.receive().request_id, ping);
    wait_active(&owner, 0);
}
