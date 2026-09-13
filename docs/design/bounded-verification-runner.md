# Bounded verification runner

Status: implemented local Unix CLI and desktop launch/collection workflow.
Automated execution evidence is macOS arm64; native visual acceptance is pending.
This is an opt-in client of the existing runtime, not another scheduler or
workflow database. It adds no always-running process or listener.

For Cargo binary projects, an opt-in [Rust-project recipe](rust-project-verification.md)
generates these same plans with real build/install, smoke, provenance and
repeatability operations. Missing or failing checks never become pass stubs.

## Desktop workflow

Open the Mission graph and select a completed producer with a Candidate awaiting
review, or an already-created pending verifier. In **Project checks**:

1. Choose an owner-private JSON plan (0600), an existing private evidence folder
   (0700), and the SuperPlexr CLI executable. Keep plan and evidence outside the
   source repository. The desktop does not change permissions for you.
2. Select **Review check plan**. All six commands, arguments, time/output limits,
   and the SHA-256 of the exact plan bytes appear without executing anything.
3. Select **Run reviewed checks** and confirm the native warning. Cancellation
   launches nothing. The shared library checks the digest before creating a Run;
   the worker checks it again before executing commands. Editing the plan requires
   another review. This pins plan bytes, not executable/tool contents.
4. When the verifier finishes, select **Collect evidence**. The inspector shows
   each receipt and its check results; **Inspect verifier evidence** navigates
   back to that verifier. Collection never accepts or merges the Candidate.

File choices persist per Mission and launched verifier in personal workspace
state. Loading them does not execute anything. Existing workspace documents
default to no setup. A failed preparation retains its verifier identity so a
pending Run can be launched after correcting the setup; a started/finished
verifier cannot be relaunched. No engine configuration file is rewritten.

Desktop and CLI use `superplexr-verification`, not separate execution or receipt
implementations. The CLI worker's optional `--plan-sha256` argument is supplied
by the desktop launch path. Async results update only matching Mission views
and never replace a newer projection with an older one.

## Trust and scope

Only run plans you reviewed. Check commands execute with the OS user's normal
permissions and inherited environment, including access to that user's files
and credentials. The runner is **not a sandbox** and does not authorize arbitrary
repository-supplied code automatically. Runtime agent/Git-redirection environment
variables are removed from check subprocesses; this is hygiene, not isolation.

Plans must be explicit owner-only regular JSON files outside the Candidate
checkout. The evidence root must already exist, be owner-only, and be outside
that checkout. Use an absolute local path for each executable. Shell syntax is
interpreted only when the plan explicitly invokes a shell.

This first version assumes the owner CLI, runtime, managed checkout and evidence
files are on the same Unix host. It is not a remote evidence-transfer protocol,
Windows implementation, or browser configuration surface.

## Configure a project

Build `cargo build -p superplexr-cli`. Create a private evidence directory and
an owner-reviewed plan (directory mode 0700, plan mode 0600). The version-1 plan
must define exactly these six checks, each with explicit arguments, timeout and
per-stream output limit. For example, for a Rust project:

```json
{
  "version": 1,
  "checks": [
    {"id":"format", "program":"/usr/bin/env", "args":["cargo","fmt","--all","--","--check"], "timeout_seconds":60, "output_limit_bytes":65536},
    {"id":"lint", "program":"/usr/bin/env", "args":["cargo","clippy","--workspace","--all-targets","--","-D","warnings"], "timeout_seconds":600, "output_limit_bytes":1048576},
    {"id":"tests", "program":"/usr/bin/env", "args":["cargo","test","--workspace","--locked"], "timeout_seconds":600, "output_limit_bytes":1048576},
    {"id":"delivery", "program":"/bin/sh", "args":["/absolute/owner-checks/delivery.sh"], "timeout_seconds":300, "output_limit_bytes":1048576},
    {"id":"provenance", "program":"/bin/sh", "args":["/absolute/owner-checks/provenance.sh"], "timeout_seconds":60, "output_limit_bytes":65536},
    {"id":"repeatability", "program":"/bin/sh", "args":["/absolute/owner-checks/repeatability.sh"], "timeout_seconds":600, "output_limit_bytes":1048576}
  ]
}
```

