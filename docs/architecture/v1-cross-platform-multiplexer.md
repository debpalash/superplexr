# Cross-platform v1 multiplexer

The first release supports macOS and Linux from one Rust codebase. It is a real
terminal multiplexer: the runtime—not the GUI—owns child processes, PTYs,
terminal state, and durable output. The agentic mission model determines what the
human sees and where attention goes; it does not change terminal correctness.

## System shape

```text
┌────────────────── termi9ne desktop ──────────────────┐
│ GPUI                                                   │
│   Mission tabs · Session sidebar · terminal waterfall│
│                                                       │
│ Surfaces                                              │
│   SessionProjection → custom TerminalElement         │
└────────────────────────┬──────────────────────────────┘
                         │ binary terminal data + JSON control
┌────────────────────────▼──────────────────────────────┐
│ termi9ned                                              │
│                                                       │
│ MissionRuntime                                        │
│   event log · run graph · attention projection       │
│                                                       │
│ SessionSupervisor                                     │
│   process · PTY · Control lease · output sequence    │
│      │                                                │
│      └─ canonical libghostty-vt terminal model       │
│         render cache · output journal · device replies│
└─────────────────┬─────────────────────┬───────────────┘
                  │                     │
            shell/program         agent side-channel
```

## Platform contract

The shared application uses GPUI for windows, GPU rendering, text, input, IME,
focus, accessibility, display scale, and asynchronous work. Browser-style
Mission tabs, the per-Mission Session sidebar, and the waterfall are ordinary
GPUI views. A custom `TerminalElement` owns terminal-grid layout and painting so
cells are not represented as thousands of general-purpose view nodes.

GPUI is pre-1.0, so `termi9ne-desktop` pins an exact revision and is the only
module allowed to expose GPUI types. Core, protocol, PTY, and terminal modules
remain GUI-independent. Small platform adapters remain for notifications,
application menus, secure storage, service installation, and packaging.

Initial targets are Apple Silicon and Intel macOS, plus x86-64 Linux under both
Wayland and X11. Linux ARM can follow without changing the architecture.

## Ghostty seam

`libghostty-vt` is pinned to an exact upstream commit and built as a static
library. The first spike audits `libghostty-rs`, which already separates generated
raw bindings from safe Rust wrappers and includes `Terminal`, `RenderState`, key
and mouse encoders, and a Rust Ghostling port. If the audit passes, termi9ne pins
or forks that revision rather than recreating its unsafe work. The product seam
remains ours:

```text
libghostty-vt-sys / libghostty-vt   raw and safe upstream bindings
                  ↓
termi9ne-terminal                   terminal state and effects
                  ↓
termi9ned                           sole canonical consumer
```

The terminal module presents three operations:

```rust,ignore
let mut terminal = TerminalModel::new(size, config)?;
let effects = terminal.advance(TerminalAction::Output(bytes))?;
let update = terminal.frame()?;
```

`TerminalAction` also covers encoded key, mouse, paste, focus, and resize input.
`TerminalEffects` carries PTY replies, damaged rows, title changes, bells, and
other observable changes. `TerminalFrameUpdate` is either a backend-neutral full
frame or a sequenced set of row deltas. The interface hides allocation callbacks,
raw pointers, render-state lifetimes, and version-specific Ghostty types.

## PTY and attachment model

Every Session owns exactly one leader child process, one POSIX PTY, one logical
grid size, and one monotonically increasing output sequence. A Run may use a
Session but does not own it: ordinary shells may have no Run, headless Runs may
have no Session, and sequential Runs may reuse an idle Session. The daemon
continuously feeds Session PTY output into its canonical terminal model,
including while no GUI is open. This is necessary because terminal queries can
require immediate replies even when a Session is detached.

An attachment begins with a backend-neutral `FullFrame` containing the accepted
grid, cursor, colors, visible rows, and a bounded scrollback window. Live updates
are sequenced `RowDelta` batches produced from Ghostty dirty tracking and
coalesced at most once per display frame. A client that detects a sequence gap
requests another `FullFrame`; it never guesses or applies updates out of order.

These messages describe terminal semantics rather than GPU draw calls or
Ghostty structs. They use a length-prefixed binary data plane with bounded
queues. JSON remains the control plane for low-frequency commands and Mission
events. Every Surface attached to the same Session in one desktop process shares
one `SessionProjection`, so terminal updates are decoded once rather than once
per tile.

