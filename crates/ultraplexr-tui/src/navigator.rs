//! Read-only navigation over authorized runtime state. No local work database,
//! inferred Attention, approval actions, or owner fallback for scoped Shares.
use ultraplexr_client::{ClientError, ControlClient};
use ultraplexr_core::{MissionId, SessionId};
use ultraplexr_protocol::{SessionGroupId, SessionGroupSummary};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Sessions,
    Missions,
    Attention,
    Groups,
}
impl Tab {
    pub fn next(self) -> Self {
        match self {
            Self::Sessions => Self::Missions,
            Self::Missions => Self::Attention,
            Self::Attention => Self::Groups,
            Self::Groups => Self::Sessions,
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Sessions => "Sessions",
            Self::Missions => "Missions",
            Self::Attention => "Attention",
            Self::Groups => "Session groups",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Session(SessionId),
    Mission(MissionId),
    Group(SessionGroupSummary),
    Verifier(crate::workflow::Inspection),
    VerifierPage(crate::workflow::CatalogPage),
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub priority: u8,
    pub key: String,
    pub label: String,
    pub detail: String,
    pub target: Target,
}

#[derive(Default)]
pub struct Navigator {
    pub tab: Tab,
    pub mission: Option<MissionId>,
    pub group: Option<SessionGroupId>,
    pub query: String,
    pub editing: bool,
    pub selected: usize,
    pub entries: Vec<Entry>,
    pub note: String,
    pub action_note: String,
    pub group_edit: Option<GroupEdit>,
    pub workflow_read: bool,
    pub inspection: Option<crate::workflow::Inspection>,
    pub catalog: Option<crate::workflow::CatalogPage>,
    pub verifier_edit: Option<crate::workflow::Draft>,
}

#[derive(Clone, Copy)]
pub enum GroupField {
    Name,
    Position,
}
impl GroupField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Position => "Order",
        }
    }
    pub fn help(self) -> &'static str {
        match self {
            Self::Name => "Rename | Enter save | Esc cancel | Ctrl-U clear | 128 UTF-8 bytes",
            Self::Position => {
                "Order 0..4294967295 | lower sorts first | Enter save | Esc cancel | Ctrl-U clear"
            }
        }
    }
}

pub struct GroupEdit {
    pub group: SessionGroupSummary,
    pub value: String,
    pub field: GroupField,
    overflowed: bool,
}
impl GroupEdit {
    pub fn new(group: SessionGroupSummary, field: GroupField) -> Self {
        let value = match field {
            GroupField::Name => group.name.clone(),
            GroupField::Position => group.position.to_string(),
        };
        Self {
            group,
            value,
            field,
            overflowed: false,
        }
    }
    pub fn append(&mut self, text: &str) {
        let limit = match self.field {
            GroupField::Name => 128,
            GroupField::Position => 10,
        };
        for character in text.chars().filter(|character| !character.is_control()) {
            if self.value.len() + character.len_utf8() > limit {
                self.overflowed = true;
                break;
            }
            self.value.push(character);
        }
    }
    pub fn backspace(&mut self) {
        self.value.pop();
        self.overflowed = false;
    }
    pub fn clear(&mut self) {
        self.value.clear();
        self.overflowed = false;
    }
    pub fn action(&self) -> Result<crate::group_actions::Action, &'static str> {
        if self.overflowed {
            return Err("Input exceeded the field limit; edit or clear it before saving");
        }
        match self.field {
            GroupField::Name if self.value.trim().is_empty() => Err("Group name must not be blank"),
            GroupField::Name => Ok(crate::group_actions::Action::Rename(
                self.group.clone(),
                self.value.trim().to_owned(),
            )),
            GroupField::Position => {
                if self.value.is_empty() || !self.value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err("Order must contain decimal digits only");
                }
                let position = self
                    .value
                    .parse::<u32>()
                    .map_err(|_| "Order must be between 0 and 4294967295")?;
                Ok(crate::group_actions::Action::Position(
                    self.group.clone(),
                    position,
                ))
            }
        }
    }
}

