# Product completion matrix

Assessment date: 2026-09-06. This is a source and evidence review, not an executed
test report or a release declaration. It covers the native macOS/Linux contract
and the broader universal product requested by the owner. No tests, builds,
application runs, installations, or external writes were performed for this
assessment.

The next critical step is to validate the current integrated source. A substantial
set of client, protocol, automation, and packaging changes was written after the
last recorded workspace run. The owner's renewed request for testing and fixing
supersedes the historical testing pauses recorded in those implementation notes;
the notes remain accurate descriptions of what had been executed at that time.

## Contract and evidence rules

The controlling release requirements are [quality and release](../spec/08-quality-and-release.md),
[delivery plan](../spec/09-delivery-plan.md), and the referenced numbered
specifications. The [north star](../product/north-star.md) defines the complete
native journey. The [universal architecture](../architecture/universal-runtime-and-clients.md)
adds browser, TUI, remote, embedding, platform, and modular distribution direction.
It explicitly describes a proposal, with no requirement IDs for several new
capabilities. Those rows name their exact proposal sections and existing related
requirements; they do not invent normative IDs or silently rewrite the v1 scope.

The [unified roadmap](../ultraplexr-unified-roadmap.md) remains the capability
ledger. Its aggregate completion percentages and older capability descriptions
are not acceptance evidence. More recent implementation notes qualify older
statements that browser styling, embedding, or verification commands are absent.

- **Recorded local evidence:** a linked report describes an executed build and
  workload. It supports that build, host, and workload only.
- **Source present, validation open:** implementation exists, but the reviewed
  notes explicitly report no build/test execution for those changes.
- **Incomplete:** implementation or the complete required journey still needs
  engineering. A passing predecessor test cannot close this state.
- **External/platform gate:** a named environment, credential, or deployment
  decision is needed for a specific remaining acceptance step. This does not
  block independent implementation or local validation.

For every newly executed result, retain requirement ID, test identifier, source
revision and dirty-source identity, platform/backend, command, result, and
artifact digest as required by `Q-TRACE-001`. A commit alone does not identify
this dirty worktree. Test counts, configured CI jobs, and successful compilation
alone do not prove complete behavior or release readiness.

## Completion matrix

