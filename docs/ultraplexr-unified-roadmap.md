# ultraplexr unified product, research, and delivery roadmap

**Status:** authoritative planning view  
**Updated:** 2026-09-06<br>
**Targets:** macOS and x86-64 Linux, including Wayland and X11  
**Planning horizon:** developer preview through production v1, followed by
explicitly separated post-v1 research

This document unifies the actionable conclusions from:

- [the agent-systems research review](arxiv-agent-systems-review-2026-09-01.md);
- [the multiplexer and agent-workspace feature survey](multiplexer-feature-survey.md);
- [the v1 delivery plan](spec/09-delivery-plan.md);
- [the quality and release contract](spec/08-quality-and-release.md); and
- [the verified-delivery implementation status](verified-delivery-status.md);
- [the product north star and Superlogical evidence bar](product/north-star.md);
  and
- [the executed runtime/release evidence ledger](engineering/m2-runtime-release-evidence.md).

The source documents remain the evidence record and bibliography. This file is
the single planning view: what ultraplexr is, what exists, what remains, what comes
later, and what is deliberately excluded. Normative requirements in `docs/spec`
still win if wording conflicts.

### Development update: lifecycle and observer foundation

The current branch adds persisted desktop dismissals, duplicate-view group
reconciliation, and a reconnectable native client transport adapter. Real-daemon
tests cover two-client edits, dismissal reload, detach/reattach, archive/restore,
and scoped observer access over an alternate byte transport.

The observer now consumes native frame events, reconnects with the same scoped
identity, and exposes independent retained-history navigation. Forced transport
loss and HTTP reattachment tests preserve Session ID and PTY PID; revocation
still ends access. An explicit macOS test launches the actual desktop twice and
checks persisted names, pins, dismissals, and process continuity. Measurements
and remaining limits are in the [local baseline](engineering/observer-streaming-baseline.md).

The optional [browser observer prototype](design/browser-observer-prototype.md)
is a localhost-only text/SSE projection, read-only by default. Explicit
Controller Share mode now supports per-view Control handoff, ordered/non-replayed
input, confirmed paste, resize, and retirement on detach/reconnect. Real-daemon
tests cover two-browser contention, stale input, scope, origin checks, and
revocation. Automated and rendered-browser results are in the
[Control evidence ledger](engineering/browser-control-evidence.md). It is an experimental
post-v1 client slice, not a change to the macOS/Linux release contract or a
claim that browser parity, public remote collaboration, embeds, full TUI parity, or footprint
budgets have shipped. Broader direction and remaining limits are recorded in
[universal runtime and clients](architecture/universal-runtime-and-clients.md).

An optional [focused-session TUI](design/focused-session-tui.md) now provides the
terminal-client proof on the same runtime/client layer. It defaults to observation,
requires explicit non-forced Control for input/resize, preserves Session/PTY
identity across detach and dropped links, and restores the outer terminal after
normal exit, signals, and revoked access. Scoped Mission/Session/Attention
navigation and two-pane side-by-side/stacked views are now implemented. Selecting
Attention only observes its primary Session; approval remains explicit outside
this view. Focus changes return Control and do not duplicate PTYs. Richer layouts,
remote/Linux acceptance and nonblocking command handling remain follow-up work;
this is not a new runtime or full desktop parity.

### Development update: verified-delivery acceptance journey

Two real-daemon integration tests now exercise a producer/verifier/return/retry/
acceptance journey with immutable Candidates, actual fixture check execution,
digested evidence, and daemon restarts; interruption after publication stays
failed and does not relaunch. These tests are part of the normal workspace suite.
This is macOS headless acceptance evidence, not an automated landing, desktop
interaction, or Linux acceptance claim. See
[the journey evidence](engineering/delivery-journey-evidence.md).

The production CLI now also has an opt-in
[bounded verification runner](design/bounded-verification-runner.md): explicit
owner-configured checks execute in a verifier Run, and a separate resumable
collection command records their exact evidence and receipt without accepting
work. Bounds, cancellation, stale/tampered evidence and restart are tested.
This is trusted local Unix execution, not a sandbox or cross-host verifier.

The desktop now configures and reviews these plans, launches through the same
`ultraplexr-verification` library, and collects/inspects receipts. Choices persist
per Mission and verifier without automatic execution. Plan SHA-256 is checked
before launch and again in the worker; integration tests cover mutations at both
boundaries and pending-verifier identity reuse. Native visual QA remains open:
the isolated macOS window launched, but Screen Recording access was unavailable.
These additions do not establish platform parity or production readiness.

An optional [Rust-project recipe](design/rust-project-verification.md) now
generates normal plans without execution. Real Cargo-project integration tests
exercise compilation/tests, fresh installation with expected output, local-input
provenance, two-build executable comparison and post-restart collection. Wrong
delivery and external path dependencies produce failing receipts. This is a
reusable binary-project recipe with local fixture evidence, not acceptance for
Ultraplexr's full build, hermetic provenance, all project types or other platforms.

### Development update: retained history and release resource evidence

Two real 100,000-row regressions now cover batch and chunked PTY output. The
terminal adapter explicitly configures both row and byte retention limits;
previously Ghostty's smaller default byte cap defeated the requested history.
The isolated desktop/TUI/HTTP benchmark and its limits are recorded in
[shared-client resource evidence](engineering/shared-client-resource-evidence.md).
The initial owner-desktop sample changed the reference grids and cannot certify
the budget. The corrected benchmark uses a real Observer desktop with explicit
grid/history checks. Its 300-second Apple M2 release sample measured runtime CPU
at 1.156% (target <= 0.5%) and peak runtime-plus-desktop RSS at 1,065.48 MiB
(target <= 600 MiB). Desktop CPU was 0.373%; idle SSE frames and TUI bytes were
zero. A subsequent [idle-maintenance implementation and measurement](engineering/idle-history-maintenance.md)
passed 365 Rust tests plus 20 browser/harness tests. The same five-minute workload
measured runtime CPU at 0.323% and desktop CPU at 0.357%, meeting those local
targets, with history/identity intact and no idle terminal updates. Peak combined
RSS was 1,101.47 MiB: that sample missed the memory target.

The subsequent [macOS native-reclamation milestone](engineering/macos-history-reclamation.md)
adds a reproducible hash-checked Ghostty patch and explicitly schedules maintenance
after search/clipboard formatting restores history. The normal release workspace
passed 372 Rust tests plus 20 browser/harness tests, with Clippy and license checks.
The unchanged five-minute reference sample measured runtime CPU 0.317%, desktop
CPU 0.377%, and combined peak RSS 187.44 MiB, meeting those local targets. All
history/grid/identity checks passed with no idle SSE/TUI output. This closes the
local reference-sample memory miss, not cross-platform certification, startup
budgets, sustained-output/search performance, or full Q-PERF-003 acceptance.

