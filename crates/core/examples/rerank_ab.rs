//! A/B benchmark of hybrid search vs. hybrid + cross-encoder reranking, run
//! in-process against an existing Nucleus database (the fiscal demo corpus).
//!
//! It isolates the rerank stage: same loaded indexes, same queries, only the
//! reranker (and its candidate window) changes. Reports a latency↔quality sweep
//! over the rerank candidate cap, plus a per-query A/B at the proposed default.
//!
//! Run (PowerShell):
//!   $env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"; $env:CARGO_INCREMENTAL="0"
//!   cargo run --release -p nucleus-core --example rerank_ab          # CPU
//!   cargo run --release -p nucleus-core --example rerank_ab --features gpu

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use nucleus_core::embed::{Embedder, LocalEmbedder};
use nucleus_core::engine::{QueryInput, SearchHit, SearchRequest};
use nucleus_core::id::DomainId;
use nucleus_core::index::IndexKind;
use nucleus_core::rerank::LocalReranker;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;

const DB: &str = r"C:\tmp\nucleus_demo\fiscal_v2.redb";
const MODEL_CACHE: &str = r"C:\tmp\nucleus_models";
const RERANK_MODEL: &str = "bge-reranker-base";
const K: usize = 5;
const PROPOSED_DEFAULT: usize = 20;

fn queries() -> Vec<(u64, &'static str)> {
    vec![
        (1, "tipos de retención de IRPF en 2026"),
        (1, "ayudas en el IRPF por los daños de la DANA"),
        (1, "plazos del calendario del contribuyente 2025"),
        (1, "real decreto-ley aprobado en 2026"),
        (1, "deducción por maternidad en el IRPF"),
        (1, "tipos de IVA aplicables"),
    ]
}

fn req(text: &str, k: usize) -> SearchRequest {
    SearchRequest {
        query: QueryInput::Text(text.to_string()),
        k,
        tags: vec![],
        match_all: false,
        document_ids: vec![],
        subdomain: None,
        filter: None,
    }
}

fn fname(hit: &SearchHit) -> String {
    hit.chunk
        .metadata
        .get("filename")
        .cloned()
        .unwrap_or_else(|| format!("doc {}", hit.chunk.document_id))
}

fn snippet(hit: &SearchHit, n: usize) -> String {
    let s: String = hit
        .chunk
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    s.chars().take(n).collect()
}

fn ids(hits: &[SearchHit]) -> Vec<u64> {
    hits.iter().map(|h| h.chunk.id.get()).collect()
}

fn run_all(engine: &Engine, k: usize) -> (Vec<Vec<SearchHit>>, f64) {
    let mut out = Vec::new();
    let mut ms = 0f64;
    for (dom, q) in queries() {
        let t = Instant::now();
        let hits = engine.search(DomainId::new(dom), req(q, k)).unwrap();
        ms += t.elapsed().as_secs_f64() * 1000.0;
        out.push(hits);
    }
    (out, ms / queries().len() as f64)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let gpu = cfg!(feature = "gpu");
    let storage = Storage::open(PathBuf::from(DB))?;
    let embedder: Arc<dyn Embedder> = Arc::new(LocalEmbedder::with_options(
        Some(PathBuf::from(MODEL_CACHE)),
        gpu,
    ));
    let engine = Engine::open(storage, embedder, IndexKind::Flat, None)?;

    println!("GPU: {}", if gpu { "sí (DirectML)" } else { "no (CPU)" });
    // Warm up the embedder (first call loads the e5 model from cache).
    let _ = engine.search(DomainId::new(1), req("calentamiento", 1))?;

    // --- Baseline: hybrid only -------------------------------------------
    let (baseline, base_ms) = run_all(&engine, K);

    // --- Enable reranking and warm up the cross-encoder ------------------
    println!("\ncargando reranker '{RERANK_MODEL}' (descarga la 1ª vez)…");
    let t_load = Instant::now();
    engine.set_reranker(Arc::new(LocalReranker::with_options(
        RERANK_MODEL,
        Some(PathBuf::from(MODEL_CACHE)),
        gpu,
    )));
    let _ = engine.search(DomainId::new(1), req("calentamiento del reranker", K))?;
    println!("reranker listo en {:.1}s\n", t_load.elapsed().as_secs_f64());

    // --- Sweep the rerank candidate cap ----------------------------------
    // cap=50 (== fetch for k=5) is the "gold" full reranking we compare to.
    let caps = [50usize, 20, 10, 5];
    let mut results: BTreeMap<usize, (Vec<Vec<SearchHit>>, f64)> = BTreeMap::new();
    for &cap in &caps {
        engine.set_rerank_candidates(cap);
        results.insert(cap, run_all(&engine, K));
    }
    let gold = &results[&50].0;

    println!("{}", "=".repeat(72));
    println!(
        "SWEEP latencia↔calidad (k={K}, 6 queries, {})",
        if gpu { "GPU" } else { "CPU" }
    );
    println!("{}", "-".repeat(72));
    println!(
        "{:>5} | {:>12} | {:>14} | {:>16}",
        "cap", "ms/búsqueda", "top-1 == gold", "top-3 ∩ gold (med)"
    );
    println!("{}", "-".repeat(72));
    println!(
        "{:>5} | {:>12.0} | {:>14} | {:>16}",
        "—", base_ms, "(híbrido)", "—"
    );
    for &cap in &caps {
        let (hits, ms) = &results[&cap];
        let mut top1 = 0usize;
        let mut overlap_sum = 0f64;
        for (qi, h) in hits.iter().enumerate() {
            let g = &gold[qi];
            if h.first().map(|x| x.chunk.id.get()) == g.first().map(|x| x.chunk.id.get()) {
                top1 += 1;
            }
            let a: HashSet<u64> = ids(h).into_iter().take(3).collect();
            let b: HashSet<u64> = ids(g).into_iter().take(3).collect();
            overlap_sum += a.intersection(&b).count() as f64;
        }
        let n = hits.len() as f64;
        println!(
            "{:>5} | {:>12.0} | {:>11}/{} | {:>14.1}/3",
            cap,
            ms,
            top1,
            hits.len(),
            overlap_sum / n
        );
    }

    // --- Per-query A/B at the proposed default cap -----------------------
    let r = &results[&PROPOSED_DEFAULT].0;
    println!("\n{}", "=".repeat(72));
    println!("DETALLE por query: híbrido vs reranking (cap={PROPOSED_DEFAULT})");
    for (qi, (_dom, q)) in queries().iter().enumerate() {
        let b = &baseline[qi];
        let base_top = ids(b);
        println!("\n{}", "-".repeat(72));
        println!("QUERY: {q}");
        println!("  -- híbrido --");
        for (rank, h) in b.iter().enumerate() {
            println!(
                "   {}. chunk={:<6} [{}]  {}",
                rank + 1,
                h.chunk.id.get(),
                fname(h),
                snippet(h, 90)
            );
        }
        println!("  -- + reranking --");
        for (rank, h) in r[qi].iter().enumerate() {
            let was = base_top.iter().position(|c| *c == h.chunk.id.get());
            let mark = match was {
                Some(p) if p == rank => "  =".to_string(),
                Some(p) => format!(
                    " {}{}",
                    if p > rank { "↑" } else { "↓" },
                    (p as i64 - rank as i64).abs()
                ),
                None => "NEW".to_string(),
            };
            println!(
                "   {}.{} chunk={:<6} [{}]  {}",
                rank + 1,
                mark,
                h.chunk.id.get(),
                fname(h),
                snippet(h, 90)
            );
        }
    }
    Ok(())
}
