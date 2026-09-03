# Sandbox configured drivers at the process-spec seam

Sandbox intent belongs to an owner-controlled named driver, while enforcement
belongs to a platform adapter that rewrites the structured process spec before
the atomic Run/Session launch. Scheduling, the Mission model, PTY ownership, and
the desktop never branch on macOS versus Linux.

The first profile is `workspace_write`. It canonicalizes both workspace and
runtime-state paths, permits host reads and workspace writes, and re-protects the
runtime state even when `.superplexr` is under the workspace. macOS uses
`sandbox-exec`; Linux uses Bubblewrap with a read-only root and writable bind.
Backend absence is a hard resolution failure—there is no silent cooperative
fallback.

The adapter returns a sandbox attestation that enters redacted preview and the
immutable driver snapshot. Network isolation is a separate boolean and is false
for this profile because agents need provider access and their local restricted
channel. This decision provides useful filesystem-write containment without
claiming a stronger network or same-UID boundary than is implemented and tested.
