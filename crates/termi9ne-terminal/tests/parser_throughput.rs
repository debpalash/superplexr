//! Guards the build configuration of the vendored VT library.
//!
//! `libghostty-vt-sys` builds its Zig source in `Debug` mode whenever cargo sets
//! `DEBUG=true`, which is every dev build — so `cargo run` shipped a parser with
//! no optimization and full safety checks, managing well under 1 MiB/s. That
//! parser sees every byte of every terminal: rebuilding one screen from a journal
//! took minutes and left the daemon, and every client waiting on it, unresponsive.
//!
//! `.cargo/config.toml` sets `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast` to correct
//! this. That file is easy to lose in a merge or a fresh clone, and nothing else
//! would notice, so this test asserts the parser is actually optimized.

use std::time::Instant;
use termi9ne_terminal::{GridSize, TerminalAction, TerminalModel};

/// Floor for accepting the parser as optimized.
///
/// An unoptimized build measures under 1 MiB/s and an optimized one over
/// 400 MiB/s on the same input, so this sits orders of magnitude away from both
/// and does not depend on the speed of the machine running it.
const MINIMUM_THROUGHPUT_MIB_PER_SECOND: f64 = 20.0;

fn throughput_mib_per_second(data: &[u8]) -> f64 {
    let grid = GridSize::new(80, 24).expect("grid should be valid");
    let mut model = TerminalModel::new(grid).expect("model should build");
    let started = Instant::now();
    for chunk in data.chunks(64 * 1024) {
        model
            .advance(TerminalAction::Output(chunk))
            .expect("output should parse");
    }
    let elapsed = started.elapsed().as_secs_f64();
    (data.len() as f64 / (1024.0 * 1024.0)) / elapsed
}

#[test]
fn the_vendored_parser_is_built_optimized() {
    // Ordinary scrolling output: the shape that was slowest unoptimized,
    // because cost was dominated by per-line work rather than by parsing.
    let mut lines = Vec::new();
    while lines.len() < 4 * 1024 * 1024 {
        lines.extend_from_slice(b"hello world this is a fairly ordinary line of output\r\n");
    }
    let rate = throughput_mib_per_second(&lines);
    assert!(
        rate >= MINIMUM_THROUGHPUT_MIB_PER_SECOND,
        "terminal parser managed {rate:.2} MiB/s on plain scrolling output. \
         The vendored Zig VT library is almost certainly built in Debug mode: \
         check that .cargo/config.toml still sets LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseFast."
    );
}
