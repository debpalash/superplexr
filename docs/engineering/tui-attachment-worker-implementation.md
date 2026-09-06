# TUI background session attachment

Status: source implemented, unbuilt and untested. No tests, builds, lint checks,
remote runs, or resource measurements were performed, per owner direction.

Choosing a Session in the TUI navigator (Enter, split/stack selection, or D for
a duplicate view) now schedules observing attachment on one lazy reusable worker.
The terminal input loop stays in the navigator while the remote subscription is
being established. It displays the opening state and admits no additional queued
attachments from repeated Enter/D.

## Ownership and cancellation

The worker owns one request, at most one completed pane, and at most one retiring
pane. These phases serialize: no per-click thread or unbounded job queue exists.
Newly prepared panes are always observing and never claim Control or start a
process. The worker's finish operation changes only local workspace state; any
replaced, unused, rejected, cancelled, or stale prepared pane is destroyed back
on the worker. Subscription cancellation and possible remote cleanup therefore
do not run while that local finish operation holds the input thread.

An exact generation identifies the requested attachment. Navigation keys,
editing, pasted text, or leaving the navigator cancel the old intent. Esc during
an opening cancels it and leaves the navigator visible; a subsequent Esc keeps
the normal return/detach behavior. An already admitted remote request may finish
within its existing deadline, but its stale result cannot switch the workspace.
While that request or its cleanup remains active, a new Enter reports that the
worker is busy instead of creating another connection attempt.

Installing the prepared pane preserves the existing two-pane limit, ordinary
focus of an already-open Session, explicit duplicate-Surface semantics, and
split orientation. It refuses to change views if the outgoing pane still has an
unconfirmed Control release. The incoming pane is retired in that case; no force
takeover or cleanup retry is performed on the input thread. Input is never
automatically enabled on the new pane.

Worker shutdown wakes its wait without joining a stalled remote request. It
owns final pending-result destruction. Rejected Share access and receive-limit
errors follow the TUI's existing clear-and-detach path. Other attachment errors
remain in the navigator with an explicit retry message.

## Scope and remaining blocking work

Subsequent source implementation adds a separate
[ordered input worker](tui-ordered-input-implementation.md) for interactive
Control, key/paste, and resize. It remains unbuilt and untested. The original
scope description below records what the attachment-only change covered.

This change covers navigator-driven attachment and retirement caused by its
installation, not every TUI network operation. Startup with an explicit Session
ID still opens before entering the interactive loop. Explicit claim/return,
input/paste, controlled resize, focus/close release, and final Control cleanup
still use their existing synchronous command path. Moving those writes requires
ordered, non-replayed command handling and immediate local input fencing; this
worker does not claim to solve them.

Ordinarily focusing an already-open Session through the navigator currently
prepares an observing subscription before local deduplication, then retires that
unused pane on the worker. This preserves behavior without blocking the input
loop but adds avoidable remote setup; it is not a latency optimization claim.

## Deferred acceptance

Future checks must cover slow and failed attachment, duplicate admission,
cancel-before/after-completion races, latest navigation intent, worker shutdown,
subscription-slot retirement, active Control-release refusal, replacement and
two-pane limits, identical Session IDs with distinct Surfaces, Share revocation,
resource-limit errors, remote link interruption, outer-terminal restoration,
and measured responsiveness/resource costs on supported platforms. Existing
synchronous-attachment evidence does not certify these changes.
