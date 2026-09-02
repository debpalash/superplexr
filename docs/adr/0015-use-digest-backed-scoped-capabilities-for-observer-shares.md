# Use digest-backed scoped capabilities for observer Shares

A Share is a revocable person/device invitation, not an SSH login and not a
terminal Control lease. Its first role is `observer`: it may read explicitly
scoped Missions and terminal Sessions, including live subscriptions, retained
history, search, and deterministic scheduler projections. It cannot mutate the
graph, launch or control processes, acquire terminal control, inspect runtime-wide
diagnostics, administer scheduling, or create/revoke Shares.

The owner mints a 256-bit random secret. The plaintext capability is returned
once; the owner-only atomic Share store persists only its SHA-256 digest plus a
bounded label, role, Mission/Session scope, creation time, expiry, and revocation
time. Clients read capabilities from owner-only files so secrets do not enter
argv or process listings. Authentication is repeated for every request. One
connection cannot change between owner and Share identities.

Authorization happens before payload-dependent side effects. Collection
responses and live subscription events are filtered to the same scope. Revoking
a Share durably marks it and publishes an in-process revocation event; all live
Mission, terminal-index, and terminal-frame subscriptions for that identity fail
closed immediately, including if the revocation channel lagged.

The preview protocol currently carries the capability over an ordered local Unix
stream. It is suitable behind an encrypted transport but is not itself an
Internet gateway. Browser/mobile delivery needs a separate TLS gateway, device
enrollment, origin policy, rate limits, and secret storage while reusing this
authorization module. Controller/editor roles require a new decision; they are
not obtained by widening the observer allowlist. ADR 0016 separately promotes
scoped terminal Controller authority while leaving this Observer contract
unchanged.