impl Navigator {
    pub fn refresh(&mut self, client: &ControlClient) -> Result<(), ClientError> {
        let selected = self.current().map(|entry| entry.key.clone());
        // A failed or revoked read must not retain previously authorized rows.
        self.entries.clear();
        self.selected = 0;
        let entries = if let Some(inspection) = self.inspection {
            crate::workflow::load(client, inspection, &|| false)?
        } else if let Some(page) = self.catalog {
            crate::workflow::load_catalog(client, page, &|| false)?
        } else {
            load(client, self.tab, self.mission, self.group, &|| false)?
        }
        .unwrap_or_default();
        self.replace(entries, selected);
        Ok(())
    }
    pub fn apply(&mut self, entries: Vec<Entry>) -> bool {
        let changed = self.entries != entries;
        let previous_note = std::mem::take(&mut self.note);
        let selected = self.current().map(|entry| entry.key.clone());
        self.replace(entries, selected);
        changed || previous_note != self.note
    }
    fn replace(&mut self, entries: Vec<Entry>, selected: Option<String>) {
        self.entries = entries;
        self.selected = selected
            .and_then(|key| self.visible().iter().position(|entry| entry.key == key))
            .unwrap_or(0);
        self.note = if self.inspection.is_some() {
            "Recorded snapshot only; r refreshes. Evidence not rechecked; no launch, collection or acceptance."
        } else if self.catalog.is_some() {
            "Live page only; r refreshes, n next, 0 first, W UUID. Evidence not rechecked; no automatic refresh."
        } else if self.group.is_some() && self.entries.is_empty() {
            "Group has no available Sessions or was removed; Backspace returns to groups."
        } else {
            "Refreshes while open; r refresh. Attention opens without approval."
        }.into();
    }
    pub fn visible(&self) -> Vec<&Entry> {
        let query = self.query.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.label.to_lowercase().contains(&query)
                    || entry.detail.to_lowercase().contains(&query)
            })
            .collect()
    }
    pub fn workflow_active(&self) -> bool {
        self.inspection.is_some() || self.catalog.is_some()
    }

    pub fn open_catalog(&mut self, page: crate::workflow::CatalogPage) {
        self.entries.clear();
        self.catalog = Some(page);
        self.inspection = None;
        self.mission = Some(page.mission);
        self.group = None;
        self.tab = Tab::Sessions;
        self.query.clear();
        self.selected = 0;
    }

    pub fn open_verifier(&mut self, inspection: crate::workflow::Inspection) {
        self.entries.clear();
        self.inspection = Some(inspection);
        self.mission = Some(inspection.mission);
        self.group = None;
        self.tab = Tab::Sessions;
        self.query.clear();
        self.selected = 0;
    }

    pub fn next_verifier_page(&self) -> Option<crate::workflow::CatalogPage> {
        self.entries.iter().find_map(|entry| match entry.target {
            Target::VerifierPage(page) => Some(page),
            _ => None,
        })
    }
    pub fn current(&self) -> Option<&Entry> {
        self.visible().get(self.selected).copied()
    }
    pub fn step(&mut self, forward: bool) {
        let count = self.visible().len();
        self.selected = if count == 0 {
            0
        } else if forward {
            (self.selected + 1) % count
        } else {
            (self.selected + count - 1) % count
        };
    }
}

