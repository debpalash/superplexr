//! Sidebar strip, hover card, and settings page for agent-provider usage.
//!
//! The strip sits at the bottom of the session navigator: one chip per
//! provider (Claude, Codex, OpenCode) with a short-window and a long-window
//! bar. Hovering shows the full reading; clicking opens the provider settings
//! page on that provider's tab. Everything is glyphs and bars; words are kept
//! for accessibility labels and the rare error string.

use gpui::{
    AnyElement, AnyView, Context, FontWeight, IntoElement, Render, Window, accesskit::Role, div,
    prelude::*, px, relative, svg,
};

use crate::{
    SuperplexrDesktop,
    app_zoom::ui_size,
    provider_usage::{
        ProviderKind, ProviderUsage, ProviderUsageSettings, ProviderUsageSnapshot,
        ProviderUsageStatus, UsageWindow, WindowSpan, format_age, format_percent, format_reset,
        now_unix,
    },
    theme::{
        ACTIVE, CHALK, ColorToken, DECK, FAULT, HAIRLINE, PANEL, PRODUCT_FONT, RELAY, SIGNAL,
        SUCCESS, TRACE, UI_FONT, rgb,
    },
};

const EXPANDED_STRIP_HEIGHT: f32 = 34.0;
const COLLAPSED_CHIP_HEIGHT: f32 = 30.0;
const MARK_SIZE: f32 = 18.0;
const LOGO_SIZE: f32 = 12.0;
const WARN_PERCENT: f32 = 75.0;
const CRITICAL_PERCENT: f32 = 90.0;

const GLYPH_SHORT_WINDOW: &str = "◷";
const GLYPH_LONG_WINDOW: &str = "▦";
const GLYPH_OTHER_WINDOW: &str = "·";
const GLYPH_RESET: &str = "↻";
const GLYPH_SETTINGS: &str = "⚙";
const GLYPH_READY: &str = "●";
const GLYPH_SIGNED_OUT: &str = "○";
const GLYPH_FAILED: &str = "△";
const GLYPH_LOADING: &str = "…";
const GLYPH_HIDDEN: &str = "◌";
const GLYPH_VISIBLE: &str = "▣";
const GLYPH_INVISIBLE: &str = "▢";

fn usage_token(percent: Option<f32>) -> ColorToken {
    match percent {
        None => TRACE,
        Some(value) if value >= CRITICAL_PERCENT => FAULT,
        Some(value) if value >= WARN_PERCENT => SIGNAL,
        Some(_) => RELAY,
    }
}

fn status_glyph(status: &ProviderUsageStatus) -> (&'static str, ColorToken) {
    match status {
        ProviderUsageStatus::Disabled => (GLYPH_HIDDEN, TRACE),
        ProviderUsageStatus::NotSignedIn(_) => (GLYPH_SIGNED_OUT, TRACE),
        ProviderUsageStatus::Loading => (GLYPH_LOADING, TRACE),
        ProviderUsageStatus::Ready(_) => (GLYPH_READY, SUCCESS),
        ProviderUsageStatus::Failed { .. } => (GLYPH_FAILED, FAULT),
    }
}

/// Spoken summary for assistive technology; the visual chip is glyph-only.
fn status_summary(snapshot: &ProviderUsageSnapshot) -> String {
    match &snapshot.status {
        ProviderUsageStatus::Disabled => "hidden".to_owned(),
        ProviderUsageStatus::NotSignedIn(_) => "not signed in".to_owned(),
        ProviderUsageStatus::Loading => "checking".to_owned(),
        ProviderUsageStatus::Ready(usage)
        | ProviderUsageStatus::Failed {
            previous: Some(usage),
            ..
        } => {
            let mut parts = usage
                .windows
                .iter()
                .map(|window| format!("{} {}", window.label, format_percent(window.used_percent)))
                .collect::<Vec<_>>();
            if matches!(snapshot.status, ProviderUsageStatus::Failed { .. }) {
                parts.push("refresh failed".to_owned());
            }
            parts.join(", ")
        }
        ProviderUsageStatus::Failed { .. } => "unavailable".to_owned(),
    }
}

fn bar(percent: Option<f32>, height: f32) -> gpui::Div {
    let fraction = percent.map_or(0.0, |value| (value / 100.0).clamp(0.0, 1.0));
    div()
        .h(px(height))
        .rounded_full()
        .bg(rgb(ACTIVE))
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(relative(fraction))
                .rounded_full()
                .bg(rgb(usage_token(percent))),
        )
}

