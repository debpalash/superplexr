# In-flight Share authority and lease identity

This follows the [bounded-search milestone](bounded-history-search.md). Longer
asynchronous requests exposed the distinction between authorizing a request at
entry and retaining authority while it waits or writes a response.

## Reproduced failures

Real daemon/Unix-wire/PTY regressions established three failures before their
fixes:

- An Observer started a terminal text wait. The owner durably revoked the Share,
  then produced new output. The pending request still returned a capture with
  that post-revocation output. The isolated regression failed in 0.82 seconds.
- A connection claiming the owner's public client ID could cause disconnect
  cleanup to release the owner's Control, including with an invalid capability.
- A Controller Share supplying the owner's visible client/surface/epoch could
  be treated as the same Control holder. Share identity was missing from the
  lease comparison. Both identity regressions failed against the real daemon.

Tests only use private fixture runtimes and generated capabilities; no user
runtime, provider credentials, or remote service is involved.

## Implementation

`share_request::Access` centralizes ordinary shared-RPC authority. It subscribes
to revocation before checking the durable Share store, then guards computation,
writer queueing and response I/O with a revocation listener and expiry timer.
The durable expiry/revocation check is repeated after acquiring the writer.
An unrelated revocation is ignored; a lost interval in the revocation channel
fails closed. Owner requests do not install Share timers or event listeners.

Revocation/expiry cancels the operation and terminates the connection. It does
not append an error response to a possibly partially written frame. Connection
cleanup aborts its subscription tasks and releases only leases matching both
the authenticated client ID and authenticated Share/owner identity. A claimed
client ID without completed authentication cannot release a lease.

Control-holder comparisons for claim, input, resize and release now include
`controller_share_id`; input/resize/release retain their existing client,
Surface and epoch checks. A Controller Share cannot
impersonate an owner's lease using coordinates learned from a projection. No
role allowlist, capability format, protocol version, or persistent schema changed.

Cancelling a terminal wait releases its active-wait slot. Its blocking event
forwarder checks consumer closure at a bounded 100 ms receive interval, so a
silent PTY cannot strand that thread. This applies to explicit wait RPCs only;
ordinary idle terminal subscriptions do not gain a new polling timer.

## Verification

Five real-process integration tests cover:

- pending-read revocation, prompt retirement, refused reauthentication, continued
  owner input, and unchanged Session ID / PTY PID;
- another Share's revocation not cancelling a valid wait;
- expiry during a pending wait after daemon restart;
- invalid/differently scoped connections being unable to release owner Control;
- rejection of spoofed claim/release/input with valid Controller credentials.

The expiry fixture adjusts only its stopped private Share record to simulate
restarting near expiry; the production minimum TTL stays 60 seconds. It confirms
the request was actually admitted via runtime active-wait diagnostics.

Five focused tests at the production guard/forwarder interfaces cover stale
authority before admission, revocation-channel lag, writer-queue revalidation
without an event, cancellation during an 8 MiB response over a real Unix socket,
and forwarder retirement while its source stays silent. The partial-write test
requires bytes to have started, then proves a complete response was not sent and
closes the writer. These complement rather than replace the real-daemon tests.

All ten targeted tests passed locally. The full release workspace suite passed
**391 Rust tests**, with six explicitly ignored tests, on 2026-09-06 IST. The
browser/harness suite passed **20 tests**. Workspace release Clippy with all
targets/features and `-D warnings`, Rust formatting checks (including the new
server modules), and diff whitespace checks passed. The existing `block v0.1.6`
future-incompatibility notice remains. All five release executables (server, CLI,
observer gateway, TUI and desktop) rebuilt successfully. The user's existing
desktop/runtime processes were checked alive and were not restarted or upgraded.

Reproduction from the repository root:

```sh
cargo test --workspace --release --locked -j 2 --test share_revocation_tests
cargo test --workspace --release --locked -j 2
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
node --test crates/superplexr-observer/web/control_tests.mjs crates/superplexr-observer/web/stream_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

## Limits and next work

The subsequent [awaitable-search implementation](awaitable-history-search.md)
propagates request-future cancellation into live admission/scanning and archived
replay. The blocking-collector limitation below records this milestone's original
boundary; wire-visible paging and comprehensive transport cancellation remain open.

- Revocation cannot retract bytes already delivered, undo terminal input/effects
  already admitted, or preempt synchronous code halfway through an operation.
  This is cancellation at async wait/write points, not transactional rollback.
- A cancelled `spawn_blocking` history collector/replay may finish its bounded
  read work. It cannot deliver that result on the retired connection. Wiring
  cancellation down to search jobs and exposing paged results remains open.
- Agent-process channels have a separate identity/lifecycle protocol and are not
  covered by this Share guard. Existing subscription event handlers retain their
  own revocation logic; this milestone is not a comprehensive transport audit.
- The raw local Unix endpoint remains an owner-trusted endpoint. This does not
  establish public TLS access, device enrollment, per-user identity, a secure
  raw remote relay, or production Internet collaboration.
- Tests ran on local macOS. Physical Linux/Windows, broader denial-of-service and
  clock-change testing, native visual QA, and production security review remain.
- Earlier five-minute resource artifacts retain their original binary hashes.
  They are historical measurements, not measurements of these new binaries.
