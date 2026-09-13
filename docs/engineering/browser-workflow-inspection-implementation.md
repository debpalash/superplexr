# Optional browser verification inspector

Status: source implemented; JavaScript request/DOM-adapter tests pass. Real
browser/visual checks and resource measurements remain separate acceptance work.

The browser gateway accepts `--workflow-read` independently of terminal Control.
It exposes authenticated GETs for a bounded verifier list and Mission/verifier status, and advertises
the module through the existing session feed. Disabled gateways reject direct
requests and do not show the inspector. Owner connections remain prohibited;
the native runtime requires the Share's Mission-read scope. Session-only Shares
do not gain Mission access from this flag.

The UI reuses the existing palette/type system in a collapsed Verification
inspector. A Mission UUID and **List verifiers** explicitly request up to 32
entries, ordered by verifier Run UUID. **Next page** reads after the returned
cursor and replaces the previous page; the browser never accumulates pages.
The endpoint accepts limits from 1 to 64. Each entry shows verifier and subject
Run IDs, execution phase/outcome, subject disposition, and an optional primary
Session ID. Selecting **Inspect** reads that verifier's existing compact status.
Direct Mission and verifier UUID entry remains available. Execution,
recorded receipt counts, and subject disposition remain separate facts, followed
by the frozen Candidate identity and up to 16 receipt summaries. No inferred
green completion state combines them. Evidence is explicitly not rechecked.
There are no launch, retry, collection or acceptance controls.

Inspecting sends one request, with a ten-second browser timeout and a 64 KiB
response-read cap for both list and status reads. The current page's Mission
version is shown as an observation; separate pages are not a retained snapshot.
There is no background poll or automatic retry. ID edits,
closing the inspector, reconnect, access-key changes and page exit cancel or
invalidate the current read; late responses cannot replace another view's
result. Closing/disable clears displayed data, and disabling the module clears
entered IDs. Nothing is persisted in browser storage. Labels/results use text
nodes, never HTML; the runtime's next-action enum is not an executable control.

The gateway uses the compact native status request with no whole-Mission
fallback. It holds one existing read permit through native work even if HTTP
is cancelled. A missing runtime capability requests an update, denied scope
remains denied, and no owner fallback is attempted. Existing Host, access-key,
fetch-site, CSP, no-store and embedding rules remain. The static script is public
like other bundled scripts; the data route is not.

This does not add Mission discovery, agent creation, browser workflow editing,
or live workflow streaming. The owner must provide the Mission ID. Status is a
point-in-time record, not proof of current artifact contents or platform readiness.

`node --test crates/superplexr-observer/web/workflow_tests.mjs` exercises manual
discovery and direct-ID form submission, page replacement and selection,
malformed/inconsistent JSON, DOM and byte limits, streamed-body cancellation,
ten-second timeouts, late responses, HTTP failures, feature disable, and
GET-only requests. Tests use a small DOM adapter and native streamed Response
objects; they are included by the existing CI JavaScript test glob.

Deferred browser acceptance includes keyboard access, narrow/zoomed layouts,
and real browser resource costs. Gateway/runtime scope, revocation,
capabilities, and admission require their own integration coverage.
