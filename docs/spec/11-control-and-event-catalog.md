# 11 — Control and event catalog

This catalog is the normative typed surface for protocol v3 control JSON. Section
05 defines framing and encoding. Section 02 defines semantics and invariants.

## 1. Common scalar and envelope types — C-TYPE-001

```text
Uuid             lowercase RFC 4122 string
Sequence         decimal string encoding u64
Timestamp        UTC RFC 3339 with exactly six fractional digits
DurationMs       JSON integer, 0..=86_400_000
NonEmptyText     UTF-8 string, trimmed length 1..=65,536 bytes
ShortText        UTF-8 string, trimmed length 1..=1,024 bytes
PathText         UTF-8 platform path string, 1..=32,768 bytes
Digest           { algorithm: "sha256", value: 64 lowercase hex chars }
```

Every mutating domain request uses:

```json
{
  "mission_id": "uuid",
  "expected_version": "42",
  "command_id": "uuid",
  "idempotency_key": "uuid",
  "actor_id": "uuid",
  "command": { "type": "..." }
}
```

Success returns:

```json
{
  "mission_id": "uuid",
  "previous_version": "42",
  "version": "44",
  "events": [],
  "projection": {}
}
```

The event array is the exact committed atomic batch. An idempotent retry returns
the original batch and resulting version even when the Mission has since advanced.

## 2. Shared domain records — C-RECORD-001

### ActorRecord

```text
id, kind { human | agent }, display_name
engine?                 required for agent
engine_version?         optional diagnostic label
```

### RunRecord

```text
id, mission_id, objective, actor
parent_id?
dependency_ids[]
retry_of?
execution_mode          Headless | NewSession | ExistingSession
priority                urgent | normal | background
phase                   pending | running | paused | finished
pause_reason?           attention | human | resource | system
outcome?                succeeded | failed | cancelled
disposition             none | awaiting_review | accepted | rejected
driver_snapshot?        driver_id, profile_version, process_spec_sha256,
                        argument_count, environment_keys[]
primary_session_id?
planned_at_unix_micros?, finished_at_unix_micros?
ready_since_unix_micros?  derived; never client-authored
summary?
```

Execution modes:

```text
Headless        { type, driver_id, program, argv[], cwd, environment_delta }
NewSession      { type, driver_id, session_spec }
ExistingSession { type, driver_id, session_id }
```

`program`, argv, environment, and cwd returned to restricted clients are redacted
according to capability. Desktop/CLI receive them only on explicit inspection.

### SessionRecord

```text
id, mission_id, name, started_by
state                   creating | running | exited | lost
exit_status?            { type: code, code } | { type: signal, signal } | unknown
controller              ActorRecord
control_epoch           Sequence
active_run_id?
run_history[]
grid?                   { columns, rows }
created_at, started_at?, finished_at?
last_output_at?
history_truncated       boolean
```

### SignalRecord

```text
id, mission_id, run_id, kind, raised_at
payload                 kind-specific object
resolution?             { by, response, grant_id?, resolved_at }
withdrawal?             { by, reason, withdrawn_at }
```

### ArtifactRecord

```text
id, mission_id, run_id, name, media_type, locator
digest?, byte_size?, created_at
```

### GrantRecord

```text
id, mission_id, run_id, subject_actor_id
operation_class, normalized_scope
decision                 allowed | denied
enforcement              cooperative | enforced
issued_by, issued_at, expires_at?, remaining_uses?
revoked_at?, revocation_reason?
```

### MissionProjection

```text
id, intent, created_by, state, version, created_at
runs[]
sessions[]
signals[]
artifacts[]
grants[]
attention[]
lineage_edges[]         { parent_id, child_id }
dependency_edges[]      { prerequisite_id, dependent_id }
```

Large terminal history and raw output are never embedded in MissionProjection.

## 3. Runtime methods — C-RUNTIME-001

### `runtime.status`

Params `{}`. Result contains runtime ID/version, protocol selection, uptime,
state path, durability profile, platform/backend, active counts, dependency pins,
and redacted health warnings.

### `runtime.subscribe`

