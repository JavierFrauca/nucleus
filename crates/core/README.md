# nucleus-core

The engine behind **Nucleus**, a database specialised for RAG workloads with
first-class **domains** (namespaces) and **tagging**. This crate is
transport-agnostic: it owns storage ([redb]), the vector + lexical indexes, the
in-process embedding provider ([fastembed]/ONNX), the job queue and auth. The
HTTP surface lives in the separate `nucleus-server` crate.

- **In-process embeddings** (default `multilingual-e5-small`, 384-dim).
- **Hybrid retrieval**: dense (cosine, flat or HNSW) + BM25, fused with RRF;
  optional cross-encoder reranking.
- **Transparent ingestion**: extract (pdf/docx/xlsx/html/md/txt) → chunk → embed
  → index, all inside the engine.
- **Embedded & ACID** via redb; values encoded with bincode 2.

## Use as a dependency

```toml
# crates.io (once published)
nucleus-core = "0.1"

# or pin to the git repo
nucleus-core = { git = "https://github.com/your-org/nucleus", tag = "v0.1.0" }

# or a local path (workspaces / monorepos)
nucleus-core = { path = "../nucleus/crates/core" }
```

Enable GPU inference (ONNX DirectML, Windows) with the `gpu` feature:

```toml
nucleus-core = { version = "0.1", features = ["gpu"] }
```

## Quick start

```rust
use std::sync::Arc;
use nucleus_core::{Engine, engine::{IngestBody, QueryInput, SearchRequest}};
use nucleus_core::embed::LocalEmbedder;
use nucleus_core::storage::Storage;

let storage = Storage::open("nucleus.redb")?;
let embedder = Arc::new(LocalEmbedder::new()); // downloads the model on first use
let engine = Engine::new(storage, embedder)?;

let domain = engine.create_domain("docs", None)?;
engine.ingest_document(
    domain.id, "nota", None, Default::default(), vec![],
    IngestBody::Text("el contrato laboral indefinido".into()),
)?;

let hits = engine.search(domain.id, SearchRequest {
    query: QueryInput::Text("contrato".into()),
    k: 5, tags: vec![], match_all: false,
    document_ids: vec![], subdomain: None, filter: None,
})?;
# Ok::<(), nucleus_core::NucleusError>(())
```

See the [workspace README](../../README.md) and [`docs/`](../../docs) for the
full picture (server, API, deployment).

License: MIT OR Apache-2.0.

[redb]: https://www.redb.org/
[fastembed]: https://github.com/Anush008/fastembed-rs
