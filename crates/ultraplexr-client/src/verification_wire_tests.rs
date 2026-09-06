use super::*;
use std::{
    collections::VecDeque,
    fs::{self, DirBuilder},
    os::unix::{fs::DirBuilderExt, net::UnixListener},
};
use ultraplexr_core::{
    ArtifactId, EvaluationVerdict, FinishOutcome, ReceiptStatus, RunDisposition, RunId, RunPhase,
    VerificationCatalog, VerificationNextAction, VerificationStatus, VerifierSummary,
};
use ultraplexr_protocol::{
    VERIFICATION_CATALOG_FEATURE, VERIFICATION_STATUS_FEATURE,
    wire_v3::{Hello, MAX_PAYLOAD_BYTES, MAX_UNCOMPRESSED_BYTES, Welcome, WireLimits},
};

struct PeerFixture {
    client: Option<ControlClient>,
    calls: Arc<Mutex<Vec<Request>>>,
    replies: Arc<Mutex<VecDeque<ResponseBody>>>,
    peer: Option<thread::JoinHandle<()>>,
    directory: PathBuf,
}

impl PeerFixture {
    fn new(status: bool, catalog: bool, replies: Vec<ResponseBody>) -> Self {
        let directory = std::env::temp_dir().join(format!("uv-{}", Uuid::new_v4().simple()));
        DirBuilder::new().mode(0o700).create(&directory).unwrap();
        let socket = directory.join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let replies: Arc<Mutex<VecDeque<_>>> = Arc::new(Mutex::new(replies.into()));
        let queued = replies.clone();
        let peer = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut wire = SyncWire::new(stream);
            let hello: Hello = wire.receive_json(FrameKind::Hello, 0).unwrap();
            // Negotiate real feature flags, without compression, so missing
            // workflow capabilities are visible to the production receiver.
            let features = hello
                .features
                .into_iter()
                .filter(|feature| {
                    feature != "zstd"
                        && (status || feature != VERIFICATION_STATUS_FEATURE)
                        && (catalog || feature != VERIFICATION_CATALOG_FEATURE)
                })
                .collect();
            let welcome = Welcome {
                runtime_id: Uuid::new_v4(),
                runtime_version: "fixture".into(),
                selected_minor: hello.protocol.max_minor,
                features,
                connection_id: Uuid::new_v4(),
                limits: WireLimits {
                    max_payload_bytes: MAX_PAYLOAD_BYTES as u32,
                    max_uncompressed_bytes: MAX_UNCOMPRESSED_BYTES as u32,
                },
                server_time_unix_micros: 1,
            };
            wire.send_json(FrameKind::Welcome, 0, &welcome).unwrap();
            while let Ok(request) = wire.receive_json::<ClientRequest>(FrameKind::Request, 0) {
                let response = if matches!(request.action, Request::Ping) {
                    ServerResponse::success(request.request_id, ResponseBody::Pong)
                } else {
                    recorded.lock().unwrap().push(request.action);
                    match queued.lock().unwrap().pop_front() {
                        Some(body) => ServerResponse::success(request.request_id, body),
                        None => ServerResponse::error(
                            request.request_id,
                            "unexpected_rpc",
                            "unexpected native request",
                        ),
                    }
                };
                if wire.send_json(FrameKind::Response, 0, &response).is_err() {
                    break;
                }
            }
        });
        let client = ControlClient::connect(&socket).unwrap();
        Self {
            client: Some(client),
            calls,
            replies,
            peer: Some(peer),
            directory,
        }
    }

    fn client(&self) -> &ControlClient {
        self.client.as_ref().unwrap()
    }

    fn assert_consumed(&self, expected_calls: usize) {
        let calls = self.calls.lock().unwrap();
        assert_eq!(calls.len(), expected_calls);
        assert!(
            calls.iter().all(|request| matches!(
                request,
                Request::VerificationStatus { .. } | Request::VerificationCatalog { .. }
            )),
            "no GetMission fallback or unrelated RPC"
        );
        assert!(self.replies.lock().unwrap().is_empty());
    }
}