### Development update: bounded canonical history search

The [bounded-search milestone](engineering/bounded-history-search.md) adds an
actor-local scanner with bounded row/buffer/result steps, a four-page result
queue, cancellation, admission limits, and a five-second job deadline. Actor
input/snapshot/exit remain responsive with a paused result consumer. The server
waits off the async executor; a shutdown regression now confirms actor death
before replaying closed history, without treating an expired live search as a
stopped Session. The release workspace passed 381 Rust tests plus 20
browser/harness tests, Clippy, and formatting checks. Wire-visible result pages,
client cancellation and cell coordinates/history epochs remain open. In-flight
Share authority is addressed by the following milestone; neither closes
T-HISTORY-001 or certifies production remote collaboration. The rebuilt
five-minute macOS reference sample passed all
six harness observations: runtime CPU 0.320%, desktop CPU 0.380%, combined peak
RSS 107.63 MiB, retained history/identity intact, and no idle SSE/TUI output.
Other workloads, rendered-browser resources and platform evidence remain open.

### Development update: in-flight Share authority

The [Share authority milestone](engineering/in-flight-share-authority.md) guards
ordinary shared RPCs through computation, writer queueing and response writing.
Revocation/expiry retires the connection and its tasks instead of returning a
delayed capture. A durable recheck covers the writer-admission race, and missed
revocation intervals fail closed. Control-holder comparisons and disconnect
cleanup now bind authenticated Share identity as well as client/Surface identity;
real-wire regressions reject spoofed owner coordinates and unauthorized cleanup.
Cancelled explicit waits release their slot and forwarding thread without waiting
for new PTY output. The release workspace passed 391 Rust tests plus 20
browser/harness tests and Clippy. This is local macOS evidence, not public TLS,
device enrollment, transactional effect rollback, or a completed security audit.
Next work includes propagating search cancellation to jobs and exposing bounded
pages consistently across clients, alongside the remaining platform/release gates.

### Development update: awaitable search and cooperative cancellation

The [awaitable-search milestone](engineering/awaitable-history-search.md) removes
the blocking live-page collector and connects request-future cancellation to
queued admission and actor work. Archived replay owns one queued page and stops
cooperatively when its consumer disappears; an expired search replay cannot
claim successful results from a partial model. Real-PTY tests preserve process
identity/input across cancellation and readmission, and native archive tests
prove worker retirement under backpressure. The release workspace passed 399
Rust tests plus 20 browser/harness tests, Clippy, and formatting checks. These
are local macOS results. Wire/client paging, byte-budgeted previews, immediate
transport-loss cancellation and platform/security acceptance remain open.

### Development update: acknowledged wire search streams

The [acknowledged-search milestone](engineering/acknowledged-search-streams.md)
adds negotiated page streams on the shared connection, with <=512 KiB/64-match
wire pages, acknowledgement-driven backpressure, explicit cancellation and
four-per-connection/eight-per-process stream caps. The shared native client
preserves negotiated capabilities and falls back to the legacy collected RPC
only when streaming is unavailable. `terminal-search-pages` emits incremental
JSON lines. Revocation, disconnect, cross-connection isolation, byte bounds,
partial-write cancellation and quota release have real-runtime/socket coverage.
The tests also exposed and fixed cancelled-job readmission and exit-summary
publication races. The release workspace passed 410 Rust tests plus 20
browser/harness tests, Clippy and formatting checks. Dedicated incremental
desktop/TUI/browser search UI, broader native memory bounds, coordinate semantics,
remote security and platform/release acceptance remain open.

### Development update: incremental TUI history search

The [TUI search milestone](engineering/tui-streaming-search.md) adds `Ctrl-] /`
literal queries, incremental results, cancellation, Unicode editing/paste and
read-only history navigation. Two reused background workers handle stream/history
I/O and cancellation; a latest-command mailbox prevents per-query thread growth
and rejects obsolete output. Displayed previews are capped at 2 MiB/1000 matches.
Reconnect clears results rather than replaying search, and revoked access detaches
without stopping the host. The release workspace passed 417 Rust tests plus 20
browser/harness tests and Clippy. This is macOS real-daemon/outer-PTY evidence,
not full TUI parity or a new performance/platform certification. Desktop/browser
incremental search, stable history coordinates, nonblocking remaining TUI commands
and the broader remote/platform/release gates remain open.

### Development update: oversized-record termination

The [oversized-record milestone](engineering/oversized-record-recovery.md)
distinguishes intrinsically inadmissible subscription records from temporary
queue saturation. Native metadata, desktop, TUI and browser feeds stop retrying
the former with an explicit resource-limit reason. Queued and held terminal data
is invalidated, old input remains retired, and explicit recovery preserves the
PTY. Repeated real-daemon testing exposed cleanup reconnecting merely to release
obsolete Control; release now uses only the existing connection. The final
release workspace passed 475 Rust tests plus 29 browser/harness tests, strict
Clippy, formatting, five executable builds and native-keyboard/CLI multiplex
smoke checks. The tightened cleanup regression passed 20 consecutive standalone
runs. This is not large-record support or a new footprint/platform certification.
Next: complete collection snapshot/replacement semantics and retained GUI-state
reconciliation, including items deleted while disconnected. Chunking/paging,
metadata-feed status UX and production remote/platform gates remain open.

### Development update: pull-driven metadata

The [metadata handoff milestone](engineering/pull-driven-metadata.md) removes
the four native metadata relay threads and unbounded decoded FIFOs. Desktop
handoffs have one queued delivery per feed; the CLI has four shared slots.
Connection/subscription-pinned deliveries are checked at use, so data held
before cancellation, observed logical closure or reconnect cannot update the
UI afterward. Cancellation wakes idle reads without a GUI socket write, and
read-only reconnect occurs on consumer demand. The release suite passed 470
Rust tests plus 28 browser/harness tests, Clippy, formatting, release builds and
native-keyboard/CLI multiplex smoke checks. Four real-daemon regressions passed
an extra standalone run. The fresh five-minute shared-client resource sample
passed all six reference observations: 120.33 MiB runtime+desktop sampled peak
RSS, 0.320% runtime idle CPU, 0.367% desktop idle CPU and zero idle SSE/TUI output.
History, grid and PTY identity checks passed; this is one macOS idle workload,
not a claim of causal memory savings or cross-platform performance certification.
Complete metadata snapshot/replacement semantics, oversized-record admission,
retained GUI-state invalidation, other queue/response budgets and broader
platform/security/workflow acceptance remain open.

