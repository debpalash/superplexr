//! Read-only native status; deliberately no whole-Mission fallback.
use super::*;

pub fn inspect(
    client: &ControlClient,
    mission: MissionId,
    verifier: RunId,
) -> Result<VerificationStatus> {
    Ok(client.verification_status(mission, verifier)?)
}
