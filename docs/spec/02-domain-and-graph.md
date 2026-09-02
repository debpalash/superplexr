# 02 — Domain and execution graph

The canonical definitions live in [`CONTEXT.md`](../../CONTEXT.md). This section
specifies relationships, lifecycle, commands, events, and invariants.

## 1. Aggregate and identity — D-IDENTITY-001

A Mission is the consistency aggregate for v1. Every durable domain object has a
globally unique UUID and belongs to exactly one Mission. IDs are never reused.
Display names are mutable labels and MUST NOT be used as identity.

The Mission event stream is the source of truth. Current Mission, Run, Session,
Signal, Artifact, Intervention, and attention views are deterministic projections.
Terminal bytes and frame checkpoints are Session records, not Mission events.

## 2. Graph model — D-GRAPH-000

The Run graph contains two distinct edge types:

```text
lineage:     parent Run ──created/delegated──> child Run
dependency:  prerequisite Run ──must succeed──> dependent Run
```

Lineage explains causality and authority. Dependency controls readiness. A child
does not automatically depend on its parent, and a dependency does not imply that
one Run created the other.

### D-GRAPH-001 — Shape

- A Mission MAY contain zero or more root Runs.
- A Run MAY have one lineage parent and any number of children.
- A Run MAY have zero or more dependencies and dependents.
- Lineage MUST form a forest; dependency edges MUST form a DAG.
- Both endpoints of every edge MUST belong to the same Mission.
- Self-edges, duplicate edges, and cycles MUST be rejected before commit.

### D-GRAPH-002 — Immutability

Once a Run enters `Running`, its Actor, objective, parent, dependencies, and
execution mode MUST NOT change. A materially different attempt is a new Run.
Pending Runs MAY be cancelled and replaced; v1 does not edit them in place.

### D-GRAPH-003 — Readiness

A pending Run is:

- `waiting` while any dependency is unfinished;
- `blocked` when any dependency finished with `Failed` or `Cancelled`;
- `ready` when every dependency finished with `Succeeded`.

Readiness is a projection, not a stored lifecycle state. A blocked dependent does
not start and is not silently cancelled. An Actor can cancel it, or create a new
Run that supersedes the failed prerequisite and rewire future work through a new
dependent Run.

### D-GRAPH-004 — Scheduling

> Preview implementation: `scheduler_plan` already classifies agent Runs by
> dependency state, excludes manual-human Runs, counts occupied slots, and
> deterministically selects `urgent`, `normal`, then `background` work under a
> caller-supplied cap. An explicit daemon-side configured-engine batch freezes
> that plan, records each selected Run's redacted driver snapshot, and launches
> each PTY with independent success/failure reporting. An owner-only durable
> per-Mission policy can opt into continuous reconciliation; disable prevents new
> launches without terminating active work, and the policy resumes after daemon
> restart. The launch admission gate enforces the persisted runtime-wide limit
> (twelve by default) across all Missions, while policy order rotates each
> interval so one
> Mission cannot monopolize newly opened capacity. The event store stamps
> planning/completion facts; dependency-free Runs age from planning, while a
> dependent Run ages from the latest successful prerequisite completion. Exact
> five-minute intervals promote one effective-priority level. Explicit, batch,
> and continuous launches all read the same owner-only global setting. All three
> execution modes remain required for the complete release scheduler.

Configured drivers may additionally require a host sandbox profile. Successful
resolution records the actual adapter backend/profile and network-isolation fact
in the immutable driver snapshot; a missing adapter fails before Run mutation.
Sandbox intent alone is never treated as enforcement.

The runtime starts ready Runs by priority, then readiness time, then Run ID for a
deterministic tie break. Priorities are `urgent`, `normal`, and `background`;
normal is default. Every five minutes of waiting promotes effective priority by
one level to prevent starvation. Default limits are twelve running Runs globally
and eight per Mission. Users MAY lower these limits.

Runs that require an interactive Session reserve that Session before entering
`Running`. Headless Runs require no Session. A Session cannot host two active Runs.

### D-GRAPH-004A — Execution mode

