# 08 — Quality and release specification

## 1. Performance budgets — Q-PERF-000

Measurements use release builds, bundled fonts, default configuration, and the
reference workloads below. Debug results are never used for claims.

### Q-PERF-001 — Latency

| Measure | Budget |
|---|---:|
| key event received to PTY write, p95 | <= 4 ms |
| PTY byte received to delta queued, p95 | <= 8 ms |
| local key event to visible pixel, p95 at 60 Hz | <= 16.7 ms |
| settled layout to resize accepted, excluding 120 ms debounce, p95 | <= 12 ms |
| attach request to first exact FullFrame, warm p95 | <= 150 ms |
| attention Signal commit to visible badge, p95 | <= 100 ms |

Input-to-pixel is measured with instrumentation spanning desktop event receipt,
protocol, PTY echo, terminal parse, projection, and presented frame. Hardware
timestamps are used when available.

The macOS native-input smoke MUST additionally post platform keyboard events to
an isolated desktop, observe their text in the canonical frame, and prove the
desktop retains its Mission and terminal-index streams on the multiplexed wire.

### Q-PERF-002 — Throughput

- One Session MUST parse and journal at least 50 MiB/s sustained synthetic output
  without byte loss on reference hardware.
- Twelve Sessions producing 1 MiB/s aggregate MUST keep control requests within
  latency budgets.
- Six visible Surfaces across twelve active Sessions MUST sustain 60 presented
  frames per second during the standard animated-output workload.
- A lagging client MUST resynchronize without reducing canonical parse throughput
  by more than 5%.

### Q-PERF-003 — Idle and memory

- Runtime with twelve quiet Sessions: <= 0.5% of one CPU core averaged over five
  minutes.
- Visible desktop with clean frames: <= 1% of one CPU core, excluding compositor.
- Desktop startup to interactive warm window: <= 700 ms p95.
- Runtime startup with 100 completed Missions: <= 500 ms p95.
- Combined RSS for runtime plus desktop with twelve Sessions and 100,000 retained
  lines each: <= 600 MiB, excluding Kitty image payloads.
- A clean terminal frame performs zero row translation and zero glyph upload.
- Publishing one semantic plugin event MUST be a non-blocking bounded enqueue;
  a full or disconnected plugin queue MUST return without waiting.
- With no installed executable plugins, the runtime MUST start no plugin worker
  thread or child process and retain only one disabled publisher handle.

Reference hardware is one supported Apple Silicon laptop and one four-core
x86-64 Linux machine with integrated graphics, both at least four years old when
the baseline is frozen. CI records exact models and never moves the baseline
without a specification change.

## 2. Correctness strategy — Q-TEST-001

### Unit and property tests

- Domain command/event examples and generated command sequences prove every
  D-INV requirement.
- Cycle detection generates lineage forests and dependency DAG mutations.
- Terminal translation tests use upstream and termi9ne golden VT fixtures.
- Protocol codecs round-trip valid values and reject every invalid bound.
- Layout property tests preserve stable order, valid spans, and one settled
  resize across arbitrary window sequences.

### Integration tests

- Runtime + real PTY + `/bin/sh` create/type/resize/exit.
- Mission commit + crash injection at every storage step + recovery.
- Attach snapshot race while output continues.
- Sequence gap, slow client, disconnect, and resynchronization.
- Control contention among agent, human controller, and observers.
- Agent capability attempts every forbidden method.
- Daemon remains alive when a Session/parser/client task fails.
- A real executable Plugin must handshake, receive a capability-filtered event,
  publish a bounded status, crash, restart with backoff, and shut down without
  delaying a terminal control request.

### End-to-end tests

- The product journey in section 01.
- Keyboard-only and screen-reader journeys.
- Close/crash/reopen desktop under high output.
- Packaging install, first launch, upgrade, rollback refusal, and uninstall that
  preserves user state unless explicitly selected.

### Fuzz and soak

- Continuous fuzz targets: Ghostty adapter inputs/effects, protocol frame decoder,
  terminal protobuf decoder, event envelope recovery, graph commands, and
  scrollback/history ranges.
- Nightly 12-hour soak: twelve Sessions, randomized output/resize/attach/control.
- Weekly 72-hour soak: 64 Sessions with bounded churn and repeated desktop crash.
- Any lost PTY byte, divergent frame digest, invariant violation, unbounded growth,
  or daemon abort fails the run.

## 3. Cross-platform CI matrix — Q-CI-001

Every merge runs formatting, Clippy with warnings denied, unit/property tests,
protocol fixtures, and domain migration tests on macOS and Linux.

Release matrix:

| Platform | Architecture/backend | Required suites |
|---|---|---|
| macOS | Apple Silicon / Metal | all, VoiceOver, package/sign/update |
| macOS | Intel / Metal | terminal, protocol, smoke, package |
| Linux | x86-64 / Wayland | all, Orca, AppImage/deb |
| Linux | x86-64 / X11 | all, Orca, AppImage/deb |

Headless virtual displays are permitted for most UI tests, but final keyboard,
IME, accessibility, compositor, and presentation-timing gates run on physical or
hardware-accelerated hosts.

## 4. Terminal compatibility gate — Q-TERMINAL-001

Release requires:

