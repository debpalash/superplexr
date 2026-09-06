use super::*;
use ultraplexr_core::{
    Actor, Command, FinishOutcome, MissionId, RunCandidate, RunDriverSnapshot, RunHarnessSnapshot,
    RunId, RunPriority, VerificationCatalog, VerificationStatus, VerifiedDeliveryCommand,
};

fn mission(owner: &ControlClient) -> MissionId {
    let id = MissionId::new();
    owner
        .create_mission(
            id,
            "browser workflow fixture",
            Actor::human("owner").unwrap(),
        )
        .unwrap();
    id
}

fn workflow_gateway(
    owner: &ControlClient,
    fixture: &RuntimeFixture,
    missions: Vec<MissionId>,
    sessions: Vec<SessionId>,
) -> (
    ultraplexr_protocol::ShareSummary,
    ultraplexr_observer::Observer,
) {
    let (share, token) = owner
        .create_share(
            "browser workflow test",
            ShareRole::Observer,
            missions,
            sessions,
            60,
        )
        .unwrap();
    let client = ControlClient::connect_with_share(fixture.root.join("s"), token).unwrap();
    let gateway = ultraplexr_observer::Observer::new(client, HOST.into(), KEY.into()).unwrap();
    (share, gateway)
}

async fn get(router: &Router, path: &str) -> axum::response::Response {
    tokio::time::timeout(
        Duration::from_secs(5),
        router
            .clone()
            .oneshot(request(path, "GET", Value::Null, None, true)),
    )
    .await
    .expect("workflow HTTP deadline")
    .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    let bytes = to_bytes(response.into_body(), 65536)
        .await
        .expect("compact workflow response <=64 KiB");
    serde_json::from_slice(&bytes).expect("workflow JSON")
}

// Populate domain facts through the real owner's native protocol. These Runs
// have no Session or managed checkout; this fixture launches no agent process.
fn verifiers(owner: &ControlClient, mission: MissionId, count: usize) -> (RunId, Vec<RunId>) {
    let subject = RunId::new();
    let actor = Actor::agent("fixture-producer", "fixture").unwrap();
    let driver = RunDriverSnapshot {
        driver_id: "fixture".into(),
        profile_version: 1,
        process_spec_sha256: "a".repeat(64),
        argument_count: 0,
        environment_keys: vec![],
        sandbox_backend: None,
        sandbox_profile: None,
        sandbox_network_isolated: false,
    };
    for command in [
        Command::PlanRun {
            run_id: subject,
            parent: None,
            dependencies: vec![],
            retry_of: None,
            actor: actor.clone(),
            objective: "private objective excluded from catalog".into(),
            priority: RunPriority::Normal,
        },
        Command::ResolveRunDriver {
            run_id: subject,
            snapshot: driver.clone(),
        },
        Command::VerifiedDelivery {
            command: VerifiedDeliveryCommand::RecordHarnessSnapshot {
                run_id: subject,
                snapshot: RunHarnessSnapshot {
                    schema_version: 1,
                    objective_sha256: "b".repeat(64),
                    driver,
                    tools_sha256: None,
                    skills_sha256: None,
                    context_sha256: "c".repeat(64),
                    evaluator_sha256: None,
                },
            },
        },
        Command::StartReadyRun { run_id: subject },
        Command::VerifiedDelivery {
            command: VerifiedDeliveryCommand::SubmitCandidate {
                run_id: subject,
                candidate: RunCandidate {
                    revision: "frozen-fixture-revision".into(),
                    content_sha256: "d".repeat(64),
                    execution_lease_epoch: None,
                    realized_changes: None,
                    artifact_ids: vec![],
                    submitted_by: actor.id,
                },
            },
        },
        Command::FinishRun {
            run_id: subject,
            outcome: FinishOutcome::Succeeded,
            summary: "awaiting independent review".into(),
        },
    ] {
        owner
            .dispatch(mission, command)
            .expect("native fixture command");
    }
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let id = RunId::new();
        owner
            .dispatch(
                mission,
                Command::CreateVerifierRun {
                    subject_run_id: subject,
                    verifier_run_id: id,
                    actor: Actor::agent("fixture-verifier", "fixture").unwrap(),
                    priority: RunPriority::Normal,
                },
            )
            .unwrap();
        ids.push(id);
    }
    ids.sort_unstable();
    (subject, ids)
}

