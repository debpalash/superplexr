# TODO

Updated: 2026-09-03. Companion to
[superplexr-unified-roadmap.md](superplexr-unified-roadmap.md), which stays the
authoritative capability catalog. This file is the short, ordered list.

Legend: `[x]` done and verified · `[~]` partial · `[ ]` not started

---

## 1. Fault / debugging loop

The differentiator: agents write code, superplexr owns what happens when it
breaks. A Fault closes only when a replay actually passes.

- [x] `FaultId`, protocol types, 6 requests
- [x] Durable owner-only store, atomic writes, open Faults never evicted
- [x] Replay executor (bounded, off the async path, receipts)
- [x] Resolve refused without a passing replay; dismiss needs a note
- [x] Auto-detect: terminal session ends in failure
- [x] Auto-detect: per-command via OSC 133 marks
- [x] CLI: `report list show repro resolve dismiss handoff`
- [x] Desktop: sidebar `△ Broken` card + panel (⌘⇧F)
- [x] MCP bridge (`superplexr-mcp`), Fault tools only
- [x] Live end-to-end proof on a real daemon and PTY

Remaining:

- [ ] **Bisect a Fault across Candidates** — find which Run introduced it.
      Needs Candidate patch history, not the Fault store. **Next.**
- [x] **Flaky vs real** — `fault classify` replays N× (optionally each in a
      fresh worktree) and records runs/failures/errors with a verdict of
      real, flaky, passing or inconclusive; a flaky Fault cannot be resolved
      on a lucky pass; verified end to end including worktree cleanup
- [x] **Fault → Run** — `fault fix` plans a Run with the Fault as its
      objective, links the two, and launches the configured engine driver;
      verified that a Run claiming success still cannot close the Fault
- [x] **Regression guard** — `fault guard` re-replays resolved Faults and
      reopens any that fail again, keeping the replay that closed them so the
      regression can be read against it; verified end to end on a real daemon
- [x] Shell integration installer (`superplexr shell-init`) for zsh, bash and
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

## 3b. Wire cost of a terminal subscription (measured 2026-09-03/04)

`crates/superplexr-client/examples/delta_bandwidth` against a live daemon, on
the protobuf data plane the desktop actually uses. Before: a repainting agent
screen was **2.27 MB/s** at ~117 deltas/s (~20 KB each), scrolling build
output **0.97 MB/s**, one full 120×36 frame **21 KB** (~5 B/cell).

- [x] Within a changed row, send only the changed cell span (protocol 26):
      repaint **162 KB/s**, scroll **121 KB/s**, average delta 1.4 KB — a
      14× and 8× reduction with no change in what the viewer sees
- [x] Coalesce publishes per subscriber (`SubscribeTerminal.max_hz`, default
      60, actor publishes at up to 125), as a tested `RateGate`: the first
      version flushed the held frame on the next frame's arrival and
      coalesced nothing
- [x] Re-measured 2026-09-04: repaint **86 KB/s @60 Hz, 47 KB/s @30 Hz**;
      scroll **75 KB/s @60, 43 KB/s @30**. Target met at the remote rate
      (≤ 50 KB/s): 48× below where this started
- [ ] Run-length styles within a row — not needed for the target; keep for
      mobile (≤ 20 KB/s) if measurement there demands it
- [x] Protocol version policy written (`docs/protocol-versioning.md`);
      a bump carries the previous version's state forward

## 3c. Network gateway (platform plan, phase 3 — built 2026-09-04)

Rules in `docs/gateway.md`. Off unless `--gateway host:port`; nothing
unpaired gets past the handshake; tokens and pairing codes are stored only
as digests; identity is a pinned certificate fingerprint; revocation ends
open connections at their next request; devices carry owner authority and
scoped access uses share tokens over the same listener.

- [x] TLS listener (rustls/ring) with a self-signed identity created once
- [x] Device store: pairing codes (5 min, single use, not persisted), device
      tokens (digest only), list, revoke; `is_active` checked per request
- [x] Handshake admission: pairing code → token minted once in `Welcome`;
      token → authenticated; neither → `gateway_unauthorized` and close
- [x] Client: `Endpoint::{Unix, Gateway}`, pinned-fingerprint verifier, a
      blocking TLS stream shared by reader and writer, `connect_gateway`
- [x] CLI: `device-pair`, `device-list`, `device-revoke` on the host;
      `pair --gateway --fingerprint <code>` on the device; global
      `--gateway` routes any one-shot command; `attach --gateway` over TLS
- [x] End-to-end (`ci/gateway-smoke.sh`): unpaired refused, pair, request
      over TLS, wrong fingerprint refused, attach under a pty over TLS,
      revoke → refused; the first TLS pump starved its writer under a
      tight relock loop and was rewritten to block outside the lock
- [ ] Per-connection request rate limits
- [ ] Outside security review of pairing and token scope before any public
      exposure (the phase gate)

## 3d. Web shell (platform plan, phase 4 — built 2026-09-04)

- [x] One port: the gateway listener sniffs wire magic vs HTTP and serves
      the page, its scripts and `wss://…/ws` beside the native wire
- [x] WebSocket hand-rolled in `crates/superplexr-server/src/web.rs` (RFC
      6455 framing, masking, origin check; unit tests against the RFC's own
      vectors) — no web framework, no new crates beyond `ring` for SHA-1