Every planned Run chooses one immutable execution mode:

- `headless`: the Run executor owns a child with bounded stdout/stderr records and
  no terminal Session;
- `new_session`: the runtime creates a Session whose leader is the agent command;
- `existing_session`: the Run reserves an idle Session and starts through verified
  shell integration.

Automatic `existing_session` start is allowed only when installed shell
integration reports a prompt-ready state, current directory, and no foreground
job over the restricted side channel. Terminal text that resembles a prompt is
never sufficient. Without that proof, the runtime requires a human to start the
command and explicitly bind it to the Run, or creates a new Session.

### D-GRAPH-005 — Fan-out, fan-in, and retries

- Fan-out is represented by several Runs sharing a parent or dependency.
- Fan-in is represented by one Run depending on several prerequisites.
- A retry is a new Run with `retry_of` pointing to one finished Run.
- `retry_of` is provenance; dependents are never rewired automatically.
- A retry target MUST be a different, finished Run in the same Mission.
- Cancelling a parent MUST NOT recursively cancel descendants without an explicit
  cascade command that previews the affected IDs.
- Rejecting a successful result does not rewrite its Outcome or retroactively
  cancel dependents that already started; the rejection view offers explicit
  pause/cancel/replan actions for affected Runs.

## 3. Run state — D-RUN-001

Run phase, outcome, disposition, and attention are orthogonal:

```text
Pending ──start──> Running ──pause──> Paused
   │                  │   <─resume───┘
   └──cancel──────────┴──finish──> Finished(outcome)

Finished(Succeeded) -> AwaitingReview -> Accepted | Rejected
```

### Phase

- `Pending`: recorded but not executing.
- `Running`: Actor is actively attempting the objective.
- `Paused`: execution intentionally stopped; reason is `attention`, `human`,
  `resource`, or `system`.
- `Finished`: attempt ended with exactly one Outcome.

### Outcome

- `Succeeded`: Actor reports a result capable of satisfying dependencies.
- `Failed`: attempt ended without satisfying its objective.
- `Cancelled`: attempt was deliberately stopped.

### Disposition

- `None`: Run has not produced a reviewable successful result.
- `AwaitingReview`: successful result has not been judged.
- `Accepted`: human accepted the result for the Mission.
- `Rejected`: human rejected the result; the immutable Outcome remains
  `Succeeded` because execution and judgment answer different questions.

Unresolved attention does not replace phase. A Running or Paused Run can both
have attention items. A successful Run with unresolved response-required Signals
MUST NOT finish until those Signals are resolved or explicitly withdrawn.

## 3A. Verified work delivery — D-DELIVERY-001

A writable Run MAY declare a versioned Change intent while Pending. The intent
identifies one repository, an exact base revision, and bounded exact-file claims
for create, modify, or delete operations. Admission issues an Execution lease
with a monotonically increasing epoch. A stale epoch MUST NOT authorize a write,
and two active intents with conflicting claims MUST NOT be admitted together.
Contingent claims do not grant write authority or participate in conflicts until
an explicit versioned promotion atomically rechecks conflicts and rotates the
Execution lease epoch.
Semantic or symbol claims MAY be advisory, but exact file claims MUST fail
closed before the protected effect.

A configured Run MUST record one immutable Harness snapshot before execution.
The snapshot identifies the objective, Driver, tool set, Skill set, sandbox,
delivered context, and evaluator contract using bounded metadata and content
digests. An absent capability is represented explicitly; it is not silently
omitted from the snapshot.

A successful writable Run submits one immutable Candidate before Settlement.
Candidate publication MUST carry the active Execution lease epoch when the Run
has a Change intent; a missing or stale epoch is rejected before publication.
The runtime MUST inspect the managed Run checkout outside terminal and async
control hot paths, compare every realized exact-file operation with committed
claims, and replace caller-supplied revision and digest fields with a bounded
Realized change manifest. Rename is normalized to delete-old plus create-new;
untracked files are creates; contingent-unpromoted and undeclared changes fail
closed. Inspection MUST use an isolated Git index and freeze the admitted bytes
as a retained Git commit without modifying the Run's index. Candidate and its
base-to-snapshot patch Artifact MUST commit atomically. Settlement MUST verify
the retained ref, snapshot tree, and patch digest; later checkout writes are
unpublished work and MUST NOT alter the already frozen Candidate.
Mission policy MAY require an independent verifier Run. Its Evaluation Receipt
MUST name the exact Candidate, verifier Run, checks, verdict, delivery result,
and repeatability result. CI or provider observations may support the receipt
but MUST NOT become a receipt or Settlement automatically.