pub(super) fn load(
    client: &ControlClient,
    tab: Tab,
    mission_id: Option<MissionId>,
    group_id: Option<SessionGroupId>,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Vec<Entry>>, ClientError> {
    if cancelled() {
        return Ok(None);
    }
    let terminals = if tab == Tab::Missions {
        Vec::new()
    } else {
        client.list_terminals()?
    };
    if cancelled() {
        return Ok(None);
    }
    let missions = client.list_missions()?;
    if cancelled() {
        return Ok(None);
    }
    let mut entries = Vec::new();
    match tab {
        Tab::Groups => {
            if client.is_shared() {
                return Ok(Some(vec![Entry {
                    priority: 0,
                    key: "groups-unavailable".into(),
                    label: "Session groups require an owner connection".into(),
                    detail: "Shared Sessions remain available in the Sessions view".into(),
                    target: Target::Unavailable,
                }]));
            }
            let mut groups = client.list_session_groups(mission_id)?;
            groups.sort_by_key(|group| (!group.pinned, group.position, group.group_id));
            let available_sessions: std::collections::HashSet<_> = terminals
                .iter()
                .map(|terminal| terminal.session_id)
                .collect();
            for group in groups {
                if cancelled() {
                    return Ok(None);
                }
                let available = group
                    .session_ids
                    .iter()
                    .filter(|id| available_sessions.contains(id))
                    .count();
                let project = group
                    .mission_id
                    .and_then(|id| missions.iter().find(|mission| mission.id == id))
                    .map(|mission| mission.intent.as_str())
                    .unwrap_or("No Mission");
                entries.push(Entry {
                    priority: u8::from(!group.pinned),
                    key: group.group_id.to_string(),
                    label: format!("{}{}", if group.pinned { "* " } else { "" }, group.name),
                    detail: format!(
                        "{project} | order {} | {available}/{} available Sessions{} | {}",
                        group.position,
                        group.session_ids.len(),
                        if group.detached { " | detached" } else { "" },
                        group.group_id
                    ),
                    target: Target::Group(group),
                });
            }
            return Ok(Some(entries));
        }
        Tab::Missions => {
            for mission in missions {
                entries.push(Entry {
                    priority: 0,
                    key: mission.id.to_string(),
                    label: mission.intent,
                    detail: format!(
                        "{:?} | {} Sessions | {} Attention",
                        mission.status, mission.session_count, mission.attention_count
                    ),
                    target: Target::Mission(mission.id),
                });
            }
        }
        Tab::Sessions => {
            let mission = if group_id.is_some() {
                None
            } else {
                mission_id.map(|id| client.get_mission(id)).transpose()?
            };
            if cancelled() {
                return Ok(None);
            }
            let groups = if client.is_shared() {
                vec![]
            } else {
                client.list_session_groups(if group_id.is_some() { None } else { mission_id })?
            };
            let selected_group =
                group_id.and_then(|id| groups.iter().find(|group| group.group_id == id));
            for terminal in terminals {
                if cancelled() {
                    return Ok(None);
                }
                // A removed group is an empty view, never a fallback to all
                // Sessions. Membership stays authoritative even across Missions.
                if group_id.is_some()
                    && selected_group
                        .is_none_or(|group| !group.session_ids.contains(&terminal.session_id))
                {
                    continue;
                }
                if mission.as_ref().is_some_and(|mission| {
                    !mission.sessions.contains_key(&terminal.session_id)
                        && terminal.mission_id != Some(mission.id)
                }) {
                    continue;
                }
                let group = selected_group.or_else(|| {
                    groups
                        .iter()
                        .find(|group| group.session_ids.contains(&terminal.session_id))
                });
                let name = group
                    .filter(|_| group_id.is_none())
                    .map(|group| group.name.clone())
                    .or_else(|| {
                        mission
                            .as_ref()
                            .and_then(|mission| mission.sessions.get(&terminal.session_id))
                            .map(|session| session.name.clone())
                    })
                    .or_else(|| terminal.display_title.clone())
                    .or_else(|| {
                        terminal
                            .display_directory
                            .as_deref()
                            .and_then(|directory| {
                                directory
                                    .trim_end_matches(['/', '\\'])
                                    .rsplit(['/', '\\'])
                                    .find(|part| !part.is_empty())
                            })
                            .map(str::to_owned)
                    })
                    .or_else(|| {
                        terminal
                            .foreground_process
                            .as_ref()
                            .map(|process| process.executable.clone())
                    })
                    .unwrap_or_else(|| "Terminal".into());
                let project = terminal
                    .mission_id
                    .and_then(|id| missions.iter().find(|mission| mission.id == id))
                    .map(|mission| mission.intent.as_str())
                    .unwrap_or("Ungrouped");
                entries.push(Entry {
                    priority: if group.is_some_and(|group| group.pinned) {
                        0
                    } else {
                        1
                    },
                    key: terminal.session_id.to_string(),
                    label: format!(
                        "{}{}",
                        if group.is_some_and(|group| group.pinned) {
                            "* "
                        } else {
                            ""
                        },
                        name
                    ),
                    detail: format!(
                        "{project} | {:?} | {}",
                        terminal.status, terminal.session_id
                    ),
                    target: Target::Session(terminal.session_id),
                });
            }
        }
        Tab::Attention => {
            for summary in missions
                .into_iter()
                .filter(|mission| mission.attention_count > 0)
            {
                if cancelled() {
                    return Ok(None);
                }
                let mission = client.get_mission(summary.id)?;
                for item in mission.attention_queue() {
                    let session = mission
                        .runs
                        .get(&item.run_id)
                        .and_then(|run| run.primary_session)
                        .filter(|id| terminals.iter().any(|terminal| terminal.session_id == *id));
                    entries.push(Entry {
                        priority: item.kind as u8,
                        key: item.signal_id.to_string(),
                        label: format!("{:?}: {}", item.kind, item.summary),
                        detail: format!(
                            "{} | Run {} | {}",
                            mission.intent,
                            item.run_id,
                            if session.is_some() {
                                "Enter to observe"
                            } else {
                                "No accessible primary Session"
                            }
                        ),
                        target: session.map(Target::Session).unwrap_or(Target::Unavailable),
                    });
                }
            }
        }
    }
    entries.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(a.label.cmp(&b.label))
            .then(a.key.cmp(&b.key))
    });
    Ok(Some(entries))
}
