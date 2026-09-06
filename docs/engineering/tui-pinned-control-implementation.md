# TUI connection-pinned Control and uncertain claims

Status: source implemented, unbuilt and untested. Tests, builds, validation,
app runs, and remote/platform checks remain deferred by owner direction.

The ordered TUI input worker now retains a `TerminalInputLease` acquired through
the shared native client. Claim's initial resize and subsequent keys, paste, and
resize use that exact connection, Session, Surface, and acknowledged epoch.
They cannot borrow a replacement connection opened by a read worker. Local
queue fencing retires the shared lease immediately; native admission checks
retirement again after obtaining its socket writer. Writes already admitted
cannot be undone.

## Claim acknowledgements

A native claim requires a matching Session ID and nonzero epoch in its ACK.
A missing, malformed, or otherwise unconfirmed ACK retires the original
physical wire, rather than leaving an unknown acquisition on an open socket.
The claim is never replayed. Legacy `DaemonSession::claim_control` uses this
same claim path, while retaining its explicit, synchronous return contract.

An authenticated remote error is treated separately: the current server claim
handler performs fallible checks before changing ownership and returns its
success body after mutation. Ordinary denials therefore do not close the
connection. If that server ordering changes, this classification must change
with it. A transport/protocol error is not classified as a definite denial.

Wire retirement interrupts every subscription and outstanding request sharing
that physical connection, not just this TUI pane. Read subscriptions may recover
according to their existing policies; input does not regain authority. Closing
the wire requests host disconnect cleanup but is not evidence that the host
has finished cleaning up or that the claim never executed. It does not terminate
the terminal process.

The TUI records an unconfirmed claim independently of its known-lease flag.
An empty cleanup operation cannot clear that uncertainty or complete a pending
view transition. A later live frame likewise does not prove a return. The view
keeps input disabled and directs the user to detach (`Ctrl-]`, `d`) and reconnect
with a new observing view. Pre-claim connection failures are conservatively
handled the same way. There is no automatic takeover or reacquisition.

## Returning known authority

`TerminalInputLease::release_control` first retires local input, then sends one
return on the lease's original wire with its original epoch. It can return a
locally retired lease; retirement alone is not a remote return. It never repairs
the connection or picks up another claim's epoch. A response must identify the
same Session and a nonzero changed epoch. Updating the shared local epoch uses
compare-and-exchange so a late return does not overwrite a different local
acquisition. This method is now used by the TUI worker's prioritized cleanup.

Return errors remain unconfirmed. Explicit retry uses the same handle and
epoch; it does not silently rebase to current ownership. A lost return ACK can
therefore require detach/reconnect instead of successful local view completion.
No destructor joins a blocked write, and neither dropping a lease nor process
exit is represented as acknowledged return. Other clients' legacy return paths
are outside this change.

## Deferred acceptance

Future coverage must exercise lost/late claim ACKs, malformed Session IDs and
epochs, ordinary contention/Share denial, initial-resize failure, release during
claim, queue fencing while waiting on the socket writer, read reconnect before
input or return, shared-wire interruption, stale return ACKs after acquisition,
unknown-claim view-change cancellation, explicit retry/detach, and shutdown.
Remote behavior, responsiveness, and aggregate resource costs are not measured
by this source change. Earlier synchronous-input results do not certify it.
