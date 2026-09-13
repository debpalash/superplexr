//! Interactive diagnostic for native history allocation, not a release gate.
//! At each phase inspect the printed PID with `ps`/`vmmap`, then press Enter.
//! EOF exits without retaining a terminal process or touching runtime state.
use std::{
    error::Error,
    io::{self, Write},
    thread,
    time::{Duration, Instant},
};
use superplexr_terminal::{GridSize, HistoryViewport, TerminalAction, TerminalModel};

fn checkpoint(phase: &str) -> io::Result<bool> {
    println!("PHASE {phase} pid={}", std::process::id());
    io::stdout().flush()?;
    let mut input = String::new();
    Ok(io::stdin().read_line(&mut input)? != 0)
}

fn compact(models: &mut [TerminalModel]) -> Result<(), Box<dyn Error>> {
    let end = Instant::now() + Duration::from_secs(30);
    loop {
        let now = Instant::now();
        if now > end {
            return Err("history maintenance did not converge within 30 seconds".into());
        }
        let mut next: Option<Duration> = None;
        for model in &mut *models {
            if let Some(delay) = model.maintain_history(now)? {
                next = Some(next.map_or(delay, |old| old.min(delay)));
            }
        }
        match next {
            Some(delay) => thread::sleep(delay),
            None => return Ok(()),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let count = std::env::args()
        .nth(1)
        .map_or(Ok(1), |value| value.parse::<usize>())?;
    if !(1..=12).contains(&count) {
        return Err("supply 1..=12 terminal models".into());
    }
    let text = (0..100_024)
        .map(|i| format!("H{i:06} fixed retained history sample"))
        .collect::<Vec<_>>()
        .join("\r\n");
    let mut models = Vec::with_capacity(count);
    for _ in 0..count {
        let mut model = TerminalModel::new(GridSize::new(80, 24)?)?;
        for chunk in text.as_bytes().chunks(4096) {
            model.advance(TerminalAction::Output(chunk))?;
        }
        models.push(model);
    }
    drop(text);
    if !checkpoint("loaded")? {
        return Ok(());
    }
    compact(&mut models)?;
    if !checkpoint("compressed")? {
        return Ok(());
    }
    for model in &mut models {
        let rows = model.search("H", true, 100_025)?;
        if rows.len() != 100_024 || rows[0].preview != "H000000 fixed retained history sample" {
            return Err("reference history was not preserved".into());
        }
        let frame = model.frame_at_history_viewport(HistoryViewport::RowFromTop(0))?;
        if frame.rows[0].text() != "H000000 fixed retained history sample" {
            return Err("oldest viewport lost".into());
        }
        model.frame_at_history_viewport(HistoryViewport::RowsBeforeBottom(0))?;
    }
    if !checkpoint("restored-and-verified")? {
        return Ok(());
    }
    compact(&mut models)?;
    if !checkpoint("recompressed")? {
        return Ok(());
    }
    drop(models);
    checkpoint("dropped")?;
    Ok(())
}
