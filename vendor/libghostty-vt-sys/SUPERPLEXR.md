# SuperPlexr native build patch

This directory vendors `crates/libghostty-vt-sys` from
`https://github.com/Uzaaft/libghostty-rs` at
`f4c72b931588cb3e53db0ea8470eaa1b2e5427d9` (version 0.2.1).
`src/` and binding-generation tooling are unchanged. The included upstream MIT
license is the selected license for this copy; the upstream manifest also offers
Apache-2.0. The workspace safe Rust wrapper remains pinned upstream.

Local changes normalize the standalone Cargo manifest, add SHA-256-checked
native patch installation, and always build native sources (the `pkg-config`
compatibility feature no longer selects an installed, unverified library).
No new runtime dependency or thread is added.

The native pin remains Ghostty `22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018`.
See `../../patches/ghostty/README.md` for the reviewed source overlay. Explicit
`GHOSTTY_SOURCE_DIR` overrides are copied into Cargo's `OUT_DIR` before patching;
their whole-tree provenance is the caller's responsibility. The modified file
must match the pinned preimage or exact reviewed postimage. Unknown revisions
fail closed. The normal no-override path fetches the pinned revision as before.

The source-copy safeguards protect ordinary build inputs, not against a hostile
process racing filesystem changes. Zig build scripts themselves execute trusted
code. The local crate is excluded from workspace membership so workspace
`--all-features` does not accidentally select upstream dynamic linking or binding
generation. The product uses the static build.
