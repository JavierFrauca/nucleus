use serde::{Deserialize, Serialize};

use crate::id::DomainId;

/// A domain is a named namespace that segments the knowledge base. Each domain
/// pins one embedding model (and therefore one vector dimension), owns its own
/// vector index, and has its own tag vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain {
    pub id: DomainId,
    pub name: String,
    /// Embedding model id pinned for this domain (e.g. `multilingual-e5-small`).
    pub model: String,
    /// Vector dimension produced by `model`.
    pub dim: usize,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
}
