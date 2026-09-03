# 09 — Delivery plan

The plan delivers thin vertical slices on macOS and Linux together. A milestone
does not finish with isolated library code; it finishes when its user-visible
journey and failure cases pass on both platforms.

## M0 — Domain/control foundation (completed baseline) — DEL-M0

Baseline repository capability was:

- Rust workspace, event-sourced Mission prototype, Runs, Sessions, Signals, and
  attention projection;
- append-only JSONL store;
- Unix-socket JSON protocol v2;
- CLI for Mission, Run, Session, Signal, and control operations;
- ten passing tests.

The implementation has since completed dependency edges; independent Run phase,
outcome, disposition, and attention; event envelopes; expected versions;
idempotency; durable PTYs; and native desktop projection. The current preview
wire version is 21 and the macOS gate includes structured terminal automation,
durable exact-base Run checkouts, truthful expiring provider facts, NDJSON push
events, durable SCM/CI Run evidence, version-safe self-hosted desktop startup,
and release-v3 single-connection subscription multiplexing. Complete Linux
execution evidence remains migration work; the preview is evidence, not the
final storage/protocol contract.

## M1 — Upstream feasibility lock (week 1) — DEL-M1

Deliver:

- audit and pin/fork `libghostty-rs` and its exact Ghostty/Zig sources;
- build one Ghostty terminal fixture on macOS and Linux CI;
- GPUI windows on macOS, Wayland, and X11;
- custom element paints a fixed terminal grid;
- keyboard richness, IME, clipboard, focus, accessibility, scale, and packaged
  smoke checks;
- benchmark harness and reference hardware manifest.

Exit gate:

- public Ghostty VT state can be translated without unsafe leakage;
- GPUI delivers required key events and one custom-painted grid on all backends;
- release builds can be produced without network access;
- accepted pins and licenses are recorded.

If GPUI fails keyboard, accessibility, or Linux presentation criteria, stop UI
feature work and replace ADR-0006. If `libghostty-rs` fails the safety/build audit,
implement the minimal generated raw bindings plus a superplexr-owned safe wrapper;
do not use Ghostty's private application surface.

## M2 — One durable terminal vertical slice (weeks 2–3) — DEL-M2

Deliver:

- `superplexr-terminal`, real POSIX PTY Session actor, raw journal, checkpoint;
- v3 handshake/framing and one Session subscription;
- FullFrame, FrameDelta, input, resize, acknowledgement, and resync;
- one GPUI SessionProjection and TerminalElement;
- create `/bin/sh`, render prompt, type, resize, detach desktop, reattach exact
  frame, exit, and review final frame;
- crash, slow-client, sequence-gap, and storage-tail tests.

Exit gate: that exact journey passes on macOS, Wayland, and X11 with no lost byte,
and latency budgets are instrumented even if not fully optimized.

## M3 — Multiplexer experience (weeks 4–5) — DEL-M3

Deliver:

- multiple concurrent Session actors and subscriptions;
- Mission tabs, per-tab sidebar, one-to-four-column waterfall;
- shared SessionProjection, controller and observer Surfaces;
- stable-grid resizing, scrollback, selection, search, clipboard, links;
- view-state persistence, disconnected/resync/lost states;
- keyboard navigation and first VoiceOver/Orca pass.

Exit gate: twelve Sessions, six visible Surfaces, controller plus three observers,
tab close/reopen, and responsive three-to-one-column resize pass on all backends.

## M4 — Agentic graph and attention (weeks 6–7) — DEL-M4

Deliver:

- v3 event envelope and migrations from prototype v2;
- Run phase/outcome/disposition split;
- lineage forest plus dependency DAG, readiness, deterministic scheduler, retry
  provenance, concurrency limits;
- restricted agent capability channel;
- structured Signals, attention sheets, Artifacts, cooperative Grants;
- graph inspector and complete human takeover/return audit.

Exit gate: the section 01 three-Run fan-out/fan-in journey passes from desktop,
CLI, and agent channel, including blocked dependency and rejected result cases.

## M5 — Fidelity, hardening, and packaging (weeks 8–10) — DEL-M5

Deliver:

- complete terminal compatibility corpus and renderer fidelity;
- font fallback, emoji, IME, mouse, alternate screen, Kitty keyboard;
- persistence recovery, retention, disk-full, diagnostics, redaction;
- all accessibility, performance, fuzz, soak, and security gates;
- signed/notarized macOS package, AppImage, `.deb`, SBOM, migration and updater;
- dogfood evidence and release report.

Exit gate: every requirement in section 08 passes. Conditional Kitty graphics is
either tested and enabled or explicitly deferred with no false claim.

## M4.5 — Verified work delivery — DEL-M4.5

