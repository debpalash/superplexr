# Background TUI history paging

Date: 2026-09-06 IST

Status: source implementation only. No tests, builds or latency measurements were
run; verification is deferred at the owner's request.

`Ctrl-] h` / prefixed Page Up reuses the existing lazy history/search worker pair.
The shared worker accepts a bounded bottom-relative offset job, using the same
replaceable-command mailbox and single result slot as search. No new worker is
created per page. Cancellation prevents obsolete admission where observed and
discards results; an already admitted history RPC retains its native deadline.

The TUI pauses immediately while a page loads, cancels pending paste and disables
an initial automatic control claim. Repeated paging advances from the most recent
requested offset, capped at 100,000 rows, without accumulating a work queue. The
completed offset is committed only on success. Failure leaves the current view
paused with an explicit retry/live instruction.

Results must match the active Session and observed connection continuity, with
navigation/search closed and the feed still live or ended. Returning live,
toggling pause, changing panes, closing a pane, entering navigation/search or
detaching discards pending results. An observed reconnect also cancels the read.
No shared viewport, process lifecycle or input lease is mutated by history reads.

Bottom-relative coordinates can move as output arrives or retention changes;
this is not stable bookmarking. Existing synchronous public history convenience
APIs remain available, but the TUI input loop no longer uses one for paging.
Other blocking TUI actions and full cancellation/continuity/platform acceptance
remain open.
