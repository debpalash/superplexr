use std::{error::Error, fmt};

const RECENTLY_CLOSED_LIMIT: usize = 20;
pub(crate) const PINNED_TAB_WIDTH: f32 = 44.0;
pub(crate) const MIN_TAB_WIDTH: f32 = 112.0;
pub(crate) const MAX_TAB_WIDTH: f32 = 196.0;
pub(crate) const OVERFLOW_BUTTON_WIDTH: f32 = 42.0;

fn preferred_regular_width<T>(tab: &WorkspaceTab<T>) -> f32 {
    // Padding, status, count, close control, and the gaps between them. Keeping
    // this explicit prevents useful titles from being squeezed by tab chrome.
    const TAB_CHROME_WIDTH: f32 = 84.0;
    const AVERAGE_GLYPH_WIDTH: f32 = 6.5;
    let title_width = tab.title.chars().count().min(32) as f32 * AVERAGE_GLYPH_WIDTH;
    (TAB_CHROME_WIDTH + title_width).clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH)
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct WorkspaceId(u64);

impl WorkspaceId {
    pub(crate) const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum TabError {
    EmptyInitialSet,
    UnknownWorkspace(WorkspaceId),
    LastWorkspace,
}

impl fmt::Display for TabError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInitialSet => formatter.write_str("a tab strip needs one workspace"),
            Self::UnknownWorkspace(id) => write!(formatter, "unknown workspace {}", id.raw()),
            Self::LastWorkspace => formatter.write_str("the last workspace cannot be closed"),
        }
    }
}

impl Error for TabError {}

pub(crate) struct WorkspaceTab<T> {
    id: WorkspaceId,
    title: String,
    pinned: bool,
    content: T,
}

impl<T> WorkspaceTab<T> {
    pub(crate) const fn id(&self) -> WorkspaceId {
        self.id
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) const fn pinned(&self) -> bool {
        self.pinned
    }

    pub(crate) const fn content(&self) -> &T {
        &self.content
    }

    pub(crate) const fn content_mut(&mut self) -> &mut T {
        &mut self.content
    }
}

struct ClosedWorkspace<T> {
    tab: WorkspaceTab<T>,
    index: usize,
}

/// Owns browser-style workspace lifecycle independently from GPUI rendering.
///
/// The tab order is authoritative, IDs remain stable while a tab is open or
/// recently closed, pinned tabs always precede regular tabs, and the active ID
/// always identifies an open tab.
pub(crate) struct WorkspaceTabs<T> {
    tabs: Vec<WorkspaceTab<T>>,
    active: WorkspaceId,
    recently_closed: Vec<ClosedWorkspace<T>>,
    next_id: u64,
}

impl<T> WorkspaceTabs<T> {
    #[cfg(test)]
    pub(crate) fn new(initial: Vec<(String, T)>) -> Result<Self, TabError> {
        if initial.is_empty() {
            return Err(TabError::EmptyInitialSet);
        }

        let tabs: Vec<_> = initial
            .into_iter()
            .enumerate()
            .map(|(index, (title, content))| WorkspaceTab {
                id: WorkspaceId(index as u64 + 1),
                title,
                pinned: false,
                content,
            })
            .collect();
        let active = tabs[0].id;
        let next_id = tabs.len() as u64 + 1;

        Ok(Self {
            tabs,
            active,
            recently_closed: Vec::new(),
            next_id,
        })
    }

    pub(crate) fn restore(
        initial: Vec<(WorkspaceId, String, bool, T)>,
        active: WorkspaceId,
    ) -> Result<Self, TabError> {
        if initial.is_empty() {
            return Err(TabError::EmptyInitialSet);
        }
        let tabs = initial
            .into_iter()
            .map(|(id, title, pinned, content)| WorkspaceTab {
                id,
                title,
                pinned,
                content,
            })
            .collect::<Vec<_>>();
        if !tabs.iter().any(|tab| tab.id == active) {
            return Err(TabError::UnknownWorkspace(active));
        }
        let next_id = tabs
            .iter()
            .map(|tab| tab.id.raw())
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Ok(Self {
            tabs,
            active,
            recently_closed: Vec::new(),
            next_id,
        })
    }

