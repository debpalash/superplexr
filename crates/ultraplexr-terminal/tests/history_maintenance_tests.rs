use std::time::{Duration, Instant};
use ultraplexr_terminal::{
    GridSize, HistoryViewport, SelectionPoint, TerminalAction, TerminalModel,
};

fn populated() -> TerminalModel {
    let mut terminal = TerminalModel::new(GridSize::new(80, 24).expect("grid")).expect("terminal");
    let text = (0..100_024)
        .map(|i| format!("H{i:06} retained history"))
        .collect::<Vec<_>>()
        .join("\r\n");
    for chunk in text.as_bytes().chunks(4096) {
        terminal
            .advance(TerminalAction::Output(chunk))
            .expect("PTY output");
    }
    terminal
}

fn finish(terminal: &mut TerminalModel, now: &mut Instant) -> usize {
    for step in 0..2048 {
        match terminal.maintain_history(*now).expect("maintenance") {
            None => return step,
            Some(delay) => {
                assert!(delay > Duration::ZERO, "never schedule a busy loop");
                *now += delay;
            }
        }
    }
    panic!("bounded reference maintenance must converge");
}

#[test]
fn idle_maintenance_is_incremental_preserves_history_and_stops_until_activity() {
    let mut terminal = populated();
    let mut now = Instant::now();
    let before = terminal.frame().expect("live frame");
    let delay = terminal
        .maintain_history(now)
        .expect("schedule")
        .expect("initial idle delay");
    assert_eq!(
        terminal.maintain_history(now).expect("too early"),
        Some(delay)
    );
    now += delay;
    assert!(
        finish(&mut terminal, &mut now) > 1,
        "large history must take incremental steps"
    );
    assert_eq!(terminal.frame().expect("unchanged live frame"), before);
    now += Duration::from_secs(60);
    assert_eq!(terminal.maintain_history(now).expect("idle"), None);

    let historical = terminal
        .frame_at_history_viewport(HistoryViewport::RowsBeforeBottom(100_000))
        .expect("compressed history");
    assert_eq!(historical.rows[0].text(), "H000000 retained history");
    assert_eq!(
        terminal
            .search("H", true, 100_025)
            .expect("full history")
            .len(),
        100_024
    );
    assert!(
        finish(&mut terminal, &mut now) > 1,
        "restored history must schedule another pass"
    );
    terminal
        .advance(TerminalAction::Resize {
            grid: GridSize::new(100, 24).expect("wider grid"),
            cell_width_px: 8,
            cell_height_px: 16,
        })
        .expect("resize compressed history");
    assert!(
        finish(&mut terminal, &mut now) > 0,
        "resize activity must reschedule"
    );
    assert_eq!(
        terminal
            .search("H000000", true, 1)
            .expect("oldest after resize")
            .len(),
        1
    );
    terminal
        .advance(TerminalAction::Output(b"\r\nAFTER-MAINTENANCE"))
        .expect("new output");
    finish(&mut terminal, &mut now);
    assert_eq!(
        terminal
            .search("AFTER-MAINTENANCE", true, 1)
            .expect("new output survives")
            .len(),
        1
    );
}

#[test]
fn maintenance_preserves_styled_unicode_selection_and_alternate_screen_history() {
    let mut terminal = TerminalModel::new(GridSize::new(80, 24).expect("grid")).expect("terminal");
    let text = (0..5000)
        .map(|i| format!("\x1b[31mH{i:06} 界 e\u{301}\x1b[0m\r\n"))
        .collect::<String>();
    terminal
        .advance(TerminalAction::Output(text.as_bytes()))
        .expect("styled Unicode history");
    terminal
        .frame_at_history_viewport(HistoryViewport::RowFromTop(0))
        .expect("oldest viewport");
    terminal
        .advance(TerminalAction::Select {
            anchor: SelectionPoint { column: 0, row: 0 },
            head: SelectionPoint { column: 15, row: 0 },
            rectangle: false,
        })
        .expect("select old history");
    let frame = terminal.frame().expect("styled selection");
    let selected = terminal
        .selected_text()
        .expect("selection")
        .expect("selected text");
    assert!(selected.contains("界 e\u{301}"));
    let mut now = Instant::now();
    assert!(finish(&mut terminal, &mut now) > 1);
    assert_eq!(terminal.frame().expect("preserved frame"), frame);
    assert_eq!(
        terminal.selected_text().expect("preserved selection"),
        Some(selected)
    );
    assert!(
        finish(&mut terminal, &mut now) > 0,
        "clipboard formatting schedules idle maintenance"
    );
    assert_eq!(
        terminal.maintain_history(now).expect("idle after copy"),
        None
    );
    terminal
        .advance(TerminalAction::Output(b"\x1b[?1049hALT-SCREEN"))
        .expect("alternate screen");
    finish(&mut terminal, &mut now);
    assert!(
        terminal
            .frame()
            .expect("alternate frame")
            .rows
            .iter()
            .any(|row| row.text().contains("ALT-SCREEN"))
    );
    terminal
        .advance(TerminalAction::Output(b"\x1b[?1049l"))
        .expect("primary screen");
    finish(&mut terminal, &mut now);
    assert_eq!(
        terminal
            .search("H000000", true, 1)
            .expect("primary history")
            .len(),
        1
    );
    assert!(
        terminal
            .search("ALT-SCREEN", true, 1)
            .expect("no alternate leakage")
            .is_empty()
    );
}

#[test]
fn repeated_reclamation_across_threads_and_page_sizes_preserves_each_terminal() {
    // Exercise the real native mapping path, not Zig's test-allocator fallback.
    // Different widths change native page allocations; concurrent owners must
    // not disturb each other's live cells or retained history when remapping.
    let workers = [80, 81, 132, 257].map(|columns| {
        std::thread::spawn(move || {
            for generation in 0..4 {
                let mut terminal = TerminalModel::new(GridSize::new(columns, 24).expect("grid"))
                    .expect("terminal");
                let prefix = format!("W{columns}-G{generation}-");
                let text = (0..8000)
                    .map(|row| format!("{prefix}R{row:05} 界 e\u{301}"))
                    .collect::<Vec<_>>()
                    .join("\r\n");
                for chunk in text.as_bytes().chunks(4096) {
                    terminal
                        .advance(TerminalAction::Output(chunk))
                        .expect("output");
                }
                let live = terminal.frame().expect("live cells");
                let mut now = Instant::now();
                for cycle in 0..4 {
                    let steps = finish(&mut terminal, &mut now);
                    assert!(
                        steps > 1,
                        "width={columns} generation={generation} cycle={cycle} steps={steps}"
                    );
                    assert_eq!(
                        terminal.maintain_history(now).expect("idle before search"),
                        None
                    );
                    assert_eq!(terminal.frame().expect("unchanged live cells"), live);
                    assert_eq!(
                        terminal
                            .search(&prefix, true, 8001)
                            .expect("all retained rows")
                            .len(),
                        8000
                    );
                    for row in [0, 1999, 3999, 7999] {
                        assert_eq!(
                            terminal
                                .search(&format!("{prefix}R{row:05} 界 e\u{301}"), true, 2)
                                .expect("exact retained row")
                                .len(),
                            1
                        );
                    }
                }
                finish(&mut terminal, &mut now);
                // Drop compressed mappings before the next independent owner.
            }
        })
    });
    for worker in workers {
        worker.join().expect("native mapping worker");
    }
}
