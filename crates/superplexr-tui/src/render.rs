//! Only canonical cells become output; OSC titles/hyperlinks/clipboard are never
//! replayed. Clipboard export is a separate explicit local command.
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
    },
    terminal::{Clear, ClearType},
};
use std::io::{self, Write};
use superplexr_terminal::{CellStyle, FullFrame, Rgb, UnderlineStyle};
use unicode_width::UnicodeWidthChar;

pub fn safe_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

pub fn clipped(text: &str, columns: u16) -> String {
    let mut width = 0;
    safe_text(text)
        .chars()
        .take_while(|c| {
            width += c.width().unwrap_or(0);
            width <= usize::from(columns)
        })
        .collect()
}

fn color(value: Rgb) -> Color {
    Color::Rgb {
        r: value.red,
        g: value.green,
        b: value.blue,
    }
}

fn style(out: &mut impl Write, value: &CellStyle) -> io::Result<()> {
    queue!(
        out,
        SetAttribute(Attribute::Reset),
        SetForegroundColor(color(value.foreground)),
        SetBackgroundColor(color(value.background))
    )?;
    for (enabled, attribute) in [
        (value.bold, Attribute::Bold),
        (value.italic, Attribute::Italic),
        (value.faint, Attribute::Dim),
        (value.inverse, Attribute::Reverse),
        (value.invisible, Attribute::Hidden),
        (value.strikethrough, Attribute::CrossedOut),
        (
            value.underline != UnderlineStyle::None,
            Attribute::Underlined,
        ),
    ] {
        if enabled {
            queue!(out, SetAttribute(attribute))?;
        }
    }
    Ok(())
}

/// Repaint changed rows only. Explicit cell placement handles wide continuation
/// cells and combining graphemes; clipping never resizes an observed PTY.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

pub fn draw(
    out: &mut impl Write,
    frame: Option<&FullFrame>,
    previous: Option<&FullFrame>,
    size: (u16, u16),
    status: &str,
    show_cursor: bool,
) -> io::Result<()> {
    draw_pane(
        out,
        frame,
        previous,
        Rect {
            x: 0,
            y: 0,
            width: size.0,
            height: size.1,
        },
        status,
        show_cursor,
    )
}

