# Scoped browser client prototype

Status: optional development executable. Not part of the native v1 release contract.

One runtime owns the terminal; the browser observes canonical text snapshots.
The observer is a separate Rust/Axum executable so native builds do not acquire
an HTTP listener or frontend bundle. Its static HTML/CSS/JavaScript needs no Node
build and uses no third-party scripts, fonts, analytics, or assets.

## Run

Build with `cargo build -p ultraplexr-observer -p ultraplexr-cli`. Obtain an existing
Observer Share token in an owner-only file, or mint one for explicit Sessions:

```sh
(umask 077; target/debug/ultraplexr --socket /absolute/path/control.sock \
  share-create browser --role observer --session SESSION_ID \
  --expires-in-seconds 3600 | jq -r .token > observer.token)

target/debug/ultraplexr-observer --socket /absolute/path/control.sock \
  --share-token-file observer.token
```

Open the private localhost URL printed by the command, select a Session, and
observe its output. Pause updates before marking text if output is changing.
Copy output copies the selected text, or the displayed frame if nothing is selected.
Ctrl-C closes the observer and its feeds; it does not stop the runtime or Sessions.
Revoke the Share with `ultraplexr share-revoke SHARE_ID` on the owner connection.

The default port is ephemeral; `--port` chooses a fixed local port. A browser
reload clears the access key from memory, so reopen the private URL to reconnect.
Transient connection loss automatically reattaches to the same Session with the
same Share and a fresh canonical frame. Denied or revoked access stops retries.
Restarting the gateway rotates its private browser key; reopen its new URL.

Earlier output reads retained host scrollback without changing the shared PTY
viewport. Back to live (or Resume updates) immediately displays the latest cached
frame, even if the terminal has become idle. History retention is bounded by the
runtime; reconnect does not promise replay of every intermediate screen.

Search history accepts literal, case-insensitive queries and streams bounded
results. Cancel keeps partial hits; query edits and reconnect clear them. Opening
a result is read-only and returns Control; Back to live restores the current
frame. Search failures require explicit resubmission. The
[search evidence and limits](../engineering/browser-streaming-search.md) describe
cancellation, authority, byte bounds and the remaining unstable row coordinates.

## Explicit terminal Control

Read-only remains the default. To permit input, mint a **Controller** Share
scoped to the required Sessions and launch with `--allow-control`:

```sh
(umask 077; target/debug/ultraplexr --socket /absolute/path/control.sock \
  share-create browser-control --role controller --session SESSION_ID \
  --expires-in-seconds 3600 | jq -r .token > controller.token)
target/debug/ultraplexr-observer --socket /absolute/path/control.sock \
  --share-token-file controller.token --allow-control
```

Select a Session, choose **Take control**, then focus **Terminal input**. Another
controller must return Control first; the browser cannot force takeover. Keys
go through the existing terminal encoder. Pasting multiline/control text opens
an explicit confirmation. Resize is an explicit owner-of-Control operation,
not a side effect of observing or resizing the browser window.

Return control, pause/history, changing Session, or closing the page disables
local input. A background tab does not gain or steal Control. HTTP detach retires that view's
input identity and attempts to release only its lease. Native-link loss retires
it too; automatic observation reconnect gets a new view with no input authority.
Use **Reattach safely** after an uncertain command response. Inspect output
before repeating the action: the command may already have executed.

The gateway's [connection-pinned input implementation](../engineering/browser-pinned-control-implementation.md)
now binds input and cleanup to the original native lease and lets invalidation
retire it independently of an in-flight command. It also separates activity
expiry from the command lock. This extension is source-only, unbuilt and
untested; earlier Control acceptance does not cover it.

This is a canonical-frame terminal-control proof, not a complete browser terminal
emulator. A bounded styled-cell and cursor projection is now implemented in
source for live output and history, with text-only fallback; it remains unbuilt
and untested. See the [styled rendering implementation](../engineering/browser-styled-terminal-implementation.md).
Browser-reserved shortcuts, full keyboard/IME parity, mouse forwarding, exact
font/cursor fidelity, and richer workspace layouts need more work. Up to three
independent read-only side views are now implemented alongside the main view,
without process creation or automatic Control. This source change remains
unbuilt and untested; see the [side-view controls and limits](../engineering/browser-side-views-implementation.md).

## Security and limits

Share-token loading now uses the source-only
[shared descriptor-based reader](../engineering/shared-token-file-loading-implementation.md),
with effective-user ownership, bounded reads and concurrent-change checks.
It remains unbuilt and untested.

An independent `--workflow-read` option now adds a collapsed, read-only
verification inspector in source. It uses explicit Mission/verifier IDs and
manual refresh, requires Mission-scoped Share access, and adds no launch or
acceptance controls. It remains unbuilt and untested; see the
[workflow inspector implementation](../engineering/browser-workflow-inspection-implementation.md).

- The executable binds only `127.0.0.1`. There is no public listen option.
- The gateway always refuses owner clients. Default mode requires an Observer
  Share; interactive mode requires an explicit flag and a Controller Share.
  Every operation retains that Share; there is no owner fallback.
- A separate random browser access key is carried in the URL fragment, removed
  from the address bar, then sent in an Authorization header. The Share token
  stays server-side. Neither key is stored in browser local/session storage.
