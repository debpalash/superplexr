use gpui::{Pixels, Rems, px, rems};

const DEFAULT_REM_SIZE_PX: f32 = 16.0;

// Browser-like stops keep repeated key presses predictable and avoid float drift.
const ZOOM_FACTORS: [f32; 11] = [
    0.50, 0.67, 0.75, 0.80, 0.90, 1.00, 1.10, 1.25, 1.50, 1.75, 2.00,
];
const DEFAULT_ZOOM_INDEX: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AppZoom {
    index: usize,
}

impl Default for AppZoom {
    fn default() -> Self {
        Self {
            index: DEFAULT_ZOOM_INDEX,
        }
    }
}

impl AppZoom {
    pub(crate) fn increase(&mut self) -> bool {
        let next = (self.index + 1).min(ZOOM_FACTORS.len() - 1);
        let changed = next != self.index;
        self.index = next;
        changed
    }

    pub(crate) fn decrease(&mut self) -> bool {
        let next = self.index.saturating_sub(1);
        let changed = next != self.index;
        self.index = next;
        changed
    }

    pub(crate) fn reset(&mut self) -> bool {
        let changed = self.index != DEFAULT_ZOOM_INDEX;
        self.index = DEFAULT_ZOOM_INDEX;
        changed
    }

    pub(crate) fn rem_size(self) -> Pixels {
        px(DEFAULT_REM_SIZE_PX * ZOOM_FACTORS[self.index])
    }

    #[cfg(test)]
    pub(crate) fn percentage(self) -> u16 {
        (ZOOM_FACTORS[self.index] * 100.0).round() as u16
    }
}

/// Converts a design pixel value at 100% into a root-relative UI length.
///
/// Text and line metrics expressed through this helper follow `AppZoom` via
/// `Window::set_rem_size`, including the custom terminal renderer.
pub(crate) fn ui_size(px_at_100_percent: f32) -> Rems {
    rems(px_at_100_percent / DEFAULT_REM_SIZE_PX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_uses_stable_browser_like_stops_and_resets_to_one_hundred_percent() {
        let mut zoom = AppZoom::default();
        assert_eq!(zoom.percentage(), 100);

        assert!(zoom.increase());
        assert_eq!(zoom.percentage(), 110);
        assert!(zoom.increase());
        assert_eq!(zoom.percentage(), 125);

        assert!(zoom.decrease());
        assert_eq!(zoom.percentage(), 110);
        assert!(zoom.reset());
        assert_eq!(zoom.percentage(), 100);
        assert!(!zoom.reset());
    }

    #[test]
    fn zoom_is_bounded() {
        let mut zoom = AppZoom::default();
        while zoom.increase() {}
        assert_eq!(zoom.percentage(), 200);
        assert!(!zoom.increase());

        while zoom.decrease() {}
        assert_eq!(zoom.percentage(), 50);
        assert!(!zoom.decrease());
    }
}
