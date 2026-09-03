//! Owner-adjustable split geometry: the sidebar edge and the splitters
//! between embedded terminals. Weights are integers so the persisted
//! document stays `Eq`; fractions are derived on demand.

use std::collections::HashMap;

use gpui::{Context, IntoElement, Render, Window, div};
use serde::{Deserialize, Serialize};

pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 160.0;
pub(crate) const SIDEBAR_MAX_WIDTH: f32 = 520.0;
pub(crate) const SPLITTER_THICKNESS: f32 = 6.0;
const WEIGHT_SCALE: u32 = 1_000;
const MIN_PANE_FRACTION: f32 = 0.12;

/// Clamp a dragged sidebar edge to something that leaves room for terminals.
pub(crate) fn sidebar_drag_width(pointer_x: f32, viewport_width: f32) -> f32 {
    let max = SIDEBAR_MAX_WIDTH
        .min(viewport_width * 0.5)
        .max(SIDEBAR_MIN_WIDTH);
    pointer_x.clamp(SIDEBAR_MIN_WIDTH, max).round()
}

/// Relative sizes of the columns in a session's terminal grid and of the
/// rows inside each column.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct PaneLayout {
    pub(crate) columns: Vec<u32>,
    pub(crate) rows: Vec<Vec<u32>>,
}

impl PaneLayout {
    pub(crate) fn equal(column_sizes: &[usize]) -> Self {
        Self {
            columns: vec![WEIGHT_SCALE; column_sizes.len()],
            rows: column_sizes
                .iter()
                .map(|rows| vec![WEIGHT_SCALE; *rows])
                .collect(),
        }
    }

    /// True when this layout describes exactly this grid shape.
    pub(crate) fn matches(&self, column_sizes: &[usize]) -> bool {
        self.columns.len() == column_sizes.len()
            && self.rows.len() == column_sizes.len()
            && self
                .rows
                .iter()
                .zip(column_sizes)
                .all(|(rows, size)| rows.len() == *size)
            && self.columns.iter().all(|weight| *weight > 0)
            && self.rows.iter().flatten().all(|weight| *weight > 0)
    }

    pub(crate) fn column_fractions(&self) -> Vec<f32> {
        fractions(&self.columns)
    }

    pub(crate) fn row_fractions(&self, column: usize) -> Vec<f32> {
        self.rows
            .get(column)
            .map(|rows| fractions(rows))
            .unwrap_or_default()
    }

    /// Move the boundary between column `split` and `split + 1` so that it
    /// sits at `fraction` of the grid width.
    pub(crate) fn set_column_split(&mut self, split: usize, fraction: f32) {
        move_boundary(&mut self.columns, split, fraction);
    }

    /// Move the boundary between row `split` and `split + 1` of `column` so
    /// that it sits at `fraction` of the grid height.
    pub(crate) fn set_row_split(&mut self, column: usize, split: usize, fraction: f32) {
        if let Some(rows) = self.rows.get_mut(column) {
            move_boundary(rows, split, fraction);
        }
    }

    pub(crate) fn reset_columns(&mut self) {
        self.columns
            .iter_mut()
            .for_each(|weight| *weight = WEIGHT_SCALE);
    }

    pub(crate) fn reset_rows(&mut self, column: usize) {
        if let Some(rows) = self.rows.get_mut(column) {
            rows.iter_mut().for_each(|weight| *weight = WEIGHT_SCALE);
        }
    }
}

fn fractions(weights: &[u32]) -> Vec<f32> {
    let total: u32 = weights.iter().sum();
    if total == 0 {
        return vec![1.0 / weights.len().max(1) as f32; weights.len()];
    }
    weights
        .iter()
        .map(|weight| *weight as f32 / total as f32)
        .collect()
}

fn move_boundary(weights: &mut [u32], split: usize, fraction: f32) {
    if split + 1 >= weights.len() || !fraction.is_finite() {
        return;
    }
    let total: u32 = weights.iter().sum();
    if total == 0 {
        return;
    }
    let before: u32 = weights[..split].iter().sum();
    let pair = weights[split] + weights[split + 1];
    let minimum = ((total as f32) * MIN_PANE_FRACTION).round() as u32;
    if pair <= minimum * 2 {
        return;
    }
    let target = (fraction.clamp(0.0, 1.0) * total as f32).round() as u32;
    let first = target.saturating_sub(before).clamp(minimum, pair - minimum);
    weights[split] = first;
    weights[split + 1] = pair - first;
}

/// In-memory layouts keyed by the ordered surface indices of a session's
/// terminals. Adding or removing a terminal changes the key, which resets
/// that session to an even grid.
#[derive(Debug, Default)]
pub(crate) struct PaneLayoutStore {
    layouts: HashMap<Vec<usize>, PaneLayout>,
}

impl PaneLayoutStore {
    pub(crate) fn fitted(&self, terminals: &[usize], column_sizes: &[usize]) -> PaneLayout {
        self.layouts
            .get(terminals)
            .filter(|layout| layout.matches(column_sizes))
            .cloned()
            .unwrap_or_else(|| PaneLayout::equal(column_sizes))
    }

