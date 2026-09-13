//! Compact discovery/status through real runtime authorization and persistence.
use super::*;
use superplexr_core::{RunPhase, VerificationNextAction};
use superplexr_protocol::ShareRole;

#[test]
fn compact_verifiers_page_without_mutating_and_keep_execution_separate_from_acceptance() {
    let mut fixture = Fixture::new();
    let owner = fixture.client();
    let mission = MissionId::new();
    owner
        .create_mission(
            mission,
            "Compact verification",
            Actor::human("owner").unwrap(),
        )
        .unwrap();
    assert!(
        owner
            .verification_catalog(mission, None, 32)
            .unwrap()
            .entries
            .is_empty()
    );
    let (producer, checkout) = prepare_producer(&fixture, &owner, mission);
    let (candidate, _, _) = publish(&owner, mission, producer, &checkout.worktree_path);
    let receipt = examine(&fixture, &owner, mission, producer, &candidate);
    let verifier = receipt.verifier_run_id;
    let status = owner.verification_status(mission, verifier).unwrap();
    assert_eq!(status.phase, RunPhase::Finished);
    assert_eq!(status.outcome, Some(FinishOutcome::Succeeded));
    assert!(status.execution_finished);
    assert_eq!(status.receipt_count, 0);
    assert_eq!(
        status.next_action,
        VerificationNextAction::InspectEvidenceBeforeCollection
    );
    assert_ne!(status.subject_disposition, RunDisposition::Accepted);
    assert!(status.observation_only);
    assert!(!status.evidence_rechecked);
    assert_eq!(status.candidate_sha256, candidate.content_sha256);
    assert!(owner.verification_status(mission, producer).is_err());

    commit(
        &owner,
        mission,
        verified(VerifiedDeliveryCommand::RecordEvaluationReceipt {
            receipt: receipt.clone(),
        }),
    );
    let mut expected = vec![verifier];
    for index in 0..64 {
        let id = RunId::new();
        commit(
            &owner,
            mission,
            Command::CreateVerifierRun {
                subject_run_id: producer,
                verifier_run_id: id,
                actor: Actor::agent(format!("pending-{index}"), "verifier").unwrap(),
                priority: RunPriority::Normal,
            },
        );
        expected.push(id);
        assert_eq!(
            owner.verification_status(mission, id).unwrap().phase,
            RunPhase::Pending
        );
    }
    expected.sort();
    let before = owner.get_mission(mission).unwrap();
    let mut after = None;
    let mut found = Vec::new();
    for index in 0..3 {
        let page = owner.verification_catalog(mission, after, 32).unwrap();
        assert_eq!(page.mission_version, before.version);
        assert_eq!(page.after, after);
        assert_eq!(page.entries.len(), if index < 2 { 32 } else { 1 });
        assert!(
            page.entries
                .iter()
                .all(|entry| entry.subject_run_id == producer)
        );
        found.extend(page.entries.iter().map(|entry| entry.verifier_run_id));
        after = page.next_after;
        assert_eq!(after.is_some(), index < 2);
    }
    assert_eq!(found, expected);
    let bounded = owner.verification_catalog(mission, None, u16::MAX).unwrap();
    assert_eq!(bounded.limit, 64);
    assert_eq!(bounded.entries.len(), 64);
    assert_eq!(bounded.next_after, Some(expected[63]));
    let status = owner.verification_status(mission, verifier).unwrap();
    assert_eq!(status.receipt_count, 1);
    assert_eq!(
        status.passing_receipt_count, 0,
        "fixture checks failed despite successful execution"
    );
    assert_eq!(status.receipts[0].verdict, EvaluationVerdict::Failed);
    assert!(!status.receipts[0].passes_recorded_candidate);
    assert_eq!(
        status.next_action,
        VerificationNextAction::InspectRecordedReceipts
    );
    assert_eq!(
        owner.get_mission(mission).unwrap(),
        before,
        "all inspection is non-mutating"
    );

    let unrelated = MissionId::new();
    owner
        .create_mission(unrelated, "Other Mission", Actor::human("owner").unwrap())
        .unwrap();
    let (share, token) = owner
        .create_share(
            "Inspect only",
            ShareRole::Observer,
            vec![mission],
            vec![],
            300,
        )
        .unwrap();
    let observer = ControlClient::connect_with_share(fixture.root.join("s"), token).unwrap();
    assert_eq!(
        observer
            .verification_catalog(mission, None, 32)
            .unwrap()
            .entries
            .len(),
        32
    );
    assert_eq!(
        observer.verification_status(mission, verifier).unwrap(),
        status
    );
    assert!(observer.verification_catalog(unrelated, None, 32).is_err());
    assert!(observer.verification_status(unrelated, verifier).is_err());
    owner.revoke_share(share.share_id).unwrap();
    assert!(observer.verification_catalog(mission, None, 32).is_err());
    drop(observer);
    drop(owner);
    fixture.restart();
    let recovered = fixture.client();
    assert_eq!(
        recovered.verification_status(mission, verifier).unwrap(),
        status
    );
    let page = recovered.verification_catalog(mission, None, 64).unwrap();
    let mut recovered_ids: Vec<_> = page
        .entries
        .into_iter()
        .map(|entry| entry.verifier_run_id)
        .collect();
    recovered_ids.extend(
        recovered
            .verification_catalog(mission, page.next_after, 64)
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.verifier_run_id),
    );
    assert_eq!(recovered_ids, expected);
}
