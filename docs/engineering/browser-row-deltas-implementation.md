# Browser changed-row streaming

Status: source implemented; unbuilt and untested. Tests, builds, browser runs,
and performance/resource measurement remain deferred at the owner's request.

The bundled browser opts into `row-delta-v1` using the authenticated frame GET's
`X-Ultraplexr-Frames` header. Callers without the header keep full-frame SSE.
This is an optional browser transport encoding, not a native protocol-version
change, input channel, durable resume token, or new runtime authority.

## Wire and recovery

Each HTTP attachment starts with a full `frame`. Full frames retain existing
fields and add a decimal-string `revision`, preserving exact native u64 sequence
identity beyond JavaScript's safe integer range. The numeric sequence remains
available for existing display-only consumers.

When compatible, subsequent `frame-delta` events contain the exact base and next
revision, current title/directory/status/cursor, and changed rows with canonical
plain text plus styled runs. Grid size, row count, style-table or default-color
changes force a full frame. Text-only projections and completion also use full
frames. The server serializes the candidate patch and full frame and sends the
patch only when its JSON byte count is smaller. This comparison costs extra
serialization; CPU and net memory/network improvements have not been measured.

The producer computes each patch against the previous snapshot queued by that
same ordered HTTP attachment. Native latest-state coalescing is retained; the
browser does not require every intermediate native sequence. Its next revision
must be greater than the exact base, not necessarily consecutive. No global
patch log or per-client replay history is introduced. Only opt-in feeds retain
one previous snapshot for patch construction.

Native reconnect advances an internal continuity generation. The next snapshot
therefore cannot patch the old base even if coalescing hides the reconnect
notification. HTTP reconnect creates a new producer and full base. Input views
still retire on reconnect and cannot inherit Control from an old connection.

The browser builds a prospective patched frame without mutating its base or any
displayed history. It validates revisions, unique/in-range row indices, row and
run bounds, aggregate styled text/run limits, and resulting plain-text size
before replacing its current frame. Live reconstruction continues while local
history is paused; returning live uses the newest reconstructed frame.

Missing or invalid continuity retires local input and retries read-only with
delta negotiation disabled for that selection. If full-frame recovery also
fails frame admission, the view ends and requires explicit reconnect rather
than looping indefinitely. Fresh input claim is unavailable until a new frame
arrives. No keys, paste, resize, or claim commands are replayed.

## Unchanged boundaries and deferred acceptance

Same-origin/bearer/Share checks, eight-read stream admission, one-item HTTP
handoff, native receive limits, shutdown, and access revocation remain in force.
Revision strings are data continuity identifiers only; they grant no access.
No compression library, WebSocket service, browser persistence, new timer, or
third-party frontend dependency is added.

This is not a measured bandwidth, latency, or tiny-footprint claim. A patch can
save wire bytes while costing additional serialization and retaining another
snapshot. Sustained full-screen churn may continue to send full frames. Terminal
input fidelity and multi-pane browser layout remain separate work.

Deferred acceptance must cover exact reconstruction through narrow/wide styled
row changes, cursor-only changes, title/status changes, empty rows and grid/style
replacement, out-of-range/duplicate/malformed patches, large u64 revisions,
coalesced native reconnect, HTTP reconnect and full-only fallback, paused history,
revocation and input fencing, backpressure, old full-only consumers, and measured
CPU/RSS/network costs under sparse and full-screen updates. Prior full-frame
evidence cannot certify this implementation.