### Development update: bounded native receive queues

The [native receive milestone](engineering/bounded-native-receive.md) registers
one ordered mailbox at subscription ACK dispatch, with per-subscription,
connection and process count/byte limits. Overflow interrupts both transport
directions, discards queued data and invalidates connection-pinned input.
The targeted real-daemon regression verifies Control release, same-PTY recovery,
a fresh snapshot, no automatic input claim and rejected stale input. Socket tests
cover pending RPCs, writer-lock independence and final-owner teardown. Server
unsubscribe joins the cancelled producer before its ACK; interrupted partial
subscription frames close the socket before another writer can use it.
The final release workspace passed 466 Rust tests plus 28 browser/harness tests,
Clippy and formatting checks. All five release executables rebuilt; isolated
native-keyboard and CLI multiplex smoke checks passed. Higher-level metadata
relay queues, oversized-snapshot admission/retry policy, response/decoder budgets,
remaining blocking client calls and platform/security/resource acceptance remain
open. This milestone does not replace the target daemon-side reserved-capacity
and per-stream resync protocol with a claim of full completion.

### Development update: concurrent terminal waits

The [wait-dispatch milestone](engineering/concurrent-terminal-waits.md) moves
terminal waits off the serial control-connection request loop, so input can
satisfy a wait on the same socket. Replies correlate by request ID; Control/input
mutations remain ordered. Eight tasks per connection and 32 permits per process
bound observation, response delivery and retiring native forwarders. Disconnect
and revoked access cancel waits without stopping Sessions; partial response
cancellation shuts down the socket before writer reuse. The full release suite
passed 450 Rust tests plus 28 browser/harness tests, Clippy and formatting checks.
Wait/input regressions passed three additional runs; all five release executables
rebuilt and isolated native-keyboard/CLI multiplex smoke checks passed.
Other ordinary long reads and the agent channel remain serial. Per-request wait
cancellation, native receive/response byte budgets, interruptible client writers,
platform/security acceptance and sustained resource evidence remain open.

### Development update: bounded desktop input delivery

The [desktop input milestone](engineering/bounded-desktop-input.md) replaces
unbounded command/error FIFOs with 128-command/8 MiB retained-payload admission
and one latest-status slot. Input is bound to the exact connection and explicit
Control ACK; overflow, uncertainty and lifecycle changes discard waiting input.
Observation/metadata never re-arm retired input, and stale pre-claim metadata
cannot retire a freshly acknowledged lease. Archived Surfaces no longer create
input workers. The full release workspace passed 442 Rust tests plus 28 browser/
harness tests, Clippy and formatting checks; all five executables rebuilt and
the real macOS release-desktop keyboard smoke passed in an isolated fixture.
This closes desktop input admission/retirement, not already-admitted effect
rollback, native receive bounds, interruptible transport teardown, blocking RPC
head-of-line behavior, full reconnect/resize UX or platform/security acceptance.
No fresh performance measurement is claimed for this binary.

### Development update: bounded desktop repaint delivery

The [desktop feed milestone](engineering/desktop-coalesced-feed.md) replaces the
unbounded event-to-GPUI FIFO with a latest canonical frame, four fixed notice
slots and one repaint wakeup. Every delta is applied; stale intermediate paints
are coalesced. Reconnect remains observable across a fast snapshot, rejection
invalidates already-taken batches, and old attachment tasks cannot repaint hidden
or reattached surfaces. Exit also clears pending paste/composition. The full
release workspace passed 434 Rust tests plus 28 browser/harness tests, Clippy and
formatting checks, and all five release executables rebuilt. Native receive and
input queues, cancellation ownership, hidden/archive authority coverage and
production remote/platform/release acceptance remain open. The linked audit
records those boundaries explicitly; this is not an end-to-end memory/security
certification.
The rebuilt five-minute shared-client reference sample passed all six harness
observations: runtime CPU 0.310%, desktop CPU 0.373%, combined runtime-plus-desktop
peak RSS 162.59 MiB, preserved history/grids/identity, and zero idle SSE/TUI output.
This is one macOS idle workload, not sustained-output or cross-platform evidence.

### Development update: browser incremental history search

The [browser search milestone](engineering/browser-streaming-search.md) adds
scoped POST/SSE search, incremental bounded results, cancellation and independent
read-only history navigation. Four gateway search slots stay owned until their
native workers retire, including after browser detach. Revocation interrupts a
backpressured relay; partial or mismatched streams never report completion.
The full release workspace passed 427 Rust tests plus 28 browser/harness tests,
Clippy and formatting checks, and all five release executables rebuilt. Isolated
rendered Chromium checks covered search, Control return, history/live navigation,
throttled cancellation, plain-text previews, revocation and a 390 px viewport.
Stable history coordinates, broader client parity, comprehensive authority/queue
audits, production remote identity, sustained resource evidence and platform/
release acceptance remain open. These checks do not close the universal goal.

### Development update: desktop incremental history search

The [desktop search milestone](engineering/desktop-streaming-search.md) moves
the TUI's bounded search/history worker into the native client and reuses it from
the existing sidebar. Query edits cancel immediately and debounce admission;
pages arrive incrementally with explicit errors/limits and a Cancel control.
Stable workspace/group/terminal membership is checked before opening results.
History opens in a read-only local viewport, preserving the shared live frame
and blocking input/resize until return. Explicit live-feed reconnect/rejection
notifications retire search state. The full release workspace passed 423 Rust
tests plus 20 browser/harness tests, Clippy and formatting checks. Browser search,
stable history coordinates, general desktop event-queue/authority hardening,
native visual QA, remote acceptance and the platform/release gates remain open.

## 1. Product thesis

ultraplexr is a local-first, agent-native terminal multiplexer for one developer
supervising parallel human and agent work. It combines an excellent ordinary
terminal with a deterministic execution-control plane around probabilistic
agents.

The application is organized as browser-style Mission workspaces. Each Mission
has a compact Session navigator and a responsive, gapless waterfall of terminal
Surfaces. A background runtime owns durable PTYs and canonical terminal state,
so closing or crashing the desktop does not end work.

Its differentiation is not “more panes” or “more autonomous agents.” It is the
ability to answer, from durable evidence:

1. What work was authorized?
2. Which harness, context, tools, skills, model, and policy governed it?
3. Which actor and Session performed it?
4. What effects and exact Candidate were produced?
5. What needs human judgment now?
6. What did an independent verifier establish?
7. What did the owner accept, return, retry, or stop?
8. Can another actor resume without replaying an unbounded transcript?

