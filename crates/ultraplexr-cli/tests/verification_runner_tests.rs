//! Public CLI verification commands exercised against real local daemon/PTY/Git.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    process::{Child, Command as Process, Stdio},
    thread,
    time::{Duration, Instant},
};
use ultraplexr_client::ControlClient;
use ultraplexr_core::{
    Actor, ActorId, ChangeClaim, ChangeIntentSpec, ChangeOperation, ChangeScope, Command,
    EvaluationReceipt, EvaluationVerdict, FinishOutcome, MissionId, RunCandidate, RunDisposition,
    RunId, RunPriority, SessionId, VerificationPolicy, VerifiedDeliveryCommand,
};
use ultraplexr_protocol::TerminalSessionSpec;
use ultraplexr_terminal::GridSize;
use uuid::Uuid;

#[path = "verification/rust_project_recipe_tests.rs"]
mod rust_project_recipe_tests;

#[path = "verification/search_pages_tests.rs"]
mod search_pages_tests;

#[test]
#[ignore = "child process entry point; parent test supplies an isolated root"]
fn isolated_verification_daemon() {
    let root = PathBuf::from(std::env::var_os("ULTRAPLEXR_VERIFY_TEST_ROOT").expect("test root"));
    ultraplexr_server::run_blocking(root.join("s"), root.join("state"), None)
        .expect("isolated daemon");
}

