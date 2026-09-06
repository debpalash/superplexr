# Local restart and event-streaming baseline

Measured 2026-09-05 on macOS arm64, Apple M2 (8 logical CPUs), optimized
development builds, not release. These are small-workload development samples,
not certified budgets or evidence of public remote readiness.

## Real desktop restart

The explicit `native_desktop_restart_preserves_personal_state_and_live_sessions`
test launched the actual desktop binary twice against an isolated real daemon.
Both launches rendered a window, sampled idle performance, saved, and exited.
Workspace/session names and pins survived; a dismissed Mission stayed hidden.
A new Mission created between launches was discovered without stealing focus.
The runtime retained exactly one terminal with the original Session ID and PTY
PID. No user daemon or saved workspace file was used.

| Sample | Startup to first render | Desktop idle RSS | Desktop idle CPU |
| --- | ---: | ---: | ---: |
| First launch | 163.9 ms | 112.2 MiB | 0.77% |
| Second launch | 131.3 ms | 112.7 MiB | 1.44% |

Each launch used the built-in six-second idle benchmark and one visible terminal.
Names/pins were preseeded; this is not a native menu-click automation test.

## HTTP event delivery

Reproduce from the repository root:

```sh
cargo build -p ultraplexr-server -p ultraplexr-cli -p ultraplexr-observer
node ci/observer-stream-benchmark.mjs
```

The script creates and removes its own daemon, `/bin/cat` terminal (80×24),
Observer Share, gateway, and temporary state. It measures five seconds of idle
time and 50 echoed markers, with the browser's two-second session-list polling
cadence included. Raw samples are in [the JSON record](observer-streaming-baseline.json).

| Component | Idle RSS | Idle CPU | Streaming RSS | Streaming CPU |
| --- | ---: | ---: | ---: | ---: |
| Runtime | 11.34 MiB | 0.20% | 12.39 MiB | 3.02% |
| Observer gateway | 6.20 MiB | 0.20% | 6.64 MiB | 0.75% |

Zero terminal frames arrived during the idle window. Marker latency was **34.97
ms median, 36.64 ms p95, 36.89 ms maximum**. This includes CLI process launch,
IPC, PTY echo, and HTTP SSE delivery, with 25 ms observation granularity; it is
not browser-paint latency. The terminal count remained one.

## Correctness evidence and limits

- 120 Rust tests passed across desktop/client/protocol/observer, plus the
  separately invoked native restart test. Eight JavaScript stream tests passed.
  Clippy passed with warnings denied (Cargo still reports a pre-existing future
  compatibility notice for the external `block` dependency).
- Real-daemon fault injection covers native-link loss, output during outage,
  canonical-frame repair, retained history, HTTP detach/reattach, unchanged
  Session/PTY identity, and live Share revocation. No reconnect path sends input
  or starts a replacement terminal.
- ego-lite checks verified local pause, immediate latest-frame resume, earlier
  output, back-to-live, revocation clearing, and 1280×800 / 390×844 layouts.
  Forced disconnect correctness is established by the automated relay/HTTP
  tests; browser network emulation alone is not proof of an existing SSE drop.
- Browser process memory, full agent workloads, sustained large output, many
  viewers, WAN/tunnel latency, browser paint, Linux/Windows, and long-duration
  leak/retention behavior were not measured here. CPU is a short sampled process
  CPU-time delta; the figures vary with host load and measurement resolution.
- Gateway frame queues are bounded/coalesced; this does not certify all native
  queues. Retained host history is bounded, and coalescing deliberately skips
  intermediate screens. Scope checks remain mandatory on reattachment.
- This measurement predates the optional Controller input/handoff and focused
  TUI milestones. Their existence does not extend these performance results;
  see the [browser Control evidence](browser-control-evidence.md) and
  [TUI design](../design/focused-session-tui.md). Public remote transport and TLS
  remain separate deliverables. The optional browser still binds only loopback.
