//! Desktop surface for Faults: a sidebar card for what is currently broken,
//! and a panel that shows one Fault's evidence with replay and close actions.
//!
//! Glyph-first, in keeping with the rest of the navigator: `△` open, `↻`
//! replay, `●` still failing, `○` passes now.

use gpui::{
    AnyElement, Context, FontWeight, IntoElement, Window, accesskit::Role, div, prelude::*, px,
};
use termi9ne_core::FaultId;
use termi9ne_protocol::{FaultState, FaultSummary};

use crate::{
    Termi9neDesktop,
    theme::{
        ACTIVE, CHALK, ColorToken, DECK, FAULT, HAIRLINE, PANEL, PRODUCT_FONT, RELAY, SIGNAL,
        SUCCESS, TRACE, UI_FONT, rgb,
    },
};

const GLYPH_FAULT: &str = "△";
const GLYPH_REPLAY: &str = "↻";
const GLYPH_STILL_FAILS: &str = "●";
const GLYPH_PASSES: &str = "○";
const GLYPH_HANDOFF: &str = "→";
/// Faults shown in the sidebar card before it stops growing.
const SIDEBAR_FAULT_LIMIT: usize = 4;

/// How a Fault's latest replay reads at a glance.
fn replay_glyph(fault: &FaultSummary) -> (&'static str, ColorToken, String) {
    match &fault.repro {
        None => (GLYPH_REPLAY, TRACE, "not replayed".to_owned()),
        Some(receipt) => match (&receipt.error, receipt.reproduced) {
            (Some(error), _) => (GLYPH_FAULT, SIGNAL, error.clone()),
            (None, true) => (
                GLYPH_STILL_FAILS,
                FAULT,
                format!("still fails · {} ms", receipt.duration_ms),
            ),
            (None, false) => (
                GLYPH_PASSES,
                SUCCESS,
                format!("passes now · {} ms", receipt.duration_ms),
            ),
        },
    }
}

impl Termi9neDesktop {
    /// Sidebar card listing what is currently broken. Absent when nothing is.
    pub(crate) fn fault_sidebar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let open = self
            .faults
            .iter()
            .filter(|fault| fault.is_open())
            .collect::<Vec<_>>();
        if open.is_empty() {
            return None;
        }
        let total = open.len();

