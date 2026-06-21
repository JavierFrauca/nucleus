//! Embedding generation.
//!
//! Generating embeddings **inside the engine** is Nucleus's differentiator: there
//! is no external embedding service to deploy. [`LocalEmbedder`] runs the models
//! in-process via fastembed (ONNX Runtime). [`MockEmbedder`] is a deterministic,
//! dependency-free stand-in for tests.
//!
//! The trait is intentionally **synchronous**: inference is CPU-bound, so callers
//! (the engine's ingest path and search) run it on `tokio::task::spawn_blocking`
//! rather than forcing async all the way down.

mod local;
mod mock;

pub use local::LocalEmbedder;
pub use mock::MockEmbedder;

use crate::Result;

/// Default embedding model — multilingual so Spanish and English both work well.
pub const DEFAULT_MODEL: &str = "multilingual-e5-small";

/// Produces vector embeddings for text.
pub trait Embedder: Send + Sync {
    /// The output dimension for `model`, or `None` if the model is unknown.
    fn dim(&self, model: &str) -> Option<usize>;

    /// Embed a batch of documents (passages) for indexing.
    fn embed_documents(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Embed a single search query.
    fn embed_query(&self, model: &str, text: &str) -> Result<Vec<f32>>;

    /// Embed several search queries in one call. Backends that batch (e.g. ONNX)
    /// override this for throughput; the default falls back to per-query calls.
    /// Used by the [`EmbedBatcher`](crate::batch::EmbedBatcher) to coalesce
    /// concurrent queries into a single inference.
    fn embed_queries(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.embed_query(model, t)).collect()
    }
}
