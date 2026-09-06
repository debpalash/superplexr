use gpui::{
    Anchored, AnchoredPositionMode, AnyElement, ParentElement, Pixels, Rems, Window, anchored,
    point, px, rems,
};

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
        px(DEFAULT_REM_SIZE_PX * self.factor())
    }

    /// Convert between window/pointer pixels and the 100% design coordinate
    /// space used by responsive layout and persisted user sizes.
    pub(crate) fn factor(self) -> f32 {
        ZOOM_FACTORS[self.index]
    }

    /// Modal contents scale with zoom, but their frame stays inside the window.
    /// Read the current viewport at render time rather than caching old bounds.
    pub(crate) fn overlay_width(self, window: &Window, design_width: f32) -> Pixels {
        px((design_width * self.factor()).min(f32::from(window.viewport_size().width) * 0.95))
    }

    pub(crate) fn overlay_height(self, window: &Window, design_height: f32) -> Pixels {
        px((design_height * self.factor()).min(f32::from(window.viewport_size().height) * 0.90))
    }

    /// Position relative to the owning row/tab, then let GPUI flip and clamp
    /// against measured window bounds. The child must have no absolute offset
    /// or margin, and must already have viewport-bounded dimensions.
    pub(crate) fn anchored_popup(self, x: f32, y: f32, child: AnyElement) -> Anchored {
        anchored()
            .position_mode(AnchoredPositionMode::Local)
            .position(point(px(x * self.factor()), px(y * self.factor())))
            .child(child)
    }

    #[cfg(test)]
    pub(crate) fn percentage(self) -> u16 {
        (ZOOM_FACTORS[self.index] * 100.0).round() as u16
    }
}

/// Converts a design pixel value at 100% into a root-relative UI length.
///
/// Text, controls and layout metrics expressed through this helper follow `AppZoom` via
/// `Window::set_rem_size`, including the custom terminal renderer.
pub(crate) fn ui_size(px_at_100_percent: f32) -> Rems {
    rems(px_at_100_percent / DEFAULT_REM_SIZE_PX)
}

#[cfg(test)]
#[path = "app_zoom_tests.rs"]
mod tests;
