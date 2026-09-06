use super::*;
use std::{
    fs::{self, DirBuilder},
    os::unix::{fs::DirBuilderExt, net::UnixListener},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};
use ultraplexr_core::{MissionId, RunId};
use ultraplexr_protocol::{
    ClientRequest, Request, ResponseBody, ServerResponse, ShareRole, ShareSummary,
    wire_v3::{FrameKind, SyncWire},
};

/// The real native client connects and negotiates capabilities. The peer records
/// all non-handshake requests and rejects them promptly, so an accidental RPC
/// cannot masquerade as an argument-validation failure or hang for its timeout.
struct NativeFixture {
    backend: Option<ControlBackend>,
    calls: Arc<Mutex<Vec<Request>>>,
    peer: Option<JoinHandle<()>>,
    directory: PathBuf,
}

impl NativeFixture {
    fn new(terminal_read: bool, workflow_read: bool, shared: bool) -> Self {
        let directory =
            std::env::temp_dir().join(format!("um-{}", MissionId::new().as_uuid().simple()));
        DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let socket = directory.join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let peer = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut wire = SyncWire::new(stream);
            wire.server_handshake(MissionId::new().as_uuid()).unwrap();
            while let Ok(request) = wire.receive_json::<ClientRequest>(FrameKind::Request, 0) {
                let response = match request.action {
                    Request::Ping => {
                        ServerResponse::success(request.request_id, ResponseBody::Pong)
                    }
                    Request::ShareIdentity if shared => ServerResponse::success(
                        request.request_id,
                        ResponseBody::ShareIdentity {
                            share: ShareSummary {
                                share_id: MissionId::new().as_uuid(),
                                label: "inspection fixture".into(),
                                role: ShareRole::Observer,
                                mission_ids: vec![],
                                session_ids: vec![],
                                created_at_micros: 1,
                                expires_at_micros: u64::MAX,
                                revoked_at_micros: None,
                            },
                        },
                    ),
                    action => {
                        recorded.lock().unwrap().push(action);
                        ServerResponse::error(
                            request.request_id,
                            "fixture_rpc",
                            "native RPC reached",
                        )
                    }
                };
                if wire.send_json(FrameKind::Response, 0, &response).is_err() {
                    break;
                }
            }
        });
        let client = if shared {
            ControlClient::connect_with_share(&socket, "fixture-share")
        } else {
            ControlClient::connect(&socket)
        }
        .unwrap();
        Self {
            backend: Some(ControlBackend {
                client,
                read_only: shared,
                terminal_read,
                workflow_read,
            }),
            calls,
            peer: Some(peer),
            directory,
        }
    }

    fn backend(&self) -> &ControlBackend {
        self.backend.as_ref().unwrap()
    }

    fn request(&self, method: &str, params: Value) -> Value {
        let line = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
        serde_json::from_str(&handle_line(self.backend(), &line.to_string()).unwrap()).unwrap()
    }

    fn call(&self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name":name,"arguments":arguments}))
    }

    fn no_calls(&self) {
        assert!(
            self.calls.lock().unwrap().is_empty(),
            "unexpected native RPC"
        );
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        self.backend.take();
        if let Some(peer) = self.peer.take() {
            let _ = peer.join();
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn error_text(response: &Value) -> &str {
    assert_eq!(response["result"]["isError"], true, "{response}");
    response["result"]["content"][0]["text"].as_str().unwrap()
}

#[test]
fn share_token_argument_requires_either_explicit_inspection_module() {
    for args in [
        vec!["ultraplexr-mcp", "--share-token-file", "/private/token"],
        vec![
            "ultraplexr-mcp",
            "--read-only",
            "--share-token-file",
            "/private/token",
        ],
    ] {
        let error = Args::try_parse_from(args).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
    }
    for (terminal, workflow) in [(true, false), (false, true), (true, true)] {
        let mut argv = vec!["ultraplexr-mcp", "--share-token-file", "/private/token"];
        if terminal {
            argv.push("--terminal-read");
        }
        if workflow {
            argv.push("--workflow-read");
        }
        let args = Args::try_parse_from(argv).unwrap();
        assert_eq!(
            (args.terminal_read, args.workflow_read),
            (terminal, workflow)
        );
        assert_eq!(
            args.share_token_file.as_deref(),
            Some(std::path::Path::new("/private/token"))
        );
    }
    let defaults = Args::try_parse_from(["ultraplexr-mcp"]).unwrap();
    assert!(!defaults.terminal_read && !defaults.workflow_read);
}

#[test]
fn inspection_modules_are_independent_and_disabled_tools_reject_direct_calls() {
    for shared in [false, true] {
        for (terminal, workflow) in [(false, false), (true, false), (false, true), (true, true)] {
            let fixture = NativeFixture::new(terminal, workflow, shared);
            let listed = fixture.request("tools/list", json!({}));
            let names: Vec<_> = listed["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap())
                .collect();
            for name in ["verification_list", "verification_status"] {
                assert_eq!(names.contains(&name), workflow);
                if !workflow {
                    assert!(error_text(&fixture.call(name, json!({}))).contains("--workflow-read"));
                }
            }
            for name in ["terminal_list", "terminal_capture", "terminal_history"] {
                assert_eq!(names.contains(&name), terminal);
                if !terminal {
                    assert!(error_text(&fixture.call(name, json!({}))).contains("--terminal-read"));
                }
            }
            assert_eq!(names.contains(&"fault_list"), !shared);
            if shared {
                for name in [
                    "fault_list",
                    "fault_show",
                    "fault_repro",
                    "fault_guard",
                    "fault_resolve",
                    "fault_dismiss",
                    "fault_report",
                ] {
                    assert!(!names.contains(&name));
                    assert!(error_text(&fixture.call(name, json!({}))).contains("Share-connected"));
                }
            }
            fixture.no_calls();
        }
    }
}

#[test]
fn malformed_verification_list_arguments_are_rejected_before_native_rpc() {
    let fixture = NativeFixture::new(false, true, false);
    let mission = MissionId::new().to_string();
    for (args, expected) in [
        (json!({}), "mission_id must be a UUID string"),
        (json!({"mission_id":42}), "mission_id must be a UUID string"),
        (
            json!({"mission_id":"short"}),
            "mission_id must be a UUID string",
        ),
        (json!({"mission_id":"z".repeat(36)}), "Invalid Mission ID"),
        (
            json!({"mission_id":mission,"verifier_run_id":RunId::new()}),
            "Unknown workflow inspection argument",
        ),
        (
            json!({"mission_id":mission,"after":null}),
            "after must be a UUID string",
        ),
        (
            json!({"mission_id":mission,"after":12}),
            "after must be a UUID string",
        ),
        (
            json!({"mission_id":mission,"after":"short"}),
            "after must be a UUID string",
        ),
        (
            json!({"mission_id":mission,"after":"z".repeat(36)}),
            "Invalid page cursor",
        ),
    ] {
        assert_eq!(
            error_text(&fixture.call("verification_list", args)),
            expected
        );
    }
    for limit in [
        json!(0),
        json!(65),
        json!(-1),
        json!(1.5),
        json!("32"),
        json!(true),
        Value::Null,
    ] {
        assert_eq!(
            error_text(&fixture.call(
                "verification_list",
                json!({"mission_id":mission,"limit":limit})
            )),
            "limit must be an integer between 1 and 64"
        );
    }
    fixture.no_calls();
}

#[test]
fn malformed_verification_status_and_nonobject_arguments_never_reach_native_rpc() {
    let fixture = NativeFixture::new(false, true, false);
    let mission = MissionId::new().to_string();
    for (args, expected) in [
        (json!({}), "mission_id must be a UUID string"),
        (json!({"mission_id":"z".repeat(36)}), "Invalid Mission ID"),
        (
            json!({"mission_id":mission}),
            "verifier_run_id must be a UUID string",
        ),
        (
            json!({"mission_id":mission,"verifier_run_id":null}),
            "verifier_run_id must be a UUID string",
        ),
        (
            json!({"mission_id":mission,"verifier_run_id":"short"}),
            "verifier_run_id must be a UUID string",
        ),
        (
            json!({"mission_id":mission,"verifier_run_id":"z".repeat(36)}),
            "Invalid verifier Run ID",
        ),
        (
            json!({"mission_id":mission,"verifier_run_id":RunId::new(),"limit":1}),
            "Unknown workflow inspection argument",
        ),
    ] {
        assert_eq!(
            error_text(&fixture.call("verification_status", args)),
            expected
        );
    }
    for name in ["verification_list", "verification_status"] {
        for args in [Value::Null, json!([]), json!("not an object"), json!(false)] {
            assert_eq!(fixture.call(name, args)["error"]["code"], -32602);
        }
    }
    fixture.no_calls();
}

#[test]
fn valid_workflow_arguments_reach_the_native_boundary_with_their_exact_bounds() {
    let fixture = NativeFixture::new(false, true, false);
    let mission = MissionId::new();
    let verifier = RunId::new();
    let after = RunId::new();
    for args in [
        json!({"mission_id":mission}),
        json!({"mission_id":mission,"after":after,"limit":1}),
        json!({"mission_id":mission,"limit":64}),
    ] {
        assert!(
            error_text(&fixture.call("verification_list", args)).contains("native RPC reached")
        );
    }
    assert!(
        error_text(&fixture.call(
            "verification_status",
            json!({"mission_id":mission,"verifier_run_id":verifier})
        ))
        .contains("native RPC reached")
    );
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert!(
        matches!(calls[0], Request::VerificationCatalog { mission_id, after: None, limit: 32 } if mission_id == mission)
    );
    assert!(
        matches!(calls[1], Request::VerificationCatalog { mission_id, after: Some(cursor), limit: 1 } if mission_id == mission && cursor == after)
    );
    assert!(
        matches!(calls[2], Request::VerificationCatalog { mission_id, after: None, limit: 64 } if mission_id == mission)
    );
    assert!(
        matches!(calls[3], Request::VerificationStatus { mission_id, verifier_run_id } if mission_id == mission && verifier_run_id == verifier)
    );
}
