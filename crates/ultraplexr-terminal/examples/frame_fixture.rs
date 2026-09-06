use ultraplexr_terminal::{GhosttyBuild, GridSize, TerminalAction, TerminalModel};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = TerminalModel::new(GridSize::new(20, 6)?)?;
    let effects = terminal.advance(TerminalAction::Output(
        b"\x1b[2J\x1b[Hultraplexr terminal\r\n\x1b[1;32mghostty ready\x1b[0m \xE2\x9C\x93",
    ))?;
    let frame = terminal.frame()?;

    eprintln!("ghostty: {:?}", GhosttyBuild::current()?);
    eprintln!("effects: {effects:?}");
    println!("{}", serde_json::to_string_pretty(&frame)?);
    Ok(())
}
