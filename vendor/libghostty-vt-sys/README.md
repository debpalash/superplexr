# libghostty-vt-sys

Raw FFI bindings for libghostty-vt.

This is Ultraplexr's reviewed build fork. See [ULTRAPLEXR.md](ULTRAPLEXR.md)
for upstream provenance and local changes. Native build paths described here
are not Ultraplexr platform-acceptance claims.

- Fetches and builds `libghostty-vt.a` from ghostty sources via Zig by default.
- Exposes checked-in generated bindings in `src/bindings.rs`.
- Static linking is the baseline rather than a Cargo feature. Enable the
  additive `link-dynamic` feature to link the shared library instead.
- Set `GHOSTTY_SOURCE_DIR` to provide a local Ghostty checkout. This fork copies
  it into Cargo output before applying the hash-checked native overlay.
- Set `GHOSTTY_ZIG_SYSTEM_DIR` to force Zig package resolution through a
  pre-fetched `zig build --system` directory. This is intended for Nix and other
  sandboxed package managers that cannot fetch during build scripts.
- Vendored builds target Zig's portable `baseline` CPU by default. Set
  `LIBGHOSTTY_VT_SYS_CPU` to `native`, a named CPU model such as `x86_64_v3`, or
  another Zig CPU expression to optimize for known deployment hardware.
- Set `LIBGHOSTTY_VT_SYS_OPTIMIZE` to `Debug`, `ReleaseSafe`, `ReleaseFast`, or
  `ReleaseSmall` to override the Zig optimize mode used by vendored builds.
- iOS targets (`aarch64-apple-ios`, `aarch64-apple-ios-sim`) build through
  ghostty's xcframework emit instead of a flat cross build. This requires a
  macOS host with Xcode and the iOS SDK installed, and supports static linking
  only. The simulator library is arm64-only (what simulators run on Apple
  silicon), so `x86_64-apple-ios` is not supported.
- The `pkg-config` feature is retained as a compatibility flag but cannot bypass
  the reviewed native build with an installed library.
- libghostty-vt is pre-1.0, so these bindings do not guarantee compatibility
  with arbitrary installed C API revisions.
