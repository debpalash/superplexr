# Concurrent terminal waits on a shared connection

Date: 2026-09-06 IST

The multiplexed connection previously awaited every ordinary RPC inline. A
terminal wait sent before a background input command therefore blocked the
input that could satisfy it. Multiplexed event streams alone did not prevent
this request-dispatch head-of-line blocking.

## Implemented behavior

`TerminalWait` now enters a bounded connection-owned task set after normal
protocol-version, pinned identity, Share scope and parameter validation. The
connection continues reading input, Ping and other requests while the wait
observes terminal state. Responses retain their original request IDs and may
complete out of request order. Input and other mutations remain serial on the
connection loop; this change does not make all RPCs concurrent or replay writes.

- At most eight wait tasks belong to one connection. A process-wide semaphore
  limits this control-connection lane to 32 permits, including scheduling,
  observation and response delivery. Admission is fail-fast (`wait_capacity`),
  with no additional waiting-task queue.
- Each native event-forwarding thread holds a clone of its task's permit until
  it exits. Cancelling its async consumer cannot free that permit prematurely.
  Existing active-condition diagnostics and the 128-condition cap across runtime
  entry points are unchanged. The separate agent channel remains serial.
- No new idle polling thread or timer is introduced. Only an active wait may
  own a native event-forwarder; Exit waits use authoritative summary events.
- Connection destruction closes the socket before aborting its wait tasks.
  Disconnect does not wait for the condition's deadline and does not stop the
  Session's PTY. Waits are not automatically restored on another connection.
- Share lifetime/revocation guards cover computation and response writing.
  Access is rechecked after acquiring the writer. Interrupted response framing
  shuts down the socket before releasing the writer mutex to another task.
- Response delivery has a two-second deadline, including writer queueing; total
  task lifetime is also bounded by the requested wait timeout plus two seconds.
  A normally expired condition receives an ordinary error and leaves the
  connection usable. Revocation, uncertain framing or failed delivery closes it.

The existing wait validation remains: timeout 1 ms–1 hour, text query 1–1024
bytes without NUL, and quiet interval 50 ms–60 seconds. No wire schema, new
transport, permission or terminal authority is introduced.

## Verification status

Before the implementation, the real-wire regression sent a 30-second wait and
then Ping on the same socket. It failed at the two-second socket-read deadline,
reproducing the reported blocking independently of the desktop queue fixture.

The full release workspace passed **450 Rust tests**, with zero failures and
six explicitly ignored fixture/acceptance entries; all **28 browser/harness
tests** passed. Eight new tests cover same-wire input and response correlation,
Control release ahead of subsequent input, per-connection and process saturation,
completion/readmission, disconnect, revoked and out-of-scope access, wait timeout,
and cancellation/revalidation at a blocked response writer.

The six real-daemon wait regressions and seven desktop input regressions passed
three additional consecutive runs. Desktop input tests now use same-connection
waits again; their earlier separate-observer workaround is removed. The
process-cap fixture intentionally performs no startup wait, because even a
completed readiness wait can still own a retiring forwarder permit.

Workspace release Clippy across all targets/features passed with `-D warnings`,
as did workspace formatting, explicit formatting checks for changed server
include modules, and diff-whitespace checks. The existing `block v0.1.6`
future-incompatibility notice remains. All five release executables rebuilt.
The isolated macOS native-keyboard smoke passed against the rebuilt desktop
(protocol 25, physical-key events reaching a real PTY, multiplexed subscriptions
retained). The CLI event smoke also passed with four logical feeds on one
persistent connection. These checks do not certify visual quality, hardware IME,
accessibility or another platform.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
node --test crates/superplexr-observer/web/*_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
cargo build --release --locked -j 2 -p superplexr-server -p superplexr-cli -p superplexr-observer -p superplexr-tui -p superplexr-desktop
SUPERPLEXR_INPUT_SKIP_BUILD=1 SUPERPLEXR_INPUT_BINARY_DIR="$PWD/target/release" bash ci/desktop-input-smoke.sh "$PWD"
SUPERPLEXR_MULTIPLEX_BINARY_DIR="$PWD/target/release" sh ci/cli-event-multiplex-smoke.sh "$PWD"
```

The harnesses cleaned up their temporary runtime/Session/window fixtures. The
user's active apps and saved workspaces were not restarted or rewritten.

## Deliberately remaining boundaries

This closes the terminal-wait dependency cycle, not all head-of-line blocking.
Collected legacy search, journal history reconstruction and other ordinary RPCs
still use the serial dispatch path. The negotiated acknowledged search stream
already uses its own bounded path. Those other long reads need their own
cancellation/admission audit before concurrent dispatch; uncancellable blocking
workers must not escape quotas when an async caller disappears.

The native client's public wait method is still blocking to its caller, and no
per-request wait-cancel command was added. Disconnect cancels connection-owned
waits together. Native receive queues, total response-byte budgets, client
writer interruption, broader desktop/TUI command responsiveness, public remote
identity/TLS, cross-platform acceptance and sustained-load resource evidence
remain open. No new footprint or latency benchmark is claimed here.