struct Fixture {
    root: PathBuf,
    child: Option<Child>,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from("/tmp").join(format!("up-verify-{}", Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .expect("fixture root");
        let mut fixture = Self { root, child: None };
        fs::create_dir(fixture.root.join("repo")).expect("repo");
        fs::DirBuilder::new()
            .mode(0o700)
            .create(fixture.root.join("evidence"))
            .expect("evidence");
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@ultraplexr.invalid"],
        ] {
            let out = Process::new("git")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .arg("-C")
                .arg(fixture.root.join("repo"))
                .args(args)
                .output()
                .expect("Git");
            assert!(out.status.success(), "{:?}", out);
        }
        fs::write(fixture.root.join("repo/product"), "base\n").expect("base");
        for args in [
            vec!["add", "product"],
            vec![
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(
                Process::new("git")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .arg("-C")
                    .arg(fixture.root.join("repo"))
                    .args(args)
                    .status()
                    .expect("Git")
                    .success()
            );
        }
        fixture.start();
        fixture
    }
    fn start(&mut self) {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("daemon.log"))
            .expect("log");
        self.child = Some(
            Process::new(std::env::current_exe().expect("test binary"))
                .args([
                    "--exact",
                    "isolated_verification_daemon",
                    "--ignored",
                    "--nocapture",
                ])
                .env("ULTRAPLEXR_VERIFY_TEST_ROOT", &self.root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(Stdio::from(log.try_clone().expect("log handle")))
                .stderr(Stdio::from(log))
                .spawn()
                .expect("daemon"),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        while ControlClient::connect(self.root.join("s")).is_err() {
            assert!(
                self.child
                    .as_mut()
                    .expect("child")
                    .try_wait()
                    .expect("status")
                    .is_none(),
                "{:?}",
                fs::read_to_string(self.root.join("daemon.log"))
            );
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(20));
        }
    }
    fn restart(&mut self) {
        let mut child = self.child.take().expect("daemon");
        child.kill().expect("stop fixture");
        child.wait().expect("reap");
        self.start();
    }
    fn client(&self) -> ControlClient {
        ControlClient::connect(self.root.join("s")).expect("owner")
    }
    fn plan(&self, script: &str) -> PathBuf {
        let path = self.root.join(format!("plan-{}.json", Uuid::new_v4()));
        let checks:Vec<_>=["format","lint","tests","delivery","provenance","repeatability"].into_iter().map(|id|serde_json::json!({
            "id":id,"program":"/bin/sh","args":["-c",script],"timeout_seconds":1,"output_limit_bytes":1024
        })).collect();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("private plan");
        file.write_all(
            &serde_json::to_vec(&serde_json::json!({"version":1,"checks":checks}))
                .expect("plan JSON"),
        )
        .expect("plan");
        path
    }
    fn collect(&self, mission: MissionId, run: RunId) -> std::process::Output {
        Process::new(env!("CARGO_BIN_EXE_ultraplexr"))
            .arg("--socket")
            .arg(self.root.join("s"))
            .arg("verification-collect")
            .arg(mission.to_string())
            .arg(run.to_string())
            .arg("--evidence-root")
            .arg(self.root.join("evidence"))
            .output()
            .expect("collector CLI")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn verified(command: VerifiedDeliveryCommand) -> Command {
    Command::VerifiedDelivery { command }
}
fn wait(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "stage timeout");
        thread::sleep(Duration::from_millis(20));
    }
}
fn finish(client: &ControlClient, mission: MissionId, run: RunId) {
    wait(|| {
        client.get_mission(mission).expect("Mission").runs[&run]
            .status
            .is_finished()
    });
    assert_eq!(
        client.get_mission(mission).expect("finished").runs[&run].outcome,
        Some(FinishOutcome::Succeeded)
    );
}

fn subject(fixture: &Fixture, client: &ControlClient) -> (MissionId, RunId, RunCandidate) {
    let mission = MissionId::new();
    let run = RunId::new();
    client
        .create_mission(
            mission,
            "Check runner",
            Actor::human("owner").expect("owner"),
        )
        .expect("Mission");
    client
        .dispatch(
            mission,
            Command::PlanRun {
                run_id: run,
                parent: None,
                dependencies: vec![],
                retry_of: None,
                actor: Actor::agent("producer", "direct-command").expect("actor"),
                objective: "Produce the answer".into(),
                priority: RunPriority::Normal,
            },
        )
        .expect("plan producer");
    let checkout = client
        .prepare_run_checkout(mission, run, fixture.root.join("repo"), "HEAD")
        .expect("producer checkout");
    client
        .dispatch(
            mission,
            verified(VerifiedDeliveryCommand::DeclareChangeIntent {
                run_id: run,
                expected_version: None,
                spec: ChangeIntentSpec {
                    repository_identity: fixture.root.to_string_lossy().into_owned(),
                    base_revision: checkout.base_revision,
                    claims: vec![ChangeClaim {
                        path: "product".into(),
                        operation: ChangeOperation::Modify,
                        scope: ChangeScope::Committed,
                    }],
                },
            }),
        )
        .expect("intent");
    client
        .dispatch(
            mission,
            verified(VerifiedDeliveryCommand::SetVerificationPolicy {
                run_id: run,
                policy: VerificationPolicy::Independent,
            }),
        )
        .expect("independent policy");
    let session = SessionId::new();
    let (_, state, _) = client
        .launch_agent_run(
            mission,
            run,
            "producer",
            TerminalSessionSpec {
                session_id: session,
                mission_id: Some(mission),
                run_id: Some(run),
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "printf '42\\n' > product; read -r finish; test \"$finish\" = finish".into(),
                ],
                cwd: checkout.worktree_path.clone(),
                environment_delta: BTreeMap::new(),
                grid: GridSize::new(80, 24).expect("grid"),
            },
        )
        .expect("producer");
    wait(|| {
        fs::read_to_string(checkout.worktree_path.join("product")).is_ok_and(|text| text == "42\n")
    });
    let (_, published) = client
        .dispatch(
            mission,
            verified(VerifiedDeliveryCommand::SubmitCandidate {
                run_id: run,
                candidate: RunCandidate {
                    revision: "runtime-authored".into(),
                    content_sha256: "0".repeat(64),
                    execution_lease_epoch: Some(
                        state.verified_delivery.change_intents[&run].lease_epoch,
                    ),
                    realized_changes: None,
                    artifact_ids: vec![],
                    submitted_by: ActorId::new("producer").expect("actor"),
                },
            }),
        )
        .expect("candidate");
    let terminal = client.terminal(session);
    terminal.claim_control(true).expect("fixture owner control");
    terminal.paste(b"finish\n".to_vec(), true).expect("finish");
    finish(client, mission, run);
    (
        mission,
        run,
        published.verified_delivery.candidates[&run].clone(),
    )
}

