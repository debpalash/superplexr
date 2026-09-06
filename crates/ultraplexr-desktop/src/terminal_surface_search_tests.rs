use super::*;

#[gpui::test]
fn desktop_feed_surface_rejects_old_attachments_and_clears_revoked_history(
    cx: &mut gpui::TestAppContext,
) {
    use crate::terminal_feed::Batch;
    let (surface, cx) = cx.add_window_view(|window, cx| TerminalSurface::new(window, cx).unwrap());
    cx.update(|_, cx| {
        surface.update(cx, |surface, cx| {
            let original = surface.frame.clone();
            surface.presented = true;
            surface.event_generation = 4;
            assert!(!surface.accept_feed_batch(
                3,
                Batch {
                    rejected: true,
                    continuity_lost: true,
                    ..Batch::default()
                },
                cx
            ));
            assert_eq!(surface.frame, original);
            surface.show_search_history(original.clone(), cx);
            assert!(surface.accept_feed_batch(
                4,
                Batch {
                    frame: Some(original.clone()),
                    ..Batch::default()
                },
                cx
            ));
            assert!(
                surface.search_live_frame.is_some(),
                "ordinary frames preserve local preview"
            );
            assert!(surface.accept_feed_batch(
                4,
                Batch {
                    continuity_lost: true,
                    frame: Some(original.clone()),
                    ..Batch::default()
                },
                cx
            ));
            assert!(surface.search_live_frame.is_none());
            assert!(!surface.writable);
            assert_eq!(surface.frame, original);
            surface.pending_paste = Some(PasteConfirmation {
                bytes: b"old command\n".to_vec(),
                risk: ultraplexr_terminal::PasteRisk::MultilineOrEscape,
            });
            surface.composition = "uncommitted".into();
            let mut exited = Batch::default();
            exited.notices[3] = Some(ServerEvent::TerminalExited {
                session_id: ultraplexr_core::SessionId::new(),
                code: 0,
                signal: None,
                success: true,
            });
            assert!(surface.accept_feed_batch(4, exited, cx));
            assert_eq!(surface.ended, Some(true));
            assert!(surface.pending_paste.is_none());
            assert!(surface.composition.is_empty());
            assert!(surface.accept_feed_batch(
                4,
                Batch {
                    continuity_lost: true,
                    rejected: true,
                    ..Batch::default()
                },
                cx
            ));
            assert!(surface.frame.rows.is_empty());
            surface.presented = false;
            assert!(!surface.accept_feed_batch(
                4,
                Batch {
                    frame: Some(original),
                    ..Batch::default()
                },
                cx
            ));
            assert!(surface.frame.rows.is_empty());
        })
    });
}

#[gpui::test]
fn search_history_banner_paints_and_returns_to_terminal(cx: &mut gpui::TestAppContext) {
    let (surface, cx) =
        cx.add_window_view(|window, cx| TerminalSurface::new(window, cx).expect("surface"));
    cx.update(|_, cx| {
        surface.update(cx, |surface, cx| {
            surface.show_search_history(surface.frame.clone(), cx)
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("close-search-history").is_some());
    cx.update(|_, cx| surface.update(cx, |surface, cx| surface.close_search_history(cx)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("close-search-history").is_none());
}

#[gpui::test]
fn search_history_is_local_read_only_copyable_and_restores_or_clears(
    cx: &mut gpui::TestAppContext,
) {
    let (surface, cx) =
        cx.add_window_view(|window, cx| TerminalSurface::new(window, cx).expect("surface"));
    cx.update(|_, cx| {
        surface.update(cx, |surface, cx| {
            let original = surface.frame.clone();
            let mut model = TerminalModel::new(GridSize {
                columns: 30,
                rows: 5,
            })
            .expect("model");
            model
                .advance(TerminalAction::Output("history 界 é\r\n".as_bytes()))
                .expect("output");
            surface.show_search_history(Arc::new(model.frame().expect("frame")), cx);
            let previous = surface.last_encoded.clone();
            assert!(!surface.apply(
                TerminalAction::Paste {
                    bytes: b"MUST-NOT-EXECUTE",
                    confirmed: true
                },
                cx
            ));
            assert_eq!(surface.last_encoded, previous);
            assert!(surface.frame.cursor.is_none());
            let grid = surface.requested_grid;
            surface.resize_to(
                GridSize {
                    columns: 50,
                    rows: 20,
                },
                8,
                16,
                cx,
            );
            assert_eq!(surface.requested_grid, grid);
            surface.copy_visible(cx);
            assert!(
                cx.read_from_clipboard()
                    .and_then(|item| item.text())
                    .expect("clipboard")
                    .contains("history 界 é")
            );
            surface.close_search_history(cx);
            assert_eq!(surface.frame, original);
            surface.show_search_history(Arc::new(model.frame().expect("frame")), cx);
            surface.invalidate_search_history(true, cx);
            assert!(surface.frame.rows.is_empty());
            assert!(surface.search_live_frame.is_none());
            assert!(!surface.writable);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
}
