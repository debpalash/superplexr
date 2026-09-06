use super::*;
use crate::navigator::Navigator;
use ultraplexr_core::{
    FinishOutcome, RunDisposition, RunPhase, VerificationCatalog, VerifierSummary,
};

#[test]
fn discovery_opens_exact_verifier_and_pages_even_when_next_row_is_filtered() {
    let mission = MissionId::new();
    let verifier = RunId::new();
    let subject = RunId::new();
    let page = CatalogPage {
        mission,
        after: None,
    };
    let mut nav = Navigator::default();
    nav.open_catalog(page);
    nav.apply(catalog_entries(VerificationCatalog {
        mission_id: mission,
        mission_version: 27,
        after: None,
        limit: 1,
        entries: vec![VerifierSummary {
            verifier_run_id: verifier,
            subject_run_id: subject,
            phase: RunPhase::Finished,
            outcome: Some(FinishOutcome::Succeeded),
            subject_disposition: RunDisposition::AwaitingReview,
            primary_session_id: None,
        }],
        next_after: Some(verifier),
    }));
    nav.query = subject.to_string();
    assert_eq!(nav.visible().len(), 1);
    assert_eq!(
        nav.next_verifier_page(),
        Some(CatalogPage {
            mission,
            after: Some(verifier)
        })
    );
    let Target::Verifier(inspection) = nav.current().expect("verifier row").target else {
        panic!("inspection target")
    };
    assert_eq!(inspection, Inspection { mission, verifier });
    assert!(nav.entries[0].detail.contains("owner AwaitingReview"));
    assert!(
        nav.entries
            .last()
            .expect("observation")
            .detail
            .contains("version 27")
    );
    nav.open_verifier(inspection);
    assert!(nav.workflow_active());
    assert_eq!(
        nav.catalog,
        Some(page),
        "status retains the current page for Backspace"
    );
    assert!(nav.query.is_empty());
    assert!(
        nav.entries.is_empty(),
        "new scope drops stale actions before worker admission"
    );
}

#[test]
fn empty_catalog_has_no_action_or_fabricated_next_page() {
    let entries = catalog_entries(VerificationCatalog {
        mission_id: MissionId::new(),
        mission_version: 1,
        after: Some(RunId::new()),
        limit: 64,
        entries: vec![],
        next_after: None,
    });
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].target, Target::Unavailable);
    assert!(entries[0].label.contains("No verifiers"));
}

#[test]
fn full_catalog_page_stays_bounded_and_exposes_only_read_targets() {
    let mission = MissionId::new();
    let summaries: Vec<_> = (0..ultraplexr_core::MAX_VERIFIER_PAGE)
        .map(|_| VerifierSummary {
            verifier_run_id: RunId::new(),
            subject_run_id: RunId::new(),
            phase: RunPhase::Pending,
            outcome: None,
            subject_disposition: RunDisposition::AwaitingReview,
            primary_session_id: None,
        })
        .collect();
    let next = summaries.last().expect("full page").verifier_run_id;
    let rows = catalog_entries(VerificationCatalog {
        mission_id: mission,
        mission_version: 9,
        after: None,
        limit: ultraplexr_core::MAX_VERIFIER_PAGE,
        entries: summaries,
        next_after: Some(next),
    });
    assert_eq!(
        rows.len(),
        usize::from(ultraplexr_core::MAX_VERIFIER_PAGE) + 2
    );
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row.target, Target::Verifier(_)))
            .count(),
        64
    );
    assert!(rows.iter().all(|row| matches!(
        row.target,
        Target::Verifier(_) | Target::VerifierPage(_) | Target::Unavailable
    )));
}

#[test]
fn manual_uuid_draft_rejects_whole_oversized_and_invalid_pastes() {
    let mission = MissionId::new();
    let verifier = RunId::new();
    let mut draft = Draft::new(mission);
    draft.append(&format!("{verifier}0"));
    assert!(draft.value.is_empty());
    assert!(draft.inspection().is_err());
    draft.clear();
    draft.append(&verifier.to_string());
    draft.append("\n");
    assert!(draft.inspection().is_err());
    draft.clear();
    draft.append(&verifier.to_string());
    assert_eq!(
        draft.inspection().expect("manual inspection"),
        Inspection { mission, verifier }
    );
}
