//! HTTP routes: request/response DTOs, handlers, and the router.
//!
//! Storage- and inference-touching work is run via [`blocking`] so the async
//! runtime is never stalled by redb I/O or ONNX inference.

use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::trace::TraceLayer;

use nucleus_core::auth::{Perm, Scope};
use nucleus_core::backup::{BackupKind, BackupRecord};
use nucleus_core::engine::{QueryInput, SearchRequest};
use nucleus_core::id::{ChunkId, DocumentId, DomainId, JobId, SubdomainId, TagId, TokenId};
use nucleus_core::jobs::JobBody;
use nucleus_core::model::{Chunk, Document, Domain, Subdomain, Tag};
use nucleus_core::storage::Storage;
use nucleus_core::util::now_millis;
use nucleus_core::{Engine, NucleusError};

use crate::app::{blocking, ApiError, AppState, Auth, ScheduleConfig};

/// Resolve a subdomain name and label names into ids, creating any that don't
/// exist yet (turnkey ingest: the caller passes names, not ids).
fn resolve_structure(
    engine: &Engine,
    domain_id: DomainId,
    subdomain: Option<String>,
    labels: Vec<String>,
) -> nucleus_core::Result<(Option<SubdomainId>, Vec<TagId>)> {
    let sub = match subdomain {
        Some(name) if !name.trim().is_empty() => Some(
            engine
                .get_or_create_subdomain(domain_id, name.trim(), "")?
                .id,
        ),
        _ => None,
    };
    let mut tag_ids = Vec::new();
    for label in labels {
        let label = label.trim();
        if !label.is_empty() {
            tag_ids.push(engine.get_or_create_label(domain_id, label)?.id);
        }
    }
    Ok((sub, tag_ids))
}

/// Build the application router.
pub fn router(state: AppState) -> Router {
    let rate_limit_rpm = state.rate_limit_rpm;
    let app = Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .route("/v1/domains", post(create_domain).get(list_domains))
        .route("/v1/domains/{id}", get(get_domain))
        .route(
            "/v1/domains/{id}/documents",
            post(ingest_document).get(list_documents),
        )
        .route("/v1/domains/{id}/files", post(upload_file))
        .route("/v1/domains/{id}/search", post(search))
        .route("/v1/domains/{id}/tags", post(create_tag).get(list_tags))
        .route(
            "/v1/domains/{id}/subdomains",
            post(create_subdomain).get(list_subdomains),
        )
        .route(
            "/v1/documents/{id}",
            get(get_document).delete(delete_document),
        )
        .route("/v1/chunks/{id}", get(get_chunk))
        .route("/v1/chunks/{id}/context", get(chunk_context))
        .route("/v1/jobs", get(list_jobs))
        .route("/v1/jobs/{id}", get(get_job))
        .route("/v1/tokens", post(create_token).get(list_tokens))
        .route("/v1/tokens/{id}", delete(delete_token))
        .route("/v1/maintenance/persist", post(persist_indexes))
        .route("/v1/backups", post(create_backup).get(list_backups))
        .route("/v1/backups/restore", post(restore_backup))
        .route("/v1/backups/schedule", get(get_schedule).post(set_schedule))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024)) // allow large file uploads
        .layer(TraceLayer::new_for_http());

    // Rate limiting (outermost, so floods are shed before any work): added last so
    // it runs first. Off unless NUCLEUS_RATE_LIMIT_RPM > 0.
    let app = if rate_limit_rpm > 0 {
        let limiter = std::sync::Arc::new(crate::rate_limit::RateLimit::new(rate_limit_rpm));
        app.layer(axum::middleware::from_fn(move |req, next| {
            crate::rate_limit::enforce(limiter.clone(), req, next)
        }))
    } else {
        app
    };

    app.with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

/// Readiness: confirms the storage is reachable (cheap query).
async fn readyz(State(st): State<AppState>) -> Result<&'static str, ApiError> {
    let engine = st.engine.current();
    blocking(move || engine.list_domains()).await?;
    Ok("ready")
}

/// Prometheus-style metrics (plain text). Protect via network/proxy.
async fn metrics(State(st): State<AppState>) -> String {
    st.metrics.render()
}

// --- domains ---------------------------------------------------------------

#[derive(Deserialize)]
struct CreateDomainReq {
    name: String,
    #[serde(default)]
    model: Option<String>,
}

async fn create_domain(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<CreateDomainReq>,
) -> Result<Json<Domain>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let engine = st.engine.current();
    let domain = blocking(move || engine.create_domain(&req.name, req.model.as_deref())).await?;
    Ok(Json(domain))
}

