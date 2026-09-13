//! Real process/IPC/Git acceptance journeys. No provider credentials or user state.
#[path = "delivery_journey_tests/verification_inspection_tests.rs"]
mod verification_inspection_tests;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command as Process, Stdio},
    thread,
    time::{Duration, Instant},
};
use superplexr_client::ControlClient;
use superplexr_core::{
    Actor, ActorId, ArtifactId, ChangeClaim, ChangeIntentSpec, ChangeOperation, ChangeScope,
    Command, EvaluationCheck, EvaluationReceipt, EvaluationVerdict, FinishOutcome, Mission,
    MissionId, RunCandidate, RunDisposition, RunId, RunPriority, SessionId, VerificationPolicy,
    VerifiedDeliveryCommand,
};
use superplexr_protocol::RunCheckoutSummary;
use superplexr_terminal::GridSize;
use uuid::Uuid;

const PRODUCER: &str = r#"set -eu
test -n "$SUPERPLEXR_EXECUTION_LEASE_EPOCH"
if test "${SUPERPLEXR_DELIVERY_PURPOSE:-}" = retry; then
  test -n "$SUPERPLEXR_RETURN_NOTE"
  test "$(git rev-parse HEAD)" = "$SUPERPLEXR_CANDIDATE_REVISION"
  answer=42
else
  answer=41
fi
printf '%s\n' '#!/bin/sh' "printf '$answer\\n'" > product.sh
printf 'producer-ready\n'
read -r finish
test "$finish" = finish
"#;

// A successful verifier process means it completed its examination, not that
// its subject passed. Each check records its actual result in retained evidence.
const VERIFIER: &str = r#"set -eu
test "$SUPERPLEXR_DELIVERY_PURPOSE" = verification
report="$JOURNEY_EVIDENCE/$SUPERPLEXR_RUN_ID.log"
check() {
  name=$1; shift
  if "$@"; then printf '%s=0\n' "$name"; else printf '%s=1\n' "$name"; fi >> "$report"
}
: > "$report"
check format sh -c 'test "$(tail -c 1 product.sh | od -An -tu1 | tr -d " ")" = 10'
check lint sh -n product.sh
check tests sh -c 'test "$(sh product.sh)" = 42'
check delivery sh -c 'install -m 700 product.sh "$1.install" && test "$("$1.install")" = 42' sh "$report"
check provenance sh -c 'test "$(git rev-parse HEAD)" = "$SUPERPLEXR_CANDIDATE_REVISION" && test -z "$(git status --porcelain)"'
check repeatability sh -c 'test "$(sh product.sh)" = "$(sh product.sh)"'
printf 'verification-evidence-ready\n'
"#;

struct Fixture {
    root: PathBuf,
    daemon: Option<Child>,
}

