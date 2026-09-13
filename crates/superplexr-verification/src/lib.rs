//! Bounded local verification and resumable evidence collection. Checks execute
//! only from an explicit owner-controlled plan; collection never executes code.
mod launch;
mod rust_project;
mod status;
pub use launch::{LaunchOutcome, LaunchTarget, PreparedPlan, Setup, launch, prepare};
pub use rust_project::{execute_rust_project_check, write_rust_project_plan};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub use status::inspect;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command as Process, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use superplexr_client::ControlClient;
use superplexr_core::{
    ArtifactId, Command, DeliveryRunPurpose, EvaluationCheck, EvaluationReceipt, EvaluationVerdict,
    FinishOutcome, MissionId, RunId, VerifiedDeliveryCommand,
};
pub use superplexr_core::{ReceiptStatus, VerificationNextAction, VerificationStatus};
use thiserror::Error;
use uuid::Uuid;

const MAX_DOCUMENT: usize = 65_536;
const MAX_OUTPUT: usize = 1_048_576;
const REQUIRED: [&str; 6] = [
    "format",
    "lint",
    "tests",
    "delivery",
    "provenance",
    "repeatability",
];

#[derive(Debug, Error)]
pub enum VerificationError {
    #[error("verification I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("verification JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Client(#[from] superplexr_client::ClientError),
    #[error("verification refused: {0}")]
    Invalid(String),
}
type Result<T> = std::result::Result<T, VerificationError>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: u16,
    checks: Vec<Check>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Check {
    id: String,
    program: PathBuf,
    args: Vec<String>,
    timeout_seconds: u64,
    output_limit_bytes: usize,
}

