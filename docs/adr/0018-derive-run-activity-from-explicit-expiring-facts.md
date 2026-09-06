# Derive Run activity from explicit expiring facts

Agent-provider activity is operational evidence, not Mission truth. ultraplexr
persists only the latest bounded provider fact per Run in an owner-only runtime
store. The daemon authors its observation time and provenance: a report arrived
through either the authenticated Run-scoped agent channel or the owner control
socket. Every fact expires; expiry produces `Unknown`, never inferred `Idle`.

The visible Activity projection is derived in strict order: finished/pending
Run lifecycle, unresolved typed Signals, fresh provider fact, then `Unknown`.
Provider facts cannot finish a Run, resolve attention, issue Grants, or change
dependencies. A later conservative rendered-frame heuristic may sit below
provider facts, but cannot be authoritative and must remain explainable.

This store remains outside Mission history because provider activity is
high-frequency, host/integration-specific, and intentionally ephemeral. Durable
Mission decisions still use Runs, Signals, Artifacts, and Disposition. The
separate fact store uses bounded validation, atomic replacement, directory
fsync, owner-only permissions, and stable adapter/provider identity so status
explanations remain auditable without recording terminal content or secrets.
