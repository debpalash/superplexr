# Desktop incremental history search

This follows [acknowledged search streams](acknowledged-search-streams.md) and
[TUI search](tui-streaming-search.md). Desktop and TUI now share the same bounded
background search/history worker in `superplexr-client::history_worker`, without
a desktop dependency on the TUI or another session/index database.

## Interaction and scheduling

The existing sidebar query still filters session metadata locally. Two or more
query characters also start literal, case-insensitive history search after a
200 ms debounce. Editing immediately cancels old work and clears old hits; it
does not wait for the debounce before retiring the previous generation. Queries
are bounded to 1024 UTF-8 bytes. Progress, completion, errors and limits are
displayed explicitly, and Cancel stops further work while retaining partial hits.
The results panel scrolls within a 320 px height bound, so all retained hits are
reachable without consuming the entire sidebar.
Clearing/Escape, closing search or changing the workspace/session selection
retires pending work. Search errors clear partial hits rather than appearing as
an empty successful result.

Targets are deduplicated by terminal Session ID within the current workspace.
The worker searches one terminal at a time, requesting up to eight matches each;
the UI retains at most 24 hits and 2 MiB of preview text. Pages are applied as
they arrive, including pages smaller than the requested match limit. A 40 ms
GPUI timer checks the mailbox only while work is pending; completed/idle searches
do not keep polling. Two worker threads are reused per desktop owner, not spawned
per edit. No search, ACK, cancel or result-history socket operation runs on the
GPUI thread.

Results carry SessionGroup ID, terminal Session ID and surface slot. Rendering
and activation resolve current membership again, within the original workspace.
Reordering/deleting a sidebar row or replacing a terminal cannot make an old hit
index a different row or surface. This is distinct from terminal history-row
stability, which is still not guaranteed.

## Read-only history

Opening a hit uses the shared worker's independent history read and stale-preview
check. It does not scroll the live shared terminal. The surface shows a local
read-only viewport with a return banner; Escape or clicking the banner restores
the latest retained live frame. Terminal input/paste and host resize are blocked
while that viewport is displayed. Live frame/delta updates continue into a
separate latest-frame slot, and automatic title/directory labels continue to use
that live state. Copy uses the displayed history/selection.

The existing desktop delta-event path now requests explicit reconnect/rejection
notifications from the native client. A continuity failure cancels sidebar search
and dismisses the search viewport; rejection also clears displayed cells and
disables input. This does not constitute a comprehensive desktop transport audit:
the older generic event queue remains unbounded, hidden/historical surfaces do
not maintain the same live subscription coverage, and full application-wide
revocation/identity behavior still needs broader acceptance.

## Limits and compatibility

- Incremental search requires a runtime advertising `acknowledged_search_v1`.
  Older runtimes produce an explicit search error; this UI does not fall back to
  a blocking collected search. The native client's older collected interface
  and legacy CLI command remain available separately.
- Coordinates are physical rows and Unicode-scalar columns, not stable logical
  rows/cell columns. Preview comparison catches obvious stale targets but cannot
  distinguish identical repeated lines. No immutable history snapshot or precise
  match highlighting is claimed.
- An admitted history read cannot be preempted. Cancellation discards its result
  but the worker can remain occupied until the RPC finishes or times out.
  Previously admitted terminal input cannot be rolled back by opening history.
- The 2 MiB preview bound excludes frames, object overhead, native/wire queues and
  target references. Previous resource artifacts are not measurements of these
  new binaries.
- Other desktop metadata/control actions, archive scrolling, complete IME and
  accessibility acceptance, remote-link behavior and native visual QA remain
  separate work. Browser search UI and full platform parity are not implemented
  by this milestone.

## Verification

On 2026-09-06 IST, the full release workspace passed **423 Rust tests**, with six
explicitly ignored fixture/acceptance entries; the browser/harness suite passed
**20 tests**. Release Clippy across all workspace targets/features with
`-D warnings`, workspace formatting and diff-whitespace checks passed. The
pre-existing `block v0.1.6` future-incompatibility notice remains.
The six new desktop search tests and four existing real-TUI search tests passed
three further consecutive runs. All five release executables (server, CLI,
observer gateway, TUI and desktop) rebuilt successfully after the final changes.

Six new tests cover:

- real-daemon partial pages across three terminals, the 8-per-terminal/24-total
  result bounds, independent retained-history opening, and unchanged live frame,
  process identity and Control holder;
- replacing a query/workspace, explicit Share rejection on the exact event
  interface consumed by the desktop, and removal of stale/unauthorized hits;
- GPUI read-only history blocking paste and resize, copying Unicode, restoring
  the previous frame, and clearing it on rejection;
- actual GPUI layout of the return banner and Cancel control, including removal
  after dismissal;
- stable session-group lookup after row reordering, and rejection after surface
  replacement, removal or membership changes.

The initial streaming test incorrectly assumed a page contained all eight
requested matches; native pages may be smaller. It now checks early partial
results and separately verifies the final per-terminal and total bounds. GPUI
layout assertions use the project's existing debug-selector mechanism.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
node --test crates/superplexr-observer/web/control_tests.mjs crates/superplexr-observer/web/stream_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

Tests use isolated fixture runtimes and GPUI's test context; existing TUI
real-PTY tests now cross the same extracted worker implementation. These are
local macOS results, not physical native-window, Linux/Windows or authenticated
remote-link certification. The user's active desktop/runtime and saved workspace
state were not restarted, upgraded or rewritten.
