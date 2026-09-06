# Ultraplexr: universal runtime and clients

Date: 2026-09-05  
Status: architecture proposal and implementation sequence  
Scope: the broader Ultraplexr product direction; repository packages currently use `ultraplexr`.

## Goal and relationship to v1

Build one coherent environment for agent development across desktop, terminal,
browser, embedded interfaces, and remote execution hosts. It should support
multiplayer observation and control, streaming, recovery, secure collaboration,
and automation, with a small baseline and optional capabilities.

One product means consistent identities, commands, permissions, and work history
across clients. It does not require one executable, programming language, or UI
implementation for every environment.

This proposal records the product direction discussed with the owner. It does
not claim these capabilities are shipped or silently expand the current
[macOS/Linux v1 contract](../spec/README.md). Platform and protocol changes need
corresponding specification updates and acceptance evidence as they are
implemented. The [unified roadmap](../ultraplexr-unified-roadmap.md) remains the
status ledger. Existing ADRs govern implemented behavior until explicitly
superseded.

Use the existing [domain glossary](../../CONTEXT.md): Missions organize outcomes;
Runs are attempts; Sessions own process environments; Session groups organize
Sessions; Surfaces display terminal state. A presentation workspace or tab is
not another Mission or a second workflow system.

## Stack direction

### Implemented foundation (2026-09-05)

- Desktop view dismissals are persisted explicitly. Closing the last tab for a
  Mission hides it on this device without stopping its Sessions; new Missions
  remain discoverable. Restore Closed Workspace can reopen dismissed Missions
  after restart (restored tabs then use the Mission title/default layout).
- Session-group reconciliation runs through a window-independent module and
  updates duplicate views, rejects older revisions, and preserves generated
  labels on unrelated pin/order updates. Explicit renames stop automatic labels.
- The blocking client accepts a reconnectable transport adapter. Wire sequencing,
  Share authorization, and subscriptions stay common. The built-in adapter is
  Unix sockets; a real-daemon integration test also uses a loopback byte relay.
  This is not a Windows/WASM port or an authenticated TCP server.
- The optional `ultraplexr-observer` executable serves a localhost-only browser
  observer with scoped terminal listing, selectable/copyable text, pause/resume
  of local updates, retained history, and event-driven SSE snapshots with scoped
  reconnect. It uses no frontend framework or build
  pipeline. See [the observer guide](../design/browser-observer-prototype.md).
- The optional `ultraplexr-tui` attaches to one existing Session using the same
  client. It renders styled canonical cells, supports non-forced Control,
  guarded paste, controlled resize, local pause/history and explicit clipboard
  export. Real-PTY tests exercise shell/vi, detach, cleanup and Share revocation;
  forced native-link loss repairs the view without resuming input automatically.
  See [the focused-session TUI guide](../design/focused-session-tui.md).
- The TUI now browses scoped Missions, Sessions and unresolved Attention, with
  filter/refresh and explicit observation of a Run's primary Session. Two-pane
  side-by-side/stacked views share the same runtime; focus changes return Control,
  pane closure never kills a Session, and compact layouts show only the active pane.

These are development implementations, not a claim of complete platform parity.
The observer projects native frame events into canonical text, coalescing frames
under browser backpressure. Only session-list metadata still polls (every two
seconds). It does not yet render styled VT cells/deltas, replay every
intermediate screen, embed in third-party origins, or expose a public TLS gateway.
It is a proof of shared runtime access, not the finished browser client below.
[Local restart/resource/latency measurements](../engineering/observer-streaming-baseline.md)
are available; no cross-platform RSS/CPU/latency budget is certified yet.

The browser proof now also has opt-in Controller Share support: each live HTTP
attachment owns one runtime Surface, explicit non-forced Control, ordered
lease-bound input, paste confirmation, resize, and disconnect cleanup. Default
mode remains read-only. This closes the first browser input seam, not the public
gateway, complete terminal rendering, or platform acceptance milestones.

### Longer-term direction

Retain Rust, the durable runtime, and the Ghostty VT boundary. Extract shared
behavior from clients before adding more interfaces. Rewriting the server in
Go would not resolve ambiguous lifecycle ownership or conflicting view state.

