# 10 — Dependency baseline

Verified: 2026-08-31. This document records facts used by the specification, not
floating dependency declarations. The build uses immutable commit IDs and hashes
recorded in `DEPENDENCIES.lock` when implementation begins.

## 1. GPUI — DEP-GPUI-001

The official GPUI material describes a GPU-accelerated Rust framework with
declarative Views and low-level imperative Elements. Its current platform crate
exposes Metal on macOS and feature-selected Wayland/X11 support on Linux. The
project remains pre-1.0 and warns of frequent breaking changes.

Sources:

- [GPUI site](https://gpui.rs/)
- [GPUI crate README](https://github.com/zed-industries/zed/tree/main/crates/gpui)

termi9ne therefore:

- pins one exact Zed/GPUI commit, never `*`, a moving branch, or an unbounded
  semver range;
- isolates GPUI in `termi9ne-desktop`;
- enables both Wayland and X11 for Linux release builds;
- proves custom Element painting, rich keys, IME, clipboard, focus, accessibility,
  display scale, timers, and packaging before final UI construction;
- carries a small patch only when upstreaming or repinning cannot meet the
  milestone, with each patch documented and tested.

M1 found that the pinned Zed graph otherwise resolves GPL-only `ztracing` and
two unlicensed auxiliary packages, `gpui_util` and `gpui_shared_string`.
termi9ne therefore patches those package names to small, independently
implemented `MIT OR Apache-2.0` compatibility crates. The normal/build license
gate checks both target graphs and fails if the names resolve remotely, a
package omits its license, or a copyleft-only license enters the graph. Exact
rationale and verification evidence are recorded in `DEPENDENCIES.lock`.

## 2. libghostty-vt — DEP-GHOSTTY-001

Ghostty publishes `libghostty-vt` as its C-compatible virtual-terminal library.
The public header covers terminal state/rendering, key and mouse encoding, paste,
selection, snapshots, Unicode, and related facilities. The header explicitly says
the interface is incomplete, under development, and subject to breaking changes.
Ghostling demonstrates that libghostty-vt supplies terminal semantics/render
state but intentionally does not supply application tabs, Sessions, windowing, or
product UI.

Sources:

- [public libghostty-vt header](https://github.com/ghostty-org/ghostty/blob/main/include/ghostty/vt.h)
- [Ghostty repository](https://github.com/ghostty-org/ghostty)
- [Ghostling](https://github.com/ghostty-org/ghostling)

termi9ne therefore uses only public VT interfaces, pins a commit and build inputs,
statically links by default, and translates all upstream state behind
`termi9ne-terminal`. The private full Ghostty application/surface interface is
not a Linux product foundation.

## 3. libghostty-rs — DEP-RUST-001

The community `libghostty-rs` workspace currently contains generated raw bindings,
safe wrappers including Terminal/RenderState and encoders, and a Rust Ghostling
example. Its build pins/fetches Ghostty, supports local prefetched sources, and
uses Zig. It is useful prior art, not automatically trusted production code.

Source: [libghostty-rs](https://github.com/Uzaaft/libghostty-rs)

The adoption audit MUST record:

- exact wrapper and Ghostty commits plus Zig version;
- license compatibility and redistributed notices;
- bindgen reproducibility and checked-in/generated artifact policy;
- allocator and callback ownership;
- Send/Sync claims and thread-affinity assumptions;
- panic/unwind behavior across FFI;
- slice/string lifetime soundness, null/error handling, and integer bounds;
- terminal creation/destruction, RenderState snapshot lifetime, device reply,
  resize, selection, input encoder, and snapshot coverage;
- Miri results for Rust-owned unsafe code, sanitizer runs where applicable, and
  fuzz coverage;
- macOS and Linux static, network-free builds;
- missing public Ghostty capabilities and the maintenance cost of a fork.

Audit outcomes:

1. **Adopt pinned:** upstream passes and termi9ne adds only its terminal seam.
2. **Fork pinned:** wrapper is sound/useful but needs bounded changes.
3. **Replace binding:** generate minimal raw bindings and implement a small safe
   wrapper owned by termi9ne.

The audit cannot select the private Ghostty app surface as a fourth outcome.

## 4. Dependency pin record — DEP-PIN-001

`DEPENDENCIES.lock` will record for every source dependency:

```text
name, upstream URL, commit/tag, source archive SHA-256
license and notice path
build tool versions
enabled features
local patches and their rationale
last audit date and owner
update test report
```

Cargo.lock alone is insufficient for Git submodules, fetched Zig packages,
fonts, platform packaging inputs, and generated bindings.

## 5. Font baseline — DEP-FONT-001

Familjen Grotesk and Iosevka Term are the specified interface and terminal fonts.
Before bundling, release engineering MUST verify exact font versions, license
files, redistribution rights, glyph coverage, variable/static format behavior,
and macOS/Linux rasterization. The terminal supports user fonts and fallback; the
bundled baseline makes fixtures and first launch deterministic.

## 6. Upgrade policy — DEP-UPGRADE-001

Upstream updates are deliberate projects, not automatic weekly bumps. An update
must:

1. change the immutable pin and dependency record;
2. regenerate bindings reproducibly;
3. pass safe-wrapper, terminal semantic, protocol, UI, package, fuzz, and
   performance suites on both platforms;
4. identify changed terminal snapshots or pixels and justify them;
5. remove obsolete local patches;
6. ship no state/protocol migration unless separately specified.

Security fixes may accelerate this process but cannot bypass the cross-platform
terminal corpus.