| Area and exact requirements | Implementation and evidence state | Next engineering work and acceptance boundary |
| --- | --- | --- |
| Native product journey: `P-001`, `P-002`, `P-003`, `P-004`, `P-005`, `P-006`, `P-JOURNEY-001`, `DEL-M2`, `DEL-M3`, `DEL-M4` | Durable runtime, graph, native terminal, multiple Surfaces, intervention, and reconnect have [historical macOS evidence](m2-runtime-release-evidence.md). This is an implemented foundation; the full journey on macOS, Wayland, and X11 remains open. | Rebuild the current source and run create/delegate/observe/attention/intervene/detach/recover/review through native UI, CLI, and agent channel. Preserve process identities and exact frame evidence. Include twelve Sessions, six visible Surfaces, one controller plus three observers, and layout transitions. |
| Shared metadata and lifetime: `T-OWN-001`, `T-ATTACH-001`, `W-STREAM-001`, `W-PRESSURE-001`, `UX-PERSIST-001`, `Q-RELIABILITY-001` | Earlier bounded queues and reconnect fixes were locally tested. [Collection snapshots](collection-snapshot-implementation.md), membership replacement, stopped-feed diagnostics/retry, and downstream state retirement are explicitly unverified. | Test all actual consumers against empty/partial snapshots, concurrent mutation, deletion during disconnect, lag recovery, stale queued replacements, revoked scope, old peers, and close-during-connect. Verify missing membership clears retained input/search/copy state and never terminates the underlying Session. |
| Terminal fidelity across clients: `T-FRAME-001`, `T-INPUT-001`, `T-RESIZE-001`, `T-HISTORY-001`, `T-SAFETY-001`, `T-COMPAT-001`, `Q-TERMINAL-001` | Native and TUI terminal foundations have local fixtures. Browser [styled cells](browser-styled-terminal-implementation.md) and [row deltas](browser-row-deltas-implementation.md) are source present, validation open. They supersede the older text-only implementation description without proving parity. | Run shared canonical frame fixtures through native/TUI/browser rendering and recovery. Exercise Unicode/wide/combining cells, alternate screens, delta gaps, style bounds, history, copy, paste, IME/keys, links, and controller-only resize. Finish stable history coordinates/truncation semantics and missing input capabilities against the spec. Record browser visual and accessibility evidence separately from Node tests. |
| TUI operator workflow: `P-002`, `P-003`, `P-005`, `D-CONTROL-001`, `W-STREAM-001`; universal proposal “Do we need a Herdr-style TUI?” and stages 4/7 | Focused-session observation/control, two panes, and navigation have historical real-PTY evidence. [Group actions](tui-group-actions-implementation.md), [verification inspection](tui-verification-inspection-implementation.md), worker and ordered-input extensions include later unbuilt source. | Validate keyboard navigation, escape/prefix handling, group edits across duplicate clients, pinned Control, ordered input, failure cleanup, and restoration of the outer terminal. Complete structured Attention responses and remaining blocking commands. Same Session identity, explicit handoff, and no input replay must hold through reconnect. |
| Verified delivery and automation: `D-DELIVERY-001`, `D-EFFECT-001`, `D-GRAPH-004`, `D-GRAPH-005`, `A-AGENT-001`, `DEL-M4.5` | [Return/retry/verify/accept fixture](delivery-journey-evidence.md) passed on macOS with development binaries. Bounded verification exists. [Reviewed headless launch](headless-reviewed-verification-implementation.md) and compact status are source present, validation open. The fixture is not a native UI, Linux, production-project, or landing proof. | Test current prepare/launch/status/collect paths through the production CLI and native review interface, including changed plan digests, uncertain creation ACKs, reserved-ID collisions, resumed pending verifiers, wrong Candidate receipts, and restart. Complete generated-file provenance, external-effect lease fencing, concurrent-write cases, native review details, and previewed owner landing. Acceptance remains a separate owner action. |
| Broader bounded automation: `D-GRAPH-004`, `D-SIGNAL-001`, `R-LIMIT-001`, `R-OBSERVE-001`; roadmap sections 5.4, 5.7 and gates C/D | Deterministic and continuous scheduling, configured drivers, Signals, and evidence are foundations. Durable trigger/template reconciliation, mailbox, typed references, hierarchical budgets, conservative orphan reconciliation, recovery manifests, and context/effect records remain listed work. These additions do not all have dedicated specification IDs. | Specify each new command/event and its failure/authorization contract before shared type changes (`DEL-PACKET-001`). Implement one complete bounded Mission automation path with idempotent triggers, restart recovery, explicit stop conditions, evidence-linked supervision, and no silent process respawn. Test local deterministic drivers before production provider integration. |
| MCP and SDK boundary: `W-AGENT-001`, `C-COMPAT-001`, `S-AUTHZ-001`, `R-LIMIT-001`; universal proposal “SDKs and automation” | MCP executable exists; [input](mcp-input-boundary-implementation.md) and [output](mcp-output-boundary-implementation.md) bounds and additional read surfaces are source present, validation open. Versioned generated SDKs and a supported native FFI remain proposed. | Test MCP over its real process/stdio interface: malformed/oversized messages, EOF, blocked output, Share allowlists, pagination, capability negotiation, and no owner fallback. Define versioned SDK schemas and conformance fixtures using the same runtime lifecycle and permissions. Merely packaging MCP does not establish an extension SDK. |
| Owner remote and scoped collaboration: `S-AUTH-004`, `S-AUTH-005`, `S-AUTH-006`, `D-CONTROL-001`, `Q-SECURITY-001` | OpenSSH forwarding and native Observer/Controller Shares have [local evidence](m2-runtime-release-evidence.md). [Browser control](browser-control-evidence.md) has earlier local evidence; later pinned-control/feed changes need fresh tests. Public identity/device gateway and broader collaboration roles are incomplete. | Revalidate expiry, revocation, in-flight waits/search, every method allowlist, stream teardown, and stale lease/input rejection across native/TUI/browser. Implement authenticated connection identity and runtime resource/action scope before public exposure. Then prove two-host SSH/gateway reconnect and least-authority behavior. A shared terminal lease does not implement multi-writer editing or fleet orchestration. |
| Browser embedding and web workflow: `S-AUTHZ-001`, `S-TERMINAL-001`, `Q-A11Y-001`; universal proposal “Runtime and client boundaries” and stage 7 | [Framing policy](browser-embedding-implementation.md), [side views](browser-side-views-implementation.md), and [workflow inspection](browser-workflow-inspection-implementation.md) are unbuilt source. The gateway still binds loopback; this is not an authenticated public TLS service or an embedding SDK. | Test allowed/denied/nested parent origins, actual iframe loading and authenticated streams, credential non-disclosure, write rejection, Controller opt-ins, revocation, CSP, zoom, keyboard and screen reader behavior. Design the supported component/SDK API and parent messaging only when its authority contract is explicit. Browser local-network restrictions are an acceptance constraint, not justification to expose the gateway publicly. |
| Platforms and constrained hosts: `A-PLATFORM-001`, `A-MODULE-001`, `A-DEPENDENCY-001`, `Q-CI-001`, `DEL-M1`; universal proposal stage 5 | macOS has local results. Linux execution remains unproven in the reviewed ledger despite a configured Docker path. Windows named pipes/authentication/process support, mobile-specific interfaces, and embedded target certification remain proposed. | Finish required macOS Apple Silicon/Intel and Linux x86-64 Wayland/X11 coverage first, while separating Unix-only execution/credentials/transport behind platform adapters. Build a Windows lifecycle/control/transport slice with equivalent authorization; assess each constrained target as host, client, or SDK rather than claiming arbitrary process execution everywhere. Hardware availability is separate from implementation completeness. |
| Optional connectivity: related `S-AUTH-004`, `S-AUTHZ-001`, `R-LIMIT-001`; universal proposal “Secure remote access and Tailcat” and stage 6 | Tailcat is an experimental proposed companion. No accepted production adapter or direct/relay acceptance is recorded. | Evaluate an optional supervised byte-stream adapter with pinned dependency/license review, runtime-bound identity, failure containment and measured companion cost. Two-machine direct/relay tests must include scope, revocation, reconnect and baseline comparison. Selecting or deploying a relay is separate from writing/testing the adapter locally. |
| Modular distribution: `A-MODULE-001`, `A-DEPENDENCY-001`, `A-PLUGIN-001`, `S-PLUGIN-001`, `Q-PACKAGE-001`; universal proposal “Footprint and optional modules” | [Six executable profiles](../design/distribution-profiles.md) and `ci/build-profile.py` exist as unexecuted source. They select packages, not all optional internal dependencies. CLI retains verification/plugin dependencies; shared terminal types retain native VT coupling. | Execute profile builds and inspect emitted artifacts/metadata. Test terminal-only attachment without bundled server, headless host without GPUI, disabled integrations with no worker/process/listener, output refusal, partial failures, and SBOM selection. Define and test a small supported feature matrix that removes optional internal dependencies without disabling mandatory authority checks. |
| Resource and latency budgets: `Q-PERF-001`, `Q-PERF-002`, `Q-PERF-003` | Latest recorded five-minute [pull-metadata release sample](shared-client-pull-metadata-release.json): runtime CPU 0.320%, desktop CPU 0.367%, runtime+desktop peak sampled RSS 120.33 MiB, zero idle SSE/TUI output. [Ledger](shared-client-resource-evidence.md) identifies exact binary hashes. This predates later unbuilt changes and excludes rendered browser/outer terminal/compositor cost. | Rebuild all measured executables and resample after integration stabilizes. Add active search/output/viewer fan-out, 50 MiB/s parse+journal, control latency, six-Surface 60 FPS, warm attach/startup and true input-to-pixel instrumentation. Measure each profile's installed/compressed size and complete process costs on frozen reference hosts. Current thresholds are not proof of minimal-device suitability. |
| Security, recovery, and production release: `Q-TEST-001`, `Q-SECURITY-001`, `Q-RELIABILITY-001`, `Q-A11Y-001`, `Q-TRACE-001`, `Q-STAGE-001`, `Q-PACKAGE-001`, `Q-DONE-001`, `DEL-M5` | Package smoke/SBOM, local sandbox checks, short soaks and recovery tests have historical evidence. No complete current release report establishes all adversarial, fuzz, 10,000 reconnect, 12-hour/72-hour soak, migration/update, physical accessibility, signing and distribution gates. | Complete the missing harnesses and run current fixtures, crash/disk-full/migration and adversarial process tests. Perform physical VoiceOver/Orca/IME/presentation gates; produce current locked offline packages and rehearse installation/update/rollback refusal/state-preserving uninstall. Finish signing/notarization with release credentials and publish the requirement-linked report only after every applicable gate passes. |

