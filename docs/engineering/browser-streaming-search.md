# Browser incremental history search

The optional localhost browser client now consumes the same canonical native
search pages as the CLI, desktop and TUI. It does not maintain another terminal,
search index, or session store. This follows
[desktop search](desktop-streaming-search.md) and
[acknowledged native streams](acknowledged-search-streams.md).

## Interaction

Select a Session, enter literal case-insensitive text in Search history, and
press Enter or Search. Results arrive incrementally. Cancel retains explicitly
partial results; editing, changing Session, reconnecting or losing access clears
them. Search is never automatically replayed. An incomplete stream is an error,
not a successful empty or partial result set.

Opening a hit pauses only this browser's display, disables local terminal input,
and returns any Control lease. History is fetched independently; the host's live
frame, grid, process and other clients are unchanged. Back to live restores the
latest received frame. Queries, previews and history are not terminal input.
Results use text nodes, so terminal-produced HTML is displayed as text.

## HTTP and lifetime boundary

- `POST /sessions/{id}/search` carries the query in JSON, never in the URL.
  The existing exact Host/Origin, fetch-site and browser-key guards apply.
  The scoped native Share remains the authority; no owner fallback exists.
- Native admission errors occur after HTTP 200 and are explicit `search-error`
  SSE events. Gateway guard/validation/capacity failures use HTTP status codes.
  The client accepts only ordered `search-page` events for one Search/Session,
  and only an explicit final page proves completion.
- The gateway permits four searches. Each owns one-slot native-to-relay and
  relay-to-HTTP queues, plus the producer/relay's held pages and native queues.
  Native pages remain bounded to 64 matches/512 KiB. HTTP is not an additional
  application-acknowledged protocol; browser/network buffers are separate.
- Browser body detach and gateway shutdown stop the relay. It drops the native
  receiver first and cancels the exact native search on a blocking worker.
  Cancellation before native admission is remembered. Permits remain held by
  outstanding producer/cancel workers so slow native I/O cannot create an
  unlimited population of replacement jobs.
- An active-only 100 ms check detects closure of that exact native transport
  even while the HTTP consumer is backpressured. Search has a 25-second relay
  deadline and a 30-second browser deadline; no idle search timer remains.
- `GET /sessions/{id}/history-line/{row}` is a separate scoped read under the
  existing eight-read/feed semaphore. Its admitted blocking RPC cannot be
  preempted, but late browser results are discarded after cancellation.

Queries are limited to 1024 UTF-8 bytes and requested matches to 1000. The browser
retains at most 1000 matches/2 MiB of preview text and explicitly reports display
limits. These bounds exclude DOM/object overhead, frames, sockets and native
storage; they are not a total browser-memory guarantee.

Coordinates are still physical rows and Unicode-scalar columns. Comparing the
original preview with returned history rejects obvious stale rows but cannot
distinguish repeated identical lines. Stable history epochs, cell coordinates
and precise match highlighting remain open. Already delivered bytes and terminal
input cannot be retracted by a later revocation or history-open action.

## Executed verification

On 2026-09-06 IST, the full release workspace passed **427 Rust tests**, with six
explicitly ignored fixture/acceptance entries. The browser/harness suite passed
**28 tests**. Release workspace Clippy across all targets/features passed with
`-D warnings`; the existing `block v0.1.6` future-incompatibility notice remains.

Four new real-daemon HTTP tests cover canonical-result equivalence, independent
history reads, unchanged live rows/PID/Control, authentication/origin/scope/query
bounds, four-search admission, capacity recovery after body detach, shutdown
admission rejection, and revocation of a backpressured stream without successful
completion or PTY termination.
All four passed three additional consecutive runs against isolated runtimes.

Eight new JavaScript controller tests cover incremental split-UTF-8 delivery,
identity/order errors, incomplete EOF without retries, late replaced fetches,
active-read cancellation, query/preview byte bounds, read-only history ordering,
stale rows/late frames, and authority versus capacity failures. They run in the
macOS CI verification step alongside existing browser/harness tests.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
node --test crates/superplexr-observer/web/*_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

Rendered acceptance can use `node ci/browser-search-fixture.mjs` after building
the release server, CLI and observer. It prints a private URL for a disposable
Controller Share and seeded terminal. Enter or termination retires only its own
Session/processes and generated state. It never attaches to the user's runtime.

All five release executables (server, CLI, observer, TUI and desktop) rebuilt.
Formatting, diff-whitespace and JavaScript syntax checks passed.

### Rendered browser acceptance

An isolated ego-browser/Chromium taskspace exercised the actual release gateway
and a disposable Controller Share on macOS, at 1228 px desktop width and a
390×844 emulated mobile viewport. Screenshots and DOM readback confirmed:

- a ten-hit incremental query, a one-hit Unicode/HTML-like preview rendered as
  plain text (zero script elements in results), and the explicit 1000-hit limit;
- explicit Control acquisition, history opening at `needle 0000`, input disabled
  and Control returned, then Back to live restoring `needle 1179` without
  reacquiring Control;
- cancellation with an 800 ms/30 KB/s throttled connection, followed by a new
  successful query;
- document width equal to viewport width at both sizes; results scroll within
  their bounded panel;
- Share revocation clearing results immediately on continuity loss, followed by
  confirmed denial clearing displayed output and disabling query/input controls.
  The first reconnect interval can still display the previously received frame;
  this is not instantaneous retraction of already delivered information.

Opening history focuses its output, which scrolls the page. The initial automated
Back to live clicks targeted an off-screen location; a capture-phase click trace
identified their target as HTML, not the button. Explicitly scrolling the toolbar
into view and clicking the actual button restored live output. This is recorded
to distinguish an automation miss from a claimed application failure; keeping
history-return controls near the viewport remains an ergonomic improvement.

The browser taskspace was closed and the fixture's own terminal, processes and
generated state were retired. Existing user desktop/runtime processes and saved
workspaces were not restarted or changed. These local results do not certify
public remote collaboration, Linux/Windows, full browser-terminal parity or new
resource budgets. Earlier resource artifacts measure earlier binaries.