Params `{ topics: [runtime_health | mission_list] }`. Result `{ stream_id }`.
Events are versioned snapshots/deltas with no terminal payload.

### `runtime.shutdown_preview`

Params `{}`. Result lists active Run/Session IDs and allowed choices. No mutation.

### `runtime.shutdown`

Params `{ choice: keep_running | terminate_selected, session_ids[], confirmation }`.
`keep_running` is a no-op response used by UI cancellation. Termination requires
the exact preview confirmation token, which expires after 30 seconds or state
change.

### `runtime.diagnostics`

Params `{ include_content: false, selected_session_ids: [] }`. V1 rejects
`include_content: true` unless the desktop completed an explicit preview flow.
Result is a local archive path and SHA-256 digest.

## 4. Mission and graph methods — C-MISSION-001

### `mission.create`

Params `{ mission_id?, intent, created_by, idempotency_key }`. If ID is absent the
runtime generates one. Result is MissionProjection at version one.

### `mission.get`

Params `{ mission_id, at_version? }`. Result is MissionProjection. Historical
projection MAY be slower but is required for retained versions.

### `mission.list`

Params `{ states[], cursor?, limit }`, where limit is 1..=200. Result contains
summaries, next cursor, and snapshot time. Summary contains ID, intent, state,
counts, version, last activity, and unresolved attention count.

### `mission.execute`

Params use the mutation envelope from section 1. Command payloads are defined in
section 6. Result is the atomic commit result.

### `mission.subscribe`

Params `{ mission_id, after_sequence }`. Result `{ stream_id, current_sequence }`.
The stream sends EventBatch payloads and starts with the first sequence greater
than `after_sequence`. If retention cannot satisfy it, response is
`recovery_required` with a snapshot version.

### `mission.history`

Params `{ mission_id, before_sequence?, after_sequence?, limit }`, limit
1..=1,000. Result contains complete event envelopes, paging cursors, and
truncation state.

### `graph.get`

Params `{ mission_id, include_finished, root_run_id? }`. Result contains RunRecord
nodes and separately typed lineage/dependency edges. Supplying root limits only
lineage descendants; dependency endpoints needed to explain readiness are still
included as external nodes.

### `scheduler.plan`

Params `{ mission_id, max_concurrency }`. The read-only result contains occupied
and available slots plus ordered `startable`, `ready_queued`,
`waiting_dependencies`, `blocked_dependencies`, and `manual` collections.
Preview ordering is priority then Run ID. Automatic execution additionally
requires the readiness-time aging and durable effect protocol in D-GRAPH-004 and
D-EFFECT-001.

### `scheduler.launch_configured_batch`

Params `{ mission_id, max_concurrency, session_name_prefix, cwd, grid }`. The
daemon freezes one `scheduler.plan`, resolves every selected agent Run through
its named owner-controlled driver, and returns `launched[]` and `failures[]`.
Each successful Run commits `run_driver_resolved`, `run_execution_started`,
`session_started`, and `session_assigned` as one durable batch before PTY spawn.
Driver resolution failure leaves the Run pending and records no partial graph
mutation. Calling again reconciles current capacity and never duplicates a Run
that is already running.

### `scheduler.policy.set` / `scheduler.policy.list`

Set params contain `{ mission_id, enabled, max_concurrency,
session_name_prefix, cwd, grid }`. Policies are owner-only runtime state, not
Mission events, because cwd/grid are host presentation/execution settings. The
daemon persists them atomically and reconciles enabled policies at a bounded
interval. Disable affects future selection only; it does not terminate a
running Run. List returns the complete durable policy set. Driver arguments,
environment values, terminal content, and socket paths are never returned.

### `scheduler.settings.set` / `scheduler.settings.get`

Set params contain `{ global_max_concurrency }`, constrained to 1..=256. The
owner-only store atomically persists the value and rolls back the in-memory
change if persistence fails. Get returns the authoritative value used by direct,
configured-batch, and continuous launches. Runtime diagnostics report the same
limit with only aggregate occupancy.

### `run_checkout.prepare` / `run_checkout.list` / `run_checkout.retire`