```text
Mission plan
  -> isolated and admitted Run / Attempt
  -> durable Session + observable effects
  -> Candidate + Handoff + evidence
  -> independent verification when policy requires it
  -> owner Settlement
  -> provenance-preserving continuation, retry, or archival
```

## 2. Canonical model

The existing Mission graph remains the only durable orchestration truth.
Kanban, inboxes, problem lists, timelines, and dashboards are projections.

| Term | Meaning |
|---|---|
| Mission | Durable objective, governance scope, and event-sourced graph. |
| Run | One bounded Attempt to advance a Mission objective. |
| Session | Durable PTY/process lifetime; it is not the work item itself. |
| Surface | One client presentation of a Session. |
| Signal | Structured request, progress fact, escalation, or attention source. |
| Artifact | Bounded evidence linked to durable source events. |
| ChangeIntent | Versioned declaration of exact resources and operations a writable Run proposes. |
| Execution lease | Runtime-authored epoch that grants and fences one admitted writable Attempt. |
| HarnessSnapshot | Immutable identity of task, Driver, context, tools, skills, sandbox, and evaluator. |
| Candidate | Immutable revision/content digest submitted for verification and Settlement. |
| EvaluationReceipt | Independent result for the exact Candidate, including checks and delivery validation. |
| HandoffArtifact | Bounded, evidence-linked state needed by a successor or reviewer. |
| Settlement | Owner judgment that accepts or returns a Candidate without rewriting execution Outcome. |

Future records extend this model rather than create a second workflow system:

| Planned record | Purpose |
|---|---|
| EffectRecord | Requested effect, authority, idempotency key, realized result, and verification. |
| ContextReceipt | Exact typed context delivered, original source role/trust, omissions, size, and management cost. |
| RecoveryManifest | State domains, effects, leases, assumptions, nondeterminism, and reconciliation result. |
| FailureDiagnosis | Derived critical event/range, violated invariant, evidence, cause/symptom distinction, and supersession. |
| ExecutionBranch | Provenance-linked fork over reversible Run state, never a claim that all external effects are reversible. |
| WebEvidenceArtifact | URL, retrieval time, digest, excerpt span, citation, method, freshness, and degraded-state flags. |

## 3. Current implementation baseline

### 3.1 Implemented and validated locally

- Rust workspace with separated core, runtime, protocol, client, CLI, desktop,
  terminal, and plugin modules.
- Durable daemon-owned POSIX PTYs, detach/reattach, canonical frames, retained
  history, archive, resize, search, selection, clipboard, input, and process
  lifecycle.
- Ghostty-derived backend-neutral terminal state and a GPUI custom renderer.
- Browser-like Mission tabs in the titlebar, compact Session sidebar, gapless
  terminal waterfall, focus mode, command deck, native menus, and persistence.
- Event-sourced Missions, Run lineage and dependencies, deterministic readiness
  and scheduling, phase/outcome/disposition separation, and history replay.
- Structured Signals, Attention, Escalation, Artifacts, cooperative Grants,
  human takeover/return, and a graph inspector.
- Exact-base managed Git worktrees with conservative retirement.
- Restricted agent side channel, configured engine Drivers, provider-neutral
  activity facts, SCM/CI evidence, capture, bounded waits, and NDJSON events.
- Owner SSH attachment plus scoped Observer and non-forced Controller Shares.
- Workspace-write sandbox seam through macOS Sandbox and Linux Bubblewrap.
- Out-of-process executable plugins with manifests, capability filtering,
  bounded non-blocking queues, health, restart isolation, themes, and a
  first-party Agent Status proof plugin.
- Versioned ChangeIntents, atomic cross-Mission conflict admission,
  server-authored lease epochs, contingent-claim promotion, launch gating,
  HarnessSnapshots, lease-fenced Candidates, Handoffs, independent
  EvaluationReceipts, and Settlement rules.
- Managed-checkout realized-change inspection before Candidate publication,
  with authoritative manifests, rename normalization, exact-scope rejection,
  an isolated-index immutable Git snapshot, and an atomic patch Artifact.
- Native verified-delivery review showing intent, harness, Candidate, realized
  changes, Handoff, receipt, Accept, and operator-authored Return.
- Preview protocol version 25 with multiplexed subscriptions over one
  connection; terminal data uses protobuf and negotiated zstd.
- Validation counts are milestone-specific; see the linked execution-evidence
  documents rather than treating historical counts as current regression status.

### 3.2 Honest completion estimate

| Goal slice | Approximate completion | Main reason it remains open |
|---|---:|---|
| Runnable multiplexer foundation | 80% | Cross-platform terminal and recovery acceptance is incomplete. |
| Agent-native control plane | 65% | External-effect enforcement, native workflow acceptance, richer evidence, and context/recovery records remain. |
| Superlogical-class operational parity | 70% | Review, SCM workflow, adapters, templates, mailbox, and comparison need completion. |
| Production macOS/Linux release | 35% | Physical Linux evidence, accessibility, fuzz/soak, performance certification, migration, and distribution remain. |
| Entire stated production-v1 goal | **55–60%** | Approximately **40–45%** remains. |
| Broader universal-product vision | **About 50%** | Client parity, production remote identity/collaboration, embeds/SDKs, Windows, wider platform acceptance, and certified resource budgets remain. |

These percentages describe implementation plus proof. A feature is not counted
as production-complete merely because its data structure or happy path exists.

### 3.3 Superlogical public-plan comparison

The comparison is against Superlogical's public plan, not assumptions about a
private implementation. “Superior” remains an evidence gate rather than a
styling or technology claim.

| Public-plan capability | Current ultraplexr position | Remaining proof/work |
|---|---|---|
| Long-lived local terminal Sessions | Implemented | Cross-platform crash/upgrade/reconnect certification. |
| Preserved operational history | Mission causality, terminal journals, capture, and search implemented | Retention, disk-full, migration, and long soak gates. |
| Coding-agent/background work | Runs, scheduler, configured Drivers, worktrees, agent channel, status, realized-change enforcement, frozen verifier/retry Runs, bounded checks, and receipts implemented | Real-project recipes, native interaction acceptance, adapters, budgets, and Linux proof. |
| Production-safe intervention | Surface epochs, takeover, return, interrupt, termination, Signals, and Grants implemented | Broader adversarial authorization and enforced effect policy. |
| Structured composition beyond bytes | Mission graph, Artifacts, dependencies, Handoffs, Candidates, Receipts, plugins, and CLI implemented | Mailbox, templates, typed references, semantic review, and SDK. |
| Remote owner attachment | OpenSSH attachment to one authoritative runtime implemented | Fleet provisioning, diagnostics, and cross-platform E2E. |
| Shared terminals | Revocable Observer and scoped non-forced Controller roles implemented | Wider role/accessibility proof; concurrent editing remains later scope. |
| Web/mobile access | Not a v1 dependency | Authenticated attention-first clients are post-v1. |

