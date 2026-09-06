use super::*;
use crate::{UltraplexrDesktop, create_surfaces};

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
fn zoom_is_bounded_and_every_stop_scales_text_and_control_metrics_together() {
    let mut zoom = AppZoom::default();
    while zoom.decrease() {}
    assert_eq!(zoom.percentage(), 50);
    assert!(!zoom.decrease());
    let mut observed = Vec::new();
    loop {
        observed.push(zoom.percentage());
        let text = f32::from(ui_size(12.0).to_pixels(zoom.rem_size()));
        let row = f32::from(ui_size(46.0).to_pixels(zoom.rem_size()));
        assert!((text - 12.0 * zoom.factor()).abs() < 0.001);
        assert!((row / text - 46.0 / 12.0).abs() < 0.001);
        if !zoom.increase() {
            break;
        }
    }
    assert_eq!(observed, [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200]);
    assert!(!zoom.increase());
}

#[gpui::test]
fn zoom_overlay_caps_follow_current_viewport_after_resize(cx: &mut gpui::TestAppContext) {
    let (_, cx) = cx.add_window_view(|window, cx| {
        let surfaces = create_surfaces(window, cx);
        UltraplexrDesktop::new(surfaces, cx.focus_handle())
    });
    for (width, height) in [(1440.0, 900.0), (640.0, 360.0), (1024.0, 768.0)] {
        cx.simulate_resize(gpui::size(px(width), px(height)));
        cx.update(|window, _| {
            let mut zoom = AppZoom::default();
            while zoom.decrease() {}
            loop {
                assert_eq!(
                    zoom.overlay_width(window, 820.0),
                    px((820.0 * zoom.factor()).min(width * 0.95))
                );
                assert_eq!(
                    zoom.overlay_height(window, 580.0),
                    px((580.0 * zoom.factor()).min(height * 0.90))
                );
                assert_eq!(zoom.overlay_height(window, f32::MAX), px(height * 0.90));
                if !zoom.increase() {
                    break;
                }
            }
        });
    }
}

#[gpui::test]
fn zoom_responsive_columns_and_sidebar_drag_use_design_coordinates(cx: &mut gpui::TestAppContext) {
    let (desktop, cx) = cx.add_window_view(|window, cx| {
        let surfaces = create_surfaces(window, cx);
        UltraplexrDesktop::new(surfaces, cx.focus_handle())
    });
    cx.update(|window, cx| {
        desktop.update(cx, |desktop, cx| {
            let (_, reference_columns) = desktop.grid_columns(1440.0);
            assert_eq!(reference_columns.len(), 2);
            while desktop.app_zoom.increase() {}
            desktop.apply_app_zoom(window, cx);
            assert_eq!(
                desktop.grid_columns(1440.0).1.len(),
                1,
                "200% must reduce the responsive column count"
            );
            assert_eq!(
                desktop.grid_columns(2880.0).1,
                reference_columns,
                "equivalent design width keeps column assignment"
            );
            desktop.set_sidebar_custom_width(640.0, 2880.0, cx);
            assert_eq!(
                desktop.sidebar_custom_width,
                Some(320.0),
                "persist width in design units"
            );
            desktop.reset_app_zoom(window, cx);
            assert_eq!(
                desktop.sidebar_custom_width,
                Some(320.0),
                "reset does not rewrite the user's size"
            );
            desktop.set_sidebar_custom_width(0.0, 1440.0, cx);
            assert_eq!(
                desktop.sidebar_custom_width,
                Some(crate::pane_layout::SIDEBAR_MIN_WIDTH)
            );
        });
    });
}
