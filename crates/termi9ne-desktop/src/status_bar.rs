//! Composable, responsive widgets for the desktop's bottom instrument rail.

use gpui::{AnyElement, IntoElement, div, prelude::*, px};

use crate::theme::{HAIRLINE, IntoThemeColor, PANEL, TRACE, UI_FONT, rgb};

pub(crate) const STATUS_BAR_HEIGHT: f32 = 28.0;

/// Controls when a widget yields space without teaching callers layout math.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WidgetPriority {
    Essential,
    Standard,
    Detailed,
}

impl WidgetPriority {
    fn visible_at(self, viewport_width: f32) -> bool {
        match self {
            Self::Essential => true,
            Self::Standard => viewport_width >= 760.0,
            Self::Detailed => viewport_width >= 1_120.0,
        }
    }
}

struct WidgetEntry {
    priority: WidgetPriority,
    element: AnyElement,
}

/// Owns alignment, responsive visibility, and separators for arbitrary widgets.
/// Widgets only provide their content and importance.
pub(crate) struct StatusBar {
    viewport_width: f32,
    leading: Vec<WidgetEntry>,
    trailing: Vec<WidgetEntry>,
}

impl StatusBar {
    pub(crate) fn new(viewport_width: f32) -> Self {
        Self {
            viewport_width,
            leading: Vec::new(),
            trailing: Vec::new(),
        }
    }

    pub(crate) fn leading(mut self, priority: WidgetPriority, element: AnyElement) -> Self {
        self.leading.push(WidgetEntry { priority, element });
        self
    }

    pub(crate) fn trailing(mut self, priority: WidgetPriority, element: AnyElement) -> Self {
        self.trailing.push(WidgetEntry { priority, element });
        self
    }

    pub(crate) fn into_any_element(self) -> AnyElement {
        div()
            .id("status-bar")
            .debug_selector(|| "status-bar".to_owned())
            .h(px(STATUS_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .overflow_hidden()
            .border_t_1()
            .border_color(rgb(HAIRLINE))
            .bg(rgb(PANEL))
            .font_family(UI_FONT)
            .text_xs()
            .child(widget_group(self.leading, self.viewport_width))
            .child(div().flex_1().min_w_0())
            .child(widget_group(self.trailing, self.viewport_width))
            .into_any_element()
    }
}

fn widget_group(entries: Vec<WidgetEntry>, viewport_width: f32) -> AnyElement {
    div()
        .h_full()
        .flex()
        .items_center()
        .children(
            entries
                .into_iter()
                .filter(move |entry| entry.priority.visible_at(viewport_width))
                .enumerate()
                .map(|(index, entry)| {
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .when(index > 0, |slot| {
                            slot.border_l_1().border_color(rgb(HAIRLINE))
                        })
                        .child(entry.element)
                }),
        )
        .into_any_element()
}

pub(crate) fn metric(
    id: &'static str,
    label: &str,
    value: String,
    color: impl IntoThemeColor,
) -> AnyElement {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .h_full()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .whitespace_nowrap()
        .child(div().text_color(rgb(TRACE)).child(label.to_owned()))
        .child(div().text_color(rgb(color)).child(value))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_priority_keeps_the_rail_useful_at_every_width() {
        assert!(WidgetPriority::Essential.visible_at(320.0));
        assert!(!WidgetPriority::Standard.visible_at(759.0));
        assert!(WidgetPriority::Standard.visible_at(760.0));
        assert!(!WidgetPriority::Detailed.visible_at(1_119.0));
        assert!(WidgetPriority::Detailed.visible_at(1_120.0));
    }
}
