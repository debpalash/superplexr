use super::*;
use crate::workspace_tabs::WorkspaceId;
use ultraplexr_core::MissionId;

fn session(group_id: SessionGroupId, version: u64, terminal: usize) -> SessionView {
    SessionView {
        group_id,
        group_version: version,
        pinned: false,
        automatic_name: true,
        name: format!("terminal {terminal}"),
        actor: "you".into(),
        status: SessionStatus::Idle,
        terminals: vec![terminal],
    }
}

fn workspace(mission: MissionId, sessions: Vec<SessionView>, selected: usize) -> WorkspaceView {
    WorkspaceView {
        sessions,
        selected_session: selected,
        focus_mode: false,
        mission_id: Some(mission),
        mission: None,
    }
}

fn selected_id(workspace: &WorkspaceView) -> SessionGroupId {
    workspace.sessions[workspace.selected_session].group_id
}

#[test]
fn deleted_group_is_removed_from_every_duplicate_while_surviving_selection_stays_stable() {
    let mission = MissionId::new();
    let deleted = SessionGroupId::new();
    let kept = SessionGroupId::new();
    let other = SessionGroupId::new();
    let original = workspace(
        mission,
        vec![session(deleted, 7, 0), session(kept, 4, 1)],
        1,
    );
    let mut tabs = WorkspaceTabs::restore(
        vec![
            (
                WorkspaceId::from_raw(11),
                "Mission".into(),
                true,
                original.clone(),
            ),
            (
                WorkspaceId::from_raw(23),
                "Duplicate".into(),
                false,
                original,
            ),
            (
                WorkspaceId::from_raw(40),
                "Other".into(),
                false,
                workspace(MissionId::new(), vec![session(other, 3, 2)], 0),
            ),
        ],
        WorkspaceId::from_raw(23),
    )
    .unwrap();
    let mut versions = HashMap::from([
        (deleted, (7, "deleted".into())),
        (kept, (4, "kept".into())),
        (other, (3, "other".into())),
    ]);

    remove_group(&mut tabs, &mut versions, deleted);

    assert_eq!(tabs.active_id(), WorkspaceId::from_raw(23));
    assert_eq!(tabs.tabs().len(), 3);
    for tab in &tabs.tabs()[..2] {
        let view = tab.content();
        assert_eq!(view.sessions.len(), 1);
        assert_eq!(view.mission_id, Some(mission));
        assert_eq!(view.selected_session, 0);
        assert_eq!(selected_id(view), kept);
        assert_eq!(view.sessions[0].group_version, 4);
        assert_eq!(view.sessions[0].terminals, [1]);
        assert!(view.sessions[0].automatic_name);
    }
    assert!(tabs.tabs()[0].pinned());
    assert_eq!(tabs.tabs()[1].title(), "Duplicate");
    assert_eq!(selected_id(tabs.tabs()[2].content()), other);
    assert!(!versions.contains_key(&deleted));
    assert_eq!(versions.len(), 2);
}

#[test]
fn deleting_selected_row_uses_next_position_or_previous_last_row() {
    let first = SessionGroupId::new();
    let middle = SessionGroupId::new();
    let last = SessionGroupId::new();
    let mission = MissionId::new();
    for (selected, deleted, fallback, index) in [(1, middle, last, 1), (2, last, middle, 1)] {
        let mut tabs = WorkspaceTabs::new(vec![(
            "Mission".into(),
            workspace(
                mission,
                vec![
                    session(first, 1, 0),
                    session(middle, 1, 1),
                    session(last, 1, 2),
                ],
                selected,
            ),
        )])
        .unwrap();
        remove_group(&mut tabs, &mut HashMap::new(), deleted);
        let view = tabs.tabs()[0].content();
        assert_eq!(view.selected_session, index);
        assert_eq!(selected_id(view), fallback);
        assert_eq!(view.sessions.len(), 2);
    }
}