This milestone turns successful agent execution into reviewable, settleable
work rather than treating a zero exit code as completion.

Implemented foundation:

- versioned exact-file ChangeIntents with committed and contingent claims;
- runtime-authored Execution lease epochs, stale-version rejection, release on
  completion, and conflict admission across Missions;
- immutable Harness Snapshots bound to the exact resolved driver and objective;
- immutable Candidates, bounded Handoff Artifacts, verification policy, and
  independent Evaluation Receipts bound to one candidate digest;
- settlement rules that preserve Outcome separately from owner Disposition;
- restricted agent-channel authority for self-submission and verifier receipts;
- CLI commands for every record plus typed Escalation Signals;
- native graph review showing intent, harness, candidate, handoff, receipt, and
  an operator-authored return note.

Remaining exit work:

- enforce realized filesystem changes against the admitted claim set on macOS
  and Linux, including create/delete semantics and stale-lease fencing;
- materialize Git diff and semantic blast-radius Artifacts for the review view;
- create verifier Runs and normalized checks from the desktop instead of only
  consuming records created through the CLI/agent channel;
- add a cross-platform end-to-end fixture covering conflict, agent execution,
  candidate submission, independent verification, return, retry, and acceptance;
- prove the review path and admission checks stay within section 08 latency and
  memory budgets.

Exit gate: two conflicting writable Runs cannot execute concurrently; an
undeclared realized change cannot settle; a stale lease cannot publish a
candidate; independent policy cannot be accepted without a passing receipt for
the exact candidate; and the complete return/retry/accept journey passes on
macOS, Wayland, and X11.

## 6. Work ordering — DEL-ORDER-001

Critical path:

```text
upstream pins
  -> TerminalModel
  -> PTY SessionActor
  -> v3 terminal stream
  -> SessionProjection/TerminalElement
  -> multi-Surface waterfall
  -> graph/agent integration
  -> hardening/package
```

Work that may proceed beside the critical path after interfaces freeze:

- event envelope/store migration;
- Mission graph property tests;
- UI chrome and view-state model using fixture projections;
- terminal fixture corpus and protocol fuzzing;
- package scripts and accessibility harness.

No one should build final agent orchestration on fake terminal ownership, and no
one should optimize renderer internals before the semantic frame corpus passes.

## 7. Engineering work packets — DEL-PACKET-001

Each packet handed to a human or coding agent contains:

- requirement IDs and owning module interface;
- input/output/failure contract;
- fixtures and acceptance command;
- platform matrix;
- prohibited dependency directions;
- performance/size limits;
- migration impact;
- completion evidence path.

A packet should be independently mergeable and keep the workspace green. Shared
type invention requires a specification change first.

## 8. Risk register — DEL-RISK-001

| Risk | Trigger | Mitigation/fallback |
|---|---|---|
| GPUI pre-1.0 churn | pin cannot pass all three backends | isolate desktop; repin deliberately; replace ADR if spike gate fails |
| libghostty C interface churn | pinned wrapper unsound or missing state | fork audited wrapper or own minimal binding; keep terminal seam stable |
| custom text rendering drift | wide/IME/ligature corpus fails | disable optional ligatures; prioritize cell correctness; use GPUI text primitives where adequate |
| daemon output pressure | journal queue reaches emergency cap | pause process before loss; surface storage attention; optimize/chunk writes |
| responsive PTY thrash | more than one resize per settled action | stable-grid debounce and authoritative-controller tests |
| protocol overdesign | vertical slice cannot attach in week 3 | keep owner Remote attachment as transport reuse; keep Observer and Controller allowlists separate and require a new decision for every broader role |
| graph ambiguity | scheduler and UI derive different readiness | one core projection and property tests; no UI-local scheduling rules |
| false security claim | unrestricted process bypasses Grant | cooperative label; no enforcement language; adversarial tests for future adapters |
| Linux packaging variance | Wayland/X11 or distro dependency failure | AppImage baseline plus `.deb`; physical-backend release hosts |
| schedule pressure | one platform lags | milestone remains open; never mark Linux as post-v1 |

## 9. Schedule assumptions — DEL-SCHEDULE-001

The 8–10 week target assumes one experienced full-time Rust engineer with focused
design support and access to both platform test machines. Two engineers can
parallelize UI and runtime after M1, but upstream uncertainty and integration keep
the likely calendar above six weeks. Estimates excluded enforced cross-platform
OS sandboxing, owner Remote attachment, multiplayer, and web; the first two have
since been promoted into repository scope without promoting multi-user sharing.

The first runnable GPUI shell is expected in days; the first meaningful app is M2,
not a mock window. Public dates should be based on milestone evidence rather than
the optimistic path.
