# Event-driven browser session list

Status: source implemented, unbuilt and untested. No tests, builds, browser
checks, or resource measurements were run for this change, per owner direction.

The browser now subscribes to authenticated `GET /sessions/events` instead of
calling `/sessions` every two seconds. The new endpoint projects the existing
Share-scoped native terminal-index subscription; it does not acquire owner
authority or subscribe separately to every terminal's output. The existing
one-shot `/sessions` endpoint remains available to callers.

## Delivery and recovery

- `configuration` identifies observer versus Controller-capable gateway mode.
- `collection` forwards negotiated snapshot begin/end markers and generation.
- `session` carries only Session ID, process executable, lifecycle status, and
  archive state. Executable labels remove control characters and are capped at
  256 Unicode characters. Process arguments, credentials, tty paths, and control
  identities are not part of this projection.
- The browser stages a snapshot separately and replaces current membership only
  on its matching end. A disconnected or partial snapshot never proves deletion.
  Live updates change entries; archive updates remove them. The existing selected
  Session attachment is cleared if authoritative membership removes it.
- The UI disables list navigation while an announced snapshot or HTTP reconnect
  is incomplete. A completed empty snapshot renders an empty list immediately.
  Foreground-process changes update labels without list polling; project/task
  naming beyond the native summary is not inferred from process arguments.
- HTTP recovery reuses the existing bounded-backoff read-only feed follower and
  starts a fresh native subscription. Native receiver recovery can resubscribe
  internally and supplies fresh collection markers. Neither layer replays input.
  Internal native reconnect currently has no separate browser status event until
  the next snapshot; the list's connection label alone is not a liveness proof.
- Revocation, receive limits, unsupported snapshot negotiation, and terminal
  metadata errors end the catalog with an explicit message. The UI clears its
  retained list/output and requires explicit reconnect for terminal errors.
  A legacy runtime without collection snapshots is not silently treated as an
  authoritative empty list. Upgrade it to use this new browser feed.

## Resource and security boundaries

There is one admitted native reader per browser catalog connection, not one per
Session. It shares the existing eight-read/stream semaphore and keeps its permit
until native work ends. Its native-to-async queue and async-to-HTTP queue each
hold one item. Connection-pinned delivery is checked after waiting for HTTP queue
capacity. Detach/shutdown drops a cancellation guard, waking an idle native read;
an admitted native handshake can still finish within its existing deadline.
Backpressure can reach the existing bounded native mailbox and its explicit
receive-limit ending. This is not a claim of lossless delivery at arbitrary rates.

Browser state contains at most 4,096 current entries plus 4,096 staged entries;
exceeding the per-collection cap ends access locally with instructions to narrow
the Share. It stores no lifetime event log. Complete snapshots render once;
unchanged live process/status entries are ignored. SSE keepalives remain, but
there is no session-list sampling timer. Existing explicit Control heartbeats
are unrelated and remain in place.

The endpoint retains the same bearer-key, Host, Origin, fetch-site, CSP, and Share
checks as the other browser APIs. Embedding permission does not grant a parent
cross-origin access to this stream. The static `sessions.mjs` module contains no
credentials. Browser credentials stay in memory and are not persisted.

## Deferred acceptance

Future checks must cover empty and populated snapshots, live create/archive/exit
and process changes, mismatched/partial generations, native and HTTP reconnect,
stale queued deliveries, scope/revocation, key replacement and page teardown,
slow readers, entry caps, semaphore admission, legacy peers, idle cancellation,
browser rendering, and measured idle/resource behavior. Older polling-era
evidence does not validate the new implementation.
