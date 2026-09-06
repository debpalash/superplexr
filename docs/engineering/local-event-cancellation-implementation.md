# Local event-subscription cancellation

Status: source implemented, unbuilt and untested. Tests, builds, lint checks,
runtime/browser runs, and performance measurements remain deferred by request.

`EventSubscription::cancel` now sets the local cancellation flag, retires its
stream mailbox, and wakes reconnect backoff. It does not acquire the socket
writer, send Unsubscribe, join a thread, or establish a connection. The existing
receiving worker destroys its subscription and performs the eventual unsubscribe
write. This removes network cleanup from callers such as observing TUI pane
destruction, desktop surface suspension, and browser HTTP-detach guards.

Reconnect delay now uses a cancellation-aware condition variable instead of
uninterruptible sleep. The worker checks cancellation after waking and before
admitting reconnect, after reconnect returns, and while installing the new
cancellation handle. A locally cancelled receiver cannot simply sleep out its
backoff and then intentionally open a replacement stream. A handshake/capture
already admitted can still finish within its existing native deadline.

The callback path also checks cancellation after receipt and before fallback
snapshot work or publication. Already admitted callbacks and socket writes are
not transactionally undone. Callers must still retire their own presentation
and input identity at detach; this change is not an authority substitute.

The native unsubscribe path now rechecks wire closure after acquiring its writer
and retires the physical connection if writing unsubscribe fails. A partial
unsubscribe frame must not be followed by unrelated requests on a potentially
desynchronized transport. Other streams on that failed physical wire use their
existing read-only recovery or terminal-error behavior.

No new thread or polling timer is added. Each event subscription adds a local
mutex/condition-variable wake pair to its existing receiving worker. The explicit
cancel contract is unchanged: merely dropping an EventSubscription handle does
not cancel it. Blocking receiver/destructor callers remain synchronous unless
their consumer owns destruction off-thread.

This closes the observing TUI pane's direct socket-write cancellation path in
source. It does not establish zero-latency destruction, confirmed remote cleanup
before process exit, complete platform support, or measured resource budgets.
Deferred acceptance must cover idle receive and backoff cancellation, reconnect
installation races, blocked writer isolation, no post-cancel reconnect intent,
already admitted operations, eventual native unsubscribe/slot recovery, partial
write retirement, callback retirement, and all desktop/TUI/browser detach paths.
The existing struct-construction fixture was adjusted for the wake field but
was not run; previous cancellation evidence does not certify this change.