fn brand_mark(kind: ProviderKind, token: ColorToken, dimmed: bool) -> AnyElement {
    div()
        .size(px(MARK_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .bg(rgb(ACTIVE))
        .when(dimmed, |mark| mark.opacity(0.55))
        .child(
            svg()
                .path(kind.icon_path())
                .size(px(LOGO_SIZE))
                .text_color(rgb(token)),
        )
        .into_any_element()
}

fn glyph(text: &'static str, token: ColorToken) -> AnyElement {
    div()
        .w(px(12.0))
        .flex_none()
        .text_color(rgb(token))
        .child(text)
        .into_any_element()
}

/// `◷ 5h ████░░ 42% ↻ 2h 13m`
fn window_row(window: &UsageWindow, now: i64) -> AnyElement {
    let percent = Some(window.used_percent);
    let (icon, token) = match window.span {
        WindowSpan::Short => (GLYPH_SHORT_WINDOW, CHALK),
        WindowSpan::Long => (GLYPH_LONG_WINDOW, CHALK),
        WindowSpan::Other => (GLYPH_OTHER_WINDOW, TRACE),
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(glyph(icon, token))
        .child(
            div()
                .w(px(56.0))
                .flex_none()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(token))
                .child(window.label.clone()),
        )
        .child(bar(percent, 4.0).flex_1().min_w(px(40.0)))
        .child(
            div()
                .w(px(32.0))
                .flex_none()
                .text_right()
                .text_color(rgb(usage_token(percent)))
                .child(format_percent(window.used_percent)),
        )
        .child(
            div()
                .w(px(58.0))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(3.0))
                .text_size(ui_size(10.0))
                .text_color(rgb(TRACE))
                .when_some(window.resets_at_unix, |cell, resets_at| {
                    cell.child(GLYPH_RESET).child(format_reset(resets_at, now))
                }),
        )
        .into_any_element()
}

fn state_row(icon: &'static str, token: ColorToken, text: String) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(glyph(icon, token))
        .child(div().text_color(rgb(token)).child(text))
        .into_any_element()
}

fn command_row(command: &'static str) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(glyph(">", TRACE))
        .child(
            div()
                .px(px(5.0))
                .py(px(1.0))
                .rounded(px(3.0))
                .bg(rgb(ACTIVE))
                .text_color(rgb(CHALK))
                .child(command),
        )
        .into_any_element()
}

/// Rows describing one snapshot: windows and notes when a reading exists,
/// plus a state row for anything other than a clean reading.
fn reading_rows(snapshot: &ProviderUsageSnapshot, now: i64) -> Vec<AnyElement> {
    let mut rows = Vec::new();
    if let Some(usage) = snapshot.usage() {
        rows.extend(usage.windows.iter().map(|window| window_row(window, now)));
        rows.extend(
            usage
                .notes
                .iter()
                .map(|note| state_row(GLYPH_OTHER_WINDOW, TRACE, note.clone())),
        );
    }
    match &snapshot.status {
        ProviderUsageStatus::Disabled => rows.push(state_row(GLYPH_HIDDEN, TRACE, String::new())),
        ProviderUsageStatus::NotSignedIn(detail) => {
            rows.push(state_row(GLYPH_SIGNED_OUT, TRACE, detail.clone()));
            rows.push(command_row(snapshot.kind.sign_in_command()));
        }
        ProviderUsageStatus::Loading => rows.push(state_row(GLYPH_LOADING, TRACE, String::new())),
        ProviderUsageStatus::Failed { detail, .. } => {
            rows.push(state_row(GLYPH_FAILED, FAULT, detail.clone()));
        }
        ProviderUsageStatus::Ready(_) => {}
    }
    rows
}

/// Hover card built on demand by GPUI's tooltip machinery.
pub(crate) struct ProviderUsageTooltip {
    snapshot: ProviderUsageSnapshot,
}

impl Render for ProviderUsageTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let now = now_unix();
        let snapshot = &self.snapshot;
        let plan = snapshot.usage().and_then(|usage| usage.plan.clone());
        let peak = snapshot.usage().and_then(ProviderUsage::peak_percent);
        let (state_icon, state_token) = status_glyph(&snapshot.status);
        div()
            .id(("provider-usage-tooltip", snapshot.kind.index()))
            .mt(px(-4.0))
            .p(px(10.0))
            .w(px(250.0))
            .flex()
            .flex_col()
            .gap(px(7.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(rgb(HAIRLINE))
            .bg(rgb(PANEL))
            .shadow_lg()
            .font_family(UI_FONT)
            .text_xs()
            .text_color(rgb(CHALK))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(brand_mark(snapshot.kind, usage_token(peak), false))
                    .child(
                        div()
                            .font_family(PRODUCT_FONT)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(ui_size(13.0))
                            .child(snapshot.kind.label()),
                    )
                    .when_some(plan, |header, plan| {
                        header.child(div().text_color(rgb(TRACE)).child(plan))
                    })
                    .child(div().ml_auto().text_color(rgb(state_token)).child(
                        if snapshot.refreshing {
                            GLYPH_LOADING
                        } else {
                            state_icon
                        },
                    )),
            )
            .children(reading_rows(snapshot, now))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .text_size(ui_size(10.0))
                    .text_color(rgb(TRACE))
                    .child(GLYPH_SETTINGS)
                    .when_some(snapshot.fetched_at_unix, |footer, fetched_at| {
                        footer
                            .child(div().ml_auto().child(GLYPH_RESET))
                            .child(format_age(fetched_at, now))
                    }),
            )
    }
}