    pub(crate) fn tabs(&self) -> &[WorkspaceTab<T>] {
        &self.tabs
    }

    #[cfg(not(test))]
    pub(crate) fn tabs_mut(&mut self) -> &mut [WorkspaceTab<T>] {
        &mut self.tabs
    }

    pub(crate) fn len(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) const fn active_id(&self) -> WorkspaceId {
        self.active
    }

    pub(crate) fn active(&self) -> &WorkspaceTab<T> {
        self.tab(self.active)
            .expect("active workspace invariant must hold")
    }

    pub(crate) fn active_mut(&mut self) -> &mut WorkspaceTab<T> {
        self.tab_mut(self.active)
            .expect("active workspace invariant must hold")
    }

    pub(crate) fn tab(&self, id: WorkspaceId) -> Option<&WorkspaceTab<T>> {
        self.tabs.iter().find(|tab| tab.id == id)
    }

    pub(crate) fn tab_mut(&mut self, id: WorkspaceId) -> Option<&mut WorkspaceTab<T>> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    pub(crate) fn add(&mut self, title: String, content: T) -> WorkspaceId {
        let id = WorkspaceId(self.next_id);
        self.next_id += 1;
        self.tabs.push(WorkspaceTab {
            id,
            title,
            pinned: false,
            content,
        });
        self.active = id;
        id
    }

    pub(crate) fn add_background(&mut self, title: String, content: T) -> WorkspaceId {
        let active = self.active;
        let id = self.add(title, content);
        self.active = active;
        id
    }

    pub(crate) fn activate(&mut self, id: WorkspaceId) -> Result<(), TabError> {
        if self.tab(id).is_none() {
            return Err(TabError::UnknownWorkspace(id));
        }
        self.active = id;
        Ok(())
    }

    pub(crate) fn cycle(&mut self, offset: isize) -> WorkspaceId {
        let active_index = self
            .tabs
            .iter()
            .position(|tab| tab.id == self.active)
            .expect("active workspace invariant must hold");
        let len = self.tabs.len() as isize;
        let next = (active_index as isize + offset).rem_euclid(len) as usize;
        self.active = self.tabs[next].id;
        self.active
    }

    pub(crate) fn close(&mut self, id: WorkspaceId) -> Result<WorkspaceId, TabError> {
        if self.tabs.len() == 1 {
            return Err(TabError::LastWorkspace);
        }
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or(TabError::UnknownWorkspace(id))?;
        let was_active = self.active == id;
        let tab = self.tabs.remove(index);

        if was_active {
            self.active = self.tabs[index.min(self.tabs.len() - 1)].id;
        }

        self.recently_closed.push(ClosedWorkspace { tab, index });
        if self.recently_closed.len() > RECENTLY_CLOSED_LIMIT {
            self.recently_closed.remove(0);
        }
        Ok(self.active)
    }

    pub(crate) fn can_restore(&self) -> bool {
        !self.recently_closed.is_empty()
    }

    pub(crate) fn restore_closed(&mut self) -> Option<WorkspaceId> {
        let closed = self.recently_closed.pop()?;
        let id = closed.tab.id;
        let index = closed.index.min(self.tabs.len());
        self.tabs.insert(index, closed.tab);
        self.active = id;
        Some(id)
    }

