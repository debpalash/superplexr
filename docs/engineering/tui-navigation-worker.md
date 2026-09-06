# TUI navigation refresh implementation

Date: 2026-09-06 IST

Status: implemented in source, unbuilt and untested. Verification is deferred
at the owner's request; no new responsiveness or performance result is claimed.

Navigation no longer performs its multi-request list refresh on the input thread.
One worker is created on first navigation use and reused thereafter. Its mailbox
holds one replaceable request and one completed result; it does not create a new
thread per refresh. New requests invalidate earlier generations, and closing the
navigator drops pending results and prevents more obsolete requests between RPCs.
Drop wakes an idle worker without joining a stalled native request. Native client
deadlines still bound an already running request.

Automatic refresh remains limited to the visible navigator and waits for the
current job/result to finish. Explicit refresh and scope changes replace pending
work. Changing tab or Mission clears rows from the old scope; same-scope refresh
retains rows and selection until replacement succeeds. Applying an unchanged
snapshot avoids a redraw and no longer requires cloning the displayed list first.
The Missions tab skips an unnecessary terminal-list RPC.

Remote errors and receive-limit failures still clear the outer screen and exit;
the worker does not change Share scope, fall back to owner access, approve
Attention, replay commands or acquire Control. Ordinary refresh failures retain
the last list with an error note. This is not an atomic cross-collection view or
connection-pinned collection delivery API, and the decoded list still has its
existing whole-response size behavior.

Deferred work includes real stalled-transport and close/scope-change acceptance,
explicit list paging/retention limits, worker failure coverage, and moving other
blocking TUI actions off the input thread without weakening lease ordering.
