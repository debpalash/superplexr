# Optional TUI verification inspection

Status: `cargo test -p ultraplexr-tui --release --all-features --locked --offline`
passes: 14 unit tests and 16 integration tests, with the isolated daemon helper
ignored as intended. `cargo fmt -p ultraplexr-tui` completed. Integration tests
use isolated runtimes and outer PTYs; the owner's application was not touched.

`ultraplexr-tui --workflow-read` adds `w` to the existing navigator. Select a
Mission row, or enter a Mission's Sessions, then press `w` for a bounded live
page of up to 64 verifiers. Enter on a verifier reads its compact recorded
status. `n` or Enter on the next-page row reads the next page; `0` reads the first
page. The navigator retains only the current cursor, not an accumulating page
history. Filtering searches only the loaded page; `n` also works when the next
row is filtered out. Each page reports its observed Mission version; multiple
pages do not share a retained snapshot.

`W` opens manual entry of the full verifier Run UUID from a launch result or
existing workflow record. Enter reads that verifier; Esc or Ctrl-C cancels the
draft, Ctrl-U clears it. Bracketed paste
is local, accepts at most 36 ASCII UUID characters and never submits itself.
Invalid/oversized insertions are rejected whole and require an edit or clear
before submission. The optional module is disabled by default.

Discovery and inspection call the same negotiated compact native catalog and
status RPCs as CLI, browser and MCP. Neither fetches a full Mission or enumerates
terminals to inspect a verifier. The runtime's Mission authorization still applies to scoped Shares;
there is no owner fallback and no additional permission implied by the flag.
The existing Mission navigator retains its existing catalog reads and limits.

The display separates recorded execution/outcome, exact candidate revision and
SHA-256, subject disposition, Session identity, matching receipt counts, and at
most 16 receipt summaries. The suggested review step is text, not an action.
Evidence has not been freshly rechecked; successful exit and recorded passing
receipts are not owner acceptance. No launch, collection, settlement, terminal
attachment, Control acquisition or automatic retry is offered by this view.

`r` explicitly refreshes and clears the preceding result before admission;
neither workflow view uses the navigator's two-second refresh timer. `w` returns
to the first catalog page. Backspace from status rereads its catalog page when
present, otherwise returns to the selected Mission's Sessions. Backspace from
the catalog returns to Mission Sessions. Tab
changes navigation scope, and Esc returns to panes (or detaches an empty TUI).
Normal navigator scrolling/filtering and viewport clipping are reused. Outer
terminal font/palette and terminal escape sanitization remain unchanged.

The existing lazy navigation worker owns one active read, one replaceable
pending request and one result slot. Inspection identity and catalog cursor participate in its
generation/scope checks. Editing, leaving and detach discard obsolete results;
they do not join a stalled RPC on the input thread or create another worker.
Cancellation is local/cooperative around the native call, not a hard deadline
or remote request cancellation. A missing/out-of-scope verifier is shown as
unavailable; transport/protocol failures are unconfirmed, with explicit retry.
An oversized native response retains the TUI's fail-closed detach behavior.

The shared native client additionally rejects contradictory compact status
flags/counts: inspection must be observation-only, evidence cannot claim a fresh
recheck, execution completion must match phase, and receipt counts must agree
with truncation and per-receipt check bounds. This affects all compact-status
consumers. Focused tests cover catalog row identity, pagination while filtering,
manual UUID validation, native Mission scope enforcement, unchanged Mission
versions, and cancellation/replacement through the shared worker. Failed
synchronous navigation reads clear previously authorized rows. Paste prompts
survive late input acknowledgements while authority remains valid, and split
pane help retains the active-pane/Control indicators. Native TUI regressions
wait for rendered rows and completed view transitions before sending input.

Deferred work includes workflow editing, receipt
artifact/evidence navigation, authorization/reconnect/cancellation acceptance,
input routing, small-terminal rendering and platform/resource measurement.