Replace the example script paths with actual reviewed checks for the project.
Delivery should exercise the delivered executable/package; provenance should
validate the project's inputs/generated files; repeatability should compare
independent results. There are no built-in pass stubs for missing checks. An
explicit `/usr/bin/env` uses your PATH; use an absolute tool path to avoid PATH
lookup. Neither choice pins the tool's executable bytes or transitive inputs.

Add a driver entry to the runtime's existing owner-managed `engines.json`,
preserving its other entries:

```json
{
  "version": 1,
  "drivers": {
    "bounded-checks": {
      "program": "/absolute/to/superplexr",
      "args": [
        "verification-execute",
        "--plan", "/absolute/private/project-checks.json",
        "--evidence-root", "/absolute/private/verification-evidence"
      ]
    }
  }
}
```

## Run and collect

A source-only [headless reviewed-launch workflow](../engineering/headless-reviewed-verification-implementation.md)
now avoids manual engine configuration for this path. It is unbuilt and
untested. The examples below describe future use; they were not run:

```sh
superplexr verification-prepare \
  --plan /absolute/private/project-checks.json \
  --evidence-root /absolute/private/verification-evidence
```

Review its commands, paths and limits, then use the returned SHA-256:

```sh
superplexr --socket /absolute/control.sock verification-launch MISSION \
  --subject SUBJECT_RUN --plan-sha256 REVIEWED_SHA256 \
  --plan /absolute/private/project-checks.json \
  --evidence-root /absolute/private/verification-evidence
```

Use `--verifier VERIFIER_RUN` instead of `--subject` only when resuming a known
pending verifier. Launch confirmation is not a passing result or acceptance.
Automation may supply `--new-verifier-id UUID` with `--subject` and retain that
UUID before starting the command. Existing IDs are refused; an interrupted
creation must be inspected before choosing explicit pending-verifier resume.
The existing lower-level configured-engine sequence remains available:

For an existing successfully finished producer with a frozen Candidate:

```sh
superplexr --socket /absolute/control.sock verifier-create MISSION SUBJECT_RUN --engine bounded-checks
superplexr --socket /absolute/control.sock run-checkout-new MISSION VERIFIER_RUN --repository /absolute/repository --base-ref CANDIDATE_REVISION
superplexr --socket /absolute/control.sock run-engine MISSION VERIFIER_RUN --checkout
```

Use the verifier Run ID returned by the first command. The runtime supplies the
frozen Candidate binding and enforces the managed checkout. The worker refuses
to start against a dirty checkout or another HEAD. The copy of the plan actually
executed is retained and hashed with its results.

After the verifier Run finishes:

The source-only `verification-status` command can inspect its execution and
recorded receipt state without collecting anything. It remains unbuilt and
untested. Successful command exit indicates a successful read, not passing
verification:

```sh
superplexr --socket /absolute/control.sock verification-status MISSION VERIFIER_RUN
```

Collect evidence separately:

```sh
superplexr --socket /absolute/control.sock verification-collect MISSION VERIFIER_RUN --evidence-root /absolute/private/verification-evidence
```

Collection validates the finished Run, frozen Candidate, retained normalized
review contract, plan/check identities and every output digest before writing
Artifacts or the receipt. Each check links its report, stdout and stderr evidence.
It requires the owner connection; a Share cannot collect or gain owner fallback.

Both successful examination and collection can exit zero with a **failed**
receipt verdict. Exit zero means examination/recording completed, not that the
Candidate passed. Automation must inspect the JSON `verdict`; only an explicit
owner acceptance can settle the Candidate. Neither command accepts or merges it.

## Bounds and recovery

- Checks execute sequentially. Each allows 1–3600 seconds and 1–1048576 retained
  bytes per stdout/stderr stream. A limit breach terminates that process group
  and records a failed result, not a truncated success. Spawn errors and nonzero
  exits are also failed check results; remaining checks still execute.