- [x] Browser client in `web/` as plain ES modules: wire_v3 header and JSON
      control plane, a hand-written protobuf reader for the terminal data
      plane, a canvas renderer, pairing with the same code, keys, resize,
      paste, selection and copy, reconnect with backoff
- [x] `start_terminal` with an empty program/cwd means the login shell at
      home, so screens that do not know the machine can say "new shell"
- [x] End-to-end (`ci/web-smoke.sh`) green: page + CSP, unpaired refused, pair,
      shell started from the browser, frames decoded and applied, echo seen,
      off-host origin refused
- [x] Found by the smoke test: the subscriber task hit an `unreachable!` on
      a finished command block (OSC 133), freezing that viewer's frames —
      the desktop's too — after the first command; `protocol_event` is
      total now, with a test
- [ ] Mouse reporting to the session when the program asks for it
- [ ] Scrollback in the browser (history pages over the wire)
- [ ] IME / composition input

## 3e. Rooms and streams (platform plan, phase 5 — streams built 2026-09-04)

- [x] Streams: `superplexr stream <session>` mints a Share link; the gateway
      admits a Share token at the handshake and binds the connection to it
      (every request must carry the same token → `share_token_required`)
- [x] Presence: subscriptions register on their terminal record for their
      lifetime; `terminal-viewers`, `Request::TerminalViewers`, `👁` in the page
- [x] `Request::GatewayInfo` so links can be composed on the host
- [x] `ci/stream-smoke.sh`: link, viewer admitted and limited (no keys, no
      token-less request, no owner request), presence, 50-viewer fan-out,
      revocation → refused
- [x] Fan-out cost measured 2026-09-04: 50 viewers on one session scrolling
      20 lines/s, each subscribed at 30 Hz → 16.1 KB/s per viewer (max
      16.1), 15.3 frames/s each, presence exact at 51 — under the plan's
      20 KB/s gate without compression (the browser has no zstd)
- [x] Rooms: control handed, never seized — raise hand / offer / accept /
      withdraw on top of claim/release, offers bound to the control epoch,
      participants named in presence, room state on the index stream, the
      browser's control panel; `ci/rooms-smoke.sh` (two hand-offs, no
      keystroke lost)
- [ ] Rooms in the desktop: show hands and offers, offer from the pane
- [ ] Per-participant cursor and selection shown to the others
- [ ] Replay: journal playback at speed with Faults, Runs and hand-offs as
      chapters

## 4. Terminal and desktop certification (Gate E)

- [ ] Unicode / IME / Kitty keyboard / Neovim compatibility matrix
- [ ] Six visible surfaces at 60 FPS, measured
- [ ] Reconnect, crash, disk-full, and long-soak evidence
- [ ] Wayland and X11 passes on physical Linux
- [ ] 200% zoom visual and accessibility pass
- [ ] Linux keyboard-shortcut smoke test
- [~] Resizable sidebar and terminal splitters (built; needs platform pass)
- [x] Sessions titled by the agent's own title, else `repo@branch` from the
      terminal's directory (runtime cwd, or OSC 7 from `shell-init`); a name a
      person chose always wins; derived at render so it never freezes
- [x] Viewer-side selection: highlight and copy from the frame the desktop
      holds, no daemon request, so it works on agent-owned, observed and
      finished terminals; Shift bypasses an app's mouse reporting, ⌥ drags a
      rectangle, a plain click clears
- [x] TUI shell: `superplexr attach <session>` paints frames into any
      terminal and forwards keys (Ctrl-] detaches, `--observe`, `--take`,
      `--max-hz`); no VT parsing client-side; verified under a real pty
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

- macOS TCC denies the terminal app (Ghostty here) access to `~/Desktop`,
  so every process it spawns gets `Operation not permitted` on the repo and
  `cargo` dies before the app starts:
  `Could not locate working directory: Operation not permitted (os error 1)`.
  This is what "the app doesn't run" was on 2026-09-03, after two rounds of
  real fixes to the app itself. The grant is per folder and a dismissed
  prompt means denied, so it flips mid-session. Do not keep the repo under
  `~/Desktop`, `~/Documents` or `~/Downloads`; `~/src/superplexr` never hits
  this. If the Desktop copy must be used: System Settings → Privacy &
  Security → Files and Folders → Ghostty → Desktop Folder, then restart
  Ghostty.
- Concurrent agent sessions sharing `target/` can corrupt incremental
  artifacts (`Undefined symbols` at link time). Fix:
  `rm -rf target/debug/incremental/<crate>-*`. Or give each session its own
  `CARGO_TARGET_DIR`.
- Daemon socket and state-dir paths must be short (`SUN_LEN`), e.g. `/tmp/t9`.
- Parser build mode: `libghostty-vt-sys` builds its Zig source in `Debug`
  whenever cargo sets `DEBUG=true`, so every `cargo run` and `cargo test` got
  an unoptimized VT parser at well under 1 MiB/s instead of over 400 MiB/s.
  `.cargo/config.toml` pins `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast`, and
  `crates/superplexr-terminal/tests/parser_throughput.rs` fails if that is lost.
- Long-running dev sessions leave orphaned `--internal-daemon` processes and
  their state directories behind. Each retains terminal journals, so
  `.superplexr-dev/` grows without anything reclaiming it.
