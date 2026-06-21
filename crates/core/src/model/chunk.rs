use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::{ChunkId, DocumentId, DomainId, SubdomainId, TagId};

/// A retrievable unit of text with an embedding. Chunks are chained (`prev`/`next`)
/// so neighbours can be fetched for context, mirroring the knowledge-vault model.
/// The embedding vector itself is stored separately (keyed by [`ChunkId`]) so the
/// index can be loaded without dragging the text along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub id: ChunkId,
    pub document_id: DocumentId,
    pub domain_id: DomainId,
    /// Optional subdomain, inherited from the document.
    pub subdomain_id: Option<SubdomainId>,
    /// Position of this chunk within its document (0-based).
    pub ordinal: u32,
    pub text: String,
    pub tags: Vec<TagId>,
    pub metadata: BTreeMap<String, String>,
    pub prev: Option<ChunkId>,
    pub next: Option<ChunkId>,
}
