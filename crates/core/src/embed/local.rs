use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use super::Embedder;
use crate::error::NucleusError;
use crate::Result;

/// Static description of a supported model: which fastembed model backs it, its
/// output dimension, and whether it needs the e5 `query:`/`passage:` prefixes.
#[derive(Clone)]
struct ModelSpec {
    fe: EmbeddingModel,
    dim: usize,
    e5_prefixes: bool,
}

impl ModelSpec {
    fn doc_prefix(&self) -> &'static str {
        if self.e5_prefixes {
            "passage: "
        } else {
            ""
        }
    }

    fn query_prefix(&self) -> &'static str {
        if self.e5_prefixes {
            "query: "
        } else {
            ""
        }
    }
}

/// Map a friendly model id to its [`ModelSpec`]. Unknown ids yield
/// [`NucleusError::ModelNotFound`].
fn spec(model: &str) -> Option<ModelSpec> {
    let (fe, dim, e5_prefixes) = match model {
        "multilingual-e5-small" => (EmbeddingModel::MultilingualE5Small, 384, true),
        "bge-small-en-v1.5" => (EmbeddingModel::BGESmallENV15, 384, false),
        "all-minilm-l6-v2" => (EmbeddingModel::AllMiniLML6V2, 384, false),
        _ => return None,
    };
    Some(ModelSpec {
        fe,
        dim,
        e5_prefixes,
    })
}

/// In-process embedder backed by fastembed / ONNX Runtime.
///
/// Models are loaded lazily on first use (downloaded from HuggingFace and cached)
/// and shared behind `Arc`, so repeated calls reuse a single ONNX session.
pub struct LocalEmbedder {
    cache_dir: Option<PathBuf>,
    gpu: bool,
    loaded: Mutex<HashMap<String, Arc<TextEmbedding>>>,
}

impl LocalEmbedder {
    /// Use fastembed's default cache directory for model files (CPU).
    pub fn new() -> Self {
        Self::with_options(None, false)
    }

    /// Cache downloaded model files under `dir` (CPU).
    pub fn with_cache_dir(dir: impl Into<PathBuf>) -> Self {
        Self::with_options(Some(dir.into()), false)
    }

    /// Full constructor. `gpu` requests a GPU execution provider (only effective
    /// when compiled with the `gpu` feature; otherwise CPU is used).
    pub fn with_options(cache_dir: Option<PathBuf>, gpu: bool) -> Self {
        Self {
            cache_dir,
            gpu,
            loaded: Mutex::new(HashMap::new()),
        }
    }

    fn get_or_load(&self, model: &str) -> Result<(Arc<TextEmbedding>, ModelSpec)> {
        let spec = spec(model).ok_or_else(|| NucleusError::ModelNotFound(model.to_string()))?;
        let mut guard = self.loaded.lock();
        if let Some(existing) = guard.get(model) {
            return Ok((existing.clone(), spec));
        }
        let mut opts = InitOptions::new(spec.fe.clone()).with_show_download_progress(false);
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
            TextEmbedding::try_new(opts).map_err(|e| NucleusError::embedding_msg(e.to_string()))?;
        let arc = Arc::new(te);
        guard.insert(model.to_string(), arc.clone());
        Ok((arc, spec))
    }
}

impl Default for LocalEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for LocalEmbedder {
    fn dim(&self, model: &str) -> Option<usize> {
        spec(model).map(|s| s.dim)
    }

    fn embed_documents(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let (te, spec) = self.get_or_load(model)?;
        let inputs: Vec<String> = texts
            .iter()
            .map(|t| format!("{}{}", spec.doc_prefix(), t))
            .collect();
        te.embed(inputs, None)
            .map_err(|e| NucleusError::embedding_msg(e.to_string()))
    }

    fn embed_query(&self, model: &str, text: &str) -> Result<Vec<f32>> {
        let (te, spec) = self.get_or_load(model)?;
        let input = format!("{}{}", spec.query_prefix(), text);
        let mut out = te
            .embed(vec![input], None)
            .map_err(|e| NucleusError::embedding_msg(e.to_string()))?;
        out.pop()
            .ok_or_else(|| NucleusError::embedding_msg("embedding backend returned no vector"))
    }

    fn embed_queries(&self, model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let (te, spec) = self.get_or_load(model)?;
        let inputs: Vec<String> = texts
            .iter()
            .map(|t| format!("{}{}", spec.query_prefix(), t))
            .collect();
        let out = te
            .embed(inputs, None)
            .map_err(|e| NucleusError::embedding_msg(e.to_string()))?;
        if out.len() != texts.len() {
            return Err(NucleusError::embedding_msg(
                "embedder returned a different number of vectors than inputs",
            ));
        }
        Ok(out)
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
