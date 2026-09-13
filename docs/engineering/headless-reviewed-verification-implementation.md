# Headless review and launch of verification plans

Status: source implemented, unbuilt and untested. No verification checks,
tests, builds, CLI/app runs, runtime mutations, or platform measurements were
executed while implementing this command surface, per owner direction.

Two CLI commands now expose the same `superplexr-verification::prepare` and
`launch` workflow used by the desktop. They do not add another runner, engine
configuration file, daemon, scheduler, or receipt format.

## Review without execution

`verification-prepare --plan PATH --evidence-root PATH [--runner PATH]` reads and
validates the explicit private plan and local paths. It prints JSON containing
canonical setup paths, the plan SHA-256, every check command and its limits,
and a trust warning. The runner defaults to the current CLI executable.

This command opens no runtime connection, creates no Run or checkout, writes
no plan/evidence, and executes no check. The returned digest is not itself owner
approval: the operator must review the described commands, paths, and limits.

## Explicit reviewed launch

`verification-launch MISSION --subject SOURCE_RUN --plan PATH --evidence-root
PATH --plan-sha256 REVIEWED_SHA256 [--runner PATH]` prepares the plan again and
compares its digest before connecting to the owner runtime. The shared launch
module rechecks the plan before creating the verifier; the launched worker
receives the same digest for its own pre-execution check.

The result is JSON with Mission/verifier identities, whether launch was
acknowledged, any launch error, and the next action. Acknowledged launch is not
successful verification. `verification_passed` remains unknown, and acceptance
is never requested. Existing collection and owner acceptance remain separate.

To resume preparation of a known pending verifier, replace `--subject` with
`--verifier VERIFIER_RUN`. Exactly one selector is required. The shared module
refuses to relaunch a non-pending verifier and does not create a replacement
when the existing-verifier selector is used.

If setup/launch fails after creation was acknowledged, the result retains the
verifier ID and the CLI exits unsuccessfully. Inspect it before resuming. The
CLI now reserves that ID before its launch worker starts, so lost creation ACKs
and worker errors also report the attempted ID. `run_existence: unconfirmed`
does not mean the Run exists or that the operation had no effect. There is no
automatic action replay or exactly-once claim.
HTTP/MCP/TUI launch controls are not introduced by these CLI commands.

For automation that must recover after the whole CLI process is killed, use
`--new-verifier-id UUID` with `--subject` and retain that UUID before invocation.
The shared `NewVerifier` target uses the supplied identity for creation. If it
already exists, the command refuses rather than implicitly adopting or
relaunching it. Inspect the Mission and explicitly use `--verifier` only if the
intended verifier exists and is still pending. Concurrent creation is still
subject to the runtime's version and identity validation. Repeating a creation
command is not equivalent to resuming launch preparation.

The desktop also reserves an ID before its confirmation prompt and includes it
in unconfirmed-error messages. This does not add durable desktop attempt storage
or prove recovery after the desktop process itself is killed. Both extensions
remain source-only, unbuilt and untested.

## Boundaries and deferred acceptance

### Read-only workflow status

The subsequent source-only `verification-status MISSION VERIFIER_RUN` command
now uses a [compact native status request](compact-verification-status-implementation.md).
The projection is shared with the runtime through the core module. It reports the
frozen Candidate identity, verifier phase/outcome, primary Session identity and
status, matching receipt counts, and the subject's current owner disposition.
Missing/non-verifier Runs are errors rather than fabricated empty status.

Execution exit, recorded evaluation, and owner disposition are independent
fields. A successful exit with no receipt recommends inspecting evidence before
explicit collection; it is not presented as passing checks. Matching receipt
counts bind verifier, subject, and exact Candidate digest. At most 16 receipt
summaries are retained in stable Artifact-ID order, with a truncation flag and
counts across all matching receipts. The domain's receipt predicate is reported
as `passes_recorded_candidate`; this does not reread logs or attest artifact
contents, and `evidence_rechecked` is false. Repeatability remains a separate
recorded field, not an inference from a successful process exit.

This status command executes no checks, captures no terminal, reads no local
evidence files, and does not collect, accept, resume or retry anything. A zero
CLI exit means the status read succeeded, not that verification passed. Share
credentials are allowed only with the native runtime's existing Mission-read
authorization; denied scope has no owner fallback. Mutation overrides are
rejected. The response is bounded and the full Mission is no longer transferred
or cloned for this read. The runtime still scans stored receipts to count them;
this is not a polling service or a constant-time latency claim. Older runtimes
without the negotiated capability are explicitly refused, without full-Mission
fallback. This additional native request path is also unbuilt and untested.

This extension remains unbuilt and untested. Deferred coverage includes pending,
paused and failed Runs, successful exit without evidence, receipt identity
mismatches, more than 16 receipts, acceptance versus recorded verification,
Share scope, missing Runs, stable receipt order and oversized native Missions.

Share credentials, forced terminal Control, and generic mutation/version/key
overrides are rejected for preparation/launch (status permits authorized scoped
reads as described above). Native connection failure does not start a daemon.
Plan/evidence/runner paths refer to this Unix host; forwarding a control socket
does not make local files available on another host. Executable contents,
inherited environment, build caches, and tool configuration remain unpinned.
This is trusted local OS-user execution, not sandboxing or remote attestation.

Future acceptance must cover CLI argument conflicts, local-only preparation,
changed/malformed digests, invalid private paths, owner-only admission, fresh
and pending-verifier launch, launch-error JSON/exit status, uncertain outcomes,
restart/collection, and confirmation that no check or acceptance runs during
preparation. Existing desktop/shared-library results do not certify these new
entry points or any platform/production readiness claim. Further deferred cases
include reserved-ID collision, concurrent creation, missing creation ACKs,
structured unconfirmed-ID errors, explicit resume after interruption, and the
desktop's attempted-ID presentation.