fn verifier(
    fixture: &Fixture,
    client: &ControlClient,
    mission: MissionId,
    subject: RunId,
    candidate: &RunCandidate,
    plan: PathBuf,
) -> (RunId, SessionId) {
    let run = RunId::new();
    client
        .dispatch(
            mission,
            Command::CreateVerifierRun {
                subject_run_id: subject,
                verifier_run_id: run,
                actor: Actor::agent("reviewer", "direct-command").expect("actor"),
                priority: RunPriority::Normal,
            },
        )
        .expect("verifier");
    let checkout = client
        .prepare_run_checkout(mission, run, fixture.root.join("repo"), &candidate.revision)
        .expect("verifier checkout");
    let session = SessionId::new();
    client
        .launch_agent_run(
            mission,
            run,
            "bounded verification",
            TerminalSessionSpec {
                session_id: session,
                mission_id: Some(mission),
                run_id: Some(run),
                program: env!("CARGO_BIN_EXE_ultraplexr").into(),
                args: vec![
                    "verification-execute".into(),
                    "--plan".into(),
                    plan.to_string_lossy().into_owned(),
                    "--evidence-root".into(),
                    fixture.root.join("evidence").to_string_lossy().into_owned(),
                ],
                cwd: checkout.worktree_path,
                environment_delta: BTreeMap::new(),
                grid: GridSize::new(80, 24).expect("grid"),
            },
        )
        .expect("worker launch");
    (run, session)
}

#[test]
fn real_verifier_collects_exact_evidence_after_restart_without_accepting_or_reexecuting() {
    let mut fixture = Fixture::new();
    let client = fixture.client();
    let (mission, producer, candidate) = subject(&fixture, &client);
    let plan = fixture.plan("test \"$(cat product)\" = 42 && printf checked");
    let (run, _) = verifier(&fixture, &client, mission, producer, &candidate, plan);
    finish(&client, mission, run);
    drop(client);
    fixture.restart();
    let output = fixture.collect(mission, run);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: EvaluationReceipt = serde_json::from_slice(&output.stdout).expect("receipt JSON");
    assert_eq!(receipt.verdict, EvaluationVerdict::Passed);
    assert_eq!(receipt.candidate_sha256, candidate.content_sha256);
    let client = fixture.client();
    let state = client.get_mission(mission).expect("Mission");
    assert_eq!(
        state
            .artifacts
            .values()
            .filter(|artifact| artifact.run_id == run)
            .count(),
        14
    );
    assert_eq!(
        state.runs[&producer].disposition,
        RunDisposition::AwaitingReview
    );
    let repeated = fixture.collect(mission, run);
    assert!(repeated.status.success());
    assert_eq!(
        serde_json::from_slice::<EvaluationReceipt>(&repeated.stdout).expect("same receipt"),
        receipt
    );
    assert_eq!(
        client
            .get_mission(mission)
            .expect("unchanged graph")
            .version,
        state.version
    );
    let report_dir = fixture.root.join("evidence").join(run.to_string());
    fs::write(report_dir.join("0.stdout"), "tampered").expect("tamper own fixture evidence");
    assert!(
        !fixture.collect(mission, run).status.success(),
        "changed evidence must fail before any commits"
    );
    assert_eq!(
        client.get_mission(mission).expect("no new commits").version,
        state.version
    );
    fs::write(report_dir.join("0.stdout"), b"checked").expect("restore exact fixture evidence");
    let accepted = client
        .dispatch(
            mission,
            Command::AcceptRunResult {
                run_id: producer,
                by: ActorId::new("owner").expect("owner"),
                note: "Owner reviewed the executed checks".into(),
            },
        )
        .expect("explicit acceptance")
        .1;
    assert_eq!(
        accepted.runs[&producer].disposition,
        RunDisposition::Accepted
    );
}

