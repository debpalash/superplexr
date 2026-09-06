use ultraplexr_terminal::{GridSize, HistoryViewport, TerminalAction, TerminalModel};

fn model(rows: usize) -> TerminalModel {
    let mut model = TerminalModel::new(GridSize::new(300, 24).expect("grid")).expect("model");
    let text = (0..rows)
        .map(|row| format!("ROW-{row:06} retained 界 e\u{301}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    for chunk in text.as_bytes().chunks(4096) {
        model
            .advance(TerminalAction::Output(chunk))
            .expect("output");
    }
    model
}

#[test]
fn bounded_batches_find_complete_history_without_changing_the_view() {
    let mut model = model(4000);
    model
        .frame_at_history_viewport(HistoryViewport::RowsBeforeBottom(100))
        .expect("personal viewport");
    let before = model.frame().expect("frame");
    let mut search = model
        .begin_search("retained 界 e\u{301}", true, 5000)
        .expect("start");
    let mut results = Vec::new();
    let mut steps = 0;
    loop {
        let batch = model.search_step(&mut search).expect("batch");
        assert!(batch.rows_scanned <= 32);
        assert!(batch.matches.len() <= 64);
        steps += 1;
        results.extend(batch.matches);
        if batch.complete {
            break;
        }
        assert!(steps < 1000);
    }
    assert!(steps > 100);
    assert_eq!(results.len(), 4000);
    assert_eq!(results.first().expect("oldest").line, 0);
    assert_eq!(results.last().expect("newest").line, 3999);
    assert_eq!(model.frame().expect("unchanged view"), before);
}

#[test]
fn dense_matches_resume_within_a_row_and_folded_columns_refer_to_original_text() {
    let mut model = model(0);
    model
        .advance(TerminalAction::Output(
            format!("İx{}", "a".repeat(250)).as_bytes(),
        ))
        .expect("dense output");
    let mut search = model.begin_search("a", true, 200).expect("dense search");
    let mut matches = Vec::new();
    loop {
        let batch = model.search_step(&mut search).expect("dense batch");
        assert!(batch.matches.len() <= 64);
        matches.extend(batch.matches);
        if batch.complete {
            break;
        }
    }
    assert_eq!(matches.len(), 200);
    for (index, found) in matches.iter().enumerate() {
        assert_eq!(found.column, index + 2);
    }
    let mut folded = model.begin_search("X", false, 1).expect("folded");
    assert_eq!(
        model
            .search_step(&mut folded)
            .expect("folded batch")
            .matches[0]
            .column,
        1
    );
}

#[test]
fn append_does_not_expand_search_but_resize_reset_screen_changes_and_wrong_models_fail() {
    let mut model = model(1000);
    let mut search = model.begin_search("ROW", true, 2000).expect("start");
    let mut count = model.search_step(&mut search).expect("first").matches.len();
    model
        .advance(TerminalAction::Output(b"\r\nROW-APPENDED"))
        .expect("append");
    loop {
        let batch = model
            .search_step(&mut search)
            .expect("append preserves refs");
        count += batch.matches.len();
        if batch.complete {
            break;
        }
    }
    assert_eq!(count, 1000);
    let mut search = model.begin_search("ROW", true, 2000).expect("start");
    let other = TerminalModel::new(GridSize::new(300, 24).expect("grid")).expect("other");
    assert!(other.search_step(&mut search).is_err());
    model
        .advance(TerminalAction::Resize {
            grid: GridSize::new(301, 24).expect("resize"),
            cell_width_px: 8,
            cell_height_px: 16,
        })
        .expect("resize");
    assert!(model.search_step(&mut search).is_err());
    let mut search = model.begin_search("ROW", true, 2000).expect("start");
    model
        .advance(TerminalAction::Output(b"\x1b[?1049h"))
        .expect("alternate screen");
    assert!(model.search_step(&mut search).is_err());
    model
        .advance(TerminalAction::Output(b"\x1b[?1049l"))
        .expect("primary screen");
    let mut search = model.begin_search("ROW", true, 2000).expect("start");
    model
        .advance(TerminalAction::Output(b"\x1bc"))
        .expect("reset");
    assert!(model.search_step(&mut search).is_err());
}

#[test]
fn empty_queries_and_zero_limits_finish_and_oversize_queries_fail() {
    let model = model(100);
    for (query, limit) in [("", 100), ("ROW", 0)] {
        let mut search = model.begin_search(query, true, limit).expect("empty");
        let batch = model.search_step(&mut search).expect("done");
        assert!(batch.complete);
        assert_eq!(batch.rows_scanned, 0);
        assert!(batch.matches.is_empty());
    }
    assert!(model.begin_search(&"a".repeat(1025), true, 1).is_err());
}

#[test]
fn pruning_invalidates_the_coordinate_origin_instead_of_relabeling_matches() {
    let mut model = model(1000);
    let mut search = model.begin_search("ROW", true, 2000).expect("search");
    model.search_step(&mut search).expect("initial page");
    let text = "evict old history\r\n".repeat(110_000);
    for chunk in text.as_bytes().chunks(4096) {
        model
            .advance(TerminalAction::Output(chunk))
            .expect("pruning output");
    }
    assert!(model.search_step(&mut search).is_err());
}

#[test]
fn large_unicode_rows_shrink_batches_and_oversized_single_rows_fail_explicitly() {
    let mut model = model(0);
    let cell = format!("a{}", "\u{301}".repeat(64));
    let text = (0..40)
        .map(|row| format!("R{row:02}{}", cell.repeat(290)))
        .collect::<Vec<_>>()
        .join("\r\n");
    model
        .advance(TerminalAction::Output(text.as_bytes()))
        .expect("large grapheme rows");
    let mut search = model.begin_search("R", true, 100).expect("search");
    let mut count = 0;
    let mut empty_yields = 0;
    for _ in 0..1000 {
        let page = model
            .search_step(&mut search)
            .expect("adaptive bounded formatting");
        assert!(page.rows_scanned <= 32);
        if page.matches.is_empty() && !page.complete {
            empty_yields += 1;
        }
        count += page.matches.len();
        if page.complete {
            break;
        }
    }
    assert!(empty_yields > 0, "capacity retries must yield to the actor");
    assert_eq!(count, 40);
    let mut large =
        TerminalModel::new(GridSize::new(1000, 24).expect("wide grid")).expect("large model");
    large
        .advance(TerminalAction::Output(cell.repeat(999).as_bytes()))
        .expect("oversized row");
    let mut search = large.begin_search("missing", true, 1).expect("start");
    let mut refused = false;
    for _ in 0..10 {
        match large.search_step(&mut search) {
            Err(ultraplexr_terminal::TerminalError::SearchRowTooLarge) => {
                refused = true;
                break;
            }
            Ok(page) => assert!(!page.complete),
            other => panic!("unexpected result: {other:?}"),
        }
    }
    assert!(refused, "never allocate or truncate an unbounded row");
}
