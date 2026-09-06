# Browser connection-pinned input authority

Status: source implemented, unbuilt and untested. No builds, tests, validation,
browser/app runs, or remote/platform checks were performed, per owner direction.

The browser gateway now uses the native `TerminalInputLease` for claim, key,
paste, resize, and known-lease return. The browser nonce and ordered command
sequence remain mandatory, but no longer depend on a mutable `DaemonSession`
epoch to select input authority. A lease pins the original connection, Session,
Surface, and acknowledged epoch. Read-side reconnect cannot move a pending
write or cleanup onto a replacement connection.

## Retirement and cleanup

- Surface invalidation retires the pinned lease through a short, separate
  authority lock. This lock is never held across a native RPC. It therefore
  does not wait for the command-state lock held by an in-flight command.
  Native send checks retirement again after acquiring its socket writer.
- Claim ACKs arriving after invalidation retain their retired handle for exact
  cleanup, but issue no usable browser nonce. Cleanup serializes behind the
  command worker so it cannot miss a late acquisition.
- A second claim cannot overwrite retained authority. Known Control must first
  be returned, or the HTTP attachment must end. Unconfirmed native claims end
  the Surface; the shared client's ambiguous-ACK handling retires the original
  physical wire. Ordinary remote denials remain distinct from uncertain claims.
- Input/return errors disable the Surface and clear its browser nonce. Neither
  HTTP commands nor native terminal input are replayed. Guard cleanup attempts
  return using only the retained original wire and epoch. It never acquires
  Control or reconnects just to clean up. A plain observing attachment has no
  lease to return. Cleanup after an uncertain explicit return may repeat only
  that exact fenced return, never input or acquisition.
- Heartbeats check the pinned lease before capture, apply captured ownership
  metadata to it, and check it again afterward. A successful read from a new
  connection is not proof that old input authority survived. Capture failure,
  takeover, or retired authority ends the controlled Surface.

## Activity deadline

The 15-second activity timestamp lives beside the pinned authority, outside
the long-held command-state lock. The existing SSE watchdog can inspect expiry
while an RPC is blocked. A successful command also checks expiry and refreshes
the timestamp under one short lock; a late command cannot refresh expired
authority before the watchdog notices it. A fresh claim starts its own activity
window, rather than inheriting time spent merely observing.

This is not a hard real-time cancellation deadline. Already admitted native
writes cannot be undone, and local retirement is not proof of remote return.
The subsequent [frame admission implementation](browser-feed-admission-implementation.md)
keeps the watchdog active during HTTP-capacity waits and retains stream admission
through setup and delayed cleanup. That source extension also remains unbuilt
and untested; cleanup can still wait for native I/O, and process-wide shutdown
and resource behavior are not certified.

## Unchanged boundaries and deferred acceptance

Controller Share opt-in, read-only default, no force takeover, request/queue
bounds, Origin/Host/key checks, and terminal-process ownership are unchanged.
No wire-protocol or browser command schema change is introduced. Retiring a
physical wire can interrupt other feeds sharing it; observation recovery must
not be mistaken for renewed Control.

Future acceptance must cover delayed claim on detach, writer-lock contention
with invalidation, read reconnect between enqueue and send/return, stale nonce
and duplicate claim rejection, heartbeat takeover/revocation, expiry during a
blocked command, lost return ACK, HTTP backpressure, guard cleanup, and shutdown.
Browser interaction, remote behavior, and aggregate resource costs remain
unverified. The earlier browser Control evidence does not certify this extension.
