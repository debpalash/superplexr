# macOS history reclamation follow-up

This follows the [idle-maintenance measurement](idle-history-maintenance.md).
The workload and budgets remain unchanged: twelve 80x24 Sessions, 100,000
retained short ASCII history rows each, <=600 MiB sampled runtime-plus-desktop
RSS, and five-minute idle CPU <=0.5% runtime / <=1% desktop on one core.

## Two causes, two fixes

1. Darwin's reusable-memory advice reduced physical-footprint accounting without
   promptly dropping RSS. A one-model diagnostic against the unmodified native
   library showed physical footprint falling from 80.3 MiB to 16.0 MiB, while
   writable resident regions stayed around 80 MiB. The original
   [before/after memory maps](history-reusable-mapping-diagnostic.txt) retain this
   distinction. They establish the reusable-mapping issue for that workload,
   not a universal attribution of every page-pool allocation.
2. Native selection formatting restores compressed pages without changing the
   native compression activity token. The adapter's search and clipboard paths
   use that formatting API. A new repeated search test failed at width 80,
   generation 0, cycle 1: maintenance returned zero steps after the first search.
   The earlier 100,000-row regression navigated a viewport before searching,
   which independently changed the token and hid this scheduling gap.

The native patch uses one macOS `MAP_FIXED` operation to replace the exact
owned anonymous backing with zero-filled pages at the same virtual address.
Actual system-page address/length alignment is checked before replacement.
There is no separate unmap/remap allocation gap. Strict decommit runs after
compression succeeds; zero-mode callers already guarantee the untouched tail
is zero. Recommit on macOS requires no extra operation. Linux and non-macOS
Darwin behavior is unchanged. No unsafe Rust was added to the terminal adapter.

The adapter now records search/clipboard formatting as actor-local read activity
using a `Cell<bool>` inside history maintenance. After a read, the existing
one-second idle delay and bounded incremental steps run again. Clean frames
do not schedule maintenance; completed passes remain idle until another
relevant operation. No extra worker, polling loop, or frame publication is added.

## Reproducible build boundary

The local `vendor/libghostty-vt-sys` copy retains upstream Rust bindings and
vendors only the native build boundary. Cargo.lock resolves that crate locally;
the safe Rust wrapper remains at its original pinned Git revision. The native
pin remains `22d13172cde98a0a4dda05d3d6a3fcb0dd8ed018`.

The build helper hashes the preimage before installing the exact reviewed
[overlay and diff](../../patches/ghostty/README.md). Repeated application is
idempotent; unknown source revisions fail before modification. Source overrides
are copied into Cargo-owned output, not patched in place. Copies exclude Git
and Zig caches, materialize internal file links, and reject escaping/directory
links, overlapping source/output trees, and destination symlinks. Replacement
also preserves read-only and hard-linked input files. These safeguards assume
trusted build inputs, not a hostile concurrent filesystem writer.

Installed `pkg-config` libraries cannot bypass the patch. The normal/build
license audit verifies local resolution. Upstream MIT license notices for the
Rust bindings and native Ghostty code are included for release notice collection.

## Regression coverage

- Six integration tests check diff/preimage/postimage fidelity, repeated
  application, unknown revisions, independent source copies, refresh, excluded
  caches, immutable/hard-linked inputs, overlaps, and symlink boundaries.
- A real native stress test creates sixteen terminal models across four threads
  at widths 80, 81, 132, and 257. It performs four search/recompression cycles
  per model, checks all 8,000 retained rows plus exact Unicode markers and live
  frames, and drops compressed mappings before allocating the next model.
- Existing full 100,000-row batch/chunked, history/resize, Unicode/selection,
  alternate-screen and real-PTY tests remain unchanged in purpose. Clipboard
  formatting now explicitly checks that maintenance is scheduled and settles.

These Rust integration tests link the actual native library, so they exercise
macOS mapping replacement. Native Zig unit tests simulate reclamation and would
not alone prove this behavior. Memory-pressure failure injection remains open.

## Release evidence

The default (no source override) release workspace suite passed 372 Rust tests,
with six explicitly ignored tests. The browser/harness suite passed 20 tests.
Release workspace Clippy with all targets/features and `-D warnings`, Rust/Zig
format checks, the normal/build license gate, and all five release executable
builds passed. Generated third-party notices include both the native Ghostty
and Rust bindings MIT texts. Cargo's existing `block v0.1.6`
future-incompatibility notice remains.

The normal fetched native tree's HEAD and installed overlay hash were checked
against the recorded pin; the explicit-source build path was also exercised.
The [five-minute release sample](shared-client-native-reclaim-release.json)
completed on 2026-09-06 IST (2026-09-05 UTC), on Apple M2 / 8 logical CPUs /
16 GiB RAM, using the same version-2 harness and unchanged workload. No build,
test, metadata generation or dependency download overlapped the measurement.
The user's existing apps remained running; this was not an exclusive lab host.

| Observation | Previous sample | This sample | Target |
|---|---:|---:|---:|
| Idle window | 300.069 s | 300.054 s | >=300 s |
| Runtime idle CPU, one core | 0.323% | 0.317% | <=0.5% |
| Desktop idle CPU, one core | 0.357% | 0.377% | <=1% |
| Runtime + desktop sampled peak RSS | 1,101.47 MiB | 187.44 MiB | <=600 MiB |
| Unexpected idle SSE frames | 0 | 0 | 0 |
| Idle TUI output bytes | 0 | 0 | 0 |

All six harness gate observations passed. The sampled combined RSS peak fell
about 83% from the previous run, while retaining the same history allowance.
Runtime RSS peaked at 98.14 MiB and desktop at 89.31 MiB; their sum is calculated
from coincident samples, not independently timed maxima. The separately reported
gateway peaked at 4.78 MiB / 0.047% CPU and TUI at 2.53 MiB / 0.057% CPU.

Every Session's 80x24 grid, lack of a Controller, and oldest `H000000` history
marker were verified before and after idle. All twelve Session IDs and PTY PIDs
survived the post-idle input checks. CLI-to-SSE p95 was 69.26 ms with 50 ms
observation granularity; this is not input-to-pixel evidence. Fixture revocation,
termination and cleanup completed. The user's pre-existing desktop/runtime
processes were separately rechecked alive afterward.

The raw artifact retains all samples and the five executable hashes/sizes.
Reproduce with the commands in the [shared-client ledger](shared-client-resource-evidence.md).
This establishes a local reference-sample pass, not the entire Q-PERF-003
contract: startup budgets, other platforms and broader workloads remain open.
Browser here is the native gateway plus HTTP/SSE consumer, not a rendered browser
process. Browser/outer-terminal/compositor/benchmark-driver/Python-host RSS and
pre-idle allocation peaks are outside this sample.

## Remaining scope

One host and short ASCII history cannot certify the cross-platform release gate,
sustained output/search, all allocation-pressure failures, styled/wide history
resource budgets, accessibility/IME, or input-to-pixel latency. At this milestone,
search still materialized retained text synchronously. The subsequent
[bounded-search work](bounded-history-search.md) replaces that live path with
actor-local incremental reads; wire-visible pages/cancellation and the spec's
logical-line retention semantics remain open.