| Layer | Proposed choice | Role and current qualification |
| --- | --- | --- |
| Domain core | Rust library | Existing Mission graph, identities, lifecycle, and authorization rules; independent of UI and transport. |
| Execution runtime | Rust and Tokio | Host execution, scheduling, subscriptions, persistence, and resource limits; reduce unconditional optional dependencies. |
| Terminal engine | Ghostty VT behind the existing Rust abstraction | Canonical terminal interpretation; keep engine-specific types behind the adapter. |
| Platform execution | `portable-pty` plus OS adapters | PTYs, signals, process groups, credentials, and Windows process handling; existing Unix assumptions still need separation. |
| Local transport | Unix sockets and proposed Windows named pipes | Local host communication with OS-appropriate authentication. |
| Owner remote attachment | Existing OpenSSH forwarding | Preserve the accepted first remote path and its owner-only semantics. |
| Browser transport | Proposed HTTPS/WebSocket gateway | Authenticated browser access to the same commands and streams. |
| Optional peer connectivity | Experimental Tailcat adapter | Evaluate a supervised Go companion for NAT traversal and relay fallback. |
| Desktop | Existing GPUI client | Continue macOS/Linux work; evaluate platform integration and upstream maintenance before expanding support. |
| Browser and web embed | TypeScript, accessible HTML/CSS, isolated terminal renderer | Proposed browser client and embeddable component; no duplicate workflow authority. |
| CLI and TUI | Existing Rust CLI; proposed Rust TUI | Scriptable control and interactive operation through the shared client API. |
| SDKs and automation | Versioned schemas, bindings, CLI, and MCP adapters | Same authorization and lifecycle semantics as human clients; SDK generation remains proposed. |
| Extensions | Capability-scoped subprocesses | Build on existing supervised plugins; add heavier execution engines only when a use case justifies their cost. |

The [GPUI ADR](../adr/0006-use-gpui-for-the-desktop.md) already excludes GPUI
from core, protocol, and PTY types. Preserve that boundary and evaluate GPUI
against input, accessibility, packaging, maintenance, and platform evidence.
Its retention is not a commitment to implement every future client in GPUI.

## Runtime and client boundaries

```text
Desktop      Browser / embed      Terminal UI      CLI / agents
    \               |                 |                 /
             Versioned commands and subscriptions
                              |
             Authenticated, authorized connection
                              |
                 Authoritative host runtime
          Missions / Runs / Sessions / Control leases
                    /                    \
           Durable state           Execution adapters
           and Artifacts          Local OS / sandbox
```

Each Session has one authoritative execution host. Several clients can attach
without mirroring the process or creating a second Session. Connecting several
hosts is initially discovery and routing; automatic process migration and
cross-host transactions are separate future designs.

Shared state belongs to the runtime: Session lifecycle, Session group identity
and membership, Control leases, Shares, workflow state, and durable history.
Personal view state belongs to a person/device: open tabs, hidden items, focus,
layout, scroll position, and selection. Syncing personal views between devices
can be optional and must preserve that ownership distinction.

The recent restore failure illustrates why this matters: absence from one
desktop document cannot distinguish a deliberately hidden Mission from a newly
discovered Mission. Persist explicit dismissal state at the appropriate view
scope. New work can then appear without resurrecting intentionally hidden work.

Duplicating a view should retain explicit references to shared Sessions/groups,
and every duplicate must consume their updates. Creating independent processes
is a separate operation. Closing a view, hiding a row, detaching a group,
archiving history, and terminating a process need distinct effects across all
clients. Preserve the [Session group decision](../adr/0021-group-sessions-without-changing-process-identity.md).

Embedding also needs distinct support targets:

- An embedded Linux machine may host execution if its OS and resources support it.
- A browser, mobile interface, editor integration, or terminal can control a remote host.
- An embedded SDK can expose commands and events without rendering a terminal.
- Restricted or very small devices may only support a client or integration endpoint.

No browser or constrained-device claim implies the ability to create arbitrary
host processes. A native SDK/FFI boundary is future work, not guaranteed merely
by implementing the core in Rust.

## Do we need a Herdr-style TUI?

Yes, for the stated product scope. A first-class terminal client makes
interactive supervision possible over SSH, on machines without a GUI, and from
an existing terminal emulator. A CLI supports scripting; a TUI adds persistent
navigation, attention triage, and interactive Session access. Neither requires
a desktop or browser to be installed on the execution host.

