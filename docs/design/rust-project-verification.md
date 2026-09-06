# Rust-project verification recipe

Status: opt-in local Unix recipe for Cargo binary projects. Automated acceptance
uses a real dependency-free Cargo project on macOS arm64. This is not evidence
that Ultraplexr itself, arbitrary large projects, or Linux releases have passed
the recipe.

The recipe generates a normal [bounded verification plan](bounded-verification-runner.md).
It uses the existing verifier Run, frozen Candidate, process-group timeout/output
bounds, evidence collector and owner acceptance rules. No service, scheduler,
listener or runtime dependency is added.

## Configure and review

Prepare an owner-only directory for the recipe/plan and an existing owner-only
scratch directory (0700), outside the source repository. Save an owner-private
(0600) JSON recipe there:

```json
{
  "version": 1,
  "cargo": "/absolute/toolchain/bin/cargo",
  "package": ".",
  "binary": "my-tool",
  "scratch_root": "/absolute/private-build-scratch",
  "smoke_args": ["--version"],
  "expected_stdout": "my-tool 0.1.0\n"
}
```

Replace the tool, binary, paths and expected output with the project's actual
contract. `package` is relative to the Candidate root; workspace members can use
paths such as `crates/my-tool`. Absolute packages, parent traversal and non-simple
binary names are refused. At most 32 smoke arguments and 1024 expected UTF-8
stdout bytes are supported; the embedded recipe must fit the plan's 4096-byte
argument bound.

```sh
ultraplexr verification-plan-rust \
  --recipe /absolute/private-checks/rust-recipe.json \
  --output /absolute/private-checks/rust-plan.json
```

Generation executes no project or tool command, creates the plan with mode 0600,
and refuses to overwrite any existing file. The output directory must already
be private. Recipe options are copied into the plan, not read from the recipe
file at execution time. Review the generated plan in the desktop Project checks
panel or use the CLI verifier workflow in the bounded-runner guide. Choose a
separate private evidence directory for retained results.

Each generated check defaults to 600 seconds and 1 MiB per output stream. Owners
can edit these normal plan limits before review. Plan SHA-256 pinning applies
when launched through the desktop/shared launch module. Executable contents,
Cargo configuration, toolchains and inherited environment are **not** pinned.

## What the six checks establish

| Check | Actual operation |
| --- | --- |
| Format | `cargo fmt --all -- --check` from the selected package. |
| Lint | `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`. |
| Tests | `cargo test --workspace --locked --offline`. |
| Delivery | `cargo install --path PACKAGE --bin BINARY --locked --offline` into a new private prefix and target directory; run the installed binary from that prefix with the specified arguments; require successful exit and exact expected stdout. |
| Provenance | Record Cargo version and locked/offline dependency metadata; require the workspace, lockfile, local manifests and target entry files to resolve inside the Candidate and be Git-tracked; record their SHA-256 digests. |
| Repeatability | Install into two separate fresh target/prefix directories, require the smoke expectation for both, and compare the installed executable SHA-256 digests. A mismatch fails. |

Cargo is configured for one build job, no incremental compilation, offline
resolution and plain output. Format/lint/tests retain their normal workspace
scope; delivery/repeatability select one binary. There is no promise that the
project has enough tests simply because `cargo test` exits successfully.

Tool stdout/metadata collection is capped at 1 MiB internally, and aggregate
stdout/stderr are governed by the outer check limits. File hashing is streaming.
Nested tools inherit the check's process group, so cancellation does not leave
an intentionally separate Cargo group behind.

Normal completion and ordinary failures retire only the newly created
`rust-check-UUID` scratch directories. Timeout, cancellation, machine failure or
SIGKILL can leave those directories for owner inspection/cleanup. Retained
evidence includes logs, commands and digests, not a retained installable binary.
The owner scratch root, normal Cargo install prefix and existing targets are
never cleanup targets.

## Trust and remaining limits

- This executes trusted code with normal OS-user privileges. Cargo offline mode
  is not network isolation: project/build scripts can access the network, and a
  missing rustup toolchain/component may trigger a download. Preinstall the
  selected toolchain, rustfmt, Clippy and required offline dependencies.
- Cargo/Rust configuration and environment remain in scope. Build scripts,
  compiler wrappers, shared caches, registry/Git dependency caches, linked native
  libraries and generated/included files are not exhaustively attested. Metadata
  checks are useful provenance evidence, not hermetic-build certification.
- Fresh target directories do not isolate external caches. The recipe compares
  one installed executable on one host; it does not compare every package file,
  prove cross-host reproducibility or apply a normalization that hides differences.
- Cargo jobs are limited, but test processes, compiler memory, build disk usage
  and arbitrary subprocesses have no certified CPU/RSS/disk quota. Large projects
  need explicitly chosen budgets. No small-footprint claim follows from this test.
- Local path dependencies outside the Candidate fail provenance even if other
  checks succeed. That is an acceptance guard, not a pre-execution sandbox.
- The internal `verification-rust-check` child command relies on the bounded
  runner for timeout/cancellation; do not invoke it directly as an untrusted-code
  runner. Collection and owner acceptance remain separate.

## Executed acceptance

```sh
cargo test -p ultraplexr-cli --test verification_runner_tests \
  rust_project_recipe_tests --locked -- --nocapture --test-threads=1
```

The integration fixture commits a real Cargo binary, lockfile and tests, then
launches an admitted producer to update its compiled-in product input. Verification
runs from the frozen Candidate, not the original repository. Tests cover:

- Plan generation without execution, private output and no-overwrite behavior.
- Actual formatting, Clippy, unit tests, install/run, tracked-input provenance
  and identical binary digests across two fresh builds.
- Daemon restart before collection, Candidate binding, repeat collection and
  no automatic acceptance or modification of the original repository.
- Wrong delivery stdout and an external path dependency: unit tests pass, but
  the appropriate check and receipt fail and acceptance is refused.
- Invalid package traversal, with no plan written and no process launched.
- Cancellation while a recipe tool has a nested process, with no collectible
  report or receipt.

On 2026-09-05 all four recipe tests passed. The full workspace run passed 359
Rust tests with six explicitly ignored tests; 15 browser tests passed. Desktop
and CLI builds, workspace Clippy with warnings denied, formatting and whitespace
checks passed. The external `block` future-compatibility notice remains.

See the [bounded-runner guide](bounded-verification-runner.md) for the current
workspace regression ledger and the pending native visual acceptance gate.
