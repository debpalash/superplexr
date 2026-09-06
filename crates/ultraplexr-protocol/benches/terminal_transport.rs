use std::{env, hint::black_box, process, time::Instant};

use ultraplexr_core::SessionId;
use ultraplexr_protocol::{ChangedRow, FrameDelta, ServerEvent, encode_terminal_event};
use ultraplexr_terminal::{Cell, CellStyle, GridSize, Rgb, Row, UnderlineStyle};

const DEFAULT_ITERATIONS: usize = 10_000;

fn main() {
    let iterations = env::var("ULTRAPLEXR_BENCH_ITERATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_ITERATIONS);
    if iterations == 0 {
        eprintln!("ULTRAPLEXR_BENCH_ITERATIONS must be greater than zero");
        process::exit(2);
    }

    let event = terminal_delta_fixture();
    for _ in 0..100 {
        let (_, encoded) = encode_terminal_event(&event).expect("fixture must encode");
        black_box(encoded);
    }

    let mut samples = Vec::with_capacity(iterations);
    let mut encoded_bytes = 0_usize;
    for _ in 0..iterations {
        let started = Instant::now();
        let (_, encoded) = encode_terminal_event(&event).expect("fixture must encode");
        samples.push(started.elapsed().as_nanos() as u64);
        encoded_bytes = encoded.len();
        black_box(encoded);
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    let total: u128 = samples.iter().copied().map(u128::from).sum();

    println!(
        concat!(
            "{{\"benchmark\":\"terminal_delta_wire_encode\",",
            "\"iterations\":{},\"changed_rows\":8,\"columns\":160,",
            "\"encoded_bytes\":{},\"mean_ns\":{},\"p50_ns\":{},",
            "\"p95_ns\":{},\"os\":\"{}\",\"arch\":\"{}\"}}"
        ),
        iterations,
        encoded_bytes,
        total / iterations as u128,
        p50,
        p95,
        env::consts::OS,
        env::consts::ARCH,
    );
}

fn terminal_delta_fixture() -> ServerEvent {
    let style = CellStyle {
        foreground: Rgb {
            red: 232,
            green: 235,
            blue: 241,
        },
        background: Rgb {
            red: 10,
            green: 12,
            blue: 16,
        },
        bold: false,
        italic: false,
        faint: false,
        blink: false,
        inverse: false,
        invisible: false,
        strikethrough: false,
        overline: false,
        underline: UnderlineStyle::None,
    };
    let changed_rows = (0_u16..8)
        .map(|index| ChangedRow {
            index,
            row: Row {
                wrapped: false,
                cells: (0..160)
                    .map(|column| Cell {
                        grapheme: char::from(b'a' + (column % 26) as u8).to_string(),
                        width: 1,
                        style_index: 0,
                        hyperlink: None,
                    })
                    .collect(),
            },
        })
        .collect();
    let session_id = SessionId::new();
    ServerEvent::TerminalDelta {
        session_id,
        delta: Box::new(FrameDelta {
            base_sequence: 41,
            sequence: 42,
            grid: GridSize::new(160, 60).expect("fixture grid must be valid"),
            changed_rows,
            styles: vec![style],
            cursor: None,
            default_foreground: style.foreground,
            default_background: style.background,
            mouse_tracking: false,
            title: Some("agent build".to_owned()),
            current_directory: Some("/workspace/project".to_owned()),
        }),
    }
}

fn percentile(samples: &[u64], percentile: usize) -> u64 {
    let index = (samples.len() - 1).saturating_mul(percentile) / 100;
    samples[index]
}