async fn list_domains(
    State(st): State<AppState>,
    Auth(_ctx): Auth,
) -> Result<Json<Vec<Domain>>, ApiError> {
    let engine = st.engine.current();
    let domains = blocking(move || engine.list_domains()).await?;
    Ok(Json(domains))
}

async fn get_domain(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
) -> Result<Json<Domain>, ApiError> {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Read) {
        return Err(NucleusError::Forbidden.into());
    }
    let engine = st.engine.current();
    let domain = blocking(move || engine.get_domain(domain_id)).await?;
    Ok(Json(domain))
}

// --- documents & ingest ----------------------------------------------------

#[derive(Deserialize)]
struct IngestReq {
    title: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    chunks: Option<Vec<String>>,
    /// Subdomain name (created if it doesn't exist).
    #[serde(default)]
    subdomain: Option<String>,
    /// Label names (created if they don't exist).
    #[serde(default)]
    labels: Vec<String>,
    /// Existing tag ids (kept for compatibility; prefer `labels`).
    #[serde(default)]
    tags: Vec<u64>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct IngestResp {
    document_id: u64,
    /// `0` if the content was already ingested (`duplicate = true`).
    job_id: u64,
    duplicate: bool,
}

async fn ingest_document(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
    Json(req): Json<IngestReq>,
) -> Result<Json<IngestResp>, ApiError> {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Write) {
        return Err(NucleusError::Forbidden.into());
    }
    let IngestReq {
        title,
        source,
        text,
        chunks,
        subdomain,
        labels,
        tags,
        metadata,
    } = req;
    let body = match (chunks, text) {
        (Some(chunks), _) => JobBody::Chunks(chunks),
        (None, Some(text)) => JobBody::Text(text),
        (None, None) => return Err(NucleusError::invalid("provide `text` or `chunks`").into()),
    };
    // Deduplicate by content hash within the domain.
    let content = match &body {
        JobBody::Text(t) => t.clone(),
        JobBody::Chunks(c) => c.join("\n"),
    };
    let hash = nucleus_core::util::sha256_hex(content.as_bytes());
    {
        let e = st.engine.current();
        let h = hash.clone();
        if let Some(existing) = blocking(move || e.find_document_by_hash(domain_id, &h)).await? {
            st.metrics
                .ingest_duplicate_total
                .fetch_add(1, Ordering::Relaxed);
            return Ok(Json(IngestResp {
                document_id: existing.get(),
                job_id: 0,
                duplicate: true,
            }));
        }
    }
    let engine = st.engine.current();
    let (subdomain_id, mut tag_ids) =
        blocking(move || resolve_structure(&engine, domain_id, subdomain, labels)).await?;
    tag_ids.extend(tags.into_iter().map(TagId::new));
    let (doc, job_id) = st.queue.enqueue_ingest(
        domain_id,
        subdomain_id,
        &title,
        source,
        metadata,
        tag_ids,
        body,
    )?;
    {
        let e = st.engine.current();
        let did = doc.id;
        blocking(move || e.set_document_hash(domain_id, did, &hash)).await?;
    }
    st.metrics.ingest_total.fetch_add(1, Ordering::Relaxed);
    Ok(Json(IngestResp {
        document_id: doc.id.get(),
        job_id: job_id.get(),
        duplicate: false,
    }))
}

#[derive(Deserialize)]
struct UploadParams {
    /// Original file name; its extension selects the extractor.
    filename: String,
    #[serde(default)]
    title: Option<String>,
    /// Subdomain name (created if it doesn't exist).
    #[serde(default)]
    subdomain: Option<String>,
    /// Comma-separated label names (created if they don't exist).
    #[serde(default)]
    labels: Option<String>,
    /// Comma-separated existing tag ids.
    #[serde(default)]
    tags: Option<String>,
}

#[derive(Serialize)]
struct UploadResp {
    document_id: u64,
    /// `0` if the content was already ingested (`duplicate = true`).
    job_id: u64,
    /// Characters of text extracted from the file.
    chars: usize,
    duplicate: bool,
}

