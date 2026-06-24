//! Shared application plumbing: state, config, the error→HTTP mapping, the
//! bearer-token extractor, and a helper to run blocking engine work off the
//! async runtime.

use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use std::sync::RwLock;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Default machine-wide data directory when `NUCLEUS_DB` is unset. We follow the
/// platform convention for service/daemon data rather than the launch directory:
/// `%ProgramData%\Nucleus` on Windows, `/var/lib/nucleus` elsewhere. Derived from
/// the environment (never a hardcoded drive), with a sane fallback.
pub fn default_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        env::var_os("ProgramData")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("Nucleus")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/var/lib/nucleus")
    }
}

use serde::{Deserialize, Serialize};

use nucleus_core::auth::AuthContext;
use nucleus_core::backup::BackupManager;
use nucleus_core::batch::EmbedBatcher;
use nucleus_core::embed::Embedder;
use nucleus_core::engine::EngineHandle;
use nucleus_core::index::IndexKind;
use nucleus_core::jobs::JobQueue;
use nucleus_core::NucleusError;

/// Cloneable handle injected into every handler.
#[derive(Clone)]
pub struct AppState {
    /// Swappable live engine (restore replaces it atomically).
    pub engine: Arc<EngineHandle>,
    pub queue: Arc<JobQueue>,
    pub metrics: Arc<Metrics>,
    /// Bounds concurrent (CPU-bound) searches to protect tail latency.
    pub search_sem: Arc<Semaphore>,
    /// How long a search waits for a permit before being shed (503).
    pub search_wait: Duration,
    /// Coalesces concurrent query embeddings into batched inferences. `None`
    /// (the default) embeds each query independently in parallel, which is faster
    /// for small CPU models; enable batching only when it helps (e.g. GPU).
    pub batcher: Option<Arc<EmbedBatcher>>,
    /// Backup catalog + snapshot/restore operations.
    pub backups: Arc<BackupManager>,
    /// Embedder used to build a fresh engine on restore.
    pub embedder: Arc<dyn Embedder>,
    /// Index backend for engines built on restore.
    pub index_kind: IndexKind,
    /// Directory where restored database files and the active-db pointer live.
    pub data_dir: PathBuf,
    /// Runtime-adjustable backup schedule.
    pub schedule: Arc<RwLock<ScheduleConfig>>,
}

/// Backup schedule (runtime-adjustable via the API).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleConfig {
    /// Whether scheduled backups run.
    pub enabled: bool,
    /// Base cadence in seconds between scheduled backups.
    pub interval_secs: u64,
    /// Every Nth scheduled backup is a full; the rest are differentials.
    pub full_every: u32,
    /// How many full backups (and their differentials) to retain.
    pub keep_fulls: usize,
}

/// Lightweight in-process counters exposed at `/metrics` (Prometheus text).
#[derive(Default)]
pub struct Metrics {
    pub search_total: AtomicU64,
    pub search_latency_ms_total: AtomicU64,
    pub search_rejected_total: AtomicU64,
    pub ingest_total: AtomicU64,
    pub ingest_duplicate_total: AtomicU64,
}

impl Metrics {
    pub fn render(&self) -> String {
        let v = |n: &AtomicU64| n.load(Ordering::Relaxed);
        format!(
            "# HELP nucleus_search_total Total search requests served.\n\
             # TYPE nucleus_search_total counter\n\
             nucleus_search_total {}\n\
             # HELP nucleus_search_latency_ms_total Cumulative search latency in ms.\n\
             # TYPE nucleus_search_latency_ms_total counter\n\
             nucleus_search_latency_ms_total {}\n\
             # HELP nucleus_search_rejected_total Searches shed because the server was at capacity.\n\
             # TYPE nucleus_search_rejected_total counter\n\
             nucleus_search_rejected_total {}\n\
             # HELP nucleus_ingest_total Documents accepted for ingestion.\n\
             # TYPE nucleus_ingest_total counter\n\
             nucleus_ingest_total {}\n\
             # HELP nucleus_ingest_duplicate_total Ingestions skipped as duplicates.\n\
             # TYPE nucleus_ingest_duplicate_total counter\n\
             nucleus_ingest_duplicate_total {}\n",
            v(&self.search_total),
            v(&self.search_latency_ms_total),
            v(&self.search_rejected_total),
            v(&self.ingest_total),
            v(&self.ingest_duplicate_total),
        )
    }
}

/// Server configuration, read from the environment with sane defaults.
pub struct Config {
    pub db_path: PathBuf,
    pub addr: String,
    pub workers: usize,
    pub model_cache: Option<PathBuf>,
    pub index_kind: IndexKind,
    pub gpu: bool,
    pub index_dir: PathBuf,
    /// Where the bootstrap admin token is written (kept out of logs).
    pub admin_token_file: PathBuf,
    /// Allow any CORS origin (for browser clients). Off by default.
    pub cors_any: bool,
    /// Cross-encoder reranker model id; `None` disables reranking.
    pub rerank_model: Option<String>,
    /// How many top candidates the reranker re-scores per query (when enabled).
    pub rerank_candidates: Option<usize>,
    /// Max concurrent searches before shedding (load-shed). Defaults to the core count.
    pub max_concurrent_searches: usize,
    /// How long a search waits for a concurrency permit before a 503.
    pub search_wait_ms: u64,
    /// Max query embeddings coalesced into one inference. `1` (default) disables
    /// batching (each query embeds independently, in parallel).
    pub embed_batch_max: usize,
    /// How long the batcher waits to fill a batch (ms).
    pub embed_batch_window_ms: u64,
    /// Directory where backups (snapshots, deltas, catalog) are stored.
    pub backup_dir: PathBuf,
    /// Scheduled-backup cadence in seconds; `0` disables scheduling.
    pub backup_interval_secs: u64,
    /// Every Nth scheduled backup is a full (the rest differentials).
    pub backup_full_every: u32,
    /// How many full backups (and their differentials) to retain.
    pub backup_keep_fulls: usize,
}