An owner MAY accept or reject only the exact current Candidate. Acceptance under
an independent-verification policy requires a passing Evaluation Receipt for
that Candidate. Rejection preserves the Run's successful Outcome. A Handoff
Artifact and an evidence-backed Escalation Signal retain source references and
MUST NOT require a successor to replay an unbounded terminal transcript.

## 4. Session state — D-SESSION-001

```text
Creating -> Running -> Exited(code)
                    -> Lost(reason)
```

- `Creating` is operational and MUST resolve to Running or Lost.
- `Running` owns one leader process, one PTY, one canonical terminal model, one
  grid, one Control lease, and monotonically increasing sequences.
- `Exited` means the leader exited and remaining PTY output was drained.
- `Lost` means the runtime can no longer prove process or terminal continuity,
  including after daemon restart.

A Session MAY host ordinary shell work without a Run. It MAY host multiple Runs
sequentially. It MUST NOT host multiple active Runs. Session exit does not decide
a Run outcome; Run finish does not terminate its Session.

## 5. Control lease and Intervention — D-CONTROL-001

A running Session has exactly one Control lease. The durable holder is an Actor;
the operational holder also identifies a client and, for a human, a Surface.
Every transfer increments a `control_epoch`.

- Input or authoritative resize MUST include the current epoch.
- Stale-epoch input MUST be rejected and MUST NOT reach the PTY.
- Human takeover creates an Intervention and pauses agent input before the lease
  commit is acknowledged.
- Returning control names the receiving agent Actor explicitly.
- Disconnecting a human client does not transfer control. After a configurable
  30-second grace period the lease becomes `unattended`; agent input remains
  paused until the human reconnects or explicitly returns control from another
  client.
- Observer Surfaces are never lease holders. A Controller Share Surface may hold
  a lease only through a non-forced claim; its revocation releases the lease and
  increments the epoch.

## 6. Signals, attention, Grants, and Artifacts — D-SIGNAL-001

Signal kinds are:

- `Progress { summary, percent? }`
- `InputNeeded { question, context? }`
- `ApprovalNeeded { operation, risk, evidence?, requested_scope? }`
- `Blocked { reason, recovery? }`
- `ArtifactReady { artifact_id }`

Only InputNeeded, ApprovalNeeded, and Blocked create attention items. Resolution
is immutable and records Actor, response, timestamp, and optional Grant ID.
Progress percentage, when present, MUST be in `0..=100` and MUST NOT decrease for
the same progress stream.

An Artifact records name, media type, locator, digest when available, producer
Run, and creation time. A locator is data, not permission to open or execute it.

A Grant records subject Actor/Run, operation class, resource scope, decision,
issuer, issuance time, expiry or use count, and enforcement mode. Enforcement
mode is `cooperative` or `enforced`. The interface MUST display this distinction.

## 7. Domain commands — D-COMMAND-001

Commands include an expected Mission version and an idempotency key. Commands are
validated against one projection and append one atomic event batch.

Required v1 commands:

- `CreateMission`, `CompleteMission`, `AbandonMission`;
- `PlanRun`, `StartReadyRun`, `PauseRun`, `ResumeRun`, `FinishRun`, `CancelRun`,
  `CreateVerifierRun`, `RetryReturnedRun`;
- `AcceptRunResult`, `RejectRunResult`;
- `StartSession`, `AssignSession`, `InterruptSession`, `TerminateSession`;
- `TakeControl`, `ReturnControl`;
- `RaiseSignal`, `ResolveSignal`, `WithdrawSignal`;
- `RecordArtifact`, `IssueGrant`, `RevokeGrant`.

