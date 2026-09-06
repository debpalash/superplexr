# Shared-client release resource evidence

This ledger tracks Q-PERF-003 without treating a working benchmark harness as a
passed performance gate. The reference workload is twelve real, isolated
Sessions, each with 100,000 retained short ASCII history rows and an 80x24 live
viewport. One native Observer desktop (one visible Surface), one real-PTY TUI
and one HTTP/SSE observer attach to the same runtime; attachment must preserve
all Session IDs and PTY PIDs.

## Reproduction

Run from the repository root on an otherwise quiet supported Unix host:

```sh
cargo build --release --locked -j 2 -p ultraplexr-server -p ultraplexr-cli -p ultraplexr-observer -p ultraplexr-tui -p ultraplexr-desktop
node --test ci/resource-samples-tests.mjs ci/pty-host-tests.mjs
node ci/shared-client-resource-benchmark.mjs --output /tmp/ultraplexr-shared-release-resources.json
```

The output path must not exist. The benchmark creates its own private temporary
runtime and fixture data, revokes its Share, terminates its fixture Sessions and
owned clients, and removes only its temporary tree. It does not connect to or
restart the user's runtime. Python 3 supplies a controlling PTY; no Python
packages or browser installation are needed. A native desktop environment is
required unless `--without-desktop` is supplied.

CPU is the process CPU-time delta divided by monotonic elapsed wall time, in
percent of one core, not `ps %cpu`'s moving average. After a ten-second warmup,
the default idle window is five minutes with samples every ten seconds. Raw
samples, binary hashes/sizes, host details and gate observations are retained.
Gate observations report failures but do not change the script's exit code;
operational errors do fail the command. A short `--idle-seconds 3` run verifies
the harness only, not the five-minute gate.

The memory safety stop is 2 GiB sampled fixture RSS (runtime-only while seeding,
all measured processes during idle), not a kernel allocation limit. A failed
run cleans up and does not write a completed result.

## History regression and fixture correction

The real 100,000-row test exposed two independent problems:

- The adapter configured Ghostty's row allowance but left its much smaller
  default byte cap. Without an explicit byte cap, this workload's oldest row
  was `H099288`: only about 700 history rows remained. The adapter now explicitly
  permits 128 MiB of terminal page allocation per Session, alongside the
  100,000-row allowance. This is a ceiling, not a preallocation or an RSS promise.
- The original fixture ended every line with a newline. Its final blank live
  row took history to 100,001 rows, legitimately triggering whole-page eviction
  even with sufficient bytes. Removing that final newline preserves the exact
  intended workload: 100,000 history rows plus 24 live rows. Verification now
  requires the very first marker (`H000000`), not a later approximate marker.

`crates/ultraplexr-terminal/tests/history_budget_tests.rs` exercises both one
large output batch and 4 KiB PTY-style chunks. It checks all 100,024 marked rows,
the oldest row through independent viewport navigation, and bounded page pruning
after exceeding the row allowance. Both tests passed locally. Removing the byte
configuration makes the corrected fixture fail, so this is a product regression
test as well as a benchmark-fixture correction.

The pinned backend counts physical rows, not logical wrapped lines, and evicts
whole pages when either limit is exceeded. The spec's configurable logical-line
retention, stable absolute history coordinates, explicit truncation markers and
nonblocking paged search remain separate work; this fix does not claim those.

## Initial baseline and limits

The corrected release sample completed on 2026-09-06 IST (2026-09-05 UTC), on an
Apple M2 with eight logical CPUs and 16 GiB host RAM. Raw samples and executable
SHA-256/size evidence are retained in
[the release baseline](shared-client-release-baseline.json). Node was v24.12.0
and rustc was 1.97.1. All five binaries were built with the release command above.

| Observation | Measured | Target | Result |
|---|---:|---:|---|
| Idle sample duration | 300.048 s | >= 300 s | Met |
| Runtime idle CPU, one core | 1.156% | <= 0.5% | Missed |
| Observer desktop idle CPU, one core | 0.373% | <= 1% | Met for this sample |
| Runtime + desktop sampled peak RSS | 1,065.48 MiB | <= 600 MiB | Missed |
| Unexpected idle SSE frames | 0 | 0 | Met |
| Idle TUI output bytes | 0 | 0 | Met |

Runtime RSS started at 956.31 MiB and ended at 343.34 MiB; desktop RSS started
at 109.17 MiB and ended at 79.84 MiB. The later lower resident readings do not
erase the observed peak or prove a product-level compression improvement. The
gateway peaked at 6.61 MiB with 0.047% CPU; the TUI peaked at 3.66 MiB with 0.060%
CPU. These are sampled resident values, not committed memory or allocation peaks.
The user's existing applications stayed running; this was not an exclusive lab
host. No repository build/test job overlapped this corrected five-minute window.

Grid geometry, no Controller, and `H000000` history presence were verified before
and after idle for every Session. All twelve Session IDs and PTY PIDs survived
client attachment and the subsequent echo check. The latter's p95 was 67.50 ms,
with the measurement limitations below. Normal cleanup completed; the user's
pre-existing desktop/runtime processes were rechecked and remained alive.

The full workspace regression run passed **361 Rust tests**, with six explicitly
ignored tests. The browser suite passed 15 tests and the harness suite passed
five. Formatting and workspace Clippy (`--all-targets --all-features -D warnings`)
passed. Cargo still reports a dependency future-incompatibility notice for
`block v0.1.6`; this is not a newly resolved dependency issue.

Browser here means the real observer gateway plus an HTTP/SSE consumer, not a
rendered Chromium/Safari process. Browser process RSS, the outer terminal,
Python PTY host, benchmark driver and compositor are excluded from the measured
process set. The memory contract compares runtime plus desktop; gateway and TUI
are also reported separately. The workload excludes images and wide/styled text.