Herdr's documented workflow validates this use case: project organization,
agent state, mouse/keyboard navigation, and detach/reattach from a terminal.
Use that as an interaction benchmark, while preserving Ultraplexr's Mission,
Run, Session, and evidence model. [Herdr quick start](https://herdr.dev/docs/quick-start/).

The TUI should be an optional client of the same runtime. It should not own a
separate session database, scheduler, permission system, or provider-status
heuristics. Its presence should add no background work to a headless host when
no terminal client is attached.

Recommended initial scope:

1. Browse Missions, Session groups, Sessions, activity, and unresolved Attention.
2. Attach to one Session, observe it, and explicitly acquire or return Control.
3. Read history, search, and copy through supported terminal capabilities.
4. Navigate by keyboard, with mouse support where the outer terminal permits it.
5. Detach and reconnect without affecting process lifetime or other clients.

Follow with split terminal Surfaces, group organization, and evidence review
once the focused-session path is correct. The eventual terminal multiplexer
needs pane layouts; the first proof does not need desktop feature parity.

[Ratatui](https://ratatui.rs/) is a candidate for navigation and interface chrome,
not a selected terminal emulator. Prototype a terminal Surface consuming our
canonical frames. A UI toolkit does not solve terminal key encoding, mouse
reporting, alternate screens, wide/combining characters, resize ownership, or
clipboard limitations. Explicitly test escape/prefix handling and restoring the
outer terminal after exit or failure. Never let client rendering escape
sequences become unauthorized clipboard or terminal-control operations.

Build a minimal browser observer first to expose transport assumptions; then
add the focused-session TUI on the same client boundary. Both remain small
proofs before broader client polish. If SSH-only operation becomes the first
delivery target, the two proofs can exchange order without changing the stack.

Both proof clients now exist on the development branch. The TUI uses Crossterm
directly for input, a lightweight Mission/Session/Attention navigator, two-pane
layouts and our canonical frame renderer. Ratatui remains an optional future UI
choice, not a required terminal emulator. Search UI, structured Attention responses,
mouse forwarding and richer split/layout management remain separate deliverables.

## Streaming, recovery, and multiplayer

Retain versioned control messages and the existing binary terminal-frame seam.
Transport independence does not require replacing the existing protocol.
Browser and TUI clients need compatible decoding and rendering adapters; avoid
independently parsing divergent output histories without conformance evidence.

Connections need capability negotiation, bounded messages, ordering, stream
identity, and a consistent snapshot-to-live transition. Reconnect must detect
missing or expired history and resnapshot rather than silently dropping updates.
Use idempotency identities and acknowledgement rules for commands: blindly
replaying terminal input after reconnect could execute it twice.

Multiple observers may watch one Session. Only the current Control lease holder
may input or resize it. A stale client must be rejected after handoff; observer
viewport changes must not resize the shared PTY. Slow clients need bounded
queues and repair paths so they cannot stall execution or other clients.

Keep recovery claims precise:

| Event | Required behavior |
| --- | --- |
| Client disconnect or GUI exit | Runtime-owned processes continue; clients reattach to current state. |
| Missing stream updates | Replay retained updates or obtain a consistent new snapshot. |
| Runtime crash or host reboot | Recover committed history; explicitly report lost processes. |
| Agent process failure | Provider resume or a new Run with defined checkpoint/context inputs. |

Terminal journals do not preserve a live process across host loss, and a
passing reconnection test does not establish agent or workflow resumption.
Concurrent shared text editing would need an additional collaboration model;
terminal observation and Control leases do not implement it.

## Secure remote access and Tailcat

Preserve [OpenSSH owner attachment](../adr/0014-forward-the-local-protocol-through-openssh-for-owner-remote-attachments.md).
A browser gateway or Tailcat bridge cannot simply inherit the bridge process's
local owner privileges for every remote caller. Bind each connection to an
authorized identity and enforce resource/action scope at the runtime boundary.
Keep Share revocation, expiry, stream shutdown, and Control release effective
over every transport. Browser embedding additionally needs explicit origin and
parent-frame messaging policies and isolation of connection credentials.

Tailcat is a candidate connectivity module. It supplies WireGuard encryption,
NAT traversal, and relay fallback without tailnet membership. It does not supply
our work-state synchronization or application authorization. The initial Rust
integration can supervise a Go companion behind a byte-stream adapter, subject
to measured packaging and memory overhead. [Tailcat overview](https://tailscale.com/blog/tailcat).

Its browser demo is experimental and currently relay-only. Its API, CLI, and
wire format have no stability guarantee, and public relays are rate-limited
without service guarantees. These constrain production adoption and throughput
claims. [Tailcat README](https://github.com/tailscale/tailcat/blob/main/README.md).

Tailcat's security model also describes an experimental wrapper originally
designed for use by the same person at both ends. Start evaluation between
trusted machines and do not treat encryption as proof of safe collaboration
between mutually untrusting parties. [Tailcat security model](https://github.com/tailscale/tailcat/blob/main/SECURITY.md).

## Footprint and optional modules

Support a small number of tested build profiles rather than every possible
feature-flag combination. These are proposed packaging boundaries:

| Profile | Included capabilities |
| --- | --- |
| Minimal host | Domain/runtime essentials, local API, execution and terminal support. |
| Remote host | Minimal host plus a selected remote transport. |
| Desktop installation | Desktop client and convenient local-host startup. |
| Terminal client | CLI/TUI and client protocol; no mandatory local execution runtime. |
| Web gateway | Browser assets, authenticated connection handling, and runtime forwarding. |
| Automation host | Host plus selected workflow/provider extensions. |

Compile-time features should remove optional dependencies from minimal builds.
Runtime switches should stop unused listeners, subscriptions, tasks, and polling.
Separate processes can contain extension failures but have a memory/startup
cost. The always-required trust and lifecycle checks cannot become optional
security downgrades. The domain library should be reusable without a renderer,
network listener, or PTY service.

Measure release artifacts for each supported profile: installed and compressed
size, startup time, idle CPU/RSS, incremental memory per Session/viewer, terminal
throughput, input-to-display latency, reconnect time, wire bandwidth, and retained
history growth. Report workload, hardware, percentile, and direct/relay path.
Count browser, gateway, and companion-process costs explicitly; report agent
subprocess consumption separately from Ultraplexr overhead.

The current [idle benchmark](../../ci/desktop-idle-benchmark.sh) allows 600 MiB
combined desktop/runtime RSS. That is a configured ceiling, not a measurement
or evidence for a tiny host. Establish separate host/client budgets before
claiming minimal-resource suitability. Bound queues, history, decompression,
viewer fan-out, and workflow concurrency; only render presented Surfaces and
avoid continuous polling when events are available.

For storage, define atomic domain-change and recovery boundaries, snapshots,
and bounded terminal/Artifact retention. Audit existing journals against those
requirements before choosing a replacement database. No storage migration or
automatic deletion of existing work is authorized by this proposal.

## Implementation sequence and proof

| Stage | Deliverable | Acceptance evidence |
| --- | --- | --- |
| 1. State authority | Explicit per-view dismissal and consistent shared group updates. | Remove/hide, save, restart, and discover new work against real runtime state; duplicate views converge. |
| 2. Client boundary | Transport-independent client commands/subscriptions; minimal host build. | Headless launch without GPUI; existing local and OpenSSH tests remain valid. |
| 3. Browser proof | One Session observed from desktop and browser with authorized input. | Matching snapshots, handoff, stale-input rejection, dropped-link repair, and live Share revocation. |
| 4. Terminal proof | Focused-session TUI and Attention navigation. | Shell/editor/agent input, resize, copy, terminal cleanup, and detach/reattach without process loss. |
| 5. Platform proof | Windows execution/local transport and tested Unix hosts. | Equivalent lifecycle/control tests and release resource measurements on each supported target. |
| 6. Peer transport proof | Optional Tailcat companion. | Two-machine direct/relay tests, reconnect, authorization, and comparison with baseline resource/latency measurements. |
| 7. Expansion | Multi-pane TUI, embeddable web UI, SDKs, broader workflows. | Each interface uses the same lifecycle, permissions, evidence, and recovery contracts. |

The next implementation milestone is a dependable Session shared across clients,
with explicit control and recovery. The TUI is part of the target product and
gets a concrete delivery stage; it is not a reason to build a second runtime.