ultraplexr exceeds a stream-only model only when the complete Mission journey is
proven: delegate, observe, receive structured attention, intervene, reconnect,
review causal evidence, verify the exact Candidate, and settle it on macOS,
Wayland, and X11.

## 4. Research-derived invariants

1. A Run is not reproducible unless its Harness and evaluator are identified.
2. Process exit, a final message, or agent confidence cannot settle work.
3. Every agent needs a policy-backed escalation path for defective work.
4. A Handoff must be useful without replaying an unbounded transcript.
5. Provider compaction must never delete durable evidence.
6. A restored checkpoint is not resumable until assumptions and external
   effects are reconciled.
7. An alert after a privileged effect is observability, not prevention.
8. Repository text, tool output, skills, and agent-authored notes retain their
   source trust and original model-facing role when copied.
9. Evaluation distinguishes authorized labor, realized effects, result
   provenance, final delivery, and repeatability.
10. Failure diagnosis is derived and disputable; source events stay immutable.
11. Worktree isolation does not replace semantic conflict admission.
12. Old workers cannot publish under a transferred or expired lease.
13. Only verified facts advance authoritative Mission state.
14. Coordination protocols transport facts; they do not own governance,
   acceptance, dissent, or audit.
15. Parallelism is useful only when critical-path savings exceed handoff,
   conflict, and integration cost.

## 5. Unified capability catalog

Status labels:

- **Implemented:** present in the repository and locally tested.
- **Partial:** useful foundation exists but the complete user journey or proof is missing.
- **V1:** required before production v1.
- **Post-v1:** valuable after the v1 trust and quality gates.
- **Experimental:** requires local evidence before product commitment.

### 5.1 Durable execution and workspace isolation

| Capability | Status | Remaining work |
|---|---|---|
| Daemon-owned PTYs and durable Sessions | Implemented | Complete platform acceptance, crash/disk-full, and long soak evidence. |
| Managed worktree per Run | Implemented | Bind it by default to writable agent policy and review workflow. |
| Preflight and conservative cleanup | Partial | Add dirty, unpushed, hook, and failure UI across all paths. |
| Exact ChangeIntent admission | Implemented | Add broader resource types only after file-level evidence justifies them. |
| Realized-change Candidate admission | Partial | Scanner, exact comparison, immutable snapshot, patch Artifact, promotion, and Settlement verification work; add generated classification and remaining adversarial/platform evidence. |
| Execution lease and Candidate fencing | Implemented | Add heartbeat/reconciliation and fence external effect adapters. |
| Setup/teardown hooks | V1 | Closed stdin, bounded runtime, captured output, explicit failure, no silent cleanup. |
| Git branch/commit/diff facts | Partial | Candidate snapshot and patch Artifact are automatic; add changed-file navigation, semantic impact, and landing workflow. |
| Retry/resume policy | Partial | Add explicit retry-from-return and continuity-aware resume policy. |
| RecoveryManifest | V1 | Reconcile process, worktree, tool, lease, approval, nondeterminism, and external state. |
| Effect-aware checkpoints | Post-v1 | Classify turns and checkpoint only recovery-relevant state. |
| Diff-based process/sandbox branches | Experimental | Evaluate OverlayFS/process checkpoint backend without claiming semantic rollback. |

### 5.2 Agent identity, status, attention, and supervision

| Capability | Status | Remaining work |
|---|---|---|
| Restricted Run identity and agent channel | Implemented | Expand adversarial authorization matrix on both OSes. |
| Structured Signals and Escalation | Implemented | Add richer proposed alternatives and resolution actions in native UI. |
| Explainable activity precedence | Implemented | Ship more provider adapters and an explicit `status explain` surface. |
| Stable Session ordering with dot-only state changes | Implemented | Cross-platform visual/accessibility acceptance. |
| Oldest/highest-risk attention navigation | Partial | Finish global keyboard action and problem aggregation. |
| Token/context/cost/model facts | Partial | Normalize adapter contract without scraping credentials. |
| Deterministic Supervisor projection | V1 | Group redundant questions, rank decisions, route answers by capability epoch. |
| Heartbeats and orphan reconciliation | V1 | Treat missed heartbeats as observations; conservatively reassign leases. |
| Confidence-driven early termination | Rejected | Confidence may suggest verification but never create truth or Settlement. |

### 5.3 Semantic terminal control and automation

| Capability | Status | Remaining work |
|---|---|---|
| Canonical structured capture | Implemented | Extend process/mode metadata and stability fixtures. |
| Text, quiet, exit, and lifecycle waits | Implemented | Add Signal, Artifact, mailbox, and attention wait predicates. |
| Push event stream | Implemented | Golden CLI/API compatibility and load evidence. |
| Exact key/text/binary-safe input | Implemented | Full IME, Kitty keyboard, paste, and physical-input matrix. |
| Typed SDK and MCP adapters | Post-v1 | Generate over the stable protocol; do not wrap tmux. |
| Capability-checked semantic computer interface | Post-v1 | Add file, symbol, test, Git, and browser facts while keeping PTY fallback. |

### 5.4 Coordination and durable task state

| Capability | Status | Remaining work |
|---|---|---|
| Mission DAG and scheduler | Implemented | Add cohesion-aware partitioning and dependency-wave recommendations. |
| Work/Attempt/Settlement separation | Implemented | Make sibling Candidate comparison and synthesis first-class desktop journeys. |
| Independent verification policy | Implemented foundation | Execute normalized checks, issue independent receipts, and prove the journey on Linux. |
| Run mailbox | V1 | Event-sourced threads with unread/read/acknowledged state; never write into PTY or steal focus. |
| Typed cross-agent references | V1 | Resolve Mission/Run/Artifact/file/symbol/commit/diff references under receiver capability. |
| Read-only Scout Run | V1 | Verify reproduction claims before handing off or routing. |
| Goal continuation with limits | V1 | Stop/continue decisions bounded by evidence, turn/time/token/cost, and recorded as events. |
| Fan-out compare and synthesize | V1 after core gate | Frozen identical inputs, isolated worktrees, normalized evidence, explicit winner or synthesis Run. |
| Learned routing/intervention | Experimental | Must beat a deterministic cheapest-capable baseline by a measured margin. |

