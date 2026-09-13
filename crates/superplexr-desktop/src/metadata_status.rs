//! Collection-sync failures belong in the product UI, not only stderr.
//! A stopped feed is not an empty snapshot and must never delete cached rows.
use gpui::{
    AnyElement, ClipboardItem, Context, FontWeight, IntoElement, Window, accesskit::Role, div,
    prelude::*, px,
};

use crate::{
    SuperplexrDesktop,
    app_zoom::ui_size,
    theme::{ACTIVE, CHALK, HAIRLINE, PANEL, SIGNAL, TRACE, UI_FONT, rgb},
};

impl SuperplexrDesktop {
    #[cfg(not(test))]
    pub(super) fn metadata_feed_stopped(
        &mut self,
        feed: &'static str,
        error: impl std::fmt::Display,
        cx: &mut Context<Self>,
    ) {
        let detail = error.to_string().chars().take(1024).collect::<String>();
        eprintln!("{feed} sync stopped: {detail}");
        self.metadata_retries.remove(feed);
        self.metadata_errors.insert(feed, detail);
        cx.notify();
    }

    #[cfg(not(test))]
    pub(super) fn metadata_feed_resumed(&mut self, feed: &'static str, cx: &mut Context<Self>) {
        self.metadata_retries.remove(feed);
        if self.metadata_errors.remove(feed).is_some() {
            cx.notify();
        }
    }

    #[cfg(not(test))]
    fn retry_metadata_sync(&mut self, cx: &mut Context<Self>) {
        let stopped: Vec<_> = self
            .metadata_errors
            .keys()
            .copied()
            .filter(|feed| !self.metadata_retries.contains(feed))
            .collect();
        for feed in stopped {
            self.metadata_retries.insert(feed);
            match feed {
                "Activity" => self.start_activity_updates(cx),
                "Terminals" => self.start_terminal_updates(cx),
                "Session groups" => self.start_session_group_updates(cx),
                "Missions" => self.start_mission_updates(cx),
                _ => {
                    self.metadata_retries.remove(feed);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn metadata_sync_warning(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.metadata_errors.is_empty() {
            return None;
        }
        // The viewport cap stays physical so high UI zoom cannot consume the
        // terminal area. The contents use the same zoom-aware scale as chrome.
        let height_limit = f32::from(window.viewport_size().height) * 0.25;
        Some(
            div()
                .id("session-sync-warning")
                .role(Role::Alert)
                .aria_label("Session list syncing stopped; displayed lists may be stale")
                .flex_none()
                .min_h_0()
                .max_h(px(height_limit))
                .overflow_y_scroll()
                .border_b_1()
                .border_color(rgb(HAIRLINE))
                .bg(rgb(PANEL))
                .font_family(UI_FONT)
                .text_xs()
                .p_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_normal()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(SIGNAL))
                                .child(if self.metadata_retries.is_empty() {
                                    "Session list syncing stopped — lists may be stale"
                                } else {
                                    "Reconnecting session lists — waiting for fresh data"
                                }),
                        )
                        .when(self.metadata_errors.keys().any(|feed| !self.metadata_retries.contains(feed)), |header| {
                            header.child(
                                div()
                                    .id("retry-session-sync")
                                    .role(Role::Button)
                                    .aria_label("Retry stopped session list syncing")
                                    .flex_none()
                                    .px_2()
                                    .py_1()
                                    .rounded(ui_size(3.0))
                                    .cursor_pointer()
                                    .text_color(rgb(SIGNAL))
                                    .hover(|button| button.bg(rgb(ACTIVE)))
                                    .on_click(cx.listener(|desktop, _, _, cx| {
                                        #[cfg(not(test))]
                                        desktop.retry_metadata_sync(cx);
                                        #[cfg(test)]
                                        let _ = (desktop, cx);
                                    }))
                                    .child("Retry sync"),
                            )
                        })
                        .child(
                            div()
                                .id("copy-session-sync-errors")
                                .role(Role::Button)
                                .aria_label("Copy session sync error details")
                                .flex_none()
                                .px_2()
                                .py_1()
                                .rounded(ui_size(3.0))
                                .cursor_pointer()
                                .text_color(rgb(CHALK))
                                .hover(|button| button.bg(rgb(ACTIVE)))
                                .on_click(cx.listener(|desktop, _, _, cx| {
                                    let details = desktop.metadata_errors.iter()
                                        .map(|(feed, error)| format!("{feed}: {error}"))
                                        .collect::<Vec<_>>()
                                        .join("\n");
                                    cx.write_to_clipboard(ClipboardItem::new_string(details));
                                }))
                                .child("Copy details"),
                        ),
                )
                .child(
                    div()
                        .text_color(rgb(TRACE))
                        .whitespace_normal()
                        .child("Resolve the reported error, then retry sync. This does not replay commands or take terminal control."),
                )
                .children(self.metadata_errors.iter().map(|(feed, error)| {
                    div()
                        .mt_1()
                        .text_color(rgb(CHALK))
                        .whitespace_normal()
                        .child(if self.metadata_retries.contains(feed) {
                            format!("{feed} (retrying): {error}")
                        } else {
                            format!("{feed}: {error}")
                        })
                }))
                .into_any_element(),
        )
    }
}