/// Upload a raw file (pdf, docx, xlsx, html, md, txt…). Nucleus extracts the
/// text **in-engine** and ingests it — transparent to the caller.
async fn upload_file(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
    Query(params): Query<UploadParams>,
    body: Bytes,
) -> Result<Json<UploadResp>, ApiError> {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Write) {
        return Err(NucleusError::Forbidden.into());
    }
    let filename = params.filename;
    let title = params.title.unwrap_or_else(|| filename.clone());

    // Extract text off the async runtime (PDF/spreadsheet parsing is CPU-bound).
    let bytes = body.to_vec();
    let fname = filename.clone();
    let text = blocking(move || nucleus_core::extract::extract_text(&fname, &bytes)).await?;
    let chars = text.chars().count();
    let hash = nucleus_core::util::sha256_hex(text.as_bytes());

    // Deduplicate: identical content already ingested in this domain.
    {
        let e = st.engine.current();
        let h = hash.clone();
        if let Some(existing) = blocking(move || e.find_document_by_hash(domain_id, &h)).await? {
            st.metrics
                .ingest_duplicate_total
                .fetch_add(1, Ordering::Relaxed);
            return Ok(Json(UploadResp {
                document_id: existing.get(),
                job_id: 0,
                chars,
                duplicate: true,
            }));
        }
    }

    // Resolve subdomain + labels by name (created if absent).
    let subdomain = params.subdomain.clone();
    let labels: Vec<String> = params
        .labels
        .as_deref()
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let engine = st.engine.current();
    let (subdomain_id, mut tag_ids) =
        blocking(move || resolve_structure(&engine, domain_id, subdomain, labels)).await?;
    tag_ids.extend(
        params
            .tags
            .as_deref()
            .unwrap_or("")
            .split(',')
            .filter_map(|s| s.trim().parse::<u64>().ok())
            .map(TagId::new),
    );

    let mut metadata = BTreeMap::new();
    metadata.insert("filename".to_string(), filename.clone());

    let (doc, job_id) = st.queue.enqueue_ingest(
        domain_id,
        subdomain_id,
        &title,
        Some(filename),
        metadata,
        tag_ids,
        JobBody::Text(text),
    )?;
    {
        let e = st.engine.current();
        let did = doc.id;
        blocking(move || e.set_document_hash(domain_id, did, &hash)).await?;
    }
    st.metrics.ingest_total.fetch_add(1, Ordering::Relaxed);
    Ok(Json(UploadResp {
        document_id: doc.id.get(),
        job_id: job_id.get(),
        chars,
        duplicate: false,
    }))
}

async fn get_document(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
) -> Result<Json<Document>, ApiError> {
    let doc_id = DocumentId::new(id);
    let engine = st.engine.current();
    let doc = blocking(move || engine.get_document(doc_id)).await?;
    if !ctx.allows(doc.domain_id, Perm::Read) {
        return Err(NucleusError::Forbidden.into());
    }
    Ok(Json(doc))
}

async fn delete_document(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
) -> Result<StatusCode, ApiError> {
    let doc_id = DocumentId::new(id);
    let engine = st.engine.current();
    let doc = {
        let engine = engine.clone();
        blocking(move || engine.get_document(doc_id)).await?
    };
    if !ctx.allows(doc.domain_id, Perm::Write) {
        return Err(NucleusError::Forbidden.into());
    }
    blocking(move || engine.delete_document(doc_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- chunks ----------------------------------------------------------------

async fn get_chunk(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
) -> Result<Json<Chunk>, ApiError> {
    let chunk_id = ChunkId::new(id);
    let engine = st.engine.current();
    let chunk = blocking(move || engine.get_chunk(chunk_id)).await?;
    if !ctx.allows(chunk.domain_id, Perm::Read) {
        return Err(NucleusError::Forbidden.into());
    }
    Ok(Json(chunk))
}

fn default_context() -> usize {
    1
}

fn default_limit() -> usize {
    50
}

/// Pagination query (`?offset=&limit=`); `limit` is capped server-side.
#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

async fn list_documents(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<Document>>, ApiError> {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Read) {
        return Err(NucleusError::Forbidden.into());
    }
    let limit = page.limit.min(500);
    let engine = st.engine.current();
    let docs = blocking(move || engine.list_documents(domain_id, page.offset, limit)).await?;
    Ok(Json(docs))
}

async fn list_jobs(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Query(page): Query<Page>,
) -> Result<Json<Vec<JobResp>>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let limit = page.limit.min(500);
    let engine = st.engine.current();
    let jobs = blocking(move || engine.list_jobs(page.offset, limit)).await?;
    let out = jobs
        .into_iter()
        .map(|j| JobResp {
            id: j.id.get(),
            status: format!("{:?}", j.status),
            attempts: j.attempts,
            error: j.error,
        })
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize)]
struct ContextParams {
    #[serde(default = "default_context")]
    before: usize,
    #[serde(default = "default_context")]
    after: usize,
}

/// Return a chunk plus its neighbours (for retrieval context expansion).
async fn chunk_context(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
    Query(params): Query<ContextParams>,
) -> Result<Json<Vec<Chunk>>, ApiError> {
    let chunk_id = ChunkId::new(id);
    let engine = st.engine.current();
    let center = {
        let engine = engine.clone();
        blocking(move || engine.get_chunk(chunk_id)).await?
    };
    if !ctx.allows(center.domain_id, Perm::Read) {
        return Err(NucleusError::Forbidden.into());
    }
    let chunks =
        blocking(move || engine.chunk_context(chunk_id, params.before, params.after)).await?;
    Ok(Json(chunks))
}

// --- search ----------------------------------------------------------------

fn default_k() -> usize {
    10
}

#[derive(Deserialize)]
struct SearchReq {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    query_vector: Option<Vec<f32>>,
    #[serde(default = "default_k")]
    k: usize,
    #[serde(default)]
    tags: Vec<u64>,
    #[serde(default)]
    match_all: bool,
    #[serde(default)]
    document_ids: Vec<u64>,
    /// Restrict to a subdomain (by name).
    #[serde(default)]
    subdomain: Option<String>,
    /// Optional query-language filter, e.g. `tag:legal AND NOT tag:draft`.
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Serialize)]
struct Hit {
    chunk_id: u64,
    document_id: u64,
    score: f32,
    text: String,
    tags: Vec<u64>,
    metadata: BTreeMap<String, String>,
}

