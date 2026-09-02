# 05 — Local protocol v3

> Implementation status (2026-09-01): the runtime implements the normative
> 32-byte header, Hello/Welcome feature negotiation, strict per-stream sequence
> validation, bounded zstd, JSON control frames, and protobuf terminal
> full-frame/delta/lifecycle payloads. Mission, terminal-index, Session-group,
> Run-Activity, and terminal subscriptions receive independent nonzero stream
> identifiers after acceptance on stream zero. The native client, desktop, and
> CLI event stream carry those independently cancellable streams on one physical
> connection.
> `runtime diagnostics` reports
> `v3-json-control-protobuf-terminal-zstd-multiplexed` so tools can distinguish
> that topology without mistaking it for the old preview codec. Protocol 21 / wire
> v3.1 requires the `multiplexed_streams_v1` feature; peers fail the handshake
> instead of silently pairing with the former split-subscription topology.

## 1. Goals — W-GOAL-001

Protocol v3 carries low-frequency domain control and high-frequency terminal data
over a local Unix stream while preserving:

- explicit version and feature negotiation;
- request idempotency and optimistic concurrency;
- independent bounded streams;
- deterministic terminal sequencing and repair;
- restricted capabilities for untrusted agent children;
- backend neutrality: no GPUI, PTY handle, or Ghostty type crosses the wire.

V2 newline-delimited JSON remains readable only by the migration CLI. The v3
runtime does not mix unframed v2 messages into a v3 connection.

## 2. Transport and framing — W-FRAME-001

The desktop and CLI connect to the user-only `control.sock`. Agents connect to a
Run-scoped socket or inherited file descriptor and present a scoped bearer token.
All transports use reliable ordered Unix-domain `SOCK_STREAM`.

An opt-in owner Remote attachment MAY expose a second owner-only Unix socket on
the client host through OpenSSH StreamLocal forwarding to `control.sock`. The
framing, negotiation, authorization classes, connection identity, subscriptions,
repair, and Control-lease rules are unchanged across that channel. The runtime
MUST NOT open a TCP listener for this feature. A forwarding supervisor MUST use
structured SSH arguments, refuse an existing local path, create the socket in an
owner-only directory, remove only the exact owner-owned socket it created, and
reconnect without implying that control survived disconnect. A connect-only
client MUST fail rather than spawn a replacement local runtime when the channel
is absent.

This transport authenticates one trusted OS owner through the remote SSH account.
It is not a Share and MUST NOT be used as evidence of multi-user authorization.

A Share-capability client adds `share_token` to every request, including
subscription setup. The runtime stores only a digest, reauthenticates every
request, pins one authorization identity and role per connection, checks exact
Mission/Session scope before side effects, and filters collection and event
streams. `ShareIdentity` lets a successfully authenticated client discover that
pinned metadata without exposing a secret. `CreateShare`, `ListShares`, and
`RevokeShare` are owner-only. The plaintext token is returned only by
`CreateShare`; identity, listing, and revocation never return it. Revocation MUST
terminate already-open streams immediately, release Control leases held by a
Controller Share, increment their epochs, and fail closed if a revocation
notification was missed. Expiry MUST durably enter the same active-revocation
path without waiting for another guest request.

Each frame begins with this 32-byte, network-byte-order header:

| Offset | Size | Field | Meaning |
|---:|---:|---|---|
| 0 | 4 | magic | ASCII `T9NE` |
| 4 | 2 | major | wire major, `3` |
| 6 | 2 | minor | wire minor |
| 8 | 2 | kind | payload kind |
| 10 | 2 | flags | bit 0 compressed; all unknown bits rejected |
| 12 | 4 | stream_id | zero for connection control |
| 16 | 8 | stream_sequence | starts at one per direction and stream |
| 24 | 4 | payload_length | bytes following header |
| 28 | 4 | uncompressed_length | zero if not compressed |

Maximum payload is 16 MiB; maximum uncompressed payload is 64 MiB. A receiver
validates limits before allocation. Zstandard is the only v1 compression codec
and is negotiated. Payloads below 1 KiB SHOULD remain uncompressed.

Frame kinds:

```text
1 Hello              2 Welcome             3 Close
10 Request           11 Response            12 EventBatch
20 FullFrame         21 FrameDelta          22 ResyncRequired
23 HistoryPage       24 SearchPage          25 TerminalLifecycle
30 Ping              31 Pong
```

