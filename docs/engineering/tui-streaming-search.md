# Incremental TUI history search

Verified locally on macOS arm64, 2026-09-06 IST. This connects the optional TUI
to the [acknowledged search stream](acknowledged-search-streams.md); it does not
create another runtime, terminal process, session database, or search index.

## User journey

From a focused terminal, `Ctrl-] /` opens search. Type or bracket-paste a literal
query and press Enter. Queries are case-insensitive and bounded to 1024 UTF-8
bytes; pasted control characters are removed. Results arrive incrementally.
Arrows or `j`/`k` move selection; Enter loads a retained-history viewport and
pauses the local display. `Ctrl-] l` returns to live output.

`/` edits the query and cancels previous work. Ctrl-C cancels search while keeping
already displayed partial results. Esc cancels and returns to the previous pane
view; `Ctrl-] d` detaches. Prefix navigation/focus commands leave search and
cancel its work before performing their normal operation. Search keys/paste never
become terminal input, even if the focused pane already holds Control. Opening
search does not acquire, force, or transfer Control. Outer-terminal resizing while
search is visible does not resize the host PTY; request Control again to refit it.

The UI uses the existing outer-terminal palette/font, reverse-video selection,
and cell-width clipping. Result labels are one-based physical row and
Unicode-scalar column numbers, not persistent bookmarks or native cell positions.
Search retains no more than 1000 matches and 2 MiB of preview text; reaching a
bound is explicitly reported, with a prompt to narrow the query. This bound
excludes object overhead, frames, native queues and in-flight wire pages.

## Scheduling and lifetime

The subsequent [desktop search milestone](desktop-streaming-search.md) moves
this worker into `ultraplexr-client::history_worker`. The TUI reuses that shared
implementation with the same default 1000-match limit and cancellation semantics.

The TUI lazily creates one search/history I/O worker and one cancellation writer,
reused until detach. There is at most one latest pending command, one update in
the mailbox, and one page held by the producer waiting to publish. Threads are
not created per query. Mailbox locks protect only local state, never socket I/O.
The input loop polls the mailbox without waiting for a search or history RPC.

Every replacement/cancellation increments a local generation, removes queued
output, and wakes the producer. Obsolete generations cannot publish pages,
errors or history frames. Cancellation also covers a request whose start response
has not yet arrived: the worker retires the stream as soon as admission returns.
The separate cancellation writer can wake an already blocked page receiver.
Subsequent work waits for that writer to finish before admission, preserving
same-connection cancellation/start ordering without blocking user input.

Frame-feed continuity changes survive latest-frame coalescing. A reconnect clears
search results and requires explicit resubmission; the search is not replayed.
Revoked access clears the screen and detaches through the existing subscription
path. Worker drop signals shutdown without joining blocked I/O on the UI thread.

History opening checks for the original preview in the returned viewport and
rejects an obviously stale target. This is **not stable-row/epoch validation**:
repeated identical lines remain ambiguous, and pruning/reflow can move physical
coordinates. No precise match highlighting or immutable search snapshot is
claimed. A history RPC already admitted to the server has no cancellation verb;
its result is discarded after cancellation, but the worker can remain occupied
until the RPC completes or times out. Existing non-search TUI commands also
remain synchronous and can delay input on stalled transports.

## Evidence

The full release workspace passed **417 Rust tests**, with six explicitly
ignored fixture/acceptance entries, and the browser/harness suite passed **20
tests**. Release Clippy across all workspace targets/features with `-D warnings`,
workspace formatting and diff-whitespace checks passed. The pre-existing
`block v0.1.6` future-incompatibility notice remains.
The four new real-runtime search tests passed three further consecutive runs.
All five release executables (server, CLI, observer gateway, TUI and desktop)
rebuilt successfully; `ultraplexr-tui --help` also exited successfully. Existing
user desktop/runtime processes were not restarted or upgraded.

Seven new tests cover:

- real-daemon incremental pages, replacement under a backpressured mailbox,
  cancellation, retained-history reads, stale-preview rejection, and unchanged
  live frame, PTY PID and control holder;
- actual TUI keyboard/Unicode query editing, bracketed paste, empty results,
  result selection/history, local cancellation, safe detach and lease return;
- actual TUI search under a scoped observer Share and revocation with the
  host process left running;
- a paused private daemon delaying admission, 100 cancelled/replaced pending
  queries, and only the latest query producing results after the daemon resumes;
- bounded query/result retention, prefix-only search routing, and safe rendering
  through a real outer VT at small sizes, including wide/combining Unicode and
  untrusted control characters.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
node --test crates/ultraplexr-observer/web/control_tests.mjs crates/ultraplexr-observer/web/stream_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

Tests use private runtimes and real outer PTYs, not the user's active desktop or
saved workspaces. This is automated terminal/VT evidence, not a native desktop
visual review or Linux/Windows/SSH acceptance. The earlier resource artifacts
retain their original binary hashes; no new resource budget is certified here.
Desktop/browser incremental search UI, stable history coordinates, nonblocking
metadata/control commands, broader remote security and platform acceptance remain
open.