impl Plan {
    fn validate(&self) -> Result<()> {
        let names: BTreeSet<_> = self.checks.iter().map(|check| check.id.as_str()).collect();
        if self.version != 1
            || self.checks.len() != REQUIRED.len()
            || names != REQUIRED.into_iter().collect()
        {
            return invalid(
                "plan version 1 requires exactly format, lint, tests, delivery, provenance, repeatability",
            );
        }
        for check in &self.checks {
            if !check.program.is_absolute()
                || check.args.len() > 64
                || check
                    .args
                    .iter()
                    .any(|arg| arg.len() > 4096 || arg.contains('\0'))
                || !(1..=3600).contains(&check.timeout_seconds)
                || !(1..=MAX_OUTPUT).contains(&check.output_limit_bytes)
            {
                return invalid(
                    "checks require an absolute executable, <=64 bounded arguments, 1–3600 seconds, and 1–1048576 output bytes per stream",
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Binding {
    mission: MissionId,
    verifier: RunId,
    subject: RunId,
    revision: String,
    candidate_sha256: String,
}

impl Binding {
    fn environment() -> Result<Self> {
        let value = |name: &str| {
            std::env::var(name)
                .map_err(|_| VerificationError::Invalid(format!("missing runtime binding {name}")))
        };
        if value("SUPERPLEXR_DELIVERY_PURPOSE")? != "verification" {
            return invalid("only a runtime-launched verifier may execute checks");
        }
        let binding = Self {
            mission: value("SUPERPLEXR_MISSION_ID")?
                .parse()
                .map_err(|_| VerificationError::Invalid("invalid Mission ID".into()))?,
            verifier: value("SUPERPLEXR_RUN_ID")?
                .parse()
                .map_err(|_| VerificationError::Invalid("invalid verifier ID".into()))?,
            subject: value("SUPERPLEXR_SOURCE_RUN_ID")?
                .parse()
                .map_err(|_| VerificationError::Invalid("invalid subject ID".into()))?,
            revision: value("SUPERPLEXR_CANDIDATE_REVISION")?,
            candidate_sha256: value("SUPERPLEXR_CANDIDATE_SHA256")?,
        };
        if binding.revision.len() > 256 || binding.candidate_sha256.len() != 64 {
            return invalid("invalid candidate identity");
        }
        Ok(binding)
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Outcome {
    Exit { code: Option<i32> },
    Timeout,
    OutputLimit,
    SpawnFailed { message: String },
}

impl Outcome {
    fn passed(&self) -> bool {
        matches!(self, Self::Exit { code: Some(0) })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CheckResult {
    id: String,
    outcome: Outcome,
    elapsed_ms: u64,
    stdout_sha256: String,
    stderr_sha256: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Report {
    version: u16,
    binding: Binding,
    plan_sha256: String,
    candidate_clean: bool,
    checks: Vec<CheckResult>,
}

fn invalid<T>(message: &str) -> Result<T> {
    Err(VerificationError::Invalid(message.into()))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn private_directory(path: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    // SAFETY: geteuid has no preconditions and only reads process credentials.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return invalid("evidence root must be an existing owner-only directory, not a symlink");
    }
    Ok(path.canonicalize()?)
}

fn read_private(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    // SAFETY: geteuid has no preconditions and only reads process credentials.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || metadata.len() > maximum as u64
    {
        return invalid("evidence/plan must be a bounded owner-only regular file");
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return invalid("document grew beyond its limit");
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

struct ChildGroup(Child, bool);
impl ChildGroup {
    fn terminate(&mut self) {
        if self.1 {
            return;
        }
        self.1 = true;
        // SAFETY: the child was spawned into a new process group whose ID is
        // its PID. Negative PID targets only that check's process group.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Drop for ChildGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> Result<()> {
    // SAFETY: the pipe owns a live fd. fcntl does not retain any pointer.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

// Bound each drain as well as retained bytes, so a busy writer cannot starve
// timeout/exit checks. Pipes are nonblocking; no detached reader threads exist.
fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, limit: usize) -> Result<(bool, bool)> {
    let mut buffer = [0; 4096];
    for _ in 0..16 {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok((true, false)),
            Ok(count) => {
                let remaining = limit.saturating_sub(bytes.len());
                bytes.extend_from_slice(&buffer[..count.min(remaining)]);
                if count > remaining {
                    return Ok((false, true));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok((false, false))
}

fn run_check(
    check: &Check,
    cwd: &Path,
    cancelled: &AtomicBool,
) -> Result<(CheckResult, Vec<u8>, Vec<u8>)> {
    if cancelled.load(Ordering::Relaxed) {
        return invalid("verification cancelled; no receipt was produced");
    }
    let started = Instant::now();
    let mut command = Process::new(&check.program);
    command
        .args(&check.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // Check subprocesses do not receive the verifier's agent-channel binding
    // or caller-controlled Git redirection. This is still trusted OS-user code.
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("SUPERPLEXR_")
            || text.starts_with("ULTRAPLEXR_")
            || text.starts_with("TERMI9NE_")
            || text.starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let outcome = match command.spawn() {
        Err(error) => Outcome::SpawnFailed {
            message: error.to_string(),
        },
        Ok(child) => {
            let mut group = ChildGroup(child, false);
            let mut out = group
                .0
                .stdout
                .take()
                .ok_or_else(|| VerificationError::Invalid("missing stdout pipe".into()))?;
            let mut err = group
                .0
                .stderr
                .take()
                .ok_or_else(|| VerificationError::Invalid("missing stderr pipe".into()))?;
            nonblocking(&out)?;
            nonblocking(&err)?;
            let mut exited = None;
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    return invalid("verification cancelled; no receipt was produced");
                }
                let (out_done, out_full) = drain(&mut out, &mut stdout, check.output_limit_bytes)?;
                let (err_done, err_full) = drain(&mut err, &mut stderr, check.output_limit_bytes)?;
                if out_full || err_full {
                    break Outcome::OutputLimit;
                }
                if exited.is_none() {
                    exited = group.0.try_wait()?;
                }
                if let Some(status) = exited {
                    // Background children cannot outlive a completed check or
                    // keep its pipes open indefinitely.
                    if out_done && err_done {
                        break Outcome::Exit {
                            code: status.code(),
                        };
                    }
                    group.terminate();
                }
                if started.elapsed() >= Duration::from_secs(check.timeout_seconds) {
                    break Outcome::Timeout;
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    Ok((
        CheckResult {
            id: check.id.clone(),
            outcome,
            elapsed_ms: started.elapsed().as_millis() as u64,
            stdout_sha256: digest(&stdout),
            stderr_sha256: digest(&stderr),
        },
        stdout,
        stderr,
    ))
}

fn candidate_clean(cwd: &Path, revision: &str, cancelled: &AtomicBool) -> Result<bool> {
    for (args, expected) in [
        (vec!["rev-parse", "--verify", "HEAD"], revision),
        (vec!["status", "--porcelain"], ""),
    ] {
        let check = Check {
            id: "candidate_identity".into(),
            program: "/usr/bin/git".into(),
            args: [
                vec![
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ],
                args,
            ]
            .concat()
            .into_iter()
            .map(str::to_owned)
            .collect(),
            timeout_seconds: 10,
            output_limit_bytes: MAX_DOCUMENT,
        };
        let (result, out, _) = run_check(&check, cwd, cancelled)?;
        if !result.outcome.passed() || String::from_utf8_lossy(&out).trim() != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Execute once inside a verifier Run. Existing output refuses replay. Partial
/// directories are intentionally retained after failure; they cannot be collected.
pub async fn execute(plan_path: PathBuf, evidence_root: PathBuf) -> Result<PathBuf> {
    execute_reviewed(plan_path, evidence_root, None).await
}

/// Execute only the plan bytes approved by a client, when a digest is supplied.
pub async fn execute_reviewed(
    plan_path: PathBuf,
    evidence_root: PathBuf,
    expected_plan_sha256: Option<String>,
) -> Result<PathBuf> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        execute_blocking(
            &plan_path,
            &evidence_root,
            expected_plan_sha256.as_deref(),
            &worker_cancelled,
        )
    });
    let result = tokio::select! {
        result=&mut worker => result,
        _=async {tokio::select! {_=terminate.recv()=>{}, _=interrupt.recv()=>{}, _=hangup.recv()=>{}}} => {
            cancelled.store(true,Ordering::Relaxed);
            worker.await
        }
    };
    result.map_err(|error| {
        VerificationError::Invalid(format!("verification worker stopped: {error}"))
    })?
}

fn execute_blocking(
    plan_path: &Path,
    evidence_root: &Path,
    expected_plan_sha256: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<PathBuf> {
    let binding = Binding::environment()?;
    let plan_bytes = read_private(plan_path, MAX_DOCUMENT)?;
    if expected_plan_sha256.is_some_and(|expected| expected != digest(&plan_bytes)) {
        return invalid("plan changed after review; review it again before execution");
    }
    let plan: Plan = serde_json::from_slice(&plan_bytes)?;
    plan.validate()?;
    let cwd = std::env::current_dir()?.canonicalize()?;
    let root = private_directory(evidence_root)?;
    if root.starts_with(&cwd) || plan_path.canonicalize()?.starts_with(&cwd) {
        return invalid("keep the trusted plan and evidence outside the candidate checkout");
    }
    if !candidate_clean(&cwd, &binding.revision, cancelled)? {
        return invalid("verifier checkout must be clean at the exact frozen Candidate");
    }
    let directory = root.join(binding.verifier.to_string());
    fs::DirBuilder::new().mode(0o700).create(&directory)?;
    write_new(&directory.join("plan.json"), &plan_bytes)?;
    let mut checks = Vec::new();
    for (index, check) in plan.checks.iter().enumerate() {
        let (result, stdout, stderr) = run_check(check, &cwd, cancelled)?;
        write_new(&directory.join(format!("{index}.stdout")), &stdout)?;
        write_new(&directory.join(format!("{index}.stderr")), &stderr)?;
        checks.push(result);
    }
    let report = Report {
        version: 1,
        plan_sha256: digest(&plan_bytes),
        candidate_clean: candidate_clean(&cwd, &binding.revision, cancelled)?,
        binding,
        checks,
    };
    let bytes = serde_json::to_vec_pretty(&report)?;
    write_new(&directory.join("report.pending"), &bytes)?;
    // Publish without replacing an existing path. A pending file alone is
    // never accepted by collection.
    fs::hard_link(
        directory.join("report.pending"),
        directory.join("report.json"),
    )?;
    fs::remove_file(directory.join("report.pending"))?;
    File::open(&directory)?.sync_all()?;
    Ok(directory.join("report.json"))
}

fn stable_id(report_sha: &str, label: &str) -> Uuid {
    let bytes = Sha256::digest(format!("superplexr-verification-v1\0{report_sha}\0{label}"));
    let mut id = [0; 16];
    id.copy_from_slice(&bytes[..16]);
    Uuid::from_bytes(id)
}

/// Validate all local evidence before graph writes. Stable IDs permit collection
/// to resume after partial commits without repeating checks or accepting work.
pub fn collect(
    client: &ControlClient,
    mission: MissionId,
    verifier: RunId,
    evidence_root: &Path,
) -> Result<EvaluationReceipt> {
    if client.is_shared() {
        return invalid("only the owner may collect verification evidence");
    }
    let root = private_directory(evidence_root)?;
    let directory = private_directory(&root.join(verifier.to_string()))?;
    let bytes = read_private(&directory.join("report.json"), MAX_DOCUMENT)?;
    let report: Report = serde_json::from_slice(&bytes)?;
    let report_sha = digest(&bytes);
    let plan_bytes = read_private(&directory.join("plan.json"), MAX_DOCUMENT)?;
    let plan: Plan = serde_json::from_slice(&plan_bytes)?;
    plan.validate()?;
    if report.version != 1
        || report.binding.mission != mission
        || report.binding.verifier != verifier
        || digest(&plan_bytes) != report.plan_sha256
        || report.checks.len() != plan.checks.len()
    {
        return invalid("report/plan identity mismatch");
    }
    let state = client.get_mission(mission)?;
    let input = state
        .verified_delivery
        .delivery_run_inputs
        .get(&verifier)
        .ok_or_else(|| VerificationError::Invalid("Run has no frozen verification input".into()))?;
    if input.purpose != DeliveryRunPurpose::Verification
        || input.source_run_id != report.binding.subject
        || input.candidate.revision != report.binding.revision
        || input.candidate.content_sha256 != report.binding.candidate_sha256
        || state
            .runs
            .get(&verifier)
            .is_none_or(|run| run.outcome != Some(FinishOutcome::Succeeded))
    {
        return invalid("report must match an exact successfully finished verifier Run");
    }
    let contract_artifact = input
        .candidate
        .artifact_ids
        .iter()
        .filter_map(|id| state.artifacts.get(id))
        .find(|artifact| artifact.media_type == "application/vnd.superplexr.candidate-review+json")
        .ok_or_else(|| {
            VerificationError::Invalid(
                "Candidate has no retained normalized review contract".into(),
            )
        })?;
    let contract_bytes = read_private(Path::new(&contract_artifact.locator), MAX_OUTPUT)?;
    let contract: serde_json::Value = serde_json::from_slice(&contract_bytes)?;
    let required: BTreeSet<_> = contract["required_checks"]
        .as_array()
        .ok_or_else(|| VerificationError::Invalid("invalid required checks".into()))?
        .iter()
        .filter_map(|check| check["id"].as_str())
        .collect();
    if contract_artifact.digest != Some(format!("sha256:{}", digest(&contract_bytes)))
        || contract["schema_version"] != 1
        || contract["snapshot_revision"] != input.candidate.revision
        || contract["patch_sha256"] != input.candidate.content_sha256
        || required != plan.checks.iter().map(|check| check.id.as_str()).collect()
    {
        return invalid("plan does not cover the exact retained review contract");
    }
    let mut artifacts = vec![
        ("report.json".to_owned(), report_sha.clone()),
        ("plan.json".to_owned(), report.plan_sha256.clone()),
    ];
    for (index, (result, check)) in report.checks.iter().zip(&plan.checks).enumerate() {
        if result.id != check.id {
            return invalid("check identity mismatch");
        }
        for (stream, expected) in [
            ("stdout", &result.stdout_sha256),
            ("stderr", &result.stderr_sha256),
        ] {
            let name = format!("{index}.{stream}");
            let data = read_private(&directory.join(&name), check.output_limit_bytes)?;
            if digest(&data) != *expected {
                return invalid("verification output digest mismatch");
            }
            artifacts.push((name, expected.clone()));
        }
    }
    let report_artifact = ArtifactId::from_uuid(stable_id(&report_sha, "report.json"));
    for (name, sha) in artifacts {
        let id = stable_id(&report_sha, &name);
        client.dispatch_idempotent(
            mission,
            None,
            id,
            Command::RecordArtifact {
                artifact_id: ArtifactId::from_uuid(id),
                run_id: verifier,
                name: format!("Verification {name}"),
                media_type: if name.ends_with(".json") {
                    "application/json"
                } else {
                    "application/octet-stream"
                }
                .into(),
                locator: directory.join(&name).to_string_lossy().into_owned(),
                digest: Some(format!("sha256:{sha}")),
            },
        )?;
    }
    let passed = |id: &str| {
        report
            .checks
            .iter()
            .find(|check| check.id == id)
            .is_some_and(|check| check.outcome.passed())
    };
    let success =
        report.candidate_clean && report.checks.iter().all(|check| check.outcome.passed());
    let mut checks: Vec<_> = report
        .checks
        .iter()
        .enumerate()
        .map(|(index, check)| EvaluationCheck {
            name: check.id.clone(),
            passed: check.outcome.passed(),
            evidence: vec![
                report_artifact,
                ArtifactId::from_uuid(stable_id(&report_sha, &format!("{index}.stdout"))),
                ArtifactId::from_uuid(stable_id(&report_sha, &format!("{index}.stderr"))),
            ],
        })
        .collect();
    checks.push(EvaluationCheck {
        name: "frozen_candidate_unchanged".into(),
        passed: report.candidate_clean,
        evidence: vec![report_artifact],
    });
    let id = stable_id(&report_sha, "receipt");
    let receipt = EvaluationReceipt {
        artifact_id: ArtifactId::from_uuid(id),
        subject_run_id: report.binding.subject,
        verifier_run_id: verifier,
        candidate_sha256: report.binding.candidate_sha256.clone(),
        verdict: if success {
            EvaluationVerdict::Passed
        } else {
            EvaluationVerdict::Failed
        },
        checks,
        delivery_validated: passed("delivery") && report.candidate_clean,
        repeatable: passed("repeatability"),
        summary: format!(
            "Executed owner-configured checks; report sha256:{report_sha}. Owner acceptance remains separate."
        ),
    };
    client.dispatch_idempotent(
        mission,
        None,
        id,
        Command::VerifiedDelivery {
            command: VerifiedDeliveryCommand::RecordEvaluationReceipt {
                receipt: receipt.clone(),
            },
        },
    )?;
    Ok(receipt)
}

#[cfg(test)]
#[path = "verification_tests.rs"]
mod tests;
