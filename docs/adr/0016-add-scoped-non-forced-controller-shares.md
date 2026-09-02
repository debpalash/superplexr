# Add scoped non-forced Controller Shares

Observer remains strictly read-only. A new `controller` Share role inherits only
scoped reads and may interact with explicitly scoped terminal Sessions through
the existing Control lease protocol. It may claim when unoccupied with
`force=false`, release, and send terminal input, focus, mouse, scroll, selection,
selection-text, clear-selection, paste, and resize operations under the current
Control epoch.

Controller is intentionally not an editor, runtime operator, or process owner.
It cannot force takeover; create, interrupt, terminate, kill, archive, or restore
a process; mutate the Mission graph; launch or schedule Runs; inspect runtime
diagnostics; or administer Shares. This is a separate authorization allowlist,
not a widening of Observer.

The protocol exposes authenticated Share role metadata through `ShareIdentity`
so clients select the correct interaction model without inferring permissions.
Terminal projections record `controller_share_id` beside the controlling client
and Surface. Connection loss releases that client's leases. Share revocation
also scans and releases every lease held by that Share, increments each Control
epoch, publishes the new terminal projection, and ends scoped streams. A missed
revocation event fails closed. A runtime expiry reaper durably promotes elapsed
capabilities into this same revocation path, including after daemon restart.

This grants effective shell input authority inside the scoped terminal, so the
owner UI and documentation must describe it as control rather than collaboration
viewing. Browser/mobile exposure still requires a separate authenticated TLS
gateway decision. Graph mutation, process lifecycle, forced takeover, concurrent
editing, and broader roles remain out of scope.
