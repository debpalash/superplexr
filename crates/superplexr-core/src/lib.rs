//! The durable domain model for agentic execution.
//!
//! The module deliberately knows nothing about PTYs, terminal rendering, sockets,
//! or UI layout. Callers create a mission, ask it to decide commands, and persist
//! the returned events before applying them.

mod error;
mod id;
mod mission;
mod verified_delivery;

pub use error::DomainError;
pub use id::{ActorId, ArtifactId, FaultId, GrantId, MissionId, RunId, SessionId, SignalId};
pub use mission::{
    Actor, ActorKind, Artifact, AttentionItem, AttentionKind, Command, Event, FinishOutcome, Grant,
    GrantEnforcement, Mission, MissionStatus, PauseReason, Risk, Run, RunDisposition,
    RunDriverSnapshot, RunPhase, RunPriority, RunReadiness, RunSettlement, RunStatus, ScheduledRun,
    SchedulerPlan, Session, SessionStatus, Signal, SignalKind, SignalResolution,
};
pub use verified_delivery::{
    ChangeClaim, ChangeClaimKey, ChangeIntent, ChangeIntentSpec, ChangeIntentState,
    ChangeOperation, ChangeScope, DeliveryRunInput, DeliveryRunPurpose, EvaluationCheck,
    EvaluationReceipt, EvaluationVerdict, HandoffArtifact, RealizedChange, RealizedChangeManifest,
    RunCandidate, RunHarnessSnapshot, VerificationPolicy, VerifiedDelivery,
    VerifiedDeliveryCommand, VerifiedDeliveryEvent,
};