### 5.5 Source control, CI, review, and provenance

| Capability | Status | Remaining work |
|---|---|---|
| Provider-neutral SCM/CI evidence | Implemented | Add production GitHub/GitLab/Bitbucket/Azure adapters. |
| Native verified-delivery review | Partial | Rich patch, files, checks, install/delivery evidence, and inline comments. |
| Review-and-return | Implemented foundation | Bundle inline comments into one structured Return. |
| Realized-change verifier | Implemented | Add generated-file provenance, external-effect fencing, race coverage, and Linux proof. |
| Candidate checkpoint provenance | Implemented foundation | Attach executed tools, tests, install evidence, and independent receipts. |
| Semantic diff and blast radius | V1/P1 | Optional Tree-sitter adapter; remain advisory until accuracy is proven. |
| Git Notes attribution export | Post-v1 | Explicit opt-in provenance; never heuristic AI detection. |
| Landing/merge | V1 | Previewed owner action with checks; no automatic or forceful merge. |

### 5.6 Context, memory, handoff, and diagnosis

| Capability | Status | Remaining work |
|---|---|---|
| Bounded HandoffArtifact | Implemented foundation | Auto-generate at pause/replacement/timeout and measure handoff debt. |
| Evidence-preserving context summary | V1 | Cite source event ranges, mark omissions/conflicts/uncertainty, retain full evidence. |
| ContextReceipt | V1 | Record object IDs, source role/trust, full/summary form, size, omissions, and management cost. |
| FailureDiagnosis | V1/P1 | Derived critical event/range and evidence-backed problem view; allow dispute/supersession. |
| Owner event export | V1 | Versioned scrubbed schema for actions, costs, effects, Artifacts, SCM, CI, and review. |
| Consequence-aware context optimization | Experimental | Learn only after re-fetch, token, latency, and outcome telemetry exists. |

### 5.7 Mission templates, automation, budgets, and health

| Capability | Status | Remaining work |
|---|---|---|
| Deterministic scheduling and concurrency | Implemented | Add resource pressure and cohesion signals. |
| Continuous scheduler policy | Implemented | Complete owner UI and cross-platform recovery evidence. |
| `.ultraplexr/mission.toml` | V1 | Versioned Runs, dependencies, Drivers, worktrees, hooks, policies, and idempotent reconciliation. |
| Scheduled/webhook Missions | V1/P1 | Prechecks, durable triggers, and no duplicate launch. |
| Hierarchical budgets | V1/P1 | Runtime/Mission/Run/provider token, cost, time, retry, and slot warnings/hard stops. |
| `doctor` and bounded `fix` | V1 | Inspect daemon, socket, terminfo, renderer, hooks, Drivers, sandbox, permissions, and protocol. |
| Resource-aware scheduling | V1/P1 | Attribute CPU/RAM/I/O without recording secrets; reduce concurrency under policy. |
| Harness Experiment Mission | Post-v1 | Frozen cohort, falsifiable prediction, stop condition, measured promotion, instant rollback. |

### 5.8 Terminal and desktop experience

| Capability | Status | Remaining work |
|---|---|---|
| Browser-like titlebar workspaces | Implemented | Final overflow, drag, keyboard, accessibility, and persistence acceptance. |
| Compact Arc/Zen-inspired Session navigator | Implemented foundation | Density presets, richer actions, non-color status, and Linux polish. |
| Gapless responsive terminal waterfall | Implemented | One-to-four columns, resize stability, six-visible-Surface performance proof. |
| Hidden intelligent overlay scrolling | Implemented | Platform and accessibility acceptance. |
| Geometric Surface navigation | Partial | Predictable cross-grid edge behavior and resize commands. |
| OSC 133 command blocks | V1/P1 | Jump/search/copy/export and confirmation-gated rerun. |
| Multiline prompt composer and per-Session drafts | V1/P1 | Keep separate from raw PTY journal; support attachments and history. |
| Read-only files/diff Surface | V1 | Do not expand into a general IDE/editor. |
| Optional scrolling-column layout | Post-v1 | Secondary presentation only; waterfall remains primary. |

### 5.9 Browser, remote, mobile, and fleet

| Capability | Status | Remaining work |
|---|---|---|
| Owner SSH attachment | Implemented | Cross-platform E2E, reconnect, and diagnostics. |
| Scoped Observer/Controller Shares | Implemented | Expanded authorization, expiry, revocation, and accessibility proof. |
| Run-bound isolated browser Surface | Post-v1 | Separate profile/cookies, semantic refs, console/network/screenshot Artifacts. |
| Authenticated TLS/device gateway | Post-v1 | Pairing, scoped roles, expiry, revocation; never unauthenticated LAN exposure. |
| Attention-first mobile/web client | Post-v1 | Read-only offline cache and explicit queued-response outbox before editing. |
| Multi-machine Runner fleet | P2 | Separate control, execution, terminal data, and device-client planes. |
| Remote live workspace delegation | Experimental | Requires granular authority, audit, exact snapshots, and multi-writer conflict semantics. |
| Multi-user co-drive/rooms | P2 | Explicit ownership, privacy, and control model first. |

### 5.10 Security, supply chain, plugins, and evidence adapters

| Capability | Status | Remaining work |
|---|---|---|
| Workspace sandbox seam | Implemented foundation | Adversarial macOS/Linux proof and finer file/network/credential policy. |
| First-seen executable admission | V1 | Repository scripts, packages, workflows, MCP, hooks, skills, and config require digest-bound review. |
| Source trust and role preservation | V1 | Prevent copied tool/agent text from gaining instruction privilege. |
| Pre-effect cumulative policy | V1 | Sequence-aware gate across user, repository, tool, skill, and prior effects. |
| Nested per-tool/credential policy | Post-v1 | Consider L7 egress and credential mediation after local proof. |
| Out-of-process plugin runtime | Implemented foundation | Stabilize adapter/command/Signal/widget contract and conformance kit. |
| Capability-safe extension SDK | Post-v1 | Exact provenance, pinned resolution, explicit install, no owner authority inheritance. |
| Marketplace | P2 | Only after the extension contract and trust model stabilize. |
| WebEvidence adapter | P1/Post-v1 | HTTP first; provenance, byte/DOM span, freshness, cache, and degraded flags. |
| Signed Skill manifests/registry | P2 | Identity, digest, origin, capabilities, compatibility, evaluation, and revocation. |
| Recoverable information-flow control | Experimental | Reproduce locally before expanding the trusted computing base. |

## 6. Unified remaining plan

The order below merges the previous plan with both research roadmaps. Work does
not advance because a calendar week elapsed; each gate requires its evidence.

