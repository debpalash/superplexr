# Collection snapshot implementation

Date: 2026-09-06 IST

Status: work in progress, **unverified**. Testing/build validation is deferred at
the owner's explicit request. The 475-test oversized-record milestone describes
the previous source state, not these edits.

## Implemented source paths

The optional `collection_snapshots_v1` feature negotiates a separate
`MetadataSnapshot` frame carrying UUID-matched begin/end markers. Existing item
events keep their representation. Mission, terminal-index, Run Activity and
Session Group producers delimit their initial and lag-recovery snapshots;
non-negotiating peers receive the legacy item-only stream.

The shared native receiver validates marker ordering and exposes
`recv_update_delivery`. Item-only convenience reads remain available but cannot
establish complete collection membership. The shared `CollectionTracker` checks
delivery identity at use, accumulates IDs rather than decoded record copies,
and exposes completed membership only after matching delimiters. Changing the
physical connection or logical stream abandons incomplete membership.

Desktop handoffs retain markers in the existing one-slot queue. Completed
snapshots reconcile activity entries, non-archived terminal presentation,
Mission projections and authoritative Session Group rows. Group deletion removes
the authoritative row instead of turning it into a version-zero local row that
can be persisted and recreated. The CLI forwards markers with their stream name.

Session Group mutations publish while holding the group-store lock. Initial and
lag-recovery capture subscribe and list under that same lock, then release it
before socket writes. Recovery discards the old cursor so pre-snapshot changes
cannot replace newer membership. Mission, terminal-index and activity feeds also
discard their lagged cursor before snapshot capture; subsequent queued payloads
are treated as notifications and resolved from current state rather than replayed
as stale replacements. Activity resolution uses current provider facts and expiry.

Missing terminal membership retires presentation/input and clears retained search,
selection and terminal frame state. Missing Mission membership clears the related
history, activity and verification preview. Group changes and collection removals
resynchronize the active terminal and surface presentation, including when no user
selection event occurs. Malformed metadata records invalidate the receiver so a
later end marker cannot establish membership after a discarded record.

Desktop metadata handoffs now deliver a final bounded error through the same
one-slot channel. Startup subscription failures, worker-start failures and
unexpected worker exits also populate a persistent warning below the workspace
tabs. The warning names each stopped collection and provides copyable details;
its scrollable height is capped at a quarter of the viewport independently of UI
zoom. It does not clear collection membership, terminate PTYs, retry a permanently
failed feed, or claim that terminal processes have stopped. Diagnostics retain at
most 1,024 characters per feed. No timer or polling loop is added.

The warning now offers explicit **Retry sync** for stopped feeds only. Subscription
establishment runs on each feed's existing worker rather than the GUI thread.
Retries retain the old diagnostic until a current complete snapshot arrives; a
legacy item-only peer clears the warning on its first accepted item without
claiming complete membership. In-progress retries are deduplicated. Subscription
or handoff failures return to the warning, without an automatic permanent-error
retry loop. Dropping a feed during establishment prevents its eventual receiver
from entering the read loop; dropping an established feed wakes its native wait.
An in-flight connection attempt remains bounded by the native client deadlines.
Retry establishes read-only subscriptions, not command replay or control claims.

## Work still required

- Exercise snapshot/publication ordering under concurrent mutation, lag recovery
  and deletion followed by an older queued replacement once testing resumes.
- Review archive/hide versus explicit dismissal behavior and cached inspector,
  input, selection and search state when collection membership disappears.
- Exercise explicit retry, close-during-connect and repeated-click cancellation
  behavior after testing resumes, including legacy empty collections.
- Add and run real-daemon acceptance for deletions during transport loss, empty
  snapshots, partial snapshots, scope filtering and legacy negotiation.
- Review protocol/raw-stream fixtures that negotiate features but directly decode
  EventBatch records without the shared receiver.
- Build and validate all consumers after the owner resumes verification.

This is not arbitrary-size snapshot support, an atomic cross-collection view,
or completed remote/security/platform acceptance.