async fn search(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
    Json(req): Json<SearchReq>,
) -> Response {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Read) {
        return ApiError::from(NucleusError::Forbidden).into_response();
    }

    // Load-shed: bound concurrent (CPU-bound) searches so a flood inflates queue
    // depth, not every request's latency. Wait briefly for a permit, then 503.
    let permit =
        match tokio::time::timeout(st.search_wait, st.search_sem.clone().acquire_owned()).await {
            Ok(Ok(p)) => p,
            _ => {
                st.metrics
                    .search_rejected_total
                    .fetch_add(1, Ordering::Relaxed);
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "server busy, retry shortly" })),
                )
                    .into_response();
            }
        };

    let result = run_search(&st, domain_id, req).await;
    drop(permit);
    match result {
        Ok(out) => Json(out).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn run_search(
    st: &AppState,
    domain_id: DomainId,
    req: SearchReq,
) -> Result<Vec<Hit>, ApiError> {
    let SearchReq {
        query,
        query_vector,
        k,
        tags,
        match_all,
        document_ids,
        subdomain,
        filter,
    } = req;

    // Resolve the subdomain by name (no creation). An unknown name scopes the
    // search to nothing, so return early with no results.
    let subdomain_id = match subdomain {
        Some(name) if !name.trim().is_empty() => {
            let engine = st.engine.current();
            let name = name.trim().to_string();
            match blocking(move || engine.subdomain_id_by_name(domain_id, &name)).await? {
                Some(sid) => Some(sid),
                None => return Ok(Vec::new()),
            }
        }
        _ => None,
    };

    // A precomputed `query_vector` stays dense-only. For a text query: if the
    // batcher is enabled, embed via it (coalesced) and pass `Hybrid` so BM25 still
    // runs; otherwise pass `Text` and let the engine embed it on the blocking pool
    // (independent, parallel — faster for small CPU models).
    let query_input = match (query_vector, query) {
        (Some(v), _) => QueryInput::Vector(v),
        (None, Some(text)) => match &st.batcher {
            Some(batcher) => {
                let engine = st.engine.current();
                let domain = blocking(move || engine.get_domain(domain_id)).await?;
                let vector = batcher.embed_query(&domain.model, &text).await?;
                QueryInput::Hybrid { text, vector }
            }
            None => QueryInput::Text(text),
        },
        (None, None) => {
            return Err(NucleusError::invalid("provide `query` or `query_vector`").into())
        }
    };

    let request = SearchRequest {
        query: query_input,
        k,
        tags: tags.into_iter().map(TagId::new).collect(),
        match_all,
        document_ids: document_ids.into_iter().map(DocumentId::new).collect(),
        subdomain: subdomain_id,
        filter,
        diversity: 0.0,
    };
    let started = std::time::Instant::now();
    let engine = st.engine.current();
    let hits = blocking(move || engine.search(domain_id, request)).await?;
    st.metrics.search_total.fetch_add(1, Ordering::Relaxed);
    st.metrics
        .search_latency_ms_total
        .fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
    Ok(hits
        .into_iter()
        .map(|h| Hit {
            chunk_id: h.chunk.id.get(),
            document_id: h.chunk.document_id.get(),
            score: h.score,
            text: h.chunk.text,
            tags: h.chunk.tags.into_iter().map(|t| t.get()).collect(),
            metadata: h.chunk.metadata,
        })
        .collect())
}

// --- tags ------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateTagReq {
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parent: Option<u64>,
}

