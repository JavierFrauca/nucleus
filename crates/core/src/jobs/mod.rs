//! Background jobs for scalability.
//!
//! Heavy work (chunking + embedding inference, deletes) is offloaded to a
//! persisted queue stored in redb and drained by a pool of tokio workers.
//! Because jobs are durable, an ingest survives a restart; because claiming is a
//! single redb write transaction, two workers never run the same job. Inference
//! is CPU-bound, so each job runs on `spawn_blocking`.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::engine::{Engine, EngineHandle, IngestBody};
use crate::id::{DocumentId, DomainId, JobId, SubdomainId, TagId};
use crate::model::Document;
use crate::Result;

/// Serializable mirror of [`IngestBody`] (stored inside a job).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobBody {
    Text(String),
    Chunks(Vec<String>),
}

impl From<JobBody> for IngestBody {
    fn from(body: JobBody) -> Self {
        match body {
            JobBody::Text(t) => IngestBody::Text(t),
            JobBody::Chunks(c) => IngestBody::Chunks(c),
        }
    }
}

/// What a job does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JobKind {
    /// Chunk, embed and index the body for an already-created document.
    Ingest {
        document_id: DocumentId,
        body: JobBody,
    },
    /// Delete a document and its chunks.
    DeleteDocument { document_id: DocumentId },
}

/// Lifecycle of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Running,
    Done,
    Failed,
}

/// A unit of background work.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub status: JobStatus,
    pub attempts: u32,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Handle used to enqueue work and wake workers.
pub struct JobQueue {
    handle: Arc<EngineHandle>,
    notify: Arc<Notify>,
}

impl JobQueue {
    /// Start `workers` background tasks. Requeues any jobs left `Running` by a
    /// previous (crashed) run. Must be called from within a tokio runtime. The
    /// queue resolves the live engine through `handle` on every job, so a runtime
    /// engine swap (restore) is picked up automatically.
    pub fn start(handle: Arc<EngineHandle>, workers: usize, max_attempts: u32) -> Arc<Self> {
        // Best-effort crash recovery; a failure here shouldn't abort startup.
        let _ = handle.current().storage().requeue_running();

        let notify = Arc::new(Notify::new());
        let queue = Arc::new(Self {
            handle: handle.clone(),
            notify: notify.clone(),
        });

        for _ in 0..workers.max(1) {
            let handle = handle.clone();
            let notify = notify.clone();
            tokio::spawn(async move { worker_loop(handle, notify, max_attempts).await });
        }
        // Periodic retention sweep: drop terminal jobs older than 24h.
        {
            let handle = handle.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(600));
                loop {
                    tick.tick().await;
                    let cutoff = crate::util::now_millis() - 24 * 3600 * 1000;
                    let e = handle.current();
                    let _ = tokio::task::spawn_blocking(move || e.storage().purge_finished(cutoff))
                        .await;
                }
            });
        }
        // Kick once in case there are pending jobs from a previous run.
        notify.notify_waiters();
        queue
    }

    /// Create the document row immediately and enqueue its population. Returns
    /// the new document plus the job id tracking the work.
    #[allow(clippy::too_many_arguments)]
    pub fn enqueue_ingest(
        &self,
        domain_id: DomainId,
        subdomain_id: Option<SubdomainId>,
        title: &str,
        source: Option<String>,
        metadata: std::collections::BTreeMap<String, String>,
        tags: Vec<TagId>,
        body: JobBody,
    ) -> Result<(Document, JobId)> {
        let engine = self.handle.current();
        let doc = engine.create_document_record(
            domain_id,
            subdomain_id,
            title,
            source,
            metadata,
            tags,
        )?;
        let job = engine.storage().create_job(JobKind::Ingest {
            document_id: doc.id,
            body,
        })?;
        self.notify.notify_waiters();
        Ok((doc, job.id))
    }

    /// Enqueue a document deletion.
    pub fn enqueue_delete(&self, document_id: DocumentId) -> Result<JobId> {
        let engine = self.handle.current();
        engine.get_document(document_id)?; // 404 fast if missing
        let job = engine
            .storage()
            .create_job(JobKind::DeleteDocument { document_id })?;
        self.notify.notify_waiters();
        Ok(job.id)
    }
}

async fn worker_loop(handle: Arc<EngineHandle>, notify: Arc<Notify>, max_attempts: u32) {
    loop {
        let engine = handle.current();
        let did_work = tokio::task::spawn_blocking(move || process_one(&engine, max_attempts))
            .await
            .unwrap_or(Ok(false))
            .unwrap_or(false);

        if !did_work {
            // Idle: wake on new work, but also poll occasionally as a safety net.
            tokio::select! {
                _ = notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            }
        }
    }
}

/// Claim and run a single job. Returns `Ok(true)` if a job was processed.
fn process_one(engine: &Engine, max_attempts: u32) -> Result<bool> {
    let Some(job) = engine.storage().claim_next_pending()? else {
        return Ok(false);
    };
    let id = job.id;
    match run_job(engine, job) {
        Ok(()) => engine.storage().finish_job(id, JobStatus::Done, None)?,
        Err(e) => {
            // `attempts` was incremented on claim; retry until the cap.
            let attempts = engine.storage().get_job(id)?.attempts;
            let status = if attempts >= max_attempts {
                JobStatus::Failed
            } else {
                JobStatus::Pending
            };
            engine
                .storage()
                .finish_job(id, status, Some(e.to_string()))?;
        }
    }
    Ok(true)
}

fn run_job(engine: &Engine, job: Job) -> Result<()> {
    match job.kind {
        JobKind::Ingest { document_id, body } => {
            let doc = engine.get_document(document_id)?;
            engine.populate_document(&doc, body.into())?;
            Ok(())
        }
        JobKind::DeleteDocument { document_id } => engine.delete_document(document_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedder;
    use crate::engine::{QueryInput, SearchRequest};
    use crate::storage::Storage;

    async fn wait_done(engine: &Engine, job: JobId) -> JobStatus {
        for _ in 0..200 {
            let status = engine.storage().get_job(job).unwrap().status;
            if matches!(status, JobStatus::Done | JobStatus::Failed) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("job did not finish in time");
    }

    #[tokio::test]
    async fn ingest_via_job_is_searchable() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path().join("n.redb")).unwrap();
        let engine = Arc::new(Engine::new(storage, Arc::new(MockEmbedder::new(64))).unwrap());
        let queue = JobQueue::start(EngineHandle::new(engine.clone()), 2, 3);

        let dom = engine.create_domain("docs", None).unwrap();
        let (_doc, job) = queue
            .enqueue_ingest(
                dom.id,
                None,
                "d",
                None,
                Default::default(),
                vec![],
                JobBody::Chunks(vec!["el contrato laboral".into(), "pizza con piña".into()]),
            )
            .unwrap();

        assert_eq!(wait_done(&engine, job).await, JobStatus::Done);

        let hits = engine
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].chunk.text.contains("contrato"));
    }
}
