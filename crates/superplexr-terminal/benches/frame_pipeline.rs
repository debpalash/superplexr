use std::{env, process, time::Instant};

use superplexr_terminal::{GridSize, TerminalAction, TerminalModel};

const DEFAULT_ITERATIONS: usize = 1_000;
const WARMUP_ITERATIONS: usize = 100;

fn main() {
    let iterations = env::var("SUPERPLEXR_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_ITERATIONS);
    if iterations == 0 {
        eprintln!("SUPERPLEXR_BENCH_ITERATIONS must be greater than zero");
        process::exit(2);
    }

    let mut terminal = TerminalModel::new(
        GridSize::new(160, 60).expect("benchmark grid must satisfy the product contract"),
    )
    .expect("pinned terminal must initialize");

    for iteration in 0..WARMUP_ITERATIONS {
        exercise(&mut terminal, iteration);
    }

    let mut samples = Vec::with_capacity(iterations);
    for iteration in 0..iterations {
        let started = Instant::now();
        exercise(&mut terminal, iteration);
        samples.push(started.elapsed().as_nanos() as u64);
    }
    samples.sort_unstable();

    let total: u128 = samples.iter().copied().map(u128::from).sum();
    let mean = total / iterations as u128;
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    let p99 = percentile(&samples, 99);

    println!(
        concat!(
            "{{\"benchmark\":\"terminal_output_to_full_frame\",",
            "\"iterations\":{},\"grid\":\"160x60\",",
            "\"min_ns\":{},\"mean_ns\":{},\"p50_ns\":{},",
            "\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{},",
            "\"os\":\"{}\",\"arch\":\"{}\"}}"
        ),
        iterations,
        samples[0],
        mean,
        p50,
        p95,
        p99,
        samples[iterations - 1],
        env::consts::OS,
        env::consts::ARCH,
    );

    if let Some(limit_us) = env::var("SUPERPLEXR_BENCH_P95_US")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        && p95 > limit_us.saturating_mul(1_000)
    {
        eprintln!("p95 {p95} ns exceeded configured limit {limit_us} us");
        process::exit(1);
    }
}

fn exercise(terminal: &mut TerminalModel, iteration: usize) {
    let payload = format!(
        "\x1b[H\x1b[38;5;75msuperplexr\x1b[0m frame {iteration:06} · agent output 界\r\n{}",
        "responsive terminal workload ".repeat(12)
    );
    terminal
        .advance(TerminalAction::Output(payload.as_bytes()))
        .expect("benchmark output should parse");
    let frame = terminal.frame().expect("benchmark frame should translate");
    std::hint::black_box(frame);
}

fn percentile(samples: &[u64], percentile: usize) -> u64 {
    let index = (samples.len() - 1).saturating_mul(percentile) / 100;
    samples[index]
}
