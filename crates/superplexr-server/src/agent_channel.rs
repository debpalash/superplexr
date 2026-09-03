use superplexr_core::{Command, MissionId, RunId, SessionId, VerifiedDeliveryCommand};
use superplexr_protocol::Request;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AgentIdentity {
    pub mission_id: MissionId,
    pub run_id: RunId,
    pub session_id: SessionId,
    pub peer_pid: i32,
    pub process_group_id: i32,
}

#[derive(Debug, Error)]
#[error("agent channel does not authorize this request")]
pub(crate) struct AuthorizationError;

pub(crate) fn authorize(
    identity: AgentIdentity,
    request: &Request,
) -> Result<(), AuthorizationError> {
    let allowed = match request {
        Request::Ping => true,
        Request::GetMission { mission_id } | Request::MissionHistory { mission_id, .. } => {
            *mission_id == identity.mission_id
        }
        Request::PreviewConfiguredAgentRun {
            mission_id, run_id, ..
        }
        | Request::ReportProviderFact {
            mission_id, run_id, ..
        }
        | Request::ReportRunEvidence {
            mission_id, run_id, ..
        }
        | Request::ListRunEvidence { mission_id, run_id }
        | Request::GetRunActivity { mission_id, run_id } => {
            *mission_id == identity.mission_id && *run_id == identity.run_id
        }
        Request::TerminalCapture { session_id } | Request::TerminalWait { session_id, .. } => {
            *session_id == identity.session_id
        }
        Request::Dispatch {
            mission_id,
            command: Command::RaiseSignal { run_id, .. } | Command::RecordArtifact { run_id, .. },
        } => *mission_id == identity.mission_id && *run_id == identity.run_id,
        Request::Dispatch {
            mission_id,
            command:
                Command::VerifiedDelivery {
                    command:
                        VerifiedDeliveryCommand::SubmitCandidate { run_id, .. }
                        | VerifiedDeliveryCommand::RecordHandoff {
                            handoff: superplexr_core::HandoffArtifact { run_id, .. },
                        },
                },
        } => *mission_id == identity.mission_id && *run_id == identity.run_id,
        Request::Dispatch {
            mission_id,
            command:
                Command::VerifiedDelivery {
                    command:
                        VerifiedDeliveryCommand::RecordEvaluationReceipt { receipt },
                },
        } => {
            *mission_id == identity.mission_id && receipt.verifier_run_id == identity.run_id
        }
        _ => false,
    };
    allowed.then_some(()).ok_or(AuthorizationError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use superplexr_core::{
        Actor, ArtifactId, ChangeIntentSpec, EvaluationCheck, EvaluationReceipt,
        EvaluationVerdict, HandoffArtifact, RunCandidate, SignalId, SignalKind,
    };

    fn identity() -> AgentIdentity {
        AgentIdentity {
            mission_id: MissionId::new(),
            run_id: RunId::new(),
            session_id: SessionId::new(),
            peer_pid: 1001,
            process_group_id: 1000,
        }
    }

    #[test]
    fn permits_only_run_scoped_agent_operations() {
        let identity = identity();
        assert!(authorize(identity, &Request::Ping).is_ok());
        assert!(
            authorize(
                identity,
                &Request::GetMission {
                    mission_id: identity.mission_id
                }
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::VerifiedDelivery {
                        command: VerifiedDeliveryCommand::SubmitCandidate {
                            run_id: identity.run_id,
                            candidate: RunCandidate {
                                revision: "abc123".to_owned(),
                                content_sha256: "a".repeat(64),
                                execution_lease_epoch: None,
                                realized_changes: None,
                                artifact_ids: Vec::new(),
                                submitted_by: superplexr_core::ActorId::new("worker")
                                    .expect("actor should be valid"),
                            },
                        },
                    },
                },
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::VerifiedDelivery {
                        command: VerifiedDeliveryCommand::RecordHandoff {
                            handoff: HandoffArtifact {
                                artifact_id: ArtifactId::new(),
                                run_id: identity.run_id,
                                summary: "ready".to_owned(),
                                completed: Vec::new(),
                                remaining: Vec::new(),
                                evidence: Vec::new(),
                                external_effects: Vec::new(),
                            },
                        },
                    },
                },
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::VerifiedDelivery {
                        command: VerifiedDeliveryCommand::RecordEvaluationReceipt {
                            receipt: EvaluationReceipt {
                                artifact_id: ArtifactId::new(),
                                subject_run_id: RunId::new(),
                                verifier_run_id: identity.run_id,
                                candidate_sha256: "b".repeat(64),
                                verdict: EvaluationVerdict::Passed,
                                checks: vec![EvaluationCheck {
                                    name: "test".to_owned(),
                                    passed: true,
                                    evidence: Vec::new(),
                                }],
                                delivery_validated: true,
                                repeatable: true,
                                summary: "passed".to_owned(),
                            },
                        },
                    },
                },
            )
            .is_ok()
        );
        let evidence = superplexr_protocol::RunEvidenceInput {
            provider_id: "github".to_owned(),
            adapter_version: "1".to_owned(),
            evidence_key: "check/test".to_owned(),
            revision: "abc123".to_owned(),
            summary: "tests passed".to_owned(),
            url: None,
            kind: superplexr_protocol::RunEvidenceKind::Check {
                name: "test".to_owned(),
                state: superplexr_protocol::CheckEvidenceState::Passed,
            },
        };
        assert!(
            authorize(
                identity,
                &Request::ReportRunEvidence {
                    mission_id: identity.mission_id,
                    run_id: identity.run_id,
                    evidence,
                },
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::ListRunEvidence {
                    mission_id: identity.mission_id,
                    run_id: identity.run_id,
                },
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::ReportProviderFact {
                    mission_id: identity.mission_id,
                    run_id: identity.run_id,
                    fact: superplexr_protocol::ProviderFactInput {
                        provider_id: "codex".to_owned(),
                        adapter_version: "1".to_owned(),
                        state: superplexr_protocol::ProviderActivityState::Working,
                        summary: "running a tool".to_owned(),
                        valid_for_seconds: 30,
                    },
                },
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::GetRunActivity {
                    mission_id: identity.mission_id,
                    run_id: identity.run_id,
                },
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::TerminalCapture {
                    session_id: identity.session_id,
                },
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::TerminalWait {
                    session_id: identity.session_id,
                    condition: superplexr_protocol::TerminalWaitCondition::Exit,
                    timeout_millis: 1_000,
                },
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::RaiseSignal {
                        signal_id: SignalId::new(),
                        run_id: identity.run_id,
                        kind: SignalKind::InputNeeded {
                            question: "Need input".to_owned(),
                        },
                    },
                }
            )
            .is_ok()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::RecordArtifact {
                        artifact_id: ArtifactId::new(),
                        run_id: identity.run_id,
                        name: "report".to_owned(),
                        media_type: "text/plain".to_owned(),
                        locator: "artifact://report".to_owned(),
                        digest: None,
                    },
                }
            )
            .is_ok()
        );
    }

    #[test]
    fn denies_cross_run_and_privileged_operations() {
        let identity = identity();
        assert!(authorize(identity, &Request::ListMissions).is_err());
        assert!(
            authorize(
                identity,
                &Request::TerminalCapture {
                    session_id: SessionId::new(),
                },
            )
            .is_err()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::VerifiedDelivery {
                        command: VerifiedDeliveryCommand::DeclareChangeIntent {
                            run_id: identity.run_id,
                            expected_version: None,
                            spec: ChangeIntentSpec {
                                repository_identity: "repo".to_owned(),
                                base_revision: "abc123".to_owned(),
                                claims: Vec::new(),
                            },
                        },
                    },
                },
            )
            .is_err()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::VerifiedDelivery {
                        command: VerifiedDeliveryCommand::RecordEvaluationReceipt {
                            receipt: EvaluationReceipt {
                                artifact_id: ArtifactId::new(),
                                subject_run_id: identity.run_id,
                                verifier_run_id: RunId::new(),
                                candidate_sha256: "b".repeat(64),
                                verdict: EvaluationVerdict::Passed,
                                checks: Vec::new(),
                                delivery_validated: true,
                                repeatable: true,
                                summary: "forged".to_owned(),
                            },
                        },
                    },
                },
            )
            .is_err()
        );
        assert!(
            authorize(
                identity,
                &Request::ListRunEvidence {
                    mission_id: identity.mission_id,
                    run_id: RunId::new(),
                },
            )
            .is_err()
        );
        assert!(
            authorize(
                identity,
                &Request::GetRunActivity {
                    mission_id: identity.mission_id,
                    run_id: RunId::new(),
                },
            )
            .is_err()
        );
        assert!(
            authorize(
                identity,
                &Request::GetMission {
                    mission_id: MissionId::new()
                }
            )
            .is_err()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::StartRun {
                        run_id: RunId::new(),
                        parent: Some(identity.run_id),
                        actor: Actor::agent("child", "driver").expect("test actor should be valid"),
                        objective: "unauthorized delegation".to_owned(),
                    },
                }
            )
            .is_err()
        );
        assert!(
            authorize(
                identity,
                &Request::Dispatch {
                    mission_id: identity.mission_id,
                    command: Command::RecordArtifact {
                        artifact_id: ArtifactId::new(),
                        run_id: RunId::new(),
                        name: "foreign".to_owned(),
                        media_type: "text/plain".to_owned(),
                        locator: "artifact://foreign".to_owned(),
                        digest: None,
                    },
                }
            )
            .is_err()
        );
    }
}