impl Fixture {
    fn new() -> Self {
        // Short paths are necessary for Unix domain sockets on macOS.
        let root = PathBuf::from("/tmp").join(format!("up-delivery-{}", Uuid::new_v4()));
        fs::create_dir(&root).expect("isolated fixture root");
        let mut fixture = Self { root, daemon: None };
        fs::create_dir(fixture.root.join("repository")).expect("repository");
        fs::create_dir(fixture.root.join("state")).expect("state");
        fs::create_dir(fixture.root.join("evidence")).expect("evidence");
        fixture.git(&["init", "-q"]);
        fixture.git(&["config", "user.name", "Superplexr fixture"]);
        fixture.git(&["config", "user.email", "fixture@superplexr.invalid"]);
        fs::write(
            fixture.repository().join("product.sh"),
            "#!/bin/sh\nprintf '0\\n'\n",
        )
        .expect("base product");
        fixture.git(&["add", "product.sh"]);
        fixture.git(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture base",
        ]);
        fs::write(fixture.root.join("producer.sh"), PRODUCER).expect("producer fixture");
        fs::write(fixture.root.join("verifier.sh"), VERIFIER).expect("verifier fixture");
        fs::write(
            fixture.root.join("state/engines.json"),
            serde_json::to_vec(&serde_json::json!({
                "version":1,
                "drivers": {
                    "producer": {"program":"/bin/sh", "args":[fixture.root.join("producer.sh")]},
                    "verifier": {"program":"/bin/sh", "args":[fixture.root.join("verifier.sh")],
                        "environment_delta":{"JOURNEY_EVIDENCE":fixture.root.join("evidence")}}
                }
            }))
            .expect("driver configuration"),
        )
        .expect("private fixture config");
        fixture.start();
        fixture
    }

    fn repository(&self) -> PathBuf {
        self.root.join("repository")
    }

    fn git(&self, args: &[&str]) -> String {
        let result = Process::new("git")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(["-c", "core.hooksPath=/dev/null"])
            .arg("-C")
            .arg(self.repository())
            .args(args)
            .output()
            .expect("Git process");
        assert!(
            result.status.success(),
            "Git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout)
            .expect("Git text")
            .trim()
            .to_owned()
    }

    fn start(&mut self) {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("daemon.log"))
            .expect("daemon log");
        self.daemon = Some(
            Process::new(env!("CARGO_BIN_EXE_superplexr-server"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .arg("--socket")
                .arg(self.root.join("s"))
                .arg("--state-dir")
                .arg(self.root.join("state"))
                .stdout(Stdio::from(log.try_clone().expect("log handle")))
                .stderr(Stdio::from(log))
                .spawn()
                .expect("real daemon"),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if ControlClient::connect(self.root.join("s")).is_ok() {
                break;
            }
            assert!(
                self.daemon
                    .as_mut()
                    .expect("daemon")
                    .try_wait()
                    .expect("status")
                    .is_none(),
                "daemon exited: {:?}",
                fs::read_to_string(self.root.join("daemon.log"))
            );
            assert!(Instant::now() < deadline, "daemon startup deadline");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn client(&self) -> ControlClient {
        ControlClient::connect(self.root.join("s")).expect("owner client")
    }

    fn restart(&mut self) {
        let mut child = self.daemon.take().expect("daemon");
        child.kill().expect("crash only fixture daemon");
        child.wait().expect("reap daemon");
        self.start();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.daemon.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Only the uniquely created fixture directory; never a user checkout.
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn verified(command: VerifiedDeliveryCommand) -> Command {
    Command::VerifiedDelivery { command }
}

fn commit(client: &ControlClient, mission: MissionId, command: Command) -> Mission {
    client
        .dispatch(mission, command)
        .expect("workflow command")
        .1
}

fn await_condition(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "workflow stage deadline");
        thread::sleep(Duration::from_millis(20));
    }
}

fn finished(client: &ControlClient, mission: MissionId, run: RunId) -> Mission {
    await_condition(|| {
        client.get_mission(mission).expect("Mission").runs[&run]
            .status
            .is_finished()
    });
    let state = client.get_mission(mission).expect("finished Mission");
    assert_eq!(state.runs[&run].outcome, Some(FinishOutcome::Succeeded));
    state
}

fn prepare_producer(
    fixture: &Fixture,
    client: &ControlClient,
    mission: MissionId,
) -> (RunId, RunCheckoutSummary) {
    let run = RunId::new();
    commit(
        client,
        mission,
        Command::PlanRun {
            run_id: run,
            parent: None,
            dependencies: vec![],
            retry_of: None,
            actor: Actor::agent("producer", "producer").expect("actor"),
            objective: "Deliver a program that prints 42".into(),
            priority: RunPriority::Normal,
        },
    );
    let checkout = client
        .prepare_run_checkout(mission, run, fixture.repository(), "HEAD")
        .expect("managed producer checkout");
    commit(
        client,
        mission,
        verified(VerifiedDeliveryCommand::DeclareChangeIntent {
            run_id: run,
            expected_version: None,
            spec: ChangeIntentSpec {
                repository_identity: fixture.repository().to_string_lossy().into_owned(),
                base_revision: checkout.base_revision.clone(),
                claims: vec![ChangeClaim {
                    path: "product.sh".into(),
                    operation: ChangeOperation::Modify,
                    scope: ChangeScope::Committed,
                }],
            },
        }),
    );
    commit(
        client,
        mission,
        verified(VerifiedDeliveryCommand::SetVerificationPolicy {
            run_id: run,
            policy: VerificationPolicy::Independent,
        }),
    );
    (run, checkout)
}

fn publish(
    client: &ControlClient,
    mission: MissionId,
    run: RunId,
    checkout: &Path,
) -> (RunCandidate, Uuid, Command) {
    let (candidate, key, command, session) = publish_while_running(client, mission, run, checkout);
    let terminal = client.terminal(session);
    // Launch owns a different Surface. This fixture's owner explicitly hands
    // input to its test Surface; no shared client can perform this takeover.
    terminal
        .claim_control(true)
        .expect("fixture owner input Control");
    terminal
        .paste(b"finish\n".to_vec(), true)
        .expect("finish producer");
    finished(client, mission, run);
    (candidate, key, command)
}

fn publish_while_running(
    client: &ControlClient,
    mission: MissionId,
    run: RunId,
    checkout: &Path,
) -> (RunCandidate, Uuid, Command, SessionId) {
    let admitted = commit(
        client,
        mission,
        verified(VerifiedDeliveryCommand::AdmitChangeIntent {
            run_id: run,
            expected_version: 1,
            lease_epoch: 0,
        }),
    );
    let session = SessionId::new();
    client
        .launch_configured_agent_run_in_checkout(
            mission,
            run,
            session,
            "journey producer",
            GridSize::new(80, 24).expect("grid"),
        )
        .expect("producer launch");
    let expected = if admitted
        .verified_delivery
        .delivery_run_inputs
        .contains_key(&run)
    {
        "42"
    } else {
        "41"
    };
    await_condition(|| {
        fs::read_to_string(checkout.join("product.sh")).is_ok_and(|text| text.contains(expected))
    });
    let command = verified(VerifiedDeliveryCommand::SubmitCandidate {
        run_id: run,
        candidate: RunCandidate {
            revision: "server-authored".into(),
            content_sha256: "0".repeat(64),
            execution_lease_epoch: Some(
                admitted.verified_delivery.change_intents[&run].lease_epoch,
            ),
            realized_changes: None,
            artifact_ids: vec![],
            submitted_by: ActorId::new("producer").expect("actor"),
        },
    });
    let key = Uuid::new_v4();
    let state = client
        .dispatch_idempotent(mission, None, key, command.clone())
        .expect("freeze actual producer bytes")
        .1;
    let candidate = state.verified_delivery.candidates[&run].clone();
    (candidate, key, command, session)
}

fn examine(
    fixture: &Fixture,
    client: &ControlClient,
    mission: MissionId,
    subject: RunId,
    candidate: &RunCandidate,
) -> EvaluationReceipt {
    let run = RunId::new();
    let state = commit(
        client,
        mission,
        Command::CreateVerifierRun {
            subject_run_id: subject,
            verifier_run_id: run,
            actor: Actor::agent("independent-verifier", "verifier").expect("actor"),
            priority: RunPriority::Normal,
        },
    );
    assert_eq!(
        &state.verified_delivery.delivery_run_inputs[&run].candidate,
        candidate
    );
    let contract_artifact = candidate
        .artifact_ids
        .iter()
        .filter_map(|id| state.artifacts.get(id))
        .find(|artifact| artifact.media_type == "application/vnd.superplexr.candidate-review+json")
        .expect("server-authored review contract");
    let contract_bytes = fs::read(&contract_artifact.locator).expect("retained review contract");
    assert_eq!(
        contract_artifact.digest,
        Some(format!("sha256:{:x}", Sha256::digest(&contract_bytes)))
    );
    let contract: serde_json::Value = serde_json::from_slice(&contract_bytes).expect("review JSON");
    assert_eq!(contract["snapshot_revision"], candidate.revision);
    assert_eq!(contract["patch_sha256"], candidate.content_sha256);
    let required_checks: Vec<_> = contract["required_checks"]
        .as_array()
        .expect("required checks")
        .iter()
        .map(|check| {
            assert_eq!(
                check["state"], "not_run",
                "the frozen contract never fabricates verification"
            );
            check["id"].as_str().expect("check identity")
        })
        .collect();
    let checkout = client
        .prepare_run_checkout(mission, run, fixture.repository(), &candidate.revision)
        .expect("frozen verifier checkout");
    // Even an owner-supplied different cwd cannot redirect a delivery Run.
    let preview = client
        .preview_configured_agent_run(
            mission,
            run,
            fixture.repository(),
            GridSize::new(80, 24).expect("grid"),
        )
        .expect("verifier preview");
    assert_eq!(preview.cwd, checkout.worktree_path);
    client
        .launch_configured_agent_run_in_checkout(
            mission,
            run,
            SessionId::new(),
            "independent examination",
            GridSize::new(80, 24).expect("grid"),
        )
        .expect("verifier launch");
    finished(client, mission, run);
    let report = fixture.root.join("evidence").join(format!("{run}.log"));
    let bytes = fs::read(&report).expect("actual verifier results");
    let artifact = ArtifactId::new();
    commit(
        client,
        mission,
        Command::RecordArtifact {
            artifact_id: artifact,
            run_id: run,
            name: "Executed fixture review checks".into(),
            media_type: "text/plain".into(),
            locator: report.to_string_lossy().into_owned(),
            digest: Some(format!("sha256:{:x}", Sha256::digest(&bytes))),
        },
    );
    let checks: Vec<_> = std::str::from_utf8(&bytes)
        .expect("report text")
        .lines()
        .map(|line| {
            let (name, result) = line.split_once('=').expect("check result");
            assert!(["0", "1"].contains(&result));
            EvaluationCheck {
                name: name.into(),
                passed: result == "0",
                evidence: vec![artifact],
            }
        })
        .collect();
    assert_eq!(
        checks
            .iter()
            .map(|check| check.name.as_str())
            .collect::<Vec<_>>(),
        required_checks,
        "every check in the normalized contract needs an actual result"
    );
    let passed = |name: &str| {
        checks
            .iter()
            .find(|check| check.name == name)
            .expect("required check")
            .passed
    };
    EvaluationReceipt {
        artifact_id: ArtifactId::new(),
        subject_run_id: subject,
        verifier_run_id: run,
        candidate_sha256: candidate.content_sha256.clone(),
        verdict: if checks.iter().all(|check| check.passed) {
            EvaluationVerdict::Passed
        } else {
            EvaluationVerdict::Failed
        },
        delivery_validated: passed("delivery"),
        repeatable: passed("repeatability"),
        checks,
        summary:
            "Observed exit results from an independent fixture verifier; not an owner acceptance"
                .into(),
    }
}

fn accept(run: RunId) -> Command {
    Command::AcceptRunResult {
        run_id: run,
        by: ActorId::new("owner").expect("owner"),
        note: "Reviewed exact verified candidate".into(),
    }
}

#[test]
fn delivery_journey_returns_retries_verifies_and_accepts_across_daemon_restarts() {
    let mut fixture = Fixture::new();
    let client = fixture.client();
    let mission = MissionId::new();
    client
        .create_mission(
            mission,
            "Verified delivery journey",
            Actor::human("owner").expect("owner"),
        )
        .expect("Mission");
    let (producer, checkout) = prepare_producer(&fixture, &client, mission);
    let (candidate, key, submission) = publish(&client, mission, producer, &checkout.worktree_path);
    assert!(
        client.dispatch(mission, accept(producer)).is_err(),
        "independent verification is mandatory"
    );
    // Post-publication mutable work cannot change the verifier's input.
    fs::write(
        checkout.worktree_path.join("product.sh"),
        "#!/bin/sh\nprintf '999\\n'\n",
    )
    .expect("later unpublished edit");
    let failed_receipt = examine(&fixture, &client, mission, producer, &candidate);
    assert_eq!(failed_receipt.verdict, EvaluationVerdict::Failed);
    assert!(
        failed_receipt
            .checks
            .iter()
            .find(|check| check.name == "provenance")
            .expect("provenance")
            .passed
    );
    commit(
        &client,
        mission,
        verified(VerifiedDeliveryCommand::RecordEvaluationReceipt {
            receipt: failed_receipt,
        }),
    );
    assert!(
        client.dispatch(mission, accept(producer)).is_err(),
        "failed checks do not grant acceptance"
    );
    let return_note = "Return 42, not 41; preserve the exact candidate lineage";
    commit(
        &client,
        mission,
        Command::RejectRunResult {
            run_id: producer,
            by: ActorId::new("owner").expect("owner"),
            note: return_note.into(),
        },
    );
    let retry = RunId::new();
    let retry_key = Uuid::new_v4();
    let retry_command = Command::RetryReturnedRun {
        source_run_id: producer,
        retry_run_id: retry,
        actor: Actor::agent("producer", "producer").expect("actor"),
        priority: RunPriority::Normal,
    };
    client
        .dispatch_idempotent(mission, None, retry_key, retry_command.clone())
        .expect("retry plan");
    drop(client);
    fixture.restart();
    let client = fixture.client();
    client
        .dispatch_idempotent(mission, None, key, submission)
        .expect("candidate replay after restart");
    client
        .dispatch_idempotent(mission, None, retry_key, retry_command)
        .expect("retry replay after restart");
    let recovered = client.get_mission(mission).expect("recovered Mission");
    assert_eq!(
        recovered.runs.len(),
        3,
        "no duplicate producer, verifier or retry"
    );
    assert_eq!(recovered.verified_delivery.candidates[&producer], candidate);
    assert_eq!(
        recovered.verified_delivery.delivery_run_inputs[&retry]
            .return_note
            .as_deref(),
        Some(return_note)
    );
    assert_eq!(
        recovered
            .verified_delivery
            .verification_policies
            .get(&retry),
        Some(&VerificationPolicy::Independent)
    );
    let retry_checkout = client
        .prepare_run_checkout(mission, retry, fixture.repository(), &candidate.revision)
        .expect("retry checkout");
    assert!(
        fs::read_to_string(retry_checkout.worktree_path.join("product.sh"))
            .expect("retry base")
            .contains("41")
    );
    let (retry_candidate, _, _) = publish(&client, mission, retry, &retry_checkout.worktree_path);
    assert_ne!(retry_candidate.content_sha256, candidate.content_sha256);
    assert!(
        client.dispatch(mission, accept(retry)).is_err(),
        "old receipt cannot settle retry"
    );
    let receipt = examine(&fixture, &client, mission, retry, &retry_candidate);
    assert_eq!(receipt.verdict, EvaluationVerdict::Passed);
    let mut wrong = receipt.clone();
    wrong.candidate_sha256 = candidate.content_sha256;
    assert!(
        client
            .dispatch(
                mission,
                verified(VerifiedDeliveryCommand::RecordEvaluationReceipt { receipt: wrong })
            )
            .is_err(),
        "wrong candidate digest rejected"
    );
    let receipt_key = Uuid::new_v4();
    let receipt_command = verified(VerifiedDeliveryCommand::RecordEvaluationReceipt {
        receipt: receipt.clone(),
    });
    client
        .dispatch_idempotent(mission, None, receipt_key, receipt_command.clone())
        .expect("record executed checks");
    drop(client);
    fixture.restart();
    let client = fixture.client();
    client
        .dispatch_idempotent(mission, None, receipt_key, receipt_command)
        .expect("receipt replay after restart");
    let settlement_key = Uuid::new_v4();
    client
        .dispatch_idempotent(mission, None, settlement_key, accept(retry))
        .expect("explicit owner acceptance");
    client
        .dispatch_idempotent(mission, None, settlement_key, accept(retry))
        .expect("safe settlement replay");
    let result = client.get_mission(mission).expect("final Mission");
    assert_eq!(result.runs.len(), 4);
    assert_eq!(result.runs[&producer].disposition, RunDisposition::Rejected);
    assert_eq!(result.runs[&retry].disposition, RunDisposition::Accepted);
    assert_eq!(result.runs[&retry].retry_of, Some(producer));
    assert_eq!(result.verified_delivery.evaluation_receipts.len(), 2);
    assert_eq!(
        result.verified_delivery.evaluation_receipts[&receipt.artifact_id],
        receipt
    );
    for check in &receipt.checks {
        for evidence in &check.evidence {
            let artifact = &result.artifacts[evidence];
            let bytes = fs::read(&artifact.locator).expect("retained evidence after restart");
            assert_eq!(
                artifact.digest,
                Some(format!("sha256:{:x}", Sha256::digest(bytes)))
            );
        }
    }
    assert_eq!(
        client.list_all_terminals().expect("terminal history").len(),
        4,
        "no process relaunch on recovery"
    );
    assert_eq!(
        fixture.git(&["status", "--porcelain"]),
        "",
        "source checkout remains untouched"
    );
    assert!(
        fs::read_to_string(fixture.repository().join("product.sh"))
            .expect("original source")
            .contains("0"),
        "acceptance is not an implicit merge"
    );
}

#[test]
fn delivery_journey_crash_retains_candidate_but_cannot_claim_success_or_relaunch() {
    let mut fixture = Fixture::new();
    let client = fixture.client();
    let mission = MissionId::new();
    client
        .create_mission(
            mission,
            "Interrupted delivery",
            Actor::human("owner").expect("owner"),
        )
        .expect("Mission");
    let (run, checkout) = prepare_producer(&fixture, &client, mission);
    let (candidate, key, submission, session) =
        publish_while_running(&client, mission, run, &checkout.worktree_path);
    await_condition(|| {
        client
            .terminal(session)
            .snapshot()
            .expect("live frame")
            .rows
            .iter()
            .any(|row| row.text().contains("producer-ready"))
    });
    assert!(
        !client.get_mission(mission).expect("live Mission").runs[&run]
            .status
            .is_finished()
    );
    drop(client);
    fixture.restart();
    let client = fixture.client();
    let recovered = client.get_mission(mission).expect("recovered Mission");
    assert_eq!(recovered.runs[&run].outcome, Some(FinishOutcome::Failed));
    assert_eq!(
        recovered.verified_delivery.change_intents[&run].state,
        superplexr_core::ChangeIntentState::Released
    );
    assert_eq!(recovered.verified_delivery.candidates[&run], candidate);
    assert!(
        client.dispatch(mission, accept(run)).is_err(),
        "retained Candidate cannot turn interrupted execution into success"
    );
    assert!(
        client
            .dispatch(
                mission,
                Command::CreateVerifierRun {
                    subject_run_id: run,
                    verifier_run_id: RunId::new(),
                    actor: Actor::agent("independent-verifier", "verifier").expect("actor"),
                    priority: RunPriority::Normal
                }
            )
            .is_err(),
        "interrupted producer is not reviewable"
    );
    client
        .dispatch_idempotent(mission, None, key, submission)
        .expect("publication replay is historical, not new work");
    let state = client.get_mission(mission).expect("state after replay");
    assert_eq!(state.version, recovered.version);
    assert_eq!(state.runs[&run].outcome, Some(FinishOutcome::Failed));
    let terminals = client.list_all_terminals().expect("retained Sessions");
    assert_eq!(terminals.len(), 1);
    assert_eq!(terminals[0].session_id, session);
    assert!(terminals[0].controller_surface_id.is_none());
    assert!(
        client
            .terminal(session)
            .snapshot()
            .expect("retained output")
            .rows
            .iter()
            .any(|row| row.text().contains("producer-ready"))
    );
    drop(client);
    fixture.restart();
    let twice = fixture
        .client()
        .get_mission(mission)
        .expect("second recovery");
    assert_eq!(
        twice.version, recovered.version,
        "recovery is itself idempotent"
    );
    assert_eq!(twice.runs.len(), 1);
}
