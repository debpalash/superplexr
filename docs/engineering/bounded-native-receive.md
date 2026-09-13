# Bounded native subscription receive queues

Date: 2026-09-06 IST

## Implementation and intended behavior

The native multiplexed client registers a subscription mailbox when its
acceptance response reaches the dispatcher, before delivering that response to
the caller. Frames received before and after the caller attaches use the same
FIFO. There is no separate early-event buffer to flush or reorder.

Queued payload capacity is bounded at these boundaries:

| Boundary | Limit |
|---|---|
| One subscription | 128 frames and 8 MiB payload capacity |
| One physical connection | 256 subscription mailboxes and 32 MiB payload capacity |
| Native client process | 1,024 mailboxes, 8,192 frames and 64 MiB payload capacity |

These are ceilings, not preallocations. Payload charging uses `Vec::capacity`,
not just its length. Charges are released when a frame leaves the queue or is
discarded. A cancelled subscription retains an empty, bounded tombstone until
the daemon acknowledges Unsubscribe; the daemon joins the aborted producer
before writing that ACK. Late frames before this boundary are discarded.
If abort interrupts a potentially partial subscription frame, a guard shuts
down the socket before releasing the writer mutex. Aborting a task that has
not acquired the writer does not close an otherwise healthy connection.

Overflow retires the physical connection instead of dropping ordered terminal
deltas and pretending continuity. Retirement interrupts both socket directions,
clears queued subscription data, wakes receivers and fails pending RPCs. It
does not wait for the writer mutex. A terminal input lease is pinned to that
connection and therefore cannot remain current after retirement. A failed
native request write also retires the connection before releasing its writer:
an uncertain partial frame must not be followed by another request.

The dispatcher owns only a weak reference while waiting for socket input.
Dropping the final connection owner therefore interrupts its idle reader,
instead of leaving a self-owned socket/thread alive indefinitely.

Development transport-adapter API change: `Connection::from_negotiated` now
requires an interrupt callback. The callback must promptly close both I/O
directions, including blocked reads/writes, without either wire lock, peer
cooperation or a panic. Unix and test TCP adapters use a cloned socket handle
with `Shutdown::Both`. This is not a new network authentication mechanism.

## Verification status

The full release workspace passed **466 Rust tests**, with zero failures and
six explicitly ignored fixture/acceptance entries. All **28 browser/harness
tests** passed. Sixteen new tests cover seven mailbox/accounting cases, six
real native socket cases, two daemon subscription-writer cancellation cases and
one real-daemon/PTY recovery journey. The socket tests include overflow while
a real multi-megabyte request write is backpressured, in addition to retirement
with a deliberately occupied writer mutex.

The real-daemon regression stalls a Controller's first snapshot callback while
an independent PTY producer emits 400 paced lines. Before allowing the callback
to resume, it verifies that the old input lease is no longer current and the
daemon has released Control. Unblocking the callback delivers reconnect state
and a fresh snapshot. The PTY PID survives, no Controller is automatically
restored, stale input is rejected and absent from history, and an explicit new
claim can send fresh input successfully.

Release Clippy across all workspace targets/features passed with `-D warnings`,
as did workspace formatting, explicit formatting checks for the added server
test module and whitespace checks. The pre-existing `block v0.1.6`
future-incompatibility notice remains. All five release executables rebuilt.
The isolated native-keyboard smoke passed on macOS (protocol 25, keyboard input
reaching a real PTY, multiplexed subscriptions retained). The CLI event smoke
passed with four logical feeds on one persistent connection. The real-daemon
overflow regression also passed an additional standalone run after the final
suite. Fixture cleanup left the user's existing desktop and daemon running.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
node --test crates/superplexr-observer/web/*_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
cargo build --release --locked -j 2 -p superplexr-server -p superplexr-cli -p superplexr-observer -p superplexr-tui -p superplexr-desktop
SUPERPLEXR_INPUT_SKIP_BUILD=1 SUPERPLEXR_INPUT_BINARY_DIR="$PWD/target/release" bash ci/desktop-input-smoke.sh "$PWD"
SUPERPLEXR_MULTIPLEX_BINARY_DIR="$PWD/target/release" sh ci/cli-event-multiplex-smoke.sh "$PWD"
```

This is transport/PTY evidence on macOS, not visual, IME, accessibility,
cross-platform, production-identity or new performance certification.

## Remaining boundaries

This is not a total process-memory or universal non-blocking-I/O guarantee.
Decoded objects already handed to a consumer, current wire decoding buffers,
the separate acknowledged-search queues, pending RPC responses and some
higher-level metadata relay channels were outside this mailbox budget. The
subsequent [pull-driven metadata milestone](pull-driven-metadata.md) removes
those four native decoded FIFOs and bounds desktop/CLI handoffs; decoded
consumer state still is not charged to the raw wire budget. Protocol sequence
tracking across long-lived stream churn needs its own lifetime audit.

Mailbox limits are stricter than the wire protocol's absolute decoded-frame
ceiling. Oversized snapshots and metadata batches need explicit size/admission
and reconnect-policy acceptance; the small-grid recovery test is not evidence
for every maximum-size grid or metadata collection.

Explicit claim, unsubscribe and lifecycle calls can still block their callers.
Broader share-authority, client parity, cross-platform and sustained
output/resource acceptance remain separate gates. No new resource benchmark
has been run for this change.
