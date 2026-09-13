# 01 — Product specification

## 1. Product definition — P-DEF-001

superplexr is an agent-native execution environment whose first interface is a
high-fidelity terminal multiplexer. It makes parallel work legible and
interruptible without making a terminal tab, pane, window, or GUI process the
owner of that work.

**Product promise:** a developer can delegate several pieces of work, understand
which ones need judgment, inspect the live terminal evidence, take control of
exactly one Session, close the application, return later, and reconstruct who
did what and why.

The differentiator is not GPU rendering or using Ghostty. It is the combination
of terminal correctness with a durable causal execution graph and structured
human attention.

## 2. Audience — P-AUDIENCE-001

The primary v1 user is a developer, technical founder, or operator working on one
machine and supervising between one and twelve concurrent local Sessions. They
are comfortable with terminals, use coding or operations agents, and need direct
control when automation reaches ambiguity or risk.

Secondary users are:

- developers using superplexr as a conventional durable terminal multiplexer;
- agent authors integrating through the local signal protocol;
- maintainers reviewing an execution history after work has completed.

V1 is local-first. An opt-in owner Remote attachment may forward the existing
local protocol through OpenSSH without exposing a TCP listener. A time-bounded
Observer Share may project explicitly scoped Missions and Sessions read-only and
is revocable without ending the owner's work. Controller/editor sharing,
browser clients, and mobile clients remain future products.

## 3. Jobs to be done — P-JOBS-001

### P-001 — Organize outcomes, not windows

The user MUST be able to organize work by Mission and see the Runs, Sessions,
Signals, and Artifacts that advance it. View arrangement MUST remain disposable.

### P-002 — Supervise parallel agency

The user MUST be able to operate twelve Sessions in one Mission while seeing a
quiet, prioritized account of which work needs attention. Output activity MUST
NOT steal focus or reorder Surfaces.

### P-003 — Intervene safely

The user MUST be able to inspect a structured request, answer or deny it, take a
Session's Control lease, and explicitly return control. A second writer MUST NOT
silently interleave input.

### P-004 — Preserve work across interface failure

Closing or crashing the desktop MUST NOT terminate the daemon, PTYs, processes,
Missions, Runs, or Sessions. Reattachment MUST reconstruct an exact accepted
terminal frame before applying live updates.

### P-005 — Explain the history

The user MUST be able to answer: what outcome was intended, which Actor attempted
what, which work depended on what, what evidence was produced, where a human
intervened, and how each attempt ended.

### P-006 — Remain an excellent terminal

An unaware shell, compiler, REPL, editor, or full-screen TUI MUST work without an
agent integration. Agent features MUST NOT alter its stdout/stderr byte stream.

## 4. Product principles — P-PRINCIPLES-001

1. **Durable work, disposable views.** Closing chrome never ends work.
2. **Structured truth, terminal evidence.** Signals express semantics; terminal
   output remains evidence and is never scraped as the source of truth.
3. **No invisible authority.** Control and Grants are explicit, scoped, and
   auditable.
4. **Attention without interruption.** Background work may signal urgency but
   cannot change current focus.
5. **Cross-platform parity at every milestone.** macOS and Linux are one product,
   not a primary build and a later port.
6. **Correctness before spectacle.** A beautiful waterfall cannot compensate for
   lost bytes, broken job control, incorrect reflow, or ambiguous recovery.
7. **Measured performance.** “Fast” means the budgets in section 08 pass.

## 5. V1 scope — P-SCOPE-001

### Required

- Native desktop application on macOS and x86-64 Linux.
- Browser-style Mission tabs.
- Per-Mission attention and Session sidebar.
- One-to-four-column responsive terminal waterfall.
- Local shell and command Sessions backed by POSIX PTYs.
- Canonical daemon-side `libghostty-vt` state.
- Multiple Surfaces per Session with one controller and observers.
- Detach, reattach, full-frame resynchronization, scrollback, selection, search,
  clipboard, OSC 8 links, IME, mouse modes, bracketed paste, alternate screen,
  Kitty keyboard input, Unicode, color emoji, and font fallback.
- Event-sourced Missions and a causal/dependency Run graph.
- Structured progress, input, approval, blocker, and artifact Signals.
- Human takeover and return.
- Local CLI and restricted agent protocol.
- Durable, Run-bound Git checkouts with exact-base provenance and conservative
  retirement that cannot discard dirty or unmerged work.
- Final Session snapshot and raw output history.
- Signed/notarized macOS package and reproducible Linux packages.

### Conditional

- Kitty graphics ships only if the pinned public `libghostty-vt` revision exposes
  a stable-enough snapshot/render path and it passes memory limits. Its absence
  does not block v1.

### Explicitly outside v1

- Windows, Linux ARM, mobile, and browser clients.
- Concurrent multi-controller editing, graph-editor roles, and shared accounts.
  Controller Shares intentionally grant one scoped, non-forced terminal Control
  lease; they do not grant graph mutation or process lifecycle authority.
- Cloud synchronization or hosted storage.
- Plugin marketplace.
- Arbitrary workflow language or visual graph editor.
- Guaranteed process survival after daemon or operating-system failure.
- Claims of enforced agent permission safety when the child process has the
  user's unrestricted OS privileges.

Owner Remote attachment is intentionally narrower than federation: one runtime
remains authoritative and the remote device is another transient trusted client.

## 6. Primary journey — P-JOURNEY-001

The v1 reference journey MUST complete without a terminal restart:

1. Create a Mission from the desktop.
2. Start three Runs, with two parallel and one dependent on both.
3. Give the parallel Runs isolated Sessions.
4. Watch both Surfaces without changing current keyboard focus.
5. Receive a structured question from one and an approval request from another.
6. Answer the question, inspect approval evidence, and deny or allow the action.
7. Take control of one Session, run a diagnostic, then return control.
8. Close all desktop windows while work continues.
9. Reopen, attach, and recover exact terminal state and graph state.
10. Observe the dependent Run become ready only after prerequisites succeed.
11. Inspect Artifacts and accept or reject finished Run results.
12. Complete the Mission and review its causal history.

The same journey MUST pass on macOS, Wayland, and X11.

## 7. Success measures — P-SUCCESS-001

The product is ready for a public v1 when:

- all release gates in section 08 pass;
- at least 20 dogfood Missions totaling 500 Session-hours complete with no lost
  PTY bytes or incorrect control transfer;
- at least 95% of structured attention items identify a Mission, Run, Actor,
  reason, and available action without requiring terminal-output interpretation;
- users can recover the focused Mission and Surface after a desktop restart in
  at least 99.9% of automated restart trials;
- the exact same terminal fixture corpus passes on macOS, Wayland, and X11;
- no UI text describes cooperative approval as enforced authorization.

Adoption, retention, and competitive marketing claims are deliberately not v1
engineering gates.

## 8. Product voice — P-VOICE-001

Interface language uses short, concrete verbs: `Create mission`, `Start run`,
`Take control`, `Return control`, `Terminate session`, `Allow once`, `Deny`, and
`Accept result`. Errors state what happened and the recovery action. The product
does not anthropomorphize failures, celebrate routine operations, or use “magic”
as an explanation.