    pub(crate) fn update(
        &mut self,
        terminals: &[usize],
        column_sizes: &[usize],
        change: impl FnOnce(&mut PaneLayout),
    ) {
        let mut layout = self.fitted(terminals, column_sizes);
        change(&mut layout);
        self.layouts.insert(terminals.to_vec(), layout);
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn insert(&mut self, terminals: Vec<usize>, layout: PaneLayout) {
        self.layouts.insert(terminals, layout);
    }

    pub(crate) fn get(&self, terminals: &[usize]) -> Option<&PaneLayout> {
        self.layouts.get(terminals)
    }
}

/// Drag payload for the sidebar edge.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SidebarEdgeDrag;

/// Drag payload for the boundary right of column `column`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ColumnSplitDrag {
    pub(crate) column: usize,
}

/// Drag payload for the boundary below row `row` of column `column`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowSplitDrag {
    pub(crate) column: usize,
    pub(crate) row: usize,
}

/// GPUI requires a preview view for every drag; splitters show none.
pub(crate) struct SplitDragPreview;

impl Render for SplitDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_layouts_match_their_shape_and_split_evenly() {
        let layout = PaneLayout::equal(&[2, 3]);
        assert!(layout.matches(&[2, 3]));
        assert!(!layout.matches(&[3, 2]));
        assert!(!layout.matches(&[2]));
        assert_eq!(layout.column_fractions(), [0.5, 0.5]);
        assert_eq!(layout.row_fractions(1).len(), 3);
        assert!((layout.row_fractions(1)[0] - 1.0 / 3.0).abs() < 1e-6);
        assert!(layout.row_fractions(9).is_empty());
    }

    #[test]
    fn splits_move_only_the_adjacent_pair_and_respect_minimums() {
        let mut layout = PaneLayout::equal(&[1, 1, 1]);
        layout.set_column_split(0, 0.2);
        let fractions = layout.column_fractions();
        assert!((fractions[0] - 0.2).abs() < 0.01, "{fractions:?}");
        assert!((fractions[1] - 0.4667).abs() < 0.01, "{fractions:?}");
        assert!((fractions[2] - 1.0 / 3.0).abs() < 0.01, "{fractions:?}");

        layout.set_column_split(0, 0.0);
        let fractions = layout.column_fractions();
        assert!(
            (fractions[0] - MIN_PANE_FRACTION).abs() < 0.01,
            "{fractions:?}"
        );

        layout.set_column_split(1, 1.0);
        let fractions = layout.column_fractions();
        assert!(
            (fractions[2] - MIN_PANE_FRACTION).abs() < 0.01,
            "{fractions:?}"
        );

        layout.set_column_split(5, 0.5);
        layout.set_column_split(0, f32::NAN);
        layout.reset_columns();
        assert_eq!(layout.column_fractions(), [1.0 / 3.0; 3]);
    }

    #[test]
    fn row_splits_are_scoped_to_one_column() {
        let mut layout = PaneLayout::equal(&[2, 2]);
        layout.set_row_split(1, 0, 0.75);
        assert_eq!(layout.row_fractions(0), [0.5, 0.5]);
        let rows = layout.row_fractions(1);
        assert!((rows[0] - 0.75).abs() < 0.01, "{rows:?}");
        layout.set_row_split(7, 0, 0.1);
        layout.reset_rows(1);
        assert_eq!(layout.row_fractions(1), [0.5, 0.5]);
    }

    #[test]
    fn store_resets_when_the_grid_shape_changes() {
        let mut store = PaneLayoutStore::default();
        store.update(&[0, 1], &[1, 1], |layout| layout.set_column_split(0, 0.3));
        assert!((store.fitted(&[0, 1], &[1, 1]).column_fractions()[0] - 0.3).abs() < 0.01);
        assert_eq!(store.fitted(&[0, 1], &[2]), PaneLayout::equal(&[2]));
        assert_eq!(
            store.fitted(&[0, 1, 2], &[2, 1]),
            PaneLayout::equal(&[2, 1])
        );
        assert!(store.get(&[0, 1]).is_some());
        assert!(store.get(&[0, 1, 2]).is_none());
        assert!(
            serde_json::from_str::<PaneLayout>(r#"{"columns":[1,2],"rows":[[1],[1]]}"#)
                .expect("layout")
                .matches(&[1, 1])
        );
    }

    #[test]
    fn sidebar_drag_width_stays_within_bounds() {
        assert_eq!(sidebar_drag_width(10.0, 1_400.0), SIDEBAR_MIN_WIDTH);
        assert_eq!(sidebar_drag_width(300.4, 1_400.0), 300.0);
        assert_eq!(sidebar_drag_width(900.0, 1_400.0), SIDEBAR_MAX_WIDTH);
        assert_eq!(sidebar_drag_width(900.0, 600.0), 300.0);
        assert_eq!(sidebar_drag_width(900.0, 200.0), SIDEBAR_MIN_WIDTH);
    }
}
