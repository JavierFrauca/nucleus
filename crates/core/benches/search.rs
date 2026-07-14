//! Benchmarks for search operations.
//!
//! These benchmarks measure search performance with Flat Index and HNSW Index,
//! with and without filters, and with different k values.

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

/// Create a fresh engine backed by a throwaway database and the mock embedder.
fn setup_engine(embed_dim: usize, index_kind: IndexKind) -> (Engine, TempDir, DomainId) {
    let dir = TempDir::new().expect("create temp dir");
    let keyfile = dir.path().join("test.key");
    let storage = Storage::open_with_options(dir.path().join("test.redb"), None, Some(&keyfile))
        .expect("open storage");
    let embedder: Arc<dyn nucleus_core::embed::Embedder> = Arc::new(MockEmbedder::new(embed_dim));
    let engine = Engine::open(storage, embedder, index_kind, None).expect("open engine");
    let domain = engine
        .create_domain("benchmark_domain", None)
        .expect("create domain");
    (engine, dir, domain.id)
}

/// Generate synthetic documents for benchmarking.
fn generate_documents(num_docs: usize, repeat: usize) -> Vec<(String, String)> {
    let mut docs = Vec::with_capacity(num_docs);
    for i in 0..num_docs {
        let title = format!("Document {}", i);
        let content =
            format!("This is document {i} about a specific topic with many words. ").repeat(repeat);
        docs.push((title, content));
    }
    docs
}

/// Ingest documents into the engine.
fn ingest_documents(engine: &Engine, domain_id: DomainId, docs: &[(String, String)]) {
    for (title, content) in docs {
        let meta = BTreeMap::new();
        engine
            .ingest_document(
                domain_id,
                title,
                Some(title.clone()),
                meta,
                vec![],
                IngestBody::Text(content.clone()),
            )
            .expect("ingest document");
    }
}

/// Benchmark search with Flat Index (exact search), no filter.
fn bench_search_flat(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64, IndexKind::Flat);
    let docs = generate_documents(100, 50);
    ingest_documents(&engine, domain_id, &docs);

    let queries = [
        "specific topic words",
        "document about specific",
        "words and content here",
        "test query search benchmark",
        "synthetic data generation",
    ];

    c.bench_function("search_flat_no_filter", |b| {
        b.iter(|| {
            for query in &queries {
                let _ = engine.search(
                    domain_id,
                    SearchRequest {
                        query: QueryInput::Text((*query).to_string()),
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

/// Benchmark search with HNSW Index.
fn bench_search_hnsw(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64, IndexKind::Hnsw);
    let docs = generate_documents(100, 50);
    ingest_documents(&engine, domain_id, &docs);

    c.bench_function("search_hnsw_no_filter", |b| {
        b.iter(|| {
            let _ = engine.search(
                domain_id,
                SearchRequest {
                    query: QueryInput::Text("specific topic words".to_string()),
                    k: 10,
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

/// Benchmark search with different k values.
fn bench_search_k_values(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64, IndexKind::Flat);
    let docs = generate_documents(100, 50);
    ingest_documents(&engine, domain_id, &docs);

    for k in [10usize, 50, 100] {
        c.bench_with_input(BenchmarkId::new("search_flat_k", k), &k, |b, &k_val| {
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

/// Benchmark search with query-language filter.
fn bench_search_filter(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64, IndexKind::Flat);
    let docs = generate_documents(100, 50);
    ingest_documents(&engine, domain_id, &docs);

    c.bench_function("search_flat_filter", |b| {
        b.iter(|| {
            let _ = engine.search(
                domain_id,
                SearchRequest {
                    query: QueryInput::Text("topic words".to_string()),
                    k: 10,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: Some("doc:0".to_string()),
                    diversity: 0.0,
                },
            );
        });
    });
}

/// Benchmark search with diversity penalty.
fn bench_search_diversity(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64, IndexKind::Flat);
    let docs = generate_documents(100, 50);
    ingest_documents(&engine, domain_id, &docs);

    for lambda in [0.0f32, 0.5, 1.0] {
        c.bench_with_input(
            BenchmarkId::new("search_flat_diversity", format!("{:.1}", lambda)),
            &lambda,
            |b, &l| {
                b.iter(|| {
                    let _ = engine.search(
                        domain_id,
                        SearchRequest {
                            query: QueryInput::Text("specific topic words".to_string()),
                            k: 10,
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

/// Benchmark precomputed-vector query (no embedding step).
fn bench_search_vector_query(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64, IndexKind::Flat);
    let docs = generate_documents(100, 50);
    ingest_documents(&engine, domain_id, &docs);

    // A dummy 64-dim query vector (MockEmbedder is bag-of-words, so this is just
    // for exercising the Vector path, not relevance).
    let qv = vec![0.1f32; 64];

    c.bench_function("search_flat_vector_query", |b| {
        b.iter(|| {
            let _ = engine.search(
                domain_id,
                SearchRequest {
                    query: QueryInput::Vector(qv.clone()),
                    k: 10,
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

criterion_group!(
    benches,
    bench_search_flat,
    bench_search_hnsw,
    bench_search_k_values,
    bench_search_filter,
    bench_search_diversity,
    bench_search_vector_query,
);

criterion_main!(benches);