#[test]
fn browser_workflow_requires_flag_auth_valid_queries_and_mission_scope() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let visible = mission(&owner);
    let hidden = mission(&owner);
    let (_, gateway) = workflow_gateway(&owner, &fixture, vec![visible], vec![]);
    let disabled = gateway.clone().router();
    let enabled = gateway.clone().with_workflow_inspection().router();
    let list = format!("/missions/{visible}/verifiers");
    let status = format!("{list}/{}/status", RunId::new());
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for path in [&list, &status] {
            let response = get(&disabled, path).await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert!(
                json_body(response).await["error"]
                    .as_str()
                    .unwrap()
                    .contains("disabled")
            );
            assert_eq!(
                enabled
                    .clone()
                    .oneshot(request(path, "GET", Value::Null, None, false))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            assert_eq!(
                enabled
                    .clone()
                    .oneshot(request(
                        path,
                        "GET",
                        Value::Null,
                        Some("http://foreign.invalid"),
                        true
                    ))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                enabled
                    .clone()
                    .oneshot(request(path, "POST", Value::Null, Some(ORIGIN), true))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::METHOD_NOT_ALLOWED
            );
        }
        for suffix in [
            "?limit=0",
            "?limit=65",
            "?limit=-1",
            "?limit=65536",
            "?limit=no",
            "?after=bad",
            "?unknown=1",
            "?limit=1&limit=2",
        ] {
            assert_eq!(
                get(&enabled, &format!("{list}{suffix}")).await.status(),
                StatusCode::BAD_REQUEST,
                "{suffix}"
            );
        }
        for path in [
            "/missions/bad/verifiers".to_owned(),
            format!("{list}/bad/status"),
        ] {
            assert_eq!(get(&enabled, &path).await.status(), StatusCode::BAD_REQUEST);
        }
        for id in [hidden, MissionId::new()] {
            for path in [
                format!("/missions/{id}/verifiers"),
                format!("/missions/{id}/verifiers/{}/status", RunId::new()),
            ] {
                let response = get(&enabled, &path).await;
                assert_eq!(response.status(), StatusCode::FORBIDDEN);
                assert!(json_body(response).await["error"].is_string());
            }
        }
        assert_eq!(
            get(&enabled, &status).await.status(),
            StatusCode::FORBIDDEN,
            "missing verifier does not disclose Mission contents"
        );
        gateway.shutdown();
    });
}

#[test]
fn browser_workflow_empty_catalog_preserves_cursor_limit_and_mission_version() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let id = mission(&owner);
    let before = owner.get_mission(id).unwrap();
    let (_, gateway) = workflow_gateway(&owner, &fixture, vec![id], vec![]);
    let router = gateway.clone().with_workflow_inspection().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for (limit, after) in [(32, None), (1, Some(RunId::new())), (64, None)] {
            let path = format!(
                "/missions/{id}/verifiers?limit={limit}{}",
                after.map(|id| format!("&after={id}")).unwrap_or_default()
            );
            let response = get(&router, &path).await;
            assert_eq!(response.status(), StatusCode::OK);
            let page: VerificationCatalog =
                serde_json::from_value(json_body(response).await).unwrap();
            assert_eq!(page, owner.verification_catalog(id, after, limit).unwrap());
            assert_eq!(page.mission_version, before.version);
            assert_eq!(page.after, after);
            assert_eq!(page.limit, limit);
            assert!(page.entries.is_empty());
            assert!(page.next_after.is_none());
        }
        gateway.shutdown();
    });
    assert_eq!(
        owner.get_mission(id).unwrap(),
        before,
        "HTTP inspection cannot mutate the Mission"
    );
}

#[test]
fn browser_workflow_pages_are_compact_and_status_keeps_outcome_separate_from_review() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let id = mission(&owner);
    let (subject, ids) = verifiers(&owner, id, 65);
    owner
        .dispatch(id, Command::StartReadyRun { run_id: ids[0] })
        .unwrap();
    owner
        .dispatch(
            id,
            Command::FinishRun {
                run_id: ids[0],
                outcome: FinishOutcome::Succeeded,
                summary: "execution ended".into(),
            },
        )
        .unwrap();
    let before = owner.get_mission(id).unwrap();
    let (share, gateway) = workflow_gateway(&owner, &fixture, vec![id], vec![]);
    let router = gateway.clone().with_workflow_inspection().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for (limit, after, count) in [
            (32, None, 32),
            (64, None, 64),
            (32, Some(ids[31]), 32),
            (32, Some(ids[63]), 1),
        ] {
            let path = format!(
                "/missions/{id}/verifiers?limit={limit}{}",
                after.map(|id| format!("&after={id}")).unwrap_or_default()
            );
            let response = get(&router, &path).await;
            assert_eq!(response.status(), StatusCode::OK);
            let value = json_body(response).await;
            assert!(!value.to_string().contains("private objective"));
            assert!(value.get("runs").is_none());
            assert!(
                value["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|entry| entry.as_object().unwrap().len() == 6)
            );
            let page: VerificationCatalog = serde_json::from_value(value).unwrap();
            assert_eq!(page, owner.verification_catalog(id, after, limit).unwrap());
            assert_eq!(page.entries.len(), count);
            assert!(
                page.entries
                    .iter()
                    .all(|entry| entry.subject_run_id == subject
                        && entry.primary_session_id.is_none())
            );
        }
        let path = format!("/missions/{id}/verifiers/{}/status", ids[0]);
        let response = get(&router, &path).await;
        assert_eq!(response.status(), StatusCode::OK);
        let status: VerificationStatus = serde_json::from_value(json_body(response).await).unwrap();
        assert_eq!(status, owner.verification_status(id, ids[0]).unwrap());
        assert_eq!(status.outcome, Some(FinishOutcome::Succeeded));
        assert_eq!(
            status.subject_disposition,
            ultraplexr_core::RunDisposition::AwaitingReview
        );
        assert_eq!(status.receipt_count, 0);
        assert_eq!(status.passing_receipt_count, 0);
        assert!(status.observation_only);
        assert!(!status.evidence_rechecked);
        assert_eq!(status.candidate_revision, "frozen-fixture-revision");
        assert_eq!(status.candidate_sha256, "d".repeat(64));
        assert_eq!(
            get(
                &router,
                &format!("/missions/{id}/verifiers/{subject}/status")
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(owner.get_mission(id).unwrap(), before);
        owner.revoke_share(share.share_id).unwrap();
        for path in [format!("/missions/{id}/verifiers"), path] {
            assert_eq!(
                get(&router, &path).await.status(),
                StatusCode::FORBIDDEN,
                "revocation must reach both HTTP reads"
            );
        }
        gateway.shutdown();
    });
}

