use super::*;
use gpui::{Bounds, Pixels, size};

fn contains_bounds(outer: Bounds<Pixels>, inner: Bounds<Pixels>) {
    let tolerance = px(1.0);
    assert!(
        inner.left() >= outer.left() - tolerance,
        "left edge escaped: {inner:?} in {outer:?}"
    );
    assert!(
        inner.top() >= outer.top() - tolerance,
        "top edge escaped: {inner:?} in {outer:?}"
    );
    assert!(
        inner.right() <= outer.right() + tolerance,
        "right edge escaped: {inner:?} in {outer:?}"
    );
    assert!(
        inner.bottom() <= outer.bottom() + tolerance,
        "bottom edge escaped: {inner:?} in {outer:?}"
    );
}

#[gpui::test]
fn zoom_sidebar_rows_and_labels_scale_together_at_every_stop(cx: &mut gpui::TestAppContext) {
    let (desktop, cx) = cx.add_window_view(|window, cx| {
        let surfaces = crate::create_surfaces(window, cx);
        SuperplexrDesktop::new(surfaces, cx.focus_handle())
    });
    cx.simulate_resize(size(px(1280.0), px(900.0)));
    cx.update(|window, cx| {
        desktop.update(cx, |desktop, cx| {
            desktop.toggle_session_expansion(0, cx);
            while desktop.app_zoom.decrease() {}
            desktop.apply_app_zoom(window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.executor()
        .advance_clock(EXPANSION_DURATION + Duration::from_millis(1));
    loop {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let factor = cx.update(|_, cx| desktop.read(cx).app_zoom.factor());
        let row = cx
            .debug_bounds("session-row-0")
            .expect("expanded Session row");
        let name = cx.debug_bounds("session-name-0").expect("Session name");
        let detail = cx.debug_bounds("session-detail-0").expect("Session detail");
        let child = cx
            .debug_bounds("session-terminal-row-0")
            .expect("nested terminal row");
        assert!((f32::from(row.size.height) - SESSION_ROW_HEIGHT * factor).abs() < 1.0);
        assert!((f32::from(child.size.height) - TERMINAL_CHILD_HEIGHT * factor).abs() < 1.0);
        assert!(name.size.height > px(0.0) && detail.size.height > px(0.0));
        contains_bounds(row, name);
        contains_bounds(row, detail);
        assert!(
            name.bottom() <= detail.top() + px(1.0),
            "name and detail overlap after zoom"
        );
        let sidebar = cx.debug_bounds("session-sidebar").expect("sidebar");
        assert!(
            (f32::from(sidebar.size.width) - sidebar_width(1280.0 / factor, true, None) * factor)
                .abs()
                < 1.0
        );
        let changed = cx.update(|window, cx| {
            desktop.update(cx, |desktop, cx| {
                let changed = desktop.app_zoom.increase();
                desktop.apply_app_zoom(window, cx);
                changed
            })
        });
        if !changed {
            break;
        }
    }
    cx.update(|window, cx| desktop.update(cx, |desktop, cx| desktop.reset_app_zoom(window, cx)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        (f32::from(
            cx.debug_bounds("session-row-0")
                .expect("reset row")
                .size
                .height
        ) - SESSION_ROW_HEIGHT)
            .abs()
            < 1.0
    );
    cx.update(|window, cx| {
        desktop.update(cx, |desktop, cx| {
            desktop.toggle_sidebar(cx);
            while desktop.app_zoom.increase() {}
            desktop.apply_app_zoom(window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.debug_bounds("session-sidebar")
            .expect("collapsed sidebar")
            .size
            .width,
        px(COLLAPSED_WIDTH * 2.0)
    );
    assert_eq!(
        cx.debug_bounds("collapsed-session-row-0")
            .expect("collapsed row")
            .size
            .height,
        px(64.0)
    );
}

#[gpui::test]
fn zoom_session_popups_stay_inside_small_resized_windows_without_squeezing_actions(
    cx: &mut gpui::TestAppContext,
) {
    let (desktop, cx) = cx.add_window_view(|window, cx| {
        let surfaces = crate::create_surfaces(window, cx);
        SuperplexrDesktop::new(surfaces, cx.focus_handle())
    });
    cx.update(|window, cx| {
        desktop.update(cx, |desktop, cx| {
            while desktop.app_zoom.increase() {}
            desktop.apply_app_zoom(window, cx);
            desktop.toggle_session_menu(0, window, cx);
        })
    });
    for (width, height) in [(1280.0, 900.0), (640.0, 360.0), (800.0, 500.0)] {
        cx.simulate_resize(size(px(width), px(height)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let menu = cx
            .debug_bounds("session-menu-0")
            .expect("anchored Session menu");
        let action = cx
            .debug_bounds("session-rename-action-0")
            .expect("rename action");
        contains_bounds(
            Bounds::new(gpui::point(px(0.0), px(0.0)), size(px(width), px(height))),
            menu,
        );
        assert!(menu.size.width <= px(width * 0.95 + 1.0));
        assert!(menu.size.height <= px(height * 0.90 + 1.0));
        assert!(
            (f32::from(action.size.height) - 60.0).abs() < 1.0,
            "scrolling must not squeeze menu action rows"
        );
    }
    cx.update(|window, cx| {
        desktop.update(cx, |desktop, cx| {
            desktop.begin_session_rename(0, window, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    contains_bounds(
        cx.debug_bounds("session-row-0").expect("row"),
        cx.debug_bounds("session-rename-0").expect("rename editor"),
    );
    cx.update(|_, cx| {
        desktop.update(cx, |desktop, cx| {
            desktop.session_sidebar.dismiss_session_overlays();
            desktop.request_session_termination_at(0, cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    contains_bounds(
        Bounds::new(gpui::point(px(0.0), px(0.0)), size(px(800.0), px(500.0))),
        cx.debug_bounds("session-terminate-confirm-0")
            .expect("termination confirmation"),
    );
}
