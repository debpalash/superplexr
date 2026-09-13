use crate::{ActorId, ArtifactId, GrantId, RunId, SessionId, SignalId};
use thiserror::Error;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum DomainError {
    #[error("actor id cannot be empty")]
    EmptyActorId,
    #[error("mission intent cannot be empty")]
    EmptyMissionIntent,
    #[error("run objective cannot be empty")]
    EmptyRunObjective,
    #[error("session name cannot be empty")]
    EmptySessionName,
    #[error("mission is already closed")]
    MissionClosed,
    #[error("run {0} does not exist")]
    RunNotFound(RunId),
    #[error("run {0} already exists")]
    RunAlreadyExists(RunId),
    #[error("run {0} is already finished")]
    RunFinished(RunId),
    #[error("run {0} is not pending")]
    RunNotPending(RunId),
    #[error("run {0} is not ready to start")]
    RunNotReady(RunId),
    #[error("run {0} already has a different resolved driver")]
    RunDriverConflict(RunId),
    #[error("resolved driver snapshot is invalid")]
    InvalidRunDriverSnapshot,
    #[error("run {0} is not running")]
    RunNotRunning(RunId),
    #[error("run {0} is not paused")]
    RunNotPaused(RunId),
    #[error("run {0} has no successful result awaiting review")]
    RunNotAwaitingReview(RunId),
    #[error("run {0} has no declared change intent")]
    ChangeIntentNotFound(RunId),
    #[error("run {0} change intent is no longer proposed")]
    ChangeIntentNotProposed(RunId),
    #[error(
        "run {run_id} change intent version is stale; expected {expected}, received {actual:?}"
    )]
    StaleChangeIntentVersion {
        run_id: RunId,
        expected: u64,
        actual: Option<u64>,
    },
    #[error("run {run_id} execution lease epoch must exceed {expected_after}; received {actual}")]
    StaleExecutionLease {
        run_id: RunId,
        expected_after: u64,
        actual: u64,
    },
    #[error("run {run_id} change intent conflicts with admitted run {conflicting_run_id}")]
    ChangeIntentConflict {
        run_id: RunId,
        conflicting_run_id: RunId,
    },
    #[error("run {0} change intent has not been admitted")]
    ChangeIntentNotAdmitted(RunId),
    #[error("change intent is invalid")]
    InvalidChangeIntent,
    #[error("run {0} already has an immutable harness snapshot")]
    HarnessSnapshotAlreadyExists(RunId),
    #[error("harness snapshot is invalid")]
    InvalidHarnessSnapshot,
    #[error("run {0} already has an immutable candidate")]
    CandidateAlreadyExists(RunId),
    #[error(
        "run {run_id} candidate has stale execution lease; expected {expected}, received {actual:?}"
    )]
    CandidateLeaseMismatch {
        run_id: RunId,
        expected: u64,
        actual: Option<u64>,
    },
    #[error("run {0} has no submitted candidate")]
    CandidateNotFound(RunId),
    #[error("run {0} has no immutable harness snapshot")]
    HarnessSnapshotNotFound(RunId),
    #[error("run {0} candidate was submitted by a different actor")]
    CandidateSubmitterMismatch(RunId),
    #[error("candidate is invalid")]
    InvalidCandidate,
    #[error("handoff artifact is invalid")]
    InvalidHandoff,
    #[error("evaluation receipt is invalid")]
    InvalidEvaluationReceipt,
    #[error("an evaluation verifier must be a different Run from {0}")]
    VerifierMustBeIndependent(RunId),
    #[error("evaluation requires both subject and verifier Runs to be finished")]
    EvaluationRequiresFinishedRuns,
    #[error("evaluation does not name the current candidate for Run {0}")]
    EvaluationCandidateMismatch(RunId),
    #[error("run {0} requires a passing independent evaluation before settlement")]
    PassingEvaluationRequired(RunId),
    #[error("verified-delivery record is invalid or exceeds its bounds")]
    InvalidVerifiedDeliveryRecord,
    #[error("delivery input for run {0} is already frozen")]
    DeliveryRunInputAlreadyExists(RunId),
    #[error("delivery run input is invalid or does not match its source")]
    InvalidDeliveryRunInput,
    #[error("run {0} has not been returned by its owner")]
    RunNotReturned(RunId),
    #[error("only an agent actor can own a verifier or retry run")]
    DeliveryRunRequiresAgent,
    #[error("settlement note is empty or exceeds its bounds")]
    InvalidSettlement,
    #[error("run {0} cannot depend on itself")]
    SelfDependency(RunId),
    #[error("run {run_id} repeats dependency {dependency_id}")]
    DuplicateDependency { run_id: RunId, dependency_id: RunId },
    #[error("retry target {0} is not finished")]
    RetryTargetNotFinished(RunId),
    #[error("parent run {0} is not active")]
    ParentRunNotActive(RunId),
    #[error("session {0} does not exist")]
    SessionNotFound(SessionId),
    #[error("session {0} already exists")]
    SessionAlreadyExists(SessionId),
    #[error("session {0} is already finished")]
    SessionFinished(SessionId),
    #[error("run {run_id} already uses session {session_id}")]
    RunAlreadyHasSession {
        run_id: RunId,
        session_id: SessionId,
    },
    #[error("session {session_id} is still used by active run {run_id}")]
    SessionInUse {
        session_id: SessionId,
        run_id: RunId,
    },
    #[error("signal {0} does not exist")]
    SignalNotFound(SignalId),
    #[error("signal {0} already exists")]
    SignalAlreadyExists(SignalId),
    #[error("signal {0} does not require a response")]
    SignalNeedsNoResponse(SignalId),
    #[error("signal {0} is already resolved")]
    SignalAlreadyResolved(SignalId),
    #[error("approval signal {0} requires an explicit approval decision")]
    ApprovalRequiresDecision(SignalId),
    #[error("signal {0} is not an approval request")]
    SignalIsNotApproval(SignalId),
    #[error("grant {0} already exists")]
    GrantAlreadyExists(GrantId),
    #[error("grant {0} does not exist")]
    GrantNotFound(GrantId),
    #[error("enforced grants are unavailable without an active policy adapter")]
    EnforcedGrantUnavailable,
    #[error("grant use count must be at least one")]
    InvalidGrantUseCount,
    #[error("artifact {0} already exists")]
    ArtifactAlreadyExists(ArtifactId),
    #[error("artifact name, media type, and locator cannot be empty")]
    InvalidArtifact,
    #[error("run {0} still has unresolved attention items")]
    RunNeedsAttention(RunId),
    #[error("only a human actor can take direct control")]
    ControllerMustBeHuman,
    #[error("control can only be returned to an agent actor")]
    ReturnControllerMustBeAgent,
    #[error("session {session_id} is controlled by {controller}, not {requester}")]
    NotController {
        session_id: SessionId,
        controller: ActorId,
        requester: ActorId,
    },
    #[error("all sessions must finish before the mission completes")]
    MissionHasActiveSessions,
    #[error("all successful run results must be reviewed before the mission completes")]
    MissionHasUnreviewedRuns,
    #[error("event does not belong to this mission")]
    WrongMission,
    #[error("event stream is invalid: {0}")]
    InvalidEventStream(String),
}
