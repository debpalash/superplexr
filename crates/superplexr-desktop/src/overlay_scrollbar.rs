//! Small presentation-only state for overlay scroll indicators.
//!
//! Scrolling remains owned by GPUI or the terminal model. This module only
//! decides when a non-layout-shifting thumb is visible and where it paints.

const MIN_THUMB_PX: f32 = 22.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollThumb {
    pub(crate) top_px: f32,
    pub(crate) height_px: f32,
}

#[derive(Debug, Default)]
pub(crate) struct AutoHideScrollbar {
    generation: u64,
    pointer_near_edge: bool,
    visible: bool,
}

impl AutoHideScrollbar {
    pub(crate) fn reveal(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.visible = true;
        self.generation
    }

    pub(crate) fn hide_if_current(&mut self, generation: u64) -> bool {
        if self.generation != generation || self.pointer_near_edge || !self.visible {
            return false;
        }
        self.visible = false;
        true
    }

    /// Reveal once when the pointer enters the narrow discovery zone. Keeping
    /// the pointer there does not enqueue timers or redraw on every mouse move.
    pub(crate) fn enter_edge(&mut self) -> bool {
        if self.pointer_near_edge {
            return false;
        }
        self.pointer_near_edge = true;
        self.generation = self.generation.saturating_add(1);
        let changed = !self.visible;
        self.visible = true;
        changed
    }

    /// Release the edge hold and return the generation a delayed hide should
    /// use. A subsequent scroll or edge entry invalidates that hide.
    pub(crate) fn leave_edge(&mut self) -> Option<u64> {
        if !self.pointer_near_edge {
            return None;
        }
        self.pointer_near_edge = false;
        self.generation = self.generation.saturating_add(1);
        self.visible.then_some(self.generation)
    }

    pub(crate) const fn visible(&self) -> bool {
        self.visible
    }
}

pub(crate) fn scroll_handle_thumb(
    track_height_px: f32,
    offset_px: f32,
    max_offset_px: f32,
) -> Option<ScrollThumb> {
    if track_height_px <= 0.0 || max_offset_px <= 0.5 {
        return None;
    }
    let content_height = track_height_px + max_offset_px;
    let height = (track_height_px * track_height_px / content_height)
        .clamp(MIN_THUMB_PX.min(track_height_px), track_height_px);
    let travel = (track_height_px - height).max(0.0);
    let progress = (offset_px.abs() / max_offset_px).clamp(0.0, 1.0);
    Some(ScrollThumb {
        top_px: travel * progress,
        height_px: height,
    })
}

/// Terminal frames intentionally do not expose Ghostty scrollback internals.
/// Use a logarithmic position rail so deep history remains navigable without
/// inventing an exact content length at the backend-neutral rendering seam.
pub(crate) fn terminal_history_thumb(track_height_px: f32, rows_before_bottom: u32) -> ScrollThumb {
    let height = MIN_THUMB_PX.min(track_height_px.max(0.0));
    let travel = (track_height_px - height).max(0.0);
    let depth = (rows_before_bottom as f32 + 1.0).ln() / 100_001.0_f32.ln();
    ScrollThumb {
        top_px: travel * (1.0 - depth.clamp(0.0, 1.0)),
        height_px: height,
    }
}

/// Convert high-resolution trackpad pixels into terminal rows without losing
/// sub-row deltas between events.
pub(crate) fn accumulate_terminal_scroll_rows(
    pixel_delta: f32,
    line_height_px: f32,
    remainder: &mut f32,
) -> i32 {
    if line_height_px <= 0.0 {
        return 0;
    }
    let rows = *remainder - pixel_delta / line_height_px;
    let whole_rows = rows.trunc();
    *remainder = rows - whole_rows;
    whole_rows as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_hide_cannot_conceal_a_new_scroll_gesture() {
        let mut state = AutoHideScrollbar::default();
        let first = state.reveal();
        let second = state.reveal();
        assert!(!state.hide_if_current(first));
        assert!(state.visible());
        assert!(state.hide_if_current(second));
        assert!(!state.visible());
    }

    #[test]
    fn edge_intent_holds_visibility_without_rearming_on_mouse_move() {
        let mut state = AutoHideScrollbar::default();
        assert!(state.enter_edge());
        assert!(!state.enter_edge());
        let generation = state.leave_edge().expect("visible rail should arm a hide");
        assert!(state.hide_if_current(generation));
    }

    #[test]
    fn edge_intent_protects_against_an_older_scroll_hide() {
        let mut state = AutoHideScrollbar::default();
        let scroll = state.reveal();
        assert!(!state.enter_edge());
        assert!(!state.hide_if_current(scroll));
        assert!(state.visible());
    }

    #[test]
    fn overlay_thumb_tracks_offset_without_reserving_layout_space() {
        let top = scroll_handle_thumb(200.0, 0.0, 600.0).expect("scrollable content");
        let bottom = scroll_handle_thumb(200.0, -600.0, 600.0).expect("scrollable content");
        assert_eq!(top.top_px, 0.0);
        assert!(bottom.top_px > top.top_px);
        assert!(bottom.height_px >= MIN_THUMB_PX);
        assert!(scroll_handle_thumb(200.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn terminal_history_rail_moves_up_as_history_gets_older() {
        let bottom = terminal_history_thumb(200.0, 0);
        let older = terminal_history_thumb(200.0, 10_000);
        assert!(older.top_px < bottom.top_px);
    }

    #[test]
    fn terminal_trackpad_scroll_preserves_fractional_rows() {
        let mut remainder = 0.0;
        assert_eq!(
            accumulate_terminal_scroll_rows(-4.0, 16.0, &mut remainder),
            0
        );
        assert_eq!(
            accumulate_terminal_scroll_rows(-4.0, 16.0, &mut remainder),
            0
        );
        assert_eq!(
            accumulate_terminal_scroll_rows(-8.0, 16.0, &mut remainder),
            1
        );
        assert_eq!(remainder, 0.0);
    }
}
