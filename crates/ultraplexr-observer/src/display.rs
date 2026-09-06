//! Bounded browser styling projection; canonical VT parsing stays on the host.
use serde::Serialize;
use ultraplexr_terminal::{CellStyle, Cursor, FullFrame, Rgb};

#[derive(Clone, Serialize)]
pub(super) struct Display {
    columns: u16,
    rows: Vec<Vec<Run>>,
    styles: Vec<CellStyle>,
    foreground: Rgb,
    background: Rgb,
    cursor: Option<Cursor>,
}

#[derive(Clone, PartialEq, Serialize)]
struct Run {
    text: String,
    columns: u16,
    style: u32,
}

#[derive(Serialize)]
struct ChangedRow<'a> {
    index: usize,
    text: &'a str,
    runs: &'a [Run],
}

/// Browser-local wire optimization. Revision strings preserve u64 identity in
/// JavaScript; they are neither replay tokens nor terminal input authority.
#[derive(Serialize)]
pub(super) struct FramePatch<'a> {
    base_revision: String,
    revision: String,
    sequence: u64,
    title: &'a Option<String>,
    directory: &'a Option<String>,
    status: ultraplexr_protocol::TerminalSessionStatus,
    rows: u16,
    cursor: Option<Cursor>,
    changed: Vec<ChangedRow<'a>>,
}

impl<'a> FramePatch<'a> {
    pub(super) fn between(previous: &super::Snapshot, next: &'a super::Snapshot) -> Option<Self> {
        let before = previous.display.as_ref()?;
        let after = next.display.as_ref()?;
        if next
            .title
            .as_ref()
            .is_some_and(|value| value.len() > 65_536)
            || next
                .directory
                .as_ref()
                .is_some_and(|value| value.len() > 65_536)
        {
            return None;
        }
        if next.sequence <= previous.sequence
            || previous.continuity != next.continuity
            || previous.rows != next.rows
            || before.columns != after.columns
            || before.rows.len() != after.rows.len()
            || before.styles != after.styles
            || before.foreground != after.foreground
            || before.background != after.background
        {
            return None;
        }
        let old_lines: Vec<_> = previous.text.split('\n').collect();
        let new_lines: Vec<_> = next.text.split('\n').collect();
        if old_lines.len() != before.rows.len() || new_lines.len() != after.rows.len() {
            return None;
        }
        if new_lines.iter().any(|line| line.contains('\r')) {
            return None;
        }
        let changed = after
            .rows
            .iter()
            .enumerate()
            .filter_map(|(index, runs)| {
                (before.rows[index] != *runs || old_lines[index] != new_lines[index]).then_some(
                    ChangedRow {
                        index,
                        text: new_lines[index],
                        runs,
                    },
                )
            })
            .collect();
        Some(Self {
            base_revision: previous.sequence.to_string(),
            revision: next.sequence.to_string(),
            sequence: next.sequence,
            title: &next.title,
            directory: &next.directory,
            status: next.status,
            rows: next.rows,
            cursor: after.cursor,
            changed,
        })
    }
}

impl Display {
    /// Keep the text-only representation for unsupported sizes. No native
    /// frame, terminal process, or input geometry is changed to fit the browser.
    pub(super) fn from_frame(frame: &FullFrame) -> Option<Self> {
        if frame.grid.columns > 400 || frame.grid.rows > 200 || frame.styles.len() > 4096 {
            return None;
        }
        let mut rows = Vec::with_capacity(frame.rows.len());
        let mut bytes = 0usize;
        let mut cells = 0usize;
        // Conservative JSON-size allowance (including worst-case escaping),
        // separate from the canonical text already present in the snapshot.
        let mut encoded_budget = frame.styles.len().saturating_mul(384) + 4096;
        for row in &frame.rows {
            if rows.len() >= 200 {
                return None;
            }
            let mut runs: Vec<Run> = Vec::new();
            let mut columns = 0u16;
            let mut previous_narrow = false;
            for cell in &row.cells {
                cells += 1;
                bytes = bytes.saturating_add(cell.grapheme.len());
                if cells > 80_000 || bytes > 2 * 1024 * 1024 {
                    return None;
                }
                // Width-zero cells continue a preceding wide grapheme.
                if cell.width == 0 {
                    continue;
                }
                if cell.width > 2 || cell.style_index as usize >= frame.styles.len() {
                    return None;
                }
                columns = columns.checked_add(u16::from(cell.width))?;
                if columns > frame.grid.columns {
                    return None;
                }
                let text = if cell.grapheme.is_empty() {
                    " "
                } else {
                    &cell.grapheme
                };
                // Keep each wide grapheme in its own fixed two-column box;
                // adjacent narrow cells with the same style share one DOM run.
                if cell.width == 1
                    && previous_narrow
                    && let Some(run) = runs.last_mut()
                    && run.style == cell.style_index
                {
                    run.text.push_str(text);
                    run.columns += 1;
                } else {
                    encoded_budget = encoded_budget.saturating_add(64);
                    runs.push(Run {
                        text: text.to_owned(),
                        columns: u16::from(cell.width),
                        style: cell.style_index,
                    });
                }
                encoded_budget = encoded_budget.saturating_add(text.len().saturating_mul(6));
                if encoded_budget > 2 * 1024 * 1024 {
                    return None;
                }
                previous_narrow = cell.width == 1;
            }
            rows.push(runs);
        }
        Some(Self {
            columns: frame.grid.columns,
            rows,
            styles: frame.styles.clone(),
            foreground: frame.default_foreground,
            background: frame.default_background,
            cursor: frame.cursor,
        })
    }
}
