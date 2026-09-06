# M1 benchmark evidence

## Current desktop output gate

`ci/desktop-render-benchmark.sh` starts an isolated daemon, six `/bin/sh` PTYs,
and one six-Surface waterfall, drives continuous ANSI output through every PTY,
measures actual GPUI draw completions for five seconds after a warm-up frame,
emits both raw callback cadence and display-normalized presented FPS, and fails
below `ULTRAPLEXR_DESKTOP_MIN_FPS` (60 by default) or above the 16.7 ms p95 frame
budget. It cleans up only its unique
temporary state root and daemon. This is a graphical-host gate and therefore
runs on the physical macOS, Wayland, and X11 release hosts rather than in a
headless unit-test process.

The 2026-09-01 Apple M2 local run reported 60.00 display-normalized FPS, 58.58
raw callbacks per second, and 5.042 ms p95 draw time with six visible Surfaces.
The renderer consumed 30.3% of its 16.667 ms frame budget. A separate
twelve-Surface quiet run reported 0.54% desktop CPU, 0.0% runtime CPU,
124.6 MiB combined RSS, and 207.5 ms startup-to-first-render. The short quiet
run is a deterministic regression gate; the five-minute idle averages required
by section 08 still belong to the physical reference-host matrix. The terminal model
benchmark reported 99.833 µs p95
for output-to-160×60-frame translation. After the protocol-v3 migration, the
release protobuf benchmark reported 70.041 µs p95 and 9,131 encoded bytes for
an eight-row 160-column delta across 10,000 samples. These are local
measurements, not substitutes for the full reference-host matrix. Production
signing and notarization are explicitly deferred from the unsigned preview.

The M1 harness measures one serialized terminal mutation plus owned full-frame
translation at a 160×60 grid. It intentionally excludes GPUI presentation and
PTY I/O; those become end-to-end latency spans in M2.

Run the release harness with:

```sh
cargo bench -p ultraplexr-terminal --bench frame_pipeline
```

Machine-readable JSON is printed to stdout. `ULTRAPLEXR_BENCH_ITERATIONS` changes
the sample count and `ULTRAPLEXR_BENCH_P95_US` turns a recorded p95 budget into a
failing gate. Reference-host definitions live in `ci/reference-hardware.toml`.

## Apple Silicon baseline

- Date: 2026-08-31
- Host: MacBook Air, Apple M2 (4 performance + 4 efficiency cores), 16 GiB
- OS: macOS 26.4.1, arm64
- Toolchain: Rust 1.97.1, Zig 0.16.0
- Samples: 1,000 after 100 warmups, 160×60 full-frame translation
- Result: p50 0.881 ms, p95 1.253 ms, p99 1.455 ms, mean 0.986 ms
- Raw maximum: 22.725 ms (reported, not discarded)

Linux physical-GPU baselines remain open and must be recorded separately for
Wayland and X11. Container timings are useful regression signals but are not a
replacement for those reference hosts.