Prepare params contain `{ mission_id, run_id, repository, base_ref }` and are
valid only for a pending Run. The owner-only result records the exact repository
root, commit object ID, deterministic branch/path, and lifecycle state. Repeating
the same provenance is idempotent; different provenance for the Run conflicts.
List optionally scopes by Mission and never crosses Share or agent authority.
Retire params add `{ merged_into_ref }`; it succeeds only after the Run and its
terminal have finished, the worktree is clean, and its HEAD is an ancestor of
the resolved target commit. No force form exists. Configured launch and preview
accept `use_run_checkout`; when true, the daemon—not the caller—selects the ready
checkout as cwd.

## 5. Session methods — C-SESSION-001

### `session.create`

Uses a Mission mutation envelope with `StartSession` command. The result first
projects Creating. Lifecycle stream/events report Running or Lost; callers do not
block on process spawn.

### `session.list` / `session.get`

List params `{ mission_id, states[], cursor?, limit }`; get params
`{ mission_id, session_id }`. Results contain SessionRecord values.

### `session.attach`

Params:

```text
session_id
known_frame_sequence?
history_epoch?
viewport               { anchor: live | absolute_line, rows }
role                    controller_if_owned | observer
```

Result `{ stream_id, session, continuation: incremental | full_frame }`. A
FullFrame is sent before success is considered live.

### `session.detach`

Params `{ session_id, stream_id }`. Result `{ detached: true }`. It never changes
Session or Run lifecycle.

### `session.input`

Params `{ session_id, control_epoch, client_sequence, input }`. Input is exactly
one of key, mouse, focus, or inline paste up to 64 KiB. Larger paste references a
PastePayloadV1 frame by request ID. Result reports accepted client sequence and
control epoch.

### `session.resize`

Params `{ session_id, control_epoch, columns, rows, width_px?, height_px? }`.
Result reports requested/accepted grid and reflecting frame sequence.

### `session.interrupt`

Params `{ session_id, control_epoch }`. Result records target foreground process
group when available and delivery status.

### `session.terminate_preview` / `session.terminate` / `session.kill`

Preview params `{ session_id, cascade_run: none | cancel }`. Result identifies
leader, active Run, escalation policy, and confirmation token. Terminate/kill
require the token. Kill additionally requires `{ destructive: true }`.

### `session.take_control`

Uses a Mission mutation envelope with Session ID, requesting human Actor, client
ID, Surface ID, and expected control epoch. Result contains new controller and
epoch. Already-held identical requests are idempotent.

### `session.return_control`

Uses a Mission mutation envelope with Session ID, current human Actor, receiving
agent Actor, and expected control epoch. Result contains new controller and epoch.

### `session.history`

Params `{ session_id, history_epoch?, before_line?, after_line?, rows }`, rows
1..=2,000. Result opens a HistoryPage stream.

### `session.search`

Params `{ session_id, query, case_sensitive, before_line?, limit }`, query
1..=4,096 bytes and limit 1..=1,000. Result opens a SearchPage stream.

### `session.resync` / `session.ack_frame`

Resync params `{ session_id, stream_id, last_valid_sequence, reason }`; success
sends a FullFrame. Ack params `{ session_id, stream_id, frame_sequence }` and
allows the runtime to release buffered attach deltas.

## 6. Domain command payloads — C-COMMAND-001

Every command object has a stable `type` and these fields:

```text
plan_run
  run_id, parent_id?, dependency_ids[], retry_of?, actor, objective,
  execution_mode, priority

start_ready_run                    runtime/scheduler only
  run_id, attempt_token

pause_run
  run_id, reason

resume_run
  run_id

finish_run
  run_id, outcome, summary

cancel_run
  run_id, reason, cascade_descendant_ids[]

accept_run_result | reject_run_result
  run_id, reason?

start_session
  session_id, name, started_by, session_spec, attempt_token

assign_session
  session_id, run_id

interrupt_session
  session_id

terminate_session
  session_id, reason

take_control
  session_id, human, client_id, surface_id, expected_control_epoch

return_control
  session_id, human_id, receiving_agent, expected_control_epoch

raise_signal
  signal_id, run_id, kind, payload

resolve_signal
  signal_id, response, grant_id?

withdraw_signal
  signal_id, reason

record_artifact
  artifact

issue_grant
  grant

revoke_grant
  grant_id, reason

complete_mission
  no additional fields

abandon_mission
  reason, selected_run_ids_to_cancel[], selected_session_ids_to_terminate[]
```