impl Drop for PeerFixture {
    fn drop(&mut self) {
        self.client.take();
        if let Some(peer) = self.peer.take() {
            let _ = peer.join();
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn receipt(id: u128) -> ReceiptStatus {
    ReceiptStatus {
        artifact_id: ArtifactId::from_uuid(Uuid::from_u128(id)),
        verdict: EvaluationVerdict::Passed,
        checks: 2,
        passing_checks: 2,
        delivery_validated: true,
        repeatable: true,
        passes_recorded_candidate: true,
    }
}

fn status(mission: MissionId, verifier: RunId, count: usize) -> VerificationStatus {
    VerificationStatus {
        mission_id: mission,
        mission_version: 20,
        verifier_run_id: verifier,
        subject_run_id: RunId::new(),
        candidate_revision: "frozen-candidate".into(),
        candidate_sha256: "a".repeat(64),
        phase: RunPhase::Finished,
        outcome: Some(FinishOutcome::Succeeded),
        execution_finished: true,
        primary_session_id: None,
        primary_session_status: None,
        subject_disposition: RunDisposition::AwaitingReview,
        receipt_count: count,
        passing_receipt_count: count,
        receipts: (1..=count.min(16)).map(|id| receipt(id as u128)).collect(),
        receipts_truncated: count > 16,
        next_action: VerificationNextAction::InspectRecordedReceipts,
        evidence_rechecked: false,
        observation_only: true,
    }
}

fn run(id: u128) -> RunId {
    RunId::from_uuid(Uuid::from_u128(id))
}

fn catalog(
    mission: MissionId,
    after: Option<RunId>,
    limit: u16,
    ids: &[u128],
) -> VerificationCatalog {
    VerificationCatalog {
        mission_id: mission,
        mission_version: 20,
        after,
        limit,
        entries: ids
            .iter()
            .map(|id| VerifierSummary {
                verifier_run_id: run(*id),
                subject_run_id: run(1000 + *id),
                phase: RunPhase::Finished,
                outcome: Some(FinishOutcome::Succeeded),
                subject_disposition: RunDisposition::AwaitingReview,
                primary_session_id: None,
            })
            .collect(),
        next_after: None,
    }
}

#[test]
fn compact_capabilities_are_independent_and_absence_makes_no_native_request_or_fallback() {
    let mission = MissionId::new();
    let verifier = RunId::new();
    for (has_status, has_catalog) in [(false, false), (false, true), (true, false)] {
        let fixture = PeerFixture::new(has_status, has_catalog, vec![]);
        if !has_status {
            assert!(
                matches!(fixture.client().verification_status(mission, verifier), Err(ClientError::Io(error)) if error.kind() == std::io::ErrorKind::Unsupported)
            );
        }
        if !has_catalog {
            assert!(
                matches!(fixture.client().verification_catalog(mission, None, 32), Err(ClientError::Io(error)) if error.kind() == std::io::ErrorKind::Unsupported)
            );
        }
        fixture.assert_consumed(0);
    }
}

#[test]
fn status_rejects_wrong_identity_unbounded_receipts_and_inconsistent_observation_fields() {
    let mission = MissionId::new();
    let verifier = RunId::new();
    let valid = status(mission, verifier, 1);
    let mut cases = Vec::new();
    for (label, change) in [
        (
            "wrong Mission",
            (|s: &mut VerificationStatus| s.mission_id = MissionId::new())
                as fn(&mut VerificationStatus),
        ),
        ("wrong verifier", |s: &mut VerificationStatus| {
            s.verifier_run_id = RunId::new()
        }),
        (
            "too many retained receipts",
            |s: &mut VerificationStatus| {
                s.receipts = (1..=17).map(receipt).collect();
                s.receipt_count = 17;
                s.passing_receipt_count = 17;
            },
        ),
        (
            "receipt count below retained count",
            |s: &mut VerificationStatus| {
                s.receipt_count = 0;
                s.passing_receipt_count = 0;
            },
        ),
        (
            "passing count exceeds total",
            |s: &mut VerificationStatus| s.passing_receipt_count = 2,
        ),
        (
            "hidden receipt without truncation flag",
            |s: &mut VerificationStatus| s.receipt_count = 2,
        ),
        ("false truncation claim", |s: &mut VerificationStatus| {
            s.receipts_truncated = true
        }),
        ("finished phase denied", |s: &mut VerificationStatus| {
            s.execution_finished = false
        }),
        (
            "running phase claimed finished",
            |s: &mut VerificationStatus| s.phase = RunPhase::Running,
        ),
        ("not observation only", |s: &mut VerificationStatus| {
            s.observation_only = false
        }),
        ("claims evidence recheck", |s: &mut VerificationStatus| {
            s.evidence_rechecked = true
        }),
        (
            "passing checks exceed checks",
            |s: &mut VerificationStatus| s.receipts[0].passing_checks = 3,
        ),
        ("overlong revision", |s: &mut VerificationStatus| {
            s.candidate_revision = "x".repeat(257)
        }),
        ("short digest", |s: &mut VerificationStatus| {
            s.candidate_sha256.truncate(63)
        }),
        ("nonhex digest", |s: &mut VerificationStatus| {
            s.candidate_sha256 = "g".repeat(64)
        }),
    ] {
        let mut altered = valid.clone();
        change(&mut altered);
        cases.push((label, ResponseBody::VerificationStatus { status: altered }));
    }
    cases.push(("wrong response kind", ResponseBody::Pong));
    let fixture = PeerFixture::new(
        true,
        true,
        cases.iter().map(|(_, body)| body.clone()).collect(),
    );
    for (label, _) in &cases {
        assert!(
            matches!(
                fixture.client().verification_status(mission, verifier),
                Err(ClientError::UnexpectedResponse(_))
            ),
            "accepted {label}"
        );
    }
    fixture.assert_consumed(cases.len());
}

#[test]
fn catalog_rejects_wrong_identity_cursor_order_limits_and_continuation() {
    let mission = MissionId::new();
    let after = Some(run(10));
    let valid = catalog(mission, after, 2, &[11, 12]);
    let mut cases = Vec::new();
    for (label, change) in [
        (
            "wrong Mission",
            (|c: &mut VerificationCatalog| c.mission_id = MissionId::new())
                as fn(&mut VerificationCatalog),
        ),
        ("wrong cursor", |c: &mut VerificationCatalog| {
            c.after = Some(run(9))
        }),
        ("missing requested cursor", |c: &mut VerificationCatalog| {
            c.after = None
        }),
        ("wrong limit", |c: &mut VerificationCatalog| c.limit = 3),
        ("over limit", |c: &mut VerificationCatalog| {
            c.entries.push(c.entries[1].clone())
        }),
        ("cursor repeated", |c: &mut VerificationCatalog| {
            c.entries[0].verifier_run_id = run(10)
        }),
        ("entry before cursor", |c: &mut VerificationCatalog| {
            c.entries[0].verifier_run_id = run(9)
        }),
        ("descending order", |c: &mut VerificationCatalog| {
            c.entries.swap(0, 1)
        }),
        ("duplicate verifier", |c: &mut VerificationCatalog| {
            c.entries[1].verifier_run_id = run(11)
        }),
        (
            "continuation differs from last entry",
            |c: &mut VerificationCatalog| c.next_after = Some(run(11)),
        ),
        (
            "short page with continuation",
            |c: &mut VerificationCatalog| {
                c.entries.pop();
                c.next_after = Some(run(11));
            },
        ),
        (
            "empty page with continuation",
            |c: &mut VerificationCatalog| {
                c.entries.clear();
                c.next_after = Some(run(10));
            },
        ),
    ] {
        let mut altered = valid.clone();
        change(&mut altered);
        cases.push((
            label,
            ResponseBody::VerificationCatalog { catalog: altered },
        ));
    }
    cases.push(("wrong response kind", ResponseBody::Pong));
    let fixture = PeerFixture::new(
        true,
        true,
        cases.iter().map(|(_, body)| body.clone()).collect(),
    );
    for (label, _) in &cases {
        assert!(
            matches!(
                fixture.client().verification_catalog(mission, after, 2),
                Err(ClientError::UnexpectedResponse(_))
            ),
            "accepted {label}"
        );
    }
    fixture.assert_consumed(cases.len());
}

#[test]
fn valid_status_preserves_bounded_receipts_and_never_turns_execution_into_acceptance() {
    let mission = MissionId::new();
    let verifier = RunId::new();
    let statuses = [
        status(mission, verifier, 1),
        status(mission, verifier, 16),
        status(mission, verifier, 17),
    ];
    let fixture = PeerFixture::new(
        true,
        false,
        statuses
            .iter()
            .cloned()
            .map(|status| ResponseBody::VerificationStatus { status })
            .collect(),
    );
    for expected in &statuses {
        let actual = fixture
            .client()
            .verification_status(mission, verifier)
            .unwrap();
        assert_eq!(&actual, expected);
        assert_eq!(actual.subject_disposition, RunDisposition::AwaitingReview);
        assert!(!actual.evidence_rechecked && actual.observation_only);
    }
    fixture.assert_consumed(statuses.len());
}

#[test]
fn valid_catalog_pages_preserve_cursors_clamp_request_limits_and_never_walk_implicitly() {
    let mission = MissionId::new();
    let mut first = catalog(mission, None, 1, &[1]);
    first.next_after = Some(run(1));
    let last = catalog(mission, Some(run(1)), 64, &(2..=65).collect::<Vec<_>>());
    let empty = catalog(mission, Some(run(65)), 32, &[]);
    let fixture = PeerFixture::new(
        false,
        true,
        [first.clone(), last.clone(), empty.clone()]
            .into_iter()
            .map(|catalog| ResponseBody::VerificationCatalog { catalog })
            .collect(),
    );
    assert_eq!(
        fixture
            .client()
            .verification_catalog(mission, None, 0)
            .unwrap(),
        first
    );
    assert_eq!(
        fixture.calls.lock().unwrap().len(),
        1,
        "next page is caller-controlled"
    );
    assert_eq!(
        fixture
            .client()
            .verification_catalog(mission, Some(run(1)), u16::MAX)
            .unwrap(),
        last
    );
    assert_eq!(
        fixture
            .client()
            .verification_catalog(mission, Some(run(65)), 32)
            .unwrap(),
        empty
    );
    fixture.assert_consumed(3);
    let calls = fixture.calls.lock().unwrap();
    assert!(
        matches!(calls[0], Request::VerificationCatalog { mission_id, after: None, limit: 1 } if mission_id == mission)
    );
    assert!(
        matches!(calls[1], Request::VerificationCatalog { mission_id, after: Some(cursor), limit: 64 } if mission_id == mission && cursor == run(1))
    );
    assert!(
        matches!(calls[2], Request::VerificationCatalog { mission_id, after: Some(cursor), limit: 32 } if mission_id == mission && cursor == run(65))
    );
}
