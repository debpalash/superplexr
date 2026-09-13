//! Optional acknowledged search streams. Coordinates retain the existing
//! physical-row/Unicode-scalar convention; these are not snapshot epochs.
use serde::{Deserialize, Serialize};
use superplexr_core::SessionId;
use superplexr_terminal::SearchMatch;
use uuid::Uuid;

pub const FEATURE: &str = "acknowledged_search_v1";
pub const MAX_PAGE_BYTES: usize = 512 * 1024;
pub const MAX_PAGE_MATCHES: usize = 64;
pub const MAX_CONNECTION_SEARCHES: usize = 4;
pub const MAX_PROCESS_SEARCHES: usize = 8;

/// One JSON payload in a SearchPage wire frame. Only `complete` establishes
/// successful EOF; `error` terminates an unsuccessful stream. A nonterminal
/// page must be acknowledged before another is sent. No implicit retries.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchPage {
    pub search_id: Uuid,
    pub session_id: SessionId,
    pub sequence: u64,
    pub matches: Vec<SearchMatch>,
    pub complete: bool,
    pub error: Option<String>,
}
