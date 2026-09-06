# Bounded canonical history search

This follows [macOS history reclamation](macos-history-reclamation.md). It moves
live search off the actor's synchronous whole-history formatting path, without
introducing another terminal model or moving native references across threads.
It is progress toward T-HISTORY-001, not completion of that contract.

## Implementation boundary

`ultraplexr-terminal::HistorySearch` is an opaque, actor-local continuation over
the canonical Ghostty screen. Each `search_step` attempts at most 32 physical
rows, uses a reusable 64 KiB formatting buffer, and returns at most 64 matches.
Capacity failures yield before retrying a smaller range. A single row exceeding
the buffer returns an explicit error rather than truncating or allocating a
whole-history string. Cached rows and within-row match positions survive yields.
Search does not install a selection or move the client's viewport.

The start captures tracked origin/end references. Appended rows beyond that end
are excluded; edits to unread rows can be observed. This is an incremental live
read, not an immutable snapshot. Pruning the origin, reset, resize, or a changed
active screen invalidates incompatible coordinates. Native ownership validation
rejects a continuation passed to another terminal model.

`SessionHandle::start_search` exposes a single-consumer Rust result stream with
four queued pages, at most one pending actor page, and one active job per Session.
Dropping the stream cancels work. A five-second deadline includes consumer
backpressure; a full queue retries after at most 10 ms rather than spinning.
An empty incomplete scanner batch yields to queued actor messages without
publishing a result. No periodic search work remains after completion/cancellation.
Search reads continue to schedule the existing idle history maintenance.

The existing server request still collects up to 1,000 results; query size is
limited to 1,024 UTF-8 bytes. Its waiting runs on a blocking worker, not a Tokio
async executor thread. Closed Sessions replay their bounded retained journal
and use the same scanner through its synchronous convenience method. Replay is
not a second live terminal authority.

## Shutdown regression

The full-suite test `daemon_registry_controls_and_retains_a_real_pty_session`
exposed a queued search losing its reply stream during actor shutdown. The new
stream correctly reported `SearchIncomplete`, but the server only replayed on
`ActorStopped`, breaking closed-history search. An isolated execution reproduced
the failure in 0.19 seconds.

On an incomplete stream, the server now probes the actor with the existing
snapshot request, on the same blocking worker. Only a confirmed `ActorStopped`
allows replay. A still-live actor, probe timeout, or other error does not turn
an expired search into an automatic journal retry. A disconnected result stream
alone is never proof of successful completion or a stopped Session.

## Verification scope

Terminal integration coverage uses the actual native model: full-history results
and unchanged viewport, dense within-row pagination, Unicode lowercase expansion,
append boundaries, invalidation, wrong-model rejection, empty/oversized queries,
adaptive wide-grapheme reads, and explicit refusal of an oversized single row.
Real-PTY tests exercise input/snapshot responsiveness with a paused result
consumer, bounded admission, cancellation, queue expiry, subsequent search, and
process exit without waiting for a consumer to drain.

The release workspace suite passed **381 Rust tests**, with six explicitly
ignored tests, on 2026-09-06 IST. The browser/harness suite passed **20 tests**.
The original closed-Session regression also passed ten consecutive isolated
runs after the fix. Workspace release Clippy with all targets/features and
`-D warnings`, Rust formatting, and diff whitespace checks passed. The existing
`block v0.1.6` future-incompatibility notice remains.

Reproduction from the repository root:

```sh
cargo test --workspace --release --locked -j 2
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
node --test crates/ultraplexr-observer/web/control_tests.mjs crates/ultraplexr-observer/web/stream_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

All five release executables rebuilt successfully. The unchanged version-2
[five-minute reference sample](shared-client-bounded-search-release.json)
completed on Apple M2 / 8 logical CPUs / 16 GiB RAM, using twelve 80x24 Sessions
with 100,000 retained short ASCII history rows each, one Observer desktop,
one real-PTY TUI, and one HTTP/SSE observer. No build/test/dependency job overlapped
the measurement. The user's existing applications remained running.

| Observation | Measured | Target |
|---|---:|---:|
| Idle window | 300.053 s | >=300 s |
| Runtime idle CPU, one core | 0.320% | <=0.5% |
| Desktop idle CPU, one core | 0.380% | <=1% |
| Runtime + desktop sampled peak RSS | 107.63 MiB | <=600 MiB |
| Unexpected idle SSE frames | 0 | 0 |
| Idle TUI output bytes | 0 | 0 |

All six harness observations passed. Runtime peak RSS was 18.56 MiB; desktop
peak RSS was 89.11 MiB. The combined peak uses coincident samples. Gateway
peak RSS was 4.89 MiB and TUI peak RSS was 2.52 MiB, reported separately from
the runtime-plus-desktop budget. All twelve oldest-history markers, grids and
Session/PTY identities survived the pre/post-idle checks. CLI-to-SSE p95 was
68.71 ms with 50 ms observation granularity, not input-to-pixel latency.

The raw artifact records binary hashes/sizes, samples and host details. The
fixture revoked its Share, terminated its own Sessions/clients and removed its
private tree; the user's existing desktop/runtime stayed alive. Reproduction
and measurement exclusions are in the [resource ledger](shared-client-resource-evidence.md).
This is a local idle reference pass, not a sustained-search, rendered-browser,
startup-allocation, whole-machine or cross-platform performance certification.

## Remaining requirements and limits

The subsequent [awaitable-search implementation](awaitable-history-search.md)
replaces the blocking live collector and adds cooperative replay cancellation.
The following describes this milestone's original boundary; wire/client paging,
byte budgets and coordinate semantics remain unfinished.

- Result pages and cancellation are not yet exposed over the wire or integrated
  into all three clients. Disconnecting an RPC does not immediately cancel its
  blocking collector; the actor job has a five-second deadline.
- Match columns retain the existing Unicode-scalar convention, not canonical
  cell coordinates. Retention still counts physical rows, not logical wrapped
  lines. Stable absolute history epochs/truncation markers remain open.
- Row/byte/result limits bound individual scanner work and retained buffers,
  not strict wall-clock latency. Native point lookup can traverse history pages.
  General command-queue admission and archived journal replay have their own
  costs/bounds; the job deadline is not a whole-RPC latency guarantee.
- Previews are copied per match. A near-64-KiB row with many matches can consume
  materially more memory than the formatting buffer, including up to 1,000
  previews in the compatibility response. A byte-budgeted wire page is still
  needed; the buffer size is not a claim of total per-search memory usage.
- Case-insensitive matching uses Unicode lowercase, not full Unicode casefold
  or regex. Reads are live and incrementally cached, not snapshot-isolated.
- The subsequent [in-flight Share authority milestone](in-flight-share-authority.md)
  guards shared RPC computation/response writing and strengthens lease identity.
  Cancellation down into blocking search jobs and a comprehensive transport
  security audit remain open; this search milestone alone did not prove them.
- Sustained-output/search and wide/styled-history performance, native visual
  QA, and physical Linux/Windows acceptance remain separate evidence gates.
