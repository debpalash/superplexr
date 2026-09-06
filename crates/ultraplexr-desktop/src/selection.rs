//! Viewer-side text selection.
//!
//! Selection is the viewer's concern, not the terminal's. The daemon's native
//! selection requires controlling the terminal, so it was refused on any
//! terminal an agent had spawned, on observed terminals, and on finished ones,
//! and it was shared by every viewer of a session. The desktop already holds
//! every visible row, so it tracks the selection itself, paints it, and copies
//! from the rows it has. Nothing goes over the wire.

use std::sync::Arc;

use ultraplexr_terminal::{Row, SelectionPoint};

/// A selection over the visible grid, in viewport coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Selection {
    pub anchor: SelectionPoint,
    pub head: SelectionPoint,
    /// Select a rectangle of columns rather than a run of text.
    pub rectangle: bool,
}

/// An inclusive run of selected columns on one viewport row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Span {
    pub row: u16,
    pub start: u16,
    pub end: u16,
}

impl Selection {
    pub(crate) fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The selected columns on each row, top to bottom, for a grid of the
    /// given width. Drag direction does not matter.
    pub(crate) fn spans(&self, columns: u16) -> Vec<Span> {
        let last_column = columns.saturating_sub(1);
        if self.rectangle {
            let (top, bottom) = ordered(self.anchor.row, self.head.row);
            let (start, end) = ordered(self.anchor.column, self.head.column);
            return (top..=bottom)
                .map(|row| Span {
                    row,
                    start: start.min(last_column),
                    end: end.min(last_column),
                })
                .collect();
        }
        let (first, last) =
            if (self.anchor.row, self.anchor.column) <= (self.head.row, self.head.column) {
                (self.anchor, self.head)
            } else {
                (self.head, self.anchor)
            };
        (first.row..=last.row)
            .map(|row| Span {
                row,
                start: if row == first.row { first.column } else { 0 },
                end: if row == last.row {
                    last.column.min(last_column)
                } else {
                    last_column
                },
            })
            .collect()
    }

    /// The selected text, as it would be pasted.
    ///
    /// Trailing blanks on each row are dropped, and a row the terminal
    /// soft-wrapped continues onto the next without a newline, so a long line
    /// comes back as the one line it was.
    pub(crate) fn text(&self, rows: &[Arc<Row>], columns: u16) -> String {
        // A span past the last row selects nothing and separates nothing.
        let spans = self
            .spans(columns)
            .into_iter()
            .filter_map(|span| rows.get(usize::from(span.row)).map(|row| (span, row)))
            .collect::<Vec<_>>();
        let mut out = String::new();
        for (index, (span, row)) in spans.iter().enumerate() {
            let last = index + 1 == spans.len();
            let mut line = row
                .cells
                .iter()
                .skip(usize::from(span.start))
                .take(usize::from(span.end - span.start) + 1)
                // A zero-width cell is the spacer after a wide glyph; the
                // glyph itself already carries the text.
                .filter(|cell| cell.width > 0)
                .map(|cell| cell.grapheme.as_str())
                .collect::<String>();
            // A row the terminal soft-wrapped continues onto the next one,
            // so its trailing blanks are real and no newline follows it.
            let continues = !self.rectangle && row.wrapped && !last;
            if !continues {
                while line.ends_with(' ') {
                    line.pop();
                }
            }
            out.push_str(&line);
            if !last && !continues {
                out.push('\n');
            }
        }
        out
    }
}

fn ordered(a: u16, b: u16) -> (u16, u16) {
    if a <= b { (a, b) } else { (b, a) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ultraplexr_terminal::Cell;

    fn cell(text: &str, width: u8) -> Cell {
        Cell {
            grapheme: text.to_owned(),
            width,
            style_index: 0,
            hyperlink: None,
        }
    }

    fn row(text: &str, wrapped: bool) -> Arc<Row> {
        Arc::new(Row {
            wrapped,
            cells: text.chars().map(|c| cell(&c.to_string(), 1)).collect(),
        })
    }

    fn at(row: u16, column: u16) -> SelectionPoint {
        SelectionPoint { row, column }
    }

    #[test]
    fn a_drag_selects_the_same_text_in_either_direction() {
        let rows = vec![row("hello world", false), row("second line", false)];
        let forward = Selection {
            anchor: at(0, 6),
            head: at(1, 5),
            rectangle: false,
        };
        let backward = Selection {
            anchor: at(1, 5),
            head: at(0, 6),
            rectangle: false,
        };
        assert_eq!(forward.text(&rows, 20), "world\nsecond");
        assert_eq!(backward.text(&rows, 20), "world\nsecond");
        assert_eq!(
            forward.spans(20),
            vec![
                Span {
                    row: 0,
                    start: 6,
                    end: 19
                },
                Span {
                    row: 1,
                    start: 0,
                    end: 5
                }
            ]
        );
    }

    #[test]
    fn a_rectangle_takes_the_same_columns_from_every_row() {
        let rows = vec![
            row("abcdef", false),
            row("ghijkl", false),
            row("mnopqr", false),
        ];
        let selection = Selection {
            anchor: at(2, 4),
            head: at(0, 2),
            rectangle: true,
        };
        assert_eq!(selection.text(&rows, 6), "cde\nijk\nopq");
    }

    #[test]
    fn a_wide_glyph_is_copied_once_and_its_spacer_never() {
        let rows = vec![Arc::new(Row {
            wrapped: false,
            cells: vec![cell("a", 1), cell("日", 2), cell("", 0), cell("b", 1)],
        })];
        let selection = Selection {
            anchor: at(0, 0),
            head: at(0, 3),
            rectangle: false,
        };
        assert_eq!(selection.text(&rows, 4), "a日b");
    }

    #[test]
    fn a_soft_wrapped_row_comes_back_as_one_line() {
        let rows = vec![
            row("this line was too long and the ", true),
            row("terminal wrapped it", false),
        ];
        let selection = Selection {
            anchor: at(0, 0),
            head: at(1, 18),
            rectangle: false,
        };
        assert_eq!(
            selection.text(&rows, 32),
            "this line was too long and the terminal wrapped it"
        );
    }

    #[test]
    fn trailing_blanks_are_dropped_and_out_of_range_rows_are_ignored() {
        let rows = vec![row("short   ", false)];
        let selection = Selection {
            anchor: at(0, 0),
            head: at(3, 40),
            rectangle: false,
        };
        assert_eq!(
            selection.text(&rows, 10),
            "short",
            "rows past the end of the frame add nothing, not even newlines"
        );
        assert!(
            Selection {
                anchor: at(1, 1),
                head: at(1, 1),
                rectangle: false
            }
            .is_empty()
        );
    }
}
