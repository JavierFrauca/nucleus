use serde::{Deserialize, Serialize};

use crate::id::{DomainId, TagId};

/// A tag in a domain's hierarchical taxonomy. Tags are scoped per domain and may
/// nest via `parent`, enabling faceted, drill-down filtering of chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    pub id: TagId,
    pub domain_id: DomainId,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub parent: Option<TagId>,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
}
