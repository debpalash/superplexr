# macOS owned backing reclamation

Base: Ghostty `22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018`, MIT.
The `mem.zig` overlay is derived from upstream `src/terminal/mem.zig` under
that license. `0001-darwin-owned-backing-replacement.patch` is its reviewable diff.

- Preimage SHA-256: `633c582cb9a2ec1b279e84c0c21e848e637098ee2bedbb7dfcefeabd8288bc9f`
- Postimage SHA-256: `12ae340635185b0aa0987d8c6376f8bc6101f7183fbcc092cb6dcc17ccfa905b`

On macOS, `MADV_FREE_REUSABLE` lowers physical-footprint accounting but can keep
compressed history pages counted in RSS. This patch replaces only the exact
owned anonymous mapping with zero-filled backing using one `MAP_FIXED` call.
It preserves the virtual address, guards actual system-page alignment and
length, and avoids a separate unmap/remap allocation race. Only discarded native
terminal backing is eligible, after its compressed representation is complete
or when its contents are contractually zero. Mapping replacement retains the
original Ghostty VM tag. Linux and non-macOS Darwin behavior is unchanged.

The build helper checks the preimage and installs these exact bytes in Cargo-owned
sources. Tests reverse the diff and verify both hashes, exercise source-copy
boundaries, and call the real native library through the normal terminal
history integration tests. Native Zig unit tests simulate reclamation and do
not alone prove the OS mapping behavior. Memory-pressure failure injection and
platform acceptance outside this macOS host remain unverified.

This is not a smaller history budget or an accounting-metric substitution.
The 12-Session, 100,000-row runtime-plus-desktop RSS gate still requires its
own measured release result; a standalone terminal-model probe is insufficient.
