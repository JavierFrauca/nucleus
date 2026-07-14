//! Benchmarks for index construction and search.
//!
//! These benchmarks exercise the `VectorIndex` trait directly (flat vs HNSW):
//! bulk upsert, search with different k, and the cost of the allowed-set filter.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nucleus_core::id::ChunkId;
use nucleus_core::index::{build_index, IndexKind, VectorIndex};
use std::collections::HashSet;

/// Deterministic synthetic embedding of `dim` dimensions for id `i`.
fn make_vec(i: usize, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    for (d, slot) in v.iter_mut().enumerate() {
        *slot = (((i + 1) * (d + 7)) % 251) as f32 / 251.0;
    }
    v
}

/// Bulk-upsert `count` vectors into a fresh index of `kind`.
fn build_index_with(kind: IndexKind, dim: usize, count: usize) -> Box<dyn VectorIndex> {
    let mut idx = build_index(kind, dim);
    for i in 0..count {
        let v = make_vec(i, dim);
        idx.upsert(ChunkId::from(i as u64), &v).expect("upsert");
    }
    idx
}

/// Benchmark bulk construction (upsert N vectors).
fn bench_index_construction(c: &mut Criterion) {
    let dim = 64;
    for count in [1_000usize, 10_000, 50_000] {
        c.bench_with_input(
            BenchmarkId::new("flat_construct", count),
            &count,
            |b, &n| {
                b.iter(|| build_index_with(IndexKind::Flat, dim, n));
            },
        );
    }
    // HNSW construction is ~3 orders of magnitude slower than flat; cap at 10k
    // so a full bench run stays in minutes, not hours.
    for count in [1_000usize, 10_000] {
        c.bench_with_input(
            BenchmarkId::new("hnsw_construct", count),
            &count,
            |b, &n| {
                b.iter(|| build_index_with(IndexKind::Hnsw, dim, n));
            },
        );
    }
}

/// Benchmark search with different k values (flat).
fn bench_flat_search(c: &mut Criterion) {
    let dim = 64;
    let n = 10_000usize;
    let idx = build_index_with(IndexKind::Flat, dim, n);
    let q = make_vec(42, dim);
    for k in [10usize, 50, 100] {
        c.bench_with_input(BenchmarkId::new("flat_search_k", k), &k, |b, &k_val| {
            b.iter(|| {
                let _ = idx.search(black_box(&q), black_box(k_val), black_box(None));
            });
        });
    }
}

/// Benchmark search with different k values (HNSW).
fn bench_hnsw_search(c: &mut Criterion) {
    let dim = 64;
    let n = 10_000usize;
    let idx = build_index_with(IndexKind::Hnsw, dim, n);
    let q = make_vec(42, dim);
    for k in [10usize, 50, 100] {
        c.bench_with_input(BenchmarkId::new("hnsw_search_k", k), &k, |b, &k_val| {
            b.iter(|| {
                let _ = idx.search(black_box(&q), black_box(k_val), black_box(None));
            });
        });
    }
}

/// Benchmark search with an allowed-set filter (as the engine applies for tags).
fn bench_search_with_allowed(c: &mut Criterion) {
    let dim = 64;
    let n = 10_000usize;
    let idx = build_index_with(IndexKind::Flat, dim, n);
    let q = make_vec(42, dim);
    // Allow ~half of the ids.
    let allowed: HashSet<ChunkId> = (0..n)
        .filter(|i| i % 2 == 0)
        .map(|i| ChunkId::from(i as u64))
        .collect();

    c.bench_function("flat_search_filtered_half", |b| {
        b.iter(|| {
            let _ = idx.search(black_box(&q), black_box(10), black_box(Some(&allowed)));
        });
    });
}

/// Benchmark single upsert latency (incremental insert).
fn bench_upsert(c: &mut Criterion) {
    let dim = 64;
    let n = 10_000usize;
    let mut idx = build_index_with(IndexKind::Flat, dim, n);
    let v = make_vec(n + 1, dim);
    let id = ChunkId::from((n + 1) as u64);

    c.bench_function("flat_upsert", |b| {
        b.iter(|| {
            idx.upsert(black_box(id), black_box(&v)).expect("upsert");
        });
    });
}

criterion_group!(
    benches,
    bench_index_construction,
    bench_flat_search,
    bench_hnsw_search,
    bench_search_with_allowed,
    bench_upsert,
);

criterion_main!(benches);
