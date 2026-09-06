use ultraplexr_terminal::{
    CursorShape, FullFrame, GridSize, Rgb, TerminalAction, TerminalModel, UnderlineStyle,
};

fn render(input: &[u8], columns: u16, rows: u16) -> FullFrame {
    let mut terminal =
        TerminalModel::new(GridSize::new(columns, rows).expect("fixture grid must be valid"))
            .expect("Ghostty terminal must initialize");
    terminal
        .advance(TerminalAction::Output(input))
        .expect("fixture must parse");
    terminal.frame().expect("fixture frame must translate")
}

fn visible(frame: &FullFrame) -> Vec<String> {
    frame.rows.iter().map(|row| row.text()).collect()
}

fn assert_same_projection(expected: &FullFrame, actual: &FullFrame) {
    assert_eq!(actual.grid, expected.grid);
    assert_eq!(actual.rows, expected.rows);
    assert_eq!(actual.styles, expected.styles);
    assert_eq!(actual.cursor, expected.cursor);
    assert_eq!(actual.default_foreground, expected.default_foreground);
    assert_eq!(actual.default_background, expected.default_background);
    assert_eq!(actual.mouse_tracking, expected.mouse_tracking);
}

#[test]
fn canonical_vt_sequences_keep_expected_visible_state() {
    let cases: &[(&str, &[u8], &[&str])] = &[
        (
            "cursor addressing",
            b"\x1b[2J\x1b[Hhome\x1b[3;5HX",
            &["home", "", "X", "", "", ""],
        ),
        (
            "erase to end of line",
            b"abcdef\r\x1b[3C\x1b[K",
            &["abc", "", "", "", "", ""],
        ),
        (
            "save and restore cursor",
            b"left\x1b7\x1b[3;1Hbottom\x1b8!",
            &["left!", "", "bottom", "", "", ""],
        ),
        (
            "alternate screen restores primary",
            b"primary\x1b[?1049hsecondary\x1b[?1049l",
            &["primary", "", "", "", "", ""],
        ),
        (
            "insert and delete characters",
            b"abcde\r\x1b[2C\x1b[@X\x1b[P",
            &["abXde", "", "", "", "", ""],
        ),
    ];

    for (name, input, expected) in cases {
        let frame = render(input, 20, 6);
        assert_eq!(visible(&frame), *expected, "fixture: {name}");
        if *name == "cursor addressing" {
            assert_eq!(frame.rows[2].cells[4].grapheme, "X");
        }
    }
}

#[test]
fn unicode_width_combining_marks_and_truecolor_survive_translation() {
    let frame = render(
        "A界B e\u{301} 👩\u{200d}💻 \u{1b}[38;2;12;34;56mcolor\u{1b}[4:3mcurly".as_bytes(),
        40,
        4,
    );
    let text = visible(&frame).join("\n");
    assert!(text.contains("A界"));
    assert!(text.contains("e\u{301}"));
    assert!(text.contains("👩"));
    assert!(text.contains("color"));
    assert!(frame.styles.iter().any(|style| {
        style.foreground
            == Rgb {
                red: 12,
                green: 34,
                blue: 56,
            }
    }));
    assert!(
        frame
            .styles
            .iter()
            .any(|style| style.underline == UnderlineStyle::Curly)
    );
    assert!(frame.rows[0].cells.iter().any(|cell| cell.width == 2));
    assert!(frame.rows[0].cells.iter().any(|cell| cell.width == 0));
}

#[test]
fn parser_projection_is_invariant_to_pty_read_chunk_boundaries() {
    let input = concat!(
        "\u{1b}[2J\u{1b}[H",
        "plain界e\u{301}\r\n",
        "\u{1b}[1;3;38;2;17;34;51mstyled\u{1b}[0m ",
        "\u{1b}]8;;https://example.com/path\u{1b}\\link\u{1b}]8;;\u{1b}\\",
        "\u{1b}[?1004h\u{1b}[?2004h",
        "\u{1b}[?1049halternate\u{1b}[?1049l",
        "\r\nfinal"
    )
    .as_bytes();
    let expected = render(input, 32, 8);

    for seed in 1_u64..=32 {
        let mut terminal = TerminalModel::new(GridSize::new(32, 8).expect("valid grid"))
            .expect("Ghostty terminal must initialize");
        let mut offset = 0;
        let mut state = seed;
        while offset < input.len() {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let width = usize::try_from((state >> 32) % 11 + 1).expect("chunk width fits usize");
            let end = (offset + width).min(input.len());
            terminal
                .advance(TerminalAction::Output(&input[offset..end]))
                .expect("every chunk boundary must parse");
            offset = end;
        }
        let actual = terminal.frame().expect("chunked frame must translate");
        assert_same_projection(&expected, &actual);
    }
}

#[test]
fn indexed_colors_sgr_reset_and_osc8_links_keep_semantic_identity() {
    let frame = render(
        concat!(
            "\u{1b}[38;5;196;48;5;21;1;3;9mstyled",
            "\u{1b}[0mplain ",
            "\u{1b}]8;id=docs;https://example.com/guide\u{1b}\\docs",
            "\u{1b}]8;;\u{1b}\\"
        )
        .as_bytes(),
        32,
        3,
    );
    let row = &frame.rows[0];
    let styled = frame
        .styles
        .get(row.cells[0].style_index as usize)
        .expect("styled cell must reference an interned style");
    assert!(styled.bold);
    assert!(styled.italic);
    assert!(styled.strikethrough);
    assert_ne!(styled.foreground, frame.default_foreground);
    let plain = row
        .cells
        .iter()
        .find(|cell| cell.grapheme == "p")
        .expect("reset text should render");
    let plain_style = frame
        .styles
        .get(plain.style_index as usize)
        .expect("plain cell must reference an interned style");
    assert_eq!(plain_style.foreground, frame.default_foreground);
    assert!(
        row.cells
            .iter()
            .filter(|cell| "docs".contains(&cell.grapheme))
            .any(|cell| cell.hyperlink.as_deref() == Some("https://example.com/guide"))
    );
}

#[test]
fn decscusr_exposes_block_underline_and_beam_cursor_shapes() {
    let cases = [
        (b"\x1b[2 q".as_slice(), CursorShape::Block, false),
        (b"\x1b[3 q".as_slice(), CursorShape::Underline, true),
        (b"\x1b[6 q".as_slice(), CursorShape::Bar, false),
    ];
    for (sequence, expected_shape, expected_blinking) in cases {
        let frame = render(sequence, 10, 2);
        let cursor = frame.cursor.expect("cursor should remain visible");
        assert_eq!(cursor.shape, expected_shape);
        assert_eq!(cursor.blinking, expected_blinking);
    }
}

#[test]
fn resizing_reflows_wrapped_content_without_losing_graphemes() {
    let mut terminal = TerminalModel::new(GridSize::new(12, 4).expect("valid initial grid"))
        .expect("Ghostty terminal must initialize");
    terminal
        .advance(TerminalAction::Output(b"one-two-three-four"))
        .expect("fixture output should parse");
    terminal
        .advance(TerminalAction::Resize {
            grid: GridSize::new(7, 5).expect("valid resized grid"),
            cell_width_px: 8,
            cell_height_px: 16,
        })
        .expect("resize should reflow");
    let text = visible(&terminal.frame().expect("resized frame must translate")).join("");
    assert!(text.contains("one-two-three-four"));
}
