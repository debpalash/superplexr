# Compact native verification status

Status: source implemented, unbuilt and untested. No tests, builds, runtime
requests, app runs, or resource measurements were performed, per owner direction.

The status projection now lives in `superplexr-core` as the pure
`Mission::verification_status` interface. CLI/shared verification inspection
uses a new native `VerificationStatus` request rather than receiving a complete
Mission and projecting locally. The store projects from its borrowed Mission:
it does not clone the Mission, receipt evidence arrays, or narrative summaries.

The result retains the existing bounded shape: Candidate identity, execution
state, primary Session state, subject disposition, aggregate matching receipt
counts and at most 16 stable receipt summaries. It still distinguishes recorded
verification from successful process exit and owner acceptance. No evidence is
reread, collected, executed, or accepted.

## Compatibility and authority

The handshake advertises `verification_status_v1`; the protocol version remains
unchanged. The native client checks that capability on the exact connection it
uses for the read. An older runtime produces an explicit unsupported-capability
error without sending the unknown request or falling back to a full Mission.
The response must identify the requested Mission/verifier and respect the
receipt-count and Candidate-identity bounds. The call does not replay a request
after an uncertain response.

Owner access is unchanged. Shared access requires the same Mission scope as
`GetMission`; a Session-only Share does not acquire Mission-wide inspection.
Agent-channel access remains limited to the agent's existing Mission, which it
could already read. Existing authenticated request/revocation handling applies;
there is no alternate listener, credential fallback, or new mutation authority.

## Remaining limits

Receipt totals still scan the stored Mission's receipts under its store lock.
This removes full-Mission transfer and cloning, not all work proportional to
Mission size or any claim of constant-time latency. Native decoding and existing
wire limits remain separate costs. No cross-platform, footprint or performance
target has been measured for this change.

Deferred acceptance includes old/new capability negotiation, exact-wire use,
Share and agent scope, revocation, wrong response identities, absent/non-verifier
Runs, large Missions, stable bounded receipts, and unchanged execution/evidence/
acceptance semantics. Earlier full-Mission status behavior does not certify this
new request path.
