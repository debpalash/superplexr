# Independent browser side views

Status: source implemented, unbuilt and untested. Tests, builds, visual/browser
checks, and resource measurements remain deferred at the owner's request.

The browser's **Open side view** action duplicates observation of the selected
Session without creating a process or another owning Session Group. Up to three
side views can remain open beside the main view; they can observe different
Sessions or the same Session more than once. Switching the main selection does
not switch the side views.

Each side view owns one abortable stream, frame decoder, styled renderer, latest
frame, and local pause state. Its controls are Pause/Resume updates, Copy output,
Open in main, Reconnect view after an ending, and Close view. Pausing keeps the
host running and continues bounded latest-frame reconstruction; resuming shows
the latest output. Closing aborts that stream and clears renderer state without
terminating, archiving, or renaming the host Session.

**Open in main** observes that Session through the existing main view. It does
not automatically claim Control, and the read-only side view stays open. Search,
retained-history navigation, and explicit terminal input remain in the main view
for this implementation. The side views are independent live projections, not
full desktop workspace parity or arbitrary nested split layouts.

## Authority and lifetime

- Side feeds send `X-Superplexr-View: observe`. The gateway honors this as a
  per-attachment reduction of authority: it creates no browser input Surface,
  issues no Control nonce, and requires no input heartbeat for that attachment,
  even when the gateway itself accepts a Controller Share.
- This header cannot elevate an Observer Share. Existing Host, Origin, bearer,
  Share, revocation, and stream-admission checks still apply. It is not a new
  credential or a substitute for protecting the existing Controller browser key.
- The side-view module has no command sender or keyboard/resize handlers. If an
  older gateway unexpectedly sends an input Surface, it ends that side view and
  instructs the user to update the gateway rather than keeping an implicit
  Controller-capable attachment alive.
- Each feed starts from a full frame, negotiates the same bounded row-delta
  encoding, and falls back to full-only reconnect after continuity failure.
  Terminal errors clear that view's output and offer explicit reconnect.
- Replacing the browser key, catalog access ending, or leaving the page closes
  all side views and clears their retained frames. Complete catalog snapshots
  and archive updates close views whose Session is no longer present. Partial
  snapshots never prove removal. New side views and opening a view in the main
  pane are disabled while catalog membership is synchronizing.
- No side layout or Session identity is persisted in browser storage. A closed
  view cannot be silently restored by a previous page layout.

## Layout and resources

The existing terminal-focused slate palette and typography are retained. The
main view remains visually distinct from the labeled read-only column. Side
views stack vertically; below 1,100 CSS pixels they move below the main view
rather than shrinking the canonical terminal grid. Each output scrolls inside
its own bounded-height viewport. Changing layout never resizes a host terminal.
Buttons keep visible keyboard focus and text labels; no new animation is added.

The UI caps side views at three. Catalog + main + three side streams consume
five of the gateway's existing eight concurrent read/stream slots, leaving
capacity for explicit history reads and other consumers subject to shared
admission. There is no new polling timer or per-side Control heartbeat. Each
view still costs a subscription, retained frames and DOM state; aggregate memory,
startup, latency, browser connection limits, and throughput remain unmeasured.

Copy output uses the focused view's canonical text unless a browser selection
lies entirely inside that view's output. A selection in another pane cannot
silently replace the requested pane's copied output.

## Deferred acceptance

Future checks must cover same-Session duplication, different Sessions, switching
the main view, no Control identity on side attachments, older gateway rejection,
pause across reconnect and exit, copy isolation, close during startup/backoff,
key replacement, archive/removal/revocation, stream-slot recovery, delta fallback,
keyboard/screen-reader operation, narrow layouts and browser zoom, and measured
four-view resource/streaming costs. Existing single-view evidence does not
validate this implementation.
