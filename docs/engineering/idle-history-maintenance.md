# Idle history maintenance and actor deadlines

This follow-up to [the shared-client release baseline](shared-client-resource-evidence.md)
keeps twelve Sessions and their 100,000 retained history rows. It does not relax
the 600 MiB RSS or 0.5%-of-one-core runtime idle targets.

## Implementation

`TerminalModel::maintain_history(now)` owns the compression schedule behind the
existing terminal module interface. It observes Ghostty's opaque activity token,
waits one second after a change, then performs one incremental step per due call
with an eight-millisecond continuation interval. Callers get only an optional
delay, not Ghostty types. No thread, timer, loop-to-completion, history eviction,
frame publication or synchronous full compression is added by this method.

Completion stops compression scans until activity changes. The initial test
combined viewport navigation and search; a later [reclamation follow-up](macos-history-reclamation.md)
found and fixed missing scheduling after search-only restoration. Resize and
viewport changes already advanced the native token. Unsupported platforms disable the optimization;
an error is reported once and disables it without ending the Session. This is
opportunistic compression: completion does not prove every page was profitable
or that an OS reclamation request succeeded for every page.

The existing owning Session actor serializes maintenance with all terminal
operations, after pending output is flushed. Its quiet receive timeout now uses
the next foreground-process check (400 ms), pending frame publication (8 ms),
history step, observed-exit drain, or termination grace deadline. Channel input
wakes the actor immediately; the timeout is not input polling. Hangup and TERM
graces remain two and three seconds, and force termination remains explicit.

## Correctness checks

- A real 100,024-row terminal test proves incremental scheduling and convergence,
  unchanged live frames, no repeated idle passes, complete history after search,
  and correct resize and subsequent output. The no-op implementation failed the
  scheduling assertion before the implementation was added.
- Styled Unicode, combining characters, selection text and styled frames survive
  maintenance. Alternate-screen content does not leak into primary history.
- Real PTY tests exercise a quiet period, repeated input/snapshot wakeups,
  reattachment with the same Session/PID, kernel resize, final output before exit,
  and escalation only after both existing grace periods.

The full-workspace run passed 365 Rust tests with six explicitly ignored tests.
The 15 browser tests and five harness tests passed. Formatting and workspace
Clippy with all targets/features and `-D warnings` passed; all five release
binaries built. Cargo's existing `block v0.1.6` future-incompatibility notice
remains. The optimized five-minute release measurement completed successfully
as a benchmark operation, with the memory-budget miss reported below.

## Compression-only diagnosis

Compression was measured before changing the actor's 16 ms idle tick.
[The 15-second diagnostic](shared-client-compression-diagnostic.json) verified
all reference grids/history and Session identities, but measured 1.195% runtime
CPU and 1,093.86 MiB peak runtime-plus-desktop RSS. This is not a five-minute
performance result and does not establish a memory-budget pass.

A separate 60-second diagnostic run used the same compression-only binaries. A
read-only `vmmap -summary` of its isolated runtime is retained in
[the memory map](history-compression-memory-map.txt). It reported 587.0 MiB current
physical footprint and 921.3 MiB peak footprint, with about 976.7 MiB total resident
regions. These are different accounting measures, not interchangeable gate values.
The memory-map inspection means that run is diagnostic, not a clean CPU sample.

The pinned native `terminal/mem.zig` uses Darwin `MADV_FREE_REUSABLE` for compressed
backing mappings. `PageList.MemoryStats` also explicitly documents that pool-owned
pages can occupy only part of an allocation: unused backing tails remain resident
even after the initialized portion is compressed. The map is consistent with
partial reclamation; it does not isolate how much RSS is due to each cause.
At that diagnostic stage no native dependency/build-cache source had been
modified, and no footprint number was substituted for the RSS requirement.

## Five-minute release comparison

The follow-up completed on 2026-09-06 IST (2026-09-05 UTC), on the same Apple M2
host with the same twelve 80x24 Sessions, 100,000 retained history rows each,
Observer desktop, TUI and HTTP/SSE consumer. No build/test job overlapped it.
[Raw follow-up samples and binary hashes](shared-client-idle-maintenance-release.json)
remain separate from the original baseline.

| Observation | Original baseline | Follow-up | Target |
|---|---:|---:|---:|
| Idle window | 300.048 s | 300.069 s | >= 300 s |
| Runtime CPU, one core | 1.156% | 0.323% | <= 0.5% |
| Observer desktop CPU, one core | 0.373% | 0.357% | <= 1% |
| Runtime + desktop sampled peak RSS | 1,065.48 MiB | 1,101.47 MiB | <= 600 MiB |
| Unexpected idle SSE frames | 0 | 0 | 0 |
| Idle TUI output bytes | 0 | 0 | 0 |

Runtime CPU was about 72% lower in this comparison and met the local target.
The RSS budget **did not pass**: the observed peak was slightly higher, not
lower, than the baseline. Runtime RSS ended at 699.67 MiB and desktop at
85.02 MiB. Compression's native storage changes are not an RSS-budget success.

Every reference grid and oldest history marker survived the complete idle
window, with no Controller acquired; all twelve Session IDs and PTY PIDs
survived subsequent input. CLI-to-SSE p95 was 67.73 ms (50 ms observation
granularity), effectively similar to the earlier 67.50 ms sample, not a
key-to-pixel latency claim. Fixture cleanup completed, and the user's existing
desktop/runtime processes remained alive.

This is one local sample, not cross-platform certification, a rendering/IME
acceptance test, or proof under sustained output/search. The benchmark's browser
process/compositor exclusions and other limits still apply.

## Remaining work

The [macOS reclamation follow-up](macos-history-reclamation.md) now integrates
a reproducible pinned native patch and fixes search-only maintenance scheduling.
Its five-minute sample passed the local memory and CPU targets at 187.44 MiB
combined peak RSS, with 372 passing Rust tests. This does not alter the historical
measurements above or establish cross-platform/full-workload certification.
Keep history integrity, frame stability and input responsiveness as regressions.
Logical-line retention, paged/nonblocking search and other platform acceptance
remain separately open.
