# Multiplexer product design — v1

> Superseded by the browser-tab and terminal-waterfall direction in
> [`browser-waterfall-v1.md`](browser-waterfall-v1.md). This document is retained
> as the earlier Mission Spine exploration.

## Brief

superplexr is a local workstation for a developer supervising several autonomous
agents and ordinary shell processes. Its single job is to reveal what needs human
judgment and make intervention immediate without losing the causal history of the
work.

The product is not a pane manager with agent badges. A terminal surface is an
instrument used while inspecting or steering a run. The durable product is the
mission: its intent, causal run graph, signals, interventions, and artifacts.

## Product principles

1. **Attention, not output, drives the interface.** Routine terminal output never
   competes visually with questions, blockers, or approvals.
2. **Nothing steals focus.** New output and agent signals may update indicators,
   but only an explicit human action changes the focused terminal.
3. **Layout is disposable.** A human may arrange surfaces for comparison, but
   panes are not saved as mission structure and closing one never ends a run.
4. **Control is explicit.** Every surface visibly states whether the agent, the
   human, or nobody holds the write lease.
5. **Causality is visible.** Parent and child runs, decisions, and produced
   artifacts remain connected instead of becoming disconnected terminal logs.
6. **Unaware programs still work.** A shell or TUI gets a correct terminal even
   when it never emits an agent signal.
7. **Reported permission is not enforced permission.** Until sandbox enforcement
   exists, approval requests are labeled as agent-reported and never presented as
   an operating-system security guarantee.

## Information architecture

```text
Mission
├── intent
├── status
├── run graph
│   └── Run
│       ├── actor + objective
│       ├── parent + children
│       ├── process + terminal state
│       ├── controller
│       ├── signals
│       └── artifacts
├── attention queue (derived from unresolved signals)
└── event history

Desktop view
├── Mission Spine      causal navigation
├── Focus Stage        one primary surface or review
├── Attention Dock     unresolved human decisions
└── Command Deck       create, delegate, navigate, and act
```

The attention queue is a projection, not an inbox. It contains only unresolved
signals requiring judgment. Completion notices and ordinary progress remain on
their originating run.

## Primary workspace

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ Ship cross-platform v1     7 active · 2 need you       Delegate    Search  ⌘K│
├──────────────────────┬───────────────────────────────────┬───────────────────┤
│ MISSION SPINE        │ FOCUS STAGE                       │ ATTENTION DOCK    │
│                      │                                   │                   │
│ ● orchestrator       │ renderer-spike / researcher      │ HIGH · APPROVAL   │
│ ├──● researcher ─────┼─┐ Agent controls input           │ Delete branch?    │
│ │  └──◐ build-check  │ │                                 │ Inspect · Deny ·  │
│ ├──◆ linux-qa        │ │ $ cargo test                    │ Allow once        │
│ └──✓ protocol        │ │ ...                             │                   │
│                      │ │                                 │ INPUT              │
│ Latest signals       │ │                                 │ Choose package     │
│ researcher  18s      │ └─────────────────────────────────│ Reply              │
│ build-check needs you│                                   │                   │
├──────────────────────┴───────────────────────────────────┴───────────────────┤
│ ⌘K  Type an action, run, actor, file, or command…                            │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Mission Spine

The signature element is a compact causal rail rather than a list of cards.
Every run occupies one labeled branch from its parent. Branch length communicates
recency, not importance; status is encoded by both shape and text so color is
never the only cue. Selecting a branch changes the Focus Stage. Expanding it
reveals recent structured signals and children, not a continuously animated log.

When a run raises an attention item, one short pulse travels from its branch to
the Attention Dock. There is no ambient animation. Reduced-motion mode replaces
the pulse with an immediate state change.

### Focus Stage

The stage shows exactly one primary object:

- a live terminal surface;
- an artifact review such as a diff or test report;
- a structured attention context;
- the mission overview when no run is selected.

The human may pin up to four surfaces to compare runs. Pinned surfaces tile only
inside the stage and disappear when unpinned; their geometry is local view state.
Background and occluded surfaces stop rendering, while their daemon-owned runs
continue processing output.

The stage header always shows the run lineage, objective, actor, connection
state, and write-lease owner. Taking control adds a persistent strip:

```text
You control input · Agent input paused                     Return control
```

### Attention Dock

