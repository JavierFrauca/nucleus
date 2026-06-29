//! Nucleus HTTP server: wires storage, the in-process embedder, the engine and
//! the job queue, then serves the REST API.

mod app;
mod rate_limit;
mod routes;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use nucleus_core::backup::BackupManager;
use nucleus_core::batch::EmbedBatcher;
use nucleus_core::embed::{Embedder, LocalEmbedder};
use nucleus_core::engine::EngineHandle;
use nucleus_core::jobs::JobQueue;
use nucleus_core::rerank::LocalReranker;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;
use tokio::sync::Semaphore;

use crate::app::{AppState, Config, ScheduleConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_tracing();
    let cfg = Config::from_env();

    // Where restored database files and the active-db pointer live.
    let data_dir = cfg
        .db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    // Ensure the data directory exists — the default now lives under a platform
    // location (e.g. %ProgramData%\Nucleus) that may not exist on first run.
    std::fs::create_dir_all(&data_dir)?;
    // A previous restore may have pointed the live database at a new file.
    let db_path = active_db_path(&data_dir).unwrap_or_else(|| cfg.db_path.clone());
    if db_path != cfg.db_path {
        tracing::info!("opening restored database at {}", db_path.display());
    }

    let storage =
        Storage::open_with_options(&db_path, cfg.passphrase.as_deref(), cfg.keyfile.as_deref())?;
    if cfg.passphrase.is_some() {
        tracing::info!("encryption at rest: on (XChaCha20-Poly1305, passphrase key)");
    } else {
        tracing::info!("encryption at rest: on (XChaCha20-Poly1305, machine key)");
        tracing::warn!(
            "using a machine key{}: BACK IT UP — losing the key file makes this database \
             unrecoverable. Set NUCLEUS_PASSPHRASE for portable, recoverable protection.",
            cfg.keyfile
                .as_deref()
                .map(|p| format!(" at {}", p.display()))
                .unwrap_or_else(|| " (NUCLEUS_KEYFILE / default user-config location)".to_string())
        );
    }
    let embedder: Arc<dyn Embedder> = Arc::new(LocalEmbedder::with_options(
        cfg.model_cache.clone(),
        cfg.gpu,
    ));
    let engine = Arc::new(Engine::open(
        storage,
        embedder.clone(),
        cfg.index_kind,
        Some(cfg.index_dir.clone()),
    )?);

    if let Some(token) = engine.bootstrap_admin_token()? {
        // Write the secret to a file rather than the logs.
        match std::fs::write(&cfg.admin_token_file, &token) {
            Ok(()) => println!(
                "\nNucleus: bootstrap admin token written to {}\n(keep it safe; shown only once, not logged)\n",
                cfg.admin_token_file.display()
            ),
            Err(e) => println!(
                "\nNucleus bootstrap admin token (store it — shown once):\n   {token}\n(could not write {}: {e})\n",
                cfg.admin_token_file.display()
            ),
        }
    }

    if let Some(model) = &cfg.rerank_model {
        engine.set_reranker(Arc::new(LocalReranker::with_options(
            model.clone(),
            cfg.model_cache.clone(),
            cfg.gpu,
        )));
        if let Some(n) = cfg.rerank_candidates {
            engine.set_rerank_candidates(n);
        }
        tracing::info!(
            "reranking enabled (model: {model}, gpu: {}, candidates: {})",
            cfg.gpu,
            cfg.rerank_candidates
                .map(|n| n.to_string())
                .unwrap_or_else(|| "default".to_string())
        );
    }

    // Swappable engine handle, shared by the job queue, handlers and restore.
    let handle = EngineHandle::new(engine);
    let queue = JobQueue::start(handle.clone(), cfg.workers, 3);
    let handle_for_shutdown = handle.clone();

    // Batching only helps when per-call overhead dominates (e.g. GPU); for small
    // CPU models independent parallel embeds are faster, so it's off by default.
    let batcher = if cfg.embed_batch_max > 1 {
        Some(Arc::new(EmbedBatcher::new(
            embedder.clone(),
            cfg.embed_batch_max,
            Duration::from_millis(cfg.embed_batch_window_ms),
        )))
    } else {
        None
    };
    let backups = Arc::new(BackupManager::open(&cfg.backup_dir)?);
    let schedule = Arc::new(RwLock::new(ScheduleConfig {
        enabled: cfg.backup_interval_secs > 0,
        interval_secs: cfg.backup_interval_secs,
        full_every: cfg.backup_full_every,
        keep_fulls: cfg.backup_keep_fulls,
    }));
    start_backup_scheduler(handle.clone(), backups.clone(), schedule.clone());

    tracing::info!(
        "search concurrency limit: {} (wait {} ms); embed batching: {}",
        cfg.max_concurrent_searches,
        cfg.search_wait_ms,
        if cfg.embed_batch_max > 1 {
            format!("{} / {} ms", cfg.embed_batch_max, cfg.embed_batch_window_ms)
        } else {
            "off".to_string()
        }
    );
    tracing::info!(
        "backups: dir={}, schedule={}",
        cfg.backup_dir.display(),
        if cfg.backup_interval_secs > 0 {
            format!(
                "every {}s (full every {}, keep {})",
                cfg.backup_interval_secs, cfg.backup_full_every, cfg.backup_keep_fulls
            )
        } else {
            "disabled".to_string()
        }
    );

    let state = AppState {
        engine: handle,
        queue,
        metrics: Arc::new(app::Metrics::default()),
        search_sem: Arc::new(Semaphore::new(cfg.max_concurrent_searches)),
        search_wait: Duration::from_millis(cfg.search_wait_ms),
        batcher,
        backups,
        embedder,
        index_kind: cfg.index_kind,
        data_dir,
        schedule,
        rate_limit_rpm: cfg.rate_limit_rpm,
        passphrase: cfg.passphrase.clone(),
        keyfile: cfg.keyfile.clone(),
    };
    tracing::info!(
        "rate limiting: {}",
        if cfg.rate_limit_rpm > 0 {
            format!("{} req/min per IP", cfg.rate_limit_rpm)
        } else {
            "off".to_string()
        }
    );

    let mut app = routes::router(state);
    if cfg.cors_any {
        app = app.layer(tower_http::cors::CorsLayer::permissive());
    }

    let listener = tokio::net::TcpListener::bind(&cfg.addr).await?;
    tracing::info!("Nucleus listening on http://{}", cfg.addr);
    // `into_make_service_with_connect_info` exposes the peer address to handlers
    // and the rate-limit middleware (which keys on the client IP).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(handle_for_shutdown))
    .await?;
    Ok(())
}