#[test]
fn failed_checks_and_candidate_changes_never_produce_passing_receipts() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let (mission, producer, candidate) = subject(&fixture, &client);
    for script in ["exit 3", "printf changed > product"] {
        let (run, _) = verifier(
            &fixture,
            &client,
            mission,
            producer,
            &candidate,
            fixture.plan(script),
        );
        finish(&client, mission, run);
        let output = fixture.collect(mission, run);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: EvaluationReceipt =
            serde_json::from_slice(&output.stdout).expect("failed receipt");
        assert_eq!(receipt.verdict, EvaluationVerdict::Failed);
        assert!(
            client
                .dispatch(
                    mission,
                    Command::AcceptRunResult {
                        run_id: producer,
                        by: ActorId::new("owner").expect("owner"),
                        note: "must be denied".into()
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn hung_checks_are_bounded_and_collected_as_failed_evidence() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let (mission, producer, candidate) = subject(&fixture, &client);
    let started = Instant::now();
    let (run, _) = verifier(
        &fixture,
        &client,
        mission,
        producer,
        &candidate,
        fixture.plan("sleep 30"),
    );
    finish(&client, mission, run);
    assert!(started.elapsed() < Duration::from_secs(15));
    let output = fixture.collect(mission, run);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt: EvaluationReceipt = serde_json::from_slice(&output.stdout).expect("receipt");
    assert_eq!(receipt.verdict, EvaluationVerdict::Failed);
    let report: serde_json::Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .join("evidence")
                .join(run.to_string())
                .join("report.json"),
        )
        .expect("report"),
    )
    .expect("JSON");
    assert!(
        report["checks"]
            .as_array()
            .expect("checks")
            .iter()
            .all(|check| check["outcome"]["type"] == "timeout")
    );
}

#[test]
fn interrupted_worker_cleans_its_check_and_cannot_publish_a_receipt() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let (mission, producer, candidate) = subject(&fixture, &client);
    let marker = fixture.root.join("check.pid");
    let script = format!("echo $$ > '{}'; sleep 30", marker.display());
    let (run, session) = verifier(
        &fixture,
        &client,
        mission,
        producer,
        &candidate,
        fixture.plan(&script),
    );
    wait(|| fs::read_to_string(&marker).is_ok_and(|text| text.trim().parse::<i32>().is_ok()));
    let check_pid = fs::read_to_string(&marker)
        .expect("child pid")
        .trim()
        .parse::<i32>()
        .expect("PID");
    let worker_pid = client
        .list_terminals()
        .expect("terminals")
        .iter()
        .find(|terminal| terminal.session_id == session)
        .expect("worker")
        .process_id
        .expect("worker PID");
    // SAFETY: this PID was returned for the disposable verifier created above.
    assert_eq!(unsafe { libc::kill(worker_pid as i32, libc::SIGTERM) }, 0);
    wait(|| {
        client.get_mission(mission).expect("Mission").runs[&run]
            .status
            .is_finished()
    });
    assert_eq!(
        client.get_mission(mission).expect("failed worker").runs[&run].outcome,
        Some(FinishOutcome::Failed)
    );
    // SAFETY: signal 0 checks existence only; this is the fixture check PID.
    wait(|| unsafe { libc::kill(check_pid, 0) } != 0);
    assert!(!fixture.collect(mission, run).status.success());
    assert!(
        !fixture
            .root
            .join("evidence")
            .join(run.to_string())
            .join("report.json")
            .exists()
    );
    assert!(
        client
            .get_mission(mission)
            .expect("no receipt")
            .verified_delivery
            .evaluation_receipts
            .is_empty()
    );
}

