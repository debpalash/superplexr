use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::{
    ActorId, ArtifactId, DomainError, FinishOutcome, Run, RunDisposition, RunDriverSnapshot, RunId,
    RunPhase, mission::validate_driver_snapshot,
};

const MAX_IDENTITY_BYTES: usize = 256;
const MAX_REVISION_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 4_096;
const MAX_CLAIMS: usize = 1_024;
const MAX_LIST_ITEMS: usize = 256;
const MAX_ITEM_BYTES: usize = 4_096;
const SHA256_HEX_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOperation {
    Create,
    Modify,
    Delete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeScope {
    Committed,
    Contingent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangeClaim {
    pub path: String,
    pub operation: ChangeOperation,
    pub scope: ChangeScope,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ChangeClaimKey {
    pub path: String,
    pub operation: ChangeOperation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangeIntentSpec {
    pub repository_identity: String,
    pub base_revision: String,
    pub claims: Vec<ChangeClaim>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeIntentState {
    Proposed,
    Admitted,
    Released,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChangeIntent {
    pub run_id: RunId,
    pub version: u64,
    pub state: ChangeIntentState,
    pub lease_epoch: u64,
    pub repository_identity: String,
    pub base_revision: String,
    pub claims: Vec<ChangeClaim>,
}

impl ChangeIntent {
    fn proposed(run_id: RunId, version: u64, spec: ChangeIntentSpec) -> Self {
        Self {
            run_id,
            version,
            state: ChangeIntentState::Proposed,
            lease_epoch: 0,
            repository_identity: spec.repository_identity,
            base_revision: spec.base_revision,
            claims: spec.claims,
        }
    }

    #[must_use]
    pub fn conflicts_with(&self, other: &Self) -> bool {
        self.run_id != other.run_id
            && self.repository_identity == other.repository_identity
            && self.claims.iter().any(|left| {
                left.scope == ChangeScope::Committed
                    && other.claims.iter().any(|right| {
                        right.scope == ChangeScope::Committed && left.path == right.path
                    })
            })
    }

    #[must_use]
    pub fn authorizes(&self, path: &str, operation: ChangeOperation, lease_epoch: u64) -> bool {
        self.state == ChangeIntentState::Admitted
            && self.lease_epoch == lease_epoch
            && self.claims.iter().any(|claim| {
                claim.scope == ChangeScope::Committed
                    && claim.path == path
                    && claim.operation == operation
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunHarnessSnapshot {
    pub schema_version: u16,
    pub objective_sha256: String,
    pub driver: RunDriverSnapshot,
    pub tools_sha256: Option<String>,
    pub skills_sha256: Option<String>,
    pub context_sha256: String,
    pub evaluator_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationPolicy {
    #[default]
    OwnerReview,
    Independent,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunCandidate {
    pub revision: String,
    pub content_sha256: String,
    #[serde(default)]
    pub execution_lease_epoch: Option<u64>,
    /// Server-authored description of the checkout state admitted for review.
    ///
    /// `None` remains readable for event logs written before manifest support;
    /// new Candidate submissions for Runs with a ChangeIntent are stamped by
    /// the runtime before they reach the domain model.
    #[serde(default)]
    pub realized_changes: Option<RealizedChangeManifest>,
    pub artifact_ids: Vec<ArtifactId>,
    pub submitted_by: ActorId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RealizedChange {
    pub path: String,
    pub operation: ChangeOperation,
    /// Digest of the resulting file bytes. Deletes have no resulting content.
    pub content_sha256: Option<String>,
    /// Git tree mode captured in the immutable Candidate snapshot.
    #[serde(default)]
    pub git_mode: Option<String>,
    /// Blob object captured in the immutable Candidate snapshot.
    #[serde(default)]
    pub git_object_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RealizedChangeManifest {
    pub schema_version: u16,
    pub scanner_version: u16,
    pub repository_identity: String,
    pub base_revision: String,
    pub head_revision: String,
    /// Synthetic commit containing exactly the admitted checkout bytes.
    #[serde(default)]
    pub snapshot_revision: Option<String>,
    /// Root tree written through the isolated temporary index.
    #[serde(default)]
    pub snapshot_tree: Option<String>,
    /// Namespaced ref retaining the snapshot against Git garbage collection.
    #[serde(default)]
    pub snapshot_ref: Option<String>,
    pub patch_sha256: String,
    pub changes: Vec<RealizedChange>,
}

/// Why a later Run received one frozen verified-delivery input.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryRunPurpose {
    Verification,
    Retry,
}

/// Immutable evidence supplied to a verifier or returned-work retry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeliveryRunInput {
    pub run_id: RunId,
    pub source_run_id: RunId,
    pub purpose: DeliveryRunPurpose,
    pub candidate: RunCandidate,
    pub harness_snapshot: RunHarnessSnapshot,
    #[serde(default)]
    pub return_note: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HandoffArtifact {
    pub artifact_id: ArtifactId,
    pub run_id: RunId,
    pub summary: String,
    pub completed: Vec<String>,
    pub remaining: Vec<String>,
    pub evidence: Vec<ArtifactId>,
    pub external_effects: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationVerdict {
    Passed,
    Failed,
    Inconclusive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvaluationCheck {
    pub name: String,
    pub passed: bool,
    pub evidence: Vec<ArtifactId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvaluationReceipt {
    pub artifact_id: ArtifactId,
    pub subject_run_id: RunId,
    pub verifier_run_id: RunId,
    pub candidate_sha256: String,
    pub verdict: EvaluationVerdict,
    pub checks: Vec<EvaluationCheck>,
    pub delivery_validated: bool,
    pub repeatable: bool,
    pub summary: String,
}

impl EvaluationReceipt {
    #[must_use]
    pub fn passes(&self, candidate: &RunCandidate) -> bool {
        self.candidate_sha256 == candidate.content_sha256
            && self.verdict == EvaluationVerdict::Passed
            && self.delivery_validated
            && self.checks.iter().all(|check| check.passed)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VerifiedDeliveryCommand {
    DeclareChangeIntent {
        run_id: RunId,
        expected_version: Option<u64>,
        spec: ChangeIntentSpec,
    },
    AdmitChangeIntent {
        run_id: RunId,
        expected_version: u64,
        lease_epoch: u64,
    },
    PromoteContingentClaims {
        run_id: RunId,
        expected_version: u64,
        lease_epoch: u64,
        claims: Vec<ChangeClaimKey>,
    },
    RecordHarnessSnapshot {
        run_id: RunId,
        snapshot: RunHarnessSnapshot,
    },
    SetVerificationPolicy {
        run_id: RunId,
        policy: VerificationPolicy,
    },
    SubmitCandidate {
        run_id: RunId,
        candidate: RunCandidate,
    },
    RecordHandoff {
        handoff: HandoffArtifact,
    },
    RecordEvaluationReceipt {
        receipt: EvaluationReceipt,
    },
    FreezeDeliveryRunInput {
        input: Box<DeliveryRunInput>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VerifiedDeliveryEvent {
    ChangeIntentDeclared {
        intent: ChangeIntent,
    },
    ChangeIntentAdmitted {
        run_id: RunId,
        version: u64,
        lease_epoch: u64,
    },
    ContingentClaimsPromoted {
        run_id: RunId,
        version: u64,
        lease_epoch: u64,
        claims: Vec<ChangeClaimKey>,
    },
    HarnessSnapshotRecorded {
        run_id: RunId,
        snapshot: RunHarnessSnapshot,
    },
    VerificationPolicySet {
        run_id: RunId,
        policy: VerificationPolicy,
    },
    CandidateSubmitted {
        run_id: RunId,
        candidate: RunCandidate,
    },
    HandoffRecorded {
        handoff: HandoffArtifact,
    },
    EvaluationReceiptRecorded {
        receipt: EvaluationReceipt,
    },
    DeliveryRunInputFrozen {
        input: Box<DeliveryRunInput>,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifiedDelivery {
    pub change_intents: HashMap<RunId, ChangeIntent>,
    pub harness_snapshots: HashMap<RunId, RunHarnessSnapshot>,
    pub verification_policies: HashMap<RunId, VerificationPolicy>,
    pub candidates: HashMap<RunId, RunCandidate>,
    pub handoffs: HashMap<ArtifactId, HandoffArtifact>,
    pub evaluation_receipts: HashMap<ArtifactId, EvaluationReceipt>,
    #[serde(default)]
    pub delivery_run_inputs: HashMap<RunId, DeliveryRunInput>,
}

impl VerifiedDelivery {
    pub fn decide(
        &self,
        command: VerifiedDeliveryCommand,
        runs: &HashMap<RunId, Run>,
    ) -> Result<VerifiedDeliveryEvent, DomainError> {
        match command {
            VerifiedDeliveryCommand::DeclareChangeIntent {
                run_id,
                expected_version,
                spec,
            } => {
                require_pending_run(runs, run_id)?;
                validate_change_intent_spec(&spec)?;
                let version = match self.change_intents.get(&run_id) {
                    Some(current) => {
                        if current.state != ChangeIntentState::Proposed {
                            return Err(DomainError::ChangeIntentNotProposed(run_id));
                        }
                        if expected_version != Some(current.version) {
                            return Err(DomainError::StaleChangeIntentVersion {
                                run_id,
                                expected: current.version,
                                actual: expected_version,
                            });
                        }
                        current.version.saturating_add(1)
                    }
                    None => {
                        if expected_version.is_some() {
                            return Err(DomainError::StaleChangeIntentVersion {
                                run_id,
                                expected: 0,
                                actual: expected_version,
                            });
                        }
                        1
                    }
                };
                Ok(VerifiedDeliveryEvent::ChangeIntentDeclared {
                    intent: ChangeIntent::proposed(run_id, version, spec),
                })
            }
            VerifiedDeliveryCommand::AdmitChangeIntent {
                run_id,
                expected_version,
                lease_epoch,
            } => {
                require_pending_run(runs, run_id)?;
                let intent = self
                    .change_intents
                    .get(&run_id)
                    .ok_or(DomainError::ChangeIntentNotFound(run_id))?;
                if intent.state != ChangeIntentState::Proposed {
                    return Err(DomainError::ChangeIntentNotProposed(run_id));
                }
                if intent.version != expected_version {
                    return Err(DomainError::StaleChangeIntentVersion {
                        run_id,
                        expected: intent.version,
                        actual: Some(expected_version),
                    });
                }
                if lease_epoch == 0 || lease_epoch <= intent.lease_epoch {
                    return Err(DomainError::StaleExecutionLease {
                        run_id,
                        expected_after: intent.lease_epoch,
                        actual: lease_epoch,
                    });
                }
                if let Some(conflict) = self.change_intents.values().find(|other| {
                    other.state == ChangeIntentState::Admitted && intent.conflicts_with(other)
                }) {
                    return Err(DomainError::ChangeIntentConflict {
                        run_id,
                        conflicting_run_id: conflict.run_id,
                    });
                }
                Ok(VerifiedDeliveryEvent::ChangeIntentAdmitted {
                    run_id,
                    version: expected_version,
                    lease_epoch,
                })
            }
            VerifiedDeliveryCommand::PromoteContingentClaims {
                run_id,
                expected_version,
                lease_epoch,
                claims,
            } => {
                let run = require_started_run(runs, run_id)?;
                if run.phase == RunPhase::Finished {
                    return Err(DomainError::RunFinished(run_id));
                }
                let intent = self
                    .change_intents
                    .get(&run_id)
                    .ok_or(DomainError::ChangeIntentNotFound(run_id))?;
                if intent.state != ChangeIntentState::Admitted {
                    return Err(DomainError::ChangeIntentNotAdmitted(run_id));
                }
                if intent.version != expected_version {
                    return Err(DomainError::StaleChangeIntentVersion {
                        run_id,
                        expected: intent.version,
                        actual: Some(expected_version),
                    });
                }
                if lease_epoch <= intent.lease_epoch {
                    return Err(DomainError::StaleExecutionLease {
                        run_id,
                        expected_after: intent.lease_epoch,
                        actual: lease_epoch,
                    });
                }
                validate_claim_keys(&claims)?;
                if self.candidates.contains_key(&run_id)
                    || claims.iter().any(|promotion| {
                        !intent.claims.iter().any(|claim| {
                            claim.path == promotion.path
                                && claim.operation == promotion.operation
                                && claim.scope == ChangeScope::Contingent
                        })
                    })
                {
                    return Err(DomainError::InvalidChangeIntent);
                }
                let mut promoted = intent.clone();
                for claim in &mut promoted.claims {
                    if claims.iter().any(|promotion| {
                        promotion.path == claim.path && promotion.operation == claim.operation
                    }) {
                        claim.scope = ChangeScope::Committed;
                    }
                }
                if let Some(conflict) = self.change_intents.values().find(|other| {
                    other.state == ChangeIntentState::Admitted && promoted.conflicts_with(other)
                }) {
                    return Err(DomainError::ChangeIntentConflict {
                        run_id,
                        conflicting_run_id: conflict.run_id,
                    });
                }
                Ok(VerifiedDeliveryEvent::ContingentClaimsPromoted {
                    run_id,
                    version: intent.version.saturating_add(1),
                    lease_epoch,
                    claims,
                })
            }
            VerifiedDeliveryCommand::RecordHarnessSnapshot { run_id, snapshot } => {
                let run = require_pending_run(runs, run_id)?;
                if self.harness_snapshots.contains_key(&run_id) {
                    return Err(DomainError::HarnessSnapshotAlreadyExists(run_id));
                }
                validate_harness_snapshot(&snapshot)?;
                if run.driver_snapshot.as_ref() != Some(&snapshot.driver) {
                    return Err(DomainError::InvalidHarnessSnapshot);
                }
                Ok(VerifiedDeliveryEvent::HarnessSnapshotRecorded { run_id, snapshot })
            }
            VerifiedDeliveryCommand::SetVerificationPolicy { run_id, policy } => {
                require_pending_run(runs, run_id)?;
                Ok(VerifiedDeliveryEvent::VerificationPolicySet { run_id, policy })
            }
            VerifiedDeliveryCommand::SubmitCandidate { run_id, candidate } => {
                let run = require_started_run(runs, run_id)?;
                if run.actor.id != candidate.submitted_by {
                    return Err(DomainError::CandidateSubmitterMismatch(run_id));
                }
                match self.change_intents.get(&run_id) {
                    Some(intent) if intent.state != ChangeIntentState::Admitted => {
                        return Err(DomainError::ChangeIntentNotAdmitted(run_id));
                    }
                    Some(intent) if candidate.execution_lease_epoch != Some(intent.lease_epoch) => {
                        return Err(DomainError::CandidateLeaseMismatch {
                            run_id,
                            expected: intent.lease_epoch,
                            actual: candidate.execution_lease_epoch,
                        });
                    }
                    None if candidate.execution_lease_epoch.is_some() => {
                        return Err(DomainError::InvalidCandidate);
                    }
                    _ => {}
                }
                if self.candidates.contains_key(&run_id) {
                    return Err(DomainError::CandidateAlreadyExists(run_id));
                }
                validate_candidate(&candidate)?;
                Ok(VerifiedDeliveryEvent::CandidateSubmitted { run_id, candidate })
            }
            VerifiedDeliveryCommand::RecordHandoff { handoff } => {
                require_started_run(runs, handoff.run_id)?;
                if self.handoffs.contains_key(&handoff.artifact_id)
                    || self.evaluation_receipts.contains_key(&handoff.artifact_id)
                {
                    return Err(DomainError::ArtifactAlreadyExists(handoff.artifact_id));
                }
                validate_handoff(&handoff)?;
                Ok(VerifiedDeliveryEvent::HandoffRecorded { handoff })
            }
            VerifiedDeliveryCommand::RecordEvaluationReceipt { receipt } => {
                if receipt.subject_run_id == receipt.verifier_run_id {
                    return Err(DomainError::VerifierMustBeIndependent(
                        receipt.subject_run_id,
                    ));
                }
                let subject = runs
                    .get(&receipt.subject_run_id)
                    .ok_or(DomainError::RunNotFound(receipt.subject_run_id))?;
                let verifier = runs
                    .get(&receipt.verifier_run_id)
                    .ok_or(DomainError::RunNotFound(receipt.verifier_run_id))?;
                if subject.phase != RunPhase::Finished
                    || verifier.phase != RunPhase::Finished
                    || subject.outcome != Some(FinishOutcome::Succeeded)
                    || verifier.outcome != Some(FinishOutcome::Succeeded)
                {
                    return Err(DomainError::EvaluationRequiresFinishedRuns);
                }
                let candidate = self
                    .candidates
                    .get(&receipt.subject_run_id)
                    .ok_or(DomainError::CandidateNotFound(receipt.subject_run_id))?;
                if receipt.candidate_sha256 != candidate.content_sha256 {
                    return Err(DomainError::EvaluationCandidateMismatch(
                        receipt.subject_run_id,
                    ));
                }
                if self.handoffs.contains_key(&receipt.artifact_id)
                    || self.evaluation_receipts.contains_key(&receipt.artifact_id)
                {
                    return Err(DomainError::ArtifactAlreadyExists(receipt.artifact_id));
                }
                validate_evaluation_receipt(&receipt)?;
                Ok(VerifiedDeliveryEvent::EvaluationReceiptRecorded { receipt })
            }
            VerifiedDeliveryCommand::FreezeDeliveryRunInput { input } => {
                let target = require_pending_run(runs, input.run_id)?;
                if self.delivery_run_inputs.contains_key(&input.run_id) {
                    return Err(DomainError::DeliveryRunInputAlreadyExists(input.run_id));
                }
                let source = runs
                    .get(&input.source_run_id)
                    .ok_or(DomainError::RunNotFound(input.source_run_id))?;
                let expected = self.delivery_run_input(input.run_id, source, input.purpose)?;
                let linkage_matches = match input.purpose {
                    DeliveryRunPurpose::Verification => {
                        target.retry_of.is_none()
                            && target.dependencies.as_slice() == [input.source_run_id]
                    }
                    DeliveryRunPurpose::Retry => {
                        target.retry_of == Some(input.source_run_id)
                            && target.dependencies.is_empty()
                    }
                };
                if input.as_ref() != &expected || !linkage_matches {
                    return Err(DomainError::InvalidDeliveryRunInput);
                }
                Ok(VerifiedDeliveryEvent::DeliveryRunInputFrozen { input })
            }
        }
    }

    pub fn apply(
        &mut self,
        event: &VerifiedDeliveryEvent,
        runs: &HashMap<RunId, Run>,
    ) -> Result<(), DomainError> {
        let command = match event {
            VerifiedDeliveryEvent::ChangeIntentDeclared { intent } => {
                let expected_version = intent
                    .version
                    .checked_sub(1)
                    .and_then(|version| (version > 0).then_some(version));
                VerifiedDeliveryCommand::DeclareChangeIntent {
                    run_id: intent.run_id,
                    expected_version,
                    spec: ChangeIntentSpec {
                        repository_identity: intent.repository_identity.clone(),
                        base_revision: intent.base_revision.clone(),
                        claims: intent.claims.clone(),
                    },
                }
            }
            VerifiedDeliveryEvent::ChangeIntentAdmitted {
                run_id,
                version,
                lease_epoch,
            } => VerifiedDeliveryCommand::AdmitChangeIntent {
                run_id: *run_id,
                expected_version: *version,
                lease_epoch: *lease_epoch,
            },
            VerifiedDeliveryEvent::ContingentClaimsPromoted {
                run_id,
                version,
                lease_epoch,
                claims,
            } => VerifiedDeliveryCommand::PromoteContingentClaims {
                run_id: *run_id,
                expected_version: version.saturating_sub(1),
                lease_epoch: *lease_epoch,
                claims: claims.clone(),
            },
            VerifiedDeliveryEvent::HarnessSnapshotRecorded { run_id, snapshot } => {
                VerifiedDeliveryCommand::RecordHarnessSnapshot {
                    run_id: *run_id,
                    snapshot: snapshot.clone(),
                }
            }
            VerifiedDeliveryEvent::VerificationPolicySet { run_id, policy } => {
                VerifiedDeliveryCommand::SetVerificationPolicy {
                    run_id: *run_id,
                    policy: *policy,
                }
            }
            VerifiedDeliveryEvent::CandidateSubmitted { run_id, candidate } => {
                VerifiedDeliveryCommand::SubmitCandidate {
                    run_id: *run_id,
                    candidate: candidate.clone(),
                }
            }
            VerifiedDeliveryEvent::HandoffRecorded { handoff } => {
                VerifiedDeliveryCommand::RecordHandoff {
                    handoff: handoff.clone(),
                }
            }
            VerifiedDeliveryEvent::EvaluationReceiptRecorded { receipt } => {
                VerifiedDeliveryCommand::RecordEvaluationReceipt {
                    receipt: receipt.clone(),
                }
            }
            VerifiedDeliveryEvent::DeliveryRunInputFrozen { input } => {
                VerifiedDeliveryCommand::FreezeDeliveryRunInput {
                    input: input.clone(),
                }
            }
        };
        let decided = self.decide(command, runs)?;
        if &decided != event {
            return Err(DomainError::InvalidEventStream(
                "verified-delivery event differs from the command decision".to_owned(),
            ));
        }

        match event {
            VerifiedDeliveryEvent::ChangeIntentDeclared { intent } => {
                self.change_intents.insert(intent.run_id, intent.clone());
            }
            VerifiedDeliveryEvent::ChangeIntentAdmitted {
                run_id,
                version: _,
                lease_epoch,
            } => {
                let intent = self
                    .change_intents
                    .get_mut(run_id)
                    .ok_or(DomainError::ChangeIntentNotFound(*run_id))?;
                intent.state = ChangeIntentState::Admitted;
                intent.lease_epoch = *lease_epoch;
            }
            VerifiedDeliveryEvent::ContingentClaimsPromoted {
                run_id,
                version,
                lease_epoch,
                claims,
            } => {
                let intent = self
                    .change_intents
                    .get_mut(run_id)
                    .ok_or(DomainError::ChangeIntentNotFound(*run_id))?;
                for claim in &mut intent.claims {
                    if claims.iter().any(|promotion| {
                        promotion.path == claim.path && promotion.operation == claim.operation
                    }) {
                        claim.scope = ChangeScope::Committed;
                    }
                }
                intent.version = *version;
                intent.lease_epoch = *lease_epoch;
            }
            VerifiedDeliveryEvent::HarnessSnapshotRecorded { run_id, snapshot } => {
                self.harness_snapshots.insert(*run_id, snapshot.clone());
            }
            VerifiedDeliveryEvent::VerificationPolicySet { run_id, policy } => {
                self.verification_policies.insert(*run_id, *policy);
            }
            VerifiedDeliveryEvent::CandidateSubmitted { run_id, candidate } => {
                self.candidates.insert(*run_id, candidate.clone());
            }
            VerifiedDeliveryEvent::HandoffRecorded { handoff } => {
                self.handoffs.insert(handoff.artifact_id, handoff.clone());
            }
            VerifiedDeliveryEvent::EvaluationReceiptRecorded { receipt } => {
                self.evaluation_receipts
                    .insert(receipt.artifact_id, receipt.clone());
            }
            VerifiedDeliveryEvent::DeliveryRunInputFrozen { input } => {
                self.delivery_run_inputs
                    .insert(input.run_id, input.as_ref().clone());
            }
        }
        Ok(())
    }

    pub fn release_run(&mut self, run_id: RunId) {
        if let Some(intent) = self.change_intents.get_mut(&run_id)
            && intent.state == ChangeIntentState::Admitted
        {
            intent.state = ChangeIntentState::Released;
        }
    }

    pub fn ensure_start_admitted(&self, run_id: RunId) -> Result<(), DomainError> {
        if self
            .change_intents
            .get(&run_id)
            .is_some_and(|intent| intent.state != ChangeIntentState::Admitted)
        {
            return Err(DomainError::ChangeIntentNotAdmitted(run_id));
        }
        Ok(())
    }

    pub fn ensure_settleable(&self, run_id: RunId) -> Result<(), DomainError> {
        let Some(candidate) = self.candidates.get(&run_id) else {
            if self.verification_policy(run_id) == VerificationPolicy::Independent {
                return Err(DomainError::CandidateNotFound(run_id));
            }
            return Ok(());
        };
        if self.verification_policy(run_id) == VerificationPolicy::Independent
            && !self
                .evaluation_receipts
                .values()
                .any(|receipt| receipt.subject_run_id == run_id && receipt.passes(candidate))
        {
            return Err(DomainError::PassingEvaluationRequired(run_id));
        }
        Ok(())
    }

    #[must_use]
    pub fn verification_policy(&self, run_id: RunId) -> VerificationPolicy {
        self.verification_policies
            .get(&run_id)
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn delivery_run_input(
        &self,
        run_id: RunId,
        source: &Run,
        purpose: DeliveryRunPurpose,
    ) -> Result<DeliveryRunInput, DomainError> {
        if source.phase != RunPhase::Finished || source.outcome != Some(FinishOutcome::Succeeded) {
            return Err(DomainError::RunNotAwaitingReview(source.id));
        }
        let candidate = self
            .candidates
            .get(&source.id)
            .cloned()
            .ok_or(DomainError::CandidateNotFound(source.id))?;
        let harness_snapshot = self
            .harness_snapshots
            .get(&source.id)
            .cloned()
            .ok_or(DomainError::HarnessSnapshotNotFound(source.id))?;
        let return_note = match purpose {
            DeliveryRunPurpose::Verification => {
                if source.disposition != RunDisposition::AwaitingReview {
                    return Err(DomainError::RunNotAwaitingReview(source.id));
                }
                None
            }
            DeliveryRunPurpose::Retry => {
                if source.disposition != RunDisposition::Rejected {
                    return Err(DomainError::RunNotReturned(source.id));
                }
                Some(
                    source
                        .settlement
                        .as_ref()
                        .filter(|settlement| {
                            settlement.disposition == RunDisposition::Rejected
                                && !settlement.note.trim().is_empty()
                        })
                        .map(|settlement| settlement.note.clone())
                        .ok_or(DomainError::RunNotReturned(source.id))?,
                )
            }
        };
        Ok(DeliveryRunInput {
            run_id,
            source_run_id: source.id,
            purpose,
            candidate,
            harness_snapshot,
            return_note,
        })
    }
}

fn require_pending_run(runs: &HashMap<RunId, Run>, run_id: RunId) -> Result<&Run, DomainError> {
    let run = runs.get(&run_id).ok_or(DomainError::RunNotFound(run_id))?;
    if run.phase != RunPhase::Pending {
        return Err(DomainError::RunNotPending(run_id));
    }
    Ok(run)
}

fn require_started_run(runs: &HashMap<RunId, Run>, run_id: RunId) -> Result<&Run, DomainError> {
    let run = runs.get(&run_id).ok_or(DomainError::RunNotFound(run_id))?;
    if run.phase == RunPhase::Pending {
        return Err(DomainError::RunNotRunning(run_id));
    }
    Ok(run)
}

fn validate_change_intent_spec(spec: &ChangeIntentSpec) -> Result<(), DomainError> {
    validate_text(&spec.repository_identity, MAX_IDENTITY_BYTES)?;
    validate_text(&spec.base_revision, MAX_REVISION_BYTES)?;
    if spec.claims.is_empty() || spec.claims.len() > MAX_CLAIMS {
        return Err(DomainError::InvalidChangeIntent);
    }
    let mut unique = HashSet::with_capacity(spec.claims.len());
    for claim in &spec.claims {
        validate_relative_path(&claim.path)?;
        if !unique.insert((&claim.path, claim.operation)) {
            return Err(DomainError::InvalidChangeIntent);
        }
    }
    Ok(())
}

fn validate_claim_keys(claims: &[ChangeClaimKey]) -> Result<(), DomainError> {
    if claims.is_empty() || claims.len() > MAX_CLAIMS {
        return Err(DomainError::InvalidChangeIntent);
    }
    let mut unique = HashSet::with_capacity(claims.len());
    for claim in claims {
        validate_relative_path(&claim.path)?;
        if !unique.insert((&claim.path, claim.operation)) {
            return Err(DomainError::InvalidChangeIntent);
        }
    }
    Ok(())
}

fn validate_harness_snapshot(snapshot: &RunHarnessSnapshot) -> Result<(), DomainError> {
    if snapshot.schema_version == 0
        || !is_sha256(&snapshot.objective_sha256)
        || !is_sha256(&snapshot.driver.process_spec_sha256)
        || !is_sha256(&snapshot.context_sha256)
        || snapshot
            .tools_sha256
            .as_deref()
            .is_some_and(|value| !is_sha256(value))
        || snapshot
            .skills_sha256
            .as_deref()
            .is_some_and(|value| !is_sha256(value))
        || snapshot
            .evaluator_sha256
            .as_deref()
            .is_some_and(|value| !is_sha256(value))
    {
        return Err(DomainError::InvalidHarnessSnapshot);
    }
    validate_driver_snapshot(&snapshot.driver).map_err(|_| DomainError::InvalidHarnessSnapshot)?;
    Ok(())
}

fn validate_candidate(candidate: &RunCandidate) -> Result<(), DomainError> {
    validate_text(&candidate.revision, MAX_REVISION_BYTES)?;
    if !is_sha256(&candidate.content_sha256)
        || candidate.artifact_ids.len() > MAX_LIST_ITEMS
        || !all_unique(&candidate.artifact_ids)
    {
        return Err(DomainError::InvalidCandidate);
    }
    if let Some(manifest) = &candidate.realized_changes {
        validate_realized_change_manifest(manifest)?;
        let authoritative_revision = manifest
            .snapshot_revision
            .as_deref()
            .unwrap_or(&manifest.head_revision);
        if candidate.revision != authoritative_revision
            || candidate.content_sha256 != manifest.patch_sha256
        {
            return Err(DomainError::InvalidCandidate);
        }
    }
    Ok(())
}

fn validate_realized_change_manifest(manifest: &RealizedChangeManifest) -> Result<(), DomainError> {
    if manifest.schema_version == 0
        || manifest.scanner_version == 0
        || manifest.changes.len() > MAX_CLAIMS
        || !is_sha256(&manifest.patch_sha256)
    {
        return Err(DomainError::InvalidCandidate);
    }
    validate_text(&manifest.repository_identity, MAX_IDENTITY_BYTES)?;
    validate_text(&manifest.base_revision, MAX_REVISION_BYTES)?;
    validate_text(&manifest.head_revision, MAX_REVISION_BYTES)?;
    let mut unique = HashSet::with_capacity(manifest.changes.len());
    for change in &manifest.changes {
        validate_relative_path(&change.path)?;
        let has_snapshot_entry = change.git_mode.is_some() && change.git_object_id.is_some();
        if !unique.insert((&change.path, change.operation))
            || change
                .content_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256(digest))
            || (change.operation == ChangeOperation::Delete) != change.content_sha256.is_none()
            || (manifest.scanner_version >= 2
                && (change.operation == ChangeOperation::Delete) == has_snapshot_entry)
            || change
                .git_object_id
                .as_deref()
                .is_some_and(|object| !is_object_id(object))
            || change
                .git_mode
                .as_deref()
                .is_some_and(|mode| !matches!(mode, "100644" | "100755" | "120000"))
        {
            return Err(DomainError::InvalidCandidate);
        }
    }
    let snapshot_fields = [
        manifest.snapshot_revision.as_deref(),
        manifest.snapshot_tree.as_deref(),
        manifest.snapshot_ref.as_deref(),
    ];
    if manifest.scanner_version >= 2
        && (snapshot_fields.iter().any(Option::is_none)
            || !is_object_id(manifest.snapshot_revision.as_deref().unwrap_or_default())
            || !is_object_id(manifest.snapshot_tree.as_deref().unwrap_or_default())
            || manifest
                .snapshot_ref
                .as_deref()
                .is_none_or(|reference| !is_snapshot_ref(reference)))
    {
        return Err(DomainError::InvalidCandidate);
    }
    Ok(())
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_snapshot_ref(value: &str) -> bool {
    value.starts_with("refs/termi9ne/candidates/")
        && !value.contains("..")
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn validate_handoff(handoff: &HandoffArtifact) -> Result<(), DomainError> {
    validate_text(&handoff.summary, MAX_ITEM_BYTES)?;
    validate_text_list(&handoff.completed)?;
    validate_text_list(&handoff.remaining)?;
    validate_text_list(&handoff.external_effects)?;
    if handoff.evidence.len() > MAX_LIST_ITEMS || !all_unique(&handoff.evidence) {
        return Err(DomainError::InvalidHandoff);
    }
    Ok(())
}

fn validate_evaluation_receipt(receipt: &EvaluationReceipt) -> Result<(), DomainError> {
    if !is_sha256(&receipt.candidate_sha256)
        || receipt.checks.is_empty()
        || receipt.checks.len() > MAX_LIST_ITEMS
    {
        return Err(DomainError::InvalidEvaluationReceipt);
    }
    validate_text(&receipt.summary, MAX_ITEM_BYTES)?;
    let mut names = HashSet::with_capacity(receipt.checks.len());
    for check in &receipt.checks {
        validate_text(&check.name, MAX_ITEM_BYTES)?;
        if !names.insert(&check.name)
            || check.evidence.len() > MAX_LIST_ITEMS
            || !all_unique(&check.evidence)
        {
            return Err(DomainError::InvalidEvaluationReceipt);
        }
    }
    Ok(())
}

fn validate_text(value: &str, maximum: usize) -> Result<(), DomainError> {
    if value.trim().is_empty() || value.len() > maximum || value.contains('\0') {
        return Err(DomainError::InvalidVerifiedDeliveryRecord);
    }
    Ok(())
}

fn validate_text_list(values: &[String]) -> Result<(), DomainError> {
    if values.len() > MAX_LIST_ITEMS {
        return Err(DomainError::InvalidVerifiedDeliveryRecord);
    }
    for value in values {
        validate_text(value, MAX_ITEM_BYTES)?;
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), DomainError> {
    validate_text(path, MAX_PATH_BYTES)?;
    let candidate = std::path::Path::new(path);
    if path.contains(['\n', '\r'])
        || candidate.is_absolute()
        || candidate.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(DomainError::InvalidChangeIntent);
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == SHA256_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn all_unique<T: Eq + std::hash::Hash>(values: &[T]) -> bool {
    let mut unique = HashSet::with_capacity(values.len());
    values.iter().all(|value| unique.insert(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Actor, RunDisposition, RunPriority, RunStatus};

    fn pending_run(run_id: RunId) -> Run {
        Run {
            id: run_id,
            parent: None,
            dependencies: Vec::new(),
            retry_of: None,
            priority: RunPriority::Normal,
            phase: RunPhase::Pending,
            outcome: None,
            disposition: RunDisposition::None,
            pause_reason: None,
            actor: Actor::agent("worker", "fixture").expect("fixture actor must be valid"),
            planned_at_unix_micros: None,
            finished_at_unix_micros: None,
            driver_snapshot: None,
            primary_session: None,
            objective: "change one file".to_owned(),
            status: RunStatus::Pending,
            summary: None,
            settlement: None,
        }
    }

    fn spec(path: &str) -> ChangeIntentSpec {
        ChangeIntentSpec {
            repository_identity: "repo-sha256".to_owned(),
            base_revision: "abc123".to_owned(),
            claims: vec![ChangeClaim {
                path: path.to_owned(),
                operation: ChangeOperation::Modify,
                scope: ChangeScope::Committed,
            }],
        }
    }

    #[test]
    fn change_intents_are_versioned_admitted_and_fenced() {
        let run_id = RunId::new();
        let runs = HashMap::from([(run_id, pending_run(run_id))]);
        let mut delivery = VerifiedDelivery::default();
        let declared = delivery
            .decide(
                VerifiedDeliveryCommand::DeclareChangeIntent {
                    run_id,
                    expected_version: None,
                    spec: spec("src/main.rs"),
                },
                &runs,
            )
            .expect("first declaration must succeed");
        delivery
            .apply(&declared, &runs)
            .expect("declaration must apply");
        let admitted = delivery
            .decide(
                VerifiedDeliveryCommand::AdmitChangeIntent {
                    run_id,
                    expected_version: 1,
                    lease_epoch: 7,
                },
                &runs,
            )
            .expect("matching intent must admit");
        delivery
            .apply(&admitted, &runs)
            .expect("admission must apply");

        let intent = &delivery.change_intents[&run_id];
        assert!(intent.authorizes("src/main.rs", ChangeOperation::Modify, 7));
        assert!(!intent.authorizes("src/main.rs", ChangeOperation::Modify, 6));
        assert!(!intent.authorizes("src/lib.rs", ChangeOperation::Modify, 7));

        let contingent = ChangeIntent {
            claims: vec![ChangeClaim {
                path: "docs/notes.md".to_owned(),
                operation: ChangeOperation::Create,
                scope: ChangeScope::Contingent,
            }],
            ..intent.clone()
        };
        assert!(!contingent.authorizes("docs/notes.md", ChangeOperation::Create, 7));

        let mut running_runs = runs.clone();
        let running = running_runs
            .get_mut(&run_id)
            .expect("fixture run should exist");
        running.phase = RunPhase::Running;
        running.status = RunStatus::Running;
        let stale_candidate = RunCandidate {
            revision: "def456".to_owned(),
            content_sha256: "a".repeat(64),
            execution_lease_epoch: Some(6),
            realized_changes: None,
            artifact_ids: Vec::new(),
            submitted_by: running.actor.id.clone(),
        };
        assert!(matches!(
            delivery.decide(
                VerifiedDeliveryCommand::SubmitCandidate {
                    run_id,
                    candidate: stale_candidate,
                },
                &running_runs,
            ),
            Err(DomainError::CandidateLeaseMismatch {
                run_id: stale_run,
                expected: 7,
                actual: Some(6),
            }) if stale_run == run_id
        ));
    }

    #[test]
    fn scanner_v2_candidate_requires_a_complete_frozen_snapshot_identity() {
        let revision = "b".repeat(40);
        let patch_sha256 = "c".repeat(64);
        let mut candidate = RunCandidate {
            revision: revision.clone(),
            content_sha256: patch_sha256.clone(),
            execution_lease_epoch: Some(7),
            realized_changes: Some(RealizedChangeManifest {
                schema_version: 1,
                scanner_version: 2,
                repository_identity: "fixture".to_owned(),
                base_revision: "a".repeat(40),
                head_revision: "a".repeat(40),
                snapshot_revision: Some(revision.clone()),
                snapshot_tree: Some("d".repeat(40)),
                snapshot_ref: Some(format!("refs/termi9ne/candidates/mission/run/{revision}")),
                patch_sha256,
                changes: vec![RealizedChange {
                    path: "src/main.rs".to_owned(),
                    operation: ChangeOperation::Modify,
                    content_sha256: Some("e".repeat(64)),
                    git_mode: Some("100644".to_owned()),
                    git_object_id: Some("f".repeat(40)),
                }],
            }),
            artifact_ids: Vec::new(),
            submitted_by: ActorId::new("fixture-agent").expect("fixture actor should be valid"),
        };
        assert!(validate_candidate(&candidate).is_ok());

        candidate
            .realized_changes
            .as_mut()
            .expect("fixture manifest")
            .snapshot_tree = None;
        assert!(matches!(
            validate_candidate(&candidate),
            Err(DomainError::InvalidCandidate)
        ));
    }

    #[test]
    fn contingent_promotion_is_versioned_and_rotates_the_fence() {
        let run_id = RunId::new();
        let mut run = pending_run(run_id);
        let mut delivery = VerifiedDelivery::default();
        let mut intent_spec = spec("src/main.rs");
        intent_spec.claims.push(ChangeClaim {
            path: "docs/notes.md".to_owned(),
            operation: ChangeOperation::Create,
            scope: ChangeScope::Contingent,
        });
        let pending_runs = HashMap::from([(run_id, run.clone())]);
        let declared = delivery
            .decide(
                VerifiedDeliveryCommand::DeclareChangeIntent {
                    run_id,
                    expected_version: None,
                    spec: intent_spec,
                },
                &pending_runs,
            )
            .expect("intent should declare");
        delivery
            .apply(&declared, &pending_runs)
            .expect("declaration should apply");
        let admitted = delivery
            .decide(
                VerifiedDeliveryCommand::AdmitChangeIntent {
                    run_id,
                    expected_version: 1,
                    lease_epoch: 7,
                },
                &pending_runs,
            )
            .expect("intent should admit");
        delivery
            .apply(&admitted, &pending_runs)
            .expect("admission should apply");

        run.phase = RunPhase::Running;
        run.status = RunStatus::Running;
        let running_runs = HashMap::from([(run_id, run)]);
        let promoted = delivery
            .decide(
                VerifiedDeliveryCommand::PromoteContingentClaims {
                    run_id,
                    expected_version: 1,
                    lease_epoch: 8,
                    claims: vec![ChangeClaimKey {
                        path: "docs/notes.md".to_owned(),
                        operation: ChangeOperation::Create,
                    }],
                },
                &running_runs,
            )
            .expect("declared contingent claim should promote");
        delivery
            .apply(&promoted, &running_runs)
            .expect("promotion should apply");

        let intent = &delivery.change_intents[&run_id];
        assert_eq!(intent.version, 2);
        assert_eq!(intent.lease_epoch, 8);
        assert!(intent.authorizes("docs/notes.md", ChangeOperation::Create, 8));
        assert!(!intent.authorizes("docs/notes.md", ChangeOperation::Create, 7));
        assert!(intent.authorizes("src/main.rs", ChangeOperation::Modify, 8));
    }

    #[test]
    fn active_exact_file_claims_conflict_but_separate_files_do_not() {
        let first = RunId::new();
        let second = RunId::new();
        let runs = HashMap::from([(first, pending_run(first)), (second, pending_run(second))]);
        let mut delivery = VerifiedDelivery::default();
        for (run_id, path) in [(first, "src/main.rs"), (second, "src/main.rs")] {
            let event = delivery
                .decide(
                    VerifiedDeliveryCommand::DeclareChangeIntent {
                        run_id,
                        expected_version: None,
                        spec: spec(path),
                    },
                    &runs,
                )
                .expect("declaration must succeed");
            delivery
                .apply(&event, &runs)
                .expect("declaration must apply");
        }
        let first_admission = delivery
            .decide(
                VerifiedDeliveryCommand::AdmitChangeIntent {
                    run_id: first,
                    expected_version: 1,
                    lease_epoch: 1,
                },
                &runs,
            )
            .expect("first admission must succeed");
        delivery
            .apply(&first_admission, &runs)
            .expect("first admission must apply");

        assert!(matches!(
            delivery.decide(
                VerifiedDeliveryCommand::AdmitChangeIntent {
                    run_id: second,
                    expected_version: 1,
                    lease_epoch: 1,
                },
                &runs,
            ),
            Err(DomainError::ChangeIntentConflict {
                run_id,
                conflicting_run_id,
            }) if run_id == second && conflicting_run_id == first
        ));

        let isolated = ChangeIntent::proposed(second, 1, spec("src/lib.rs"));
        assert!(!delivery.change_intents[&first].conflicts_with(&isolated));
        let mut contingent_spec = spec("src/main.rs");
        contingent_spec.claims[0].scope = ChangeScope::Contingent;
        let mut contingent = ChangeIntent::proposed(second, 1, contingent_spec);
        assert!(!delivery.change_intents[&first].conflicts_with(&contingent));
        contingent.claims[0].scope = ChangeScope::Committed;
        assert!(delivery.change_intents[&first].conflicts_with(&contingent));
    }

    #[test]
    fn relative_claims_fail_closed_on_escape_and_duplicates() {
        for path in ["/etc/passwd", "../outside", "src/../../outside", ""] {
            assert!(matches!(
                validate_change_intent_spec(&spec(path)),
                Err(DomainError::InvalidChangeIntent)
                    | Err(DomainError::InvalidVerifiedDeliveryRecord)
            ));
        }
        let mut duplicate = spec("src/main.rs");
        duplicate.claims.push(duplicate.claims[0].clone());
        assert_eq!(
            validate_change_intent_spec(&duplicate),
            Err(DomainError::InvalidChangeIntent)
        );
    }
}