async fn create_tag(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
    Json(req): Json<CreateTagReq>,
) -> Result<Json<Tag>, ApiError> {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Write) {
        return Err(NucleusError::Forbidden.into());
    }
    let CreateTagReq {
        name,
        display_name,
        description,
        parent,
    } = req;
    let display = display_name.unwrap_or_else(|| name.clone());
    let parent = parent.map(TagId::new);
    let engine = st.engine.current();
    let tag = blocking(move || engine.create_tag(domain_id, &name, &display, &description, parent))
        .await?;
    Ok(Json(tag))
}

async fn list_tags(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
) -> Result<Json<Vec<Tag>>, ApiError> {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Read) {
        return Err(NucleusError::Forbidden.into());
    }
    let engine = st.engine.current();
    let tags = blocking(move || engine.list_tags(domain_id)).await?;
    Ok(Json(tags))
}

// --- subdomains ------------------------------------------------------------

#[derive(Deserialize)]
struct CreateSubdomainReq {
    name: String,
    #[serde(default)]
    description: String,
}

async fn create_subdomain(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
    Json(req): Json<CreateSubdomainReq>,
) -> Result<Json<Subdomain>, ApiError> {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Write) {
        return Err(NucleusError::Forbidden.into());
    }
    let engine = st.engine.current();
    let sub =
        blocking(move || engine.get_or_create_subdomain(domain_id, &req.name, &req.description))
            .await?;
    Ok(Json(sub))
}

async fn list_subdomains(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
) -> Result<Json<Vec<Subdomain>>, ApiError> {
    let domain_id = DomainId::new(id);
    if !ctx.allows(domain_id, Perm::Read) {
        return Err(NucleusError::Forbidden.into());
    }
    let engine = st.engine.current();
    let subs = blocking(move || engine.list_subdomains(domain_id)).await?;
    Ok(Json(subs))
}

// --- jobs ------------------------------------------------------------------

#[derive(Serialize)]
struct JobResp {
    id: u64,
    status: String,
    attempts: u32,
    error: Option<String>,
}

async fn get_job(
    State(st): State<AppState>,
    Auth(_ctx): Auth,
    Path(id): Path<u64>,
) -> Result<Json<JobResp>, ApiError> {
    let job_id = JobId::new(id);
    let engine = st.engine.current();
    let job = blocking(move || engine.storage().get_job(job_id)).await?;
    Ok(Json(JobResp {
        id: job.id.get(),
        status: format!("{:?}", job.status),
        attempts: job.attempts,
        error: job.error,
    }))
}

// --- tokens ----------------------------------------------------------------

#[derive(Deserialize)]
struct CreateTokenReq {
    name: String,
    scopes: Vec<Scope>,
    #[serde(default)]
    expires_at: Option<i64>,
}

#[derive(Serialize)]
struct CreateTokenResp {
    id: u64,
    name: String,
    /// Plaintext token — shown only here, never again.
    token: String,
}

async fn create_token(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<CreateTokenReq>,
) -> Result<Json<CreateTokenResp>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let engine = st.engine.current();
    let (token, plaintext) =
        blocking(move || engine.create_token(&req.name, req.scopes, req.expires_at)).await?;
    Ok(Json(CreateTokenResp {
        id: token.id.get(),
        name: token.name,
        token: plaintext,
    }))
}

#[derive(Serialize)]
struct TokenInfo {
    id: u64,
    name: String,
    scopes: Vec<Scope>,
    created_at: i64,
    expires_at: Option<i64>,
}

async fn list_tokens(
    State(st): State<AppState>,
    Auth(ctx): Auth,
) -> Result<Json<Vec<TokenInfo>>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let engine = st.engine.current();
    let tokens = blocking(move || engine.list_tokens()).await?;
    let out = tokens
        .into_iter()
        .map(|t| TokenInfo {
            id: t.id.get(),
            name: t.name,
            scopes: t.scopes,
            created_at: t.created_at,
            expires_at: t.expires_at,
        })
        .collect();
    Ok(Json(out))
}

async fn delete_token(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<u64>,
) -> Result<StatusCode, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let token_id = TokenId::new(id);
    let engine = st.engine.current();
    blocking(move || engine.delete_token(token_id)).await?;
    Ok(StatusCode::NO_CONTENT) // idempotent
}

// --- maintenance -----------------------------------------------------------

#[derive(Serialize)]
struct PersistResp {
    persisted: usize,
}