Verified-delivery commands additionally include `DeclareChangeIntent`,
`AdmitChangeIntent`, `PromoteContingentClaims`, `RecordHarnessSnapshot`,
`SubmitCandidate`, `FreezeDeliveryRunInput`, `RecordHandoff`, and
`RecordEvaluationReceipt`.

`CreateVerifierRun` MUST atomically plan a verifier and freeze the exact source
Candidate and HarnessSnapshot as its immutable delivery input.
`RetryReturnedRun` MUST additionally freeze the owner-authored Return note,
link `retry_of` to the source Run, and create a proposed exact-file ChangeIntent
against the frozen Candidate revision. A verifier or retry with managed evidence
MUST launch from its prepared Candidate checkout, never an unrelated policy CWD.

Commands MUST fail without events when the expected version is stale or an
invariant is violated. Repeating an idempotency key returns the original result.

## 8. Event envelope — D-EVENT-001

Every v3 domain event is stored in an envelope:

```text
event_id          UUID
mission_id        UUID
mission_sequence  u64, starts at 1 and is contiguous
schema_version    u16
occurred_at       UTC timestamp with microsecond precision
actor_id          UUID or well-known runtime Actor
correlation_id    UUID: user action or agent operation
causation_id      optional event/command UUID
idempotency_key   UUID
payload_type      stable snake_case string
payload           versioned object
```

Event payloads use past-tense names. Required payload families mirror the domain
commands, including `run_planned`, `run_started`, `run_paused`, `run_finished`,
`run_driver_resolved`, `run_result_accepted`, `session_started`, `session_assigned`,
`session_control_transferred`, `signal_raised`, `signal_resolved`,
`artifact_recorded`, and `grant_issued`.

### Side-effect choreography — D-EFFECT-001

External effects are driven by committed intent, never performed before it:

1. `PlanRun` commits the Run and its execution mode.
2. The scheduler selects a ready Run and commits `run_start_requested` with a
   stable attempt token.
3. The Run executor launches idempotently for that token.
4. Success commits `run_started`; launch failure commits `run_start_failed` and
   returns the Run to a reviewable Paused state.

Session creation follows the same pattern:

1. commit `session_start_requested` and project `Creating`;
2. create PTY/process once for the stable attempt token;
3. commit `session_started` or `session_start_failed`/`session_lost`.

Pending effects are recoverable records. After runtime restart, v1 never assumes
an unprovable process launch succeeded: a Creating Session with no live owned
handle becomes Lost, and a requested Run start becomes Paused for recovery rather
than launching a duplicate agent.

An event is never edited or deleted. Personal-data deletion, if later required,
uses encrypted payload keys or explicit redaction events; it does not rewrite the
causal sequence.

## 9. Mission completion — D-COMPLETE-001

A Mission MAY complete only when:

- every Run is Finished;
- no Run has an unresolved response-required Signal;
- every Session is Exited or Lost;
- every successful Run has Accepted or Rejected disposition.

Abandonment is allowed at any time but MUST require a preview and confirmation
when active Sessions or Runs exist. The runtime records abandonment first, then
performs explicitly selected cancellation/termination actions; abandonment alone
does not kill processes.

## 10. Required invariants — D-INV-000

- D-INV-001: IDs are unique and immutable.
- D-INV-002: all graph edges are Mission-local and acyclic by edge type.
- D-INV-003: an active Run uses at most one primary Session.
- D-INV-004: a Session hosts at most one active Run.
- D-INV-005: one Session has one Control lease and monotonically increasing epoch.
- D-INV-006: one Signal is resolved or withdrawn at most once.
- D-INV-007: successful completion cannot hide unresolved attention.
- D-INV-008: terminal or process exit never implies Run success.
- D-INV-009: view closure never creates lifecycle events for durable work.
- D-INV-010: dependency readiness is deterministic from the event stream.
- D-INV-011: domain event sequence is gapless within a Mission.
- D-INV-012: accepted/rejected disposition never mutates execution Outcome.

Property tests MUST generate command sequences and prove all invariants after
every accepted event batch and every rehydration.
