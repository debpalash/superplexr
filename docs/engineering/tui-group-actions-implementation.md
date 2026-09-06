# TUI group actions

Date: 2026-09-06 IST

Status: implementation only; no builds or tests were run, at the owner's request.

Owner group rows retain the runtime summary used to display them. F2 rename and
pin/unpin and F3 order edits submit one mutation against that summary's version. They never invoke
the client's rebase/create-if-missing helper. A stale version or deleted group
therefore reports an error rather than overwriting another client's edit or
recreating a removed group.

An initial duplicate-group implementation was corrected before execution: the
runtime forbids a Session from belonging to two groups. `D` now means another
view. From a group it opens member selection in explicit duplicate-view mode;
from a Session it opens a second Surface directly. Ordinary opening still focuses
an existing pane. The new Surface observes until explicitly granted Control,
and release-before-focus ordering and the two-pane limit are preserved. No group
creation or process-launch request is sent. Pending history/search continuity
checks distinguish the active pane when two panes refer to the same Session.

Transport errors from group edits are unconfirmed outcomes, include the target ID and instruct
the user to refresh before retrying. No write is automatically replayed.

One lazy worker and one-slot command/result channels keep writes off the TUI input
thread. At most one action is admitted; additional actions are refused until its
result is consumed. Drop does not join an in-flight RPC or roll back a user write;
queued work observes shutdown before admission. Results survive navigation view
changes in the local action-status line. Group editing refuses Shared clients
both in the UI and at worker creation; runtime authorization remains authoritative.

Rename drafts consume input locally, filter control characters, reject blank
names and cap UTF-8 length at the server's 128-byte name limit. No draft/paste
bytes are forwarded to a terminal. Full keyboard/outer-PTY, race, error, shutdown
and cross-client acceptance remains deferred.

F3 reuses the local edit draft for a decimal u32 ordering value. Invalid, blank,
out-of-range or overlength input is refused before worker submission. Clearing or
editing an overflowed draft is required before saving. The value is displayed in
each group row; lower positions sort first within the pinned/unpinned tier and
equal values retain the runtime's group-ID tie-break. This is an explicit single
position edit, not an atomic relative move or multi-group renumbering transaction.
