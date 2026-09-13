//! Negotiated collection snapshot delimiters. Items retain their existing
//! EventBatch representation; delimiters use their own frame kind.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const FEATURE: &str = "collection_snapshots_v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotMarker {
    SnapshotBegin { generation: Uuid },
    SnapshotEnd { generation: Uuid },
}