- Host/Origin/fetch-site checks, restrictive CSP, no-store, no-referrer, and
  nosniff headers protect the local endpoint. There is no CORS. Framing is denied
  by default; an optional explicit parent-origin policy is now implemented in
  source, but remains unbuilt and untested. See the
  [embedding configuration and limits](../engineering/browser-embedding-implementation.md).
- The sole command route accepts claim/return, key, paste, resize, and heartbeat
  commands for a live browser view. It rejects writes in default mode and
  requires the exact Origin plus the browser access key. No lifecycle, Mission,
  Share administration, force takeover, or generic request passthrough exists.
  Group names are not exposed because group listing is currently owner-only.
- Each SSE attachment has its own random view identity and runtime Surface.
  Control acknowledgements issue a per-claim nonce; commands must carry that
  nonce and an exact increasing sequence. Duplicate, out-of-order, stale-view,
  and stale-lease input is rejected. HTTP writes are never automatically retried.
- At most four command handlers execute concurrently; each view serializes
  commands. Request bodies are capped at 96 KiB, pasted UTF-8 at 16 KiB, grids
  at 400×200, and browser input queues at 128 actions. Input responses time out
  after 10 seconds in the browser and retire that local attachment.
- The source-only [frame admission extension](../engineering/browser-feed-admission-implementation.md)
  retains read/stream slots through cancelled setup and controlled-view cleanup,
  and keeps expiry checks active while waiting for HTTP capacity. This remains
  unbuilt and untested; cleanup can still wait for native I/O.
- Controlled views send a lease-validating heartbeat every five seconds. A view
  without activity for 15 seconds is retired on the next two-second check.
  A blocked native request can delay cleanup; input already in flight cannot
  be undone. Runtime epochs remain the final authority on every mutation.
- At most eight concurrent read operations or feeds. Terminal updates are
  event-driven: the native client reconstructs canonical frames from deltas and
  resynchronizes after disconnect. Each gateway feed coalesces to its latest
  snapshot and has a one-event outgoing queue, so slow browsers do not create an
  unbounded HTTP frame backlog. This is not an end-to-end native queue bound.
- Session-list metadata now uses an event-driven native subscription and a
  browser SSE feed in source, replacing the two-second list polling timer.
  Complete snapshot markers reconcile membership after reconnect. This change
  remains unbuilt and untested; see the
  [implementation and limits](../engineering/browser-session-feed-implementation.md).
  The source-only [shared label extension](../engineering/shared-session-labels-implementation.md)
  adds bounded terminal-title/project labels to that catalog without polling;
  it remains unbuilt and untested.
  Idle terminal frames are not sampled or resent; SSE keepalives maintain the
  connection.
- The bundled browser now opts into changed-row streaming after an initial full
  frame. Grid/style changes and reconnect restore a full base; smaller encoded
  patches carry exact string revisions. This source change is unbuilt and
  untested; see the [row-delta implementation](../engineering/browser-row-deltas-implementation.md).
- Terminal text uses DOM text content, never HTML. Terminal escape sequences are
  interpreted by the existing runtime, not executed as browser markup.
- Revocation cannot retract output already delivered or copied. A blocked RPC
  can delay the next access check up to the native client's request timeout.
- HTTP is appropriate only on local loopback or through an authenticated encrypted
  tunnel. Do not publish this endpoint through a reverse proxy. Public remote
  access, multi-tenant isolation, TLS, general embedded-platform support, and
  mobile authorization need a separate security design and acceptance tests.

## Verification

`cargo test -p ultraplexr-desktop workspace_lifecycle_tests` starts isolated real
daemon processes and exercises two clients, duplicated views, dismissal reload,
detach/reattach, archive/restore, alternate byte transport, HTTP scope checks,
denied writes, and live-stream revocation. It never uses the person's daemon.

The reconnect test forcibly disconnects the native byte relay, produces output
during the outage, reattaches without changing Session ID or PTY PID, retrieves
history, detaches/reattaches the HTTP feed, and verifies subsequent revocation.
`node --test crates/ultraplexr-observer/web/stream_tests.mjs` covers SSE boundaries,
packet limits, browser retry identity/backoff, and permission failures.

`cargo test -p ultraplexr-desktop browser_control_tests -- --test-threads=1`
uses real PTYs to check two-browser contention, owner handoff, stale/duplicate
input, paste confirmation, resize, origin/auth checks, scope, HTTP detach,
forced native-link loss, and Share revocation. `node --test
crates/ultraplexr-observer/web/control_tests.mjs` covers the ordered browser
command lane, lost acknowledgements, retired attachments, queue bounds, keys,
duplicate DOM input events, and IME/clipboard routing. Current automated and
rendered-browser results are recorded in the
[Control evidence ledger](../engineering/browser-control-evidence.md).

On macOS with a GUI login, build the desktop binary and run the explicit native
restart test (it opens two short-lived real windows against an isolated daemon):

```sh
cargo build -p ultraplexr-desktop
cargo test -p ultraplexr-desktop \
  workspace_lifecycle_tests::native_desktop_restart_preserves_personal_state_and_live_sessions \
  -- --ignored --nocapture
```

This tests real startup, render, save, and restart with preseeded names/pins and
dismissals; it does not automate the native rename menus or prove platform parity.
The browser remains plain text: styles, cell-accurate cursor rendering, wire delta
delivery to the browser, and event-by-event replay are deferred.
See [measured restart and streaming evidence](../engineering/observer-streaming-baseline.md).