        Some(
            div()
                .id("fault-sidebar")
                .debug_selector(|| "fault-sidebar".to_owned())
                .role(Role::Group)
                .aria_label(format!("{total} open faults"))
                .mx_2()
                .mb_2()
                .rounded(px(5.0))
                .border_1()
                .border_color(rgb(FAULT).alpha(0.5))
                .bg(rgb(PANEL))
                .overflow_hidden()
                .child(
                    div()
                        .h(px(27.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .font_family(UI_FONT)
                        .text_size(px(10.0))
                        .text_color(rgb(FAULT))
                        .child(GLYPH_FAULT)
                        .child("Broken")
                        .child(div().ml_auto().child(total.to_string())),
                )
                .children(open.into_iter().take(SIDEBAR_FAULT_LIMIT).map(|fault| {
                    let fault_id = fault.fault_id;
                    let (glyph, token, _) = replay_glyph(fault);
                    div()
                        .id(("fault-row", fault_id.as_uuid().as_u128() as usize))
                        .h(px(34.0))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .hover(|row| row.bg(rgb(ACTIVE)))
                        .on_click(cx.listener(move |desktop, _, _, cx| {
                            desktop.open_fault_panel(Some(fault_id), cx);
                        }))
                        .child(div().text_size(px(9.0)).text_color(rgb(token)).child(glyph))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .font_family(PRODUCT_FONT)
                                        .text_size(px(11.0))
                                        .text_color(rgb(CHALK))
                                        .child(fault.command.clone()),
                                )
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .font_family(UI_FONT)
                                        .text_size(px(8.0))
                                        .text_color(rgb(TRACE))
                                        .child(fault.summary.clone()),
                                ),
                        )
                }))
                .when(total > SIDEBAR_FAULT_LIMIT, |card| {
                    card.child(
                        div()
                            .id("fault-sidebar-more")
                            .h(px(22.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .font_family(UI_FONT)
                            .text_size(px(9.0))
                            .text_color(rgb(TRACE))
                            .hover(|row| row.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
                            .on_click(cx.listener(|desktop, _, _, cx| {
                                desktop.open_fault_panel(None, cx);
                            }))
                            .child(format!("+{} more", total - SIDEBAR_FAULT_LIMIT)),
                    )
                })
                .into_any_element(),
        )
    }

    pub(crate) fn open_fault_panel(&mut self, fault_id: Option<FaultId>, cx: &mut Context<Self>) {
        if let Some(fault_id) = fault_id {
            self.selected_fault = Some(fault_id);
        } else if self.selected_fault.is_none() {
            self.selected_fault = self.faults.first().map(|fault| fault.fault_id);
        }
        self.fault_panel_open = true;
        self.command_deck_open = false;
        self.session_sidebar.dismiss_session_overlays();
        self.refresh_faults(cx);
        cx.notify();
    }

    fn close_fault_panel(&mut self, cx: &mut Context<Self>) {
        self.fault_panel_open = false;
        cx.notify();
    }

    fn selected_fault(&self) -> Option<&FaultSummary> {
        let fault_id = self.selected_fault?;
        self.faults.iter().find(|fault| fault.fault_id == fault_id)
    }

    /// Pull the current Fault list. Cheap enough to run whenever the panel is
    /// opened or an action completes.
    #[cfg(not(test))]
    pub(crate) fn refresh_faults(&mut self, cx: &mut Context<Self>) {
        if self.fault_busy {
            return;
        }
        self.fault_busy = true;
        let control = self.control.clone();
        let request = cx
            .background_executor()
            .spawn(async move { control.list_faults(None, None, true) });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.fault_busy = false;
                match result {
                    Ok(faults) => {
                        desktop.fault_error = None;
                        if desktop.selected_fault.is_none_or(|selected| {
                            !faults.iter().any(|fault| fault.fault_id == selected)
                        }) {
                            desktop.selected_fault = faults.first().map(|fault| fault.fault_id);
                        }
                        desktop.faults = faults;
                    }
                    Err(error) => desktop.fault_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn refresh_faults(&mut self, cx: &mut Context<Self>) {
        self.fault_busy = false;
        cx.notify();
    }

    /// Replay the selected Fault and refresh once the receipt lands.
    #[cfg(not(test))]
    fn replay_fault(&mut self, fault_id: FaultId, cx: &mut Context<Self>) {
        if self.fault_busy {
            return;
        }
        self.fault_busy = true;
        let control = self.control.clone();
        let request = cx
            .background_executor()
            .spawn(async move { control.reproduce_fault(fault_id, None) });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.fault_busy = false;
                match result {
                    Ok(fault) => {
                        desktop.fault_error = None;
                        desktop.replace_fault(fault);
                    }
                    Err(error) => desktop.fault_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    fn replay_fault(&mut self, _fault_id: FaultId, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// Close the selected Fault. The daemon refuses a resolve without a
    /// passing replay, so the surface offers dismiss as the honest fallback.
    #[cfg(not(test))]
    fn close_fault(&mut self, fault_id: FaultId, resolve: bool, cx: &mut Context<Self>) {
        if self.fault_busy {
            return;
        }
        self.fault_busy = true;
        let control = self.control.clone();
        let note = if resolve {
            "closed from the desktop after a passing replay".to_owned()
        } else {
            "dismissed from the desktop".to_owned()
        };
        let request = cx.background_executor().spawn(async move {
            if resolve {
                control.resolve_fault(fault_id, note)
            } else {
                control.dismiss_fault(fault_id, note)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |desktop, cx| {
                desktop.fault_busy = false;
                match result {
                    Ok(fault) => {
                        desktop.fault_error = None;
                        desktop.replace_fault(fault);
                    }
                    Err(error) => desktop.fault_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    fn close_fault(&mut self, _fault_id: FaultId, _resolve: bool, cx: &mut Context<Self>) {
        cx.notify();
    }

    pub(crate) fn replace_fault(&mut self, updated: FaultSummary) {
        match self
            .faults
            .iter_mut()
            .find(|fault| fault.fault_id == updated.fault_id)
        {
            Some(existing) => *existing = updated,
            None => self.faults.insert(0, updated),
        }
    }

    /// The Fault panel: a list on the left, the selected Fault's evidence on
    /// the right.
    pub(crate) fn fault_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.fault_panel_open {
            return None;
        }
        let selected_id = self.selected_fault;
        let selected = self.selected_fault();

        Some(
            div()
                .id("fault-panel-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgb(DECK).alpha(0.78))
                .on_click(cx.listener(|desktop, _, _, cx| desktop.close_fault_panel(cx)))
                .child(
                    div()
                        .id("fault-panel")
                        .debug_selector(|| "fault-panel".to_owned())
                        .w(px(760.0))
                        .max_h(px(560.0))
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
                                .child(div().text_color(rgb(FAULT)).child(GLYPH_FAULT))
                                .child(
                                    div()
                                        .font_family(PRODUCT_FONT)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_size(px(13.0))
                                        .child("Faults"),
                                )
                                .child(div().text_color(rgb(TRACE)).child(format!(
                                    "{} open",
                                    self.faults.iter().filter(|f| f.is_open()).count()
                                )))
                                .when_some(self.fault_error.clone(), |header, error| {
                                    header.child(div().text_color(rgb(FAULT)).child(error))
                                })
                                .child(
                                    div()
                                        .id("fault-refresh")
                                        .role(Role::Button)
                                        .aria_label("Refresh faults")
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
                                            desktop.refresh_faults(cx);
                                        }))
                                        .child(if self.fault_busy { "…" } else { GLYPH_REPLAY }),
                                )
                                .child(
                                    div()
                                        .id("fault-panel-close")
                                        .role(Role::Button)
                                        .aria_label("Close faults")
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
                                            desktop.close_fault_panel(cx);
                                        }))
                                        .child("×"),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    div()
                                        .id("fault-list")
                                        .w(px(240.0))
                                        .flex_none()
                                        .flex()
                                        .flex_col()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .border_r_1()
                                        .border_color(rgb(HAIRLINE))
                                        .children(self.faults.iter().map(|fault| {
                                            let fault_id = fault.fault_id;
                                            let active = selected_id == Some(fault_id);
                                            let (glyph, token, _) = replay_glyph(fault);
                                            div()
                                                .id((
                                                    "fault-list-row",
                                                    fault_id.as_uuid().as_u128() as usize,
                                                ))
                                                .role(Role::Tab)
                                                .aria_selected(active)
                                                .h(px(40.0))
                                                .px_2()
                                                .flex()
                                                .items_center()
                                                .gap_2()
                                                .cursor_pointer()
                                                .bg(rgb(if active { ACTIVE } else { PANEL }))
                                                .hover(|row| row.bg(rgb(ACTIVE)))
                                                .on_click(cx.listener(move |desktop, _, _, cx| {
                                                    desktop.selected_fault = Some(fault_id);
                                                    cx.notify();
                                                }))
                                                .child(
                                                    div()
                                                        .text_size(px(9.0))
                                                        .text_color(rgb(if fault.is_open() {
                                                            token
                                                        } else {
                                                            TRACE
                                                        }))
                                                        .child(if fault.is_open() {
                                                            glyph
                                                        } else {
                                                            GLYPH_PASSES
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .whitespace_nowrap()
                                                        .text_color(rgb(if fault.is_open() {
                                                            CHALK
                                                        } else {
                                                            TRACE
                                                        }))
                                                        .child(fault.command.clone()),
                                                )
                                        })),
                                )
                                .child(match selected {
                                    Some(fault) => self.fault_detail(fault, cx),
                                    None => div()
                                        .flex_1()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(rgb(TRACE))
                                        .child("nothing broken")
                                        .into_any_element(),
                                }),
                        ),
                )
                .into_any_element(),
        )
    }

    fn fault_detail(&self, fault: &FaultSummary, cx: &mut Context<Self>) -> AnyElement {
        let fault_id = fault.fault_id;
        let (glyph, token, replay_text) = replay_glyph(fault);
        let can_resolve = fault.repro_passes() && fault.is_open();
        let state = match &fault.state {
            FaultState::Open => "open".to_owned(),
            FaultState::Resolved { note, .. } => format!("resolved · {note}"),
            FaultState::Dismissed { note, .. } => format!("dismissed · {note}"),
        };

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_3()
                    .border_b_1()
                    .border_color(rgb(HAIRLINE))
                    .child(
                        div()
                            .font_family(PRODUCT_FONT)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(13.0))
                            .child(fault.summary.clone()),
                    )
                    .child(
                        div()
                            .text_color(rgb(TRACE))
                            .child(format!("$ {}", fault.command)),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(rgb(TRACE))
                            .child(fault.cwd.display().to_string()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(10.0))
                            .child(div().text_color(rgb(token)).child(glyph))
                            .child(div().text_color(rgb(token)).child(replay_text))
                            .child(div().text_color(rgb(TRACE)).child(format!("· {state}"))),
                    ),
            )
            .child(
                div()
                    .id(("fault-output", fault_id.as_uuid().as_u128() as usize))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .bg(rgb(DECK))
                    .text_size(px(11.0))
                    .text_color(rgb(CHALK))
                    .child(if fault.output.trim().is_empty() {
                        "no output captured".to_owned()
                    } else {
                        fault.output.clone()
                    }),
            )
            .child(
                div()
                    .h(px(38.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .border_t_1()
                    .border_color(rgb(HAIRLINE))
                    .child(fault_action(
                        "fault-replay",
                        "Replay this fault",
                        GLYPH_REPLAY,
                        RELAY,
                        cx.listener(move |desktop, _, _, cx| {
                            cx.stop_propagation();
                            desktop.replay_fault(fault_id, cx);
                        }),
                    ))
                    .child(fault_action(
                        "fault-handoff",
                        "Send this fault to the active terminal",
                        GLYPH_HANDOFF,
                        CHALK,
                        cx.listener(move |desktop, _, _, cx| {
                            cx.stop_propagation();
                            desktop.hand_off_fault(fault_id, cx);
                        }),
                    ))
                    .child(div().flex_1())
                    .when(can_resolve, |bar| {
                        bar.child(fault_action(
                            "fault-resolve",
                            "Close this fault as fixed",
                            "✓",
                            SUCCESS,
                            cx.listener(move |desktop, _, _, cx| {
                                cx.stop_propagation();
                                desktop.close_fault(fault_id, true, cx);
                            }),
                        ))
                    })
                    .when(fault.is_open(), |bar| {
                        bar.child(fault_action(
                            "fault-dismiss",
                            "Dismiss this fault",
                            "×",
                            TRACE,
                            cx.listener(move |desktop, _, _, cx| {
                                cx.stop_propagation();
                                desktop.close_fault(fault_id, false, cx);
                            }),
                        ))
                    }),
            )
            .into_any_element()
    }

    /// Paste a compact brief for this Fault into the active terminal, so an
    /// agent running there can pick the work up without re-reading scrollback.
    fn hand_off_fault(&mut self, fault_id: FaultId, cx: &mut Context<Self>) {
        let Some(fault) = self
            .faults
            .iter()
            .find(|fault| fault.fault_id == fault_id)
            .cloned()
        else {
            return;
        };
        let Some(terminal) = self.active_terminal_index() else {
            return;
        };
        let brief = format!(
            "Fix this failure.\ncommand: {}\ncwd: {}\nexit: {}\n\n{}\n",
            fault.command,
            fault.cwd.display(),
            fault
                .exit_code
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string()),
            fault.output.trim()
        );
        if let Some(surface) = self.surfaces.get(terminal).cloned() {
            surface.update(cx, |surface, cx| surface.paste_text(&brief, cx));
        }
        self.fault_panel_open = false;
        cx.notify();
    }
}

fn fault_action(
    id: &'static str,
    label: &'static str,
    glyph: &'static str,
    token: ColorToken,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .size(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.0))
        .cursor_pointer()
        .text_color(rgb(token))
        .hover(|button| button.bg(rgb(ACTIVE)).text_color(rgb(CHALK)))
        .on_click(on_click)
        .child(glyph)
        .into_any_element()
}
