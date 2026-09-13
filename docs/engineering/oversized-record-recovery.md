# Oversized subscription records: explicit non-retryable termination

Date: 2026-09-06 IST

## Behavior

The native mailbox's 8 MiB retained-payload limit previously treated one record
larger than the entire mailbox like a temporary backlog. Automatic resubscription
could encounter that same record repeatedly, consuming resources without making
progress. This is distinct from a consumer falling behind on individually
admissible records, which can recover by obtaining a fresh snapshot.

The queue now distinguishes an intrinsically oversized record from transient
aggregate byte/count saturation. Before interrupting the connection, the native
dispatcher records the oversized allocation's capacity. Pending/attaching
subscriptions and active receivers can report typed `ClientError::ReceiveLimit`
with capacity and limit, instead of losing the cause as generic disconnection.

The whole physical connection still retires: queued data is cleared, pending
work is released and connection-pinned input becomes invalid. Existing read
subscriptions on that connection stop automatic retry. Explicit new operations
can establish a fresh connection, but old receivers and input leases never
migrate or revive. The limit does not kill the Session's PTY or delete data.

Terminal callbacks expose `ReceiveLimited`, separately from authorization
rejection. Desktop terminal presentation clears stale state and shows the
resource-limit message. The TUI clears and detaches with the resource message.
Browser SSE emits a terminal `ended` event carrying that message; an initial
receive-limit failure maps to HTTP 413. The browser does not automatically retry
either case. Metadata receivers return the typed error, CLI event streaming
propagates it as failure, and desktop metadata workers log the failure once.

## Verification status

The targeted `oversized_` release workspace run passed. Coverage includes:

- A real Unix socket sends an oversized record before receiver attachment;
  attaching returns the typed limit instead of starting a reconnect loop.
  Repeated metadata receives preserve that failure without invoking reconnect.
- A real daemon starts a PTY and a small Mission, then grants a Controller Share
  scoped to both. Desktop and browser feeds share that connection. Growing the
  Mission through a valid domain command produces a record above 8 MiB: metadata
  stops, held desktop data becomes invalid, the browser receives a resource-limit
  `ended` event, and old input authority retires. Explicit fresh Control sends
  input to the same PTY PID; the old receiver and lease remain invalid.
- A real-daemon TUI feed clears its frame and reports `ReceiveLimited` while the
  underlying PTY survives.
- The desktop handoff clears stale frames and retains the resource-limit reason.
- Browser tests cover both initial HTTP 413 and established SSE termination:
  each performs one fetch and stops without automatic retry. All 29 JS
  browser/harness tests passed.

Review also identified request admission while a writer is occupied as another
place that could erase the permanent failure. Closed-wire checks and failed
writes now preserve the recorded limit; an additional real-socket writer-wait
regression covers this race. Although the first full suite passed, an extra
standalone desktop/browser run subsequently failed its connection
cleanup assertion. Waiting for browser EOF and async view cleanup made this
failure deterministic (three consecutive failures, two connections instead of
one). The cause was cleanup's `release_control()` using generic connection
repair after the old wire had closed. Release now sends only on the existing
connection and never repairs it; disconnect already releases that wire's Control.
After that fix the full release workspace passed again: **475 Rust tests, zero
failures, six ignored**; all **29 JS tests** and strict all-target/all-feature
Clippy passed. The tightened real-daemon regression also passed 20 consecutive
standalone runs. All five release executables rebuilt; native macOS keyboard
input (protocol 25, retained multiplexed subscriptions) and CLI event multiplexing
(four logical feeds, one connection) smoke checks passed. Formatting and diff
whitespace checks passed. The pre-existing `block v0.1.6` dependency still emits
a future-Rust incompatibility advisory, not a Clippy failure.

The fixtures cleaned up their own processes and temporary data. The user's
existing desktop/runtime processes remained alive and were not restarted.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo build --release --locked -j 2 -p superplexr-server -p superplexr-cli -p superplexr-observer -p superplexr-tui -p superplexr-desktop
node --test crates/superplexr-observer/web/*_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
cargo fmt --all --check
git diff --check
SUPERPLEXR_INPUT_SKIP_BUILD=1 SUPERPLEXR_INPUT_BINARY_DIR="$PWD/target/release" bash ci/desktop-input-smoke.sh "$PWD"
SUPERPLEXR_MULTIPLEX_BINARY_DIR="$PWD/target/release" sh ci/cli-event-multiplex-smoke.sh "$PWD"
```

The earlier 470-test metadata milestone's five-minute resource artifact describes
its own binary, not this failure-path change. No new resource sample is claimed.

## Deliberately incomplete scope

This fixes futile retry behavior, not support for arbitrarily large records.
Full-frame chunking, paged/summary metadata, negotiated record-size admission,
an explicit desktop metadata-feed status banner, and complete collection
snapshot/replacement semantics remain work. Large ordinary RPC responses and
decoding buffers are not charged to the subscription mailbox budget. No new
performance or cross-platform certification is implied by this failure path.
