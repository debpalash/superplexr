//! Focused literal history search. Results are physical rows/scalar columns,
//! not persistent bookmarks. All blocking operations belong to the worker.
pub use ultraplexr_client::history_worker::{Update, Worker};
use ultraplexr_core::SessionId;
use ultraplexr_terminal::SearchMatch;

pub const MAX_MATCHES: usize = 1000;
const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_QUERY_BYTES: usize = 1024;

pub struct Panel {
    pub session_id: SessionId,
    pub query: String,
    pub editing: bool,
    pub matches: Vec<SearchMatch>,
    pub selected: usize,
    pub note: String,
    pub loading_history: bool,
    preview_bytes: usize,
}
impl Panel {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            query: String::new(),
            editing: true,
            matches: Vec::new(),
            selected: 0,
            note: "Type a literal query, then Enter to search (case-insensitive)".into(),
            loading_history: false,
            preview_bytes: 0,
        }
    }
    pub fn reset(&mut self) {
        self.matches.clear();
        self.preview_bytes = 0;
        self.selected = 0;
        self.loading_history = false;
    }
    /// Returns false at the local retention bound; caller cancels further work.
    pub fn page(&mut self, page: ultraplexr_protocol::search_stream::SearchPage) -> bool {
        for found in page.matches {
            if self.matches.len() == MAX_MATCHES
                || self.preview_bytes + found.preview.len() > MAX_PREVIEW_BYTES
            {
                self.note = format!(
                    "{} results; display limit reached. Narrow the query.",
                    self.matches.len()
                );
                return false;
            }
            self.preview_bytes += found.preview.len();
            self.matches.push(found);
        }
        self.note = if page.complete {
            if self.matches.len() == MAX_MATCHES {
                "1000 results; match limit reached. Narrow the query.".into()
            } else {
                format!("Search complete: {} results", self.matches.len())
            }
        } else {
            format!("Searching: {} results so far", self.matches.len())
        };
        true
    }
    pub fn step(&mut self, forward: bool) {
        if forward {
            self.selected = (self.selected + 1).min(self.matches.len().saturating_sub(1));
        } else {
            self.selected = self.selected.saturating_sub(1);
        }
    }
    pub fn append(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.query.len() + c.len_utf8() > MAX_QUERY_BYTES {
                break;
            }
            self.query.push(c);
        }
    }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
