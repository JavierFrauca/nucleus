//! Benchmarks for document ingestion operations.
//!
//! These benchmarks measure ingestion performance with different chunk sizes
//! and document sizes.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nucleus_core::embed::MockEmbedder;
use nucleus_core::engine::IngestBody;
use nucleus_core::id::DomainId;
use nucleus_core::index::IndexKind;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;

/// Create a fresh engine backed by a throwaway database and the mock embedder.
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

/// Generate a synthetic document of approximately `chars` characters.
fn generate_document(chars: usize) -> String {
    let unit = "This is a synthetic document about a specific topic. ";
    let mut s = String::with_capacity(chars);
    while s.len() < chars {
        s.push_str(unit);
    }
    s
}

/// Benchmark ingestion of single documents with different sizes.
fn bench_single_ingest(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64);
    let sizes = [
        (100, "100b"),
        (1_000, "1kb"),
        (10_000, "10kb"),
        (100_000, "100kb"),
    ];

    for (size, label) in sizes {
        let content = generate_document(size);
        c.bench_with_input(
            BenchmarkId::new("ingest_single", label),
            &(content.clone()),
            |b, content| {
                b.iter(|| {
                    engine
                        .ingest_document(
                            domain_id,
                            "doc",
                            Some("doc".to_string()),
                            BTreeMap::new(),
                            vec![],
                            IngestBody::Text(content.clone()),
                        )
                        .expect("ingest");
                });
            },
        );
    }
}

/// Benchmark ingestion of multiple documents (batch).
fn bench_batch_ingest(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64);
    for batch in [10usize, 50, 100] {
        c.bench_with_input(BenchmarkId::new("ingest_batch", batch), &batch, |b, &n| {
            let content = generate_document(1_000);
            b.iter(|| {
                for i in 0..n {
                    engine
                        .ingest_document(
                            domain_id,
                            &format!("doc-{i}"),
                            Some(format!("doc-{i}")),
                            BTreeMap::new(),
                            vec![],
                            IngestBody::Text(content.clone()),
                        )
                        .expect("ingest");
                }
            });
        });
    }
}

/// Benchmark ingestion of pre-split chunks (bypasses chunking).
fn bench_ingest_chunks_direct(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64);
    for n in [10usize, 50, 100] {
        c.bench_with_input(BenchmarkId::new("ingest_chunks_direct", n), &n, |b, &n| {
            b.iter(|| {
                let chunks: Vec<String> = (0..n)
                    .map(|i| format!("Chunk {i} content for the benchmark."))
                    .collect();
                engine
                    .ingest_document(
                        domain_id,
                        "direct-chunks",
                        Some("direct-chunks".to_string()),
                        BTreeMap::new(),
                        vec![],
                        IngestBody::Chunks(chunks),
                    )
                    .expect("ingest");
            });
        });
    }
}

/// Benchmark duplicate detection: a second ingest of identical content must
/// be cheap (hash lookup, no re-embedding).
fn bench_duplicate_detection(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64);
    let content = generate_document(5_000);
    // Seed the store with the original document.
    engine
        .ingest_document(
            domain_id,
            "original",
            Some("original".to_string()),
            BTreeMap::new(),
            vec![],
            IngestBody::Text(content.clone()),
        )
        .expect("seed ingest");

    c.bench_function("ingest_duplicate_lookup", |b| {
        b.iter(|| {
            // The lookup-by-hash path the HTTP layer uses before ingesting.
            let hash = nucleus_core::util::sha256_hex(content.as_bytes());
            let _ = engine.find_document_by_hash(domain_id, &hash);
        });
    });
}

/// Benchmark ingestion throughput (docs per second).
fn bench_ingest_throughput(c: &mut Criterion) {
    let (engine, _dir, domain_id) = setup_engine(64);
    for n in [10usize, 100] {
        c.bench_with_input(BenchmarkId::new("ingest_throughput", n), &n, |b, &n| {
            let content = generate_document(10_000);
            b.iter(|| {
                for i in 0..n {
                    engine
                        .ingest_document(
                            domain_id,
                            &format!("tp-{i}"),
                            Some(format!("tp-{i}")),
                            BTreeMap::new(),
                            vec![],
                            IngestBody::Text(content.clone()),
                        )
                        .expect("ingest");
                }
            });
        });
    }
}

criterion_group!(
    benches,
    bench_single_ingest,
    bench_batch_ingest,
    bench_ingest_chunks_direct,
    bench_duplicate_detection,
    bench_ingest_throughput,
);

criterion_main!(benches);