#[test]
fn browser_workflow_session_only_share_cannot_read_the_sessions_mission() {
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let id = mission(&owner);
    let terminal = terminal(&owner, &fixture);
    let session = terminal.id();
    owner
        .dispatch(
            id,
            Command::StartSession {
                session_id: session,
                name: "domain session".into(),
                started_by: Actor::human("owner").unwrap(),
            },
        )
        .unwrap();
    let (_, gateway) = workflow_gateway(&owner, &fixture, vec![], vec![session]);
    let router = gateway.clone().with_workflow_inspection().router();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for path in [
            format!("/missions/{id}/verifiers"),
            format!("/missions/{id}/verifiers/{}/status", RunId::new()),
        ] {
            assert_eq!(get(&router, &path).await.status(), StatusCode::FORBIDDEN);
        }
        gateway.shutdown();
    });
    terminal.kill().unwrap();
}

/// Launch only by exact name with a private, existing metadata directory. This
/// serves the current embedded browser against its own disposable native daemon.
#[test]
#[ignore = "explicit isolated browser visual QA fixture"]
fn browser_workflow_visual_fixture() {
    let Some(directory) = std::env::var_os("ULTRAPLEXR_BROWSER_WORKFLOW_VISUAL_DIR") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    assert!(
        directory.is_dir(),
        "private fixture metadata directory must exist"
    );
    let fixture = RuntimeFixture::start();
    let owner = fixture.client();
    let id = mission(&owner);
    let (_, ids) = verifiers(&owner, id, 35);
    owner
        .dispatch(id, Command::StartReadyRun { run_id: ids[0] })
        .unwrap();
    owner
        .dispatch(
            id,
            Command::FinishRun {
                run_id: ids[0],
                outcome: FinishOutcome::Succeeded,
                summary: "visual fixture execution ended".into(),
            },
        )
        .unwrap();
    let mut terminals = Vec::new();
    for title in ["Workflow visual QA", "Second isolated session"] {
        let terminal = owner.start_terminal(TerminalSessionSpec {
            session_id: SessionId::new(), mission_id: None, run_id: None,
            program: "/bin/sh".into(),
            args: vec!["-c".into(), format!("stty -echo; printf '\\033]0;{title}\\007\\033[32mReady for visual QA\\033[0m\\nWide glyph: 界  Literal: <script>safe text</script>\\nCOPY-FIXTURE-READY\\n'; exec cat")],
            cwd: fixture.root.clone(), environment_delta: BTreeMap::new(), grid: GridSize::new(80,24).unwrap(),
        }).unwrap();
        wait_text(&terminal, "COPY-FIXTURE-READY");
        terminals.push(terminal);
    }
    let session_ids = terminals.iter().map(DaemonSession::id).collect::<Vec<_>>();
    let (_, token) = owner
        .create_share(
            "isolated browser visual QA",
            ShareRole::Observer,
            vec![id],
            session_ids.clone(),
            1800,
        )
        .unwrap();
    let client = ControlClient::connect_with_share(fixture.root.join("s"), token).unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let key = format!("{}{}",uuid::Uuid::new_v4().simple(),uuid::Uuid::new_v4().simple());
        let gateway = ultraplexr_observer::Observer::new(client,address.to_string(),key.clone()).unwrap().with_workflow_inspection();
        let metadata = json!({"url":format!("http://{address}/#access={key}"),"mission":id,"verifiers":ids,"sessions":session_ids,
            "socket":fixture.root.join("s"),"runtime_root":fixture.root});
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut ready = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600)
            .open(directory.join("ready.json")).unwrap();
        ready.write_all(&serde_json::to_vec(&metadata).unwrap()).unwrap();
        let stop = directory.join("stop");
        axum::serve(listener,gateway.clone().router()).with_graceful_shutdown(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1800);
            while !stop.exists() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            gateway.shutdown();
        }).await.unwrap();
    });
    for terminal in terminals {
        terminal.kill().unwrap();
    }
}
