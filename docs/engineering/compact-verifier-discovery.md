# Compact verifier discovery

Implemented 2026-09-06. Integrated Rust validation is in progress; this is not a
production or platform-readiness declaration. The earlier testing pause has
been superseded by the owner's explicit request for full testing and fixes.

The shared runtime now exposes a negotiated `verification_catalog_v1` read.
Each page belongs to one authorized Mission, carries its observed version, and
contains at most 64 verifier summaries ordered by Run UUID. `after` is exclusive;
`next_after` is the last returned ID when another page exists. A page contains
Run/subject identity, execution phase/outcome, owner disposition and optional
primary Session identity. Discovery never computes every Run's receipts,
inspects evidence, attaches Sessions or changes workflow state.

The store borrows the Mission and the domain projection keeps at most 65 IDs
while scanning frozen delivery inputs. Output and temporary collection size are
bounded, but scan work still grows with the Mission's input count and happens
under the existing store lock. This is not a constant-time or measured latency
claim. Only structurally valid verification inputs with existing Run/subject
records participate; a non-verifier cannot be inspected as one.

The native client checks the capability on its captured wire, validates page
identity/order/cursor/count, and makes one read. It neither falls back to a full
Mission nor walks pages automatically. Runtime/agent-channel authorization uses
the existing Mission-read scope. Session-only Shares do not acquire Mission
visibility through this feature.

## Interfaces

- CLI: `verification-list MISSION_ID [--after RUN_ID] [--limit 1..64]` emits one
  JSON page. Mutation overrides are refused. `verification-status` inspects a
  selected verifier separately.
- MCP: optional `--workflow-read` exposes `verification_list` and
  `verification_status`, with unknown/malformed arguments rejected before RPC.
- Browser: optional `--workflow-read` exposes authenticated GET
  `/missions/{mission}/verifiers?after=RUN_ID&limit=32`. The existing read permit
  remains owned by native work even if its HTTP request is cancelled. The UI
  lists on request, replaces pages instead of accumulating them, and inspects a
  row only after selection. Direct Run-ID inspection remains available.
- TUI: optional `--workflow-read`, `w` for the selected Mission's catalog,
  `n` next page, `0` first page, Enter inspect, `W` direct UUID. The same bounded
  navigation worker and generation cancellation own catalog/status reads.

These are live pages, not a retained multi-page snapshot. A Mission mutation
can change version between pages; new IDs below the cursor require restarting
from the first page. None of these views turns process exit or a recorded
passing receipt into fresh evidence validation or owner acceptance.

## Coverage being integrated

The real-daemon delivery regression covers 65 verifier IDs, bounded ordered
paging, empty discovery, non-mutating status, failed checks despite a successful
verifier exit, scope denial/revocation and restart recovery. HTTP, TUI and MCP
tests exercise their actual authorization/dispatch boundaries. Browser tests
exercise bounded DOM/data, cancellation and explicit paging. Final current-source
suite outcomes belong in the integration evidence report, not an inferred
completion percentage.
