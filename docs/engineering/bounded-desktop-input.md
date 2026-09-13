# Bounded desktop input delivery

Date: 2026-09-06 IST

The desktop previously queued input and errors without bounds and sent commands
through a mutable Session handle. Commands waiting behind a failed request could
therefore use a subsequently repaired connection or a newly acquired Control
epoch. The desktop now pins each input generation to its explicit claim ACK.

## Implemented boundary

- `TerminalInputLease` retains the exact native wire, Session/Surface identity,
  acknowledged epoch and shared local retirement flag. Sending never reconnects,
  retries or claims Control. Every error retires the lease.
- The native writer checks lease validity again after acquiring its writer lock,
  immediately before serialization/write admission. Retirement while waiting
  behind another writer therefore discards the command without transmitting it.
- Each live Surface owns one ordered worker and a queue capped at 128 commands
  and 8 MiB of retained variable payload capacity, including its in-flight
  command. Vec/String capacities, not only lengths, count toward this bound.
  Archived Surfaces do not create input workers.
- Overflow, hide, read-only search history, connection loss, rejection and
  terminal exit retire pending input locally. Waiting commands are discarded;
  a single latest-status slot replaces the former error FIFO. Status text is
  capped at 1024 Unicode characters.
- Queue retirement and admission never perform socket I/O while holding the
  queue mutex. Dropping the Surface wakes an idle worker without joining it.
  An old in-flight result cannot retire a newly armed generation; its retained
  bytes continue counting until that worker returns.
- Initial live attachment still attempts non-forced Control. Following local
  retirement, the person must explicitly use **Request control**. Metadata and
  repaired observation never re-arm input. Ownership updates older than the
  claim ACK are ignored; current/newer ownership or epoch mismatches retire it.

Copy/selection remain local and available while input is disabled. Closing a
search preview returns to live observation, not automatic interaction. Local
retirement does not itself release the daemon's Control holder or stop its PTY.

## Verification

Eight targeted release tests passed: seven real-daemon desktop queue/lease tests
and one private native-writer/socket boundary test. Coverage includes:

- exact epoch binding, forced takeover and no resurrection of an old lease;
- command and byte overflow with a paused isolated daemon, discard of a queued
  8 MiB paste, and explicit re-arm without a late error poisoning fresh input;
- reserved capacity on an empty payload being rejected at admission;
- a dropped alternate transport, observation repair, no old-input replay and
  successful input only after explicit acquisition;
- hide/history-style retirement and queue destruction preserving the PTY;
- Observer denial, Controller Share revocation and continued owner access;
- stale pre-claim metadata, current release and no metadata-only re-arm;
- retirement while waiting for the native writer lock sending no request bytes.

The full release workspace passed **442 Rust tests**, with zero failures and
six explicitly ignored fixture/acceptance entries. The seven desktop input
regressions passed three additional consecutive runs. All **28 browser/harness
tests** passed. Workspace release Clippy across all targets/features passed
with `-D warnings`, as did formatting, diff-whitespace and smoke-script syntax
checks. The existing `block v0.1.6` future-incompatibility notice remains.
All five release executables rebuilt successfully. The macOS native keyboard
smoke passed three consecutive runs against the new release desktop: process-targeted CoreGraphics
physical-key events produced `upinput` in its real PTY, with both logical
subscriptions retained on protocol 25's multiplexed connection. This checks
the production GPUI input route, not physical hardware, rendered visual quality,
IME, accessibility or another operating system.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
node --test crates/superplexr-observer/web/*_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
cargo build --release --locked -j 2 -p superplexr-server -p superplexr-cli -p superplexr-observer -p superplexr-tui -p superplexr-desktop
SUPERPLEXR_INPUT_SKIP_BUILD=1 SUPERPLEXR_INPUT_BINARY_DIR="$PWD/target/release" bash ci/desktop-input-smoke.sh "$PWD"
```

The smoke's optional skip-build setting allows testing an already-built release
without triggering an unrelated debug/native-engine rebuild; its default build
behavior is unchanged. It creates its own temporary runtime, Session and window,
posts keyboard events only to that desktop PID, then cleans up its fixtures.
The user's running apps and saved workspaces are not restarted or rewritten.

At this milestone, the queue tests observed output through a separate client
connection. A blocking terminal-wait RPC could otherwise overtake the background
input sender and block later commands on that same server connection. Two
initial tests exposed that head-of-line behavior; the workaround added no sleeps
or command retries. The subsequent [concurrent-wait milestone](concurrent-terminal-waits.md)
fixes that dispatch dependency and restores same-connection verification in the
queue tests.

## Remaining acceptance boundaries

This is a bounded desktop admission layer, not an end-to-end transport memory or
security certification. In particular:

- Writes already admitted to socket I/O cannot be rolled back. An uncertain
  in-flight command may have executed; it is never automatically replayed.
- Encoded wire buffers, fixed object overhead, native receive subscriptions and
  total concurrent Surface/worker counts are outside this payload budget.
- A worker blocked inside native socket I/O can outlive its Surface until the
  transport returns. Interruptible writer/cancellation teardown remains open.
- Explicit Control claims, some other desktop RPCs and subscription cancellation
  remain blocking; ordinary server RPC dispatch can cause head-of-line blocking.
- Application-wide hidden/archive authority invalidation, full reconnect/resize
  UX, physical IME/accessibility, public remote identity/TLS, platform acceptance
  and sustained input/output resource budgets remain separate work.

The previous [five-minute resource sample](shared-client-coalesced-feed-release.json)
measured the preceding binary, not this change. No new resource result is implied.
