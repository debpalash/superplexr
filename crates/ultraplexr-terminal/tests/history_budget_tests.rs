use ultraplexr_terminal::{
    DEFAULT_SCROLLBACK_LINES, GridSize, HistoryViewport, TerminalAction, TerminalModel,
};

#[test]
fn reference_scrollback_survives_a_real_hundred_thousand_line_workload() {
    check_reference_history(usize::MAX);
}

#[test]
fn reference_scrollback_survives_chunked_pty_output() {
    check_reference_history(4096);
}

fn check_reference_history(chunk_size: usize) {
    let mut terminal =
        TerminalModel::new(GridSize::new(80, 24).expect("reference grid")).expect("terminal");
    // No final newline: it would add a blank live row, exceed the history
    // allowance by one, and legitimately prune an entire backend page.
    let text = (0..DEFAULT_SCROLLBACK_LINES + 24)
        .map(|i| format!("H{i:06} fixed retained history sample"))
        .collect::<Vec<_>>()
        .join("\r\n");
    for chunk in text.as_bytes().chunks(chunk_size) {
        terminal
            .advance(TerminalAction::Output(chunk))
            .expect("reference output");
    }
    assert_eq!(
        terminal
            .search("H100023", true, 1)
            .expect("last marker")
            .len(),
        1
    );
    assert_eq!(
        terminal
            .search("H000000", true, 1)
            .expect("deep marker")
            .len(),
        1,
        "the configured line allowance must not be silently truncated by a smaller byte default"
    );
    let historical = terminal
        .frame_at_history_viewport(HistoryViewport::RowsBeforeBottom(100000))
        .expect("deep history frame");
    assert_eq!(
        historical.rows[0].text(),
        "H000000 fixed retained history sample",
        "old history must be reachable independently of search"
    );
    assert_eq!(
        terminal
            .search("H", true, DEFAULT_SCROLLBACK_LINES + 25)
            .expect("all retained rows")
            .len(),
        DEFAULT_SCROLLBACK_LINES + 24,
        "all 100,000 history rows plus the live viewport must be retained"
    );

    // Exceeding the cap is expected to prune complete pages. Keep that behavior
    // distinct from losing history before reaching the configured allowance.
    terminal
        .advance(TerminalAction::Output(b"\r\nOVERFLOW"))
        .expect("cross line allowance");
    let retained = terminal
        .search("H", true, DEFAULT_SCROLLBACK_LINES + 25)
        .expect("bounded retained rows");
    assert!(retained.len() < DEFAULT_SCROLLBACK_LINES + 24);
    assert!(
        retained.len() >= 99_000,
        "reference pruning must stay page-granular"
    );
    assert_eq!(
        retained.last().expect("history survives").preview,
        "H100023 fixed retained history sample"
    );
}