/// Flush persistable (HNSW) indexes to disk. Admin only.
async fn persist_indexes(
    State(st): State<AppState>,
    Auth(ctx): Auth,
) -> Result<Json<PersistResp>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let engine = st.engine.current();
    let persisted = blocking(move || engine.persist_indexes()).await?;
    Ok(Json(PersistResp { persisted }))
}

// --- backups ---------------------------------------------------------------

#[derive(Deserialize)]
struct BackupReq {
    /// "full" (default) or "differential".
    #[serde(default)]
    kind: Option<String>,
}

/// Take a backup now (admin). Prunes old fulls per the current schedule's policy.
async fn create_backup(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<BackupReq>,
) -> Result<Json<BackupRecord>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let kind = match req.kind.as_deref().unwrap_or("full") {
        "full" => BackupKind::Full,
        "differential" | "diff" => BackupKind::Differential,
        other => return Err(NucleusError::invalid(format!("unknown backup kind: {other}")).into()),
    };
    let engine = st.engine.current();
    let backups = st.backups.clone();
    let keep = st.schedule.read().map(|s| s.keep_fulls).unwrap_or(7);
    let rec = blocking(move || {
        let rec = match kind {
            BackupKind::Full => backups.full(engine.storage())?,
            BackupKind::Differential => backups.differential(engine.storage())?,
        };
        let _ = backups.prune(keep);
        Ok(rec)
    })
    .await?;
    Ok(Json(rec))
}

/// List the backup catalog (admin).
async fn list_backups(
    State(st): State<AppState>,
    Auth(ctx): Auth,
) -> Result<Json<Vec<BackupRecord>>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let backups = st.backups.clone();
    let recs = blocking(move || backups.list()).await?;
    Ok(Json(recs))
}

#[derive(Deserialize)]
struct RestoreReq {
    /// Backup id to restore (full or differential).
    id: String,
}

#[derive(Serialize)]
struct RestoreResp {
    restored: String,
    /// New live database file the engine was swapped onto.
    active_db: String,
}

/// Restore a backup (admin). Takes a safety full backup of the current state,
/// reconstructs the chosen backup into a new database file, opens it and swaps
/// the live engine onto it (a brief in-flight cutover; the job queue follows).
async fn restore_backup(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<RestoreReq>,
) -> Result<Json<RestoreResp>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let id = req.id;
    let ts = now_millis();

    // 1) Safety snapshot of the current state before we replace it.
    {
        let engine = st.engine.current();
        let backups = st.backups.clone();
        blocking(move || backups.full(engine.storage())).await?;
    }

    // 2) Reconstruct the chosen backup into a fresh live database file.
    let new_db = st.data_dir.join(format!("nucleus-restored-{ts}.redb"));
    {
        let backups = st.backups.clone();
        let id = id.clone();
        let dst = new_db.clone();
        blocking(move || backups.restore_to(&id, &dst)).await?;
    }

    // 3) Open a new engine on the restored file and swap it in.
    let embedder = st.embedder.clone();
    let kind = st.index_kind;
    let index_dir = st.data_dir.join(format!("indexes-restored-{ts}"));
    let db_for_open = new_db.clone();
    let new_engine = blocking(move || {
        let storage = Storage::open(&db_for_open)?;
        Engine::open(storage, embedder, kind, Some(index_dir))
    })
    .await?;
    st.engine.swap(std::sync::Arc::new(new_engine));

    // 4) Persist the active-db pointer so a restart reopens the restored file.
    let _ = std::fs::write(
        st.data_dir.join("active_db.txt"),
        new_db.to_string_lossy().as_bytes(),
    );

    Ok(Json(RestoreResp {
        restored: id,
        active_db: new_db.to_string_lossy().into_owned(),
    }))
}

/// Read the current backup schedule (admin).
async fn get_schedule(
    State(st): State<AppState>,
    Auth(ctx): Auth,
) -> Result<Json<ScheduleConfig>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    let cfg = st
        .schedule
        .read()
        .map_err(|_| NucleusError::invalid("schedule lock poisoned"))?
        .clone();
    Ok(Json(cfg))
}