### Gate A — close the authorization-to-effect loop

**Objective:** an admitted writable Run can publish and settle only the exact
work it was authorized to perform.

1. Build a realized-change scanner at the managed-checkout seam.
2. Classify create, modify, delete, rename, generated, unchanged, and untracked.
3. Compare the realized set with committed and contingent ChangeIntent claims.
4. Reject undeclared expansion before Candidate publication and Settlement.
5. Add intent amendment/promotion with version and conflict re-admission.
6. Freeze an isolated-index Git snapshot and record its retained ref/tree,
   binary-safe patch Artifact and digest, changed-file manifest, base/head
   revision, lease epoch, and scanner version in one atomic Candidate batch.
7. Fence external-effect adapters with the same active epoch.
8. Add adversarial tests for stale workers, symlinks, path aliases, rename
   tricks, deleted/recreated files, nested repositories, and concurrent Runs.

**Exit:** two conflicting Runs cannot execute together; a stale lease cannot
publish; and no undeclared realized change can settle on macOS or Linux.

Current execution record (2026-09-02): items 1, exact-file portions of 2–6,
Candidate publication/Settlement verification, and contingent promotion are
implemented. Inspection runs on the blocking worker pool rather than async
control or terminal paths. It uses a private temporary Git index, hashes without
filters, freezes the admitted entries into a deterministic retained commit, and
streams the binary/full-index patch through SHA-256 without accumulating it in
memory. Candidate, patch Artifact, and review-contract Artifact commit
atomically, and idempotent replay does not repeat snapshot preparation. Real
Git fixtures cover tracked modify,
create, delete, rename, untracked expansion, exact-operation mismatch,
contingent-unpromoted scope, path aliases, Run-index isolation, retained-ref
failure, frozen post-publication state, symlinks without following their target,
nested repositories, and delete/recreate ambiguity. Remaining Gate A work is
generated-file provenance, external-effect fencing, stronger concurrent-write
adversarial coverage, and macOS plus Linux acceptance evidence.

### Gate B — complete native verification, return, retry, and landing

1. Add “Create verifier” from a finished Candidate.
2. Freeze Candidate and HarnessSnapshot as verifier inputs.
3. Normalize checks for tests, formatting, lint, delivery/install, provenance,
   and repeatability.
4. Show changed files, patch, semantic impact, evidence, Handoff, and receipt in
   one review surface.
5. Support inline comments and bundle them into one structured Return.
6. Create a retry Run linked to the returned Candidate and review note.
7. Support explicit compare/winner/synthesis for sibling Candidates.
8. Add previewed landing/merge with no automatic force operation.

**Exit:** conflict → execute → Candidate → verify → Return → retry → verify →
Accept → land passes from desktop, CLI, and agent channel.

Current execution record (2026-09-02): items 1, 2, and the Run-creation portion
of 6 are implemented in the domain, CLI, and native review surface. Candidate
publication now materializes a content-addressed JSON review contract containing
the frozen changed-file set and normalized `not_run` checks for format, lint,
tests, delivery, provenance, and repeatability. Native verifier/retry creation
runs checkout provisioning off the UI thread. The runtime injects the frozen
input identity into the agent environment and refuses to launch either workflow
outside its Candidate checkout. Remaining Gate B work is executing those checks
into evidence and a receipt, inline comment bundling, complete retry
resubmission, sibling comparison/synthesis, and previewed landing.

### Gate C — make supervision truthful at scale

1. Finish provider adapter kit and conformance fixtures.
2. Add stable status explanation and adapter capability matrix.
3. Add Run mailbox and typed references.
4. Add deterministic Supervisor/Problems projection.
5. Add authenticated heartbeats, lease reconciliation, and severity escalation.
6. Add bounded continuation policy and hierarchical budgets.
7. Add read-only Scout Run with verified Handoff before expensive routing.
8. Add idempotent Mission templates, schedules, and webhook triggers.

**Exit:** one owner can supervise twelve Sessions and multiple Missions without
screen scraping, focus theft, status flicker, silent respawn, or ambiguous
authority.

### Gate D — context, recovery, and supply-chain trust

1. Record ContextReceipts with original trust and model-facing role.
2. Generate evidence-linked compact summaries without deleting source evidence.
3. Record EffectRecords and idempotency boundaries.
4. Require a RecoveryManifest before semantic resume.
5. Add derived FailureDiagnosis and a disputable Problems view.
6. Add first-seen digest-bound Admission for executable configuration, skills,
   scripts, packages, workflows, MCP definitions, and hooks.
7. Add cumulative pre-effect policy across the action trajectory.
8. Publish a scrubbed owner-controlled event/provenance export.

**Exit:** restore never claims safe resume without reconciliation, and untrusted
context cannot silently acquire higher instruction privilege or execution
authority.

### Gate E — terminal fidelity, performance, and cross-platform proof

This gate runs alongside A–D because macOS and Linux are one product.

1. Complete Unicode, emoji, combining, wide-cell, styles, hyperlinks, reflow,
   alternate screen, mouse, focus, bracketed paste, Kitty keyboard, Neovim,
   shell editor, pager, and full-screen monitor journeys.
2. Prove identical semantic frame digests across macOS and Linux.
3. Run native keyboard, IME, clipboard, scrolling, drag, focus, and reconnect
   journeys under macOS, Wayland, and X11.
4. Certify p95 input-to-PTY <= 4 ms, PTY-to-delta <= 8 ms, local
   input-to-pixel <= 16.7 ms at 60 Hz, warm attach <= 150 ms, and attention
   visibility <= 100 ms.
5. Certify 50 MiB/s parse/journal throughput, twelve Sessions, six visible
   Surfaces at 60 FPS, bounded resync impact, idle CPU, startup, and RSS budgets.
6. Run crash injection, disk-full, slow-client, fuzz, 10,000 reconnect cycles,
   12-hour and 72-hour soak tests.
7. Complete VoiceOver and Orca journeys, keyboard reachability, visible focus,
   contrast, 200% scale, reduced motion, and non-color status.

**Exit:** every applicable section 08 performance, correctness, reliability,
security, terminal, and accessibility gate passes on the required matrix.

### Gate F — distribution and production v1

1. Freeze protocol/state compatibility and migration policy.
2. Produce network-free locked release builds with SBOM and license notices.
3. Build/test macOS Apple Silicon and Intel packages.
4. Build/test Linux AppImage and `.deb` under Wayland and X11.
5. Verify install, first launch, upgrade, downgrade refusal, rollback, and
   uninstall that preserves user state by default.
