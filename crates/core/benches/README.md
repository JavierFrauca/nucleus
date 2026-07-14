# Nucleus Benchmarks

Performance benchmarks for `nucleus-core`, built with
[Criterion](https://bheisler.github.io/criterion.rs/book/).

All benchmarks live under `crates/core/benches/` and run against the
deterministic `MockEmbedder` (no ONNX model download) and a throwaway on-disk
database, so they are reproducible on any machine.

## Running

```bash
# All benchmarks
cargo bench -p nucleus-core

# One suite
cargo bench -p nucleus-core --bench search
cargo bench -p nucleus-core --bench ingest
cargo bench -p nucleus-core --bench index
cargo bench -p nucleus-core --bench rerank

# Faster feedback (fewer samples — results noisier)
cargo bench -p nucleus-core -- --sample-size 10

# Detailed statistics + HTML report
cargo bench -p nucleus-core -- --nocapture
# then open target/criterion/report/index.html
```

Criterion writes statistical reports and plots to `target/criterion/`.

## Suites

### `search.rs` — retrieval pipeline

Exercises `Engine::search` end-to-end (embed query → candidate set → dense +
BM25 fusion → MMR diversity) on a flat index.

- `search_flat_no_filter` — baseline hybrid search, k=10
- `search_hnsw_no_filter` — same, with the HNSW backend
- `search_flat_k/{10,50,100}` — cost of returning more results
- `search_flat_filter` — with a query-language `filter`
- `search_flat_diversity/{0.0,0.5,1.0}` — MMR diversity penalty
- `search_flat_vector_query` — precomputed query vector (skips embedding)

### `ingest.rs` — ingestion pipeline

Exercises `Engine::ingest_document` (chunk → embed → index) at various sizes.

- `ingest_single/{100b,1kb,10kb,100kb}` — single document by size
- `ingest_batch/{10,50,100}` — bulk ingest of many docs
- `ingest_chunks_direct/{10,50,100}` — pre-split chunks (bypasses chunking)
- `ingest_duplicate_lookup` — dedup hash lookup (no re-embedding)
- `ingest_throughput/{10,100}` — docs-per-second

### `index.rs` — vector index primitives

Drives the `VectorIndex` trait directly (flat vs HNSW), without the engine or
database, to isolate index cost.

- `flat_construct/{1k,10k,50k}` — bulk upsert of N vectors (flat)
- `hnsw_construct/{1k,10k}` — bulk upsert (HNSW; capped because it is ~1000× slower)
- `flat_search_k/{10,50,100}` and `hnsw_search_k/{10,50,100}` — search cost vs k
- `flat_search_filtered_half` — search with an allowed-set (tag pre-filter)
- `flat_upsert` — single incremental insert latency

### `rerank.rs` — fusion & diversity cost

Measures the cost of the second-stage pipeline (RRF fusion + MMR) at different
result-set sizes and diversity settings.

- `search_k/{10,50,100,200}` — end-to-end search at growing k
- `diversity/{0.00,0.25,0.50,0.75,1.00}` — MMR penalty sweep
- `search_batch_queries` — amortized cost over a batch of queries

## Reference numbers

Collected on the dev machine (Windows x64, release profile, `--sample-size 10`).
These are orientation values, not SLAs — real-world numbers depend on hardware,
model and data distribution. With the production `LocalEmbedder` (ONNX), the
embedding step dominates and absolute numbers are much higher; these isolate the
engine/index cost using `MockEmbedder`.

| Operation | Time (median) |
|-----------|---------------|
| `flat_construct/10000` | ~6.3 ms |
| `hnsw_construct/10000` | ~3.0 s |
| `flat_search_k/10` | ~127 µs |
| `flat_search_k/100` | ~755 µs |
| `hnsw_search_k/10` | ~45 µs |
| `hnsw_search_k/100` | ~247 µs |
| `flat_search_filtered_half` | ~390 µs |
| `flat_upsert` | ~91 ns |
| `search_flat_no_filter` | ~127 µs |
| `search_flat_filter` | ~60 µs |
| `search_flat_diversity/0.0` | ~127 µs |
| `search_flat_diversity/1.0` | ~555 µs |
| `search_flat_vector_query` | ~50 µs |
| `ingest_chunks_direct/10` | ~4.9 ms |
| `ingest_chunks_direct/100` | ~7.1 ms |
| `ingest_duplicate_lookup` | ~4.8 µs |
| `ingest_throughput/100` | ~556 ms (~5.6 ms/doc) |
| `diversity/0.00` | ~192 µs |
| `diversity/1.00` | ~1.9 ms |

## Notes

- Benchmarks use `MockEmbedder` (deterministic, no model download), so they
  measure engine + index cost, **not** ONNX inference.
- Release profile: `lto = "thin"`, `opt-level = 3`.
- HNSW construction is ~3 orders of magnitude slower than flat for the same N;
  flat search scales linearly with k; HNSW search is much flatter.
- The MMR diversity stage roughly quadruples search latency at λ=1.0.

## Adding a benchmark

1. Add a file `crates/core/benches/<name>.rs` using `criterion_group!` /
   `criterion_main!`.
2. Declare it in `crates/core/Cargo.toml`:
   ```toml
   [[bench]]
   name = "<name>"
   harness = false
   ```
3. Run `cargo bench -p nucleus-core --bench <name>`.
