# M1 foundation evidence

Status date: 2026-08-31. M1 status: **PASS**. This report records the executed
evidence for the upstream-feasibility lock and maps it to the M1 gate in
`docs/spec/09-delivery-plan.md`.

## Outcome

The selected foundation is feasible on macOS and Linux:

- a pinned Ghostty VT library creates, mutates, and snapshots terminal state;
- `superplexr-terminal` translates that state into owned, backend-neutral frames;
- device replies and bells cross the adapter as explicit effects;
- rich keyboard, IME commits, focus reports, and bounded/safe paste cross the
  same backend-neutral action/effect seam;
- GPUI handler tests drive key press/repeat/release data, IME preedit/commit,
  focus transitions, copy, safe paste, and confirmed unsafe paste;
- a custom GPUI `Element` paints the frame without importing Ghostty types;
- the element registers a GPUI input handler, paints active IME composition at
  the terminal cursor, and exposes one terminal accessibility node rather than
  a node per cell;
- the native macOS window builds, launches, initializes Metal, and remains in
  its event loop without a panic;
- a clean Linux ARM64 container builds GPUI with both Wayland and X11 enabled,
  then passes formatting, all workspace tests, Clippy with warnings denied, and
  release-window smokes on virtual X11 and Wayland compositors.
- the selected normal/build dependency graph contains no package with an
  undeclared license and no copyleft-only package; CI enforces the result on
  macOS and Linux.

## Immutable inputs

`DEPENDENCIES.lock` is the source record for archives and licenses. Cargo uses
the same exact Git revisions:

| Input | Revision/version | Relevant decision |
|---|---|---|
| libghostty-rs | `f4c72b931588cb3e53db0ea8470eaa1b2e5427d9` | adopt for M1 behind our adapter |
| Ghostty | `22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018` | transitive static `libghostty-vt` source |
| GPUI/Zed | `bce0c5785bfd9172c939aca4083fd70bc4930927` | isolate in `superplexr-desktop` |
| GPUI compatibility seams | local workspace crates | independently implemented MIT/Apache interfaces |
| Rust | `1.97.1` | required by the GPUI pin |
| Zig | `0.16.0` | required by the libghostty-rs pin |

The Linux image verifies official Zig archives against architecture-specific
SHA-256 values before extraction. GitHub Actions are referenced by immutable
commit IDs rather than floating tags.

## Terminal adapter audit

The M1 adoption decision is bounded by the following findings:

- `Terminal`, `RenderState`, and their iterators are intentionally neither
  `Send` nor `Sync`. A future Session actor must construct and use them on one
  owning OS thread or local executor. No unsafe marker implementation is
  permitted to bypass this constraint.
- No Ghostty type, pointer, slice, callback lifetime, or error type crosses the
  public terminal-frame interface except as a wrapped `TerminalError` cause.
- Grid dimensions are validated before entering FFI: 2–1000 columns and 1–500
  rows. Style indexes are checked again at the renderer boundary.
- Callback effects are copied into Rust-owned buffers before they leave the
  owning terminal model. Empty writes and no-op resizes do not advance frame
  sequence numbers.
- Key input retains physical and logical identity, produced UTF-8, modifiers,
  consumed modifiers, press/repeat/release, and composition state. Ghostty's
  encoder reads active legacy/Kitty modes from canonical terminal state for
  every event.
- Focus reports are emitted only under DEC mode 1004. Paste is capped at 8 MiB;
  multiline, bracket-escape, and non-UTF-8 data stop as confirmation effects,
  while confirmed input uses Ghostty's sanitization and bracketed-paste mode.
- Full-frame capture resolves colors, interns styles, preserves wide/spacer
  cells, captures cursor state, and clears upstream dirty markers only after an
  owned frame has been constructed.
- Ghostty, libghostty-rs, GPUI itself, and GPUI's external Git transitives have
  exact source records. The audit found GPL-only `ztracing` and unlicensed
  `gpui_util`/`gpui_shared_string` packages in the otherwise Apache GPUI graph.
  Cargo now substitutes independently implemented local compatibility crates:
  the tracing seam re-exports the permissively licensed `tracing::instrument`,
  and the two GPUI data/utility seams implement only the interface consumed by
  this pin. No upstream implementation was copied into those crates.
