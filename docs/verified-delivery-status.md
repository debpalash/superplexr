# Verified work delivery status

Updated: 2026-09-05.

For the combined product, research, and release roadmap, see
[ultraplexr-unified-roadmap.md](ultraplexr-unified-roadmap.md).

The research review and multiplexer survey identify the same product gap: a
fast terminal workspace is useful, but an agentic workspace becomes distinct
only when it can prove what was authorized, what actually ran, what changed,
what another actor verified, and what the owner accepted.

## What exists now

The authoritative path is one event-sourced Mission projection:

```text
ChangeIntent -> admission + Execution lease
             -> resolved driver + HarnessSnapshot
             -> Run + Session
             -> Candidate + HandoffArtifact
             -> independent EvaluationReceipt (when required)
             -> owner Settlement (accept or return)
```

The core rejects stale intent versions, overlapping admitted file claims,
caller-authored lease epochs, mismatched harness drivers, self-verification,
receipts for failed or unfinished Runs, receipts for another candidate digest,
and independent settlement without a passing receipt. Event replay runs the
same validation as live commands.

The server serializes admission and launch, checks conflicts across Missions,
authors the lease epoch, records the harness before execution, injects the
fencing epoch into the agent process, rejects Candidate publication under a
missing or stale epoch, and releases the lease when the Run ends.
The agent socket allows a producer to submit only its own Candidate/Handoff and
a verifier to submit only a receipt bearing its own Run identity.

The CLI exposes declaration, admission, verification policy, candidate,
handoff, receipt, and escalation commands. The native graph inspector presents
these records together and supports acceptance or return with an operator note.

## What this does not claim yet

Admission prevents conflicting Runs from launching and provides a fencing
identity to adapters. Candidate publication now scans the managed checkout off
the async control path, rejects changes outside committed exact-file claims,
normalizes renames to delete plus create, and freezes admitted bytes through an
isolated temporary Git index into a retained snapshot commit. The Run's own
index is untouched. A binary-safe base-to-snapshot patch Artifact,
content-addressed review-contract Artifact, and Candidate are committed as one
idempotent batch. Settlement verifies the retained ref, tree, and patch digest,
so later checkout writes remain unpublished instead of changing what is under
review. Contingent claims require explicit versioned promotion and promotion
rotates the lease epoch. The workspace sandbox is still broader than exact
ChangeIntent enforcement. The review contract contains the exact changed files
and explicit `not_run` slots for formatting, lint, tests, delivery, provenance,
and repeatability; it never fabricates passing evidence.

The native review surface can create an independent verifier or retry returned
work. Both actions freeze the exact Candidate and HarnessSnapshot onto the new
Run. Retry additionally freezes the owner Return note, links `retry_of`, and
opens a proposed exact-file ChangeIntent at the Candidate revision. Checkout
provisioning runs off the UI thread, and the runtime refuses to launch these
Runs outside their managed Candidate checkout.

Linux remains a first-version platform. The verified-delivery milestone stays
open until its complete journey is exercised under both Linux display backends
and macOS; passing Rust unit tests on macOS is not Linux release evidence.

## Next build order

The first real-daemon macOS acceptance journey now covers actual producer and
verifier execution, failed-check return, retry resubmission, fresh evidence,
owner acceptance, and restarts between stages. A second test covers interruption
after publication without false success or automatic relaunch. Both run in the
normal workspace suite. See the
[journey evidence and its limits](engineering/delivery-journey-evidence.md).
The original journey uses deterministic fixture drivers. A reusable
[bounded verification runner](design/bounded-verification-runner.md) now executes
explicit owner-configured plans inside verifier Runs and collects retained,
digested results into receipts. Separate real-daemon tests cover this production
CLI path, including failures, restart, tampering and cancellation. Desktop setup,
reviewed-plan launch, per-Run file choices and receipt inspection now use the same
module. Plan bytes are rechecked before Run creation and before execution.
An opt-in [Cargo binary recipe](design/rust-project-verification.md) now generates
plans for actual Cargo format/lint/test, installation and expected output,
tracked-input provenance and fresh-build binary comparison. A real compiled
fixture exercises the production CLI/runtime path, including failed delivery and
external path provenance. Native visual acceptance, Linux journeys, stronger
isolation and broader real-project recipes/acceptance remain.

1. Complete the realized-change adversarial and platform matrix: generated
   provenance, concurrent-write and symlink races, and macOS/Linux fixtures;
   extend fencing to mutable external-effect adapters.
2. Extend the bounded runner with real-project test, semantic-impact,
   delivery/install, provenance and repeatability recipes; complete desktop QA and
   stronger immutable tool/plan admission around the existing receipt workflow.
3. Complete retry resubmission, inline review comments, landing preview, and
   explicit land/merge without force operations.
4. Extend the existing headless end-to-end delivery fixture to native interaction
   and run it on macOS, Linux Wayland, and Linux X11 with performance telemetry.

After that gate, the highest-value research-derived features are fan-out
experiments with side-by-side candidate comparison, evidence-preserving context
compaction, deterministic supervisor policies, effect-bound checkpoints, and
digest-bound admission for executable configuration and external tools.
