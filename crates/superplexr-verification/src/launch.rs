use super::*;
use superplexr_core::{Actor, Mission, RunPhase, RunPriority, SessionId};
use superplexr_protocol::TerminalSessionSpec;
use superplexr_terminal::GridSize;

/// Personal client configuration, not execution authority. Selecting paths
/// never starts work; each launch requires a newly reviewed plan.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub plan_path: PathBuf,
    pub evidence_root: PathBuf,
    pub runner_path: PathBuf,
}

/// The exact plan shown for owner review. Launch rechecks its digest; the worker
/// receives the same digest so a changed plan cannot slip through after launch.
#[derive(Clone, Debug)]
pub struct PreparedPlan {
    pub setup: Setup,
    pub sha256: String,
    pub description: String,
}

/// Create a fresh verifier, or resume launch preparation for a pending verifier.
#[derive(Clone, Copy, Debug)]
pub enum LaunchTarget {
    Subject(RunId),
    /// Caller-reserved identity for recovery after an uncertain creation ACK.
    /// Existing IDs are rejected; use Verifier explicitly to resume one.
    NewVerifier {
        subject: RunId,
        verifier: RunId,
    },
    Verifier(RunId),
}

/// Preserve identity if checkout/launch fails after the new Run is committed.
#[derive(Debug)]
pub struct LaunchOutcome {
    pub run_id: RunId,
    pub mission: Mission,
    pub launch_error: Option<String>,
}

/// Validate explicit local paths and describe every command without executing it.
pub fn prepare(setup: &Setup) -> Result<PreparedPlan> {
    let bytes = read_private(&setup.plan_path, MAX_DOCUMENT)?;
    let plan: Plan = serde_json::from_slice(&bytes)?;
    plan.validate()?;
    let metadata = fs::symlink_metadata(&setup.runner_path)?;
    // SAFETY: geteuid only reads current credentials.
    let uid = unsafe { libc::geteuid() };
    if !metadata.is_file()
        || ![0, uid].contains(&metadata.uid())
        || metadata.mode() & 0o022 != 0
        || metadata.mode() & 0o111 == 0
    {
        return invalid(
            "runner must be an owner/root-controlled regular executable, not group/world writable or a symlink",
        );
    }
    let description = plan
        .checks
        .iter()
        .map(|check| {
            format!(
                "{} · {}s · {} bytes/stream\n{} {}",
                check.id,
                check.timeout_seconds,
                check.output_limit_bytes,
                check.program.display(),
                serde_json::to_string(&check.args).expect("string arguments serialize")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(PreparedPlan {
        setup: Setup {
            plan_path: setup.plan_path.canonicalize()?,
            evidence_root: private_directory(&setup.evidence_root)?,
            runner_path: setup.runner_path.canonicalize()?,
        },
        sha256: digest(&bytes),
        description,
    })
}

/// Launch through the authoritative runtime. No process is started by the UI
/// thread and no owner-configured engines file is edited or overwritten.
pub fn launch(
    client: &ControlClient,
    mission_id: MissionId,
    target: LaunchTarget,
    reviewed: &PreparedPlan,
) -> Result<LaunchOutcome> {
    if client.is_shared() {
        return invalid("only an owner may launch verification");
    }
    let current = prepare(&reviewed.setup)?;
    if current.sha256 != reviewed.sha256 {
        return invalid("plan changed since review");
    }
    let mut mission = client.get_mission(mission_id)?;
    let (source, run_id) = match target {
        LaunchTarget::Subject(source) => (source, RunId::new()),
        LaunchTarget::NewVerifier { subject, verifier } => {
            if mission.runs.contains_key(&verifier) {
                return invalid(
                    "reserved verifier ID already exists; inspect it and explicitly resume only if pending",
                );
            }
            (subject, verifier)
        }
        LaunchTarget::Verifier(run) => {
            let input = mission
                .verified_delivery
                .delivery_run_inputs
                .get(&run)
                .ok_or_else(|| {
                    VerificationError::Invalid("selected Run has no verification input".into())
                })?;
            if input.purpose != DeliveryRunPurpose::Verification
                || mission.runs[&run].phase != RunPhase::Pending
            {
                return invalid("only a pending verifier can resume launch preparation");
            }
            (input.source_run_id, run)
        }
    };
    let source_checkout = client
        .list_run_checkouts(Some(mission_id))?
        .into_iter()
        .find(|checkout| checkout.run_id == source)
        .ok_or_else(|| VerificationError::Invalid("source Run has no managed checkout".into()))?;
    if reviewed
        .setup
        .plan_path
        .starts_with(&source_checkout.repository_root)
        || reviewed
            .setup
            .evidence_root
            .starts_with(&source_checkout.repository_root)
    {
        return invalid("keep trusted plan and evidence outside the source repository");
    }
    if matches!(
        target,
        LaunchTarget::Subject(_) | LaunchTarget::NewVerifier { .. }
    ) {
        mission = client
            .dispatch_at(
                mission_id,
                Some(mission.version),
                Command::CreateVerifierRun {
                    subject_run_id: source,
                    verifier_run_id: run_id,
                    actor: Actor::agent(format!("checks-{run_id}"), "direct-command")
                        .map_err(|error| VerificationError::Invalid(error.to_string()))?,
                    priority: RunPriority::Urgent,
                },
            )?
            .1;
    }
    let revision = mission.verified_delivery.delivery_run_inputs[&run_id]
        .candidate
        .revision
        .clone();
    let attempted = (|| -> Result<Mission> {
        let checkout = client.prepare_run_checkout(
            mission_id,
            run_id,
            source_checkout.repository_root,
            revision,
        )?;
        let (_, launched, _) = client.launch_agent_run(
            mission_id,
            run_id,
            format!("checks-{run_id}"),
            TerminalSessionSpec {
                session_id: SessionId::new(),
                mission_id: Some(mission_id),
                run_id: Some(run_id),
                program: reviewed.setup.runner_path.clone(),
                args: vec![
                    "verification-execute".into(),
                    "--plan".into(),
                    reviewed.setup.plan_path.to_string_lossy().into_owned(),
                    "--evidence-root".into(),
                    reviewed.setup.evidence_root.to_string_lossy().into_owned(),
                    "--plan-sha256".into(),
                    reviewed.sha256.clone(),
                ],
                cwd: checkout.worktree_path,
                environment_delta: Default::default(),
                grid: GridSize::new(120, 36).expect("fixed valid grid"),
            },
        )?;
        Ok(launched)
    })();
    match attempted {
        Ok(mission) => Ok(LaunchOutcome {
            run_id,
            mission,
            launch_error: None,
        }),
        Err(error) => Ok(LaunchOutcome {
            run_id,
            mission: client.get_mission(mission_id).unwrap_or(mission),
            launch_error: Some(error.to_string()),
        }),
    }
}
