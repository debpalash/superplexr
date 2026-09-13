//! Bounded presentation of one verifier from one authoritative Mission read.
use crate::{
    ArtifactId, DeliveryRunPurpose, DomainError, EvaluationVerdict, FinishOutcome, Mission,
    MissionId, RunDisposition, RunId, RunPhase, SessionId, SessionStatus,
};
use serde::{Deserialize, Serialize};

const MAX_RECEIPTS: usize = 16;
pub const MAX_VERIFIER_PAGE: u16 = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerifierSummary {
    pub verifier_run_id: RunId,
    pub subject_run_id: RunId,
    pub phase: RunPhase,
    pub outcome: Option<FinishOutcome>,
    pub subject_disposition: RunDisposition,
    pub primary_session_id: Option<SessionId>,
}

/// One bounded live page, ordered by Run ID. Version is an observation, not a
/// retained multi-page snapshot; mutations between pages require a fresh read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerificationCatalog {
    pub mission_id: MissionId,
    pub mission_version: u64,
    pub after: Option<RunId>,
    pub limit: u16,
    pub entries: Vec<VerifierSummary>,
    pub next_after: Option<RunId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationNextAction {
    ReviewBeforeLaunch,
    ObserveExecution,
    InspectPause,
    InspectEvidenceBeforeCollection,
    InspectRecordedReceipts,
    InspectFailedExecution,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReceiptStatus {
    pub artifact_id: ArtifactId,
    pub verdict: EvaluationVerdict,
    pub checks: usize,
    pub passing_checks: usize,
    pub delivery_validated: bool,
    pub repeatable: bool,
    /// Applies the domain's receipt predicate, not a fresh evidence recheck.
    pub passes_recorded_candidate: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VerificationStatus {
    pub mission_id: MissionId,
    pub mission_version: u64,
    pub verifier_run_id: RunId,
    pub subject_run_id: RunId,
    pub candidate_revision: String,
    pub candidate_sha256: String,
    pub phase: RunPhase,
    pub outcome: Option<FinishOutcome>,
    pub execution_finished: bool,
    pub primary_session_id: Option<SessionId>,
    pub primary_session_status: Option<SessionStatus>,
    /// The subject's current owner disposition, separate from verifier exit.
    pub subject_disposition: RunDisposition,
    pub receipt_count: usize,
    pub passing_receipt_count: usize,
    pub receipts: Vec<ReceiptStatus>,
    pub receipts_truncated: bool,
    pub next_action: VerificationNextAction,
    pub evidence_rechecked: bool,
    pub observation_only: bool,
}

impl Mission {
    /// Scan frozen inputs with O(limit) retained IDs, without cloning the
    /// Mission, inspecting evidence, or computing every verifier's receipts.
    pub fn verification_catalog(&self, after: Option<RunId>, limit: u16) -> VerificationCatalog {
        let limit = limit.clamp(1, MAX_VERIFIER_PAGE);
        let capacity = usize::from(limit);
        let mut ids = Vec::with_capacity(capacity + 1);
        for (id, input) in &self.verified_delivery.delivery_run_inputs {
            if input.purpose != DeliveryRunPurpose::Verification
                || input.run_id != *id
                || after.is_some_and(|after| *id <= after)
                || !self.runs.contains_key(id)
                || !self.runs.contains_key(&input.source_run_id)
            {
                continue;
            }
            let position = ids.binary_search(id).unwrap_or_else(|position| position);
            if position <= capacity {
                ids.insert(position, *id);
                if ids.len() > capacity + 1 {
                    ids.pop();
                }
            }
        }
        let has_more = ids.len() > capacity;
        ids.truncate(capacity);
        let next_after = if has_more { ids.last().copied() } else { None };
        let entries = ids
            .into_iter()
            .filter_map(|id| {
                let input = self.verified_delivery.delivery_run_inputs.get(&id)?;
                let run = self.runs.get(&id)?;
                let subject = self.runs.get(&input.source_run_id)?;
                Some(VerifierSummary {
                    verifier_run_id: id,
                    subject_run_id: input.source_run_id,
                    phase: run.phase,
                    outcome: run.outcome,
                    subject_disposition: subject.disposition,
                    primary_session_id: run.primary_session,
                })
            })
            .collect();
        VerificationCatalog {
            mission_id: self.id,
            mission_version: self.version,
            after,
            limit,
            entries,
            next_after,
        }
    }

    /// Pure, bounded-output status projection. Does not clone the Mission or
    /// inspect filesystem evidence; receipt predicates use recorded facts only.
    pub fn verification_status(&self, verifier: RunId) -> Result<VerificationStatus, DomainError> {
        project(self, verifier)
    }
}

fn project(mission: &Mission, verifier: RunId) -> Result<VerificationStatus, DomainError> {
    let run = mission
        .runs
        .get(&verifier)
        .ok_or(DomainError::RunNotFound(verifier))?;
    let input = mission
        .verified_delivery
        .delivery_run_inputs
        .get(&verifier)
        .filter(|input| {
            input.purpose == DeliveryRunPurpose::Verification && input.run_id == verifier
        })
        .ok_or(DomainError::InvalidDeliveryRunInput)?;
    let subject = mission
        .runs
        .get(&input.source_run_id)
        .ok_or(DomainError::RunNotFound(input.source_run_id))?;
    if input.candidate.revision.len() > 256
        || input.candidate.content_sha256.len() != 64
        || !input
            .candidate
            .content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DomainError::InvalidCandidate);
    }
    let mut receipts: Vec<ReceiptStatus> = Vec::new();
    let mut receipt_count = 0usize;
    let mut passing_receipt_count = 0usize;
    for receipt in mission
        .verified_delivery
        .evaluation_receipts
        .values()
        .filter(|receipt| {
            receipt.verifier_run_id == verifier
                && receipt.subject_run_id == input.source_run_id
                && receipt.candidate_sha256 == input.candidate.content_sha256
        })
    {
        receipt_count += 1;
        let passes = receipt.passes(&input.candidate);
        passing_receipt_count += usize::from(passes);
        // Keep only the lowest 16 IDs, in stable order, without cloning an
        // unbounded receipt collection or its evidence arrays/summaries.
        let key = receipt.artifact_id.to_string();
        let position = receipts
            .binary_search_by(|item| item.artifact_id.to_string().cmp(&key))
            .unwrap_or_else(|position| position);
        if position < MAX_RECEIPTS {
            receipts.insert(
                position,
                ReceiptStatus {
                    artifact_id: receipt.artifact_id,
                    verdict: receipt.verdict,
                    checks: receipt.checks.len(),
                    passing_checks: receipt.checks.iter().filter(|check| check.passed).count(),
                    delivery_validated: receipt.delivery_validated,
                    repeatable: receipt.repeatable,
                    passes_recorded_candidate: passes,
                },
            );
            if receipts.len() > MAX_RECEIPTS {
                receipts.pop();
            }
        }
    }
    let next_action = match run.phase {
        RunPhase::Pending => VerificationNextAction::ReviewBeforeLaunch,
        RunPhase::Running => VerificationNextAction::ObserveExecution,
        RunPhase::Paused => VerificationNextAction::InspectPause,
        RunPhase::Finished if receipt_count > 0 => VerificationNextAction::InspectRecordedReceipts,
        RunPhase::Finished if run.outcome == Some(FinishOutcome::Succeeded) => {
            VerificationNextAction::InspectEvidenceBeforeCollection
        }
        RunPhase::Finished => VerificationNextAction::InspectFailedExecution,
    };
    Ok(VerificationStatus {
        mission_id: mission.id,
        mission_version: mission.version,
        verifier_run_id: verifier,
        subject_run_id: input.source_run_id,
        candidate_revision: input.candidate.revision.clone(),
        candidate_sha256: input.candidate.content_sha256.clone(),
        phase: run.phase,
        outcome: run.outcome,
        execution_finished: run.phase == RunPhase::Finished,
        primary_session_id: run.primary_session,
        primary_session_status: run
            .primary_session
            .and_then(|id| mission.sessions.get(&id))
            .map(|session| session.status),
        subject_disposition: subject.disposition,
        receipt_count,
        passing_receipt_count,
        receipts_truncated: receipt_count > receipts.len(),
        receipts,
        next_action,
        evidence_rechecked: false,
        observation_only: true,
    })
}