Each Session has one Control lease. The controlling Surface may send input and set
the PTY grid size; observer Surfaces are read-only and render the controller's
grid. When the human takes control of a Session used by an agent Run, transfer of
the Control lease is a domain event. When the human returns control, queued agent
input may resume.

Responsive waterfall geometry never resizes the PTY continuously. A controlling
Surface keeps its last grid while moving, waits 120 ms for geometry to settle,
then sends at most one whole-cell resize. Hidden Mission tabs and observer
Surfaces never issue authoritative resize events.

## Persistence guarantees

Closing every GUI window leaves `termi9ned`, its Sessions, PTYs, and child
processes alive. For each Session the daemon stores:

- periodic backend-neutral full frames and sequenced terminal deltas;
- raw output segments for audit, history, and diagnostic replay;
- process metadata and the last authoritative grid size.

Mission and Run events remain in the append-only domain event log, where they
reference Sessions by stable ID rather than embedding terminal state.

V1 guarantees survival across GUI restarts and client disconnects. It does not
promise child-process survival if the daemon or operating system itself crashes;
that requires a separate supervisor and is not implied by terminal snapshots.

## Agent protocol

Spawned agents receive the Mission ID, Run ID, optional Session ID, and signal
socket through environment variables. Questions, approvals, blockers, progress,
and artifacts travel over that side-channel. Stdout and stderr remain an
unmodified terminal byte stream, so ordinary shells and full-screen TUIs continue
to work.

One versioned transport carries two explicit planes. Interactive terminal frames,
deltas, input, and resize acknowledgements are binary and sequenced; control
commands and domain events remain JSON. Neither plane contains GPUI or Ghostty
types.
The Unix socket is user-only (`0600`) and lives under the platform runtime
directory rather than inside a repository in packaged builds.

## V1 terminal contract

The release gate includes:

- interactive shells and job control;
- resize and reflow;
- Unicode, fallback fonts, ligatures, and color emoji;
- alternate screen, mouse reporting, focus events, and bracketed paste;
- selection, clipboard, scrollback, search, and OSC 8 links;
- Kitty keyboard input;
- detach, reattach, observer mode, and human/agent control transfer;
- identical terminal fixture snapshots on macOS and Linux.

Kitty graphics can ship only if the upstream `libghostty-vt` image interface is
stable enough at the pinned revision; its absence does not block the first
release.

## Delivery order

The original completed foundation was protocol v2 with Session identity,
lifecycle, sequential Run assignment, and a Session-owned Control lease. The
current preview is version 15; the sequence below remains the release-order
record rather than a claim that only the foundation exists.

1. Audit and pin/fork `libghostty-rs`; prove one terminal fixture builds and runs
   on macOS and Linux CI.
2. Spike GPUI on macOS, Wayland, and X11 with custom terminal painting, IME,
   clipboard, focus, accessibility, and packaging smoke tests.
3. Implement `termi9ne-terminal` with golden VT fixtures, full frames, dirty-row
   deltas, input encoding, and PTY device replies.
4. Add POSIX PTY ownership and a single-Session binary attachment data plane to
   `termi9ned`, retaining JSON for control commands.
5. Build `SessionProjection` and the GPUI `TerminalElement`, then prove detach,
   reattach, observer mode, resynchronization, and stable-grid resize.
6. Build Mission tabs, the per-tab Session sidebar, responsive waterfall,
   structured attention, and human/agent control transfer.
7. Package both platforms and run the product and performance superiority gates.

The first vertical milestone is intentionally narrow: create a Session running
`/bin/sh`, render its prompt, type a command, detach the GUI, reattach, and
recover the exact screen on both macOS and Linux. Agent orchestration then builds
on a proven multiplexer rather than masking terminal defects.

## Upstream references

- [libghostty-vt overview and supported modules](https://github.com/ghostty-org/ghostty/blob/main/include/ghostty/vt.h)
- [Ghostty's static/shared libghostty-vt build](https://github.com/ghostty-org/ghostty/blob/main/CMakeLists.txt)
- [Why the private embedded runtime is not a Linux interface](https://github.com/ghostty-org/ghostty/discussions/11722)
- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
- [libghostty-rs](https://github.com/Uzaaft/libghostty-rs)
- [Ghostling](https://github.com/ghostty-org/ghostling)
- [Vanish's daemon and controller/viewer protocol](https://github.com/psyclyx/vanish/blob/main/DESIGN.md)
