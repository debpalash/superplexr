use super::*;
use std::sync::Arc;
use superplexr_terminal::{Cell, Cursor, CursorShape, GridSize, Row};

fn frame() -> FullFrame {
    FullFrame {
        sequence: 1,
        grid: GridSize {
            columns: 4,
            rows: 1,
        },
        rows: vec![Arc::new(Row {
            wrapped: false,
            cells: vec![
                Cell {
                    grapheme: "界".into(),
                    width: 2,
                    style_index: 0,
                    hyperlink: Some("\x1b]52;evil".into()),
                },
                Cell {
                    grapheme: "".into(),
                    width: 0,
                    style_index: 0,
                    hyperlink: None,
                },
                Cell {
                    grapheme: "e\u{301}".into(),
                    width: 1,
                    style_index: 0,
                    hyperlink: None,
                },
                Cell {
                    grapheme: "\x1b\x07\n".into(),
                    width: 1,
                    style_index: 0,
                    hyperlink: None,
                },
            ],
        })],
        styles: vec![],
        cursor: Some(Cursor {
            column: 2,
            row: 0,
            shape: CursorShape::Block,
            blinking: false,
        }),
        default_foreground: Rgb {
            red: 255,
            green: 255,
            blue: 255,
        },
        default_background: Rgb {
            red: 0,
            green: 0,
            blue: 0,
        },
        mouse_tracking: false,
        title: Some("\x1b]52;evil".into()),
        current_directory: None,
    }
}

#[test]
fn renderer_preserves_graphemes_and_never_replays_remote_osc() {
    let mut bytes = Vec::new();
    draw(&mut bytes, Some(&frame()), None, (4, 2), "safe", true).expect("draw");
    let output = String::from_utf8(bytes).expect("UTF8");
    assert!(output.contains("界"));
    assert!(output.contains("e\u{301}"));
    assert!(!output.contains("\x1b]"));
    assert!(!output.contains('\x07'));
    assert!(!output.contains("evil"));
}

#[test]
fn unchanged_rows_are_not_repainted_and_wide_cells_do_not_cross_clip() {
    let frame = frame();
    let mut bytes = Vec::new();
    draw(
        &mut bytes,
        Some(&frame),
        Some(&frame),
        (4, 2),
        "status",
        true,
    )
    .expect("draw");
    assert!(!String::from_utf8(bytes).expect("UTF8").contains("界"));
    let mut bytes = Vec::new();
    draw(&mut bytes, Some(&frame), None, (1, 2), "", false).expect("clip");
    assert!(!String::from_utf8(bytes).expect("UTF8").contains("界"));
}

#[test]
fn rendered_wide_combining_cells_and_cursor_survive_outer_vt_interpretation() {
    use superplexr_terminal::{TerminalAction, TerminalModel};
    let mut outer = TerminalModel::new(GridSize {
        columns: 4,
        rows: 2,
    })
    .expect("outer VT");
    let mut bytes = Vec::new();
    draw(&mut bytes, Some(&frame()), None, (4, 2), "safe", true).expect("render");
    outer
        .advance(TerminalAction::Output(&bytes))
        .expect("interpret");
    let painted = outer.frame().expect("painted");
    assert_eq!(painted.rows[0].cells[0].grapheme, "界");
    assert_eq!(painted.rows[0].cells[0].width, 2);
    assert_eq!(painted.rows[0].cells[2].grapheme, "e\u{301}");
    assert_eq!(painted.cursor.expect("cursor").column, 2);
}

#[test]
fn repainting_one_pane_never_erases_its_neighbor() {
    use superplexr_terminal::{TerminalAction, TerminalModel};
    let mut outer = TerminalModel::new(GridSize {
        columns: 10,
        rows: 3,
    })
    .expect("outer VT");
    let left = Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 3,
    };
    let right = Rect {
        x: 5,
        y: 0,
        width: 5,
        height: 3,
    };
    let mut bytes = Vec::new();
    draw_pane(&mut bytes, Some(&frame()), None, left, "LEFT", false).expect("left");
    draw_pane(&mut bytes, Some(&frame()), None, right, "RIGHT", true).expect("right");
    draw_pane(&mut bytes, None, None, right, "CLEAR", false).expect("clear right");
    outer
        .advance(TerminalAction::Output(&bytes))
        .expect("interpret");
    let output = outer.frame().expect("frame");
    assert_eq!(output.rows[0].cells[0].grapheme, "界");
    assert_eq!(output.rows[0].cells[2].grapheme, "e\u{301}");
    let cells = &output.rows[2].cells;
    assert_eq!(
        cells[..4]
            .iter()
            .map(|cell| cell.grapheme.as_str())
            .collect::<String>(),
        "LEFT"
    );
    assert!(cells[4].grapheme.is_empty() || cells[4].grapheme == " ");
    assert_eq!(
        cells[5..10]
            .iter()
            .map(|cell| cell.grapheme.as_str())
            .collect::<String>(),
        "CLEAR"
    );
}

#[test]
fn search_view_is_cell_clipped_sanitized_and_focus_visible_in_small_grids() {
    use superplexr_terminal::{SearchMatch, TerminalAction, TerminalModel};
    let mut panel = crate::search::Panel::new(superplexr_core::SessionId::new());
    panel.query = "界 é\x1b]52;c;bad\x07".into();
    panel.matches.push(SearchMatch {
        line: 9,
        column: 2,
        preview: "界 é\x1b]52;c;bad\x07".into(),
    });
    for (columns, rows) in [(1, 1), (2, 2), (4, 3), (20, 5), (80, 25)] {
        let mut bytes = Vec::new();
        draw_search(&mut bytes, &panel, (columns, rows)).expect("render");
        assert!(!String::from_utf8_lossy(&bytes).contains("\x1b]"));
        assert!(!bytes.contains(&7));
        let mut outer = TerminalModel::new(GridSize { columns, rows }).expect("outer");
        outer
            .advance(TerminalAction::Output(&bytes))
            .expect("interpret");
        let frame = outer.frame().expect("frame");
        if rows >= 5 {
            assert!(frame.rows[3].text().starts_with("> 10:3"));
        }
    }
}
