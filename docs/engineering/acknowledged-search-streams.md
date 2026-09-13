# Acknowledged search streams

This follows [awaitable history search](awaitable-history-search.md). Search now
has an optional wire-visible page stream and a shared native client interface,
instead of requiring every caller to wait for one collected response.

## Interface and compatibility

The handshake advertises `acknowledged_search_v1`. The client transport preserves
the negotiated feature set when splitting the byte stream. No protocol version
or runtime state directory changed, and old runtimes are not sent unknown search
requests by the new convenience interface.

- `TerminalSearchStart`: a fresh search UUID, Session ID, literal query,
  case-sensitivity and match limit. Its acceptance returns the search UUID and
  a nonzero, independently sequenced stream ID on the existing connection.
- `TerminalSearchAck`: search UUID and the last received page sequence. Only
  that connection's outstanding page can receive credit. Invalid, duplicate or
  premature credit retires that search; it does not advance another stream.
- `TerminalSearchCancel`: idempotent cancellation scoped to the exact connection.
  The response follows task teardown; native work observes cooperative cancellation.
- `SearchPage` frames contain search/session identity, a sequence starting at 1,
  matches, `complete`, and an optional terminal error. Only `complete: true`
  establishes successful EOF. Errors and disconnects never become empty success.

`DaemonSession::search_pages` returns `SearchStream`. Calling `next_page` returns
the first page, then acknowledges the previous page when the caller asks for
another. Dropping the stream or using its cloneable cancellation handle retires
the same connection's job. There is no automatic search replay on reconnect.
The existing `DaemonSession::search` collects this stream when negotiated and
uses the legacy RPC otherwise; existing desktop consumers therefore share the
new implementation without changing their return type.

The CLI exposes incremental JSON lines:

```sh
superplexr --socket /path/to/control.sock terminal-search-pages SESSION_ID QUERY --limit 1000
```

The existing `terminal-search` command retains its single-response format. The
new command also supports the existing owner-only `--share-token-file` mechanism.
Neither search form takes terminal Control or creates a replacement PTY.

## Bounds and lifetime

Each wire page is at most **512 KiB of uncompressed JSON and 64 matches**. Full
Unicode previews are partitioned into additional pages, not silently truncated.
The partitioner accounts conservatively for worst-case JSON escaping. The native
scanner's existing <=64 KiB per-row formatting constraint remains in force.

The server sends one data page, then awaits acknowledgement. The client registers
its bounded queue by search UUID before sending the start request, avoiding an
acceptance/event-registration race. It retains at most one data page plus one
terminal error and discards late pages after cancellation rather than putting
them in the generic early-subscription queue. No result-relay thread is added.

New streams are capped at four per connection and eight per runtime process,
including admitted tasks waiting for credit or a writer. These slots are released
on cancellation/finish. The native actor still admits only one active search per
Session, with its existing five-second deadline. The stream producer/replay has
a 15-second timeout, error delivery gets at most one additional second, and an
outer 17-second guard also bounds acceptance writer queueing. Slow remote links
can therefore fail explicitly; this is not an indefinite resumable search cursor.

Share authority is guarded through admission, page production and writes. Durable
authority is rechecked after writer admission. Revocation/expiry retires the
connection and its search jobs. Connection loss drops its jobs even while a
consumer has withheld acknowledgement. Native objects stay on their owning actor
or replay worker.

Cancelling a search during a partially written frame shuts down the entire
socket. Other streams must reconnect rather than consume corrupted framing.
Cancelling while waiting for the writer or page credit does not itself require
closing an otherwise healthy connection. This protection applies to the new
search writer, not a claim that every older subscription writer has been audited.

## Regressions and test scope

The real-daemon cancel/readmission test initially reproduced a false "search
already active" response. Cancellation could arrive while the actor was parked
after its search poll. Admission now retires cancelled work before checking the
one-job rule, instead of making clients retry.

The full suite also exposed a pre-existing exit-wait ordering race: a native
exit event could reach the wait before the projection thread marked the Session
closed, producing "terminal is already closed and cannot satisfy this wait".
A focused test delaying only projection publication failed five consecutive
times in about 0.06 seconds per run. Exit waits now subscribe to authoritative
summary updates before checking the capture, rather than racing another native
event subscriber. They return a consistent closed-state capture without creating
a forwarding thread. Text/quiet waits retain their existing implementation.

Coverage includes real-daemon/PTY live and archived page equivalence; explicit
cancellation, revocation and disconnect without an ACK; unrelated connections
being unable to grant credit or cancel; duplicate credit; bounded admission
across connections; large combining-mark previews; and an adapter negotiating
without search support. Writer tests use real Unix sockets for partial-write
cancellation and durable revocation while queued. A CLI integration test runs
the actual executable and parses each output line as an independent page.

## Verification

On 2026-09-06 IST, the full release workspace passed **410 Rust tests**, with
six explicitly ignored tests, including eleven new tests for this milestone.
The browser/harness suite passed **20 tests**. Release workspace Clippy with all
targets/features and `-D warnings`, formatting checks for workspace and separately
included server modules, and diff whitespace checks passed. The existing
`block v0.1.6` future-incompatibility notice remains.

After the exit-wait fix, the deterministic projection-order test passed five
consecutive runs and the original real-daemon closed-history scenario passed
ten consecutive runs. Tests used private fixture runtimes; the user's existing
desktop/runtime were not restarted or upgraded.
All five release executables (server, CLI, observer gateway, TUI and desktop)
rebuilt successfully after the fixes.

```sh
cargo test --workspace --release --locked -j 2 --quiet
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
rustfmt --edition 2024 --check crates/superplexr-server/src/search_stream.rs crates/superplexr-server/src/history_search.rs crates/superplexr-server/src/share_request.rs crates/superplexr-server/src/terminal_wait_tests.rs
node --test crates/superplexr-observer/web/control_tests.mjs crates/superplexr-observer/web/stream_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

## Remaining work

- Desktop incremental results/cancel controls and search interfaces in the TUI
  and browser are not implemented by this transport milestone. The desktop's
  existing convenience call still returns a collected vector. UI thread usage
  and native visual QA remain separate work.
  The subsequent [TUI search milestone](tui-streaming-search.md) adds the terminal
  UI and its bounded background workers; desktop/browser UI remains open.
  [Desktop incremental search](desktop-streaming-search.md) subsequently reuses
  that worker through the native client; browser search UI is still open.
- Native result queues still copy full previews per match. The wire-page byte
  budget is not a 512 KiB total search-memory claim, and the process stream cap
  is not a quota on legacy RPCs, terminal retention or all runtime allocations.
- Cancellation cannot preempt an OS read or native call. Cooperative workers can
  outlive task teardown briefly; blocking native client socket writes also remain
  subject to transport behavior and belong on frontend workers.
- Match coordinates remain physical rows and Unicode-scalar columns, not stable
  logical-row epochs or canonical cell coordinates. A live search is incremental,
  not snapshot-isolated. Fallback to archive is allowed only before publishing
  results and after confirming actor death.
- Public remote identity/TLS, a comprehensive transport/security audit, Linux and
  Windows acceptance, sustained-search/resource measurements and full client
  parity remain open. Prior resource artifacts retain their original binary
  hashes; this milestone does not re-certify those measurements.