The dock is absent when empty, giving its space back to the stage. It orders
items deterministically:

1. high-risk approval;
2. blocked run;
3. other approval;
4. requested input;
5. oldest item within the same class.

An item contains the exact request, the actor and run, elapsed wait time, and one
primary verb. “Inspect context” opens the relevant terminal or artifact without
resolving the item. Destructive approvals offer `Deny` and `Allow once`; they do
not use vague `Yes` and `No` labels.

### Command Deck

The deck is the fastest path to every action and the only global overlay. It
searches runs, actors, signals, artifacts, and actions in the same field.
`Command-K` on macOS and `Super-K` on Linux opens it without consuming common
terminal control keys. All actions remain reachable through visible controls and
full keyboard navigation.

Example commands:

- `Delegate “verify Linux packaging” from renderer-spike`
- `Take control of build-check`
- `Show unresolved approvals`
- `Pin linux-qa beside researcher`
- `Cancel protocol and descendants`

## Core workflows

### 1. Begin a mission

The empty state asks for one thing: “What outcome are we driving toward?” The
human enters the intent, chooses a starting directory, and either starts a shell
or delegates the first run. Advanced runtime, environment, and agent settings
stay collapsed behind “Configure run.”

### 2. Delegate a child run

From any run, `Delegate` creates a child with a concrete objective, actor,
working directory, and optional isolated Git worktree. The child appears on the
parent branch immediately in `Starting`; failure to spawn becomes a signal on
that run rather than a transient toast.

### 3. Handle attention

Selecting an attention item opens a context sheet over the Focus Stage with:

- the structured question or operation;
- why the actor says it is needed;
- the last relevant command and bounded terminal excerpt;
- related artifact or diff, when present;
- the exact consequence of each response.

Replying appends an intervention, resolves the signal, and returns the run to
`Running` only when it has no other unresolved attention items.

### 4. Take and return control

`Take control` requests the run's write lease. Once granted, agent-originated
input pauses and keystrokes go directly to the PTY. Returning control is an
explicit action; changing focus does not return it.

If the controlling client disconnects, the daemon waits a short grace period and
then raises an attention item named “Human control disconnected.” It does not
silently resume agent input because a partially entered command may remain on the
terminal line.

### 5. Review an outcome

A successful process exit does not erase its run. The stage switches to a quiet
summary containing the actor's result, artifacts, test evidence, duration, and
children. The human can accept the result, delegate follow-up work, reopen a
surface, or mark the run failed from the mission's perspective.

### 6. Detach and reattach

Closing the desktop leaves the runtime and all PTYs alive. Reopening restores the
mission projection first, then terminal snapshots and subsequent output. A slow
or stale surface resynchronizes from a snapshot rather than blocking PTY reads or
forcing every other client to wait.

## State model

```text
Lifecycle    Starting ─────▶ Active ─────▶ Exited
                                 └────────▶ Cancelled

Attention    Clear ◀────────────▶ Needs attention

Control      Actor ◀────────────▶ Human ─────▶ Unclaimed

Disposition  Not applicable ────▶ Awaiting review ────▶ Accepted
                                                     └▶ Rejected
```

These axes are deliberately orthogonal. Human control is derived from the
controller and write lease. Attention is derived from unresolved
response-required signals. Lifecycle follows the child process, while disposition
records mission judgment. A process may therefore exit successfully, still need
an answer, and remain awaiting review without creating a contradictory status.

### Completed domain-model change

The original prototype's `RunStatus` combined lifecycle, attention, and process
outcome. The implemented model now keeps lifecycle, controller, attention
projection, process exit, and disposition independent. `RunStarted` establishes
`Active`, `SignalRaised` and `SignalResolved` affect only attention, and
`RunFinished` records process exit before a separate disposition decision.

## Visual direction

The interface should feel like a quiet routing instrument, not a cyberpunk
terminal or generic analytics dashboard. Dense information is organized by
alignment and signal traces rather than rounded cards, glow, gradients, or
decorative charts.

### Tokens

| Role | Token | Value |
|---|---|---|
| Window and terminal surround | Deck | `#10151B` |
| Raised working planes | Slate | `#19212A` |
| Primary text | Chalk | `#E8EDF2` |
| Secondary traces and labels | Trace | `#8795A5` |
| Active/focused work | Relay | `#79A7D3` |
| Decisions and failures | Signal | `#D8A85B` / `#D7776B` |

