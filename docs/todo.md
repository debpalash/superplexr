# TODO

Updated: 2026-09-03. Companion to
[termi9ne-unified-roadmap.md](termi9ne-unified-roadmap.md), which stays the
authoritative capability catalog. This file is the short, ordered list.

Legend: `[x]` done and verified · `[~]` partial · `[ ]` not started

---

## 1. Fault / debugging loop

The differentiator: agents write code, termi9ne owns what happens when it
breaks. A Fault closes only when a replay actually passes.

- [x] `FaultId`, protocol types, 6 requests
- [x] Durable owner-only store, atomic writes, open Faults never evicted
- [x] Replay executor (bounded, off the async path, receipts)
- [x] Resolve refused without a passing replay; dismiss needs a note
- [x] Auto-detect: terminal session ends in failure
- [x] Auto-detect: per-command via OSC 133 marks
- [x] CLI: `report list show repro resolve dismiss handoff`
- [x] Desktop: sidebar `△ Broken` card + panel (⌘⇧F)
- [x] MCP bridge (`termi9ne-mcp`), Fault tools only
- [x] Live end-to-end proof on a real daemon and PTY

Remaining:

- [ ] **Bisect a Fault across Candidates** — find which Run introduced it.
      Needs Candidate patch history, not the Fault store.
- [ ] **Flaky vs real** — re-run N× in isolated worktrees, classify, record
      the distribution rather than one verdict. **Next.**
- [x] **Fault → Run** — `fault fix` plans a Run with the Fault as its
      objective, links the two, and launches the configured engine driver;
      verified that a Run claiming success still cannot close the Fault
- [x] **Regression guard** — `fault guard` re-replays resolved Faults and
      reopens any that fail again, keeping the replay that closed them so the
      regression can be read against it; verified end to end on a real daemon
- [x] Shell integration installer (`termi9ne shell-init`) for zsh, bash and
      fish; verified end to end against a real interactive zsh
- [ ] Mobile/web triage surface: read a Fault, replay, hand off. No terminal.

---

## 2. Verified delivery (roadmap Gates A–B)

Assessment below is from the roadmap document and a Codex analysis session,
not independently verified here.

**Gate A — authorization to effect** (mostly implemented)

- [ ] Generated-file provenance classification
- [ ] External-effect adapter fencing
- [ ] Concurrent-write, symlink, and path-alias adversarial fixtures
- [ ] macOS + Linux evidence for the above

**Gate B — verification, return, landing**

- [ ] Execute the normalized review contract in verifier Runs
- [ ] Materialize real test / impact / install / provenance Artifacts
- [ ] Issue the resulting EvaluationReceipt
- [ ] Inline review comments bundled into one structured Return
- [ ] Retry resubmission
- [ ] Candidate comparison (side-by-side)
- [ ] Previewed landing / merge, never forced

---

## 3. Supervision at scale (Gate C)

- [ ] Provider adapter kit + conformance fixtures
- [ ] `status explain` surface and adapter capability matrix
- [ ] Event-sourced Run mailbox
- [ ] Typed cross-agent references
- [ ] Deterministic Supervisor projection and Problems view
- [ ] Hierarchical budgets (token, cost, time, retry)
- [ ] Bounded goal continuation
- [ ] Heartbeats and orphan lease reconciliation

---

## 4. Terminal and desktop certification (Gate E)

- [ ] Unicode / IME / Kitty keyboard / Neovim compatibility matrix
- [ ] Six visible surfaces at 60 FPS, measured
- [ ] Reconnect, crash, disk-full, and long-soak evidence
- [ ] Wayland and X11 passes on physical Linux
- [ ] 200% zoom visual and accessibility pass
- [ ] Linux keyboard-shortcut smoke test
- [~] Resizable sidebar and terminal splitters (built; needs platform pass)
- [ ] OSC 133 command blocks in the UI: jump, search, copy, rerun

---

## 5. Recovery, context, supply chain (Gate D)

- [ ] RecoveryManifest; reconcile external effects before resuming
- [ ] ContextReceipts with source trust and model-facing role
- [ ] Evidence-preserving compaction (cite ranges, never delete evidence)
- [ ] First-seen executable admission, digest-bound
- [ ] Cumulative pre-effect policy gate
- [ ] Owner event export, scrubbed and versioned

---

## 6. Distribution (Gate F)

- [ ] Protocol and state migration policy
- [ ] Network-free locked release builds, SBOM, license notices
- [ ] AppImage / .deb, macOS Intel + Apple Silicon
- [ ] Signing and notarization (deferred by owner)

---

## Known defects

None open.

Fixed 2026-09-03: `daemon_registry_controls_and_retains_a_real_pty_session`
was flaky at roughly 1 run in 4. The cause was the vendored Zig VT library
being built unoptimized in every dev build, which made the parser 5000x
slower than it should be; the test waits on text appearing through a real
PTY and kept missing its window. It now passes 24 runs out of 24 and
finishes in 0.2s. See "parser build mode" below.

## Known environment issues

- macOS TCC intermittently denies `getcwd` and file reads under
  `~/Desktop`, which makes `cargo` fail with
  `Could not locate working directory`. Grant Full Disk Access to the
  hosting terminal app, or move the repo outside `~/Desktop`.
- Concurrent agent sessions sharing `target/` can corrupt incremental
  artifacts (`Undefined symbols` at link time). Fix:
  `rm -rf target/debug/incremental/<crate>-*`. Or give each session its own
  `CARGO_TARGET_DIR`.
- Daemon socket and state-dir paths must be short (`SUN_LEN`), e.g. `/tmp/t9`.
- Parser build mode: `libghostty-vt-sys` builds its Zig source in `Debug`
  whenever cargo sets `DEBUG=true`, so every `cargo run` and `cargo test` got
  an unoptimized VT parser at well under 1 MiB/s instead of over 400 MiB/s.
  `.cargo/config.toml` pins `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast`, and
  `crates/termi9ne-terminal/tests/parser_throughput.rs` fails if that is lost.
- Long-running dev sessions leave orphaned `--internal-daemon` processes and
  their state directories behind. Each retains terminal journals, so
  `.termi9ne-dev/` grows without anything reclaiming it.
