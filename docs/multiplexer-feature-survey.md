# Multiplexer and agent-workspace feature survey

**Status:** research note

**Reviewed:** 2026-09-01

**Purpose:** identify features worth adapting for termi9ne without turning it
into a tmux clone, generic IDE, or unstructured agent dashboard.

The consolidated implementation status and priority plan derived from this
survey now lives in
[termi9ne-unified-roadmap.md](termi9ne-unified-roadmap.md).

## 1. Scope and method

This survey covers:

- five pages, 50 results, from GitHub's default **Best match** ordering for
  [`multiplexer`](https://github.com/search?q=multiplexer&type=repositories);
- three pages, 30 results, from GitHub's default **Best match** ordering for
  [`agent multiplexer`](https://github.com/search?q=agent+multiplexer&type=repositories);
- an earlier stars-sorted comparison pass that exposed several additional
  terminal and agent-workspace projects; these are recorded separately from the
  URL's canonical Best match pages;
- the separately requested repositories
  [Herdr](https://github.com/herdrdev/herdr),
  [T3 Code](https://github.com/pingdotgg/t3code),
  [bb](https://github.com/get-bb/bb), and
  [Agent Orchestrator](https://github.com/Untrivial-ai/agent-orchestrator);
- deeper README and documentation inspection for the projects most relevant to
  terminal persistence, agent supervision, orchestration, remote access,
  security, and human review.

GitHub search order, descriptions, stars, and repository status are volatile.
The appendix records what appeared in the reviewed result pages; it should not be
treated as a permanent ranking. A search hit is not automatically relevant:
many results use “multiplexer” for network protocols, HTTP routing, media,
hardware, or academic graph models.

This is feature research, not permission to copy code. Every implementation must
receive a separate dependency and license review. In particular, several
projects in this survey use GPL, AGPL, or source-available licenses that are not
automatically compatible with termi9ne's MIT distribution.

## 2. termi9ne baseline

The useful comparison is against capabilities termi9ne already owns, not against
a blank terminal application. The current repository already has or specifies:

- durable daemon-owned PTYs with detach, reattach, retained history, archive,
  restore, and frame resynchronization;
- a backend-neutral Ghostty-derived terminal model and GPUI desktop renderer;
- event-sourced Missions, Runs, Sessions, Signals, Artifacts, dependencies, and
  deterministic scheduling;
- a restricted agent side channel and configured engine drivers;
- a structured attention queue, takeover/return semantics, and an audit trail;
- browser-like Mission tabs, Session sidebar, terminal waterfall, graph
  inspector, and command deck;
- owner SSH attachment plus scoped Observer and non-forced Controller Shares;
- exact-base managed Run checkouts, canonical terminal capture/waits, NDJSON
  push events, and expiring provider facts with explainable precedence;
- fail-closed macOS/Linux workspace sandboxing;
- explicit separation between durable domain state and disposable presentation
  state.

Competitor features that merely recreate these primitives are not roadmap gaps.
The highest-value ideas are the ones that complete the loop around them:
isolated source work, accurate adapter signals, review, CI, automation, and
secure remote supervision.

## 3. Detailed project notes

### 3.1 Agent-native workspaces and orchestrators

| Project | Notable features | Useful direction for termi9ne |
|---|---|---|
| [Herdr](https://github.com/herdrdev/herdr) | Background server; durable sessions; working/blocked/idle pane status; CLI and socket API; agent-spawned panes; remote attachment; plugins; terminal-native UI. | Strong validation of zero-hunt attention and agent-addressable Surfaces. Add provider adapters and explainable status without replacing termi9ne's richer Mission graph. |
| [cmux](https://github.com/manaflow-ai/cmux) | Native macOS/libghostty terminal; notification rings and inbox; terminal/browser panes; agent-drivable browser; CLI/socket API; project commands; agent hooks and resume; subagents as visible panes; mobile companion. | Strong reference for an isolated browser Surface, notification locality, provider resume adapters, and composable workspace primitives. Its macOS-only embedding approach cannot be adopted as termi9ne's cross-platform architecture. |
| [T3 Code](https://github.com/pingdotgg/t3code) | Web, desktop, iOS, and Android control surfaces; multiple provider CLIs; rich composer and prompt stash; voice input; GitHub/GitLab/Bitbucket/Azure DevOps reviews; QR pairing; SSH-launched remote environments; resource telemetry. | Reference for a future authenticated multi-device gateway, SCM provider abstraction, prompt drafts, and resource-aware scheduling. |
| [bb](https://github.com/get-bb/bb) | Desktop/web/CLI/HTTP parity; durable threads; managed worktrees; setup/teardown scripts; multi-machine host daemons; public provider plugin API; plugin marketplace; self-customizing workflows. | Reference for worktree lifecycle contracts, capability-neutral provider plugins, and making every core operation scriptable. Avoid importing its full IDE/plugin scope early. |
| [Agent Orchestrator](https://github.com/Untrivial-ai/agent-orchestrator) | One branch/worktree per worker; persistent project orchestrator; live Kanban derived from session/PR/CI/review facts; 25+ agent adapters; terminal/chat handoff; isolated browser profiles; SCM observer; review feedback routing. | Best reference for attaching PR and CI facts to Runs. Preserve its key rule: durable facts are stored, display status is derived. Use the Mission graph as source of truth rather than adopting Kanban as the domain model. |
| [dmux](https://github.com/standardagents/dmux) | Prompt-to-worktree launch; multi-agent fan-out; automatic branch naming; durable agent resume; merge/PR actions; file/diff browser; lifecycle hooks; multi-project sessions. | Direct model for optional worktree-backed Runs, safe landing, and setup/merge hooks. |
| [Rove](https://github.com/Sma1lboy/rove) | Terminal-native worktree tasks; persistent sessions over SSH; Agent API; parent/child dispatch context; diff review with inline notes; cross-engine handoff; routines; Inbox; GitHub issue intake. | Useful for worktree-safe Run lifecycle, review-to-agent feedback, scheduled Missions, and explicit parent/child result routing. |
| [LeapMux](https://github.com/leapmux/leapmux) | Browser/desktop client; branch/worktree isolation; remote Worker behind NAT; Hub relay; end-to-end encrypted Frontend-to-Worker traffic; protobuf contracts; pluggable databases. | Reference for a later TLS/browser gateway and remote execution worker, especially relay blindness and versioned cross-language contracts. |
| [AgEnD Terminal](https://github.com/suzuke/agend-terminal) | `fleet.yaml`; long-lived PTY agent fleet; daemon-managed worktrees; 32 coordination tools; agent-to-agent delegation; auto-respawn; context handover; Telegram/Discord supervision. | Inspires Mission-as-code, explicit fleet reconciliation, health monitoring, and agent mail. Avoid automatic respawn unless Run policy explicitly allows it. |
| [Chartr](https://github.com/rengwu/chartr) | Interactive plan map; tickets spawn Sessions with scoped context; skills registry; self-titling tabs; transcript-based completion; notifications. | Map tickets onto the existing Mission DAG and add reusable context/prompt presets. Do not introduce a second graph source of truth. |
| [Luvus](https://github.com/RizRiyz/luvus) | Agent status, token/cost/context metrics; forking/resume; Git/GitHub views; worktree orchestration; dependent tasks; path reservations; quality gates; remote multi-client view; Universal Harness Protocol; modules. | Useful for adapter-reported usage, file reservations, quality gates, and a stable harness API. |
| [nodeterm](https://github.com/eneskirca/nodeterm) | Infinite spatial canvas plus Kanban; tmux persistence; sticky-note context links; diffs/editors/browser nodes; mobile E2E attachment; voice; GitHub issue sync; power/sleep management. | Borrow linked context, mobile attention, and sleep prevention. Do not replace the waterfall with an infinite canvas. |
| [RayLine](https://github.com/EnSue-Laboratories/RAYLINE) | One-click fan-out into worktrees; streaming tool calls; git-backed checkpoints; terminal plus chat; GitHub project tooling. | Useful inspiration for checkpoint Artifacts and explicit fan-out UX. |
| [c11](https://github.com/Stage-11-Agentics/c11) | Agent-scriptable terminal/browser/Markdown Surfaces; readable ASCII layout; open Surface metadata; agent-created splits; roles/models/progress in sidebar. | Add a narrow Surface manifest and layout inspection API. Keep layout changes bounded by presentation capabilities and user policy. |
| [Termio](https://github.com/termio-sh/termio) | Rust PTY daemon; native macOS client; hook-based status; Git/worktree views; `--wait` CLI; sibling agent spawning; agent skill; iPhone terminal; Unix/SSH/WSS transports. | Strong model for semantic waits, client-independent frames, and a later mobile client speaking the same protocol. |
| [tlbx](https://github.com/tlbx-ai/tlbx) | Self-hosted browser control station; terminal and structured Agent Controller sessions; multiline composer; per-session drafts; attachments; exact input history; scheduled follow-ups; browser verification tools. | Reference for an attention-first browser client and structured prompt/input layer that remains distinct from raw PTY input. |
| [Tortie](https://github.com/gregce/tortie) | Durable per-project Sessions; explicit conversation resume; jump-to-waiting shortcut; conversation log; Git/diff/file/search UI; restricted remote editing. | Borrow “jump to oldest attention,” cautious resume, and read-only review surfaces. |
| [wmux](https://github.com/amirlehmam/wmux) | Windows/ConPTY workspace; global agent roster; reported-then-detected status precedence; offline detection tests; explanation of matched rules; jailed file tree; optimistic save conflicts; JSON-RPC API. | Best reference for honest fallback detection: reported status wins, no match means `Unknown`, and every heuristic is explainable. The safe file boundary is also worth copying conceptually. |
| [Séance](https://github.com/no1msd/seance) | Linux-native scrolling columns; agent hooks; notification/unread state; CLI/Unix-socket control; JSON output; bundled agent skill. | Consider scrolling-column presentation as an optional mode and package the control API as a discoverable skill. |
| [flowmux](https://github.com/flowmux-ai/flowmux) | Linux GTK terminal/browser workspace; agent-controlled WebKit; worktree/file/editor views; hooks; usage popover; `doctor`/`fix`; crash diagnostics. | Add read-only `doctor`, idempotent `fix`, and integration health before a full browser or editor surface. |
| [Whip](https://github.com/KaminariOS/whip) | Mobile Herdr supervision; cross-host attention queue; native normalized chat; warm terminals; offline read-only output cache; queued composer outbox; SFTP/files; biometric SSH key storage. | Best reference for an attention-first mobile client, offline behavior, and separating terminal view from structured conversation view. |

### 3.2 Terminal engines, layout, and automation

| Project | Notable features | Useful direction for termi9ne |
|---|---|---|
| [Zellij](https://github.com/zellij-org/zellij) | Declarative layouts; floating and stacked panes; collaboration; WASM plugins; web client; strong out-of-box discoverability. | Use layouts as inspiration for Mission templates and consider a capability-limited extension model after the core contracts stabilize. |
| [WezTerm](https://github.com/wezterm/wezterm) | Cross-platform GPU terminal and multiplexer; rich configuration and automation surface. | Reference for cross-platform terminal ergonomics and configuration, not for replacing termi9ne's daemon/model seams. |
| [TUIOS](https://github.com/Gaurav-Gosain/tuios) | BSP, master-stack, and scrolling layouts; smart auto-split; command palette; copy mode; OSC 133 command blocks; graphics passthrough; JSON control protocol; session resurrection. | Borrow directional navigation, aspect-aware placement, and command-aware retained history. |
| [RMUX](https://github.com/Helvesec/rmux) | Typed Rust/Python/TypeScript SDKs; 90+ tmux commands; local daemon; snapshots/waits; Ratatui widget; encrypted web sharing. | Strong reference for a typed automation API, but tmux compatibility itself is not a termi9ne priority. |
| [amux](https://github.com/weill-labs/amux) | Parsed VT state as truth; structured JSON capture; blocking waits; push event stream; server-owned history; per-client copy mode; mailbox; SSH federation; MCP bridge. | Closest model for semantic automation and Run mail. Add capture/wait/events over existing canonical frames and domain events. |
| [boo](https://github.com/coder/boo) | Minimal libghostty-vt daemon; exact rendered `peek`; `wait --text` and `wait --idle`; binary-safe `send`; stable JSON and exit codes. | A small, high-value CLI contract termi9ne can implement before a broad SDK. |
| [smart-splits.nvim](https://github.com/mrjones2014/smart-splits.nvim) | Seamless directional navigation and resize between editor splits and multiplexer panes. | Add geometric Surface focus movement and resize commands with predictable edge behavior. |
| [tab-rs](https://github.com/austinjones/tab-rs) | Persistent named tabs; fuzzy finder; dynamic shell completion; hierarchical names; configuration-defined workspace entry points. | Borrow discoverability, hierarchical naming, and shell completion for Missions/Sessions. |
| [3mux](https://github.com/aaronjanse/3mux) | i3-inspired terminal tiling. | Useful only as layout/navigation reference. |
| [mtm](https://github.com/deadpixi/mtm) | Deliberately tiny terminal multiplexer. | Reminder to keep the PTY/session substrate small and deep. |
| [Byobu](https://github.com/dustinkirkland/byobu) | Friendly layer over traditional multiplexers with status and sensible defaults. | Reference for approachable defaults and status summaries, not architecture. |
| [pymux](https://github.com/prompt-toolkit/pymux) | Python terminal multiplexer modeled after tmux. | Compatibility reference only. |
| [muxy](https://github.com/muxy-app/muxy) | SwiftUI/libghostty terminal; project groups; worktrees; file/editor/preview surfaces; mobile companions; rich image input; quick terminal. | Reference for compact project grouping and attachments; most editor features are optional scope. |
| [Limux](https://github.com/am-will/limux) | Linux GTK/libghostty terminal; workspace persistence; browser; agent hooks; environment identity variables; generated agent instructions. | Useful packaging and agent-context patterns for Linux. |
| [Yazelix Nova](https://github.com/Yazelix/nova) | Nix-packaged terminal workspace with strict component ownership; popups; guided tutor; diagnostics; stable/main/edge channels. | Borrow component ownership discipline, `doctor`, guided onboarding, and explicit release channels. |

### 3.3 Security, review, and integration specialists

| Project | Notable features | Useful direction for termi9ne |
|---|---|---|
| [nono](https://github.com/nolabs-ai/nono) | Composable least-privilege profiles; per-tool nested policies; filesystem/network/credential rules; credential proxy; L7 API filtering; registry; audit and rollback. | Extend termi9ne's process-spec sandbox seam toward tool- and credential-scoped policy. Do not claim equivalent enforcement until independently proven. |
| [CodeGate](https://github.com/stacklok/codegate) | Security and multiplexing gateway for agent frameworks. | Reference for policy inspection and redaction; the reviewed search result was archived, so it should not become a foundational dependency. |
| [herdr-annotate](https://github.com/plannotator/herdr-annotate) | Annotate selected terminal text; review Markdown and agent replies; send collected feedback as the agent's next message. | Direct inspiration for selecting terminal ranges or Artifacts, collecting review comments, and returning one structured response to a Run. |
| [tmux-mcp](https://github.com/nickgnd/tmux-mcp) | MCP bridge for tmux. | Validates demand for agent-facing multiplexer tools. termi9ne should bridge its own typed protocol rather than wrap tmux. |
| [Navigator.nvim](https://github.com/numToStr/Navigator.nvim) | Editor-to-multiplexer directional navigation. | Additional reference for seamless focus movement. |

## 4. Consolidated feature catalog

### 4.1 Durable execution and workspace isolation

- Optional worktree per Run with explicit repository, base ref, branch, and
  cleanup policy.
- Preflight validation before mutating Git state.
- Setup and teardown hooks with closed stdin, bounded runtime, recorded output,
  and no silent cleanup failure.
- Dirty-worktree and unpushed-commit guardrails.
- Run-scoped file reservations to reduce predictable parallel collisions.
- Git branch, commits, diff, and checks recorded as structured facts or
  Artifacts rather than inferred from terminal prose.
- Explicit retry/resume policy; never auto-respawn every failed process by
  default.

### 4.2 Agent identity, status, and attention

- Bundled adapters for common agent CLIs.
- Precedence: authenticated termi9ne Signal, provider hook/event, conservative
  rendered-screen heuristic, then `Unknown`.
- `status explain` output naming the evidence, adapter version, timestamp, and
  confidence boundary.
- No timeout-based conversion from blocked to idle.
- Global shortcut to jump to the oldest or highest-risk unresolved attention.
- Optional token, context-window, cost, and model facts supplied by adapters;
  never scrape secrets or provider credentials.

### 4.3 Semantic control and automation

- Structured capture of terminal frame, cursor, viewport, title, modes, history,
  process metadata, Run, and Session.
- Blocking waits for text, quiet/idle, lifecycle, Signal, Artifact, attention,
  and mailbox events with mandatory timeout support.
- Push NDJSON event stream so scripts do not poll.
- Exact text/key input with binary-safe boundaries and existing control epochs.
- Stable exit codes and JSON errors.
- Thin MCP and typed SDK adapters generated over the same protocol after the CLI
  contract stabilizes.

### 4.4 Agent-to-agent coordination

- Event-sourced Run mailbox with sender, recipients, subject, body, topics, and
  correlation to a Mission/Run/Artifact.
- Separate unread, read, and acknowledged states.
- Replies form explicit threads.
- Delivery never changes terminal focus or writes into a PTY.
- Scheduler dependencies remain authoritative; mailbox messages do not silently
  complete or unblock Runs.

### 4.5 Source control, CI, and review

- Provider-neutral repository and change-request identity.
- Stable provider IDs instead of deriving identity repeatedly from mutable URLs.
- PR/MR, CI checks, reviews, mergeability, conflicts, and branch state attached
  to Runs.
- Failed CI and requested changes become attention backed by evidence.
- Human review of a diff, Artifact, or selected terminal range with inline
  comments bundled into one structured response.
- No automatic merge in the first slice; make merge an explicit, previewed
  owner action.

### 4.6 Mission templates and orchestration

- Versioned `.termi9ne/mission.toml` defining Runs, dependency edges, actors,
  drivers, worktree policy, scheduler policy, setup hooks, and initial Surface
  preferences.
- Idempotent reconciliation: applying a template twice does not duplicate Runs
  or launch work twice.
- Reusable prompt/context presets.
- Scheduled Missions with optional prechecks.
- Existing Mission DAG remains the only durable orchestration graph.

### 4.7 Terminal and desktop experience

- Geometric left/right/up/down Surface navigation.
- Aspect-ratio-aware initial placement while preserving stable order.
- Optional scrolling-column presentation for users supervising many tall agent
  Sessions.
- OSC 133-aware command/output blocks with jump, search, copy, export, and
  confirmation-gated rerun.
- Automatic suggested Session names from objective/branch/agent; explicit user
  names always win.
- Multiline prompt drawer, per-Session draft, attachments, and reusable input
  history kept separate from the raw PTY journal.
- Read-only changed-files and diff surface before considering a general editor.

### 4.8 Browser, remote, and multi-device access

- Optional Run-bound browser Surface with its own cookie/profile boundary.
- Accessibility-tree snapshot, stable element references, click/fill, console,
  network diagnostics, screenshot, and Artifact capture.
- Browser capability independent from terminal control and Mission mutation.
- Future TLS gateway with device pairing, expiry, revocation, and scoped roles.
- Attention-first mobile/web client before full editing: Missions, attention,
  approvals, status, Artifacts, and read-only terminal frames.
- Offline read cache and explicit queued-response outbox.
- Independent viewport/copy/search state for every client.

### 4.9 Security and reliability

- `termi9ne doctor` for read-only inspection of daemon, socket, terminfo,
  renderer, hooks, drivers, sandbox backends, permissions, and protocol versions.
- Idempotent `termi9ne fix` limited to clearly owned integration files.
- Nested per-tool sandbox profiles and credential mediation as a later security
  milestone.
- Optimistic file-save conflicts based on the exact opened version when a small
  editor is eventually allowed.
- Power, thermal, CPU, memory, and I/O measurements attributed to Runs without
  recording terminal contents or secrets.
- Scheduler policy that can reduce concurrency under resource pressure.

### 4.10 Extension model

- First extension seam should cover adapters, commands, Signal renderers, and
  narrow presentation widgets.
- Extensions declare capabilities; they do not inherit owner authority.
- Every client renders a small semantic vocabulary without executing plugin UI
  code where possible.
- Versioned manifests, exact source provenance, pinned resolutions, and explicit
  installation confirmation.
- No marketplace until the extension contract is stable and auditable.

## 5. Recommended implementation order

### P0 — complete the execution and review loop

1. Worktree-backed Runs and conservative cleanup.
2. Provider adapter kit with truthful, explainable status.
3. Structured capture, waits, and event subscription.
4. SCM/CI facts attached to Runs and attention.
5. Review-and-return workflow.
6. Event-sourced Run mailbox.

### P1 — improve repeatability and supervision

1. Mission-as-code templates.
2. Command-aware retained history.
3. Lightweight Git/diff review surface.
4. Resource-aware scheduler facts and policy.
5. Geometric navigation, automatic naming, and integration diagnostics.

### P2 — add new clients and extensibility

1. Isolated browser Surface.
2. Authenticated TLS gateway.
3. Attention-first mobile/web client.
4. Capability-safe extension SDK.
5. Curated marketplace only after the SDK and trust model stabilize.

## 6. Features deliberately not recommended now

- A freeform infinite canvas as the primary workspace.
- Kanban columns as stored workflow truth rather than a derived projection.
- Broad tmux command compatibility.
- A complete IDE/editor/file-manager replacement.
- Screen scraping as the authoritative source of agent state.
- Automatic focus changes when agents produce output or request attention.
- Automatic merging or forceful worktree deletion.
- Browser/mobile exposure through an unauthenticated LAN listener.
- Arbitrary in-process plugin execution before a capability and provenance model
  exists.

## 7. Exhaustive search-result inventory

The inventory below records every fetched result. “Relevant” means the project
has a terminal, agent-workspace, security, navigation, or automation idea worth
reviewing. “Peripheral” means its domain may contain transferable engineering
ideas but it is not a product comparator. “Off-topic” means “multiplexer” refers
to another domain.

### 7.1 `multiplexer` — page 1

| Repository | Classification | Survey disposition |
|---|---|---|
| [wezterm/wezterm](https://github.com/wezterm/wezterm) | Relevant | Cross-platform GPU terminal and multiplexer; configuration and terminal ergonomics reference. |
| [Gaurav-Gosain/tuios](https://github.com/Gaurav-Gosain/tuios) | Relevant | Layout modes, command blocks, graphics, automation, and session restoration. |
| [zellij-org/zellij](https://github.com/zellij-org/zellij) | Relevant | Layouts, collaboration, WASM plugins, and web client. |
| [go-zoo/bone](https://github.com/go-zoo/bone) | Off-topic | HTTP router. |
| [yrutschle/sslh](https://github.com/yrutschle/sslh) | Peripheral | Protocol classification on one port; not a terminal workspace. |
| [ros-teleop/twist_mux](https://github.com/ros-teleop/twist_mux) | Off-topic | ROS twist-command multiplexer. |
| [dustinkirkland/byobu](https://github.com/dustinkirkland/byobu) | Relevant | Friendly terminal multiplexer defaults and status. |
| [hashicorp/yamux](https://github.com/hashicorp/yamux) | Peripheral | Stream multiplexing protocol; possible transport reference only. |
| [soheilhy/cmux](https://github.com/soheilhy/cmux) | Off-topic | Go connection multiplexer. |
| [prompt-toolkit/pymux](https://github.com/prompt-toolkit/pymux) | Relevant | Python/tmux-style compatibility reference. |

### 7.2 `multiplexer` — page 2

| Repository | Classification | Survey disposition |
|---|---|---|
| [aaronjanse/3mux](https://github.com/aaronjanse/3mux) | Relevant | i3-inspired terminal layout. |
| [standardagents/dmux](https://github.com/standardagents/dmux) | Relevant | Worktree-isolated agent tasks, merge/PR flow, and durable resume. |
| [am-will/limux](https://github.com/am-will/limux) | Relevant | Linux libghostty workspace, browser, agent hooks, and packaging. |
| [numtide/treefmt](https://github.com/numtide/treefmt) | Off-topic | Formatter orchestration. |
| [vasanthkumarch/Exercise-07-Multiplexer-and-De-multiplexer](https://github.com/vasanthkumarch/Exercise-07-Multiplexer-and-De-multiplexer) | Off-topic | Digital logic exercise. |
| [trivago/gollum](https://github.com/trivago/gollum) | Off-topic | Message/log pipeline multiplexer. |
| [libp2p/rust-yamux](https://github.com/libp2p/rust-yamux) | Peripheral | Rust stream multiplexing protocol. |
| [deadpixi/mtm](https://github.com/deadpixi/mtm) | Relevant | Minimal terminal multiplexer and scope-discipline reference. |
| [devsisters/libquic](https://github.com/devsisters/libquic) | Off-topic | QUIC transport. |
| [danielinux/ttybus](https://github.com/danielinux/ttybus) | Relevant | Small TTY multiplexer; substrate reference. |

### 7.3 `multiplexer` — page 3

| Repository | Classification | Survey disposition |
|---|---|---|
| [amirlehmam/wmux](https://github.com/amirlehmam/wmux) | Relevant | Windows agent visibility, explainable fallback status, safe file review, and JSON-RPC. |
| [crosbymichael/slex](https://github.com/crosbymichael/slex) | Peripheral | SSH multiplexing. |
| [seebi/tmux-colors-solarized](https://github.com/seebi/tmux-colors-solarized) | Peripheral | Theme reference only. |
| [nfx/slrp](https://github.com/nfx/slrp) | Off-topic | Proxy multiplexer. |
| [goji/goji](https://github.com/goji/goji) | Off-topic | HTTP router. |
| [nickgnd/tmux-mcp](https://github.com/nickgnd/tmux-mcp) | Relevant | MCP demand signal for agent-controlled multiplexers. |
| [stripydog/kplex](https://github.com/stripydog/kplex) | Off-topic | Marine data multiplexer. |
| [max-mapper/multiplex](https://github.com/max-mapper/multiplex) | Peripheral | Binary stream multiplexing. |
| [rockorager/prise](https://github.com/rockorager/prise) | Relevant | Modern-terminal multiplexer; terminal UX reference. |
| [efrederickson/Multiplexer](https://github.com/efrederickson/Multiplexer) | Off-topic | iOS multitasking suite rather than an agent terminal. |

### 7.4 `multiplexer` — page 4

| Repository | Classification | Survey disposition |
|---|---|---|
| [chirpstack/chirpstack-packet-multiplexer](https://github.com/chirpstack/chirpstack-packet-multiplexer) | Off-topic | LoRaWAN packet forwarding. |
| [uber/tchannel](https://github.com/uber/tchannel) | Peripheral | Archived RPC multiplexing/framing protocol. |
| [Vanilagy/mp4-muxer](https://github.com/Vanilagy/mp4-muxer) | Off-topic | MP4 media muxer. |
| [rse/stmux](https://github.com/rse/stmux) | Relevant | Simple terminal multiplexer for Node environments. |
| [libimobiledevice/usbmuxd](https://github.com/libimobiledevice/usbmuxd) | Off-topic | iOS USB connection daemon. |
| [Helvesec/rmux](https://github.com/Helvesec/rmux) | Relevant | Typed SDKs, snapshots, waits, cross-platform daemon, and encrypted sharing. |
| [SapphicCode/protoplex](https://github.com/SapphicCode/protoplex) | Off-topic | Go protocol multiplexer. |
| [stealth/sshttp](https://github.com/stealth/sshttp) | Peripheral | SSH/HTTP port multiplexing. |
| [coder/boo](https://github.com/coder/boo) | Relevant | Minimal libghostty multiplexer with semantic automation primitives. |
| [rengwu/chartr](https://github.com/rengwu/chartr) | Relevant | Plan map, ticket-scoped agents, context, and transcript-backed status. |

### 7.5 `multiplexer` — page 5

| Repository | Classification | Survey disposition |
|---|---|---|
| [xtaci/smux](https://github.com/xtaci/smux) | Peripheral | Memory-efficient stream multiplexing library. |
| [inconshreveable/muxado](https://github.com/inconshreveable/muxado) | Peripheral | Go stream multiplexing. |
| [nolabs-ai/nono](https://github.com/nolabs-ai/nono) | Relevant | Agent sandbox policy, credentials, network rules, audit, and rollback. |
| [ponylang/ponyup](https://github.com/ponylang/ponyup) | Off-topic | Pony toolchain version multiplexer. |
| [austinjones/tab-rs](https://github.com/austinjones/tab-rs) | Relevant | Configured persistent tabs, fuzzy discovery, hierarchy, and shell completion. |
| [kennylevinsen/sshmux](https://github.com/kennylevinsen/sshmux) | Peripheral | SSH connection multiplexing. |
| [numToStr/Navigator.nvim](https://github.com/numToStr/Navigator.nvim) | Relevant | Seamless editor/multiplexer directional navigation. |
| [libimobiledevice/libusbmuxd](https://github.com/libimobiledevice/libusbmuxd) | Off-topic | Client library for iOS USB multiplexing. |
| [TharunPR/Multiplexer-Simulation-in-Vivado](https://github.com/TharunPR/Multiplexer-Simulation-in-Vivado) | Off-topic | Verilog multiplexer simulation. |
| [DigixGlobal/multiplexer](https://github.com/DigixGlobal/multiplexer) | Off-topic | Token-distribution smart contract. |

### 7.6 `agent multiplexer` — page 1

| Repository | Classification | Survey disposition |
|---|---|---|
| [gonet2/agent](https://github.com/gonet2/agent) | Off-topic | Game gateway with stream multiplexing. |
| [standardagents/dmux](https://github.com/standardagents/dmux) | Relevant | Worktree-isolated agent tasks and landing workflow. |
| [amirlehmam/wmux](https://github.com/amirlehmam/wmux) | Relevant | Windows agent visibility and explainable status. |
| [nolabs-ai/nono](https://github.com/nolabs-ai/nono) | Relevant | Least-privilege agent execution. |
| [rengwu/chartr](https://github.com/rengwu/chartr) | Relevant | Work map and ticket-scoped Sessions. |
| [Helvesec/rmux](https://github.com/Helvesec/rmux) | Relevant | Typed terminal automation SDK. |
| [stacklok/codegate](https://github.com/stacklok/codegate) | Relevant, archived | Agent security gateway; patterns only. |
| [herdrdev/herdr](https://github.com/herdrdev/herdr) | Relevant | Durable agent terminals, attention, automation, remote, and plugins. |
| [leapmux/leapmux](https://github.com/leapmux/leapmux) | Relevant | Worktrees, remote Worker, and E2E relay. |
| [no1msd/seance](https://github.com/no1msd/seance) | Relevant | Linux scrolling layout, hook status, notifications, and control API. |

### 7.7 `agent multiplexer` — page 2

| Repository | Classification | Survey disposition |
|---|---|---|
| [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux) | Relevant | Native terminal/browser workspace, notifications, hooks, socket API, and mobile. |
| [SiriusNEO/StarAgent](https://github.com/SiriusNEO/StarAgent) | Relevant | Lightweight web dashboard for multiple agents. |
| [overhacked/ssh-agent-mux](https://github.com/overhacked/ssh-agent-mux) | Off-topic | SSH credential-agent multiplexing. |
| [dlundquist/sshagentmux](https://github.com/dlundquist/sshagentmux) | Off-topic | SSH-agent authorization proxy. |
| [plannotator/herdr-annotate](https://github.com/plannotator/herdr-annotate) | Relevant | Terminal/document annotation returned to the agent. |
| [Sma1lboy/rove](https://github.com/Sma1lboy/rove) | Relevant | Worktrees, persistent Sessions, review, routines, and Agent API. |
| [tomtommyyuan/spmind](https://github.com/tomtommyyuan/spmind) | Off-topic | Spatial-proteomics research agent. |
| [suzuke/agend-terminal](https://github.com/suzuke/agend-terminal) | Relevant | Fleet-as-code, coordination tools, worktrees, and recovery. |
| [Stage-11-Agentics/c11](https://github.com/Stage-11-Agentics/c11) | Relevant | Agent-scriptable spatial terminal/browser/Markdown workspace. |
| [EnSue-Laboratories/RAYLINE](https://github.com/EnSue-Laboratories/RAYLINE) | Relevant | Parallel worktrees, checkpoints, tool streams, and terminal/chat supervision. |

### 7.8 `agent multiplexer` — page 3

| Repository | Classification | Survey disposition |
|---|---|---|
| [termio-sh/termio](https://github.com/termio-sh/termio) | Relevant | Durable daemon, semantic CLI waits, Git/worktrees, remote and mobile. |
| [eneskirca/nodeterm](https://github.com/eneskirca/nodeterm) | Relevant | Canvas/Kanban, context links, browser, Git, mobile, and resource behavior. |
| [RizRiyz/luvus](https://github.com/RizRiyz/luvus) | Relevant | Agent metrics, orchestration, GitHub, remote clients, and modules. |
| [tlbx-ai/tlbx](https://github.com/tlbx-ai/tlbx) | Relevant | Browser control station, structured agent sessions, composer, files, and verification. |
| [weill-labs/amux](https://github.com/weill-labs/amux) | Relevant | Structured capture, waits, events, mail, federation, and MCP. |
| [patriceckhart/hrdx](https://github.com/patriceckhart/hrdx) | Relevant | Minimal agent-era multiplexer; scope reference. |
| [gregce/tortie](https://github.com/gregce/tortie) | Relevant | Calm durable workspace, Git review, remote projects, and jump-to-attention. |
| [distribute-dev/modelplexer](https://github.com/distribute-dev/modelplexer) | Peripheral | Multiplexes AI protocols/models/tools rather than terminal work. |
| [KaminariOS/whip](https://github.com/KaminariOS/whip) | Relevant | Mobile Herdr supervision, native chat, offline cache, files, and secure SSH. |
| [flowmux-ai/flowmux](https://github.com/flowmux-ai/flowmux) | Relevant | Linux terminal/browser workspace, worktrees, hooks, editor, and diagnostics. |

### 7.9 Additional results from the stars-sorted comparison

These entries appeared in the comparison pass but not in the eight canonical
Best match pages above.

| Repository | Classification | Survey disposition |
|---|---|---|
| [herdrdev/herdr](https://github.com/herdrdev/herdr) | Relevant | Durable agent terminals, attention, automation, remote access, and plugins. |
| [acassen/keepalived](https://github.com/acassen/keepalived) | Off-topic | High-availability networking. |
| [masterking32/MasterHttpRelayVPN](https://github.com/masterking32/MasterHttpRelayVPN) | Off-topic | HTTP/SOCKS relay and VPN. |
| [directvt/vtm](https://github.com/directvt/vtm) | Peripheral | Networked text-mode desktop with overlapping windows and remote viewing. |
| [muxy-app/muxy](https://github.com/muxy-app/muxy) | Relevant | Project groups, worktrees, previews, attachments, and mobile companions. |
| [mrjones2014/smart-splits.nvim](https://github.com/mrjones2014/smart-splits.nvim) | Relevant | Seamless directional editor/multiplexer navigation and resizing. |
| [Yazelix/nova](https://github.com/Yazelix/nova) | Relevant | Component-owned packaged workspace, guided onboarding, and diagnostics. |
| [cosmos72/twin](https://github.com/cosmos72/twin) | Peripheral | Networked text-mode window environment. |
| [Arc-Compute/LibVF.IO](https://github.com/Arc-Compute/LibVF.IO) | Off-topic | GPU/VFIO multiplexing. |
| [ShahabSL/Skirk](https://github.com/ShahabSL/Skirk) | Off-topic | Encrypted TCP transport through a cloud-drive mailbox. |
| [nanozuki/tabby.nvim](https://github.com/nanozuki/tabby.nvim) | Peripheral | Declarative Neovim workspace tabline. |
| [dotnet/Nerdbank.Streams](https://github.com/dotnet/Nerdbank.Streams) | Peripheral | In-process streams, pipes, and protocol multiplexing. |
| [THUDM/GATNE](https://github.com/THUDM/GATNE) | Off-topic | Academic attributed multiplex-network representation learning. |
| [uber/tchannel-go](https://github.com/uber/tchannel-go) | Peripheral | Go RPC multiplexing/framing protocol. |
| [hkupty/nvimux](https://github.com/hkupty/nvimux) | Relevant | Neovim used as a tmux replacement; editor/workspace navigation reference. |

### 7.10 Separately requested projects outside those result pages

| Repository | Classification | Survey disposition |
|---|---|---|
| [pingdotgg/t3code](https://github.com/pingdotgg/t3code) | Relevant | Multi-device control, SCM providers, composer, pairing, and resource telemetry. |
| [get-bb/bb](https://github.com/get-bb/bb) | Relevant | First-class API surfaces, worktree provisioning, multi-machine execution, and plugins. |
| [Untrivial-ai/agent-orchestrator](https://github.com/Untrivial-ai/agent-orchestrator) | Relevant | Worktree workers, agent adapters, SCM/CI/review facts, browser profiles, and feedback routing. |

Herdr was also separately requested, but it already appears in the stars-sorted
comparison and in the detailed notes.

## 8. Broader agentic IDE and orchestration discovery

The second research pass expanded beyond repositories containing the word
“multiplexer.” GitHub searches covered these overlapping concepts:

- `agent IDE` and `agentic development environment`;
- coding-agent orchestration and control planes;
- parallel coding agents and agent fleets;
- Git-worktree-backed coding agents;
- agent workspaces, supervision, review, and durable execution;
- semantic source control, agent provenance, portable agent state, and sandbox
  infrastructure.

Search results were filtered to projects with concrete developer workflows,
runtime or workspace architecture, or a genuinely transferable primitive.
Generic LLM frameworks, awesome lists, prompt collections, individual coding
agents, and unrelated business agents were excluded even when their star counts
were high.

### 8.1 New direct comparators

| Project | Distinctive features | Relevance and cautions |
|---|---|---|
| [Superset](https://github.com/superset-sh/superset) | Large parallel worktree fleet; compare-and-merge workflow; persistent terminal presets; rich prompt editor; diff comments; per-worktree port previews; schedules; remote hosts; CLI, SDK, and MCP. | Strong product reference for fan-out experiments, preview discovery, and automation. Source is available under Elastic License 2.0, so treat it as behavioral research rather than a code source. |
| [Orca](https://github.com/stablyai/orca) | Fan one prompt to several agents and merge the winner; annotated diffs; real-browser element capture; remote SSH worktrees; desktop/mobile clients; usage and rate-limit tracking; Computer Use. | Best new reference for explicit result comparison and UI-context capture. MIT-licensed at review time. |
| [Emdash](https://github.com/generalaction/emdash) | Parallel worktrees; issue intake from Linear, GitHub, Jira, GitLab, Asana, Featurebase, Monday, Forgejo, and Plain; diff/PR/CI flow; remote SSH/SFTP projects; marker-owned provider hooks. | Useful model for tracker adapters and integration files that remain inert outside the product. Apache-2.0 at review time. |
| [OpenChamber](https://github.com/openchamber/openchamber) | Session Goals that continue until complete, blocked, or limited; up-to-five-run comparison and Fusion; guided Changes Walkthrough; browser inspection; PR feedback loop; scheduled goal runs; E2E private relay. | The goal-loop boundary and guided diff tour are distinctive. Fusion should remain an explicit new Run with provenance, never an invisible merge. |
| [Pane](https://github.com/dcouple/Pane) | Agent-agnostic terminal workspace; automatic worktree/rebase/cleanup lifecycle; remote desktop/phone client; global orchestrator terminal; stable `runpane` CLI with lazy-loaded agent help; panes as worktrees and tabs as tools. | Strong example of keeping the terminal as the universal adapter and making worktree mechanics disappear without hiding destructive actions. |
| [agtx](https://github.com/fynnfluegge/agtx) | Blackboard task model; worktree/tmux isolation; phase-specific agent switching; dependency gating; automatic artifact propagation; spec-framework plugins; orchestrator over MCP. | The phase-to-agent mapping and artifact handoff fit termi9ne's DAG. Avoid duplicating the Mission graph with a separate board database. |
| [Gas Town](https://github.com/gastownhall/gastown) | Persistent worker identity over ephemeral sessions; Git-backed work records; TOML workflow formulas; scheduler; three-tier watchdogs; severity escalation; previous-session discovery; problem view; OTLP events and metrics. | Best new reference for health reconciliation and escalation at fleet scale. Its themed role hierarchy should be translated into explicit termi9ne domain vocabulary. |
| [Paperclip](https://github.com/paperclipai/paperclip) | Organization goals, roles, budgets, governance, tickets, atomic work checkout, heartbeat queue, orphan recovery, workspaces, schedules, audit, out-of-process plugins, scoped secrets, organization export/import. | Valuable control-plane patterns: execution leases, coalesced wakeups, hierarchical budgets, hard stops, and portable scrubbed exports. Org charts are outside termi9ne's current product scope. |
| [AgentsMesh](https://github.com/AgentsMesh/AgentsMesh) | Runner fleet; AgentPods combining PTY, worktree, and stream; scheduler; control plane over gRPC/mTLS; separate stateless terminal relay; collaboration mesh/channels; autonomous pod watchdog. | Reference for a future multi-machine fleet where central orchestration does not proxy every PTY byte. The repository identified itself as BSL-1.1 at review time. |
| [Omnigent](https://github.com/omnigent-ai/omnigent) | Meta-harness across existing agents; synchronized terminal/browser/phone Sessions; multi-agent supervision; disposable cloud sandboxes; shared/forked Sessions; policy stacking; YAML agents; cross-provider review; harness conformance test bench. | Strong reference for adapter contracts, cross-provider verification, policy composition, and capability testing. Multi-user co-drive needs a stronger authority model than a shared chat link. |
| [Apache Maka](https://github.com/apache/maka) | Local-first Runtime Host; append-only model/tool/permission/termination record; context reduction without evidence deletion; sandbox boundary; crash recovery; branch-from-Turn; durable graph execution; normalized evaluation kernel. | Closely aligned with termi9ne's event-sourced philosophy. The key new idea is separating prompt compaction from evidence retention. Apache-2.0, but still incubating and prerelease at review time. |
| [ORG-II](https://github.com/org2AI/ORG2) | Imports and replays sessions from many agent CLIs; synchronized trajectory timeline; comments on execution steps; “AI blame” from code to agent decisions; shared memory; Git/browser/LSP surfaces; resource-aware execution. | Excellent reference for provenance and trajectory review. AGPL-3.0-or-later at review time. |
| [Atlas](https://github.com/pacifio/atlas) | Commits linked to prompts, tool calls, reasoning, and file changes; cross-agent local memory; `@` references to code, commits, notes, and past Sessions; ACP agents; secret scrubbing; checkpoint links surviving rebases/amends. | The most focused reference for Run/commit checkpoints and queryable development provenance. MIT at review time. |
| [Kungfu](https://github.com/kungfu-systems/kungfu) | Durable Work exists independently of chat; multiple Attempts survive disconnect/crash; single-writer ownership; explicit evidence and next action; independent review and settlement authority. | Its Work/Attempt/Settlement separation maps naturally to Mission objective, Run attempts, and human acceptance. Strong model for preventing the producing agent from approving itself. |
| [Zeroshot](https://github.com/the-open-engine/zeroshot) | Task classification selects workflow strength; isolated executor plus independent verifier; validators do not share reasoning context; crash-safe SQLite ledger; resumable message-bus graphs; issue-provider adapters. | Best direct reference for independent verification. The trivial fast path shows verification cost can be policy-driven instead of universal. MIT at review time. |
| [Mission Control](https://github.com/builderz-labs/mission-control) | Runtime-neutral task and quality-review control plane; tasks, heartbeats, Sessions, schedules, spend, activity, skills, memory graph, OpenAPI, CLI, and MCP; explicit operator evidence guidance. | Useful small control-plane reference, especially its distinction between logs and completion receipts. Alpha status means contracts should not be adopted verbatim. |
| [SuperPlane](https://github.com/superplanehq/superplane) | Git-backed workflow applications; event triggers; deterministic durable graphs; human approvals; policy checks; retries/resume; app memory; operational consoles; CI/deploy/incident integrations. | Useful for future webhook- and event-driven Missions that cross Git, CI, observability, and deployment systems. It is adjacent orchestration infrastructure rather than an agent IDE. |
| [Tutti](https://github.com/tutti-os/tutti) | Real-time shared multi-user/multi-agent workspace; cross-agent `@` references to conversations, files, tasks, and app outputs; shared task decomposition; locally run agents connected to cloud rooms. | Strong reference for typed context references and cross-agent handoffs. The cloud-room collaboration model is later scope and requires explicit privacy/authority boundaries. |
| [Claude Codex Bridge](https://github.com/SeemSeam/claude_codex_bridge) | Visible provider-neutral agent topology; stable cross-provider requests; daemon persistence; shared memory; mobile control; rich terminal/files; Agent Roles specification and Role Packs; transactional updates. | Useful for an adapter-neutral role package and cross-agent request contract. termi9ne should store coordination as domain events rather than a shared Markdown file alone. |
| [Archon](https://github.com/coleam00/Archon) | YAML coding workflows with deterministic and AI nodes; loops, validations, approvals, artifacts, and PR creation; worktree isolation; reusable workflow packs; CLI/web/chat-platform execution. | Strong reference for a future declarative Mission format. The useful unit is a versioned workflow contract, not the visual canvas itself. |
| [Babysitter](https://github.com/a5c-ai/babysitter) | Harness-neutral deterministic processes; enforced gates; human breakpoints; immutable journal; adapter runtime and SDK; internal headless harness. | Reinforces runtime-enforced workflow gates. Claims such as “hallucination-free” should not be repeated without narrowly defined proof. |
| [Stagewise](https://github.com/stagewise-io/stagewise) | Agent sees live browser tab console/debugger; temporary versus connected-codebase edits; website component/style reverse engineering; editor handoff; broad model/provider support. | Browser/debugger context is useful, but this is closer to a browser-centric IDE than a multiplexer. AGPL-3.0 at review time. |
| [Claude Code Haha](https://github.com/NanmiCoder/cc-haha) | Worktree Sessions; per-turn diffs and rollback; five permission modes; browser preview; visual MCP/SubAgent management; agent-team dependency view; dynamic orchestration scripts; local model traces; remote messaging. | Interesting observability and provider-management reference. Dynamic agent-authored orchestration scripts need capability and approval boundaries. |
| [Yao](https://github.com/YaoApp/yao) | Agents execute on user devices; isolated workspaces; shared task board across desktop/mobile/browser/API; workspace documents become knowledge; agents can read across nodes. | Reference for a multi-device worker topology and accumulated workspace knowledge. The reviewed README did not expose enough detail to adopt its trust model. |
| [Omnara](https://github.com/omnara-ai/omnara) | Durable agent state in Postgres; hot-pluggable machines and sandboxes; built-in/custom tools and MCP; organization/project RBAC; queryable history; CLI, SDK, and API. | Useful reference for durable remote agents and role separation, though it is a managed-agent platform rather than a terminal workspace. |
| [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) | Issue planning; branch/terminal/dev-server workspaces; inline diff comments; preview browser; multi-agent choice; PR creation and merge. | Feature patterns remain useful, but the repository announced it is sunsetting. Do not build a dependency or roadmap assumption around it. |

### 8.2 Innovative adjacent repositories

| Project | Primitive | Potential termi9ne use |
|---|---|---|
| [sem](https://github.com/Ataraxy-Labs/sem) | Tree-sitter entity diffs, rename/move detection, dependency impact, entity history/blame, co-change hotspots, and token-budgeted context. | Enrich diff Artifacts and agent context with semantic entities and blast radius without feeding entire files. Keep it an optional adapter until language accuracy is proven. |
| [Git AI](https://github.com/git-ai-project/git-ai) | Explicit line-level agent/model/session attribution stored in Git Notes; prompt links kept outside Git; attribution propagation across rebases, squashes, stashes, and merges. | Add an opt-in export from Run checkpoints to a standard Git provenance layer. Explicit reporting is preferable to heuristic “AI code detection.” |
| [Agent File](https://github.com/letta-ai/agent-file) | Portable serialized agent prompt, editable memory, tools, and model settings with secrets nulled on export. | Inspires a provider-neutral Driver/Actor profile bundle, but importing executable tools must require provenance and capability review. |
| [Continuous Claude](https://github.com/parcadei/Continuous-Claude-v3) | “Compound, don't compact”: extract decisions and learnings before starting fresh context; rule-based skill activation; shift-left validation hooks. | Add explicit context-summary Artifacts with source links and validation rather than treating lossy provider compaction as durable Mission memory. |
| [OpenShell](https://github.com/NVIDIA/OpenShell) | Container/MicroVM sandbox control plane; declarative filesystem/process/network/inference policy; L7 egress enforcement; credential providers; hot-reloadable dynamic policy. | Strong adjacent reference for future network and credential enforcement beyond termi9ne's current workspace-write sandbox. |
| [Wigolo](https://github.com/KnockOutEZ/wigolo) | Local-first web intelligence over MCP, REST, CLI, and SDKs; search, fetch, crawl, extraction, local cache, similar-page retrieval, research, autonomous gather, change diff, and watch; byte-pinned excerpts and explicit freshness/degradation signals. | Useful reference for an optional Web Evidence adapter and evidence Artifact contract. Keep direct HTTP as the default and escalate to a headless browser only on observable signals. The README identifies the public-beta project as AGPL-3.0. |

### 8.3 Projects intentionally left out of the direct comparison

The searches also returned important projects such as
[OpenHands](https://github.com/OpenHands/OpenHands),
[Daytona](https://github.com/daytonaio/daytona),
[CrewAI](https://github.com/crewAIInc/crewAI), agent and skill catalogs, model
routers, general workflow builders, and individual coding-agent harnesses. They
may be useful dependencies or ecosystem references, but they do not directly
answer termi9ne's product question: how should a human supervise durable,
terminal-native agent work organized as Missions, Runs, and Sessions?

## 9. New feature concepts from the broader pass

### 9.1 Work, Attempt, and Settlement

Several projects distinguish the durable objective from one agent execution.
termi9ne already has the necessary ingredients, but the product should make the
separation more explicit:

```text
Mission objective
  -> logical work item
      -> Attempt 1 / Run: disconnected
      -> Attempt 2 / Run: produced candidate
      -> independent verification Run
      -> owner settlement: accept, reject, or retry
```

- A failed or replaced agent Session must not erase the work item.
- Only one actor owns a writable Attempt at a time.
- The producer may submit evidence but cannot independently settle its own work
  when policy requires verification.
- Settlement records the exact candidate, evidence, verification, and authority.

Primary references: Kungfu, Zeroshot, Paperclip, and Agent Orchestrator.

### 9.2 Goal continuation with explicit limits

A Run may opt into a continuation policy after each agent turn:

- stop when the goal is satisfied by defined evidence;
- stop and raise attention when blocked or approval is required;
- stop at a turn, time, token, or cost limit;
- continue with the same agent Session only when its control epoch and policy
  remain valid;
- record every continuation decision as an event.

This borrows OpenChamber's Session Goals without allowing an unbounded autonomous
loop.

### 9.3 Fan-out experiments and result comparison

One prompt can create several sibling Runs with different agents, models, or
approaches. The comparison must be a first-class workflow:

- identical frozen objective/context snapshot for each sibling;
- independent worktrees and resource budgets;
- normalized checks and Artifact summaries;
- side-by-side semantic diff and evidence comparison;
- explicit winner selection or a new provenance-linked synthesis Run;
- losing Runs remain inspectable and may be archived, never silently deleted.

Primary references: Orca, Superset, OpenChamber, and RayLine.

### 9.4 Checkpoints and development provenance

- Link each commit or patch checkpoint to the producing Run, objective, agent,
  driver snapshot, relevant tool actions, tests, and review.
- Preserve links through amend/rebase where technically possible.
- Let a reviewer ask “why does this line/entity exist?” and navigate to the
  exact evidence rather than an unbounded transcript.
- Optionally export explicit attribution using Git Notes or another open format.
- Keep prompts and sensitive transcripts outside ordinary Git history unless
  the owner deliberately exports them.

Primary references: Atlas, ORG-II, Git AI, and sem.

### 9.5 Evidence-preserving context compaction

Provider context reduction must not become domain-history deletion:

- retain the original Signal, Artifact, decision, and tool-result evidence;
- create a summary Artifact that cites its source event ranges;
- mark omissions, conflicts, and uncertainty;
- validate critical summaries before using them as cross-agent handoff context;
- allow a fresh agent to receive the compact summary while humans retain the
  complete audit record.

Primary references: Apache Maka, Continuous Claude, Atlas, and Kungfu.

### 9.6 Execution leases, heartbeats, and reconciliation

- Atomically lease ready work to one Run/worker.
- Renew with authenticated heartbeats.
- Coalesce repeated wakeups for the same work.
- Treat missed heartbeats as an observation, not immediate death.
- Reconcile orphaned leases conservatively and keep prior Attempts visible.
- Escalate repeated stalls through explicit severity and attention policy.

Primary references: Paperclip, Gas Town, AgentsMesh, and Agent Orchestrator.

### 9.7 Hierarchical budgets and policy hard-stops

Budgets should be composable across runtime, Mission, Run, agent/provider, and
scheduled routine:

- tokens, currency, wall time, retries, and concurrent slots;
- warning thresholds and hard stops;
- stricter policy wins when scopes overlap;
- overspend stops new work and raises attention rather than destroying active
  evidence;
- costs are facts attached to Runs and roll up without storing credentials.

Primary references: Paperclip, Omnigent, T3 Code, and Luvus.

### 9.8 Typed cross-agent references

Instead of copying text between agents, prompts may reference stable objects:

- Mission, Run, Session, Signal, Artifact, file, symbol, commit, diff, test
  result, decision, or prior context summary;
- references resolve at send time under the receiving Run's capability;
- large objects become paths or bounded summaries, not unconditional prompt
  pastes;
- access is audited and revoked with the underlying Share or Run capability.

Primary references: Tutti, Atlas, CCB, and termi9ne's existing domain IDs.

### 9.9 Adapter conformance and capability matrices

Every provider adapter should run against a common behavioral suite:

- launch, resume, interrupt, and terminate;
- status and attention evidence;
- exact prompt delivery and completion boundaries;
- model selection and usage reporting;
- subagent visibility;
- safe failure when a capability is unavailable;
- fixture-based regression without live credentials, plus separately gated live
  acceptance tests.

Primary references: Omnigent's harness test bench, Agent Orchestrator's adapter
suite, and CCB's provider-neutral collaboration layer.

### 9.10 Remote fleet architecture

If termi9ne grows beyond owner SSH attachment, separate:

- control plane: scheduling, identity, policy, leases, events, and metadata;
- execution plane: per-machine Runner owning PTYs, worktrees, and credentials;
- terminal data plane: direct or stateless-relay frame streams that do not make
  the central scheduler a terminal-byte bottleneck;
- device clients: scoped views over the same durable Run/Session facts.

Primary references: AgentsMesh, Omnigent, Omnara, Yao, and LeapMux.

### 9.11 Evidence-native web research

Web research should produce inspectable evidence rather than only synthesized
prose:

- source URL, retrieval time, content digest, and extraction method;
- verbatim excerpt with a byte or DOM span and stable citation ID;
- freshness evidence, cache state, and decomposed relevance scores;
- explicit blocked, degraded, stale, weak, and truncated states;
- direct HTTP first, with headless-browser escalation only after observable SPA
  or challenge signals;
- cached-page diff and watch events that can trigger a versioned Mission.

This belongs behind an optional MCP/Driver seam. A nested autonomous research
loop must not become a second opaque Mission system.

Primary reference: Wigolo. The related research implications are developed in
[arxiv-agent-systems-review-2026-09-01.md](arxiv-agent-systems-review-2026-09-01.md).

## 10. Roadmap changes from the broader pass

These are deltas to the implementation order in section 5.

### Promote into P0

1. Add an authenticated execution lease when worktree-backed Run launching is
   implemented.
2. Make independent verification and settlement available as Mission policy.
3. Link candidate commits/checkpoints to the producing Run and evidence.
4. Preserve full evidence when generating compact cross-agent context.
5. Add adapter conformance fixtures alongside the first provider adapter kit.

### Add to P1

1. Bounded goal-continuation policies.
2. Fan-out/compare/synthesize as an explicit Mission workflow.
3. Hierarchical token, cost, time, and retry budgets.
4. Health reconciliation, stall escalation, and a derived “problems” view.
5. Semantic diff/impact adapter and typed cross-agent references.
6. Webhook and schedule triggers for versioned Mission templates.
7. Evidence-native web adapter with explicit provenance and degraded states.

### Keep in P2

1. Multi-machine Runner fleet and separated terminal data plane.
2. Portable Actor/Driver profile bundles.
3. Multi-user real-time rooms and co-drive.
4. Organization-level governance, plugin stores, and business workflow concepts.

## 11. Bottom line

The survey does not suggest replacing termi9ne's foundation. Its durable
Mission/Run/Session graph, structured Signals, canonical terminal frames, and
explicit control model are stronger foundations than the pane-centric designs
in most of the field.

The clearest opportunity is to complete the operational loop around that
foundation:

```text
Mission plan
  -> isolated worktree-backed Runs
  -> truthful adapter Signals
  -> semantic capture/wait/control
  -> PR, CI, and review facts
  -> human feedback and Run mailbox
  -> explicit landing or recovery
```

Browser, mobile, and plugins are valuable later products. Worktree isolation,
SCM feedback, semantic automation, and evidence-backed attention are the
features most likely to make termi9ne materially better rather than merely
broader.
