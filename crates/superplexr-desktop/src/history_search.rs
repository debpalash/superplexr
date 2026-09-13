//! Workspace search over the shared frontend worker. Stable identities travel
//! with hits; presentation indices are resolved again before navigation.
use crate::workspace_tabs::WorkspaceId;
use std::{collections::VecDeque, io, sync::Arc};
use superplexr_client::{
    DaemonSession,
    history_worker::{Update, Worker},
};
use superplexr_core::SessionId;
use superplexr_protocol::SessionGroupId;
use superplexr_terminal::{FullFrame, SearchMatch};

const MAX_HITS: usize = 24;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Target {
    pub group_id: SessionGroupId,
    pub surface_index: usize,
    pub session: DaemonSession,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hit {
    pub group_id: SessionGroupId,
    pub surface_index: usize,
    pub session_id: SessionId,
    pub found: SearchMatch,
}

pub(crate) struct Search {
    worker: Worker,
    pub workspace: WorkspaceId,
    query: String,
    targets: VecDeque<Target>,
    active: Option<Target>,
    pub hits: Vec<Hit>,
    pub note: String,
    pub running: bool,
    bytes: usize,
    reveal: Option<Hit>,
    pub preview: Option<(Hit, Arc<FullFrame>)>,
}
impl Search {
    pub fn new(workspace: WorkspaceId) -> io::Result<Self> {
        Ok(Self {
            worker: Worker::new()?,
            workspace,
            query: String::new(),
            targets: VecDeque::new(),
            active: None,
            hits: Vec::new(),
            note: String::new(),
            running: false,
            bytes: 0,
            reveal: None,
            preview: None,
        })
    }
    pub fn cancel(&mut self, clear: bool) {
        self.worker.cancel();
        self.targets.clear();
        self.active = None;
        self.running = false;
        self.reveal = None;
        self.preview = None;
        if clear {
            self.hits.clear();
            self.bytes = 0;
        }
        self.note = "Search cancelled".into();
    }
    pub fn start(&mut self, workspace: WorkspaceId, query: String, targets: Vec<Target>) {
        self.cancel(true);
        self.workspace = workspace;
        self.query = query;
        if self.query.len() > 1024 {
            self.note = "Query exceeds 1024 bytes; shorten it".into();
            return;
        }
        self.targets = targets.into();
        self.next();
    }
    fn next(&mut self) {
        self.active = self.targets.pop_front();
        if let Some(target) = &self.active {
            self.worker
                .search_with_limit(target.session.clone(), self.query.clone(), 8);
            self.running = true;
            self.note = format!("Searching history: {} results", self.hits.len());
        } else {
            self.running = false;
            self.note = format!(
                "History search complete: {} results (up to 8 per terminal)",
                self.hits.len()
            );
        }
    }
    /// Opens a read-only viewport; never scrolls a shared live terminal.
    pub fn reveal(&mut self, hit: Hit, session: DaemonSession) {
        self.cancel(false);
        self.worker.history(session, hit.found.clone());
        self.reveal = Some(hit);
        self.running = true;
        self.note = "Loading history; Cancel stops waiting".into();
    }
    pub fn poll(&mut self) -> bool {
        let Some(update) = self.worker.take_update() else {
            return false;
        };
        match update {
            Update::Page(page) => {
                let Some(target) = &self.active else {
                    return false;
                };
                if page.session_id != target.session.id() {
                    self.cancel(true);
                    self.note = "Search identity changed; enter a new query".into();
                    return true;
                }
                for found in page.matches {
                    if self.hits.len() == MAX_HITS || self.bytes + found.preview.len() > MAX_BYTES {
                        self.cancel(false);
                        self.note = "History display limit reached; narrow the query".into();
                        return true;
                    }
                    self.bytes += found.preview.len();
                    self.hits.push(Hit {
                        group_id: target.group_id,
                        surface_index: target.surface_index,
                        session_id: target.session.id(),
                        found,
                    });
                }
                if self.hits.len() == MAX_HITS {
                    self.cancel(false);
                    self.note = "24 results; display limit reached. Narrow the query.".into();
                } else if page.complete {
                    self.next();
                } else {
                    self.note = format!("Searching history: {} results", self.hits.len());
                }
            }
            Update::Error(error) => {
                self.cancel(true);
                self.note = format!("History search failed: {error}");
            }
            Update::History(frame) => {
                self.running = false;
                if let Some(hit) = self.reveal.take() {
                    self.preview = Some((hit, frame));
                }
                self.note = "History opened locally; row positions may change".into();
            }
        }
        true
    }
}
