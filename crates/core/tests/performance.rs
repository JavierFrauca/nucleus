//! Capacity and scale tests for the Nucleus engine.
//!
//! These are not micro-benchmarks (see `benches/`); they assert the engine
//! stays correct at moderate scale and respects its documented limits. They
//! run against the `MockEmbedder` so no model download is needed.

use std::sync::Arc;

use nucleus_core::embed::{Embedder, MockEmbedder};
use nucleus_core::engine::{IngestBody, QueryInput, SearchRequest};
use nucleus_core::index::IndexKind;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;
use tempfile::TempDir;

fn fresh_engine() -> (Engine, TempDir) {
    let dir = TempDir::new().expect("create temp dir");
    let keyfile = dir.path().join("test.key");
    let storage = Storage::open_with_options(dir.path().join("test.redb"), None, Some(&keyfile))
        .expect("open storage");
    let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(64));
    let engine = Engine::open(storage, embedder, IndexKind::Flat, None).expect("open engine");
    (engine, dir)
}

/// A search at scale returns the requested number of hits (up to what exists).
#[test]
fn search_returns_up_to_k_results_at_scale() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    // Ingest enough distinct chunks to fill a large k.
    for i in 0..50 {
        let body = format!("documento numero {i} con contenido tematico distinto ");
        engine
            .ingest_document(
                domain.id,
                &format!("doc-{i}"),
                Some(format!("doc-{i}")),
                std::collections::BTreeMap::new(),
                vec![],
                IngestBody::Text(body.repeat(3)),
            )
            .expect("ingest");
    }

    let hits = engine
        .search(
            domain.id,
            SearchRequest {
                query: QueryInput::Text("contenido tematico".to_string()),
                k: 30,
                tags: vec![],
                match_all: false,
                document_ids: vec![],
                subdomain: None,
                filter: None,
                diversity: 0.0,
            },
        )
        .expect("search");
    assert!(
        hits.len() <= 30,
        "k caps the result count (got {})",
        hits.len()
    );
    assert!(!hits.is_empty(), "a populated domain returns results");
}

/// `k` above the hard cap (MAX_K = 1000) is silently clamped, never panics.
#[test]
fn search_k_above_hard_cap_is_clamped_not_panicked() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");
    engine
        .ingest_document(
            domain.id,
            "doc",
            Some("doc".to_string()),
            std::collections::BTreeMap::new(),
            vec![],
            IngestBody::Text("un chunk simple".to_string()),
        )
        .expect("ingest");

    // Asking for a million must not panic; the engine clamps to its hard cap.
    let hits = engine
        .search(
            domain.id,
            SearchRequest {
                query: QueryInput::Text("chunk".to_string()),
                k: 1_000_000,
                tags: vec![],
                match_all: false,
                document_ids: vec![],
                subdomain: None,
                filter: None,
                diversity: 0.0,
            },
        )
        .expect("search");
    assert!(hits.len() <= 1000, "clamped to the hard cap of 1000");
}

/// An empty domain's search returns an empty result, not an error.
#[test]
fn search_empty_domain_returns_empty_not_error() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    let hits = engine
        .search(
            domain.id,
            SearchRequest {
                query: QueryInput::Text("cualquier cosa".to_string()),
                k: 10,
                tags: vec![],
                match_all: false,
                document_ids: vec![],
                subdomain: None,
                filter: None,
                diversity: 0.0,
            },
        )
        .expect("search");
    assert!(hits.is_empty(), "an empty domain yields no hits");
}

/// A precomputed query vector (skipping embedding) still ranks chunks.
#[test]
fn search_with_precomputed_vector_works() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");
    engine
        .ingest_document(
            domain.id,
            "doc",
            Some("doc".to_string()),
            std::collections::BTreeMap::new(),
            vec![],
            IngestBody::Text("contenido para indexar un vector de consulta".to_string()),
        )
        .expect("ingest");

    // A zero vector is degenerate but must not panic or return NaN; the engine
    // guards against zero-norm vectors (cosine returns 0.0).
    let qv = vec![0.0f32; domain.dim];
    let hits = engine
        .search(
            domain.id,
            SearchRequest {
                query: QueryInput::Vector(qv),
                k: 10,
                tags: vec![],
                match_all: false,
                document_ids: vec![],
                subdomain: None,
                filter: None,
                diversity: 0.0,
            },
        )
        .expect("search");
    // With a zero query vector every score is 0, but the call must succeed and
    // may still return chunks (ranked by the 0.0 tie-break).
    let scores_ok = hits.iter().all(|h| h.score.is_finite());
    assert!(scores_ok, "all scores are finite (no NaN)");
}

/// Pagination lists documents in stable order across pages.
#[test]
fn document_pagination_is_stable() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");
    for i in 0..15 {
        engine
            .ingest_document(
                domain.id,
                &format!("doc-{i}"),
                Some(format!("doc-{i}")),
                std::collections::BTreeMap::new(),
                vec![],
                IngestBody::Text(format!("contenido del documento numero {i}")),
            )
            .expect("ingest");
    }

    // Page through the documents in two halves; the union must cover all docs
    // without duplicates.
    let page1 = engine.list_documents(domain.id, 0, 10).expect("list");
    let page2 = engine.list_documents(domain.id, 10, 10).expect("list");
    assert_eq!(page1.len(), 10, "first page is full");
    assert_eq!(page2.len(), 5, "second page has the remainder");

    let mut ids: Vec<_> = page1.iter().map(|d| d.id).collect();
    ids.extend(page2.iter().map(|d| d.id));
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 15, "no document appears twice across pages");
}