impl SuperplexrDesktop {
    pub(crate) fn provider_usage_snapshot(&self, kind: ProviderKind) -> &ProviderUsageSnapshot {
        &self.provider_usage[kind.index()]
    }

    fn provider_usage_tooltip(
        &self,
        kind: ProviderKind,
    ) -> impl Fn(&mut Window, &mut gpui::App) -> AnyView + 'static {
        let snapshot = self.provider_usage_snapshot(kind).clone();
        move |_, cx| {
            let snapshot = snapshot.clone();
            cx.new(|_| ProviderUsageTooltip { snapshot }).into()
        }
    }

    fn provider_chip(
        &self,
        kind: ProviderKind,
        show_percent: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snapshot = self.provider_usage_snapshot(kind);
        let usage = snapshot.usage();
        let short = usage
            .and_then(ProviderUsage::short_window)
            .map(|window| window.used_percent);
        let long = usage
            .and_then(ProviderUsage::long_window)
            .map(|window| window.used_percent);
        let peak = usage.and_then(ProviderUsage::peak_percent);
        let dimmed = usage.is_none();
        let accessibility_label = format!("{} usage, {}", kind.label(), status_summary(snapshot));

        let chip = div()
            .id(("provider-usage", kind.index()))
            .role(Role::Button)
            .aria_label(accessibility_label)
            .cursor_pointer()
            .rounded(px(4.0))
            .hover(|chip| chip.bg(rgb(PANEL)))
            .tooltip(self.provider_usage_tooltip(kind))
            .on_click(cx.listener(move |desktop, _, _, cx| {
                cx.stop_propagation();
                desktop.open_provider_settings(Some(kind), cx);
            }));

        if self.sidebar_open {
            chip.h(px(26.0))
                .px(px(5.0))
                .flex()
                .items_center()
                .gap(px(5.0))
                .child(brand_mark(kind, usage_token(peak), dimmed))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(bar(short, 3.0).w(px(22.0)))
                        .child(bar(long, 3.0).w(px(22.0))),
                )
                .when(show_percent, |chip| {
                    chip.child(
                        div()
                            .font_family(UI_FONT)
                            .text_size(ui_size(9.0))
                            .text_color(rgb(usage_token(peak)))
                            .child(peak.map_or_else(|| "–".to_owned(), format_percent)),
                    )
                })
                .into_any_element()
        } else {
            chip.h(px(COLLAPSED_CHIP_HEIGHT))
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(3.0))
                .child(brand_mark(kind, usage_token(peak), dimmed))
                .child(bar(peak, 2.0).w(px(18.0)))
                .into_any_element()
        }
    }

    /// The bottom-of-sidebar strip. Adapts to the collapsed rail on its own.
    pub(crate) fn provider_usage_footer(
        &self,
        sidebar_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let show_percent = self.sidebar_open && sidebar_width >= 240.0;
        let chips = ProviderKind::ALL
            .iter()
            .map(|kind| self.provider_chip(*kind, show_percent, cx))
            .collect::<Vec<_>>();
        let strip = div()
            .id("provider-usage-footer")
            .debug_selector(|| "provider-usage-footer".to_owned())
            .role(Role::Group)
            .aria_label("Agent provider usage")
            .flex_none()
            .border_t_1()
            .border_color(rgb(HAIRLINE))
            .bg(rgb(DECK));
        if self.sidebar_open {
            strip
                .h(px(EXPANDED_STRIP_HEIGHT))
                .px(px(6.0))
                .flex()
                .items_center()
                .justify_between()
                .children(chips)
                .into_any_element()
        } else {
            strip
                .py(px(2.0))
                .flex()
                .flex_col()
                .children(chips)
                .into_any_element()
        }
    }

    pub(crate) fn open_provider_settings(
        &mut self,
        kind: Option<ProviderKind>,
        cx: &mut Context<Self>,
    ) {
        if let Some(kind) = kind {
            self.provider_settings_tab = kind;
        }
        self.provider_settings_open = true;
        self.command_deck_open = false;
        self.tab_overflow_open = false;
        self.session_sidebar.dismiss_session_overlays();
        cx.notify();
    }

    fn close_provider_settings(&mut self, cx: &mut Context<Self>) {
        self.provider_settings_open = false;
        cx.notify();
    }

    pub(crate) fn set_provider_enabled(
        &mut self,
        kind: ProviderKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.provider_usage_settings.enabled(kind) == enabled {
            return;
        }
        self.provider_usage_settings.set_enabled(kind, enabled);
        let snapshot = &mut self.provider_usage[kind.index()];
        if enabled {
            if snapshot.usage().is_none() {
                snapshot.status = ProviderUsageStatus::Loading;
            }
        } else {
            snapshot.status = ProviderUsageStatus::Disabled;
            snapshot.refreshing = false;
        }
        self.persist_workspaces();
        if enabled {
            self.refresh_provider_usage(cx);
        }
        cx.notify();
    }

    pub(crate) fn set_provider_refresh_interval(&mut self, seconds: u64, cx: &mut Context<Self>) {
        if self.provider_usage_settings.refresh_interval_secs == seconds {
            return;
        }
        self.provider_usage_settings.refresh_interval_secs = seconds;
        self.persist_workspaces();
        self.refresh_provider_usage(cx);
        cx.notify();
    }

    /// Kick off the first probe round and the recurring refresh.
    #[cfg(not(test))]
    pub(crate) fn start_provider_usage_polling(&mut self, cx: &mut Context<Self>) {
        self.refresh_provider_usage(cx);
    }

    /// Probe every enabled provider now and reschedule the next round. Any
    /// previously scheduled round is abandoned through the generation counter.
    #[cfg(not(test))]
    pub(crate) fn refresh_provider_usage(&mut self, cx: &mut Context<Self>) {
        self.provider_usage_generation = self.provider_usage_generation.wrapping_add(1);
        for kind in ProviderKind::ALL {
            self.probe_provider(kind, cx);
        }
        let generation = self.provider_usage_generation;
        let interval = self.provider_usage_settings.refresh_interval();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(interval).await;
            let _ = this.update(cx, |desktop, cx| {
                if desktop.provider_usage_generation == generation {
                    desktop.refresh_provider_usage(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn refresh_provider_usage(&mut self, cx: &mut Context<Self>) {
        for snapshot in &mut self.provider_usage {
            snapshot.refreshing = false;
        }
        cx.notify();
    }

    #[cfg(not(test))]
    fn probe_provider(&mut self, kind: ProviderKind, cx: &mut Context<Self>) {
        let snapshot = &mut self.provider_usage[kind.index()];
        if !self.provider_usage_settings.enabled(kind) {
            snapshot.status = ProviderUsageStatus::Disabled;
            snapshot.refreshing = false;
            return;
        }
        if snapshot.refreshing {
            return;
        }
        snapshot.refreshing = true;
        if snapshot.usage().is_none()
            && !matches!(snapshot.status, ProviderUsageStatus::NotSignedIn(_))
        {
            snapshot.status = ProviderUsageStatus::Loading;
        }
        let probe = cx.background_executor().spawn(async move {
            let environment = crate::provider_usage::ProbeEnvironment::from_process();
            crate::provider_usage::probe(kind, &environment)
        });
        cx.spawn(async move |this, cx| {
            let status = probe.await;
            let _ = this.update(cx, |desktop, cx| {
                let snapshot = &mut desktop.provider_usage[kind.index()];
                if desktop.provider_usage_settings.enabled(kind) {
                    snapshot.apply(status, now_unix());
                } else {
                    snapshot.status = ProviderUsageStatus::Disabled;
                    snapshot.refreshing = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The provider settings page: one tab per provider, a visibility switch,
    /// the live reading, and the refresh cadence.
    pub(crate) fn provider_settings(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.provider_settings_open {
            return None;
        }
        let now = now_unix();
        let selected = self.provider_settings_tab;
        let snapshot = self.provider_usage_snapshot(selected);
        let enabled = self.provider_usage_settings.enabled(selected);
        let refreshing = self
            .provider_usage
            .iter()
            .any(|snapshot| snapshot.refreshing);
        let plan = snapshot.usage().and_then(|usage| usage.plan.clone());
        let source = snapshot.usage().map(|usage| usage.source.clone());
        let (state_icon, state_token) = status_glyph(&snapshot.status);

        Some(
            div()
                .id("provider-settings-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(DECK).alpha(0.78))
                .on_click(cx.listener(|desktop, _, _, cx| {
                    desktop.close_provider_settings(cx);
                }))
                .child(
                    div()
                        .id("provider-settings")
                        .debug_selector(|| "provider-settings".to_owned())
                        .w(px(440.0))
                        .max_h(px(520.0))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(rgb(HAIRLINE))
                        .bg(rgb(PANEL))
                        .shadow_lg()
                        .font_family(UI_FONT)
                        .text_xs()
                        .text_color(rgb(CHALK))
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .h(px(44.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .border_b_1()
                                .border_color(rgb(HAIRLINE))
                                .child(div().text_color(rgb(TRACE)).child(GLYPH_SETTINGS))
                                .child(
                                    div()
                                        .font_family(PRODUCT_FONT)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_size(ui_size(13.0))
                                        .child("Providers"),
                                )
                                .child(
                                    div()
                                        .id("provider-settings-refresh")
                                        .role(Role::Button)
                                        .aria_label("Refresh usage now")
                                        .ml_auto()
                                        .size(px(26.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(3.0))
                                        .cursor_pointer()
                                        .text_color(rgb(RELAY))
                                        .hover(|button| button.bg(rgb(ACTIVE)))
                                        .on_click(cx.listener(|desktop, _, _, cx| {
                                            cx.stop_propagation();
                                            desktop.refresh_provider_usage(cx);
                                        }))
                                        .child(if refreshing {
                                            GLYPH_LOADING
                                        } else {
                                            GLYPH_RESET
                                        }),
                                )
                                .child(
                                    div()
                                        .id("provider-settings-close")
                                        .role(Role::Button)
                                        .aria_label("Close provider settings")
                                        .size(px(26.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(3.0))
                                        .cursor_pointer()
                                        .text_color(rgb(TRACE))
                                        .hover(|button| {
                                            button.bg(rgb(ACTIVE)).text_color(rgb(CHALK))
                                        })
                                        .on_click(cx.listener(|desktop, _, _, cx| {
                                            desktop.close_provider_settings(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .h(px(38.0))
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap_1()
                                .px_2()
                                .border_b_1()
                                .border_color(rgb(HAIRLINE))
                                .children(ProviderKind::ALL.iter().map(|kind| {
                                    let kind = *kind;
                                    let active = kind == selected;
                                    let tab_snapshot = self.provider_usage_snapshot(kind);
                                    let tab_peak =
                                        tab_snapshot.usage().and_then(ProviderUsage::peak_percent);
                                    let (tab_icon, tab_token) = status_glyph(&tab_snapshot.status);
                                    div()
                                        .id(("provider-settings-tab", kind.index()))
                                        .role(Role::Tab)
                                        .aria_label(format!("{} settings", kind.label()))
                                        .aria_selected(active)
                                        .h(px(28.0))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .rounded(px(4.0))
                                        .cursor_pointer()
                                        .bg(rgb(if active { ACTIVE } else { PANEL }))
                                        .text_color(rgb(if active { CHALK } else { TRACE }))
                                        .hover(|tab| tab.bg(rgb(ACTIVE)))
                                        .on_click(cx.listener(move |desktop, _, _, cx| {
                                            cx.stop_propagation();
                                            desktop.provider_settings_tab = kind;
                                            cx.notify();
                                        }))
                                        .child(brand_mark(kind, usage_token(tab_peak), false))
                                        .child(kind.label())
                                        .child(
                                            div()
                                                .text_size(ui_size(9.0))
                                                .text_color(rgb(tab_token))
                                                .child(tab_icon),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .id("provider-settings-body")
                                .flex()
                                .flex_col()
                                .min_h_0()
                                .overflow_y_scroll()
                                .p_3()
                                .gap_3()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .child(div().text_color(rgb(state_token)).child(state_icon))
                                        .child(
                                            div()
                                                .font_family(PRODUCT_FONT)
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_size(ui_size(14.0))
                                                .child(selected.label()),
                                        )
                                        .when_some(plan, |line, plan| {
                                            line.child(div().text_color(rgb(TRACE)).child(plan))
                                        })
                                        .when_some(source, |line, source| {
                                            line.child(
                                                div()
                                                    .text_size(ui_size(10.0))
                                                    .text_color(rgb(TRACE))
                                                    .child(format!("· {source}")),
                                            )
                                        })
                                        .child(self.provider_toggle(selected, enabled, cx)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(8.0))
                                        .p_3()
                                        .rounded(px(4.0))
                                        .border_1()
                                        .border_color(rgb(HAIRLINE))
                                        .bg(rgb(DECK))
                                        .children(reading_rows(snapshot, now)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.0))
                                        .child(div().text_color(rgb(TRACE)).child(GLYPH_RESET))
                                        .children(
                                            ProviderUsageSettings::REFRESH_CHOICES.iter().map(
                                                |(seconds, label)| {
                                                    let seconds = *seconds;
                                                    let active = self
                                                        .provider_usage_settings
                                                        .refresh_interval_secs
                                                        == seconds;
                                                    div()
                                                        .id((
                                                            "provider-refresh-choice",
                                                            seconds as usize,
                                                        ))
                                                        .role(Role::Button)
                                                        .aria_label(format!(
                                                            "Refresh every {label}"
                                                        ))
                                                        .h(px(22.0))
                                                        .px_2()
                                                        .flex()
                                                        .items_center()
                                                        .rounded(px(3.0))
                                                        .cursor_pointer()
                                                        .border_1()
                                                        .border_color(rgb(if active {
                                                            RELAY
                                                        } else {
                                                            HAIRLINE
                                                        }))
                                                        .bg(rgb(if active {
                                                            ACTIVE
                                                        } else {
                                                            PANEL
                                                        }))
                                                        .text_color(rgb(if active {
                                                            CHALK
                                                        } else {
                                                            TRACE
                                                        }))
                                                        .hover(|pill| pill.bg(rgb(ACTIVE)))
                                                        .on_click(cx.listener(
                                                            move |desktop, _, _, cx| {
                                                                cx.stop_propagation();
                                                                desktop
                                                                    .set_provider_refresh_interval(
                                                                        seconds, cx,
                                                                    );
                                                            },
                                                        ))
                                                        .child(*label)
                                                },
                                            ),
                                        )
                                        .when_some(snapshot.fetched_at_unix, |row, fetched_at| {
                                            row.child(
                                                div()
                                                    .ml_auto()
                                                    .text_size(ui_size(10.0))
                                                    .text_color(rgb(TRACE))
                                                    .child(format_age(fetched_at, now)),
                                            )
                                        }),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn provider_toggle(
        &self,
        kind: ProviderKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(("provider-visibility", kind.index()))
            .role(Role::Switch)
            .aria_label(format!(
                "Show {} in sidebar, {}",
                kind.label(),
                if enabled { "on" } else { "off" }
            ))
            .ml_auto()
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .on_click(cx.listener(move |desktop, _, _, cx| {
                cx.stop_propagation();
                let enabled = desktop.provider_usage_settings.enabled(kind);
                desktop.set_provider_enabled(kind, !enabled, cx);
            }))
            .child(
                div()
                    .text_color(rgb(if enabled { CHALK } else { TRACE }))
                    .child(if enabled {
                        GLYPH_VISIBLE
                    } else {
                        GLYPH_INVISIBLE
                    }),
            )
            .child(
                div()
                    .w(px(28.0))
                    .h(px(16.0))
                    .p(px(2.0))
                    .flex()
                    .items_center()
                    .when(enabled, |track| track.justify_end())
                    .rounded_full()
                    .bg(rgb(if enabled { RELAY } else { HAIRLINE }))
                    .child(div().size(px(12.0)).rounded_full().bg(rgb(CHALK))),
            )
            .into_any_element()
    }
}