Familjen Grotesk carries mission intent, navigation, and readable interface copy.
Iosevka Term carries terminals, identifiers, timestamps, and compact state labels.
Both fonts are bundled so metrics and hierarchy remain stable across macOS and
Linux. Type uses sentence case; uppercase is reserved for short machine states
such as `BLOCKED` or `FAILED`.

Geometry uses an 8 px spacing grid, square branch joints, 2 px control radii,
and one-pixel separators. The Mission Spine is the one expressive device; other
surfaces remain visually restrained. Keyboard focus uses a two-pixel Relay ring,
and all state colors have a paired glyph and text label.

## Runtime modules and interfaces

| Module | Small interface | Complexity hidden behind the seam |
|---|---|---|
| `MissionRuntime` | `dispatch(command) -> events`, `view()` | command validation, causal graph, attention projection, optimistic versioning |
| `RunSupervisor` | `apply(run_command) -> run_events`, `subscribe(run)` | child lifecycle, PTY, resize, signals, write lease, process groups |
| `TerminalModel` | `advance(action) -> effects`, `frame()`, `checkpoint()` | all `libghostty-vt` pointers, lifetimes, callbacks, encoding, snapshots |
| `AttachmentHub` | `attach(request) -> stream` | sequencing, replay window, resync, observer backpressure |
| `AgentGateway` | `report(capability, signal)` | run authentication, schema validation, rate limits, event conversion |
| `TerminalRenderer` | `prepare(frame)`, `paint(pass)` | glyph shaping, atlas eviction, damage tracking, cursor and decorations |

The public runtime interface is the versioned protocol, not these internal
modules. Only true variation gets an adapter: the disk event store has an
in-memory test adapter, and small platform adapters cover notifications,
clipboard, and runtime installation. The Ghostty wrapper is concrete rather than
a speculative interchangeable terminal trait.

## Attachment protocol

Control traffic remains versioned JSON. High-volume terminal traffic uses a
length-prefixed binary frame with protocol version, stream ID, frame kind,
sequence number, and payload length.

```text
Client → daemon                    Daemon → client
Attach(run, last_seq, mode)        Attached(stream, lease, grid)
RequestLease                       Snapshot(snapshot_seq, bytes)
Input(bytes)                       Output(seq, bytes)
Resize(cols, rows)                 LeaseGranted / LeaseRevoked
Detach                             ResyncRequired
```

Invariants:

- PTY output is drained even when every client is slow or disconnected.
- Output frames for one run are applied in sequence or discarded and resynced.
- Only the lease holder may send input or authoritative resize events.
- Observers render the controller's logical grid and never resize the PTY.
- A Ghostty snapshot is accepted only when its pinned build ID matches.
- GUI and daemon upgrades are deferred while incompatible active runs exist.

## Safety and privacy

- Runtime directories are `0700`; sockets, logs, and snapshots are `0600`.
- Clients are checked against the daemon's operating-system user credentials.
- Each agent receives an unguessable per-run capability for the signal channel.
- OSC 52 clipboard reads and writes require application-level policy and visible
  confirmation when not initiated by the human.
- Paste safety checks happen before bytes reach the PTY.
- Recorded terminal output may contain secrets. Missions can be marked ephemeral,
  which retains only the in-memory state required for detach/reattach.
- V1 approval signals are cooperative. Enforced capabilities require a later
  sandbox/execution-broker module and use different interface language.

## Performance budgets

- Input-to-present latency: under one 60 Hz frame at p95 on a local idle system.
- PTY reader: never waits for rendering, disk snapshotting, or an attached client.
- Reattach: stage is usable within 250 ms for a run with 50,000 scrollback lines.
- Concurrency: 12 active runs, four visible surfaces, and multiple observers
  without output loss.
- Idle desktop and daemon: under 1% CPU after terminal activity settles.
- Rendering work is limited to damaged rows; occluded surfaces do no GPU work.

Budgets are release criteria, not architectural constants. Benchmark evidence
may adjust the exact numbers without changing the module interfaces.

## Delivery plan

### M0 — Ghostty and renderer risk spike