An initial owner-desktop smoke and partial five-minute attempt are **not valid
reference measurements**: the desktop automatically acquired Control and resized
the twelve grids to 67x5. The partial run was deliberately ended by revoking its
fixture Share, exercising normal failure cleanup. Benchmark version 2 uses a
real Observer Share for the desktop and verifies 80x24 geometry, absence of a
Controller, and the oldest history marker before and after idle. The desktop
still uses its normal rendering/client path, but owner-only UI workflows are
not represented by this sample.

The post-idle latency check includes CLI startup, IPC, PTY echo and SSE delivery
with 50 ms observation granularity. It is not input-to-pixel, warm-attach, startup,
frame-rate or sustained-throughput evidence. A single macOS host/sample cannot
certify Linux, Windows, physical accessibility/IME, or the complete release gate.

## Follow-up and next performance work

Bounded idle compression and deadline-based actor waits are now implemented.
[The follow-up evidence](idle-history-maintenance.md) records 365 passing Rust
tests, 20 browser/harness tests, and the same five-minute release workload.
Runtime idle CPU improved to 0.323%, meeting the local <= 0.5% target. Desktop
CPU was 0.357%; idle SSE frames and TUI output stayed zero, with grids/history
and Session identities intact.

That earlier sample's peak runtime-plus-desktop RSS was **1,101.47 MiB**, missing
the memory requirement. Lower later RSS readings and physical-footprint
accounting did not substitute for the 600 MiB RSS requirement; this prompted
the native-reclamation and scheduling investigation.

A subsequent [macOS native-reclamation and read-activity follow-up](macos-history-reclamation.md)
passed 372 Rust tests and the same five-minute release workload: runtime CPU
0.317%, desktop CPU 0.377%, combined sampled peak RSS 187.44 MiB, and zero idle
SSE/TUI output. History, grids and Session/PID identity checks passed. This is the
first local reference sample meeting the harness's memory and CPU targets; it
does not certify other workloads/platforms or replace the failed historical
measurements above. The subsequent [bounded-search milestone](bounded-history-search.md)
implements actor-local incremental scanning and result backpressure, with 381
passing Rust tests and 20 browser/harness tests. Its
[post-change five-minute sample](shared-client-bounded-search-release.json)
passed all six observations with runtime CPU 0.320%, desktop CPU 0.380%, combined
peak RSS 107.63 MiB, unchanged history/grids/identity, and no idle SSE/TUI output.
The workload, exclusions and single-macOS-host limitations are unchanged.
Further performance work includes wire/client search streaming,
sustained-output/control responsiveness, startup budgets, and platform evidence.

## Latest client-queue follow-up

The [desktop coalesced-feed milestone](desktop-coalesced-feed.md) closes the
unbounded native-event-to-GPUI repaint FIFO while retaining ordered delta
application, reconnect/rejection state and terminal outcomes. Native wire/input
queues and broader authority teardown remain separate work. The full release
workspace passed 434 Rust tests plus 28 browser/harness tests and Clippy.

Its unchanged five-minute reference workload, after rebuilding all five release
executables, passed all six observations: 300.072 seconds, runtime CPU 0.310%,
desktop CPU 0.373%, combined runtime-plus-desktop sampled peak RSS 162.59 MiB,
and zero idle SSE frames/TUI bytes. Grids, history and Session/PTY identities
remained intact. The [raw release sample](shared-client-coalesced-feed-release.json)
records binary hashes and per-process data. No build/test job overlapped the
idle window. This does not establish that the mailbox caused a particular RSS
change between runs, or certify sustained output, browser-process costs, startup
budgets or other platforms. Wire/client search streaming and three-client search
UI are now implemented separately; those active-search workloads need their own
resource acceptance.

The subsequent [bounded desktop input milestone](bounded-desktop-input.md)
passed 442 Rust tests, 28 browser/harness tests and the isolated native-keyboard
smoke after rebuilding release executables. Its command/error queues are now
bounded and explicitly retired. It did not repeat this resource workload; the
measurements and binary hashes above remain evidence for the preceding build.

The later [concurrent terminal-wait milestone](concurrent-terminal-waits.md)
passed 450 Rust tests, 28 browser/harness tests, the native-keyboard smoke and
CLI multiplex smoke. It adds bounded active-wait task admission without an idle
poller. Those correctness checks do not constitute a fresh resource sample;
active-wait fan-out and other sustained workloads still need measurements.

## Pull-driven metadata release resample

After the [bounded native receive](bounded-native-receive.md) and
[pull-driven metadata](pull-driven-metadata.md) milestones, the final workspace
passed 470 Rust tests plus 28 browser/harness tests, Clippy, formatting and
native-keyboard/CLI multiplex smoke checks. All five release executables rebuilt.

The unchanged reference workload was repeated for 300.069 seconds after the
ten-second warmup. The [raw artifact](shared-client-pull-metadata-release.json)
retains binary hashes and all samples. All six observations passed: runtime
idle CPU 0.320%, desktop idle CPU 0.367%, runtime+desktop sampled peak RSS
120.33 MiB, zero idle SSE frames and zero idle TUI bytes. Gateway and TUI peak
RSS were 4.91 MiB and 2.67 MiB. The oldest history marker, reference grid and all
Session/PTY identities survived; CLI-to-SSE p95 was 68.23 ms with the same
50 ms observation granularity and exclusions described above.

No build/test job overlapped the idle window. The benchmark cleaned up its own
temporary processes and fixture tree; the user's existing desktop/runtime stayed
alive. This records a lower sampled resident value than the preceding run, not
causal proof of memory savings from the metadata refactor. Total allocation,
browser/compositor costs, sustained output/search, startup and other platforms
remain outside this evidence.
