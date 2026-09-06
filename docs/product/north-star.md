# The agentic multiplexer

ultraplexr is not a terminal application with agent features. It is a durable
execution environment in which humans and agents can run, observe, interrupt,
delegate, and review work. The terminal is the first high-fidelity Surface, not
the product's organizing model.

Our shorthand is:

> Superlogical multiplexes streams. ultraplexr multiplexes agency.

This is a product-design bar, not a claim we make before the evidence exists.
As of 2026-08-31, Superlogical's public plan describes durable sessions spanning
interactive and automatic work, structured data and actions, preserved history,
web and Apple clients, multiplayer sharing, composability, and production
safety. Merely combining Rust, GPUI, and libghostty does not exceed that plan.

## The durable object

The durable object in ultraplexr is the Mission: an intended outcome with actors,
Runs, Sessions, Signals, Interventions, and Artifacts. This creates relationships
that a stream-oriented multiplexer cannot express directly:

```text
Mission
├── Runs          attempts, actors, ancestry, disposition
├── Sessions      processes, PTYs, terminal state, control ownership
├── Signals       progress, questions, approvals, blockers
├── Interventions human decisions and direct-control periods
└── Artifacts     diffs, plans, logs, screenshots, reports
```

A Run may be headless. A Session may contain ordinary shell work. Several Runs
may use one Session sequentially. Several Surfaces may observe one Session, but
only the current controller may write or resize it. Views never own durable work.

## Superiority gates

We do not call ultraplexr superior until it demonstrates all applicable gates.

### Terminal foundation

- Native macOS and Linux clients ship together in v1.
- Interactive shells, job control, Unicode, IME, selection, search, scrollback,
  links, mouse reporting, bracketed paste, and Kitty keyboard input pass the
  same fixtures on both platforms.
- Sessions survive every GUI exit, crash, and upgrade that leaves the daemon
  alive.
- Attach restores an exact terminal frame before live output resumes.
- One controller and multiple observers remain deterministic under contention.

### Agent-native execution

- Agent progress, questions, approvals, blockers, and artifacts use structured
  Signals rather than terminal-output regexes as their source of truth.
- Every agent attempt is a Run with identity, actor, ancestry, lifecycle,
  disposition, and optional Session association.
- Agents can spawn child Runs, wait, send Signals, produce Artifacts, and hand
  control to a human through a documented local protocol.
- Human takeover is explicit, auditable, and reversible; no actor can silently
  steal a Session's Control lease.
- Risky operations carry enforceable Grants before the interface
  describes them as approved. Cooperative agent reports are labeled as such.

### Operator experience

- A browser-style Mission tab preserves its own Session sidebar and waterfall
  arrangement without owning any underlying lifetime.
- Twelve concurrent Sessions remain triageable without animated output previews
  or focus stealing.
- Attention identifies the Mission, Run, Session, reason, evidence, and available
  decision—not merely that a terminal emitted a bell.
- The same work is operable through desktop, CLI, and agent interfaces.
- Mission history answers who acted, why, with what authority, and what changed.

### Performance

- Normal input reaches pixels within one display frame at the 95th percentile.
- A clean Surface causes no terminal-grid work and negligible idle CPU use.
- PTY output is parsed once per Session in the daemon.
- Terminal transport is binary, sequenced, bounded, and recoverable after a
  dropped or lagging client.
- Multiple Surfaces for one Session share one client projection and glyph cache.
- Waterfall animation never emits intermediate PTY sizes.

Performance numbers are release-test results, not marketing adjectives.

## V1 claim

V1 aims to be the strongest native agentic multiplexer on macOS and Linux. It
does not claim broad product superiority while web access, multiplayer control,
remote-host operation, and production policy enforcement remain unshipped.

The v1 proof is one complete journey:

1. Create a Mission and delegate parallel Runs into isolated Sessions.
2. Watch their terminal Surfaces in a responsive waterfall.
3. Receive structured attention without losing current focus.
4. Inspect evidence, approve or deny an operation, and take control if needed.
5. Close the desktop, reconnect, and recover the exact work.
6. Review the causal Run history and resulting Artifacts.
7. Complete the same journey on macOS, Wayland, and X11.

## What will not differentiate us

- Using libghostty or rendering on the GPU.
- Adding tabs, splits, badges, notifications, or a command palette.
- Detecting agent prompts only by matching terminal text.
- Calling a collection of PTYs an agent orchestration system.
- Shipping an attractive interface without a durable execution model.

Those are expected capabilities. The differentiator is making parallel agency
legible, interruptible, auditable, and composable without weakening terminal
correctness.

## Public research baseline

- [Superlogical's public product plan](https://www.superlogical.com/)
- [Ghostling, the minimal libghostty consumer](https://github.com/ghostty-org/ghostling)
- [libghostty-rs](https://github.com/Uzaaft/libghostty-rs)
- [Vanish session architecture](https://github.com/psyclyx/vanish/blob/main/DESIGN.md)
- [Zmx persistent sessions](https://github.com/neurosnap/zmx)
- [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