    pub(crate) fn toggle_pin(&mut self, id: WorkspaceId) -> Result<bool, TabError> {
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.id == id)
            .ok_or(TabError::UnknownWorkspace(id))?;
        let mut tab = self.tabs.remove(index);
        tab.pinned = !tab.pinned;
        let pinned = tab.pinned;
        let target = self.tabs.iter().take_while(|tab| tab.pinned).count();
        self.tabs.insert(target, tab);
        Ok(pinned)
    }

    pub(crate) fn move_before(
        &mut self,
        moving: WorkspaceId,
        target: WorkspaceId,
    ) -> Result<bool, TabError> {
        if moving == target {
            return Ok(false);
        }
        let moving_index = self
            .tabs
            .iter()
            .position(|tab| tab.id == moving)
            .ok_or(TabError::UnknownWorkspace(moving))?;
        let target_tab = self.tab(target).ok_or(TabError::UnknownWorkspace(target))?;
        if self.tabs[moving_index].pinned != target_tab.pinned {
            return Ok(false);
        }

        let tab = self.tabs.remove(moving_index);
        let target_index = self
            .tabs
            .iter()
            .position(|tab| tab.id == target)
            .expect("target workspace remains after moving a different tab");
        self.tabs.insert(target_index, tab);
        Ok(true)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TabPlacement {
    pub(crate) id: WorkspaceId,
    pub(crate) width: f32,
}

#[derive(Debug, PartialEq)]
pub(crate) struct TabStripLayout {
    pub(crate) visible: Vec<TabPlacement>,
    pub(crate) hidden: Vec<WorkspaceId>,
}

impl TabStripLayout {
    pub(crate) fn calculate<T>(tabs: &WorkspaceTabs<T>, available_width: f32) -> Self {
        let available_width = available_width.max(MIN_TAB_WIDTH);
        let pinned: Vec<_> = tabs.tabs.iter().filter(|tab| tab.pinned).collect();
        let regular: Vec<_> = tabs.tabs.iter().filter(|tab| !tab.pinned).collect();
        let pinned_width = pinned.len() as f32 * PINNED_TAB_WIDTH;
        let regular_available = (available_width - pinned_width).max(MIN_TAB_WIDTH);
        let preferred_regular_widths = regular
            .iter()
            .map(|tab| preferred_regular_width(tab))
            .collect::<Vec<_>>();
        let preferred_regular_total = preferred_regular_widths.iter().sum::<f32>();

        if regular.is_empty() || preferred_regular_total <= regular_available {
            let visible = tabs
                .tabs
                .iter()
                .map(|tab| TabPlacement {
                    id: tab.id,
                    width: if tab.pinned {
                        PINNED_TAB_WIDTH
                    } else {
                        preferred_regular_width(tab)
                    },
                })
                .collect();
            return Self {
                visible,
                hidden: Vec::new(),
            };
        }

        let minimum_regular_total = regular.len() as f32 * MIN_TAB_WIDTH;
        if minimum_regular_total <= regular_available {
            let flexible_total = preferred_regular_total - minimum_regular_total;
            let available_flexible = regular_available - minimum_regular_total;
            let scale = if flexible_total > 0.0 {
                available_flexible / flexible_total
            } else {
                0.0
            };
            let visible = tabs
                .tabs
                .iter()
                .map(|tab| TabPlacement {
                    id: tab.id,
                    width: if tab.pinned {
                        PINNED_TAB_WIDTH
                    } else {
                        MIN_TAB_WIDTH + (preferred_regular_width(tab) - MIN_TAB_WIDTH) * scale
                    },
                })
                .collect();
            return Self {
                visible,
                hidden: Vec::new(),
            };
        }

        let slot_width = MIN_TAB_WIDTH;
        let regular_slots = ((regular_available - OVERFLOW_BUTTON_WIDTH) / slot_width)
            .floor()
            .max(1.0) as usize;
        let active_regular_index = regular
            .iter()
            .position(|tab| tab.id == tabs.active)
            .unwrap_or(0);
        let mut start = active_regular_index.saturating_sub(regular_slots / 2);
        start = start.min(regular.len().saturating_sub(regular_slots));
        let end = (start + regular_slots).min(regular.len());
        let visible_regular = &regular[start..end];
        let visible_regular_ids: Vec<_> = visible_regular.iter().map(|tab| tab.id).collect();

        let visible = tabs
            .tabs
            .iter()
            .filter(|tab| tab.pinned || visible_regular_ids.contains(&tab.id))
            .map(|tab| TabPlacement {
                id: tab.id,
                width: if tab.pinned {
                    PINNED_TAB_WIDTH
                } else {
                    slot_width
                },
            })
            .collect();
        let hidden = regular
            .into_iter()
            .filter(|tab| !visible_regular_ids.contains(&tab.id))
            .map(|tab| tab.id)
            .collect();

        Self { visible, hidden }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(count: usize) -> WorkspaceTabs<usize> {
        WorkspaceTabs::new(
            (0..count)
                .map(|index| (format!("Workspace {index}"), index))
                .collect(),
        )
        .expect("test tab strip must initialize")
    }

    #[test]
    fn closing_active_prefers_the_tab_to_its_right_and_restore_preserves_identity() {
        let mut tabs = tabs(3);
        let second = tabs.tabs()[1].id();
        let third = tabs.tabs()[2].id();
        tabs.activate(second).expect("second tab exists");

        assert_eq!(tabs.close(second), Ok(third));
        assert_eq!(tabs.restore_closed(), Some(second));
        assert_eq!(tabs.active_id(), second);
        assert_eq!(tabs.tabs()[1].id(), second);
    }

    #[test]
    fn background_close_does_not_change_the_active_workspace() {
        let mut tabs = tabs(3);
        let active = tabs.active_id();
        let background = tabs.tabs()[2].id();

        assert_eq!(tabs.close(background), Ok(active));
        assert_eq!(tabs.active_id(), active);
    }

    #[test]
    fn externally_discovered_workspace_is_added_without_stealing_focus() {
        let mut tabs = tabs(2);
        let active = tabs.active_id();

        let discovered = tabs.add_background("Discovered".to_owned(), 9);

        assert_eq!(tabs.active_id(), active);
        assert_eq!(tabs.tab(discovered).map(WorkspaceTab::content), Some(&9));
    }

    #[test]
    fn pinning_and_reordering_preserve_tab_groups() {
        let mut tabs = tabs(3);
        let first = tabs.tabs()[0].id();
        let second = tabs.tabs()[1].id();
        let third = tabs.tabs()[2].id();

        assert_eq!(tabs.toggle_pin(second), Ok(true));
        assert_eq!(tabs.tabs()[0].id(), second);
        assert_eq!(tabs.move_before(third, first), Ok(true));
        assert_eq!(tabs.move_before(first, second), Ok(false));
        assert_eq!(
            tabs.tabs().iter().map(WorkspaceTab::id).collect::<Vec<_>>(),
            vec![second, third, first]
        );
    }

    #[test]
    fn persisted_ids_pins_and_active_workspace_restore_exactly() {
        let first = WorkspaceId::from_raw(41);
        let second = WorkspaceId::from_raw(88);
        let tabs = WorkspaceTabs::restore(
            vec![
                (first, "Pinned".to_owned(), true, 1),
                (second, "Active".to_owned(), false, 2),
            ],
            second,
        )
        .expect("persisted tab state should restore");

        assert_eq!(tabs.active_id(), second);
        assert_eq!(tabs.tabs()[0].id(), first);
        assert!(tabs.tabs()[0].pinned());
        assert_eq!(tabs.tabs()[1].content(), &2);
    }

    #[test]
    fn compact_layout_keeps_the_active_tab_visible_and_reports_overflow() {
        let mut tabs = tabs(12);
        let active = tabs.tabs()[9].id();
        tabs.activate(active).expect("active tab exists");

        let layout = TabStripLayout::calculate(&tabs, 420.0);

        assert!(
            layout
                .visible
                .iter()
                .any(|placement| placement.id == active)
        );
        assert!(!layout.hidden.is_empty());
        assert_eq!(layout.visible.len() + layout.hidden.len(), tabs.len());
    }

    #[test]
    fn roomy_layout_sizes_regular_tabs_by_title_instead_of_filling_the_strip() {
        let tabs = WorkspaceTabs::new(vec![
            ("Run".to_owned(), 1),
            ("Terminal performance investigation".to_owned(), 2),
        ])
        .expect("test tab strip must initialize");

        let layout = TabStripLayout::calculate(&tabs, 640.0);

        assert!(layout.hidden.is_empty());
        assert!(layout.visible[0].width < layout.visible[1].width);
        assert!(layout.visible.iter().map(|tab| tab.width).sum::<f32>() < 640.0);
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        let mut tabs = tabs(3);
        let last = tabs.tabs()[2].id();

        assert_eq!(tabs.cycle(-1), last);
        assert_eq!(tabs.cycle(1), tabs.tabs()[0].id());
    }
}