#[test]
fn desktop_launch_seam_pins_reviewed_plan_and_uses_the_existing_runtime() {
    use ultraplexr_verification::{LaunchTarget, Setup, launch, prepare};
    let fixture = Fixture::new();
    let client = fixture.client();
    let (mission, producer, _) = subject(&fixture, &client);
    let plan_path = fixture.plan("test \"$(cat product)\" = 42");
    let setup = Setup {
        plan_path: plan_path.clone(),
        evidence_root: fixture.root.join("evidence"),
        runner_path: env!("CARGO_BIN_EXE_ultraplexr").into(),
    };
    let reviewed = prepare(&setup).expect("review exact commands");
    assert!(reviewed.description.contains("format"));
    assert!(reviewed.description.contains("1024 bytes/stream"));
    let version = client.get_mission(mission).expect("before launch").version;
    let mut changed = fs::read(&plan_path).expect("plan bytes");
    changed.push(b' ');
    fs::write(&plan_path, &changed).expect("owner changes plan");
    assert!(
        launch(&client, mission, LaunchTarget::Subject(producer), &reviewed).is_err(),
        "changed plan must fail before Run creation"
    );
    assert_eq!(
        client.get_mission(mission).expect("no new Run").version,
        version
    );
    let reviewed = prepare(&setup).expect("explicit review again");
    let outcome = launch(&client, mission, LaunchTarget::Subject(producer), &reviewed)
        .expect("desktop launch");
    assert!(outcome.launch_error.is_none(), "{:?}", outcome.launch_error);
    finish(&client, mission, outcome.run_id);
    let receipt =
        ultraplexr_verification::collect(&client, mission, outcome.run_id, &setup.evidence_root)
            .expect("desktop collection");
    assert_eq!(receipt.verdict, EvaluationVerdict::Passed);
    assert_eq!(
        client
            .get_mission(mission)
            .expect("owner still decides")
            .runs[&producer]
            .disposition,
        RunDisposition::AwaitingReview
    );
    assert_eq!(
        client
            .list_all_terminals()
            .expect("runtime processes")
            .len(),
        2
    );
}

