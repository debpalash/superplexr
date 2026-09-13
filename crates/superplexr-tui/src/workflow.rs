//! Optional read-only verifier inspection. No execution or settlement actions.
use superplexr_client::{ClientError, ControlClient};
use superplexr_core::{MissionId, RunId, VerificationNextAction};

use crate::navigator::{Entry, Target};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inspection {
    pub mission: MissionId,
    pub verifier: RunId,
}

/// A single live page. Keeping only its cursor bounds navigation memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalogPage {
    pub mission: MissionId,
    pub after: Option<RunId>,
}

pub(crate) fn load_catalog(
    client: &ControlClient,
    page: CatalogPage,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Entry>>, ClientError> {
    if cancelled() {
        return Ok(None);
    }
    let catalog = client.verification_catalog(
        page.mission,
        page.after,
        superplexr_core::MAX_VERIFIER_PAGE,
    )?;
    if cancelled() {
        return Ok(None);
    }
    Ok(Some(catalog_entries(catalog)))
}

fn catalog_entries(catalog: superplexr_core::VerificationCatalog) -> Vec<Entry> {
    let mut entries = Vec::with_capacity(catalog.entries.len() + 2);
    for verifier in catalog.entries {
        entries.push(Entry {
            priority: 0,
            key: verifier.verifier_run_id.to_string(),
            label: format!(
                "Verifier {} | {:?} | {:?}",
                verifier.verifier_run_id, verifier.phase, verifier.outcome
            ),
            detail: format!(
                "Subject {} | owner {:?} | Session {} | Enter recorded status",
                verifier.subject_run_id,
                verifier.subject_disposition,
                verifier
                    .primary_session_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "None".into())
            ),
            target: Target::Verifier(Inspection {
                mission: catalog.mission_id,
                verifier: verifier.verifier_run_id,
            }),
        });
    }
    if let Some(after) = catalog.next_after {
        entries.push(Entry {
            priority: 0,
            key: "next-page".into(),
            label: "Next verifier page →".into(),
            detail: "Enter or n reads the next bounded page; 0 returns to the first page.".into(),
            target: Target::VerifierPage(CatalogPage {
                mission: catalog.mission_id,
                after: Some(after),
            }),
        });
    }
    entries.push(Entry {
        priority: 0,
        key: "catalog-observation".into(),
        label: if entries.is_empty() {
            "No verifiers on this page".into()
        } else {
            "Recorded verifier discovery; evidence NOT rechecked".into()
        },
        detail: format!(
            "Mission {} | version {} | pages are live reads, not a shared snapshot | W enter UUID",
            catalog.mission_id, catalog.mission_version
        ),
        target: Target::Unavailable,
    });
    entries
}

pub struct Draft {
    pub mission: MissionId,
    pub value: String,
    invalid: bool,
}

impl Draft {
    pub fn new(mission: MissionId) -> Self {
        Self {
            mission,
            value: String::new(),
            invalid: false,
        }
    }

    pub fn append(&mut self, text: &str) {
        // Reject the whole insertion instead of silently turning truncated or
        // filtered pasted text into a different valid identifier.
        if text.len() > 36usize.saturating_sub(self.value.len())
            || !text
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            self.invalid = true;
            return;
        }
        self.value.push_str(text);
    }

    pub fn backspace(&mut self) {
        self.value.pop();
        self.invalid = false;
    }
    pub fn clear(&mut self) {
        self.value.clear();
        self.invalid = false;
    }

    pub fn inspection(&self) -> Result<Inspection, &'static str> {
        if self.invalid {
            return Err(
                "Invalid or oversized insertion; edit or clear the Run ID before submitting",
            );
        }
        if self.value.len() != 36 {
            return Err("Enter the full hyphenated verifier Run UUID (36 characters)");
        }
        let verifier = self
            .value
            .parse::<RunId>()
            .map_err(|_| "Enter a valid verifier Run UUID")?;
        Ok(Inspection {
            mission: self.mission,
            verifier,
        })
    }
}

pub(crate) fn load(
    client: &ControlClient,
    inspection: Inspection,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Entry>>, ClientError> {
    if cancelled() {
        return Ok(None);
    }
    let status = client.verification_status(inspection.mission, inspection.verifier)?;
    if cancelled() {
        return Ok(None);
    }
    let mut entries = Vec::with_capacity(12 + status.receipts.len());
    let mut row = |key: &str, label: String, detail: String| {
        entries.push(Entry {
            priority: 0,
            key: key.into(),
            label,
            detail,
            target: Target::Unavailable,
        });
    };
    row(
        "warning",
        "Read-only recorded status; evidence NOT rechecked".into(),
        "A successful process exit is not verification approval or owner acceptance.".into(),
    );
    row(
        "mission",
        format!("Mission {}", status.mission_id),
        format!(
            "Recorded Mission version {} | r explicitly refreshes this snapshot",
            status.mission_version
        ),
    );
    row(
        "verifier",
        format!("Verifier {}", status.verifier_run_id),
        format!(
            "Execution {:?} | outcome {:?}",
            status.phase, status.outcome
        ),
    );
    row(
        "subject",
        format!("Subject {}", status.subject_run_id),
        format!("Owner disposition {:?}", status.subject_disposition),
    );
    row(
        "revision",
        "Frozen candidate revision".into(),
        status.candidate_revision,
    );
    row(
        "digest",
        "Frozen candidate SHA-256".into(),
        status.candidate_sha256,
    );
    row(
        "session",
        "Primary Session (no attach or Control action)".into(),
        format!(
            "{} | {:?}",
            status
                .primary_session_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "None".into()),
            status.primary_session_status
        ),
    );
    row(
        "receipts",
        format!(
            "{} recorded receipts; {} pass recorded candidate predicates",
            status.receipt_count, status.passing_receipt_count
        ),
        if status.receipts_truncated {
            "Showing only the first 16 by Artifact ID; counts cover all matching receipts."
        } else {
            "These are recorded receipt predicates, not a fresh filesystem or tool check."
        }
        .into(),
    );
    let next = match status.next_action {
        VerificationNextAction::ReviewBeforeLaunch => {
            "Review the frozen plan and candidate before any explicit launch."
        }
        VerificationNextAction::ObserveExecution => "Observe execution; no result is implied yet.",
        VerificationNextAction::InspectPause => {
            "Inspect why execution paused; resumption requires a separate explicit action."
        }
        VerificationNextAction::InspectEvidenceBeforeCollection => {
            "Inspect retained evidence before separately collecting a receipt."
        }
        VerificationNextAction::InspectRecordedReceipts => {
            "Inspect recorded receipts and evidence before a separate owner decision."
        }
        VerificationNextAction::InspectFailedExecution => {
            "Inspect failed execution; this view never retries it."
        }
    };
    row(
        "next",
        "Suggested review step (text only)".into(),
        next.into(),
    );
    for receipt in status.receipts {
        row(
            &receipt.artifact_id.to_string(),
            format!("Receipt {} | {:?}", receipt.artifact_id, receipt.verdict),
            format!(
                "Checks {}/{} | delivery {} | repeatable {} | recorded pass {}",
                receipt.passing_checks,
                receipt.checks,
                receipt.delivery_validated,
                receipt.repeatable,
                receipt.passes_recorded_candidate
            ),
        );
    }
    Ok(Some(entries))
}

#[cfg(test)]
#[path = "workflow_tests.rs"]
mod tests;
