//! Micro-batching of **query** embeddings.
//!
//! Each search needs to embed its query — a CPU-bound ONNX call. Doing one call
//! per request under concurrency wastes the model's batch dimension and pays the
//! per-call overhead repeatedly. The [`EmbedBatcher`] coalesces queries that
//! arrive within a small time window into a single [`Embedder::embed_queries`]
//! call, then fans the results back to each caller.
//!
//! It batches across concurrent requests, so it only helps when several searches
//! are in flight; a lone request just waits at most `window` (set it to a few ms).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::embed::Embedder;
use crate::error::NucleusError;
use crate::Result;

struct Job {
    model: String,
    text: String,
    resp: oneshot::Sender<Result<Vec<f32>>>,
}

/// Coalesces concurrent query-embedding requests into batched inferences.
#[derive(Clone)]
pub struct EmbedBatcher {
    tx: mpsc::Sender<Job>,
}

impl EmbedBatcher {
    /// Start the batcher. `max_batch` caps how many queries go in one inference;
    /// `window` is how long the collector waits to fill a batch. Must be called
    /// inside a Tokio runtime (it spawns a background task).
    pub fn new(embedder: Arc<dyn Embedder>, max_batch: usize, window: Duration) -> Self {
        let max_batch = max_batch.max(1);
        let (tx, rx) = mpsc::channel::<Job>(1024);
        tokio::spawn(run(rx, embedder, max_batch, window));
        Self { tx }
    }

    /// Embed one query, transparently batched with others in flight.
    pub async fn embed_query(&self, model: &str, text: &str) -> Result<Vec<f32>> {
        let (resp, rx) = oneshot::channel();
        self.tx
            .send(Job {
                model: model.to_string(),
                text: text.to_string(),
                resp,
            })
            .await
            .map_err(|_| NucleusError::embedding_msg("embed batcher stopped"))?;
        rx.await
            .map_err(|_| NucleusError::embedding_msg("embed batcher dropped the request"))?
    }
}

async fn run(
    mut rx: mpsc::Receiver<Job>,
    embedder: Arc<dyn Embedder>,
    max_batch: usize,
    window: Duration,
) {
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        if max_batch > 1 {
            let deadline = tokio::time::sleep(window);
            tokio::pin!(deadline);
            while batch.len() < max_batch {
                tokio::select! {
                    _ = &mut deadline => break,
                    maybe = rx.recv() => match maybe {
                        Some(job) => batch.push(job),
                        None => break,
                    },
                }
            }
        }
        process(&embedder, batch).await;
    }
}

async fn process(embedder: &Arc<dyn Embedder>, batch: Vec<Job>) {
    // Group by model (a batch may mix domains/models).
    let mut by_model: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, job) in batch.iter().enumerate() {
        by_model.entry(job.model.clone()).or_default().push(i);
    }

    let mut results: Vec<Option<Result<Vec<f32>>>> = (0..batch.len()).map(|_| None).collect();
    for (model, idxs) in by_model {
        let texts: Vec<String> = idxs.iter().map(|&i| batch[i].text.clone()).collect();
        let emb = embedder.clone();
        // Inference is CPU-bound: run it off the async runtime.
        let res = tokio::task::spawn_blocking(move || emb.embed_queries(&model, &texts)).await;
        match res {
            Ok(Ok(vecs)) if vecs.len() == idxs.len() => {
                for (k, &i) in idxs.iter().enumerate() {
                    results[i] = Some(Ok(vecs[k].clone()));
                }
            }
            Ok(Ok(_)) => set_err(&mut results, &idxs, "batch returned wrong vector count"),
            Ok(Err(e)) => {
                let msg = e.to_string();
                for &i in &idxs {
                    results[i] = Some(Err(NucleusError::embedding_msg(msg.clone())));
                }
            }
            Err(e) => {
                let msg = e.to_string();
                for &i in &idxs {
                    results[i] = Some(Err(NucleusError::embedding_msg(msg.clone())));
                }
            }
        }
    }

    for (job, res) in batch.into_iter().zip(results) {
        let _ = job
            .resp
            .send(res.unwrap_or_else(|| Err(NucleusError::embedding_msg("missing batch result"))));
    }
}

fn set_err(results: &mut [Option<Result<Vec<f32>>>], idxs: &[usize], msg: &str) {
    for &i in idxs {
        results[i] = Some(Err(NucleusError::embedding_msg(msg)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedder;

    #[tokio::test]
    async fn batches_match_per_query_results() {
        let embedder = Arc::new(MockEmbedder::new(32));
        let batcher = EmbedBatcher::new(embedder.clone(), 16, Duration::from_millis(5));

        // Fire several concurrent queries; each must get the same vector it would
        // get from a direct (unbatched) embed_query.
        let texts = ["contrato laboral", "pizza", "irpf 2026", "contrato laboral"];
        let mut handles = Vec::new();
        for t in texts {
            let b = batcher.clone();
            handles.push(tokio::spawn(async move {
                (t, b.embed_query("mock", t).await.unwrap())
            }));
        }
        for h in handles {
            let (t, got) = h.await.unwrap();
            let want = embedder.embed_query("mock", t).unwrap();
            assert_eq!(
                got, want,
                "batched result must equal direct embed for {t:?}"
            );
        }
    }

    #[tokio::test]
    async fn single_query_works() {
        let embedder = Arc::new(MockEmbedder::new(16));
        let batcher = EmbedBatcher::new(embedder.clone(), 8, Duration::from_millis(2));
        let v = batcher.embed_query("mock", "hola").await.unwrap();
        assert_eq!(v, embedder.embed_query("mock", "hola").unwrap());
    }
}
