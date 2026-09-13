# Verified-delivery journey evidence

Executed 2026-09-05 on macOS arm64, using development builds.

## What this milestone adds

Two automatically discovered integration tests drive the actual server binary
through the shared client and Unix socket. They use real PTYs, configured
producer/verifier drivers, Git worktrees, frozen Candidates, content-digested
evidence, and durable Mission records. There are no mocked runtime responses,
provider credentials, paid model calls, or user repositories.

```sh
cargo test -p superplexr-server --test delivery_journey_tests --locked -- --nocapture
```

They also run under the existing `cargo test --workspace --locked` CI command;
no manually enabled ignored-test flag is required.

The workspace run passed 341 Rust tests with four explicitly ignored tests.
Workspace Clippy passed with all targets/features and warnings denied; Cargo
continues to report the existing external `block` future-compatibility notice.
The new fixture adds only a test dependency on the existing shared client; it
adds no production service, listener, dependency, or background resource usage.

## Return, retry, verify, accept

The deterministic fixture asks for a tiny shell program that prints `42`:

1. An admitted producer changes only its declared file in a managed checkout,
   but deliberately produces `41`. The server freezes the actual bytes and
   authors the patch and review contract.
2. Acceptance without an independent receipt is rejected. The producer's
   mutable checkout is then changed again; verification still uses the frozen
   Candidate, not those later bytes or an owner-supplied alternative directory.
3. A separate verifier process executes every named check in the normalized
   contract. The fixture records its actual results and an Artifact digest,
   then submits a failed receipt. Acceptance is still rejected.
4. The owner returns the result with a note. The retry retains that exact note,
   Candidate, lineage, and independent-verification requirement. A daemon crash
   and restart, followed by repeated publication/retry requests with their
   original idempotency keys, creates no duplicate work.
5. The retry starts from the returned Candidate and produces `42`. It requires
   a fresh verifier and receipt; old or wrong-candidate evidence cannot accept it.
6. A second restart preserves the passing receipt and its evidence. Explicit
   owner acceptance succeeds and is safely repeatable with its original key.
   The first Run remains rejected; the retry becomes accepted. Neither restart
   launches another process. The source repository remains unchanged: acceptance
   is not an implicit merge.

The six fixture checks are deliberately small and concrete:

| Contract check | Executed fixture check |
| --- | --- |
| Format | Final file byte is a newline. |
| Lint | `/bin/sh -n` accepts the program. |
| Tests | Running the program produces the required `42`. |
| Delivery | Install an executable copy outside the checkout and run it successfully. |
| Provenance | Verifier HEAD equals the frozen revision and its checkout is clean. |
| Repeatability | Two independent program invocations return identical output. |

The verifier Run succeeds when examination completes, even when a check fails.
Its receipt verdict describes the Candidate; only the owner settles it. The
test reads the frozen review contract and requires a result for every check,
rather than replacing `not_run` with invented pass labels.

## Crash during execution

The second test crashes the daemon after a Candidate is published but before
the producer finishes. Recovery preserves the Candidate and terminal output,
marks the interrupted Run failed, releases its Execution lease, and prevents
verification or acceptance from converting that interruption into success.
Replaying the original publication request does not change the current failed
Run. A second restart creates no additional recovery events or replacement Run.

This does **not** claim PTY process survival across daemon death. Client detach
and daemon crash have different recovery guarantees.

## Limits and next implementation

This milestone is executable acceptance coverage of the existing workflow, not
a new generic verification service. Its trusted fixture coordinator converts
observed check results into Artifacts and a receipt using the owner API. It does
not prove that arbitrary agents report truthfully or that the agent channel can
publish a post-exit receipt on its own.

Follow-up implemented: the [bounded verification runner](../design/bounded-verification-runner.md)
adds explicit owner-configured commands, time/output bounds, retained evidence,
and idempotent receipt collection to the production CLI. Its separate tests cover
the reusable path; the deterministic journey recorded above remains unchanged.
Check execution remains distinct from owner acceptance. Additional gates remain for real
project build/install checks, adversarial provenance, external-effect fencing,
landing/merge, UI journeys, Linux Wayland/X11, and sustained performance budgets.
The Linux CI configuration will discover these tests, but no Linux execution
result is claimed by this macOS run.