Unknown kinds are ignored only when the negotiated minor-version feature set
marks them optional. Otherwise the connection closes with `unsupported_frame`.

## 3. Encoding — W-ENC-001

`Hello`, `Welcome`, `Close`, `Request`, `Response`, `EventBatch`, `Ping`, and
`Pong` payloads are UTF-8 JSON objects with stable snake_case field names.
Terminal payloads use Protocol Buffers v3 with a checked-in schema and generated
Rust code. Proto field numbers are never reused. A wire schema change requires a
golden cross-version fixture. The normative v1 terminal schema is
[`proto/termi9ne/terminal/v1.proto`](../../proto/termi9ne/terminal/v1.proto).

JSON integers that may exceed JavaScript's safe range—sequences and byte
offsets—are decimal strings. UUIDs use lowercase hyphenated form. Timestamps use
UTC RFC 3339 with microseconds. Unknown JSON fields MUST be ignored; missing
required fields fail the request, not the process.

## 4. Handshake — W-HANDSHAKE-001

The client sends `Hello` as stream zero, sequence one:

```json
{
  "client_id": "uuid",
  "client_kind": "desktop",
  "client_version": "0.1.0",
  "protocol": { "major": 3, "min_minor": 0, "max_minor": 0 },
  "features": ["zstd", "terminal_v1", "mission_events_v1"],
  "device_id": "uuid"
}
```

The runtime replies `Welcome` with runtime ID/version, selected minor, enabled
features, connection ID, limits, and current server time. No other frame is valid
before Welcome. Major mismatch produces Close with upgrade instructions. Minor
negotiation selects the highest common value.

Desktop reconnect uses a stable device ID but a new client and connection ID.
Identity does not restore control implicitly.

## 5. Streams and subscriptions — W-STREAM-001

Stream zero is connection control. Each accepted subscription receives a unique
nonzero stream ID scoped to the connection:

- Mission subscription: ordered domain event batches after a supplied Mission
  sequence.
- Terminal-index subscription: a race-free snapshot followed by lifecycle and
  control-owner replacements for every runtime Session.
- Session-group subscription: a race-free snapshot followed by complete,
  versioned replacements and removals for owner-arranged sidebar groups.
- Run-Activity subscription: a race-free derived snapshot followed by lifecycle,
  Signal, provider-report, and provider-expiry replacements.
- Run evidence requests: bounded provider-neutral CI-check and change-review
  observations keyed to an exact Run and revision, with authenticated source and
  server-authored observation time.
- Session subscription: one FullFrame then FrameDelta and lifecycle messages.
- History/search query: one or more bounded pages followed by an end marker.

Frame sequence is gapless within a stream and direction. A transport-frame gap
closes the stream. Terminal frame sequence inside terminal payloads independently
protects projection correctness.

A subscription request includes a random `request_id`; success returns its stream
ID before data begins. Unsubscribe is idempotent. Streams end independently.

## 6. Control request and response — W-CONTROL-001

Request shape:

```json
{
  "request_id": "uuid",
  "idempotency_key": "uuid",
  "method": "mission.execute",
  "params": {},
  "deadline_ms": 5000
}
```

Response shape:

```json
{
  "request_id": "uuid",
  "result": {},
  "error": null
}
```

Exactly one of `result` and `error` is non-null. Error shape is:

```json
{
  "code": "conflict",
  "message": "Mission changed since version 41",
  "retryable": true,
  "details": { "expected": "41", "actual": "43" },
  "correlation_id": "uuid"
}
```

Messages safe to retry require an idempotency key retained for at least 24 hours.
Mutation methods include `expected_version`. The runtime commits events durably
before returning success or publishing them to subscribers.

## 7. Required control methods — W-METHOD-001

### Runtime

- `runtime.status`, `runtime.shutdown_preview`, `runtime.shutdown`;
- `runtime.subscribe`, `runtime.unsubscribe`;
- `runtime.diagnostics` with redacted output.

### Mission and graph

- `mission.create`, `mission.get`, `mission.list`, `mission.execute`;
- `mission.subscribe`, `mission.history`;
- `graph.get` returning Runs plus separately typed lineage/dependency edges.
- `scheduler.plan` returning a read-only capacity/readiness classification.
- `scheduler.launch_configured_batch` freezes the current capacity plan inside
  the daemon and resolves/launches the selected named engine drivers with
  independent results.
