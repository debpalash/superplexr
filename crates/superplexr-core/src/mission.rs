use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    ActorId, ArtifactId, DomainError, GrantId, MissionId, RunId, SessionId, SignalId,
    VerifiedDelivery, VerifiedDeliveryCommand, VerifiedDeliveryEvent,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ActorKind {
    Human,
    Agent { engine: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Actor {
    pub id: ActorId,
    pub kind: ActorKind,
}

impl Actor {
    pub fn human(id: impl Into<String>) -> Result<Self, DomainError> {
        Ok(Self {
            id: ActorId::new(id)?,
            kind: ActorKind::Human,
        })
    }

    pub fn agent(id: impl Into<String>, engine: impl Into<String>) -> Result<Self, DomainError> {
        Ok(Self {
            id: ActorId::new(id)?,
            kind: ActorKind::Agent {
                engine: engine.into(),
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MissionStatus {
    Active,
    Completed,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum RunStatus {
    Pending,
    Running,
    AwaitingAttention,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Pending,
    #[default]
    Running,
    Paused,
    Finished,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    Attention,
    Human,
    Resource,
    System,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunDisposition {
    #[default]
    None,
    AwaitingReview,
    Accepted,
    Rejected,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPriority {
    Urgent,
    #[default]
    Normal,
    Background,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunReadiness {
    Waiting,
    Blocked,
    Ready,
    NotPending,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledRun {
    pub run_id: RunId,
    pub actor: Actor,
    pub objective: String,
    pub priority: RunPriority,
    #[serde(default)]
    pub effective_priority: RunPriority,
    #[serde(default)]
    pub ready_since_unix_micros: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchedulerPlan {
    pub max_concurrency: u16,
    pub occupied_slots: u16,
    pub available_slots: u16,
    pub startable: Vec<ScheduledRun>,
    pub ready_queued: Vec<ScheduledRun>,
    pub waiting_dependencies: Vec<RunId>,
    pub blocked_dependencies: Vec<RunId>,
    pub manual: Vec<RunId>,
}

pub(crate) fn validate_driver_snapshot(snapshot: &RunDriverSnapshot) -> Result<(), DomainError> {
    let digest_is_valid = snapshot.process_spec_sha256.len() == 64
        && snapshot
            .process_spec_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    let keys_are_valid = snapshot.environment_keys.len() <= 64
        && snapshot
            .environment_keys
            .iter()
            .all(|key| !key.is_empty() && key.len() <= 1024 && !key.contains(['=', '\0']))
        && snapshot
            .environment_keys
            .windows(2)
            .all(|keys| keys[0] < keys[1]);
    let sandbox_is_valid = match (&snapshot.sandbox_backend, &snapshot.sandbox_profile) {
        (None, None) => !snapshot.sandbox_network_isolated,
        (Some(backend), Some(profile)) => {
            !backend.trim().is_empty()
                && backend.len() <= 128
                && !profile.trim().is_empty()
                && profile.len() <= 128
        }
        _ => false,
    };
    if snapshot.driver_id.trim().is_empty()
        || snapshot.driver_id.len() > 1024
        || !digest_is_valid
        || !keys_are_valid
        || !sandbox_is_valid
    {
        return Err(DomainError::InvalidRunDriverSnapshot);
    }
    Ok(())
}

impl RunStatus {
    #[must_use]
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum Risk {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalKind {
    Progress {
        summary: String,
        percent: Option<u8>,
    },
    InputNeeded {
        question: String,
    },
    ApprovalNeeded {
        operation: String,
        risk: Risk,
    },
    Blocked {
        reason: String,
    },
    Escalation {
        conflict: String,
        evidence: Vec<ArtifactId>,
        requested_resolution: String,
    },
    ArtifactReady {
        name: String,
        locator: String,
    },
}

impl SignalKind {
    #[must_use]
    pub const fn requires_response(&self) -> bool {
        matches!(
            self,
            Self::InputNeeded { .. }
                | Self::ApprovalNeeded { .. }
                | Self::Blocked { .. }
                | Self::Escalation { .. }
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignalResolution {
    pub by: ActorId,
    pub response: String,
    #[serde(default)]
    pub grant_id: Option<GrantId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Signal {
    pub id: SignalId,
    pub run_id: RunId,
    pub kind: SignalKind,
    pub resolution: Option<SignalResolution>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantEnforcement {
    Cooperative,
    Enforced,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Grant {
    pub id: GrantId,
    pub run_id: RunId,
    pub subject: ActorId,
    pub operation: String,
    pub resource_scope: String,
    pub risk: Risk,
    pub issued_by: ActorId,
    pub remaining_uses: u32,
    pub enforcement: GrantEnforcement,
    pub revoked: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub run_id: RunId,
    pub name: String,
    pub media_type: String,
    pub locator: String,
    pub digest: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunDriverSnapshot {
    pub driver_id: String,
    pub profile_version: u16,
    pub process_spec_sha256: String,
    pub argument_count: u16,
    pub environment_keys: Vec<String>,
    #[serde(default)]
    pub sandbox_backend: Option<String>,
    #[serde(default)]
    pub sandbox_profile: Option<String>,
    #[serde(default)]
    pub sandbox_network_isolated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Run {
    pub id: RunId,
    pub parent: Option<RunId>,
    #[serde(default)]
    pub dependencies: Vec<RunId>,
    #[serde(default)]
    pub retry_of: Option<RunId>,
    #[serde(default)]
    pub priority: RunPriority,
    #[serde(default)]
    pub phase: RunPhase,
    #[serde(default)]
    pub outcome: Option<FinishOutcome>,
    #[serde(default)]
    pub disposition: RunDisposition,
    #[serde(default)]
    pub pause_reason: Option<PauseReason>,
    pub actor: Actor,
    #[serde(default)]
    pub planned_at_unix_micros: Option<u64>,
    #[serde(default)]
    pub finished_at_unix_micros: Option<u64>,
    #[serde(default)]
    pub driver_snapshot: Option<RunDriverSnapshot>,
    pub primary_session: Option<SessionId>,
    pub objective: String,
    pub status: RunStatus,
    pub summary: Option<String>,
    #[serde(default)]
    pub settlement: Option<RunSettlement>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunSettlement {
    pub disposition: RunDisposition,
    pub by: ActorId,
    pub note: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Exited { code: Option<i32> },
}

impl SessionStatus {
    #[must_use]
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Exited { .. })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub started_by: Actor,
    pub controller: Actor,
    pub status: SessionStatus,
    pub run_history: Vec<RunId>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FinishOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

impl From<FinishOutcome> for RunStatus {
    fn from(value: FinishOutcome) -> Self {
        match value {
            FinishOutcome::Succeeded => Self::Succeeded,
            FinishOutcome::Failed => Self::Failed,
            FinishOutcome::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    PlanRun {
        run_id: RunId,
        parent: Option<RunId>,
        dependencies: Vec<RunId>,
        retry_of: Option<RunId>,
        actor: Actor,
        objective: String,
        priority: RunPriority,
    },
    StartReadyRun {
        run_id: RunId,
    },
    ResolveRunDriver {
        run_id: RunId,
        snapshot: RunDriverSnapshot,
    },
    PauseRun {
        run_id: RunId,
        reason: PauseReason,
    },
    ResumeRun {
        run_id: RunId,
    },
    CancelRun {
        run_id: RunId,
        summary: String,
    },
    AcceptRunResult {
        run_id: RunId,
        by: ActorId,
        note: String,
    },
    RejectRunResult {
        run_id: RunId,
        by: ActorId,
        note: String,
    },
    CreateVerifierRun {
        subject_run_id: RunId,
        verifier_run_id: RunId,
        actor: Actor,
        priority: RunPriority,
    },
    RetryReturnedRun {
        source_run_id: RunId,
        retry_run_id: RunId,
        actor: Actor,
        priority: RunPriority,
    },
    StartRun {
        run_id: RunId,
        parent: Option<RunId>,
        actor: Actor,
        objective: String,
    },
    RaiseSignal {
        signal_id: SignalId,
        run_id: RunId,
        kind: SignalKind,
    },
    ResolveSignal {
        signal_id: SignalId,
        by: ActorId,
        response: String,
    },
    DecideApproval {
        signal_id: SignalId,
        grant_id: Option<GrantId>,
        by: ActorId,
        allow: bool,
        resource_scope: String,
        use_count: u32,
        enforcement: GrantEnforcement,
    },
    RevokeGrant {
        grant_id: GrantId,
        by: ActorId,
        reason: String,
    },
    RecordArtifact {
        artifact_id: ArtifactId,
        run_id: RunId,
        name: String,
        media_type: String,
        locator: String,
        digest: Option<String>,
    },
    StartSession {
        session_id: SessionId,
        name: String,
        started_by: Actor,
    },
    AssignSession {
        session_id: SessionId,
        run_id: RunId,
    },
    TakeControl {
        session_id: SessionId,
        human: Actor,
    },
    ReturnControl {
        session_id: SessionId,
        human_id: ActorId,
        controller: Actor,
    },
    FinishSession {
        session_id: SessionId,
        exit_code: Option<i32>,
    },
    FinishRun {
        run_id: RunId,
        outcome: FinishOutcome,
        summary: String,
    },
    VerifiedDelivery {
        command: VerifiedDeliveryCommand,
    },
    CompleteMission,
    AbandonMission,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    MissionCreated {
        mission_id: MissionId,
        intent: String,
        created_by: Actor,
    },
    RunPlanned {
        mission_id: MissionId,
        run_id: RunId,
        parent: Option<RunId>,
        dependencies: Vec<RunId>,
        retry_of: Option<RunId>,
        actor: Actor,
        objective: String,
        priority: RunPriority,
        #[serde(default)]
        planned_at_unix_micros: Option<u64>,
    },
    RunExecutionStarted {
        mission_id: MissionId,
        run_id: RunId,
    },
    RunDriverResolved {
        mission_id: MissionId,
        run_id: RunId,
        snapshot: RunDriverSnapshot,
    },
    RunPaused {
        mission_id: MissionId,
        run_id: RunId,
        reason: PauseReason,
    },
    RunResumed {
        mission_id: MissionId,
        run_id: RunId,
    },
    RunCancelled {
        mission_id: MissionId,
        run_id: RunId,
        summary: String,
        #[serde(default)]
        cancelled_at_unix_micros: Option<u64>,
    },
    RunResultAccepted {
        mission_id: MissionId,
        run_id: RunId,
        by: ActorId,
        note: String,
    },
    RunResultRejected {
        mission_id: MissionId,
        run_id: RunId,
        by: ActorId,
        note: String,
    },
    RunStarted {
        mission_id: MissionId,
        run_id: RunId,
        parent: Option<RunId>,
        actor: Actor,
        objective: String,
    },
    SignalRaised {
        mission_id: MissionId,
        signal_id: SignalId,
        run_id: RunId,
        kind: SignalKind,
    },
    SignalResolved {
        mission_id: MissionId,
        signal_id: SignalId,
        resolution: SignalResolution,
    },
    GrantIssued {
        mission_id: MissionId,
        grant: Grant,
    },
    GrantRevoked {
        mission_id: MissionId,
        grant_id: GrantId,
        by: ActorId,
        reason: String,
    },
    ArtifactRecorded {
        mission_id: MissionId,
        artifact: Artifact,
    },
    SessionStarted {
        mission_id: MissionId,
        session_id: SessionId,
        name: String,
        started_by: Actor,
    },
    SessionAssigned {
        mission_id: MissionId,
        session_id: SessionId,
        run_id: RunId,
    },
    SessionControlTaken {
        mission_id: MissionId,
        session_id: SessionId,
        human: Actor,
    },
    SessionControlReturned {
        mission_id: MissionId,
        session_id: SessionId,
        controller: Actor,
    },
    SessionFinished {
        mission_id: MissionId,
        session_id: SessionId,
        exit_code: Option<i32>,
    },
    #[serde(rename = "control_taken")]
    LegacyRunControlTaken {
        mission_id: MissionId,
        run_id: RunId,
        human: Actor,
    },
    #[serde(rename = "control_returned")]
    LegacyRunControlReturned {
        mission_id: MissionId,
        run_id: RunId,
    },
    RunFinished {
        mission_id: MissionId,
        run_id: RunId,
        outcome: FinishOutcome,
        summary: String,
        #[serde(default)]
        finished_at_unix_micros: Option<u64>,
    },
    VerifiedDelivery {
        mission_id: MissionId,
        event: VerifiedDeliveryEvent,
    },
    MissionCompleted {
        mission_id: MissionId,
    },
    MissionAbandoned {
        mission_id: MissionId,
    },
}

impl Event {
    #[must_use]
    pub const fn mission_id(&self) -> MissionId {
        match self {
            Self::MissionCreated { mission_id, .. }
            | Self::RunPlanned { mission_id, .. }
            | Self::RunExecutionStarted { mission_id, .. }
            | Self::RunDriverResolved { mission_id, .. }
            | Self::RunPaused { mission_id, .. }
            | Self::RunResumed { mission_id, .. }
            | Self::RunCancelled { mission_id, .. }
            | Self::RunResultAccepted { mission_id, .. }
            | Self::RunResultRejected { mission_id, .. }
            | Self::RunStarted { mission_id, .. }
            | Self::SignalRaised { mission_id, .. }
            | Self::SignalResolved { mission_id, .. }
            | Self::GrantIssued { mission_id, .. }
            | Self::GrantRevoked { mission_id, .. }
            | Self::ArtifactRecorded { mission_id, .. }
            | Self::SessionStarted { mission_id, .. }
            | Self::SessionAssigned { mission_id, .. }
            | Self::SessionControlTaken { mission_id, .. }
            | Self::SessionControlReturned { mission_id, .. }
            | Self::SessionFinished { mission_id, .. }
            | Self::LegacyRunControlTaken { mission_id, .. }
            | Self::LegacyRunControlReturned { mission_id, .. }
            | Self::RunFinished { mission_id, .. }
            | Self::VerifiedDelivery { mission_id, .. }
            | Self::MissionCompleted { mission_id }
            | Self::MissionAbandoned { mission_id } => *mission_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum AttentionKind {
    HighRiskApproval,
    Escalation,
    Blocked,
    Approval,
    Input,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttentionItem {
    pub kind: AttentionKind,
    pub signal_id: SignalId,
    pub run_id: RunId,
    pub actor: Actor,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Mission {
    pub id: MissionId,
    pub intent: String,
    pub created_by: Actor,
    pub status: MissionStatus,
    pub version: u64,
    pub runs: HashMap<RunId, Run>,
    pub sessions: HashMap<SessionId, Session>,
    pub signals: HashMap<SignalId, Signal>,
    #[serde(default)]
    pub grants: HashMap<GrantId, Grant>,
    #[serde(default)]
    pub artifacts: HashMap<ArtifactId, Artifact>,
    #[serde(default)]
    pub verified_delivery: VerifiedDelivery,
}

impl Mission {
    pub fn create(
        id: MissionId,
        intent: impl Into<String>,
        created_by: Actor,
    ) -> Result<(Self, Event), DomainError> {
        let intent = intent.into();
        if intent.trim().is_empty() {
            return Err(DomainError::EmptyMissionIntent);
        }
        let event = Event::MissionCreated {
            mission_id: id,
            intent,
            created_by,
        };
        let mission = Self::rehydrate(std::slice::from_ref(&event))?;
        Ok((mission, event))
    }

    pub fn rehydrate(events: &[Event]) -> Result<Self, DomainError> {
        let Some(Event::MissionCreated {
            mission_id,
            intent,
            created_by,
        }) = events.first()
        else {
            return Err(DomainError::InvalidEventStream(
                "first event must create the mission".to_owned(),
            ));
        };

        let mut mission = Self {
            id: *mission_id,
            intent: intent.clone(),
            created_by: created_by.clone(),
            status: MissionStatus::Active,
            version: 0,
            runs: HashMap::new(),
            sessions: HashMap::new(),
            signals: HashMap::new(),
            grants: HashMap::new(),
            artifacts: HashMap::new(),
            verified_delivery: VerifiedDelivery::default(),
        };
        for event in events {
            mission.apply(event)?;
        }
        Ok(mission)
    }

    pub fn decide(&self, command: Command) -> Result<Vec<Event>, DomainError> {
        if self.status != MissionStatus::Active {
            return Err(DomainError::MissionClosed);
        }

        let event = match command {
            Command::PlanRun {
                run_id,
                parent,
                dependencies,
                retry_of,
                actor,
                objective,
                priority,
            } => {
                self.validate_plan(run_id, parent, &dependencies, retry_of, &objective)?;
                Event::RunPlanned {
                    mission_id: self.id,
                    run_id,
                    parent,
                    dependencies,
                    retry_of,
                    actor,
                    objective,
                    priority,
                    planned_at_unix_micros: None,
                }
            }
            Command::StartReadyRun { run_id } => {
                let run = self
                    .runs
                    .get(&run_id)
                    .ok_or(DomainError::RunNotFound(run_id))?;
                if run.status != RunStatus::Pending {
                    return Err(DomainError::RunNotPending(run_id));
                }
                if self.run_readiness(run_id)? != RunReadiness::Ready {
                    return Err(DomainError::RunNotReady(run_id));
                }
                self.verified_delivery.ensure_start_admitted(run_id)?;
                Event::RunExecutionStarted {
                    mission_id: self.id,
                    run_id,
                }
            }
            Command::ResolveRunDriver { run_id, snapshot } => {
                let run = self
                    .runs
                    .get(&run_id)
                    .ok_or(DomainError::RunNotFound(run_id))?;
                if run.status != RunStatus::Pending {
                    return Err(DomainError::RunNotPending(run_id));
                }
                if run.driver_snapshot.is_some() {
                    return Err(DomainError::RunDriverConflict(run_id));
                }
                validate_driver_snapshot(&snapshot)?;
                Event::RunDriverResolved {
                    mission_id: self.id,
                    run_id,
                    snapshot,
                }
            }
            Command::PauseRun { run_id, reason } => {
                let run = self.running_run(run_id)?;
                if run.phase != RunPhase::Running {
                    return Err(DomainError::RunNotRunning(run_id));
                }
                Event::RunPaused {
                    mission_id: self.id,
                    run_id,
                    reason,
                }
            }
            Command::ResumeRun { run_id } => {
                let run = self
                    .runs
                    .get(&run_id)
                    .ok_or(DomainError::RunNotFound(run_id))?;
                if run.phase != RunPhase::Paused {
                    return Err(DomainError::RunNotPaused(run_id));
                }
                if self.run_needs_attention(run_id) {
                    return Err(DomainError::RunNeedsAttention(run_id));
                }
                Event::RunResumed {
                    mission_id: self.id,
                    run_id,
                }
            }
            Command::CancelRun { run_id, summary } => {
                let run = self
                    .runs
                    .get(&run_id)
                    .ok_or(DomainError::RunNotFound(run_id))?;
                if run.phase == RunPhase::Finished {
                    return Err(DomainError::RunFinished(run_id));
                }
                Event::RunCancelled {
                    mission_id: self.id,
                    run_id,
                    summary,
                    cancelled_at_unix_micros: None,
                }
            }
            Command::AcceptRunResult { run_id, by, note } => {
                self.reviewable_run(run_id)?;
                self.verified_delivery.ensure_settleable(run_id)?;
                validate_settlement_note(&note)?;
                Event::RunResultAccepted {
                    mission_id: self.id,
                    run_id,
                    by,
                    note,
                }
            }
            Command::RejectRunResult { run_id, by, note } => {
                self.reviewable_run(run_id)?;
                validate_settlement_note(&note)?;
                Event::RunResultRejected {
                    mission_id: self.id,
                    run_id,
                    by,
                    note,
                }
            }
            Command::CreateVerifierRun {
                subject_run_id,
                verifier_run_id,
                actor,
                priority,
            } => {
                if !matches!(actor.kind, ActorKind::Agent { .. }) {
                    return Err(DomainError::DeliveryRunRequiresAgent);
                }
                let source = self.reviewable_run(subject_run_id)?;
                let candidate = self
                    .verified_delivery
                    .candidates
                    .get(&subject_run_id)
                    .ok_or(DomainError::CandidateNotFound(subject_run_id))?;
                let revision = candidate.revision.get(..12).unwrap_or(&candidate.revision);
                let objective = format!(
                    "Verify frozen candidate {revision} for: {}",
                    source.objective
                );
                self.validate_plan(verifier_run_id, None, &[subject_run_id], None, &objective)?;
                let input = self.verified_delivery.delivery_run_input(
                    verifier_run_id,
                    source,
                    crate::DeliveryRunPurpose::Verification,
                )?;
                return Ok(vec![
                    Event::RunPlanned {
                        mission_id: self.id,
                        run_id: verifier_run_id,
                        parent: None,
                        dependencies: vec![subject_run_id],
                        retry_of: None,
                        actor,
                        objective,
                        priority,
                        planned_at_unix_micros: None,
                    },
                    Event::VerifiedDelivery {
                        mission_id: self.id,
                        event: crate::VerifiedDeliveryEvent::DeliveryRunInputFrozen {
                            input: Box::new(input),
                        },
                    },
                ]);
            }
            Command::RetryReturnedRun {
                source_run_id,
                retry_run_id,
                actor,
                priority,
            } => {
                if !matches!(actor.kind, ActorKind::Agent { .. }) {
                    return Err(DomainError::DeliveryRunRequiresAgent);
                }
                let source = self
                    .runs
                    .get(&source_run_id)
                    .ok_or(DomainError::RunNotFound(source_run_id))?;
                let input = self.verified_delivery.delivery_run_input(
                    retry_run_id,
                    source,
                    crate::DeliveryRunPurpose::Retry,
                )?;
                let revision = input
                    .candidate
                    .revision
                    .get(..12)
                    .unwrap_or(&input.candidate.revision);
                let objective =
                    format!("Revise returned candidate {revision}: {}", source.objective);
                self.validate_plan(retry_run_id, None, &[], Some(source_run_id), &objective)?;
                let source_intent = self
                    .verified_delivery
                    .change_intents
                    .get(&source_run_id)
                    .ok_or(DomainError::ChangeIntentNotFound(source_run_id))?;
                let manifest = input
                    .candidate
                    .realized_changes
                    .as_ref()
                    .ok_or(DomainError::InvalidCandidate)?;
                let claims = manifest
                    .changes
                    .iter()
                    .filter(|change| change.operation != crate::ChangeOperation::Delete)
                    .map(|change| crate::ChangeClaim {
                        path: change.path.clone(),
                        operation: crate::ChangeOperation::Modify,
                        scope: crate::ChangeScope::Committed,
                    })
                    .collect::<Vec<_>>();
                if claims.is_empty() {
                    return Err(DomainError::InvalidChangeIntent);
                }
                let retry_intent = crate::ChangeIntent {
                    run_id: retry_run_id,
                    version: 1,
                    state: crate::ChangeIntentState::Proposed,
                    lease_epoch: 0,
                    repository_identity: source_intent.repository_identity.clone(),
                    base_revision: input.candidate.revision.clone(),
                    claims,
                };
                let mut events = vec![
                    Event::RunPlanned {
                        mission_id: self.id,
                        run_id: retry_run_id,
                        parent: None,
                        dependencies: Vec::new(),
                        retry_of: Some(source_run_id),
                        actor,
                        objective,
                        priority,
                        planned_at_unix_micros: None,
                    },
                    Event::VerifiedDelivery {
                        mission_id: self.id,
                        event: crate::VerifiedDeliveryEvent::DeliveryRunInputFrozen {
                            input: Box::new(input),
                        },
                    },
                    Event::VerifiedDelivery {
                        mission_id: self.id,
                        event: crate::VerifiedDeliveryEvent::ChangeIntentDeclared {
                            intent: retry_intent,
                        },
                    },
                ];
                if self.verified_delivery.verification_policy(source_run_id)
                    == crate::VerificationPolicy::Independent
                {
                    events.push(Event::VerifiedDelivery {
                        mission_id: self.id,
                        event: crate::VerifiedDeliveryEvent::VerificationPolicySet {
                            run_id: retry_run_id,
                            policy: crate::VerificationPolicy::Independent,
                        },
                    });
                }
                return Ok(events);
            }
            Command::StartRun {
                run_id,
                parent,
                actor,
                objective,
            } => {
                self.validate_new_run(run_id, parent, &objective)?;
                if let Some(parent_id) = parent
                    && self.runs[&parent_id].status.is_finished()
                {
                    return Err(DomainError::ParentRunNotActive(parent_id));
                }
                Event::RunStarted {
                    mission_id: self.id,
                    run_id,
                    parent,
                    actor,
                    objective,
                }
            }
            Command::RaiseSignal {
                signal_id,
                run_id,
                kind,
            } => {
                self.running_run(run_id)?;
                if self.signals.contains_key(&signal_id) {
                    return Err(DomainError::SignalAlreadyExists(signal_id));
                }
                if let SignalKind::Progress {
                    percent: Some(percent),
                    ..
                } = &kind
                    && *percent > 100
                {
                    return Err(DomainError::InvalidEventStream(
                        "progress percentage must be between 0 and 100".to_owned(),
                    ));
                }
                if let SignalKind::Escalation {
                    conflict,
                    evidence,
                    requested_resolution,
                } = &kind
                    && (conflict.trim().is_empty()
                        || requested_resolution.trim().is_empty()
                        || conflict.len() > 4_096
                        || requested_resolution.len() > 4_096
                        || evidence.len() > 256)
                {
                    return Err(DomainError::InvalidVerifiedDeliveryRecord);
                }
                Event::SignalRaised {
                    mission_id: self.id,
                    signal_id,
                    run_id,
                    kind,
                }
            }
            Command::ResolveSignal {
                signal_id,
                by,
                response,
            } => {
                let signal = self
                    .signals
                    .get(&signal_id)
                    .ok_or(DomainError::SignalNotFound(signal_id))?;
                if !signal.kind.requires_response() {
                    return Err(DomainError::SignalNeedsNoResponse(signal_id));
                }
                if matches!(signal.kind, SignalKind::ApprovalNeeded { .. }) {
                    return Err(DomainError::ApprovalRequiresDecision(signal_id));
                }
                if signal.resolution.is_some() {
                    return Err(DomainError::SignalAlreadyResolved(signal_id));
                }
                Event::SignalResolved {
                    mission_id: self.id,
                    signal_id,
                    resolution: SignalResolution {
                        by,
                        response,
                        grant_id: None,
                    },
                }
            }
            Command::DecideApproval {
                signal_id,
                grant_id,
                by,
                allow,
                resource_scope,
                use_count,
                enforcement,
            } => {
                let signal = self
                    .signals
                    .get(&signal_id)
                    .ok_or(DomainError::SignalNotFound(signal_id))?;
                let SignalKind::ApprovalNeeded { operation, risk } = &signal.kind else {
                    return Err(DomainError::SignalIsNotApproval(signal_id));
                };
                if signal.resolution.is_some() {
                    return Err(DomainError::SignalAlreadyResolved(signal_id));
                }
                if !allow {
                    return Ok(vec![Event::SignalResolved {
                        mission_id: self.id,
                        signal_id,
                        resolution: SignalResolution {
                            by,
                            response: "Denied by operator".to_owned(),
                            grant_id: None,
                        },
                    }]);
                }
                let grant_id = grant_id.ok_or(DomainError::InvalidEventStream(
                    "an allowed approval requires a grant id".to_owned(),
                ))?;
                if self.grants.contains_key(&grant_id) {
                    return Err(DomainError::GrantAlreadyExists(grant_id));
                }
                if use_count == 0 {
                    return Err(DomainError::InvalidGrantUseCount);
                }
                if enforcement == GrantEnforcement::Enforced {
                    return Err(DomainError::EnforcedGrantUnavailable);
                }
                let run = self.running_run(signal.run_id)?;
                let grant = Grant {
                    id: grant_id,
                    run_id: run.id,
                    subject: run.actor.id.clone(),
                    operation: operation.clone(),
                    resource_scope: if resource_scope.trim().is_empty() {
                        operation.clone()
                    } else {
                        resource_scope
                    },
                    risk: *risk,
                    issued_by: by.clone(),
                    remaining_uses: use_count,
                    enforcement,
                    revoked: false,
                };
                return Ok(vec![
                    Event::SignalResolved {
                        mission_id: self.id,
                        signal_id,
                        resolution: SignalResolution {
                            by,
                            response: "Allowed under a cooperative grant".to_owned(),
                            grant_id: Some(grant_id),
                        },
                    },
                    Event::GrantIssued {
                        mission_id: self.id,
                        grant,
                    },
                ]);
            }
            Command::RevokeGrant {
                grant_id,
                by,
                reason,
            } => {
                let grant = self
                    .grants
                    .get(&grant_id)
                    .ok_or(DomainError::GrantNotFound(grant_id))?;
                if grant.revoked {
                    return Err(DomainError::InvalidEventStream(
                        "grant is already revoked".to_owned(),
                    ));
                }
                Event::GrantRevoked {
                    mission_id: self.id,
                    grant_id,
                    by,
                    reason,
                }
            }
            Command::RecordArtifact {
                artifact_id,
                run_id,
                name,
                media_type,
                locator,
                digest,
            } => {
                let run = self
                    .runs
                    .get(&run_id)
                    .ok_or(DomainError::RunNotFound(run_id))?;
                if run.phase == RunPhase::Pending {
                    return Err(DomainError::RunNotRunning(run_id));
                }
                if self.artifacts.contains_key(&artifact_id) {
                    return Err(DomainError::ArtifactAlreadyExists(artifact_id));
                }
                if name.trim().is_empty()
                    || media_type.trim().is_empty()
                    || locator.trim().is_empty()
                {
                    return Err(DomainError::InvalidArtifact);
                }
                Event::ArtifactRecorded {
                    mission_id: self.id,
                    artifact: Artifact {
                        id: artifact_id,
                        run_id,
                        name,
                        media_type,
                        locator,
                        digest: digest.filter(|value| !value.trim().is_empty()),
                    },
                }
            }
            Command::StartSession {
                session_id,
                name,
                started_by,
            } => {
                if name.trim().is_empty() {
                    return Err(DomainError::EmptySessionName);
                }
                if self.sessions.contains_key(&session_id) {
                    return Err(DomainError::SessionAlreadyExists(session_id));
                }
                Event::SessionStarted {
                    mission_id: self.id,
                    session_id,
                    name,
                    started_by,
                }
            }
            Command::AssignSession { session_id, run_id } => {
                let session = self.active_session(session_id)?;
                if let Some(active_run_id) = session.run_history.iter().rev().find(|candidate| {
                    self.runs
                        .get(candidate)
                        .is_some_and(|run| !run.status.is_finished())
                }) {
                    return Err(DomainError::SessionInUse {
                        session_id,
                        run_id: *active_run_id,
                    });
                }
                let run = self.running_run(run_id)?;
                if let Some(existing_session) = run.primary_session {
                    return Err(DomainError::RunAlreadyHasSession {
                        run_id,
                        session_id: existing_session,
                    });
                }
                Event::SessionAssigned {
                    mission_id: self.id,
                    session_id,
                    run_id,
                }
            }
            Command::TakeControl { session_id, human } => {
                self.active_session(session_id)?;
                if human.kind != ActorKind::Human {
                    return Err(DomainError::ControllerMustBeHuman);
                }
                Event::SessionControlTaken {
                    mission_id: self.id,
                    session_id,
                    human,
                }
            }
            Command::ReturnControl {
                session_id,
                human_id,
                controller,
            } => {
                let session = self.active_session(session_id)?;
                if session.controller.id != human_id {
                    return Err(DomainError::NotController {
                        session_id,
                        controller: session.controller.id.clone(),
                        requester: human_id,
                    });
                }
                if !matches!(controller.kind, ActorKind::Agent { .. }) {
                    return Err(DomainError::ReturnControllerMustBeAgent);
                }
                Event::SessionControlReturned {
                    mission_id: self.id,
                    session_id,
                    controller,
                }
            }
            Command::FinishSession {
                session_id,
                exit_code,
            } => {
                self.active_session(session_id)?;
                Event::SessionFinished {
                    mission_id: self.id,
                    session_id,
                    exit_code,
                }
            }
            Command::FinishRun {
                run_id,
                outcome,
                summary,
            } => {
                let run = self
                    .runs
                    .get(&run_id)
                    .ok_or(DomainError::RunNotFound(run_id))?;
                if !matches!(run.phase, RunPhase::Running | RunPhase::Paused) {
                    return Err(DomainError::RunNotRunning(run_id));
                }
                if outcome == FinishOutcome::Succeeded && self.run_needs_attention(run_id) {
                    return Err(DomainError::RunNeedsAttention(run_id));
                }
                Event::RunFinished {
                    mission_id: self.id,
                    run_id,
                    outcome,
                    summary,
                    finished_at_unix_micros: None,
                }
            }
            Command::VerifiedDelivery { command } => Event::VerifiedDelivery {
                mission_id: self.id,
                event: self.verified_delivery.decide(command, &self.runs)?,
            },
            Command::CompleteMission => {
                if self.runs.values().any(|run| !run.status.is_finished()) {
                    return Err(DomainError::InvalidEventStream(
                        "all runs must finish before the mission completes".to_owned(),
                    ));
                }
                if self
                    .sessions
                    .values()
                    .any(|session| !session.status.is_finished())
                {
                    return Err(DomainError::MissionHasActiveSessions);
                }
                if self.runs.values().any(|run| {
                    run.outcome == Some(FinishOutcome::Succeeded)
                        && run.disposition == RunDisposition::AwaitingReview
                }) {
                    return Err(DomainError::MissionHasUnreviewedRuns);
                }
                Event::MissionCompleted {
                    mission_id: self.id,
                }
            }
            Command::AbandonMission => Event::MissionAbandoned {
                mission_id: self.id,
            },
        };

        Ok(vec![event])
    }

    pub fn apply(&mut self, event: &Event) -> Result<(), DomainError> {
        if event.mission_id() != self.id {
            return Err(DomainError::WrongMission);
        }

        match event {
            Event::MissionCreated { .. } => {
                if self.version != 0 {
                    return Err(DomainError::InvalidEventStream(
                        "mission can only be created once".to_owned(),
                    ));
                }
            }
            Event::RunPlanned {
                run_id,
                parent,
                dependencies,
                retry_of,
                actor,
                objective,
                priority,
                planned_at_unix_micros,
                ..
            } => {
                self.validate_plan(*run_id, *parent, dependencies, *retry_of, objective)?;
                self.runs.insert(
                    *run_id,
                    Run {
                        id: *run_id,
                        parent: *parent,
                        dependencies: dependencies.clone(),
                        retry_of: *retry_of,
                        priority: *priority,
                        phase: RunPhase::Pending,
                        outcome: None,
                        disposition: RunDisposition::None,
                        pause_reason: None,
                        actor: actor.clone(),
                        planned_at_unix_micros: *planned_at_unix_micros,
                        finished_at_unix_micros: None,
                        driver_snapshot: None,
                        primary_session: None,
                        objective: objective.clone(),
                        status: RunStatus::Pending,
                        summary: None,
                        settlement: None,
                    },
                );
            }
            Event::RunExecutionStarted { run_id, .. } => {
                if self.run_readiness(*run_id)? != RunReadiness::Ready {
                    return Err(DomainError::RunNotReady(*run_id));
                }
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                run.status = RunStatus::Running;
                run.phase = RunPhase::Running;
                run.pause_reason = None;
            }
            Event::RunDriverResolved {
                run_id, snapshot, ..
            } => {
                validate_driver_snapshot(snapshot)?;
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                if run.status != RunStatus::Pending {
                    return Err(DomainError::RunNotPending(*run_id));
                }
                if run.driver_snapshot.is_some() {
                    return Err(DomainError::RunDriverConflict(*run_id));
                }
                run.driver_snapshot = Some(snapshot.clone());
            }
            Event::RunPaused { run_id, reason, .. } => {
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                run.phase = RunPhase::Paused;
                run.pause_reason = Some(*reason);
                run.status = RunStatus::Paused;
            }
            Event::RunResumed { run_id, .. } => {
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                run.phase = RunPhase::Running;
                run.pause_reason = None;
                run.status = RunStatus::Running;
            }
            Event::RunCancelled {
                run_id,
                summary,
                cancelled_at_unix_micros,
                ..
            } => {
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                run.phase = RunPhase::Finished;
                run.outcome = Some(FinishOutcome::Cancelled);
                run.disposition = RunDisposition::None;
                run.pause_reason = None;
                run.status = RunStatus::Cancelled;
                run.summary = Some(summary.clone());
                run.finished_at_unix_micros = *cancelled_at_unix_micros;
                self.verified_delivery.release_run(*run_id);
            }
            Event::RunResultAccepted {
                run_id, by, note, ..
            } => {
                validate_settlement_note(note)?;
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                run.disposition = RunDisposition::Accepted;
                run.settlement = Some(RunSettlement {
                    disposition: RunDisposition::Accepted,
                    by: by.clone(),
                    note: note.clone(),
                });
            }
            Event::RunResultRejected {
                run_id, by, note, ..
            } => {
                validate_settlement_note(note)?;
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                run.disposition = RunDisposition::Rejected;
                run.settlement = Some(RunSettlement {
                    disposition: RunDisposition::Rejected,
                    by: by.clone(),
                    note: note.clone(),
                });
            }
            Event::RunStarted {
                run_id,
                parent,
                actor,
                objective,
                ..
            } => {
                self.runs.insert(
                    *run_id,
                    Run {
                        id: *run_id,
                        parent: *parent,
                        dependencies: Vec::new(),
                        retry_of: None,
                        priority: RunPriority::Normal,
                        phase: RunPhase::Running,
                        outcome: None,
                        disposition: RunDisposition::None,
                        pause_reason: None,
                        actor: actor.clone(),
                        planned_at_unix_micros: None,
                        finished_at_unix_micros: None,
                        driver_snapshot: None,
                        primary_session: None,
                        objective: objective.clone(),
                        status: RunStatus::Running,
                        summary: None,
                        settlement: None,
                    },
                );
            }
            Event::SignalRaised {
                signal_id,
                run_id,
                kind,
                ..
            } => {
                self.signals.insert(
                    *signal_id,
                    Signal {
                        id: *signal_id,
                        run_id: *run_id,
                        kind: kind.clone(),
                        resolution: None,
                    },
                );
                self.refresh_run_attention(*run_id)?;
            }
            Event::SignalResolved {
                signal_id,
                resolution,
                ..
            } => {
                let signal = self
                    .signals
                    .get_mut(signal_id)
                    .ok_or(DomainError::SignalNotFound(*signal_id))?;
                signal.resolution = Some(resolution.clone());
                let run_id = signal.run_id;
                self.refresh_run_attention(run_id)?;
            }
            Event::GrantIssued { grant, .. } => {
                if self.grants.insert(grant.id, grant.clone()).is_some() {
                    return Err(DomainError::GrantAlreadyExists(grant.id));
                }
            }
            Event::GrantRevoked { grant_id, .. } => {
                let grant = self
                    .grants
                    .get_mut(grant_id)
                    .ok_or(DomainError::GrantNotFound(*grant_id))?;
                grant.revoked = true;
            }
            Event::ArtifactRecorded { artifact, .. } => {
                if self
                    .artifacts
                    .insert(artifact.id, artifact.clone())
                    .is_some()
                {
                    return Err(DomainError::ArtifactAlreadyExists(artifact.id));
                }
            }
            Event::SessionStarted {
                session_id,
                name,
                started_by,
                ..
            } => {
                self.sessions.insert(
                    *session_id,
                    Session {
                        id: *session_id,
                        name: name.clone(),
                        started_by: started_by.clone(),
                        controller: started_by.clone(),
                        status: SessionStatus::Running,
                        run_history: Vec::new(),
                    },
                );
            }
            Event::SessionAssigned {
                session_id, run_id, ..
            } => {
                let session = self
                    .sessions
                    .get_mut(session_id)
                    .ok_or(DomainError::SessionNotFound(*session_id))?;
                if !session.run_history.contains(run_id) {
                    session.run_history.push(*run_id);
                }
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                run.primary_session = Some(*session_id);
            }
            Event::SessionControlTaken {
                session_id, human, ..
            } => {
                let session = self
                    .sessions
                    .get_mut(session_id)
                    .ok_or(DomainError::SessionNotFound(*session_id))?;
                session.controller = human.clone();
            }
            Event::SessionControlReturned {
                session_id,
                controller,
                ..
            } => {
                let session = self
                    .sessions
                    .get_mut(session_id)
                    .ok_or(DomainError::SessionNotFound(*session_id))?;
                session.controller = controller.clone();
            }
            Event::SessionFinished {
                session_id,
                exit_code,
                ..
            } => {
                let session = self
                    .sessions
                    .get_mut(session_id)
                    .ok_or(DomainError::SessionNotFound(*session_id))?;
                session.status = SessionStatus::Exited { code: *exit_code };
            }
            Event::LegacyRunControlTaken { .. } | Event::LegacyRunControlReturned { .. } => {}
            Event::RunFinished {
                run_id,
                outcome,
                summary,
                finished_at_unix_micros,
                ..
            } => {
                let run = self
                    .runs
                    .get_mut(run_id)
                    .ok_or(DomainError::RunNotFound(*run_id))?;
                run.phase = RunPhase::Finished;
                run.outcome = Some(*outcome);
                run.disposition = if *outcome == FinishOutcome::Succeeded {
                    RunDisposition::AwaitingReview
                } else {
                    RunDisposition::None
                };
                run.pause_reason = None;
                run.status = (*outcome).into();
                run.summary = Some(summary.clone());
                run.finished_at_unix_micros = *finished_at_unix_micros;
                self.verified_delivery.release_run(*run_id);
            }
            Event::VerifiedDelivery { event, .. } => {
                self.verified_delivery.apply(event, &self.runs)?;
            }
            Event::MissionCompleted { .. } => self.status = MissionStatus::Completed,
            Event::MissionAbandoned { .. } => self.status = MissionStatus::Abandoned,
        }
        self.version += 1;
        Ok(())
    }

    #[must_use]
    pub fn attention_queue(&self) -> Vec<AttentionItem> {
        let mut items: Vec<_> = self
            .signals
            .values()
            .filter(|signal| signal.kind.requires_response() && signal.resolution.is_none())
            .filter_map(|signal| {
                let run = self.runs.get(&signal.run_id)?;
                let (kind, summary) = match &signal.kind {
                    SignalKind::ApprovalNeeded { operation, risk } => (
                        if *risk == Risk::High {
                            AttentionKind::HighRiskApproval
                        } else {
                            AttentionKind::Approval
                        },
                        operation.clone(),
                    ),
                    SignalKind::Blocked { reason } => (AttentionKind::Blocked, reason.clone()),
                    SignalKind::Escalation { conflict, .. } => {
                        (AttentionKind::Escalation, conflict.clone())
                    }
                    SignalKind::InputNeeded { question } => {
                        (AttentionKind::Input, question.clone())
                    }
                    SignalKind::Progress { .. } | SignalKind::ArtifactReady { .. } => return None,
                };
                Some(AttentionItem {
                    kind,
                    signal_id: signal.id,
                    run_id: run.id,
                    actor: run.actor.clone(),
                    summary,
                })
            })
            .collect();
        items.sort_by_key(|item| item.kind);
        items
    }

    pub fn run_readiness(&self, run_id: RunId) -> Result<RunReadiness, DomainError> {
        let run = self
            .runs
            .get(&run_id)
            .ok_or(DomainError::RunNotFound(run_id))?;
        if run.status != RunStatus::Pending {
            return Ok(RunReadiness::NotPending);
        }
        let mut unfinished = false;
        for dependency_id in &run.dependencies {
            match self
                .runs
                .get(dependency_id)
                .ok_or(DomainError::RunNotFound(*dependency_id))?
                .status
            {
                RunStatus::Failed | RunStatus::Cancelled => return Ok(RunReadiness::Blocked),
                RunStatus::Succeeded => {}
                RunStatus::Pending
                | RunStatus::Running
                | RunStatus::AwaitingAttention
                | RunStatus::Paused => {
                    unfinished = true;
                }
            }
        }
        Ok(if unfinished {
            RunReadiness::Waiting
        } else {
            RunReadiness::Ready
        })
    }

    pub fn scheduler_plan(&self, max_concurrency: u16) -> Result<SchedulerPlan, DomainError> {
        self.scheduler_plan_at(max_concurrency, None)
    }

    pub fn scheduler_plan_at(
        &self,
        max_concurrency: u16,
        now_unix_micros: Option<u64>,
    ) -> Result<SchedulerPlan, DomainError> {
        let occupied_slots = self
            .runs
            .values()
            .filter(|run| {
                matches!(run.actor.kind, ActorKind::Agent { .. })
                    && matches!(
                        run.status,
                        RunStatus::Running | RunStatus::AwaitingAttention | RunStatus::Paused
                    )
            })
            .count()
            .min(usize::from(u16::MAX)) as u16;
        let available_slots = max_concurrency.saturating_sub(occupied_slots);
        let mut ready = Vec::new();
        let mut waiting_dependencies = Vec::new();
        let mut blocked_dependencies = Vec::new();
        let mut manual = Vec::new();

        for run in self
            .runs
            .values()
            .filter(|run| run.status == RunStatus::Pending)
        {
            if matches!(run.actor.kind, ActorKind::Human) {
                manual.push(run.id);
                continue;
            }
            match self.run_readiness(run.id)? {
                RunReadiness::Ready => {
                    let ready_since_unix_micros = self.run_ready_since(run)?;
                    let effective_priority =
                        effective_priority(run.priority, ready_since_unix_micros, now_unix_micros);
                    ready.push(ScheduledRun {
                        run_id: run.id,
                        actor: run.actor.clone(),
                        objective: run.objective.clone(),
                        priority: run.priority,
                        effective_priority,
                        ready_since_unix_micros,
                    });
                }
                RunReadiness::Waiting => waiting_dependencies.push(run.id),
                RunReadiness::Blocked => blocked_dependencies.push(run.id),
                RunReadiness::NotPending => unreachable!("scheduler filters pending Runs"),
            }
        }
        ready.sort_by_key(|run| {
            (
                priority_rank(run.effective_priority),
                run.ready_since_unix_micros.unwrap_or(u64::MAX),
                run.run_id,
            )
        });
        waiting_dependencies.sort_unstable();
        blocked_dependencies.sort_unstable();
        manual.sort_unstable();
        let queued = ready.split_off(ready.len().min(usize::from(available_slots)));

        Ok(SchedulerPlan {
            max_concurrency,
            occupied_slots,
            available_slots,
            startable: ready,
            ready_queued: queued,
            waiting_dependencies,
            blocked_dependencies,
            manual,
        })
    }

    fn run_ready_since(&self, run: &Run) -> Result<Option<u64>, DomainError> {
        let mut ready_since = run.planned_at_unix_micros;
        for dependency_id in &run.dependencies {
            let dependency = self
                .runs
                .get(dependency_id)
                .ok_or(DomainError::RunNotFound(*dependency_id))?;
            if dependency.status != RunStatus::Succeeded {
                return Ok(None);
            }
            let Some(finished_at) = dependency.finished_at_unix_micros else {
                return Ok(None);
            };
            ready_since = Some(ready_since.map_or(finished_at, |current| current.max(finished_at)));
        }
        Ok(ready_since)
    }

    fn validate_new_run(
        &self,
        run_id: RunId,
        parent: Option<RunId>,
        objective: &str,
    ) -> Result<(), DomainError> {
        if objective.trim().is_empty() {
            return Err(DomainError::EmptyRunObjective);
        }
        if self.runs.contains_key(&run_id) {
            return Err(DomainError::RunAlreadyExists(run_id));
        }
        if let Some(parent_id) = parent {
            if parent_id == run_id {
                return Err(DomainError::SelfDependency(run_id));
            }
            self.runs
                .get(&parent_id)
                .ok_or(DomainError::RunNotFound(parent_id))?;
        }
        Ok(())
    }

    fn validate_plan(
        &self,
        run_id: RunId,
        parent: Option<RunId>,
        dependencies: &[RunId],
        retry_of: Option<RunId>,
        objective: &str,
    ) -> Result<(), DomainError> {
        self.validate_new_run(run_id, parent, objective)?;
        let mut seen = std::collections::HashSet::new();
        for dependency_id in dependencies {
            if *dependency_id == run_id {
                return Err(DomainError::SelfDependency(run_id));
            }
            if !seen.insert(*dependency_id) {
                return Err(DomainError::DuplicateDependency {
                    run_id,
                    dependency_id: *dependency_id,
                });
            }
            self.runs
                .get(dependency_id)
                .ok_or(DomainError::RunNotFound(*dependency_id))?;
        }
        if let Some(retry_id) = retry_of {
            if retry_id == run_id {
                return Err(DomainError::SelfDependency(run_id));
            }
            let retry = self
                .runs
                .get(&retry_id)
                .ok_or(DomainError::RunNotFound(retry_id))?;
            if !retry.status.is_finished() {
                return Err(DomainError::RetryTargetNotFinished(retry_id));
            }
        }
        Ok(())
    }

    fn running_run(&self, run_id: RunId) -> Result<&Run, DomainError> {
        let run = self
            .runs
            .get(&run_id)
            .ok_or(DomainError::RunNotFound(run_id))?;
        if !matches!(
            run.status,
            RunStatus::Running | RunStatus::AwaitingAttention
        ) {
            return Err(if run.status.is_finished() {
                DomainError::RunFinished(run_id)
            } else {
                DomainError::RunNotRunning(run_id)
            });
        }
        Ok(run)
    }

    fn reviewable_run(&self, run_id: RunId) -> Result<&Run, DomainError> {
        let run = self
            .runs
            .get(&run_id)
            .ok_or(DomainError::RunNotFound(run_id))?;
        if run.phase != RunPhase::Finished
            || run.outcome != Some(FinishOutcome::Succeeded)
            || run.disposition != RunDisposition::AwaitingReview
        {
            return Err(DomainError::RunNotAwaitingReview(run_id));
        }
        Ok(run)
    }

    fn active_session(&self, session_id: SessionId) -> Result<&Session, DomainError> {
        let session = self
            .sessions
            .get(&session_id)
            .ok_or(DomainError::SessionNotFound(session_id))?;
        if session.status.is_finished() {
            return Err(DomainError::SessionFinished(session_id));
        }
        Ok(session)
    }

    fn run_needs_attention(&self, run_id: RunId) -> bool {
        self.signals.values().any(|signal| {
            signal.run_id == run_id
                && signal.kind.requires_response()
                && signal.resolution.is_none()
        })
    }

    fn refresh_run_attention(&mut self, run_id: RunId) -> Result<(), DomainError> {
        let needs_attention = self.run_needs_attention(run_id);
        let run = self
            .runs
            .get_mut(&run_id)
            .ok_or(DomainError::RunNotFound(run_id))?;
        if matches!(
            run.status,
            RunStatus::Running | RunStatus::AwaitingAttention
        ) {
            run.status = if needs_attention {
                RunStatus::AwaitingAttention
            } else {
                RunStatus::Running
            };
        }
        Ok(())
    }
}

fn validate_settlement_note(note: &str) -> Result<(), DomainError> {
    if note.trim().is_empty() || note.len() > 4_096 || note.contains('\0') {
        return Err(DomainError::InvalidSettlement);
    }
    Ok(())
}

const fn priority_rank(priority: RunPriority) -> u8 {
    match priority {
        RunPriority::Urgent => 0,
        RunPriority::Normal => 1,
        RunPriority::Background => 2,
    }
}

const PRIORITY_AGING_INTERVAL_MICROS: u64 = 5 * 60 * 1_000_000;

fn effective_priority(
    priority: RunPriority,
    ready_since_unix_micros: Option<u64>,
    now_unix_micros: Option<u64>,
) -> RunPriority {
    let promotions = match (ready_since_unix_micros, now_unix_micros) {
        (Some(ready_since), Some(now)) => now
            .saturating_sub(ready_since)
            .checked_div(PRIORITY_AGING_INTERVAL_MICROS)
            .unwrap_or(0),
        _ => 0,
    };
    match (priority, promotions) {
        (RunPriority::Urgent, _) | (RunPriority::Normal, 1..) => RunPriority::Urgent,
        (RunPriority::Background, 2..) => RunPriority::Urgent,
        (RunPriority::Background, 1) => RunPriority::Normal,
        (priority, _) => priority,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mission() -> Mission {
        Mission::create(
            MissionId::new(),
            "Make terminals agent-native",
            Actor::human("alice").expect("test actor should be valid"),
        )
        .expect("test mission should be valid")
        .0
    }

    fn start_agent(mission: &mut Mission) -> RunId {
        let run_id = RunId::new();
        let events = mission
            .decide(Command::StartRun {
                run_id,
                parent: None,
                actor: Actor::agent("planner", "codex").expect("test actor should be valid"),
                objective: "Design the runtime".to_owned(),
            })
            .expect("run should start");
        mission.apply(&events[0]).expect("event should apply");
        run_id
    }

    fn plan_agent(
        mission: &mut Mission,
        parent: Option<RunId>,
        dependencies: Vec<RunId>,
        objective: &str,
    ) -> RunId {
        let run_id = RunId::new();
        let events = mission
            .decide(Command::PlanRun {
                run_id,
                parent,
                dependencies,
                retry_of: None,
                actor: Actor::agent("planner", "codex").expect("test actor should be valid"),
                objective: objective.to_owned(),
                priority: RunPriority::Normal,
            })
            .expect("run should be planned");
        mission.apply(&events[0]).expect("event should apply");
        run_id
    }

    fn plan_agent_at(
        mission: &mut Mission,
        dependencies: Vec<RunId>,
        objective: &str,
        priority: RunPriority,
        planned_at_unix_micros: u64,
    ) -> RunId {
        let run_id = RunId::new();
        let mut event = mission
            .decide(Command::PlanRun {
                run_id,
                parent: None,
                dependencies,
                retry_of: None,
                actor: Actor::agent(format!("aged-{run_id}"), "codex")
                    .expect("test actor should be valid"),
                objective: objective.to_owned(),
                priority,
            })
            .expect("run should be planned")
            .remove(0);
        let Event::RunPlanned {
            planned_at_unix_micros: timestamp,
            ..
        } = &mut event
        else {
            unreachable!("PlanRun has one RunPlanned event")
        };
        *timestamp = Some(planned_at_unix_micros);
        mission.apply(&event).expect("event should apply");
        run_id
    }

    fn start_ready(mission: &mut Mission, run_id: RunId) {
        let events = mission
            .decide(Command::StartReadyRun { run_id })
            .expect("run should be ready");
        mission.apply(&events[0]).expect("event should apply");
    }

    fn driver_snapshot() -> RunDriverSnapshot {
        RunDriverSnapshot {
            driver_id: "codex".to_owned(),
            profile_version: 1,
            process_spec_sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_owned(),
            argument_count: 3,
            environment_keys: vec!["AGENT_MODE".to_owned(), "TERM".to_owned()],
            sandbox_backend: None,
            sandbox_profile: None,
            sandbox_network_isolated: false,
        }
    }

    #[test]
    fn resolved_driver_snapshot_is_immutable_and_rehydrates() {
        let mut mission = mission();
        let run_id = plan_agent(&mut mission, None, Vec::new(), "Run configured agent");
        let resolved = mission
            .decide(Command::ResolveRunDriver {
                run_id,
                snapshot: driver_snapshot(),
            })
            .expect("valid snapshot should resolve");
        mission
            .apply(&resolved[0])
            .expect("resolved snapshot should apply");

        assert_eq!(
            mission.runs[&run_id].driver_snapshot,
            Some(driver_snapshot())
        );
        assert_eq!(
            mission.decide(Command::ResolveRunDriver {
                run_id,
                snapshot: driver_snapshot(),
            }),
            Err(DomainError::RunDriverConflict(run_id))
        );

        let events = vec![
            Event::MissionCreated {
                mission_id: mission.id,
                intent: mission.intent.clone(),
                created_by: mission.created_by.clone(),
            },
            Event::RunPlanned {
                mission_id: mission.id,
                run_id,
                parent: None,
                dependencies: Vec::new(),
                retry_of: None,
                actor: mission.runs[&run_id].actor.clone(),
                objective: mission.runs[&run_id].objective.clone(),
                priority: RunPriority::Normal,
                planned_at_unix_micros: None,
            },
            resolved[0].clone(),
        ];
        let rehydrated = Mission::rehydrate(&events).expect("snapshot should survive replay");
        assert_eq!(
            rehydrated.runs[&run_id].driver_snapshot,
            Some(driver_snapshot())
        );
    }

    #[test]
    fn invalid_driver_snapshots_are_rejected_before_mutation() {
        let mut mission = mission();
        let run_id = plan_agent(&mut mission, None, Vec::new(), "Run configured agent");
        let mut invalid_digest = driver_snapshot();
        invalid_digest.process_spec_sha256 = "ABC".to_owned();
        assert_eq!(
            mission.decide(Command::ResolveRunDriver {
                run_id,
                snapshot: invalid_digest,
            }),
            Err(DomainError::InvalidRunDriverSnapshot)
        );

        let mut unsorted_keys = driver_snapshot();
        unsorted_keys.environment_keys.reverse();
        assert_eq!(
            mission.decide(Command::ResolveRunDriver {
                run_id,
                snapshot: unsorted_keys,
            }),
            Err(DomainError::InvalidRunDriverSnapshot)
        );
        let mut incomplete_sandbox = driver_snapshot();
        incomplete_sandbox.sandbox_backend = Some("macos_sandbox_exec".to_owned());
        assert_eq!(
            mission.decide(Command::ResolveRunDriver {
                run_id,
                snapshot: incomplete_sandbox,
            }),
            Err(DomainError::InvalidRunDriverSnapshot)
        );
        let mut impossible_network_isolation = driver_snapshot();
        impossible_network_isolation.sandbox_network_isolated = true;
        assert_eq!(
            mission.decide(Command::ResolveRunDriver {
                run_id,
                snapshot: impossible_network_isolation,
            }),
            Err(DomainError::InvalidRunDriverSnapshot)
        );
        assert!(mission.runs[&run_id].driver_snapshot.is_none());
    }

    fn finish_run(mission: &mut Mission, run_id: RunId, outcome: FinishOutcome) {
        let events = mission
            .decide(Command::FinishRun {
                run_id,
                outcome,
                summary: "done".to_owned(),
            })
            .expect("run should finish");
        mission.apply(&events[0]).expect("event should apply");
    }

    fn finish_run_at(
        mission: &mut Mission,
        run_id: RunId,
        outcome: FinishOutcome,
        finished_at_unix_micros: u64,
    ) {
        let mut event = mission
            .decide(Command::FinishRun {
                run_id,
                outcome,
                summary: "done".to_owned(),
            })
            .expect("run should finish")
            .remove(0);
        let Event::RunFinished {
            finished_at_unix_micros: timestamp,
            ..
        } = &mut event
        else {
            unreachable!("FinishRun has one RunFinished event")
        };
        *timestamp = Some(finished_at_unix_micros);
        mission.apply(&event).expect("event should apply");
    }

    fn start_session(mission: &mut Mission, started_by: Actor) -> SessionId {
        let session_id = SessionId::new();
        let events = mission
            .decide(Command::StartSession {
                session_id,
                name: "runtime".to_owned(),
                started_by,
            })
            .expect("session should start");
        mission.apply(&events[0]).expect("event should apply");
        session_id
    }

    #[test]
    fn unresolved_questions_enter_the_attention_queue() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        let signal_id = SignalId::new();
        let raised = mission
            .decide(Command::RaiseSignal {
                signal_id,
                run_id,
                kind: SignalKind::InputNeeded {
                    question: "Native or web UI?".to_owned(),
                },
            })
            .expect("signal should be accepted");
        mission.apply(&raised[0]).expect("event should apply");

        assert_eq!(mission.runs[&run_id].status, RunStatus::AwaitingAttention);
        assert_eq!(mission.attention_queue().len(), 1);

        let resolved = mission
            .decide(Command::ResolveSignal {
                signal_id,
                by: ActorId::new("alice").expect("test id should be valid"),
                response: "Native".to_owned(),
            })
            .expect("signal should resolve");
        mission.apply(&resolved[0]).expect("event should apply");

        assert_eq!(mission.runs[&run_id].status, RunStatus::Running);
        assert!(mission.attention_queue().is_empty());
    }

    #[test]
    fn phase_attention_outcome_and_disposition_remain_orthogonal() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        let signal_id = SignalId::new();
        let raised = mission
            .decide(Command::RaiseSignal {
                signal_id,
                run_id,
                kind: SignalKind::InputNeeded {
                    question: "Choose a release channel".to_owned(),
                },
            })
            .expect("signal should be accepted");
        mission.apply(&raised[0]).expect("event should apply");
        assert_eq!(mission.runs[&run_id].phase, RunPhase::Running);
        assert_eq!(mission.runs[&run_id].status, RunStatus::AwaitingAttention);

        let paused = mission
            .decide(Command::PauseRun {
                run_id,
                reason: PauseReason::Attention,
            })
            .expect("active run should pause");
        mission.apply(&paused[0]).expect("event should apply");
        assert_eq!(mission.runs[&run_id].phase, RunPhase::Paused);
        assert_eq!(
            mission.runs[&run_id].pause_reason,
            Some(PauseReason::Attention)
        );

        let resolved = mission
            .decide(Command::ResolveSignal {
                signal_id,
                by: ActorId::new("alice").expect("test id should be valid"),
                response: "alpha".to_owned(),
            })
            .expect("signal should resolve while paused");
        mission.apply(&resolved[0]).expect("event should apply");
        let resumed = mission
            .decide(Command::ResumeRun { run_id })
            .expect("resolved run should resume");
        mission.apply(&resumed[0]).expect("event should apply");

        finish_run(&mut mission, run_id, FinishOutcome::Succeeded);
        assert_eq!(mission.runs[&run_id].phase, RunPhase::Finished);
        assert_eq!(
            mission.runs[&run_id].outcome,
            Some(FinishOutcome::Succeeded)
        );
        assert_eq!(
            mission.runs[&run_id].disposition,
            RunDisposition::AwaitingReview
        );
        let rejected = mission
            .decide(Command::RejectRunResult {
                run_id,
                by: ActorId::new("alice").expect("test id should be valid"),
                note: "Evidence is incomplete".to_owned(),
            })
            .expect("successful result should be rejectable");
        mission.apply(&rejected[0]).expect("event should apply");
        assert_eq!(
            mission.runs[&run_id].outcome,
            Some(FinishOutcome::Succeeded)
        );
        assert_eq!(mission.runs[&run_id].disposition, RunDisposition::Rejected);
    }

    #[test]
    fn dependency_fan_in_is_ready_only_after_every_prerequisite_succeeds() {
        let mut mission = mission();
        let first = plan_agent(&mut mission, None, Vec::new(), "Build macOS package");
        let second = plan_agent(&mut mission, None, Vec::new(), "Build Linux package");
        start_ready(&mut mission, first);
        start_ready(&mut mission, second);
        let release = plan_agent(
            &mut mission,
            None,
            vec![first, second],
            "Publish both packages",
        );

        assert_eq!(mission.run_readiness(release), Ok(RunReadiness::Waiting));
        finish_run(&mut mission, first, FinishOutcome::Succeeded);
        assert_eq!(mission.run_readiness(release), Ok(RunReadiness::Waiting));
        finish_run(&mut mission, second, FinishOutcome::Succeeded);
        assert_eq!(mission.run_readiness(release), Ok(RunReadiness::Ready));

        start_ready(&mut mission, release);
        assert_eq!(mission.runs[&release].status, RunStatus::Running);
    }

    #[test]
    fn scheduler_plan_is_capacity_aware_priority_ordered_and_deterministic() {
        let mut mission = mission();
        let occupied = start_agent(&mut mission);
        let normal = plan_agent(&mut mission, None, Vec::new(), "normal");
        let background = RunId::new();
        let urgent = RunId::new();
        let waiting = plan_agent(&mut mission, None, vec![occupied], "waiting");
        let failed = plan_agent(&mut mission, None, Vec::new(), "failed prerequisite");
        start_ready(&mut mission, failed);
        finish_run(&mut mission, failed, FinishOutcome::Failed);
        let blocked = plan_agent(&mut mission, None, vec![failed], "blocked");
        let manual = RunId::new();

        for (run_id, actor, objective, priority) in [
            (
                background,
                Actor::agent("background", "codex").expect("agent should be valid"),
                "background",
                RunPriority::Background,
            ),
            (
                urgent,
                Actor::agent("urgent", "codex").expect("agent should be valid"),
                "urgent",
                RunPriority::Urgent,
            ),
            (
                manual,
                Actor::human("operator").expect("human should be valid"),
                "manual",
                RunPriority::Urgent,
            ),
        ] {
            let events = mission
                .decide(Command::PlanRun {
                    run_id,
                    parent: None,
                    dependencies: Vec::new(),
                    retry_of: None,
                    actor,
                    objective: objective.to_owned(),
                    priority,
                })
                .expect("run should be planned");
            mission.apply(&events[0]).expect("event should apply");
        }

        let plan = mission.scheduler_plan(2).expect("plan should be valid");
        assert_eq!(plan.occupied_slots, 1);
        assert_eq!(plan.available_slots, 1);
        assert_eq!(
            plan.startable
                .iter()
                .map(|run| run.run_id)
                .collect::<Vec<_>>(),
            vec![urgent]
        );
        assert_eq!(
            plan.ready_queued
                .iter()
                .map(|run| run.run_id)
                .collect::<Vec<_>>(),
            vec![normal, background]
        );
        assert_eq!(plan.waiting_dependencies, vec![waiting]);
        assert_eq!(plan.blocked_dependencies, vec![blocked]);
        assert_eq!(plan.manual, vec![manual]);
        assert_eq!(plan, mission.scheduler_plan(2).expect("repeat is stable"));
    }

    #[test]
    fn scheduler_ages_ready_runs_at_exact_five_minute_boundaries() {
        let mut mission = mission();
        let interval = PRIORITY_AGING_INTERVAL_MICROS;
        let now = interval * 2;
        let twice_aged_background = plan_agent_at(
            &mut mission,
            Vec::new(),
            "old background",
            RunPriority::Background,
            0,
        );
        let once_aged_normal = plan_agent_at(
            &mut mission,
            Vec::new(),
            "old normal",
            RunPriority::Normal,
            interval,
        );
        let fresh_urgent = plan_agent_at(
            &mut mission,
            Vec::new(),
            "fresh urgent",
            RunPriority::Urgent,
            now,
        );
        let not_yet_aged_background = plan_agent_at(
            &mut mission,
            Vec::new(),
            "almost old background",
            RunPriority::Background,
            now - interval + 1,
        );

        let plan = mission
            .scheduler_plan_at(8, Some(now))
            .expect("aged plan should be valid");
        assert_eq!(
            plan.startable
                .iter()
                .map(|run| (run.run_id, run.effective_priority))
                .collect::<Vec<_>>(),
            vec![
                (twice_aged_background, RunPriority::Urgent),
                (once_aged_normal, RunPriority::Urgent),
                (fresh_urgent, RunPriority::Urgent),
                (not_yet_aged_background, RunPriority::Background),
            ]
        );
        assert_eq!(
            mission
                .scheduler_plan_at(8, Some(now))
                .expect("repeat should remain stable"),
            plan
        );
    }

    #[test]
    fn dependent_run_ages_from_dependency_completion_not_plan_time() {
        let mut mission = mission();
        let interval = PRIORITY_AGING_INTERVAL_MICROS;
        let prerequisite = plan_agent_at(
            &mut mission,
            Vec::new(),
            "prerequisite",
            RunPriority::Normal,
            0,
        );
        let dependent = plan_agent_at(
            &mut mission,
            vec![prerequisite],
            "dependent",
            RunPriority::Background,
            0,
        );
        start_ready(&mut mission, prerequisite);
        let ready_at = interval + interval / 2;
        finish_run_at(
            &mut mission,
            prerequisite,
            FinishOutcome::Succeeded,
            ready_at,
        );

        let plan = mission
            .scheduler_plan_at(4, Some(ready_at + interval))
            .expect("dependent should be schedulable");
        let scheduled = plan
            .startable
            .iter()
            .find(|run| run.run_id == dependent)
            .expect("dependent should be selected");
        assert_eq!(scheduled.ready_since_unix_micros, Some(ready_at));
        assert_eq!(scheduled.effective_priority, RunPriority::Normal);
    }

    #[test]
    fn failed_dependencies_block_without_rewriting_lineage_or_retries() {
        let mut mission = mission();
        let parent = plan_agent(&mut mission, None, Vec::new(), "Delegate work");
        start_ready(&mut mission, parent);
        let child = plan_agent(&mut mission, Some(parent), Vec::new(), "Independent child");
        assert_eq!(mission.run_readiness(child), Ok(RunReadiness::Ready));

        let prerequisite = plan_agent(&mut mission, None, Vec::new(), "Risky attempt");
        start_ready(&mut mission, prerequisite);
        let dependent = plan_agent(
            &mut mission,
            Some(parent),
            vec![prerequisite],
            "Consume the attempt",
        );
        finish_run(&mut mission, prerequisite, FinishOutcome::Failed);
        assert_eq!(mission.run_readiness(dependent), Ok(RunReadiness::Blocked));
        assert_eq!(
            mission.decide(Command::StartReadyRun { run_id: dependent }),
            Err(DomainError::RunNotReady(dependent))
        );

        let retry = RunId::new();
        let planned_retry = mission
            .decide(Command::PlanRun {
                run_id: retry,
                parent: Some(parent),
                dependencies: Vec::new(),
                retry_of: Some(prerequisite),
                actor: Actor::agent("recovery", "codex").expect("test actor should be valid"),
                objective: "Retry risky attempt".to_owned(),
                priority: RunPriority::Urgent,
            })
            .expect("finished attempt should be retryable");
        mission
            .apply(&planned_retry[0])
            .expect("retry event should apply");
        assert_eq!(mission.runs[&dependent].dependencies, vec![prerequisite]);
        assert_eq!(mission.runs[&retry].retry_of, Some(prerequisite));
        assert_eq!(mission.run_readiness(retry), Ok(RunReadiness::Ready));
    }

    #[test]
    fn duplicate_dependency_edges_are_rejected_before_commit() {
        let mut mission = mission();
        let prerequisite = plan_agent(&mut mission, None, Vec::new(), "One prerequisite");
        let run_id = RunId::new();
        assert_eq!(
            mission.decide(Command::PlanRun {
                run_id,
                parent: None,
                dependencies: vec![prerequisite, prerequisite],
                retry_of: None,
                actor: Actor::agent("planner", "codex").expect("test actor should be valid"),
                objective: "Invalid fan-in".to_owned(),
                priority: RunPriority::Normal,
            }),
            Err(DomainError::DuplicateDependency {
                run_id,
                dependency_id: prerequisite,
            })
        );
    }

    #[test]
    fn attention_is_prioritized_by_required_judgment() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        for kind in [
            SignalKind::InputNeeded {
                question: "Which name?".to_owned(),
            },
            SignalKind::Blocked {
                reason: "Missing entitlement".to_owned(),
            },
            SignalKind::ApprovalNeeded {
                operation: "Delete remote branch".to_owned(),
                risk: Risk::High,
            },
        ] {
            let event = mission
                .decide(Command::RaiseSignal {
                    signal_id: SignalId::new(),
                    run_id,
                    kind,
                })
                .expect("signal should be accepted");
            mission.apply(&event[0]).expect("event should apply");
        }

        let kinds: Vec<_> = mission
            .attention_queue()
            .into_iter()
            .map(|item| item.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![
                AttentionKind::HighRiskApproval,
                AttentionKind::Blocked,
                AttentionKind::Input,
            ]
        );
    }

    #[test]
    fn approval_decisions_issue_explicit_cooperative_grants() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        let signal_id = SignalId::new();
        let raised = mission
            .decide(Command::RaiseSignal {
                signal_id,
                run_id,
                kind: SignalKind::ApprovalNeeded {
                    operation: "publish release".to_owned(),
                    risk: Risk::High,
                },
            })
            .expect("approval should be raised");
        mission.apply(&raised[0]).expect("event should apply");
        let by = ActorId::new("alice").expect("test id should be valid");
        assert_eq!(
            mission.decide(Command::ResolveSignal {
                signal_id,
                by: by.clone(),
                response: "yes".to_owned(),
            }),
            Err(DomainError::ApprovalRequiresDecision(signal_id))
        );
        assert_eq!(
            mission.decide(Command::DecideApproval {
                signal_id,
                grant_id: Some(GrantId::new()),
                by: by.clone(),
                allow: true,
                resource_scope: "registry/superplexr".to_owned(),
                use_count: 1,
                enforcement: GrantEnforcement::Enforced,
            }),
            Err(DomainError::EnforcedGrantUnavailable)
        );

        let grant_id = GrantId::new();
        let decided = mission
            .decide(Command::DecideApproval {
                signal_id,
                grant_id: Some(grant_id),
                by: by.clone(),
                allow: true,
                resource_scope: "registry/superplexr".to_owned(),
                use_count: 1,
                enforcement: GrantEnforcement::Cooperative,
            })
            .expect("cooperative grant should be issued");
        assert_eq!(decided.len(), 2);
        for event in &decided {
            mission.apply(event).expect("event should apply");
        }
        assert_eq!(
            mission.signals[&signal_id]
                .resolution
                .as_ref()
                .and_then(|resolution| resolution.grant_id),
            Some(grant_id)
        );
        assert_eq!(
            mission.grants[&grant_id].enforcement,
            GrantEnforcement::Cooperative
        );
        assert!(!mission.grants[&grant_id].revoked);

        let revoked = mission
            .decide(Command::RevokeGrant {
                grant_id,
                by,
                reason: "release cancelled".to_owned(),
            })
            .expect("grant should revoke");
        mission.apply(&revoked[0]).expect("event should apply");
        assert!(mission.grants[&grant_id].revoked);
    }

    #[test]
    fn artifacts_are_durable_graph_objects_with_integrity_metadata() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        let artifact_id = ArtifactId::new();
        let recorded = mission
            .decide(Command::RecordArtifact {
                artifact_id,
                run_id,
                name: "release manifest".to_owned(),
                media_type: "application/json".to_owned(),
                locator: "artifacts/release.json".to_owned(),
                digest: Some("sha256:abc123".to_owned()),
            })
            .expect("artifact should record");
        mission.apply(&recorded[0]).expect("event should apply");

        let artifact = &mission.artifacts[&artifact_id];
        assert_eq!(artifact.run_id, run_id);
        assert_eq!(artifact.media_type, "application/json");
        assert_eq!(artifact.digest.as_deref(), Some("sha256:abc123"));
        assert_eq!(
            mission.decide(Command::RecordArtifact {
                artifact_id,
                run_id,
                name: "duplicate".to_owned(),
                media_type: "text/plain".to_owned(),
                locator: "duplicate.txt".to_owned(),
                digest: None,
            }),
            Err(DomainError::ArtifactAlreadyExists(artifact_id))
        );
    }

    #[test]
    fn a_human_can_take_and_return_session_control() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        let agent = mission.runs[&run_id].actor.clone();
        let session_id = start_session(&mut mission, agent.clone());
        let assigned = mission
            .decide(Command::AssignSession { session_id, run_id })
            .expect("session should be assigned");
        mission.apply(&assigned[0]).expect("event should apply");
        let human = Actor::human("alice").expect("test actor should be valid");
        let taken = mission
            .decide(Command::TakeControl {
                session_id,
                human: human.clone(),
            })
            .expect("human should take control");
        mission.apply(&taken[0]).expect("event should apply");
        assert_eq!(mission.sessions[&session_id].controller, human);
        assert_eq!(mission.runs[&run_id].primary_session, Some(session_id));

        let returned = mission
            .decide(Command::ReturnControl {
                session_id,
                human_id: ActorId::new("alice").expect("test id should be valid"),
                controller: agent.clone(),
            })
            .expect("controller should return control");
        mission.apply(&returned[0]).expect("event should apply");
        assert_eq!(mission.sessions[&session_id].controller, agent);
    }

    #[test]
    fn finishing_a_run_does_not_finish_its_session() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        let agent = mission.runs[&run_id].actor.clone();
        let session_id = start_session(&mut mission, agent);
        for command in [
            Command::AssignSession { session_id, run_id },
            Command::FinishRun {
                run_id,
                outcome: FinishOutcome::Succeeded,
                summary: "Agent work is ready".to_owned(),
            },
        ] {
            let events = mission.decide(command).expect("command should be accepted");
            mission.apply(&events[0]).expect("event should apply");
        }

        assert_eq!(mission.runs[&run_id].status, RunStatus::Succeeded);
        assert_eq!(mission.sessions[&session_id].status, SessionStatus::Running);
        assert_eq!(
            mission.decide(Command::CompleteMission),
            Err(DomainError::MissionHasActiveSessions)
        );

        let finished = mission
            .decide(Command::FinishSession {
                session_id,
                exit_code: Some(0),
            })
            .expect("session should finish independently");
        mission.apply(&finished[0]).expect("event should apply");
        assert_eq!(
            mission.decide(Command::CompleteMission),
            Err(DomainError::MissionHasUnreviewedRuns)
        );
        let accepted = mission
            .decide(Command::AcceptRunResult {
                run_id,
                by: ActorId::new("alice").expect("test id should be valid"),
                note: "Reviewed terminal evidence".to_owned(),
            })
            .expect("successful result should be reviewable");
        mission
            .apply(&accepted[0])
            .expect("review event should apply");
        assert!(
            mission
                .decide(Command::CompleteMission)
                .expect("mission should complete")
                .iter()
                .any(|event| matches!(event, Event::MissionCompleted { .. }))
        );
    }

    #[test]
    fn a_session_can_be_reused_only_after_its_current_run_finishes() {
        let mut mission = mission();
        let first_run = start_agent(&mut mission);
        let first_agent = mission.runs[&first_run].actor.clone();
        let session_id = start_session(&mut mission, first_agent);
        let assigned = mission
            .decide(Command::AssignSession {
                session_id,
                run_id: first_run,
            })
            .expect("first run should use the session");
        mission.apply(&assigned[0]).expect("event should apply");

        let second_run = start_agent(&mut mission);
        assert_eq!(
            mission.decide(Command::AssignSession {
                session_id,
                run_id: second_run,
            }),
            Err(DomainError::SessionInUse {
                session_id,
                run_id: first_run,
            })
        );

        let finished = mission
            .decide(Command::FinishRun {
                run_id: first_run,
                outcome: FinishOutcome::Succeeded,
                summary: "First attempt complete".to_owned(),
            })
            .expect("first run should finish");
        mission.apply(&finished[0]).expect("event should apply");

        let reassigned = mission
            .decide(Command::AssignSession {
                session_id,
                run_id: second_run,
            })
            .expect("finished run should release sequential use");
        mission.apply(&reassigned[0]).expect("event should apply");
        assert_eq!(
            mission.sessions[&session_id].run_history,
            vec![first_run, second_run]
        );
    }

    #[test]
    fn legacy_run_control_events_keep_their_original_wire_tags() {
        let event = Event::LegacyRunControlTaken {
            mission_id: MissionId::new(),
            run_id: RunId::new(),
            human: Actor::human("alice").expect("test actor should be valid"),
        };
        let encoded = serde_json::to_value(&event).expect("event should encode");
        assert_eq!(encoded["type"], "control_taken");
        assert_eq!(
            serde_json::from_value::<Event>(encoded).expect("legacy event should decode"),
            event
        );
    }

    fn dispatch_delivery(mission: &mut Mission, command: VerifiedDeliveryCommand) {
        let events = mission
            .decide(Command::VerifiedDelivery { command })
            .expect("verified-delivery command should succeed");
        mission
            .apply(&events[0])
            .expect("verified-delivery event should apply");
    }

    fn digest(byte: char) -> String {
        std::iter::repeat_n(byte, 64).collect()
    }

    #[test]
    fn an_admitted_change_intent_gates_start_and_releases_on_finish() {
        let mut mission = mission();
        let run_id = plan_agent(&mut mission, None, Vec::new(), "Modify the scheduler");
        dispatch_delivery(
            &mut mission,
            VerifiedDeliveryCommand::DeclareChangeIntent {
                run_id,
                expected_version: None,
                spec: crate::ChangeIntentSpec {
                    repository_identity: "repository-1".to_owned(),
                    base_revision: "abc123".to_owned(),
                    claims: vec![crate::ChangeClaim {
                        path: "crates/core/src/lib.rs".to_owned(),
                        operation: crate::ChangeOperation::Modify,
                        scope: crate::ChangeScope::Committed,
                    }],
                },
            },
        );
        assert_eq!(
            mission.decide(Command::StartReadyRun { run_id }),
            Err(DomainError::ChangeIntentNotAdmitted(run_id))
        );
        dispatch_delivery(
            &mut mission,
            VerifiedDeliveryCommand::AdmitChangeIntent {
                run_id,
                expected_version: 1,
                lease_epoch: 1,
            },
        );

        start_ready(&mut mission, run_id);
        finish_run(&mut mission, run_id, FinishOutcome::Succeeded);
        assert_eq!(
            mission.verified_delivery.change_intents[&run_id].state,
            crate::ChangeIntentState::Released
        );
    }

    #[test]
    fn independent_verification_checks_the_exact_candidate_before_settlement() {
        let mut mission = mission();
        let producer = plan_agent(&mut mission, None, Vec::new(), "Produce candidate");
        dispatch_delivery(
            &mut mission,
            VerifiedDeliveryCommand::SetVerificationPolicy {
                run_id: producer,
                policy: crate::VerificationPolicy::Independent,
            },
        );
        start_ready(&mut mission, producer);
        let candidate = crate::RunCandidate {
            revision: "candidate-commit".to_owned(),
            content_sha256: digest('a'),
            execution_lease_epoch: None,
            realized_changes: None,
            artifact_ids: Vec::new(),
            submitted_by: mission.runs[&producer].actor.id.clone(),
        };
        dispatch_delivery(
            &mut mission,
            VerifiedDeliveryCommand::SubmitCandidate {
                run_id: producer,
                candidate: candidate.clone(),
            },
        );
        finish_run(&mut mission, producer, FinishOutcome::Succeeded);
        let owner = mission.created_by.id.clone();
        assert_eq!(
            mission.decide(Command::AcceptRunResult {
                run_id: producer,
                by: owner.clone(),
                note: "looks good".to_owned(),
            }),
            Err(DomainError::PassingEvaluationRequired(producer))
        );

        let verifier = plan_agent(&mut mission, None, vec![producer], "Verify exact candidate");
        start_ready(&mut mission, verifier);
        finish_run(&mut mission, verifier, FinishOutcome::Succeeded);
        dispatch_delivery(
            &mut mission,
            VerifiedDeliveryCommand::RecordEvaluationReceipt {
                receipt: crate::EvaluationReceipt {
                    artifact_id: ArtifactId::new(),
                    subject_run_id: producer,
                    verifier_run_id: verifier,
                    candidate_sha256: candidate.content_sha256,
                    verdict: crate::EvaluationVerdict::Passed,
                    checks: vec![crate::EvaluationCheck {
                        name: "workspace tests".to_owned(),
                        passed: true,
                        evidence: Vec::new(),
                    }],
                    delivery_validated: true,
                    repeatable: true,
                    summary: "candidate and final delivery verified".to_owned(),
                },
            },
        );
        let accepted = mission
            .decide(Command::AcceptRunResult {
                run_id: producer,
                by: owner,
                note: "verified".to_owned(),
            })
            .expect("passing receipt should allow owner settlement");
        mission
            .apply(&accepted[0])
            .expect("settlement should apply");
        assert_eq!(
            mission.runs[&producer].disposition,
            RunDisposition::Accepted
        );
    }

    #[test]
    fn verifier_and_retry_runs_receive_exact_frozen_delivery_inputs() {
        let mut mission = mission();
        let producer = plan_agent(&mut mission, None, Vec::new(), "Produce reviewed change");
        let driver = driver_snapshot();
        for command in [
            Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::DeclareChangeIntent {
                    run_id: producer,
                    expected_version: None,
                    spec: crate::ChangeIntentSpec {
                        repository_identity: "fixture-repository".to_owned(),
                        base_revision: "base-commit".to_owned(),
                        claims: vec![crate::ChangeClaim {
                            path: "src/main.rs".to_owned(),
                            operation: crate::ChangeOperation::Modify,
                            scope: crate::ChangeScope::Committed,
                        }],
                    },
                },
            },
            Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::AdmitChangeIntent {
                    run_id: producer,
                    expected_version: 1,
                    lease_epoch: 7,
                },
            },
            Command::ResolveRunDriver {
                run_id: producer,
                snapshot: driver.clone(),
            },
            Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::RecordHarnessSnapshot {
                    run_id: producer,
                    snapshot: crate::RunHarnessSnapshot {
                        schema_version: 1,
                        objective_sha256: "1".repeat(64),
                        driver,
                        tools_sha256: Some("2".repeat(64)),
                        skills_sha256: Some("3".repeat(64)),
                        context_sha256: "4".repeat(64),
                        evaluator_sha256: Some("5".repeat(64)),
                    },
                },
            },
            Command::StartReadyRun { run_id: producer },
        ] {
            let events = mission.decide(command).expect("setup should decide");
            for event in &events {
                mission.apply(event).expect("setup should apply");
            }
        }
        let candidate = crate::RunCandidate {
            revision: "candidate-commit".to_owned(),
            content_sha256: digest('a'),
            execution_lease_epoch: Some(7),
            realized_changes: Some(crate::RealizedChangeManifest {
                schema_version: 1,
                scanner_version: 1,
                repository_identity: "fixture-repository".to_owned(),
                base_revision: "base-commit".to_owned(),
                head_revision: "candidate-commit".to_owned(),
                snapshot_revision: None,
                snapshot_tree: None,
                snapshot_ref: None,
                patch_sha256: digest('a'),
                changes: vec![crate::RealizedChange {
                    path: "src/main.rs".to_owned(),
                    operation: crate::ChangeOperation::Modify,
                    content_sha256: Some(digest('b')),
                    git_mode: None,
                    git_object_id: None,
                }],
            }),
            artifact_ids: Vec::new(),
            submitted_by: mission.runs[&producer].actor.id.clone(),
        };
        for command in [
            Command::VerifiedDelivery {
                command: VerifiedDeliveryCommand::SubmitCandidate {
                    run_id: producer,
                    candidate: candidate.clone(),
                },
            },
            Command::FinishRun {
                run_id: producer,
                outcome: FinishOutcome::Succeeded,
                summary: "ready for review".to_owned(),
            },
        ] {
            let events = mission.decide(command).expect("candidate should decide");
            for event in &events {
                mission.apply(event).expect("candidate should apply");
            }
        }

        let verifier = RunId::new();
        let verifier_events = mission
            .decide(Command::CreateVerifierRun {
                subject_run_id: producer,
                verifier_run_id: verifier,
                actor: Actor::agent("reviewer", "codex").expect("agent should be valid"),
                priority: RunPriority::Urgent,
            })
            .expect("reviewable Candidate should create verifier");
        assert_eq!(verifier_events.len(), 2);
        for event in &verifier_events {
            mission.apply(event).expect("verifier event should apply");
        }
        let verifier_input = &mission.verified_delivery.delivery_run_inputs[&verifier];
        assert_eq!(verifier_input.candidate, candidate);
        assert_eq!(verifier_input.source_run_id, producer);
        assert_eq!(
            verifier_input.purpose,
            crate::DeliveryRunPurpose::Verification
        );
        assert!(verifier_input.return_note.is_none());
        assert_eq!(mission.runs[&verifier].dependencies, vec![producer]);

        let returned = mission
            .decide(Command::RejectRunResult {
                run_id: producer,
                by: ActorId::new("alice").expect("owner should be valid"),
                note: "Handle the cancellation race before resubmitting".to_owned(),
            })
            .expect("owner should return the Candidate");
        mission
            .apply(&returned[0])
            .expect("Return should remain durable");
        let retry = RunId::new();
        let retry_events = mission
            .decide(Command::RetryReturnedRun {
                source_run_id: producer,
                retry_run_id: retry,
                actor: mission.runs[&producer].actor.clone(),
                priority: RunPriority::Urgent,
            })
            .expect("returned Candidate should create retry");
        assert_eq!(retry_events.len(), 3);
        for event in &retry_events {
            mission.apply(event).expect("retry event should apply");
        }
        let retry_input = &mission.verified_delivery.delivery_run_inputs[&retry];
        assert_eq!(retry_input.candidate, candidate);
        assert_eq!(retry_input.purpose, crate::DeliveryRunPurpose::Retry);
        assert_eq!(
            retry_input.return_note.as_deref(),
            Some("Handle the cancellation race before resubmitting")
        );
        assert_eq!(mission.runs[&retry].retry_of, Some(producer));
        let retry_intent = &mission.verified_delivery.change_intents[&retry];
        assert_eq!(retry_intent.base_revision, "candidate-commit");
        assert_eq!(retry_intent.claims[0].path, "src/main.rs");
        assert_eq!(
            retry_intent.claims[0].operation,
            crate::ChangeOperation::Modify
        );
    }

    #[test]
    fn escalation_is_a_first_class_attention_item() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        let signal_id = SignalId::new();
        let raised = mission
            .decide(Command::RaiseSignal {
                signal_id,
                run_id,
                kind: SignalKind::Escalation {
                    conflict: "acceptance test contradicts the objective".to_owned(),
                    evidence: Vec::new(),
                    requested_resolution: "repair the test or cancel the Run".to_owned(),
                },
            })
            .expect("evidence-backed escalation should be accepted");
        mission.apply(&raised[0]).expect("escalation should apply");

        assert_eq!(mission.attention_queue()[0].kind, AttentionKind::Escalation);
        assert_eq!(mission.runs[&run_id].status, RunStatus::AwaitingAttention);
    }

    #[test]
    fn a_successful_run_cannot_hide_unresolved_attention() {
        let mut mission = mission();
        let run_id = start_agent(&mut mission);
        let raised = mission
            .decide(Command::RaiseSignal {
                signal_id: SignalId::new(),
                run_id,
                kind: SignalKind::ApprovalNeeded {
                    operation: "Publish release".to_owned(),
                    risk: Risk::Medium,
                },
            })
            .expect("signal should be accepted");
        mission.apply(&raised[0]).expect("event should apply");

        let result = mission.decide(Command::FinishRun {
            run_id,
            outcome: FinishOutcome::Succeeded,
            summary: "Done".to_owned(),
        });
        assert_eq!(result, Err(DomainError::RunNeedsAttention(run_id)));
    }

    #[test]
    fn events_rehydrate_the_same_projection() {
        let creator = Actor::human("alice").expect("test actor should be valid");
        let mission_id = MissionId::new();
        let (mut original, created) =
            Mission::create(mission_id, "Build it", creator).expect("test mission should be valid");
        let run_id = start_agent(&mut original);
        let started = Event::RunStarted {
            mission_id,
            run_id,
            parent: None,
            actor: original.runs[&run_id].actor.clone(),
            objective: original.runs[&run_id].objective.clone(),
        };
        let restored = Mission::rehydrate(&[created, started]).expect("events should rehydrate");
        assert_eq!(restored, original);
    }
}