## Current CI coverage and immediate order

The reviewed [workflow](../../.github/workflows/ci.yml) configures `macos-14`
formatting, Node tests, license audit, workspace Rust tests/Clippy, offline release
build and package smoke. The [Linux Dockerfile](../../ci/linux.Dockerfile)
configures corresponding Rust/audit checks, metadata, a network-disabled release
and packaging step, a five-second twelve-Session soak, and native/AppImage window
smokes. Configuration is not an execution result. The initial review found the
Linux browser/harness Node suite missing. The recipe now copies Node 24.12.0 from
a version-pinned Bookworm image and runs the same JavaScript test command as
macOS. This source fix awaits Docker execution: the CLI is installed locally,
but its `/var/run/docker.sock` daemon endpoint is unavailable. The current local
Node is v26.1.0; it does not validate the image's pinned Node runtime. No scheduled
fuzz/12-hour/72-hour jobs or complete release-platform matrix appear in this
workflow. `cargo --offline` on macOS prevents Cargo downloads; unlike the Linux
`RUN --network=none` step, it does not itself isolate build-script networking.

1. Record the exact current source, run formatting and the current Rust,
   browser/harness and lint checks, fix failures, and rebuild every product
   executable. Keep old binaries and historical counts out of the new evidence.
   The last reviewed full regression note is [475 Rust tests plus 29 browser/harness tests](oversized-record-recovery.md),
   preceding collection snapshots and the source-only extensions above.
