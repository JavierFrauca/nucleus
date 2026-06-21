//! Optional cross-encoder **reranking**: a precise second stage that re-scores
//! the top retrieval candidates by feeding `(query, passage)` pairs through a
//! cross-encoder. Disabled unless a reranker is configured (it needs a model and
//! is slower than retrieval). Runs in-process via fastembed.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use fastembed::{RerankInitOptions, RerankerModel, TextRerank};
use parking_lot::Mutex;

use crate::error::NucleusError;
use crate::Result;

/// Default reranker model id.
pub const DEFAULT_RERANK_MODEL: &str = "bge-reranker-base";

/// Scores `(query, document)` relevance for a batch of documents.
pub trait Reranker: Send + Sync {
    /// Return one relevance score per document, aligned with `docs` order.
    fn rerank(&self, query: &str, docs: &[String]) -> Result<Vec<f32>>;
}

fn rerank_model(name: &str) -> Option<RerankerModel> {
    match name {
        "bge-reranker-base" => Some(RerankerModel::BGERerankerBase),
        _ => None,
    }
}

/// In-process reranker backed by fastembed (ONNX). The model is loaded lazily on
/// first use and cached.
pub struct LocalReranker {
    model_name: String,
    cache_dir: Option<PathBuf>,
    gpu: bool,
    model: Mutex<Option<Arc<TextRerank>>>,
}

impl LocalReranker {
    /// CPU reranker.
    pub fn new(model_name: impl Into<String>, cache_dir: Option<PathBuf>) -> Self {
        Self::with_options(model_name, cache_dir, false)
    }

    /// Full constructor. `gpu` requests a GPU execution provider (only effective
    /// when compiled with the `gpu` feature; otherwise CPU is used).
    pub fn with_options(
        model_name: impl Into<String>,
        cache_dir: Option<PathBuf>,
        gpu: bool,
    ) -> Self {
        Self {
            model_name: model_name.into(),
            cache_dir,
            gpu,
            model: Mutex::new(None),
        }
    }

    fn get_or_load(&self) -> Result<Arc<TextRerank>> {
        let mut guard = self.model.lock();
        if let Some(m) = guard.as_ref() {
            return Ok(m.clone());
        }
        let model = rerank_model(&self.model_name)
            .ok_or_else(|| NucleusError::ModelNotFound(self.model_name.clone()))?;
        let mut opts = RerankInitOptions::new(model).with_show_download_progress(false);
        if let Some(dir) = &self.cache_dir {
            opts = opts.with_cache_dir(dir.clone());
        }
        #[cfg(feature = "gpu")]
        if self.gpu {
            opts = opts.with_execution_providers(gpu_execution_providers());
        }
        #[cfg(not(feature = "gpu"))]
        let _ = self.gpu; // field is only consulted under the `gpu` feature
        let te =
            TextRerank::try_new(opts).map_err(|e| NucleusError::embedding_msg(e.to_string()))?;
        let arc = Arc::new(te);
        *guard = Some(arc.clone());
        Ok(arc)
    }
}

/// GPU-first execution providers (DirectML, then CPU fallback). Only compiled
/// with the `gpu` feature.
#[cfg(feature = "gpu")]
fn gpu_execution_providers() -> Vec<fastembed::ExecutionProviderDispatch> {
    use ort::execution_providers::{CPUExecutionProvider, DirectMLExecutionProvider};
    vec![
        DirectMLExecutionProvider::default().build(),
        CPUExecutionProvider::default().build(),
    ]
}

impl Reranker for LocalReranker {
    fn rerank(&self, query: &str, docs: &[String]) -> Result<Vec<f32>> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let model = self.get_or_load()?;
        let doc_refs: Vec<&str> = docs.iter().map(|s| s.as_str()).collect();
        let results = model
            .rerank(query, doc_refs, false, None)
            .map_err(|e| NucleusError::embedding_msg(e.to_string()))?;
        let mut scores = vec![0f32; docs.len()];
        for r in results {
            if r.index < scores.len() {
                scores[r.index] = r.score;
            }
        }
        Ok(scores)
    }
}

fn tokens(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_lowercase())
        .collect()
}

/// Deterministic, dependency-free reranker for tests: term-overlap score.
#[derive(Debug, Clone, Default)]
pub struct MockReranker;

impl Reranker for MockReranker {
    fn rerank(&self, query: &str, docs: &[String]) -> Result<Vec<f32>> {
        let q = tokens(query);
        Ok(docs
            .iter()
            .map(|d| q.intersection(&tokens(d)).count() as f32)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_orders_by_overlap() {
        let rr = MockReranker;
        let docs = vec![
            "disposiciones generales".to_string(),
            "el artículo 14 sobre vacaciones".to_string(),
        ];
        let scores = rr.rerank("artículo 14 vacaciones", &docs).unwrap();
        assert!(scores[1] > scores[0]);
    }
}