/// Read the active-db pointer written by a prior restore, if it points to an
/// existing file.
fn active_db_path(data_dir: &std::path::Path) -> Option<PathBuf> {
    let ptr = data_dir.join("active_db.txt");
    let raw = std::fs::read_to_string(ptr).ok()?;
    let path = PathBuf::from(raw.trim());
    path.exists().then_some(path)
}

/// Background task taking scheduled backups per the (runtime-adjustable) policy.
fn start_backup_scheduler(
    handle: Arc<EngineHandle>,
    backups: Arc<BackupManager>,
    schedule: Arc<RwLock<ScheduleConfig>>,
) {
    tokio::spawn(async move {
        let mut count: u64 = 0;
        loop {
            let (enabled, interval, full_every, keep) = {
                let s = schedule.read().unwrap();
                (
                    s.enabled,
                    s.interval_secs,
                    s.full_every.max(1),
                    s.keep_fulls,
                )
            };
            if !enabled || interval == 0 {
                // Disabled: re-check periodically (the schedule may be enabled later).
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            tokio::time::sleep(Duration::from_secs(interval)).await;
            if !schedule.read().unwrap().enabled {
                continue;
            }
            count += 1;
            let want_full = full_every <= 1 || count % full_every as u64 == 1;
            let engine = handle.current();
            let mgr = backups.clone();
            let res = tokio::task::spawn_blocking(move || {
                let rec = if want_full {
                    mgr.full(engine.storage())?
                } else {
                    // A differential needs a full; fall back to a full if none yet.
                    match mgr.differential(engine.storage()) {
                        Ok(r) => r,
                        Err(_) => mgr.full(engine.storage())?,
                    }
                };
                let _ = mgr.prune(keep);
                Ok::<_, nucleus_core::NucleusError>(rec)
            })
            .await;
            match res {
                Ok(Ok(rec)) => tracing::info!("scheduled backup {} ({:?})", rec.id, rec.kind),
                Ok(Err(e)) => tracing::error!("scheduled backup failed: {e}"),
                Err(e) => tracing::error!("scheduled backup task panicked: {e}"),
            }
        }
    });
}

/// Wait for Ctrl-C or (on Unix) SIGTERM, then flush persistable indexes to disk.
async fn shutdown_signal(handle: Arc<EngineHandle>) {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }

    tracing::info!("shutdown: persisting indexes…");
    let engine = handle.current();
    match tokio::task::spawn_blocking(move || engine.persist_indexes()).await {
        Ok(Ok(n)) => tracing::info!("persisted {n} index(es)"),
        Ok(Err(e)) => tracing::error!("index persist failed: {e}"),
        Err(e) => tracing::error!("index persist task failed: {e}"),
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}
