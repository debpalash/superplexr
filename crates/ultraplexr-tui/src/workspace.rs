//! Two-pane presentation only. Changing focus returns Control before any new
//! target becomes active; attaching never launches or duplicates a process.
use crate::{Error, FeedStatus, FocusedSession, render::Rect};
use std::sync::Arc;
use ultraplexr_client::ControlClient;
use ultraplexr_core::SessionId;
use ultraplexr_terminal::{FullFrame, GridSize, MAX_COLUMNS, MAX_ROWS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}
#[derive(Clone, Copy, Debug)]
pub enum Open {
    Replace,
    Split(Axis),
    /// Another independent Surface of an existing Session, not a new process
    /// or Session group. Ordinary opening still focuses an existing pane.
    Duplicate(Axis),
}

pub struct Pane {
    pub continuity: u64,
    pub id: SessionId,
    pub session: FocusedSession,
    pub latest: Option<Arc<FullFrame>>,
    pub shown: Option<Arc<FullFrame>>,
    pub painted: Option<Arc<FullFrame>>,
    pub paused: bool,
    pub offset: u32,
    pub status: String,
    pub feed: FeedStatus,
    pub initial_claim: bool,
    pub pending_paste: Option<String>,
}
impl Pane {
    pub(crate) fn attach(client: &ControlClient, id: SessionId) -> Result<Self, Error> {
        Ok(Self {
            continuity: 0,
            id,
            session: FocusedSession::attach(client, id)?,
            latest: None,
            shown: None,
            painted: None,
            paused: false,
            offset: 0,
            status: "Connecting…".into(),
            feed: FeedStatus::Reconnecting,
            initial_claim: false,
            pending_paste: None,
        })
    }
    pub fn live(&mut self) {
        self.paused = false;
        self.offset = 0;
        self.shown = self.latest.clone();
        self.status = if self.feed == FeedStatus::Ended {
            "Session ended; retained output"
        } else {
            "Live output"
        }
        .into();
    }
}

#[derive(Default)]
pub struct Workspace {
    pub panes: Vec<Pane>,
    pub active: usize,
    pub axis: Option<Axis>,
}
impl Workspace {
    /// Local-only admission of a worker-prepared observing pane. Return the old
    /// or unused pane to its worker for cancellation/destruction off the UI.
    #[allow(clippy::result_large_err, reason = "callers match on the pane")]
    pub(crate) fn install_prepared(
        &mut self,
        pane: Pane,
        mode: Open,
    ) -> Result<Option<Pane>, (Error, Pane)> {
        if self
            .panes
            .get(self.active)
            .is_some_and(|current| current.session.release_pending())
        {
            return Err((Error::ReleasePending, pane));
        }
        if !matches!(mode, Open::Duplicate(_))
            && let Some(index) = self.panes.iter().position(|current| current.id == pane.id)
        {
            self.active = index;
            self.invalidate();
            return Ok(Some(pane));
        }
        if matches!(mode, Open::Split(_) | Open::Duplicate(_)) && self.panes.len() >= 2 {
            return Err((Error::PaneLimit, pane));
        }
        let retired = match mode {
            Open::Split(axis) | Open::Duplicate(axis) if !self.panes.is_empty() => {
                self.panes.push(pane);
                self.active = 1;
                self.axis = Some(axis);
                None
            }
            _ if self.panes.is_empty() => {
                self.panes.push(pane);
                self.active = 0;
                None
            }
            _ => Some(std::mem::replace(&mut self.panes[self.active], pane)),
        };
        self.invalidate();
        Ok(retired)
    }

    pub fn release_active(&mut self) -> Result<(), Error> {
        if let Some(pane) = self.panes.get_mut(self.active) {
            pane.pending_paste = None;
            pane.initial_claim = false;
            pane.session.release_control()?;
        }
        Ok(())
    }
    pub fn open(&mut self, client: &ControlClient, id: SessionId, mode: Open) -> Result<(), Error> {
        if !matches!(mode, Open::Duplicate(_))
            && let Some(index) = self.panes.iter().position(|pane| pane.id == id)
        {
            self.release_active()?;
            self.active = index;
            self.invalidate();
            return Ok(());
        }
        if matches!(mode, Open::Split(_) | Open::Duplicate(_)) && self.panes.len() >= 2 {
            return Err(Error::PaneLimit);
        }
        let pane = Pane::attach(client, id)?;
        self.release_active()?;
        match mode {
            Open::Split(axis) | Open::Duplicate(axis) if !self.panes.is_empty() => {
                self.panes.push(pane);
                self.active = 1;
                self.axis = Some(axis);
            }
            _ if self.panes.is_empty() => {
                self.panes.push(pane);
                self.active = 0;
            }
            _ => self.panes[self.active] = pane,
        }
        self.invalidate();
        Ok(())
    }
    pub fn next_pane(&mut self) -> Result<(), Error> {
        self.release_active()?;
        if !self.panes.is_empty() {
            self.active = (self.active + 1) % self.panes.len();
        }
        self.invalidate();
        Ok(())
    }
    pub fn close_active(&mut self) -> Result<(), Error> {
        self.release_active()?;
        if !self.panes.is_empty() {
            self.panes.remove(self.active);
        }
        self.active = 0;
        self.axis = None;
        self.invalidate();
        Ok(())
    }
    pub fn invalidate(&mut self) {
        for pane in &mut self.panes {
            pane.painted = None;
        }
    }
    pub fn layout(&self, size: (u16, u16)) -> Vec<(usize, Rect)> {
        let full = Rect {
            x: 0,
            y: 0,
            width: size.0,
            height: size.1,
        };
        if self.panes.is_empty() {
            return vec![];
        }
        if self.panes.len() == 1 {
            return vec![(0, full)];
        }
        split_rects(self.axis, size, self.active)
    }
    pub fn active_grid(&self, size: (u16, u16)) -> GridSize {
        let rect = self
            .layout(size)
            .into_iter()
            .find(|(index, _)| *index == self.active)
            .map(|(_, rect)| rect)
            .unwrap_or(Rect {
                x: 0,
                y: 0,
                width: size.0,
                height: size.1,
            });
        GridSize {
            columns: rect.width.clamp(2, MAX_COLUMNS),
            rows: rect.height.saturating_sub(1).clamp(1, MAX_ROWS),
        }
    }
}

fn split_rects(axis: Option<Axis>, size: (u16, u16), active: usize) -> Vec<(usize, Rect)> {
    let full = Rect {
        x: 0,
        y: 0,
        width: size.0,
        height: size.1,
    };
    match axis {
        Some(Axis::Vertical) if size.0 >= 41 && size.1 >= 3 => {
            let left = (size.0 - 1) / 2;
            vec![
                (
                    0,
                    Rect {
                        width: left,
                        ..full
                    },
                ),
                (
                    1,
                    Rect {
                        x: left + 1,
                        width: size.0 - left - 1,
                        ..full
                    },
                ),
            ]
        }
        Some(Axis::Horizontal) if size.1 >= 7 => {
            let top = (size.1 - 1) / 2;
            vec![
                (
                    0,
                    Rect {
                        height: top,
                        ..full
                    },
                ),
                (
                    1,
                    Rect {
                        y: top + 1,
                        height: size.1 - top - 1,
                        ..full
                    },
                ),
            ]
        }
        _ => vec![(active, full)],
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