6. Complete macOS hardening, signing, and notarization when the deferred release
   credential work is authorized.
7. Publish the machine-readable requirement-to-evidence release report.

**Exit:** all v1 specification gates pass with no critical/high security defect,
data-loss defect, or platform-specific terminal blocker.

## 7. Research evaluation program

| Test family | Experiment | Measures |
|---|---|---|
| Harness conformance | Same frozen fixture through each Driver/evaluator. | Launch/resume/interrupt, Signal truth, Artifact schema, effect provenance. |
| Handoff debt | Repository-only versus structured Handoff at deterministic interruption points. | Success, rediscovery, delivered tokens, time, repeated tools. |
| Chain reliability | Deterministic 1/5/10/20-step workflows with injected faults. | Full-chain and five-of-five reliability, recovery, delivery accuracy. |
| Resume continuity | Checkpoint around external reads/writes/approvals/nondeterminism. | Mismatch detection, duplicate effects, stale permissions, safe refusal. |
| Escalation uptake | Defective tests, impossible constraints, contradictory instructions. | Unsafe shortcuts, escalation precision, resolution latency, success. |
| Context accounting | Policies over typed context objects. | Stored bytes, delivered tokens, management work, re-fetch, outcome. |
| Failure diagnosis | Known critical failures plus downstream symptoms. | Localization/category accuracy, evidence quality, false diagnosis. |
| Security composition | Vary repository, scripts, skills, tools, prompts, and delayed signals. | Pre-effect block, attack success, false positives, attention timing. |
| Parallelism | Cohesion-aware packages versus file split and serial baseline. | Critical path, conflicts, integration cost, pass rate, total cost. |
| Candidate comparison | Identical frozen inputs across agents/models. | Quality, cost, time, verifier agreement, synthesis gain. |

Fixture tests run without live credentials. Provider-backed acceptance is
separately gated and never replaces deterministic fixtures.

## 8. Explicit non-goals and rejected directions

- No freeform infinite canvas as the primary workspace.
- No Kanban board as stored workflow truth.
- No broad tmux compatibility requirement.
- No complete IDE/editor/file-manager replacement.
- No screen scraping as authoritative agent state.
- No automatic focus changes when an agent emits output or requests attention.
- No default auto-respawn of every failed process.
- No automatic merge, destructive cleanup, or force operation.
- No unauthenticated browser/mobile LAN listener.
- No arbitrary in-process plugin execution.
- No plugin marketplace before the capability/provenance contract stabilizes.
- No transparent checkpoint resume without external-effect reconciliation.
- No early intervention or Settlement based primarily on agent confidence.
- No context policy evaluated only by nominal token limit.
- No security warning presented as enforcement if it occurs after the effect.
- No automatic trust of repository instructions, tests, imported skills, tool
  descriptions, generated configuration, or MCP definitions.
- No hidden autonomous Mission system nested inside a plugin or web tool.
- No cross-owner delegation cryptography in local-first v1 merely for symmetry.
- No multi-user co-drive until ownership, privacy, and authority are explicit.

## 9. Post-v1 and experimental horizon

After Gates A–F, prioritize only with measured user need and local evidence:

1. Isolated browser Surface and evidence-native web adapter.
2. Attention-first mobile/web clients and authenticated device gateway.
3. Typed SDK/MCP surface over the stable protocol.
4. Capability-safe extension SDK and later curated marketplace.
5. Multi-machine Runner fleet with separate terminal data plane.
6. Effect-aware and diff-based sandbox checkpoints.
7. ExecutionBranch comparison over reversible scopes.
8. Recoverable information-flow control and schema-bounded declassification.
9. Portable scrubbed Actor/Driver/Skill bundles and signed registries.
10. Co-signed delegation ancestry for cross-owner fleets.
11. Learned context retention, routing, or harness intervention only after it
    beats deterministic policies without violating invariants.

## 10. Immediate work packet

The active implementation packet is **Gate A: realized-change enforcement**.

**Owning seam:** managed Run checkout and Candidate admission.  
**Input:** Mission, Run, admitted ChangeIntent, active lease epoch, exact base
revision, and checkout path.  
**Output:** immutable realized-change manifest and patch digest, or a typed
policy rejection.  
**Failure behavior:** fail closed; preserve checkout and evidence; raise
attention; do not publish or settle the Candidate.  
**Performance constraint:** scan off the terminal render/input path and avoid
holding the PTY or global desktop locks.  
**Platform constraint:** identical Git semantics and fixtures on macOS and
Linux.  
**Acceptance:** exact create/modify/delete/rename succeeds; undeclared,
contingent-unpromoted, stale-lease, symlink/path-alias, and concurrent-conflict
fixtures fail before publication.

After that packet, execute Gate B rather than broadening the UI or adding a new
client. This closes the trustworthy work loop that both research sources
identify as ultraplexr's strongest differentiator.

## 11. Source-to-roadmap trace

| Source theme | Unified destination |
|---|---|
| Explicit harness and evaluator | HarnessSnapshot, EvaluationReceipt, Gates A/B. |
| Escalation rather than reward hacking | Signal/Escalation, Gate C. |
| Handoff debt | HandoffArtifact, ContextReceipt, Gate D. |
| Safe resume | RecoveryManifest, EffectRecord, Gate D. |
| Typed working memory and context | ContextReceipt and evaluation program. |
| Correct labor versus correct delivery | Candidate/Receipt/Settlement, Gate B. |
| Failure localization | FailureDiagnosis and Problems projection. |
| Delegation/output attestation | Content digests now; cross-owner signing post-v1. |
| Repository poisoning and trajectory attacks | First-seen admission and cumulative pre-effect policy, Gate D. |
| Change admission and parallel coding | ChangeIntent, leases, realized-change verification, Gate A. |
| Verified state outside model context | Mission projection plus independent verifier, Gate B. |
| Cohesion-aware scheduling | Scout/partition/dependency waves, Gate C. |
| Attention broker | Deterministic Supervisor projection, Gate C. |
| Effect-aware checkpointing | Post-v1 checkpoint horizon. |
| Multiplexer competitor features | Sections 5.1–5.10 and Gates B/C/E. |
| Work/Attempt/Settlement | Canonical model and Gate B. |
| Goal continuation and budgets | Gate C. |
| Fan-out and comparison | Gate B after exact-effect enforcement. |
| Provenance and semantic diffs | Gate B and section 5.5. |
| Evidence-preserving compaction | Gate D. |
| Adapter conformance | Gate C. |
| Remote fleets and multi-device clients | Post-v1 horizon. |
| Evidence-native web research | Optional post-v1 adapter. |