/// Replace the backup schedule at runtime (admin).
async fn set_schedule(
    State(st): State<AppState>,
    Auth(ctx): Auth,
    Json(cfg): Json<ScheduleConfig>,
) -> Result<Json<ScheduleConfig>, ApiError> {
    if !ctx.is_admin() {
        return Err(NucleusError::Forbidden.into());
    }
    *st.schedule
        .write()
        .map_err(|_| NucleusError::invalid("schedule lock poisoned"))? = cfg.clone();
    Ok(Json(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    use nucleus_core::backup::BackupManager;
    use nucleus_core::batch::EmbedBatcher;
    use nucleus_core::embed::{Embedder, MockEmbedder};
    use nucleus_core::engine::EngineHandle;
    use nucleus_core::jobs::JobQueue;
    use nucleus_core::storage::Storage;
    use nucleus_core::Engine;

    struct Harness {
        app: Router,
        token: String,
    }

    fn harness() -> (Harness, tempfile::TempDir) {
        harness_rpm(0)
    }

    fn harness_rpm(rate_limit_rpm: u32) -> (Harness, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path().join("n.redb")).unwrap();
        let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(64));
        let engine = Arc::new(Engine::new(storage, embedder.clone()).unwrap());
        let token = engine.bootstrap_admin_token().unwrap().unwrap();
        let handle = EngineHandle::new(engine);
        let queue = JobQueue::start(handle.clone(), 2, 3);
        let backups = Arc::new(BackupManager::open(dir.path().join("backups")).unwrap());
        let state = AppState {
            engine: handle,
            queue,
            metrics: std::sync::Arc::new(crate::app::Metrics::default()),
            search_sem: Arc::new(tokio::sync::Semaphore::new(8)),
            search_wait: std::time::Duration::from_secs(2),
            batcher: Some(Arc::new(EmbedBatcher::new(
                embedder.clone(),
                16,
                std::time::Duration::from_millis(5),
            ))),
            backups,
            embedder,
            index_kind: nucleus_core::index::IndexKind::Flat,
            data_dir: dir.path().to_path_buf(),
            schedule: std::sync::Arc::new(std::sync::RwLock::new(crate::app::ScheduleConfig {
                enabled: false,
                interval_secs: 0,
                full_every: 7,
                keep_fulls: 7,
            })),
            rate_limit_rpm,
        };
        (
            Harness {
                app: router(state),
                token,
            },
            dir,
        )
    }

    async fn call(
        app: &Router,
        method: &str,
        uri: &str,
        token: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, value)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn end_to_end_ingest_and_search() {
        let (h, _dir) = harness();

        // Unauthenticated request is rejected.
        let (status, _) = call(&h.app, "GET", "/v1/domains", "wrong", json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Create a domain.
        let (status, dom) = call(
            &h.app,
            "POST",
            "/v1/domains",
            &h.token,
            json!({"name":"docs"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let domain_id = dom["id"].as_u64().unwrap();

        // Create a tag.
        let (status, tag) = call(
            &h.app,
            "POST",
            &format!("/v1/domains/{domain_id}/tags"),
            &h.token,
            json!({"name":"legal"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let tag_id = tag["id"].as_u64().unwrap();

        // Ingest a document with two chunks, tagged legal.
        let (status, ing) = call(
            &h.app,
            "POST",
            &format!("/v1/domains/{domain_id}/documents"),
            &h.token,
            json!({
                "title": "doc",
                "tags": [tag_id],
                "chunks": ["el contrato laboral indefinido", "receta de pizza con piña"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let job_id = ing["job_id"].as_u64().unwrap();

        // Poll the job until it finishes.
        let mut done = false;
        for _ in 0..200 {
            let (_, job) = call(
                &h.app,
                "GET",
                &format!("/v1/jobs/{job_id}"),
                &h.token,
                json!({}),
            )
            .await;
            if job["status"] == "Done" {
                done = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(done, "ingest job never completed");

        // Search with a tag filter; expect the contract chunk.
        let (status, hits) = call(
            &h.app,
            "POST",
            &format!("/v1/domains/{domain_id}/search"),
            &h.token,
            json!({"query":"contrato laboral", "k": 3, "tags":[tag_id]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let arr = hits.as_array().unwrap();
        assert!(!arr.is_empty());
        assert!(arr[0]["text"].as_str().unwrap().contains("contrato"));

        // Search with a query-language filter.
        let (status, filtered) = call(
            &h.app,
            "POST",
            &format!("/v1/domains/{domain_id}/search"),
            &h.token,
            json!({"query":"contrato","k":5,"filter":"tag:legal AND NOT tag:draft"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!filtered.as_array().unwrap().is_empty());

        // Fetch a chunk plus its neighbours.
        let chunk_id = arr[0]["chunk_id"].as_u64().unwrap();
        let (status, ctx) = call(
            &h.app,
            "GET",
            &format!("/v1/chunks/{chunk_id}/context?before=1&after=1"),
            &h.token,
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!ctx.as_array().unwrap().is_empty());

        // Ingest with caller-provided subdomain + labels by name (auto-created).
        let (status, ing2) = call(
            &h.app,
            "POST",
            &format!("/v1/domains/{domain_id}/documents"),
            &h.token,
            json!({
                "title": "irpf",
                "subdomain": "irpf",
                "labels": ["2026", "irpf"],
                "chunks": ["tipos de retención de IRPF para 2026"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let job2 = ing2["job_id"].as_u64().unwrap();
        let mut done2 = false;
        for _ in 0..200 {
            let (_, j) = call(
                &h.app,
                "GET",
                &format!("/v1/jobs/{job2}"),
                &h.token,
                json!({}),
            )
            .await;
            if j["status"] == "Done" {
                done2 = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(done2, "subdomain ingest job never completed");

        // The subdomain was created on the fly and is listed.
        let (_, subs) = call(
            &h.app,
            "GET",
            &format!("/v1/domains/{domain_id}/subdomains"),
            &h.token,
            json!({}),
        )
        .await;
        assert!(subs.as_array().unwrap().iter().any(|s| s["name"] == "irpf"));

        // Search scoped to that subdomain returns the doc.
        let (status, scoped) = call(
            &h.app,
            "POST",
            &format!("/v1/domains/{domain_id}/search"),
            &h.token,
            json!({"query": "retención IRPF 2026", "subdomain": "irpf", "k": 3}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!scoped.as_array().unwrap().is_empty());

        // Scoping to a subdomain that doesn't exist returns nothing.
        let (status, none) = call(
            &h.app,
            "POST",
            &format!("/v1/domains/{domain_id}/search"),
            &h.token,
            json!({"query": "retención IRPF 2026", "subdomain": "no-existe", "k": 3}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(none.as_array().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rate_limit_sheds_over_budget() {
        // Burst of 3/min per IP; the test peer is unspecified (shared bucket).
        let (h, _dir) = harness_rpm(3);
        let mut statuses = Vec::new();
        for _ in 0..6 {
            let req = Request::builder()
                .method("GET")
                .uri("/healthz")
                .body(Body::empty())
                .unwrap();
            let resp = h.app.clone().oneshot(req).await.unwrap();
            statuses.push(resp.status());
        }
        assert_eq!(
            statuses[0],
            StatusCode::OK,
            "first request is within budget"
        );
        assert!(
            statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
            "a flood past the budget must yield 429s, got {statuses:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_rate_limit_by_default() {
        // rpm = 0 (default) → no shedding even under a burst.
        let (h, _dir) = harness();
        for _ in 0..20 {
            let req = Request::builder()
                .method("GET")
                .uri("/healthz")
                .body(Body::empty())
                .unwrap();
            let resp = h.app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn backup_list_restore_and_schedule() {
        let (h, _dir) = harness();

        // Domain that exists at backup time.
        let (status, dom) = call(
            &h.app,
            "POST",
            "/v1/domains",
            &h.token,
            json!({"name":"docs"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let _ = dom["id"].as_u64().unwrap();

        // Full backup.
        let (status, rec) = call(
            &h.app,
            "POST",
            "/v1/backups",
            &h.token,
            json!({"kind":"full"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let backup_id = rec["id"].as_str().unwrap().to_string();

        // The catalog lists it.
        let (status, list) = call(&h.app, "GET", "/v1/backups", &h.token, json!({})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(list
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == backup_id));

        // Create a SECOND domain AFTER the backup.
        let _ = call(
            &h.app,
            "POST",
            "/v1/domains",
            &h.token,
            json!({"name":"after"}),
        )
        .await;
        let (_s, before) = call(&h.app, "GET", "/v1/domains", &h.token, json!({})).await;
        assert_eq!(before.as_array().unwrap().len(), 2);

        // Restore the backup: the engine is swapped to the point-in-time.
        let (status, resp) = call(
            &h.app,
            "POST",
            "/v1/backups/restore",
            &h.token,
            json!({ "id": backup_id }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "restore failed: {resp}");

        // Auth still works (token was in the backup) and only the first domain is back.
        let (status, after) = call(&h.app, "GET", "/v1/domains", &h.token, json!({})).await;
        assert_eq!(status, StatusCode::OK);
        let names: Vec<&str> = after
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["docs"], "restore must reflect backup time");

        // Schedule can be set and read back.
        let (status, _) = call(
            &h.app,
            "POST",
            "/v1/backups/schedule",
            &h.token,
            json!({"enabled":true,"interval_secs":3600,"full_every":7,"keep_fulls":3}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (_s, sched) = call(&h.app, "GET", "/v1/backups/schedule", &h.token, json!({})).await;
        assert_eq!(sched["interval_secs"].as_u64().unwrap(), 3600);
        assert!(sched["enabled"].as_bool().unwrap());
    }
}