Unknown command types fail `invalid_request`. Unknown fields are ignored only
when the negotiated command schema feature permits them. Server/internal commands
cannot be invoked through an agent capability.

This restriction describes the supplied capability channel. The same-UID process
limit in S-TRUST-001 still applies when an agent is not OS-sandboxed.

## 7. Event payloads — C-EVENT-001

All events use the envelope in section 02. Payloads are:

```text
mission_created
  intent, created_by

run_planned
  all immutable PlanRun fields, planned_at_unix_micros?
delivery_run_input_frozen
  run_id, source_run_id, purpose, exact candidate, exact harness, return_note?
run_driver_resolved
  run_id, driver_id, profile_version, process_spec_sha256, argument_count,
  environment_keys[]
run_start_requested
  run_id, attempt_token
run_started
  run_id, process_ref?, session_id?, started_at
run_start_failed
  run_id, attempt_token, category, redacted_message
run_paused | run_resumed
  run_id, reason?
run_finished
  run_id, outcome, summary, finished_at_unix_micros?
run_result_accepted | run_result_rejected
  run_id, by, reason?

session_start_requested
  session_id, name, started_by, redacted_spec, attempt_token
session_started
  session_id, process_ref, accepted_grid, started_at
session_assigned
  session_id, run_id
session_control_transferred
  session_id, from_actor, to_actor, control_epoch, intervention_id?
session_finished
  session_id, exit_status, final_output_offset, final_frame_sequence
session_lost
  session_id, category, redacted_reason

signal_raised
  signal
signal_resolved
  signal_id, resolution
signal_withdrawn
  signal_id, by, reason
artifact_recorded
  artifact
grant_issued | grant_revoked | grant_consumed
  grant or grant_id plus mutation facts

mission_completed
  completed_by, completed_at
mission_abandoned
  abandoned_by, reason, selected follow-up actions
```

`process_ref` is an opaque runtime identifier, not an OS PID contract. Redacted
spec/reason fields contain enough diagnosis for history without storing secrets.

## 8. View methods — C-VIEW-001

### `view.load`

Params `{ device_id }`. Result contains schema version and the per-device values
listed in UX-PERSIST-001. Unknown Mission/Session IDs are retained for 30 days to
allow restored state, but ignored during rendering.

### `view.save`

Params `{ device_id, expected_view_version, state }`. Result contains new view
version. The runtime uses last-writer-wins only when caller supplies
`expected_view_version: null`; desktop normally uses optimistic concurrency.

View methods never emit Mission events.

## 9. Error mapping — C-ERROR-001

| Condition | Code | Retryable |
|---|---|---|
| malformed/invalid field | `invalid_request` | no |
| expected version or view conflict | `conflict` | yes after reload |
| absent object | `not_found` | no |
| Run dependencies incomplete | `not_ready` | yes after event |
| wrong lease holder/epoch | `not_controller` | yes after explicit transfer |
| missing agent or Grant capability | `capability_denied` | no |
| queue/session/storage limit | `resource_exhausted` | conditionally |
| unsupported protocol/schema | `unsupported_version` | no |
| Session spawn | `pty_spawn_failed` | conditionally |
| persistence unavailable | `storage_unavailable` | conditionally |
| persistence corruption | `storage_corrupt` | no; recovery action |
| history/event range gone | `recovery_required` | yes via snapshot/full frame |
| unexpected defect | `internal` | conditionally; correlation ID |

Domain-specific details MAY identify object IDs and current state but MUST NOT
expose tokens, terminal bytes, environment values, or unredacted argv.

## 10. Catalog compatibility — C-COMPAT-001

The implementation keeps golden JSON for every record, command, event, method,
success, and error. Rust types and generated documentation MUST round-trip those
fixtures. Field removal, semantic reuse, enum reinterpretation, or narrowing a
valid bound requires a major schema feature or migration. Additive optional fields
require a minor feature and a default projection behavior.