2. Prove one same-Session journey across desktop, TUI and browser, including
   shared collection deletion/recovery, Controller handoff and live revocation.
   In parallel with that local acceptance work, obtain an actual current Linux
   run and fix discovered build/runtime differences.
3. Validate reviewed verification and MCP process boundaries, then build the
   production remote identity/gateway and bounded automation gaps as explicit
   requirement-backed slices. Existing localhost/SSH work remains the baseline.
4. Execute every distribution profile and remove unwanted baseline dependencies;
   measure current release artifacts only after functional integration passes.
5. Close terminal/accessibility/security/reliability/migration gates and produce
   the release report. Complete Windows/embedding/SDK/optional transport slices
   with their own platform and security acceptance; native v1 evidence alone
   cannot close the broader universal product.

## Genuine external gates versus work that can proceed

| Remaining need | Requires an external resource or decision | Engineering that remains independently actionable |
| --- | --- | --- |
| Physical platform acceptance | Supported macOS Intel, Linux Wayland/X11 and Windows test environments; physical or accelerated presentation/input access; VoiceOver/Orca interaction. Availability was not checked by this document task. | Platform adapters, fixtures, CI configuration, Linux container checks, accessibility semantics and instrumentation. A missing physical host does not explain an uncompiled client. |
| Production package identity | Developer ID/notarization credentials, intended release/update identity and authorized publication destination. | Build unsigned development packages, complete hardening configuration, validate contents, migrations, atomic update behavior, SBOM/licenses and state-preserving uninstall locally. |
| Public remote deployment | Selected hosts/domain/TLS provisioning, identity enrollment policy, reachable test peers, and authorization to deploy. | Implement connection-scoped authentication/authorization, expiry/revocation and local two-principal adversarial fixtures. The identity/gateway implementation itself is still missing work. |
| Production providers and SCM/CI adapters | Credentials and explicit permission for account-specific operations or paid calls. | Typed adapter interfaces, deterministic driver fixtures, local fake endpoints, failure handling, budgets and evidence provenance. Full local verification does not require paid agents. |
| Experimental direct/relay connectivity | Two appropriately reachable test hosts and an explicitly selected relay/service when needed. | Optional adapter design, dependency review, process supervision, framing and authorization tests. Tailcat is not a prerequisite for owner SSH or a finished native release. |

The suite is complete only when these capability journeys and applicable gates
have current implementation and retained evidence. Historical testing pauses,
old passing baselines, implementation descriptions and deployment credentials
must not be used to hide work that can still be built, fixed and tested locally.
