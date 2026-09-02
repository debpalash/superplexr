# Persist opt-in scheduler policy outside Mission history

Continuous scheduling is explicit, disabled until configured, and persisted in
an owner-only runtime policy file. Each policy names a Mission, concurrency cap,
Session prefix, working directory, and terminal grid. The daemon periodically
reconciles enabled policies through the same deterministic configured-engine
batch launch used by explicit control requests. Disable stops future selection
but never terminates active Runs.

The policy is not a Mission event. Concurrency preference is operational state,
while cwd and grid are host-specific execution/presentation settings; putting
them in portable graph history would make replay depend on one machine. The
policy store uses bounded schema validation, atomic replacement, directory fsync,
and owner-only permissions. Runs still record portable redacted driver snapshots
in Mission history before launch. Missing drivers cause no graph mutation and
are safe to retry on later reconciliation.

The same file stores bounded runtime-wide scheduler settings. All explicit,
configured-batch, and continuous agent launches pass through one serialized
admission gate and read its single global limit, defaulting to twelve running
terminal-backed agents and configurable from one through 256. Invalid settings
do not replace memory or disk state. Enabled Mission policies rotate their first
position each reconciliation interval, so capacity released under a saturated
host is offered round-robin rather than always to the lexicographically first
Mission.
