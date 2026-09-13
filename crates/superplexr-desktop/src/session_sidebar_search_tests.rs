use super::*;

#[test]
fn history_hits_resolve_stable_groups_after_reorder_and_reject_removed_or_replaced_surfaces() {
    let session_id = superplexr_core::SessionId::new();
    let group_id = SessionGroupId::new();
    let hit = HistoryHit {
        group_id,
        surface_index: 7,
        session_id,
        found: superplexr_terminal::SearchMatch {
            line: 5,
            column: 1,
            preview: "needle".into(),
        },
    };
    let view = |id, terminals| SessionView {
        group_id: id,
        group_version: 0,
        pinned: false,
        automatic_name: false,
        name: "same display name".into(),
        actor: String::new(),
        status: SessionStatus::Idle,
        terminals,
    };
    let mut sessions = vec![
        view(group_id, vec![7]),
        view(SessionGroupId::new(), vec![2]),
    ];
    assert_eq!(
        history_hit_index(&sessions, &hit, Some(session_id)),
        Some(0)
    );
    sessions.swap(0, 1);
    assert_eq!(
        history_hit_index(&sessions, &hit, Some(session_id)),
        Some(1)
    );
    assert_eq!(
        history_hit_index(&sessions, &hit, Some(superplexr_core::SessionId::new())),
        None
    );
    sessions[1].terminals.clear();
    assert_eq!(history_hit_index(&sessions, &hit, Some(session_id)), None);
    sessions.remove(1);
    assert_eq!(history_hit_index(&sessions, &hit, Some(session_id)), None);
}

#[gpui::test]
fn search_cancel_control_paints_and_dismissal_clears_pending_work(cx: &mut gpui::TestAppContext) {
    let (desktop, cx) = cx.add_window_view(|window, cx| {
        let surfaces = crate::create_surfaces(window, cx);
        SuperplexrDesktop::new(surfaces, cx.focus_handle())
    });
    cx.update(|window, cx| {
        desktop.update(cx, |desktop, cx| {
            desktop.focus_sidebar_search(window, cx);
            desktop.session_sidebar.query = "query".into();
            desktop.refresh_history_search(cx);
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("cancel-history-search").is_some());
    cx.update(|_, cx| {
        desktop.update(cx, |desktop, cx| {
            desktop.session_sidebar.cancel_history(false);
            assert!(desktop.session_sidebar.history_task.is_none());
            assert_eq!(desktop.session_sidebar.history_note, "Search cancelled");
            desktop.session_sidebar.dismiss_session_overlays();
            assert!(desktop.session_sidebar.history_note.is_empty());
            assert!(
                desktop
                    .session_sidebar
                    .history
                    .as_ref()
                    .expect("worker")
                    .hits
                    .is_empty()
            );
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("cancel-history-search").is_none());
}
