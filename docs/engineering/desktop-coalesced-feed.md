# Desktop coalesced terminal feed

The desktop's native-event callback previously sent every terminal delta and
notice into an unbounded Tokio channel. Limiting a UI drain to 256 events did
not limit the queue. A slow render executor could accumulate terminal output and
delay reconnect/rejection notices behind obsolete frames.

## Implemented boundary

`terminal_feed` replaces that FIFO with one canonical frame, four fixed notice
slots, and a one-slot unit-wakeup channel. It runs on the existing native event
thread; no new relay thread, timer, socket or terminal is introduced.

Every ordered delta is applied to the current frame before the next event is
accepted. GPUI receives only the latest resulting frame. An `Arc` snapshot keeps
an already displayed/taken frame immutable; copy-on-write is needed only when
another consumer still holds that frame. The mailbox does not clone a full
snapshot merely to enqueue each repaint notification.

The four notice slots represent bell count, latest paste confirmation, termination
escalation and final outcome. Bells accumulate with saturating arithmetic; paste
prompts replace older pending prompts. Exit/failure retains its outcome and
retires pending paste/escalation state. The surface also clears already displayed
paste confirmation and uncommitted composition when it receives an outcome.
This is a current UI projection, not an audit log of every intermediate notice.

Reconnect clears the mailbox's frame and transient notices and sets a sticky
continuity-loss flag. A following full snapshot cannot erase that flag before
the UI sees it. Rejection clears data and ends publication. Wrong Session IDs,
backwards full snapshots and invalid/missing delta bases also fail closed with
an explicit reattach error; no partially applied frame is published.

Every invalidation advances a shared continuity counter. A batch already taken
from the mailbox but waiting on GPUI checks that counter before being applied;
stale batches are skipped while the consumer continues to the invalidation
notice. This is a pre-application check, not atomic retraction of an update
already being applied or pixels already displayed.

Each desktop presentation attachment additionally has a local generation.
Hiding/reattaching changes it. Updates from old tasks cannot paint a hidden
surface or a newer attachment. A current reconnect/rejection still closes local
search history and disables writable state; rejection clears terminal cells.
Normal coalesced frames update the latest live frame behind an open read-only
search preview.

## Bounds and deliberately open audit items

The event-to-GPUI boundary now retains a fixed number of objects regardless of
the number of incoming events. It is **not** a total process or transport memory
bound: a frame/notice has variable size, consumers can hold snapshots, and other
layers have their own buffers.

Read-only inspection identified these separate remaining gaps:

| Boundary | Current code evidence | Follow-up |
|---|---|---|
| Native wire subscriptions | The subsequent [native receive milestone](bounded-native-receive.md) bounds ACK-registered mailboxes. [Pull-driven metadata](pull-driven-metadata.md) removes native metadata relay FIFOs and bounds desktop/CLI handoffs. | Complete collection reconciliation, oversized snapshots, decoded/response budgets and long-lived stream bookkeeping remain open. |
| Desktop input worker | The original unbounded channels/failure continuation are replaced by the subsequent [bounded input milestone](bounded-desktop-input.md); receive overflow now interrupts blocked socket writes. | Total worker/transport budgets and remaining blocking caller paths remain open. |
| Desktop Control synchronization | The subsequent [input milestone](bounded-desktop-input.md) removes metadata-triggered claims, checks notification epochs and requires explicit re-arm. | Broader authority/transport teardown and reconnect/resize UX remain separate acceptance work. |
| Hidden/archived surfaces | Live subscriptions are stopped while hidden and are not opened for historical surfaces. | Application-wide authority invalidation and fresh scoped reads before presenting retained private state. |
| Detach/cancel I/O and ownership | Native subscription cancellation still performs a blocking socket write, and dropping its handle alone intentionally leaves the subscription active. | Worker-owned bounded cancellation/transport teardown and explicit surface-destruction coverage; this mailbox adds no network writes on GPUI. |

The native callback still has to process incoming data before it can deliver
revocation to the GUI. The later native receive milestone invalidates a
connection-pinned input lease independently of that callback when its wire
retires, and clears the generic upstream mailbox. Higher-level queues,
application-wide authority/teardown, platform acceptance and production remote
identity remain release work.

## Verification

Seven new tests cover:

- 2000 real-model deltas with one pending wakeup and exact canonical-frame
  equality, followed by idle silence and consumer-drop retirement;
- sticky reconnect, revocation invalidating already-taken batches, and rejection
  of later frames;
- bounded notice coalescing, correct bell totals/latest paste and retained exit;
- wrong identity and invalid delta rejection without publishing partial data;
- actual GPUI application of fresh versus stale/hidden attachments, local search
  preview behavior, revocation clearing, and exit clearing paste/composition;
- a real daemon with 200 PTY writes while the UI receiver does not drain, exact
  latest-frame equality, unchanged PID/Control holder and observable exit;
- an alternate byte transport with a forced outage/reconnect while the UI does
  not drain, preserved Session/PTY identity, retained continuity, revoked Share
  rejection and continuing owner input.

On 2026-09-06 IST, the full release workspace passed **434 Rust tests**, with six
explicitly ignored fixture/acceptance entries; the browser/harness suite passed
**28 tests**. The seven new tests passed three additional consecutive runs.
Workspace release Clippy across all targets/features passed with `-D warnings`,
as did formatting and diff-whitespace checks. All five release executables
rebuilt. The existing `block v0.1.6` future-incompatibility notice remains.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
node --test crates/ultraplexr-observer/web/*_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

These tests do not provide physical native-window QA or Linux/Windows acceptance.

## Fresh release resource observation

The unchanged shared-client harness completed a 300.072-second idle sample after
this build on the Apple M2/macOS host. Twelve Sessions each retained 100,000 short
ASCII history rows at 80×24, with an Observer desktop, one TUI pane and an HTTP
gateway/feed. No build or test job overlapped the measured interval. The user's
other applications remained running; this was not an exclusive lab host.

| Observation | Measured | Harness target |
|---|---:|---:|
| Runtime idle CPU, one core | 0.310% | <= 0.5% |
| Desktop idle CPU, one core | 0.373% | <= 1% |
| Runtime + desktop sampled peak RSS | 162.59 MiB | <= 600 MiB |
| Unexpected idle SSE frames | 0 | 0 |
| Idle TUI output bytes | 0 | 0 |

All six observations, including sample duration, passed. Grids, retained history,
Session IDs and PTY PIDs were verified intact. CLI-to-SSE echo p95 was 67.77 ms,
with 50 ms observation granularity; it is not input-to-pixel latency. Raw samples,
binary sizes/hashes and exclusions are in
[the release artifact](shared-client-coalesced-feed-release.json).

The gateway peaked at 6.63 MiB RSS and the TUI at 3.70 MiB. Browser/compositor,
outer terminal and benchmark-driver resources are excluded. One visible desktop
Surface and idle short-ASCII history do not establish sustained-output/search
throughput, startup budgets, all-viewer fan-out, minimal embedded suitability or
cross-platform acceptance. The mailbox correctness tests cover stalled delivery;
this resource run separately covers the existing idle reference workload.

The harness retired its own fixture Sessions/processes and temporary state.
The user's running desktop/runtime processes and saved workspaces were not
restarted or rewritten.
