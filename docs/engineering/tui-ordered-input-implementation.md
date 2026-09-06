# TUI ordered background input

Status: source implemented, unbuilt and untested. No tests, builds, lint checks,
remote sessions, terminal runs, or resource measurements were executed for this
change, per owner direction.

The interactive TUI now submits Control requests, keys, confirmed paste, and
controlled resize through a lazy worker owned by each participating Surface.
One worker serializes that Surface's commands. `queue_input` success means
admission to the local queue, not that a remote command succeeded. Control and
paste acknowledgements arrive through a one-slot status handoff to the input
loop. The UI does not enable input until both claim and its initial resize are
acknowledged on the still-current local continuity.

The subsequent [connection-pinned Control implementation](tui-pinned-control-implementation.md)
binds input and known-lease return to their original wire/epoch. An unconfirmed
claim retires that wire and remains a distinct unknown state requiring detach
and a new observing view; an empty cleanup cannot report a confirmed return.
This extension also remains unbuilt and untested.

## Ordering, bounds, and local input fencing

The subsequent resize-coalescing source change also retains the latest requested
grid while a claim is pending. A resize can replace the grid of a queued claim
or an immediately adjacent queued resize with the same generation/continuity.
If the claim is already executing, the resize follows it through the ordered
queue. It never overtakes a key or paste. A claim failure, return, or continuity
fence clears the queued geometry with the rest of the unsent input; observing
alone does not gain permission to resize. This change remains unbuilt and
untested, including resize-during-claim, dense resize bursts, ordering around
keys/paste, cancellation, and final acknowledged grid behavior.

- Each Surface permits at most 128 queued actions and 8 MiB of queued payload,
  plus one executing action. Key identity/text bytes are counted; paste uses the
  existing 8 MiB limit. Payloads move into native calls rather than being cloned
  by the worker. Native encoding, transport buffers, the in-flight command,
  frames, and paste-confirmation storage are separate costs. These bounds do
  not establish an aggregate tiny-footprint budget.
- Queue overflow disables local input, discards unsent actions, requests return
  of known Control, and reports the limit. It never silently drops one key while
  continuing to send the rest of the command line.
- Commands carry the queue generation and observed terminal continuity. The
  worker checks them against current feed state before admitting a write and
  checks again after its response. A claim's resize has its own intervening
  check. The native runtime remains the authority for every input epoch.
- Return Control, pause, history/search entry, observed connection change, and
  shutdown immediately disable local input and discard unsent actions. Known
  lease return takes priority over ordinary queued work. A write already
  admitted to the native call can still finish; cancellation cannot undo it.
- No uncertain claim, key, paste, or resize is retried by this worker. On an
  error, local Control is retired, queued work is discarded, and a known lease
  is best-effort returned over the existing native wire. Unknown effects remain
  unknown. Cleanup acknowledgement cannot overwrite an unseen write-failure
  warning. Ordinary live frames no longer overwrite an input-failure status.
- A new claim requires explicit user action after pending work/known lease
  cleanup ends. Paused output cannot claim Control. No force takeover is added.

## Presentation and shutdown

For queued-input views, `release_control` is now a local fence and pending check.
Focus/close/navigation retain their existing release-before-change ordering:
while a return is pending they stay on the current view with input disabled.
The subsequent [deferred view-change implementation](tui-deferred-view-changes-implementation.md)
now retains one Surface/continuity-bound intent and completes it after a confirmed
return, without requiring a repeated view command. It remains unbuilt and
untested. An unconfirmed return cancels that intent and requires an explicit
retry, not silent lease stealing.

Dropping a queued-input view transfers its terminal subscription to the input
worker for final cancellation after best-effort return. Drop does not join a
blocked command on the input thread. Process exit may occur before that cleanup
is acknowledged; the runtime's authenticated connection cleanup remains
necessary, and the TUI must not claim a confirmed return in that case.

The synchronous `FocusedSession` methods remain for callers that never opt into
queued input. Mixing synchronous claim/key/paste/resize with an enabled queue is
rejected rather than silently changing their acknowledgement contract. A caller
must release an existing synchronous lease before enabling the queue.

## Remaining work and deferred acceptance

Startup with a Session ID still attaches before the interactive loop. The
non-queued observing view's cancellation is now addressed by the separate,
unverified [local event-cancellation change](local-event-cancellation-implementation.md).
Miscellaneous library operations are not covered by this worker. Deferred focus/close/navigation is
implemented separately and also unverified. This is not a claim that
every TUI operation is nonblocking or that native input races are proven closed.

Future acceptance must cover bounded queued/in-flight payloads, serialized keys
and paste, claim/resize acknowledgement, return while claim or input is in flight,
overflow without partial-command continuation, late acknowledgements, coalesced
reconnect, epoch rejection, uncertain write and return errors, pause/history/search
fencing, explicit reacquisition, focus/close refusal while pending, destructor
cleanup and exit, thread/admission limits, and responsiveness on real remote
links and supported platforms. Earlier synchronous-input evidence cannot certify
this implementation.