#[test]
#[ignore = "manual native visual QA fixture; create finish-qa in its printed root to release"]
fn native_verification_visual_fixture() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let (mission, producer, _) = subject(&fixture, &client);
    let plan_path =
        fixture.plan("test \"$(cat product)\" = 42 && printf 'verified by project checks\\n'");
    let setup = ultraplexr_verification::Setup {
        plan_path,
        evidence_root: fixture.root.join("evidence"),
        runner_path: env!("CARGO_BIN_EXE_ultraplexr").into(),
    };
    let mut setups = BTreeMap::new();
    setups.insert(mission, setup);
    let document = serde_json::json!({"version":3,"active_workspace":1,"next_sequence":2,
        "verification_setups":setups,"workspaces":[{"id":1,"mission_id":mission,"title":"Verification QA","pinned":false,"selected_session":0,"focus_mode":false,"sessions":[]}]});
    fs::write(
        fixture.root.join("state/workspaces.json"),
        serde_json::to_vec(&document).expect("workspace JSON"),
    )
    .expect("seed personal choices");
    println!(
        "VISUAL_FIXTURE {}",
        serde_json::json!({"root":fixture.root,"mission":mission,"producer":producer})
    );
    let deadline = Instant::now() + Duration::from_secs(900);
    while !fixture.root.join("finish-qa").exists() {
        assert!(Instant::now() < deadline, "visual QA deadline");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn worker_refuses_plan_changed_after_runtime_launch() {
    use ultraplexr_verification::{LaunchTarget, Setup, launch, prepare};
    let fixture = Fixture::new();
    let client = fixture.client();
    let (mission, producer, _) = subject(&fixture, &client);
    let plan_path = fixture.plan("printf 'must not execute'");
    let runner_path = fixture.root.join("delayed-runner");
    let mut runner = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&runner_path)
        .expect("fixture runner");
    // Fixture paths are generated UUIDs. Quote the workspace binary path without
    // changing process-global environment in this concurrent test suite.
    let cli = env!("CARGO_BIN_EXE_ultraplexr").replace('\'', "'\\''");
    writeln!(
        runner,
        "#!/bin/sh\nprintf ' ' >> '{}'\nexec '{cli}' \"$@\"",
        plan_path.display()
    )
    .expect("delayed mutation script");
    drop(runner);
    let setup = Setup {
        plan_path,
        runner_path,
        evidence_root: fixture.root.join("evidence"),
    };
    let reviewed = prepare(&setup).expect("review before mutation");
    let outcome = launch(&client, mission, LaunchTarget::Subject(producer), &reviewed)
        .expect("launch accepted before worker changes plan");
    assert!(outcome.launch_error.is_none(), "{:?}", outcome.launch_error);
    wait(|| {
        client.get_mission(mission).expect("Mission").runs[&outcome.run_id]
            .status
            .is_finished()
    });
    let state = client.get_mission(mission).expect("failed verifier");
    assert_eq!(
        state.runs[&outcome.run_id].outcome,
        Some(FinishOutcome::Failed)
    );
    assert!(
        !setup
            .evidence_root
            .join(outcome.run_id.to_string())
            .exists(),
        "no check output may be created"
    );
    assert!(
        ultraplexr_verification::collect(&client, mission, outcome.run_id, &setup.evidence_root)
            .is_err()
    );
    assert!(state.verified_delivery.evaluation_receipts.is_empty());
    assert_eq!(
        state.runs[&producer].disposition,
        RunDisposition::AwaitingReview
    );
}

#[test]
fn pending_verifier_launch_reuses_frozen_run_and_rejects_second_launch() {
    use ultraplexr_verification::{LaunchTarget, Setup, launch, prepare};
    let fixture = Fixture::new();
    let client = fixture.client();
    let (mission, producer, _) = subject(&fixture, &client);
    let verifier = RunId::new();
    client
        .dispatch(
            mission,
            Command::CreateVerifierRun {
                subject_run_id: producer,
                verifier_run_id: verifier,
                actor: Actor::agent("prepared-checks", "direct-command").expect("actor"),
                priority: RunPriority::Urgent,
            },
        )
        .expect("existing desktop Create verifier action");
    let setup = Setup {
        plan_path: fixture.plan("test \"$(cat product)\" = 42"),
        evidence_root: fixture.root.join("evidence"),
        runner_path: env!("CARGO_BIN_EXE_ultraplexr").into(),
    };
    let reviewed = prepare(&setup).expect("review");
    let outcome = launch(
        &client,
        mission,
        LaunchTarget::Verifier(verifier),
        &reviewed,
    )
    .expect("launch pending verifier");
    assert!(outcome.launch_error.is_none(), "{:?}", outcome.launch_error);
    assert_eq!(outcome.run_id, verifier);
    finish(&client, mission, verifier);
    let version = client.get_mission(mission).expect("finished").version;
    assert!(
        launch(
            &client,
            mission,
            LaunchTarget::Verifier(verifier),
            &reviewed
        )
        .is_err()
    );
    let state = client.get_mission(mission).expect("no duplicate launch");
    assert_eq!(state.version, version);
    assert_eq!(state.runs.len(), 2);
    assert_eq!(client.list_all_terminals().expect("sessions").len(), 2);
    assert_eq!(
        ultraplexr_verification::collect(&client, mission, verifier, &setup.evidence_root)
            .expect("collect")
            .verdict,
        EvaluationVerdict::Passed
    );
}
