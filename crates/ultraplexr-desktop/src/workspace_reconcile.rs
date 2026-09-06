//! Reconcile shared runtime metadata without depending on a window or UI executor.
use crate::{
    SessionStatus, SessionView, WorkspaceView, workspace_store::ViewDismissals,
    workspace_tabs::WorkspaceTabs,
};
use std::collections::{HashMap, HashSet};
use ultraplexr_core::SessionId;
use ultraplexr_protocol::{SessionGroupId, SessionGroupSummary};

#[cfg(test)]
#[path = "workspace_reconcile_tests.rs"]
mod tests;

pub(crate) fn close_view(
    tabs: &mut WorkspaceTabs<WorkspaceView>,
    dismissals: &mut ViewDismissals,
    id: crate::workspace_tabs::WorkspaceId,
) -> Result<(), crate::workspace_tabs::TabError> {
    let mission = tabs.tab(id).and_then(|tab| tab.content().mission_id);
    tabs.close(id)?;
    if let Some(mission) = mission
        && !tabs
            .tabs()
            .iter()
            .any(|tab| tab.content().mission_id == Some(mission))
    {
        dismissals.missions.insert(mission);
    }
    Ok(())
}

pub(crate) fn apply_group(
    workspaces: &mut WorkspaceTabs<WorkspaceView>,
    group: &SessionGroupSummary,
    surfaces: &HashMap<SessionId, usize>,
    dismissals: &ViewDismissals,
    status: SessionStatus,
    versions: &mut HashMap<SessionGroupId, (u64, String)>,
) -> bool {
    if versions
        .get(&group.group_id)
        .is_some_and(|(version, _)| *version > group.version)
    {
        return false;
    }
    let renamed = versions
        .get(&group.group_id)
        .is_some_and(|(_, name)| *name != group.name);
    versions.insert(group.group_id, (group.version, group.name.clone()));
    let terminals: Vec<_> = group
        .session_ids
        .iter()
        .filter(|id| dismissals.allows_session(**id, group.mission_id))
        .filter_map(|id| surfaces.get(id).copied())
        .collect();
    let active = workspaces.active_id();
    for tab in workspaces.tabs_mut() {
        let target = group.mission_id.map_or(tab.id() == active, |id| {
            tab.content().mission_id == Some(id)
        });
        let workspace = tab.content_mut();
        let existing = workspace
            .sessions
            .iter()
            .position(|s| s.group_id == group.group_id);
        if existing.is_some_and(|index| workspace.sessions[index].group_version > group.version) {
            // Stale events in one view must not prevent another view catching up.
            continue;
        }
        let selected = workspace
            .sessions
            .get(workspace.selected_session)
            .map(|s| s.group_id);
        let unavailable = !group.session_ids.is_empty() && terminals.is_empty();
        if group.detached || unavailable {
            workspace.sessions.retain(|s| s.group_id != group.group_id);
        } else if existing.is_some() || (target && !terminals.is_empty()) {
            let mut session = existing
                .map(|index| workspace.sessions.remove(index))
                .unwrap_or_else(|| SessionView {
                    group_id: group.group_id,
                    group_version: 0,
                    pinned: false,
                    automatic_name: false,
                    name: String::new(),
                    actor: "you".to_owned(),
                    status,
                    terminals: Vec::new(),
                });
            if renamed {
                session.automatic_name = false;
            }
            session.group_id = group.group_id;
            session.group_version = group.version;
            if !session.automatic_name {
                session.name.clone_from(&group.name);
            }
            session.pinned = group.pinned;
            session.terminals.clone_from(&terminals);
            session.status = status;
            workspace.sessions.retain(|s| {
                !(s.group_version == 0
                    && (s.terminals.iter().any(|index| terminals.contains(index))
                        || (s.terminals.is_empty() && s.name == "mission history")))
            });
            let position = (group.position as usize).min(workspace.sessions.len());
            workspace.sessions.insert(position, session);
        }
        if workspace.sessions.is_empty() {
            workspace.sessions.push(history_placeholder());
        }
        workspace.selected_session = selected
            .and_then(|id| workspace.sessions.iter().position(|s| s.group_id == id))
            .unwrap_or_else(|| workspace.selected_session.min(workspace.sessions.len() - 1));
    }
    true
}

pub(crate) fn history_placeholder() -> SessionView {
    SessionView {
        group_id: Default::default(),
        group_version: 0,
        pinned: false,
        automatic_name: false,
        name: "mission history".to_owned(),
        actor: String::new(),
        status: SessionStatus::Closed,
        terminals: Vec::new(),
    }
}

/// Remove authoritative group presentation without ending or dismissing PTYs.
/// A deletion is not a local unsaved group (version zero) to recreate on reload.
pub(crate) fn remove_group(
    workspaces: &mut WorkspaceTabs<WorkspaceView>,
    versions: &mut HashMap<SessionGroupId, (u64, String)>,
    group_id: SessionGroupId,
) {
    versions.remove(&group_id);
    for tab in workspaces.tabs_mut() {
        let workspace = tab.content_mut();
        let selected = workspace
            .sessions
            .get(workspace.selected_session)
            .map(|s| s.group_id);
        workspace
            .sessions
            .retain(|session| session.group_id != group_id);
        if workspace.sessions.is_empty() {
            workspace.sessions.push(history_placeholder());
        }
        workspace.selected_session = selected
            .and_then(|id| workspace.sessions.iter().position(|s| s.group_id == id))
            .unwrap_or_else(|| workspace.selected_session.min(workspace.sessions.len() - 1));
    }
}

/// Called only for a completed full-scope snapshot/list, never an item batch.
pub(crate) fn reconcile_groups(
    workspaces: &mut WorkspaceTabs<WorkspaceView>,
    versions: &mut HashMap<SessionGroupId, (u64, String)>,
    live: &HashSet<SessionGroupId>,
) {
    let absent: HashSet<_> = workspaces
        .tabs()
        .iter()
        .flat_map(|tab| &tab.content().sessions)
        .filter(|session| session.group_version > 0 && !live.contains(&session.group_id))
        .map(|session| session.group_id)
        .collect();
    for id in absent {
        remove_group(workspaces, versions, id);
    }
    versions.retain(|id, _| live.contains(id));
}