- Pin a Ghostty commit and Zig toolchain.
- Build static `libghostty-vt` on macOS and Linux CI.
- Wrap one terminal, feed a fixture, read render state, and round-trip a snapshot.
- Draw that fixture with wgpu on Metal, Vulkan, and Linux OpenGL fallback.

Exit: one semantic frame fixture passes identically on both operating systems,
and each GPU backend produces an approved screenshot.

### M1 — Real single-run multiplexer

- Add POSIX PTY ownership, process groups, resize, and device replies.
- Implement binary attach, output sequencing, snapshot resync, and a CLI client.
- Install the daemon independently of the desktop process.

Exit: `/bin/sh` can run, accept input, detach, and reconstruct the exact screen
after reattachment on macOS and Linux.

### M2 — Production terminal surface

- Add Unicode shaping, fallback, glyph atlas, ligatures, emoji, cursor, selection,
  links, mouse reporting, clipboard, search, and paste safety.
- Validate common full-screen programs and shell job control.

Exit: the terminal contract in the cross-platform architecture document passes
on Wayland, X11, and macOS.

### M3 — Multi-run control

- Add concurrent PTYs, write leases, observers, pinning, and four-surface stage.
- Prove slow observers cannot backpressure a run.
- Add process and resource summaries without polling terminal text.

Exit: 12 scripted runs execute concurrently while the human moves control among
four visible surfaces without output loss or focus theft.

### M4 — Agentic mission experience

- Build the Mission Spine, Attention Dock, Command Deck, delegation, structured
  signals, artifact review, and takeover/return flow.
- Add capability-authenticated agent SDK commands and unaware-process fallbacks.

Exit: one human can supervise a multi-agent coding mission, resolve every
attention item, take over a failed run, and trace each artifact to its actor.

### M5 — Release hardening

- Add launchd and systemd user installation, crash recovery, diagnostics, and
  version-aware upgrades.
- Package universal macOS builds and Linux Wayland/X11 distributions.
- Complete accessibility, keyboard-only, reduced-motion, and localization passes.

Exit: the same acceptance suite and scripted product journey pass from clean
installation on both platforms.

## Verification strategy

- Golden semantic-frame fixtures test `TerminalModel` independent of the GPU.
- Snapshot/property tests vary byte chunking, resize order, reconnect points, and
  output gaps.
- PTY integration tests run deterministic shell scripts and full-screen fixtures.
- Renderer tests use a bundled font and backend-specific screenshot baselines.
- Protocol tests inject partial frames, invalid lengths, stale sequences, slow
  readers, and incompatible build IDs.
- Product tests exercise keyboard-only mission creation, attention resolution,
  human takeover, detach, and reattach.
- CI runs Rust formatting, Clippy with warnings denied, tests, and the Ghostty
  build on macOS and Linux for every integration change.

## Cross-platform release matrix

| Area | macOS gate | Linux gate | Shared expectation |
|---|---|---|---|
| CPU and OS | Apple Silicon and Intel, macOS 14+ | x86-64, glibc 2.35+ | Same mission and terminal fixtures |
| Window/input | AppKit through winit | Wayland and X11 through winit | IME, scale changes, keyboard-only flow |
| GPU | Metal | Vulkan plus OpenGL fallback | Same terminal geometry and damage behavior |
| PTY | POSIX PTY and process groups | POSIX PTY and process groups | Shell job control, signals, resize, SSH |
| Clipboard | Standard clipboard | Standard and primary selection where available | Explicit OSC 52 policy and paste safety |
| Runtime | launchd user agent | systemd user service with foreground fallback | GUI exit does not end active runs |
| Package | signed/notarized universal app | AppImage and archive with user-service installer | Clean install, upgrade deferral, diagnostics |

The release journey runs `bash`, `zsh`, `fish`, SSH, Neovim, `htop`, `fzf`, and a
nested traditional multiplexer. Support means correct input, resize, alternate
screen, detach/reattach, selection, and clean process exit—not merely launching
the command.

## Explicit v1 non-goals

- Windows support;
- remote/network attachments or cloud synchronization;
- survival of child processes after daemon or operating-system failure;
- arbitrary persistent pane layouts;
- enforced agent sandbox permissions;
- Kitty graphics if the pinned Ghostty interface is not release-ready;
- plugin or marketplace systems.

These exclusions preserve the product thesis: first prove a correct,
cross-platform multiplexer with an agent-native attention and control model.