- The worker retains at most two output buffers for the active check. An active
  check is inspected every 10 ms; no idle verifier remains after completion.
  Six checks retain at most 12 MiB of output plus a bounded plan and report.
  These are implementation caps, not measured whole-process RSS/CPU budgets.
- Background children in the check's process group are terminated when its main
  process exits. SIGINT, SIGTERM and SIGHUP request worker cancellation and clean
  up that group. SIGKILL, machine failure, malicious daemonization or a process
  escaping its group require stronger OS supervision; no containment is claimed.
- Evidence is stored under `EVIDENCE_ROOT/VERIFIER_RUN/`. Execution refuses an
  existing Run directory rather than replaying its checks. Interrupted partial
  evidence remains for inspection but cannot become a receipt without a complete
  report and a successfully finished verifier Run. Start a new verifier to retry.
- Client detach does not stop the runtime-owned worker. If collection is
  interrupted, rerun **collection**, not execution. Report-derived Artifact and
  request IDs make repeated/partially completed collection idempotent.
- The worker checks Git HEAD and status before and after execution. A final
  changed checkout adds a failed `frozen_candidate_unchanged` check. This detects
  ordinary tracked/untracked changes, not adversarial write-and-restore races,
  ignored files or external side effects. Use the configured provenance check
  and stronger sandbox/effect controls for those guarantees.
- Files are owner-only; final-file symlinks, unexpected fields, oversized data,
  missing output and digest mismatches are rejected. Digests are checked at
  collection, not continuously monitored. Reports are not signed attestations;
  malicious code with the same OS-user authority can forge or alter evidence.
  Artifacts are file references, so preserve the evidence directory after collection.

## Verification

```sh
cargo test -p superplexr-verification --locked
cargo test -p superplexr-cli --test verification_runner_tests --locked
```

Five module tests cover real process exits, separate output, flood/timeout bounds,
background-child cleanup, cancellation, plan validation and private-file checks.
Seven real-daemon integration tests cover successful collection after restart,
explicit acceptance, repeated collection without new graph events, tampered
evidence, failing checks, changed Candidates, hung checks and worker interruption.
They also cover the shared desktop launch path, plan mutation before and after
launch, and existing pending-verifier launch without duplicate Runs or PTYs.
Four additional integration tests exercise the reusable Rust-project recipe:
real Cargo build/install/repeatability, failed delivery/external provenance,
safe generation and nested-tool cancellation. Together the integration target
has eleven passing tests and two explicitly ignored helpers/manual fixtures.
Two desktop persistence tests cover saved choices and old documents. The daemon
helper and manual native fixture are explicitly ignored in the normal suite.

On 2026-09-05, after shared-module extraction, desktop integration and the Cargo
recipe, the complete workspace suite passed 359 Rust tests (six explicit environment/helper/manual
tests ignored). The browser suite also passed 15 JavaScript tests. Workspace
Clippy passed with all targets/features and warnings denied; Cargo still reports
the pre-existing external `block` future-compatibility notice. Formatting and
whitespace checks passed.

An earlier desktop-milestone full-suite attempt, overlapping a build, failed the existing paced
frame-coalescing test: 31 frames exceeded its fixed bound of 30. The unchanged
test then passed alone, in that complete rerun and in the Cargo-recipe suite.
This records an unresolved
timing-flakiness concern, not proof of a performance fix or certified throughput.

Native visual QA was attempted with an isolated desktop/runtime on 2026-09-05.
The window existed, but macOS `CGPreflightScreenCaptureAccess()` returned false
and window capture failed. No rendered-layout, picker, or confirmation-dialog
acceptance is claimed. The fixture was released without touching user sessions.

Remaining work includes native visual acceptance, owner-selected immutable
tool/plan admission, stronger execution isolation, real project delivery/provenance
recipes, remote evidence transport, Linux execution evidence, and performance
telemetry under sustained workloads.
