# Shared bounded Share-token loading

Status: source implemented, unbuilt and untested. No tests, builds, credential
files, runtime connections or app runs were executed during this implementation.

CLI, TUI, browser gateway and MCP now use one native-client token-file reader.
It opens once with `O_NOFOLLOW | O_NONBLOCK`, checks the descriptor for a regular
file owned by the effective user, refuses group/other permissions, and permits
at most 16 KiB of file content. It reads no more than that limit plus one
overflow byte and checks descriptor metadata again for observable concurrent
changes. FIFO paths cannot make the open wait for a writer; final-component
symlinks are rejected.

After trimming surrounding whitespace, one nonempty token of at most 512 UTF-8
bytes is accepted; internal whitespace and control characters are refused.
Errors never contain token contents. Runtime authentication remains separate,
and invalid loading never falls back to owner access. The browser loads in its
existing blocking connection worker; other clients retain their startup stages.

The CLI's token limit is unchanged, but its former 512-byte whole-file limit is
now the shared 16 KiB allowance. TUI/browser/MCP now apply the same token-content
and effective-user ownership checks. Compatibility has not been executed.

This does not prohibit ancestor symlinks, freeze owner-writable files, guarantee
atomic content snapshots, zeroize strings, or impose a hard regular-file I/O
deadline. Credential encryption and Windows support are outside this Unix
helper. The existing workspace `libc` dependency is now direct in the shared
client rather than MCP; no new dependency version was resolved.

Deferred acceptance includes all callers, symlink/FIFO/nonregular paths, wrong
owners/modes, growth/mutation, UTF-8/content bounds, whitespace, redacted errors,
no authentication fallback, startup cancellation and supported Unix platforms.
