# TUI view changes after asynchronous Control return

Status: source implemented, unbuilt and untested. No tests, builds, lint checks,
terminal runs, or remote/platform checks were performed, per owner direction.

Focus-next, close-pane, navigation, Attention, split, and stack selection now
retain one local presentation intent while the ordered input worker returns
Control. A successful return completes that intent automatically; the user no
longer needs to repeat the view command just because its return was asynchronous.

## Admission and cancellation

- The intent is bound to the original Surface's opaque identity and observed
  continuity, not merely its Session ID or pane index. Two views of the same
  Session cannot satisfy one another's pending return.
- Requesting the change disables local input and clears queued writes through
  the existing release interface. There is at most one pending view intent.
  Another view command updates that intent without queuing another lease return
  while the original return is still pending.
- The input loop inspects `ReturnState` locally: pending means wait, released
  means apply once, and unconfirmed means cancel the intent with an explicit
  return-retry/detach instruction. An unknown claim requires detach and a new
  observing view, not retrying a return without a known epoch. This inspection
  sends no RPC and retries no write.
- Changing active Surface, continuity, or presentation scope cancels the intent.
  A subsequent reconnect cannot unexpectedly close or focus a different pane.
- Esc cancels the view change without undoing the Control return already in
  flight or restoring input authority. Paste while pending cancels the view
  change and is not forwarded. Ordinary typing is blocked while waiting.
- Other explicit control/history/search/pause/live actions abandon the old
  presentation intent before executing their own behavior. Copy and prefix-help
  can be used while waiting. Completion waits while the command-prefix router
  is awaiting the next key, preserving the opportunity to detach or cancel.
- The local workspace operations retain release-before-focus/close semantics
  and the existing two-pane limit. Navigation refresh and Session attachment
  still use their own bounded read workers. Closing never terminates a process.

There is no new timer, thread, persistent layout record, or remote command type.
The existing event loop observes acknowledgement state. Unchanged waiting state
does not request an extra redraw, and terminal frames do not overwrite the
pending-action status with a generic observing label.

## Remaining limits and deferred acceptance

An admitted terminal write or return cannot be undone. A return error is not
treated as success; the intent is cancelled and an explicit retry is required.
The non-queued observing view's subscription cancellation now uses the separate,
unverified [local event-cancellation implementation](local-event-cancellation-implementation.md).
Startup-before-loop attachment remains outside this change. No claim is made
that every operation is nonblocking or that platform/remote races are verified.

Future acceptance must cover delayed return followed by each view action,
one-time completion, replacement and cancellation of pending intents, command
prefix handling, no forwarded paste/typing, identical Session IDs on different
Surfaces, coalesced reconnect, failed/uncertain returns, exit and final cleanup,
close-last-pane navigation, history/search interactions, and sustained remote
responsiveness/resource costs. Earlier input-worker evidence cannot certify
this implementation.