- `scheduler.policy.set` validates and atomically persists an opt-in per-Mission
  policy; `scheduler.policy.list` returns policies without driver secrets.
- `scheduler.settings.set` validates and atomically persists the owner-wide
  global agent concurrency limit; `scheduler.settings.get` returns it.
- `runtime.diagnostics` includes the configured global agent limit and current
  terminal-backed agent occupancy without exposing Run or process identity.
- Scheduler entries carry base/effective priority and optional durable
  `ready_since_unix_micros`; clients do not supply lifecycle timestamps.
- `run.launch_command`, `run.launch_configured`; both commit Run start, Session
  creation, and assignment atomically before spawning a daemon-owned PTY.
- `run.preview_configured` resolves the same named driver without mutation and
  redacts environment values. It also reports an optional OS-authored sandbox
  backend/profile and whether network was isolated.
- `provider.fact.report` records one bounded expiring observation. The runtime
  authors source and time from either an authenticated Run agent channel or the
  owner socket; callers cannot choose provenance.
- `run.activity.get`, `run.activity.list` derive explainable activity from Run
  lifecycle, unresolved Signals, a fresh provider fact, then `Unknown`.
- Successful launches atomically record `run_driver_resolved` before Run and
  Session start. Mission history carries only driver/profile identity, a
  process-spec SHA-256, argument count, environment key names, and optional
  sandbox attestation.

### Session

- `session.create`, `session.list`, `session.get`;
- `session.attach`, `session.detach`;
- `session_group.create`, `session_group.list`, `session_group.update`,
  `session_group.delete`, and `session_group.subscribe`. Group mutations use an
  expected version; membership never changes the identity or lifecycle of a
  member Session.
- `session.input`, `session.resize`, `session.interrupt`;
- `session.terminate_preview`, `session.terminate`, `session.kill`;
- `session.take_control`, `session.return_control`;
- `session.history`, `session.search`;
- `session.capture`, returning the authoritative terminal summary and exact
  visible frame as one timestamped automation snapshot;
- `session.wait`, requiring a bounded timeout and one typed condition: visible
  text, canonical-frame quiet interval, or process exit;
- `session.resync`, `session.ack_frame`.

Running Session summaries may include `foreground_process` with only its
kernel process ID and normalized executable basename. The runtime observes the
PTY foreground process group at a bounded cadence and publishes only identity
changes; arguments, command lines, terminal contents, and environment values
MUST NOT cross this boundary. Clients MUST treat the field as ephemeral and
fall back to the Session identity when it is absent or cannot be resolved.

### View state

- `view.load`, `view.save`. Presentation state is per device and is not a
  Mission event.

Method authorization depends on client kind and capabilities.
Exact request, result, command, event, and error fields are defined in the
[control and event catalog](11-control-and-event-catalog.md).

The preview profile exposes the safe read-only subset of `runtime.status` as
`runtime_diagnostics`; it includes version, wire profile, platform, uptime, and
bounded resource counts, and excludes paths, environment, commands, terminal
content, and tokens. It reports current terminal-wait occupancy but no queries or
captured content. At most 128 waits may be active. Each wait subscribes to the
canonical Session event stream rather than polling, has a mandatory timeout from
1 millisecond through 1 hour, and returns the satisfying event sequence plus a
fresh structured capture. Text queries are bounded to 1024 bytes; quiet windows
are 50 milliseconds through 1 minute. An already-closed Session can satisfy only
an exit wait or a text condition already present in its retained final frame.
The CLI's `events` command opens the Mission, Run-Activity, and terminal-index
subscriptions independently and emits one compact JSON object per line. It keeps
both write halves open for the lifetime of these server-to-client streams,
includes an explicit `stream` discriminator, delivers the race-free initial
snapshots, and exits cleanly on interrupt. It never converts terminal frames
into activity guesses.

## 8. Terminal payloads — W-TERMINAL-001

The protobuf schema mirrors section 04. All terminal messages contain Session ID
and Session stream ID context. Required message types are:

