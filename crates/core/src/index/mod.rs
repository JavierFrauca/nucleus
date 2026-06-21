//! Vector search.
//!
//! [`VectorIndex`] is the abstraction the rest of the engine talks to; today the
//! only implementation is [`FlatIndex`] (exact, brute-force cosine). An ANN
//! implementation (e.g. HNSW) can be added later behind the same trait without
//! touching callers. One index is kept per domain.

use std::collections::HashSet;
use std::path::Path;

use crate::id::ChunkId;
use crate::Result;

mod flat;
mod hnsw;
mod lexical;

pub use flat::FlatIndex;
pub use hnsw::HnswIndex;
pub use lexical::LexicalIndex;

/// An approximate or exact nearest-neighbour index over chunk embeddings.
///
/// Implementations are expected to treat vectors as cosine-similarity points;
/// `search` returns the highest-similarity chunks first.
pub trait VectorIndex: Send + Sync {
    /// The dimension every vector in this index must have.
    fn dim(&self) -> usize;

    /// Number of vectors currently indexed.
    fn len(&self) -> usize;

    /// Whether the index holds no vectors.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Insert or replace the vector for `id`.
    ///
    /// Returns [`crate::NucleusError::DimensionMismatch`] if `vector.len() != dim()`.
    fn upsert(&mut self, id: ChunkId, vector: &[f32]) -> Result<()>;

    /// Remove `id` from the index if present (no-op otherwise).
    fn remove(&mut self, id: ChunkId);

    /// Return up to `k` `(chunk, score)` pairs ordered by descending cosine
    /// similarity. When `allowed` is `Some`, only those chunk ids are considered
    /// (used to apply tag/document pre-filters).
    fn search(
        &self,
        query: &[f32],
        k: usize,
        allowed: Option<&HashSet<ChunkId>>,
    ) -> Vec<(ChunkId, f32)>;

    /// Persist the index under `dir`/`name`. Returns `true` if the backend
    /// supports persistence (and did so). The default is a no-op returning
    /// `false` — exact indexes are simply rebuilt from storage on startup.
    fn persist(&self, dir: &Path, name: &str) -> Result<bool> {
        let _ = (dir, name);
        Ok(false)
    }
}

/// Which index backend to use for a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndexKind {
    /// Exact brute-force cosine — small/medium domains, exact filters.
    #[default]
    Flat,
    /// Approximate HNSW — large domains (see [`HnswIndex`] caveats).
    Hnsw,
}

impl IndexKind {
    /// Parse from a config string (`flat` / `hnsw`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "flat" => Some(Self::Flat),
            "hnsw" => Some(Self::Hnsw),
            _ => None,
        }
    }
}

/// Construct an empty index of the requested kind for vectors of `dim`.
pub fn build_index(kind: IndexKind, dim: usize) -> Box<dyn VectorIndex> {
    match kind {
        IndexKind::Flat => Box::new(FlatIndex::new(dim)),
        IndexKind::Hnsw => Box::new(HnswIndex::new(dim)),
    }
}