- `ci/audit-normal-licenses.sh` rejects undeclared licenses, unrecognized
  `LicenseRef` expressions, copyleft-only dependencies, or remote resolution of
  any compatibility package. It permits an explicit permissive alternative such
  as `Apache-2.0 OR GPL-2.0-only`.

This is the M1 feasibility approval, not the final release supply-chain report.
`superplexr-terminal` itself contains no unsafe block, and FFI-backed terminal
tests run on both targets; Miri cannot execute through the native Zig/C ABI.
Sanitizer/fuzz corpora, regenerated-binding comparison for any upstream update,
offline source vendoring, redistributed notices, and SBOM generation remain M5
release work under section 08.

## Executed verification

macOS Apple Silicon host (`Darwin arm64`):

```text
rustc 1.97.1
Zig 0.16.0
cargo check -p superplexr-desktop                         PASS
cargo fmt --all --check                                 PASS
./ci/audit-normal-licenses.sh .                         PASS
cargo clippy --workspace --all-targets --locked -- -D warnings PASS
cargo run -p superplexr-desktop                           PASS (native event loop)
cargo test --workspace --all-targets --locked           PASS (27 tests)
cargo bench -p superplexr-terminal --bench frame_pipeline PASS (p95 1.253 ms)
cargo build --workspace --release --locked --offline    PASS (warm source cache)
./ci/package-smoke.sh .                                 PASS (signed app bundle archive)
```

Linux ARM64 (`rust:1.97.1-bookworm` in Colima):

```text
Zig archive checksum                                    PASS
cargo fmt --all --check                                 PASS
./ci/audit-normal-licenses.sh .                         PASS
cargo test --workspace --locked                         PASS (27 tests)
cargo clippy --workspace --all-targets --locked -- -D warnings PASS
cargo build --workspace --release --locked --offline    PASS (`--network=none`)
./ci/package-smoke.sh .                                 PASS (AppDir archive/linkage)
./ci/linux-window-smoke.sh ...                          PASS (Xvfb X11 + nested Weston Wayland)
GPUI features                                            font-kit, wayland, x11
```

The reproducible command is:

```sh
docker build --file ci/linux.Dockerfile --tag superplexr-ci .
```

The Linux window smoke uses Mesa software Vulkan. X11 runs under Xvfb. Wayland
runs under Weston nested on that X server so the compositor exposes a
`wl_seat`, which pinned GPUI requires at startup. Each backend must keep the
release app in its event loop for six seconds; an early exit or panic fails the
image build. These checks do not replace the physical-GPU release-host matrix.

## M1 exit-gate audit

| Criterion | Result | Evidence |
|---|---|---|
| Audit/pin libghostty-rs, Ghostty, Zig | PASS | exact revisions, archive hashes, thread/ownership bounds, local-source offline build |
| Ghostty fixture on macOS/Linux | PASS | 27-test matrix and deterministic backend-neutral frame fixture |
| GPUI macOS/Wayland/X11 windows | PASS | native macOS event loop plus six-second X11 and Wayland release smokes |
| Custom fixed-grid element | PASS | GPUI paint test at 100%/200% and native/virtual launches |
| Keyboard/IME/clipboard/focus/a11y/scale smoke | PASS | handler test, terminal action tests, single `Role::Terminal` node test, 200% draw |
| Package smoke | PASS | ad-hoc signed macOS app archive and linked Linux AppDir archive |
| Benchmark/reference manifest | PASS | release harness, Apple M2 baseline, versioned reference-host manifest |
| No unsafe leakage | PASS | owned `FullFrame`/effects/actions are the public seam; adapter contains no unsafe |
| Network-free release | PASS | macOS `--offline`; Linux Docker `--network=none` plus `--offline` |
| Accepted pins/licenses recorded | PASS | `DEPENDENCIES.lock` plus host-specific enforced license graph |

Physical VoiceOver/Orca journeys, hardware presentation timing, the full
keyboard-layout/IME corpus, and a physical x86-64 Linux performance baseline are
release gates in section 08, not M1 smoke criteria. The fixture accepts input,
but real PTY ownership deliberately begins with the M2 vertical slice.

The pinned macOS graph emits Cargo's future-incompatibility warning for
`block 0.1.6`, reached through GPUI's Cocoa/Metal stack. It is accepted only with
the pinned Rust 1.97.1 toolchain; a GPUI or Rust repin must remove or re-audit the
warning before the toolchain changes.