pub fn draw_pane(
    out: &mut impl Write,
    frame: Option<&FullFrame>,
    previous: Option<&FullFrame>,
    rect: Rect,
    status: &str,
    show_cursor: bool,
) -> io::Result<()> {
    let (columns, rows) = (rect.width, rect.height);
    if columns == 0 || rows == 0 {
        return Ok(());
    }
    queue!(out, Hide, SetAttribute(Attribute::Reset), ResetColor)?;
    let reset = previous.is_none()
        || frame.is_none()
        || frame.zip(previous).is_some_and(|(a, b)| a.grid != b.grid);
    if reset {
        for y in 0..rows {
            queue!(
                out,
                MoveTo(rect.x, rect.y + y),
                Print(" ".repeat(usize::from(columns)))
            )?;
        }
    }
    if let Some(frame) = frame {
        for (y, row) in frame
            .rows
            .iter()
            .take(usize::from(rows.saturating_sub(1)))
            .enumerate()
        {
            if !reset
                && previous
                    .is_some_and(|old| old.styles == frame.styles && old.rows.get(y) == Some(row))
            {
                continue;
            }
            queue!(
                out,
                MoveTo(rect.x, rect.y + y as u16),
                SetAttribute(Attribute::Reset),
                SetBackgroundColor(color(frame.default_background)),
                Print(" ".repeat(usize::from(columns)))
            )?;
            let mut last_style = None;
            for (x, cell) in row.cells.iter().take(usize::from(columns)).enumerate() {
                if cell.width == 0 || x + usize::from(cell.width) > usize::from(columns) {
                    continue;
                }
                if last_style != Some(cell.style_index) {
                    if let Some(value) = frame.styles.get(cell.style_index as usize) {
                        style(out, value)?;
                    } else {
                        queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
                    }
                    last_style = Some(cell.style_index);
                }
                let text = safe_text(&cell.grapheme);
                queue!(
                    out,
                    MoveTo(rect.x + x as u16, rect.y + y as u16),
                    Print(if text.is_empty() { " " } else { &text })
                )?;
            }
        }
    }
    queue!(
        out,
        MoveTo(rect.x, rect.y + rows - 1),
        SetAttribute(Attribute::Reset),
        ResetColor,
        Print(" ".repeat(usize::from(columns))),
        MoveTo(rect.x, rect.y + rows - 1),
        SetAttribute(Attribute::Reverse),
        Print(clipped(status, columns)),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;
    if show_cursor
        && let Some(cursor) = frame.and_then(|frame| frame.cursor)
        && cursor.column < columns
        && cursor.row < rows.saturating_sub(1)
    {
        queue!(
            out,
            MoveTo(rect.x + cursor.column, rect.y + cursor.row),
            Show
        )?;
    }
    out.flush()
}

pub fn draw_navigator(
    out: &mut impl Write,
    nav: &crate::navigator::Navigator,
    size: (u16, u16),
    split: bool,
) -> io::Result<()> {
    if size.0 == 0 || size.1 == 0 {
        return Ok(());
    }
    queue!(
        out,
        Hide,
        SetAttribute(Attribute::Reset),
        ResetColor,
        Clear(ClearType::All),
        MoveTo(0, 0),
        SetAttribute(Attribute::Reverse),
        Print(clipped(
            &format!(
                "Superplexr / {}{}",
                if nav.inspection.is_some() {
                    "Verification"
                } else if nav.catalog.is_some() {
                    "Verifiers"
                } else {
                    nav.tab.title()
                },
                if nav.workflow_active() {
                    " / recorded snapshot / read-only"
                } else if split {
                    " / choose second pane"
                } else {
                    " / choose Session"
                }
            ),
            size.0
        )),
        SetAttribute(Attribute::Reset)
    )?;
    if size.1 > 1 {
        queue!(
            out,
            MoveTo(0, 1),
            Print(clipped(
                if nav.verifier_edit.is_some() {
                    "Verifier Run UUID | Enter inspect | Esc cancel | Ctrl-U clear | paste accepted"
                } else if nav.inspection.is_some() {
                    "Read-only | r refresh | w verifiers | W UUID | j/k move | / filter | Backspace return | Esc panes"
                } else if nav.catalog.is_some() {
                    "Enter status | n next | 0 first | W UUID | r refresh | j/k move | / filter page | Backspace Mission"
                } else if let Some(draft) = &nav.group_edit {
                    draft.field.help()
                } else if nav.tab == crate::navigator::Tab::Groups {
                    "Groups | F2 rename | F3 order | p pin | D view | Enter members | Tab views"
                } else if nav.workflow_read {
                    "Tab views | w Mission verifiers | W UUID | j/k move | Enter open | / filter | r refresh | Esc back"
                } else {
                    "Tab views | g groups | D another view | j/k move | Enter open | / filter | r refresh | Esc back"
                },
                size.0
            ))
        )?;
    }
    if size.1 > 2 {
        queue!(
            out,
            MoveTo(0, 2),
            Print(clipped(
                &nav.verifier_edit
                    .as_ref()
                    .map(|draft| format!("Verifier: {} | Mission {}", draft.value, draft.mission))
                    .or_else(|| nav.group_edit.as_ref().map(|draft| format!(
                        "{}: {}",
                        draft.field.label(),
                        draft.value
                    )))
                    .unwrap_or_else(|| format!(
                        "Filter{}: {}{}",
                        if nav.editing { " (typing)" } else { "" },
                        nav.query,
                        if nav.workflow_active() {
                            " | Recorded snapshot; no automatic refresh"
                        } else if nav.group.is_some() {
                            " | Group filtered; Backspace returns"
                        } else if nav.mission.is_some() {
                            " | Mission filtered; Backspace clears"
                        } else {
                            ""
                        }
                    )),
                size.0
            ))
        )?;
    }
    let entries = nav.visible();
    let capacity = usize::from(size.1.saturating_sub(5)) / 2;
    let start = nav.selected.saturating_sub(capacity.saturating_sub(1));
    for (index, entry) in entries.iter().enumerate().skip(start).take(capacity) {
        let y = 3 + ((index - start) * 2) as u16;
        queue!(out, MoveTo(0, y))?;
        if index == nav.selected {
            queue!(out, SetAttribute(Attribute::Reverse))?;
        }
        queue!(
            out,
            Print(clipped(
                &format!(
                    "{} {}",
                    if index == nav.selected { ">" } else { " " },
                    entry.label
                ),
                size.0
            )),
            SetAttribute(Attribute::Reset),
            MoveTo(2.min(size.0 - 1), y + 1),
            Print(clipped(&entry.detail, size.0.saturating_sub(2)))
        )?;
    }
    if entries.is_empty() && size.1 > 4 {
        queue!(
            out,
            MoveTo(0, 3),
            Print(clipped(
                "No matching accessible items. Change view/filter or press r to refresh.",
                size.0
            ))
        )?;
    }
    if size.1 > 3 {
        queue!(
            out,
            MoveTo(0, size.1 - 1),
            Print(clipped(
                if nav.action_note.is_empty()
                    || nav.workflow_active()
                    || nav.verifier_edit.is_some()
                {
                    &nav.note
                } else {
                    &nav.action_note
                },
                size.0
            ))
        )?;
    }
    out.flush()
}

pub fn copy_text(frame: &FullFrame) -> String {
    frame
        .rows
        .iter()
        .map(|row| safe_text(&row.text()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// History row numbers identify results, not a separate terminal or PTY. Keep
/// the outer terminal's palette/type, reverse-video focus, and cell clipping.
pub fn draw_search(
    out: &mut impl Write,
    panel: &crate::search::Panel,
    size: (u16, u16),
) -> io::Result<()> {
    let (columns, rows) = size;
    if columns == 0 || rows == 0 {
        return Ok(());
    }
    queue!(
        out,
        Hide,
        SetAttribute(Attribute::Reset),
        ResetColor,
        Clear(ClearType::All),
        MoveTo(0, 0),
        SetAttribute(Attribute::Reverse),
        Print(clipped(
            &format!(
                "Superplexr / Search / {}",
                &panel.session_id.to_string()[..8]
            ),
            columns
        )),
        SetAttribute(Attribute::Reset)
    )?;
    if rows > 2 {
        queue!(
            out,
            MoveTo(0, 1),
            Print(clipped(
                if panel.editing {
                    "Enter search | Esc back | Ctrl-C cancel | ^] d detach"
                } else {
                    "j/k move | Enter history | / edit | Ctrl-C cancel | Esc back"
                },
                columns
            ))
        )?;
    }
    if rows > 3 {
        queue!(
            out,
            MoveTo(0, 2),
            Print(clipped(
                &format!(
                    "Query{}: {}",
                    if panel.editing { " (typing)" } else { "" },
                    panel.query
                ),
                columns
            ))
        )?;
    }
    let capacity = usize::from(rows.saturating_sub(4));
    let start = panel.selected.saturating_sub(capacity.saturating_sub(1));
    for (index, found) in panel.matches.iter().enumerate().skip(start).take(capacity) {
        queue!(out, MoveTo(0, 3 + (index - start) as u16))?;
        if index == panel.selected {
            queue!(out, SetAttribute(Attribute::Reverse))?;
        }
        queue!(
            out,
            Print(clipped(
                &format!(
                    "{} {}:{}  {}",
                    if index == panel.selected { ">" } else { " " },
                    found.line.saturating_add(1),
                    found.column.saturating_add(1),
                    found.preview
                ),
                columns
            )),
            SetAttribute(Attribute::Reset)
        )?;
    }
    if rows > 1 {
        queue!(
            out,
            MoveTo(0, rows - 1),
            Print(clipped(&panel.note, columns))
        )?;
    }
    out.flush()
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
