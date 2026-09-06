//! Measure what one terminal subscription puts on the wire.
//!
//! Prints full-frame and delta bytes over a window, which is the number that
//! sizes a web or mobile client.
use std::time::{Duration, Instant};

use ultraplexr_client::ControlClient;
use ultraplexr_protocol::{ServerEvent, encode_terminal_event};

fn main() {
    let mut args = std::env::args().skip(1);
    let socket = args
        .next()
        .expect("usage: delta_bandwidth <socket> <session-id> [seconds]");
    let session: ultraplexr_core::SessionId = args
        .next()
        .expect("session id")
        .parse()
        .expect("valid session id");
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
    let max_hz: Option<u16> = args.next().and_then(|s| s.parse().ok());
    let client = ControlClient::connect(std::path::Path::new(&socket)).expect("connect");
    let mut terminal = client.terminal(session);
    if let Some(hz) = max_hz {
        terminal = terminal.with_max_hz(hz);
    }
    let events = terminal.subscribe().expect("subscribe");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let (mut frames, mut deltas, mut frame_bytes, mut delta_bytes) = (0u64, 0u64, 0u64, 0u64);
    // What the wire actually carries for frames is the protobuf data plane;
    // JSON is kept only for comparison.
    let mut json_bytes = 0u64;
    while Instant::now() < deadline {
        let Ok(event) = events.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        else {
            break;
        };
        json_bytes += serde_json::to_vec(&event).map_or(0, |v| v.len() as u64);
        let bytes = encode_terminal_event(&event).map_or(0, |(_, v)| v.len() as u64);
        match event {
            ServerEvent::TerminalFrame { .. } => {
                frames += 1;
                frame_bytes += bytes;
            }
            ServerEvent::TerminalDelta { .. } => {
                deltas += 1;
                delta_bytes += bytes;
            }
            _ => {}
        }
    }
    let total = (frame_bytes + delta_bytes) as f64 / 1024.0;
    println!(
        "{seconds}s window: {frames} full frames ({} KB), {deltas} deltas ({} KB) → wire {:.0} KB/s ({:.1} events/s, avg delta {} B); same events as JSON would be {:.0} KB/s",
        frame_bytes / 1024,
        delta_bytes / 1024,
        total / seconds as f64,
        (frames + deltas) as f64 / seconds as f64,
        if deltas > 0 {
            delta_bytes.checked_div(deltas).unwrap_or(0)
        } else {
            0
        },
        json_bytes as f64 / 1024.0 / seconds as f64
    );
}