- terminfo bundled and discoverable;
- interactive shell/job-control suite;
- Unicode/emoji/combining/wide-cell corpus;
- 24-bit/palette/style/underline/hyperlink suite;
- reflow and alternate-screen suite;
- mouse/focus/bracketed-paste/Kitty keyboard suite;
- Neovim, shell editor, pager, and full-screen monitor journeys;
- selection, clipboard safety, search, and scrollback;
- identical semantic frame digests across macOS and Linux;
- pixel tolerances documented for expected rasterizer differences.

A terminal regression blocks release even when it occurs only on one window
backend.

## 5. Accessibility gate — Q-A11Y-001

Release requires:

- every action reachable through keyboard;
- visible focus and no focus loss through sheets/reconnect;
- WCAG AA contrast for application chrome;
- 200% scale and OS reduced-motion behavior;
- meaningful Mission/Session/Surface/attention names;
- no color-only status;
- VoiceOver and Orca completion of the reference journey;
- no uncontrolled live-region announcements from terminal output.

Known screen-reader defects cannot be reclassified as cosmetic.

## 6. Security gate — Q-SECURITY-001

Release requires:

- socket permission and peer-credential tests on both OSes;
- complete method authorization matrix tests;
- Run token expiry/revocation/delegation tests on the restricted agent channel;
- terminal escape and protocol fuzz corpora with no crash or unbounded allocation;
- OSC clipboard/link/paste confirmation tests;
- crash/diagnostic redaction verification;
- dependency vulnerability and license review;
- signed packages, SBOM, locked sources, and network-free release build;
- threat model reviewed against shipped features.

Any interface claim of enforcement must have an adversarial process-level bypass
test. The cooperative profile MUST pass copy review proving it makes no
containment claim.

## 7. Reliability gate — Q-RELIABILITY-001

Release requires:

- 10,000 randomized desktop disconnect/reconnect cycles with exact frame recovery;
- crash injection through every Mission commit and snapshot step;
- disk-full behavior with no acknowledged event or PTY-output loss;
- slow-client isolation and bounded queues;
- deterministic restore of 100 fixture Session histories;
- correct Lost state after forced daemon death;
- 72-hour soak with no daemon crash, invariant violation, or unbounded memory;
- migration from the latest public pre-v1 state and safe rejection of downgrade.

## 8. Product acceptance matrix — Q-TRACE-001

| Requirement | Evidence |
|---|---|
| P-001 organize by Mission | tabs/sidebar/graph E2E |
| P-002 supervise twelve Sessions | triage and performance workload |
| P-003 intervene safely | control contention E2E and event audit |
| P-004 preserve across GUI failure | 10,000 reconnect cycle suite |
| P-005 explain history | deterministic history acceptance test |
| P-006 excellent unaware terminal | terminal compatibility corpus |
| D-GRAPH-001..005 | graph property tests and scheduler integration |
| D-INV-001..012 | generated command/event/rehydration tests |
| A-SYS-001..003 | process ownership and dependency checks |
| A-PLATFORM-001..A-DEPENDENCY-001 | architecture checks and vertical-slice suites |
| T-OWN-001..T-COMPAT-001 | terminal semantic, PTY, renderer, and recovery corpus |
| W-GOAL-001..W-COMPAT-001 | wire golden fixtures, fuzz, load, and migration tests |
| C-TYPE-001..C-COMPAT-001 | golden control/event catalog and authorization fixtures |
| UX-THESIS-001..UX-PERSIST-001 | GPUI interaction, visual, keyboard, and accessibility E2E |
| S-AUTH-001..003 | platform authorization tests |
| S-TRUST-001..S-SUPPLY-001 | threat-model, adversarial, privacy, and supply-chain gates |
| R-STORE-001..003 | crash-injection recovery suite |
| R-SESSION-001..R-OBSERVE-001 | recovery, retention, limit, and diagnostic suites |
| Q-PERF-001..003 | signed benchmark report on reference hosts |

Each release candidate publishes a machine-readable report linking requirement
ID, test identifier, commit, platform, result, and artifact digest.

## 9. Release stages — Q-STAGE-001

### Developer preview

One real Session on macOS and Linux, no durability promise beyond explicitly
passing tests. State format may reset with prominent notice.

### Alpha

Multi-Session desktop, Mission graph, detach/reattach, and structured Signals.
State migrations begin. Known terminal gaps are listed in-app.

### Beta

Feature complete; state and protocol compatibility policy active; packages
signed; performance and accessibility budgets passing. Only fixes and measured
optimizations enter.

### V1

All gates in this section pass with no critical/high known security defect, no
data-loss defect, and no platform-specific terminal blocker.

## 10. Packaging and update acceptance — Q-PACKAGE-001

macOS package is signed, hardened, notarized, supports both required architectures,
installs terminfo, and starts the per-user runtime without requesting root. Linux
AppImage and `.deb` include required libraries or declare exact dependencies,
install terminfo without overwriting another package, and work under Wayland and
X11 without root at runtime.

Package install/uninstall MUST distinguish executables from user state. Uninstall
preserves Missions by default. `Delete all termi9ne data` is a separate explicit
action with path preview.

Updates are atomic. A runtime binary is never replaced underneath active PTYs.
Migration is rehearsed on a copy, and failure leaves the existing release and
state usable.

## 11. Definition of done — Q-DONE-001

A feature is done only when:

- its behavior and failure behavior match a requirement;
- tests cross the same module interface as production callers;
- macOS and Linux behavior is covered;
- accessibility and keyboard behavior are implemented;
- limits and observability are present;
- persistence/protocol compatibility is addressed;
- documentation and requirement traceability are updated;
- no TODO stands in for correctness or security.
