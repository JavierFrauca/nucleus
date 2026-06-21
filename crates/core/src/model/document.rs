use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::{DocumentId, DomainId, SubdomainId, TagId};

/// A document belongs to a domain and is split into ordered [`Chunk`](super::Chunk)s
/// at ingestion time. Tags applied here are inherited by its chunks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    pub domain_id: DomainId,
    /// Optional subdomain (topic) within the domain.
    pub subdomain_id: Option<SubdomainId>,
    pub title: String,
    /// Optional origin (path, URL, ...).
    pub source: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub tags: Vec<TagId>,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
}