- `FullFrameV1`;
- `FrameDeltaV1`;
- `ResyncRequiredV1 { reason, latest_sequence }`;
- `HistoryPageV1 { history_epoch, rows, before?, after?, truncated }`;
- `SearchPageV1 { query_id, matches, complete }`;
- `TerminalLifecycleV1 { state, exit_status?, reason? }`.

Colors use explicit tagged values: default, palette index, or 24-bit sRGB.
Graphemes are UTF-8 bytes. Protobuf decoding MUST validate cell width, row length,
grid bounds, intern-table references, and decompressed size before installing a
frame.

## 9. Input and resize acknowledgement — W-INPUT-001

`session.input` is a control JSON request so validation and errors remain clear;
large paste bytes are a protobuf terminal payload referenced by request ID.
Input success returns accepted client sequence and resulting control epoch. It
means the ordered Session actor accepted the operation, not that the child
application consumed it.

`session.resize` returns `ResizeAccepted` with requested and accepted grid plus
the first terminal frame sequence that reflects it. Stale control epoch returns
`not_controller`; out-of-range geometry returns `invalid_request`.

## 10. Backpressure — W-PRESSURE-001

Default per-connection outbound budget is 16 MiB and per-Session stream budget is
8 MiB. Control responses and lifecycle events have reserved capacity and cannot
be displaced by terminal output.

When a Session stream exceeds its budget:

1. discard queued, unsent FrameDeltas for that stream;
2. enqueue one `ResyncRequired(lagged)`;
3. stop producing deltas for that subscriber;
4. resume only after `session.resync` installs a FullFrame.

Raw PTY bytes, canonical terminal state, and other subscribers are unaffected.
If a client does not read control frames for 30 seconds or exceeds the total
budget repeatedly, the runtime closes it with `client_too_slow`.

## 11. Agent capability channel — W-AGENT-001

Each Run receives:

```text
TERMI9NE_MISSION_ID
TERMI9NE_RUN_ID
TERMI9NE_SESSION_ID              optional
TERMI9NE_AGENT_SOCKET
TERMI9NE_AGENT_AUTH=process-group-peer-credentials-v1
```

The agent socket is separate from the general control socket and has owner-only
permissions. On every connection and request, the runtime obtains the kernel
peer PID, resolves its process group, and requires it to equal the live PTY
leader bound to the Run. Authorization expires when the peer leaves that process
group, the Session stops, or the Run finishes. The channel authorizes only:

- read its own Run/Mission summary;
- raise or withdraw its own Signals;
- record its own Artifacts;
- request a Grant;
- create child Runs when delegation capability is present;
- send input only while it holds its Session Control lease.
- capture and wait on only its own bound terminal without gaining input Control.

An agent cannot list unrelated Missions, resolve its own approval, issue a Grant,
take human control, read another Session, or shut down the runtime. Child
capabilities are an explicit subset of the parent's delegable capabilities.
These statements govern requests accepted on the supplied agent channel. As
specified in S-TRUST-001, they do not claim OS containment of unrestricted
same-UID code. Kernel process-group binding prevents an unrelated process from
using the channel accidentally or by merely discovering its path; sandboxing is
still required against an agent deliberately attacking another same-user process.

## 12. Error and close behavior — W-ERROR-001

Malformed length, magic, compression, UTF-8, or protobuf data closes the affected
connection and records a bounded diagnostic. A malformed request with valid
framing returns `invalid_request` and keeps the connection open. Sensitive token,
environment, terminal content, and clipboard bytes MUST NOT appear in protocol
error logs.

Heartbeat is optional on active local connections. An idle desktop sends Ping
every 15 seconds; failure to receive Pong for 45 seconds transitions the UI to
disconnected and starts exponential reconnect from 100 ms to 5 seconds with
jitter. Reconnect never replays input automatically.

## 13. Compatibility — W-COMPAT-001

- Major versions may change framing or semantics and require explicit migration.
- Minor versions are additive and negotiated.
- Domain payload and terminal protobuf versions are independent named features.
- The runtime supports the current major and one previous stored-event schema.
- A release MUST migrate a copy of state, verify it, atomically switch manifests,
  and preserve the previous manifest for rollback.
- Downgrade that cannot read new state MUST fail safely rather than truncate it.

Golden fixtures cover every frame kind, max/min sizes, unknown fields, malformed
input, compression, version negotiation, idempotent retry, and sequence repair.
