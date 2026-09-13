# Awaitable history search and cooperative cancellation

This follows [in-flight Share authority](in-flight-share-authority.md). The
server no longer needs a blocking worker to wait for live search result pages.
This is a prerequisite for client-visible paging, not that feature's completion.

## Implementation

`SessionSearch::next_page_async` awaits the existing four-page native result
queue without adding a runtime dependency, forwarding thread or polling timer.
The receiver remains single-consumer and exclusively borrowed across the wait.
Its waker registration rechecks the queue to cover concurrent publication;
sender teardown disconnects before waking. Dropping a pending page future alone
does not cancel the search, so a caller can resume with another waiter.

`SearchCancellation` is an irreversible, cloneable token checked before command
admission and by the actor before starting/continuing native work. Dropping the
search receiver still cancels. The server also owns a cancellation-on-drop scope
outside its five-second live-search timeout, covering admission that is queued
on a blocking worker when the request future is cancelled. Native terminal
objects and tracked history references remain on their owning actor.

Archived searches own a separate replay model on a blocking worker and send at
most one queued page. Dropping the consumer cancels journal parsing/scanning and
unblocks a full result queue. Cancellation is checked before file access and
between 64 KiB replay chunks and native search steps. A search replay that
exceeds its replay deadline now returns an error instead of reporting successful
search results from a partially reconstructed model. Startup recovery retains
its existing bounded partial-frame behavior. The archived request has a
15-second overall timeout, including queueing and collection.

The existing RPC still collects at most 1,000 matches. An incomplete live job
does not trigger archived replay unless a separate actor probe confirms the
actor has stopped. No wire version, state directory, or client lifecycle changes.

## Test coverage

- Notification seam: publication, replaced waiters, sender teardown, buffered
  results after cancellation, receiver-drop cancellation, and 200 concurrent
  send/disconnect trials. A missed notification fails rather than triggering a
  timer-driven repoll that could hide the bug.
- Real PTY: asynchronous page collection, cancellation with the old receiver
  still alive, successful readmission, rejected pre-cancelled admission, unchanged
  Session ID / PTY PID, and working terminal input after search.
- Real archived metadata/journal/native model: full paged collection, cooperative
  retirement of a backpressured worker without aborting it, cancellation before
  filesystem access, and strict search-replay deadlines without changing recovery.
- Existing real-daemon closed-history and Share-revocation regressions remain
  necessary integration coverage.

## Verification

On 2026-09-06 IST, the full release workspace passed **399 Rust tests**, with
six explicitly ignored tests, including eight new search tests. The
browser/harness suite passed **20 tests**. Workspace release Clippy with all
targets/features and `-D warnings`, formatting checks (including the separately
included server modules), and diff whitespace checks passed. The existing
`block v0.1.6` future-incompatibility notice remains.

```sh
cargo test --workspace --release --locked -j 2
cargo clippy --workspace --release --all-targets --all-features --locked -j 2 -- -D warnings
cargo fmt --all --check
rustfmt --edition 2024 --check crates/superplexr-server/src/history_search.rs
node --test crates/superplexr-observer/web/control_tests.mjs crates/superplexr-observer/web/stream_tests.mjs ci/pty-host-tests.mjs ci/resource-samples-tests.mjs
```

All five release executables (server, CLI, observer gateway, TUI and desktop)
rebuilt successfully. The user's existing desktop/runtime processes remained
alive and were not restarted or upgraded. Tests used private disposable sessions
and archives.

## Remaining boundaries

The subsequent [acknowledged search streams milestone](acknowledged-search-streams.md)
adds negotiated wire pages, byte bounds, explicit cancellation and a shared
client/CLI interface. The following records this implementation's original
boundary; dedicated incremental search UI and broader acceptance remain open.

- Wire-visible bounded pages, byte budgets, explicit client cancellation and
  integration into desktop/TUI/browser remain open. The compatibility response
  still duplicates full-row previews per match; page count is not a total-memory
  budget.
- Share revocation/expiry and request timeouts drop the guarded operation.
  Merely losing a transport is not a new immediate cancellation guarantee while
  its connection handler is awaiting a request.
- Cancellation is cooperative: it cannot preempt an OS read or a native call.
  Actor admission and the stopped-actor probe can still use bounded blocking
  workers. Archived parsing necessarily remains off the async executor.
- Physical/logical row semantics, cell coordinates/history epochs, platform
  acceptance and sustained-search resource measurements remain separate work.
  Earlier performance artifacts describe their recorded binaries, not this edit.
