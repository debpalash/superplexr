# Browser frame backpressure and cleanup admission

Status: source implemented, unbuilt and untested. All tests, builds, validation,
app/browser runs, and resource measurements remain deferred by owner direction.

## Capacity before frame projection

The terminal SSE producer now reserves its one-slot HTTP channel capacity before
reading and serializing the latest canonical frame. While waiting for capacity,
it still observes gateway shutdown, receiver closure, and the controlled-view
activity watchdog. A backpressured `send` no longer hides that watchdog.

Once capacity becomes available, the producer reads the latest coalesced state.
It therefore does not retain a separately serialized stale frame while blocked.
Row deltas still use the last frame admitted to that same ordered HTTP channel;
the producer does not advance its delta base for a frame it skipped under
backpressure. There is no new queue, background watchdog task, or native poll.
Missed watchdog ticks are skipped rather than replayed in a burst.

Idle waiting also checks explicit Surface invalidation, not only activity
expiry. An invalidated controlled view ends and retires its original input
lease through the existing guard. A subsequent browser attachment observes with
a new Surface and cannot inherit old input authority.

Pending completion, access-ended, and permanent receive-limit notices are kept
distinct from ordinary Surface invalidation. Their native callbacks have already
retired input. These bounded terminal notices can wait for HTTP capacity so the
browser does not receive a misleading generic EOF instead of a permanent error.
They retain admission until delivered, disconnected, or shut down; a stalled
browser can occupy a slot, but cannot create an unbounded set of pending notices.
This does not retract frames already admitted to HTTP or copied by the browser.

## Admission survives HTTP cancellation

The existing eight-slot stream/read semaphore now travels into blocking
terminal-feed setup. Cancelling its HTTP request cannot release that slot while
the native capture/subscription setup still runs. An abandoned setup result
drops its subscription and controlled-view guard normally.

For controlled attachments, the guard then owns the slot through detach and
known-lease cleanup. Dropping the HTTP producer transfers the slot to the same
blocking cleanup task that waits behind an admitted command and returns its
pinned lease. Rapidly opening and dropping HTTP streams therefore cannot free
their admission slots while leaving arbitrarily many of these cleanup tasks
waiting behind native I/O. Plain observing feeds retain their slot for setup
and producer lifetime, without adding a Control cleanup task.

This can temporarily refuse new reads/feeds with the existing overload response
while old cleanup remains blocked. It is deliberate admission pressure, not a
claim that remote return succeeded. The four-slot command limit is unchanged.
Native receiver finalization, transport buffers, unrelated read handlers,
Tokio's shared blocking pool, and process-wide memory remain separate bounds.

## Deferred acceptance

Future coverage must exercise unconsumed HTTP bodies, expiry during blocked
send and native command, cancelled setup, repeated attach/drop with delayed
claims and returns, slot recovery after failures, observing-only attachment,
shutdown, terminal-notice priority, and changed-row base continuity after
coalescing. No throughput, responsiveness, platform, or aggregate footprint
claim is established by this source change.