impl Config {
    pub fn from_env() -> Self {
        let db_path: PathBuf = env::var("NUCLEUS_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_data_dir().join("nucleus.redb"));
        let index_dir = env::var("NUCLEUS_INDEX_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                db_path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| p.join("nucleus_indexes"))
                    .unwrap_or_else(|| PathBuf::from("nucleus_indexes"))
            });
        let admin_token_file = env::var("NUCLEUS_ADMIN_TOKEN_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                db_path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| p.join("nucleus_admin_token.txt"))
                    .unwrap_or_else(|| PathBuf::from("nucleus_admin_token.txt"))
            });
        let backup_dir = env::var("NUCLEUS_BACKUP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                db_path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| p.join("nucleus_backups"))
                    .unwrap_or_else(|| PathBuf::from("nucleus_backups"))
            });
        Self {
            db_path,
            addr: env::var("NUCLEUS_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string()),
            workers: env::var("NUCLEUS_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),
            model_cache: env::var("NUCLEUS_MODEL_CACHE").ok().map(PathBuf::from),
            index_kind: env::var("NUCLEUS_INDEX")
                .ok()
                .and_then(|v| IndexKind::parse(&v))
                .unwrap_or_default(),
            gpu: env::var("NUCLEUS_GPU")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false),
            admin_token_file,
            cors_any: env::var("NUCLEUS_CORS_ANY")
                .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false),
            rerank_model: env::var("NUCLEUS_RERANK_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            rerank_candidates: env::var("NUCLEUS_RERANK_CANDIDATES")
                .ok()
                .and_then(|v| v.parse().ok()),
            max_concurrent_searches: env::var("NUCLEUS_MAX_CONCURRENT_SEARCHES")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&n: &usize| n > 0)
                .unwrap_or_else(default_search_concurrency),
            search_wait_ms: env::var("NUCLEUS_SEARCH_WAIT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2000),
            embed_batch_max: env::var("NUCLEUS_EMBED_BATCH_MAX")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&n: &usize| n > 0)
                .unwrap_or(1),
            embed_batch_window_ms: env::var("NUCLEUS_EMBED_BATCH_WINDOW_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            backup_interval_secs: env::var("NUCLEUS_BACKUP_INTERVAL")
                .ok()
                .and_then(|v| parse_duration_secs(&v))
                .unwrap_or(0),
            backup_full_every: env::var("NUCLEUS_BACKUP_FULL_EVERY")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&n: &u32| n > 0)
                .unwrap_or(7),
            backup_keep_fulls: env::var("NUCLEUS_BACKUP_KEEP")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|&n: &usize| n > 0)
                .unwrap_or(7),
            backup_dir,
            index_dir,
        }
    }
}

/// Parse a duration like `30s`, `15m`, `6h`, `1d`, `2w` (or a bare number of
/// seconds) into seconds.
pub fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (num, mult) = match s.chars().last().unwrap() {
        's' => (&s[..s.len() - 1], 1),
        'm' => (&s[..s.len() - 1], 60),
        'h' => (&s[..s.len() - 1], 3600),
        'd' => (&s[..s.len() - 1], 86_400),
        'w' => (&s[..s.len() - 1], 604_800),
        c if c.is_ascii_digit() => (s, 1),
        _ => return None,
    };
    num.trim().parse::<u64>().ok().map(|n| n * mult)
}

/// Default max concurrent searches. A **safety valve**, not a throttle: measured
/// throughput keeps rising with oversubscription (HT, overlapping memory stalls),
/// so capping at the core count needlessly lowers it (~25%). We default generous
/// (16× cores) to bound pathological floods without throttling normal load;
/// operators wanting tighter tail latency can lower it and pair it with a short
/// `NUCLEUS_SEARCH_WAIT_MS` to shed early.
fn default_search_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_mul(16))
        .unwrap_or(64)
}

/// Wrapper that turns a [`NucleusError`] into an HTTP response.
pub struct ApiError(pub NucleusError);

impl From<NucleusError> for ApiError {
    fn from(e: NucleusError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        use NucleusError::*;
        let (status, message) = match &self.0 {
            DomainNotFound(_) | DocumentNotFound(_) | ChunkNotFound(_) | TagNotFound(_)
            | JobNotFound(_) => (StatusCode::NOT_FOUND, self.0.to_string()),
            Unauthorized => (StatusCode::UNAUTHORIZED, self.0.to_string()),
            Forbidden => (StatusCode::FORBIDDEN, self.0.to_string()),
            ModelNotFound(_) | InvalidRequest(_) | DimensionMismatch { .. } => {
                (StatusCode::BAD_REQUEST, self.0.to_string())
            }
            other => {
                tracing::error!(error = %other, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// Extractor that authenticates the `Authorization: Bearer <token>` header.
pub struct Auth(pub AuthContext);

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .ok_or(NucleusError::Unauthorized)?;
        let ctx = state.engine.current().authenticate(token.trim())?;
        Ok(Auth(ctx))
    }
}

/// Run a blocking engine operation on the blocking thread pool, mapping both the
/// engine error and any join error into an [`ApiError`].
pub async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> nucleus_core::Result<T> + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result.map_err(ApiError::from),
        Err(join) => Err(ApiError(NucleusError::embedding_msg(format!(
            "background task failed: {join}"
        )))),
    }
}