#[test]
fn deleting_last_group_keeps_only_history_and_cannot_turn_deleted_group_into_unsaved_work() {
    let deleted = SessionGroupId::new();
    let mission = MissionId::new();
    let view = workspace(mission, vec![session(deleted, 9, 42)], 0);
    let mut tabs = WorkspaceTabs::new(vec![
        ("First".into(), view.clone()),
        ("Second".into(), view),
    ])
    .unwrap();
    let mut versions = HashMap::from([(deleted, (9, "old group".into()))]);
    remove_group(&mut tabs, &mut versions, deleted);
    let placeholder_ids: Vec<_> = tabs
        .tabs()
        .iter()
        .map(|tab| selected_id(tab.content()))
        .collect();
    // Repeated deletion and a subsequent empty complete snapshot are safe. They
    // must not synthesize a version-zero copy of the old group or its PTY list.
    remove_group(&mut tabs, &mut versions, deleted);
    reconcile_groups(&mut tabs, &mut versions, &HashSet::new());
    for (tab, placeholder_id) in tabs.tabs().iter().zip(placeholder_ids) {
        let view = tab.content();
        assert_eq!(view.sessions.len(), 1);
        assert_eq!(view.selected_session, 0);
        assert_eq!(view.mission_id, Some(mission));
        let history = &view.sessions[0];
        assert_eq!(history.group_id, placeholder_id);
        assert_ne!(history.group_id, deleted);
        assert_eq!(history.group_version, 0);
        assert_eq!(history.name, "mission history");
        assert!(history.terminals.is_empty());
        assert!(matches!(history.status, SessionStatus::Closed));
        assert!(!history.pinned && !history.automatic_name);
    }
    assert!(versions.is_empty());
}

#[test]
fn completed_membership_prunes_restored_authoritative_rows_and_preserves_local_groups() {
    let absent = SessionGroupId::new();
    let live = SessionGroupId::new();
    let local = SessionGroupId::new();
    let cache_only = SessionGroupId::new();
    let mission = MissionId::new();
    let first = workspace(
        mission,
        vec![
            session(absent, 6, 0),
            session(local, 0, 1),
            session(live, 2, 2),
        ],
        1,
    );
    let mut tabs = WorkspaceTabs::restore(
        vec![
            (
                WorkspaceId::from_raw(7),
                "Restored".into(),
                false,
                first.clone(),
            ),
            (
                WorkspaceId::from_raw(19),
                "Restored duplicate".into(),
                false,
                first,
            ),
        ],
        WorkspaceId::from_raw(7),
    )
    .unwrap();
    let mut versions = HashMap::from([
        (absent, (6, "gone".into())),
        (live, (2, "live".into())),
        (cache_only, (8, "not represented".into())),
    ]);

    reconcile_groups(&mut tabs, &mut versions, &HashSet::from([live]));

    for tab in tabs.tabs() {
        let view = tab.content();
        assert_eq!(
            view.sessions
                .iter()
                .map(|row| row.group_id)
                .collect::<Vec<_>>(),
            [local, live]
        );
        assert_eq!(selected_id(view), local);
        assert_eq!(view.sessions[0].group_version, 0);
        assert_eq!(view.sessions[0].terminals, [1]);
        assert_eq!(view.sessions[0].name, "terminal 1");
    }
    assert_eq!(versions, HashMap::from([(live, (2, "live".into()))]));
    // An empty *complete* result removes every authoritative group, but it does
    // not say that a local unsaved view was deleted by the runtime.
    reconcile_groups(&mut tabs, &mut versions, &HashSet::new());
    assert!(versions.is_empty());
    for tab in tabs.tabs() {
        assert_eq!(tab.content().sessions.len(), 1);
        assert_eq!(selected_id(tab.content()), local);
    }
}

#[test]
fn individual_group_updates_do_not_treat_unseen_groups_as_deleted() {
    let mission = MissionId::new();
    let absent = SessionGroupId::new();
    let live = SessionGroupId::new();
    let terminal = SessionId::new();
    let mut tabs = WorkspaceTabs::new(vec![(
        "Mission".into(),
        workspace(mission, vec![session(absent, 4, 0), session(live, 2, 1)], 0),
    )])
    .unwrap();
    let mut versions = HashMap::from([(absent, (4, "old".into()))]);
    let group = SessionGroupSummary {
        group_id: live,
        version: 3,
        mission_id: Some(mission),
        name: "present".into(),
        session_ids: vec![terminal],
        position: 1,
        pinned: false,
        detached: false,
    };
    assert!(apply_group(
        &mut tabs,
        &group,
        &HashMap::from([(terminal, 1)]),
        &ViewDismissals::default(),
        SessionStatus::Idle,
        &mut versions
    ));
    assert_eq!(tabs.tabs()[0].content().sessions.len(), 2);
    assert_eq!(selected_id(tabs.tabs()[0].content()), absent);
    assert!(versions.contains_key(&absent));

    // This call represents a completed full-scope list/snapshot. The caller,
    // using CollectionTracker, is responsible for withholding it on partial,
    // legacy item-only, disconnected, or mismatched-generation deliveries.
    reconcile_groups(&mut tabs, &mut versions, &HashSet::from([live]));
    assert_eq!(tabs.tabs()[0].content().sessions.len(), 1);
    assert_eq!(selected_id(tabs.tabs()[0].content()), live);
    assert_eq!(tabs.tabs()[0].content().sessions[0].group_version, 3);
}
