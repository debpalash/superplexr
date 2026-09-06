# Desktop operational-history refresh

Date: 2026-09-06 IST

Status: implemented in source; no build, test or visual validation was run.
Verification remains deferred at the owner's request.

Mission inspector history reads run on the desktop background executor instead
of the GUI thread. At most one read is in flight per desktop. Requests arriving
during that read update a generation counter; completion discards obsolete data
and schedules one read for the latest visible Mission, rather than queuing each
intermediate request. The existing latest-100-event limit is unchanged.

Replies must match the current generation, Mission and open inspector. Closing
the inspector prevents publication and further refresh work; switching Missions
clears the old timeline and refreshes the new scope. Authoritative removal also
invalidates the in-flight generation. An admitted native request remains subject
to client deadlines rather than being synchronously joined or cancelled by GUI
code. These are read-only requests, not replayable Mission commands.

The timeline exposes refreshing/error state and a manual refresh action. Failure
retains same-scope entries with an explicit stale-data warning and a diagnostic
capped at 1,024 characters. No timer or permanent-error retry loop is introduced.

Deferred acceptance includes slow transport, repeated refresh, close/reopen,
Mission switching/removal, authorization loss and request failure. Other desktop
commands still have blocking paths; this is not a fully nonblocking GUI claim.
