//! Benchmarks for reranking-related operations.
//!
//! The reranker is an optional second stage installed via `set_reranker`; when
//! absent, search still does dense+BM25 fusion (RRF) and optional MMR diversity.
//! These benchmarks measure the cost of that pipeline at different k, diversity
//! and candidate volumes.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nucleus_core::embed::MockEmbedder;
use nucleus_core::engine::{IngestBody, QueryInput, SearchRequest};
use nucleus_core::id::DomainId;
use nucleus_core::index::IndexKind;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;

fn setup_engine(embed_dim: usize) -> (Engine, TempDir, DomainId) {
    let dir = TempDir::new().expect("create temp dir");
    let keyfile = dir.path().join("test.key");
    let storage = Storage::open_with_options(dir.path().join("test.redb"), None, Some(&keyfile))
        .expect("open storage");
    let embedder: Arc<dyn nucleus_core::embed::Embedder> = Arc::new(MockEmbedder::new(embed_dim));
    let engine = Engine::open(storage, embedder, IndexKind::Flat, None).expect("open engine");
    let domain = engine
        .create_domain("benchmark_domain", None)
        .expect("create domain");
    (engine, dir, domain.id)
}

fn generate_documents(num_docs: usize, repeat: usize) -> Vec<(String, String)> {
    (0..num_docs)
        .map(|i| {
            let title = format!("Document {i}");
            let content = format!("This is document {i} about a specific topic with many words. ")
                .repeat(repeat);
            (title, content)
        })
        .collect()
}

fn ingest_documents(engine: &Engine, domain_id: DomainId, docs: &[(String, String)]) {
    for (title, content) in docs {
        engine
            .ingest_document(
                domain_id,
                title,
                Some(title.clone()),
                BTreeMap::new(),
                vec![],
                IngestBody::Text(content.clone()),
            )
            .expect("ingest");
    }
}

/// Benchmark search cost at different result-set sizes (k).
fn bench_search_k(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64);
    ingest_documents(&engine, domain_id, &generate_documents(100, 50));

    for k in [10usize, 50, 100, 200] {
        c.bench_with_input(BenchmarkId::new("search_k", k), &k, |b, &k_val| {
            b.iter(|| {
                let _ = engine.search(
                    domain_id,
                    SearchRequest {
                        query: QueryInput::Text("specific topic words".to_string()),
                        k: k_val,
                        tags: vec![],
                        match_all: false,
                        document_ids: vec![],
                        subdomain: None,
                        filter: None,
                        diversity: 0.0,
                    },
                );
            });
        });
    }
}

/// Benchmark the MMR diversity stage at different lambda values.
fn bench_diversity(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64);
    ingest_documents(&engine, domain_id, &generate_documents(100, 50));

    for lambda in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
        c.bench_with_input(
            BenchmarkId::new("diversity", format!("{:.2}", lambda)),
            &lambda,
            |b, &l| {
                b.iter(|| {
                    let _ = engine.search(
                        domain_id,
                        SearchRequest {
                            query: QueryInput::Text("specific topic words".to_string()),
                            k: 20,
                            tags: vec![],
                            match_all: false,
                            document_ids: vec![],
                            subdomain: None,
                            filter: None,
                            diversity: l,
                        },
                    );
                });
            },
        );
    }
}

/// Benchmark a mixed batch of queries (amortized throughput).
fn bench_batch_queries(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64);
    ingest_documents(&engine, domain_id, &generate_documents(100, 50));

    let queries = [
        "specific topic words",
        "search query test",
        "benchmark performance evaluation",
        "data generation synthetic",
        "vector embedding search",
    ];

    c.bench_function("search_batch_queries", |b| {
        b.iter(|| {
            for q in &queries {
                let _ = engine.search(
                    domain_id,
                    SearchRequest {
                        query: QueryInput::Text((*q).to_string()),
                        k: 10,
                        tags: vec![],
                        match_all: false,
                        document_ids: vec![],
                        subdomain: None,
                        filter: None,
                        diversity: 0.0,
                    },
                );
            }
        });
    });
}

criterion_group!(
    benches,
    bench_search_k,
    bench_diversity,
    bench_batch_queries,
);

criterion_main!(benches);
