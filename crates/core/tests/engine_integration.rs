//! Integration tests for the Nucleus engine, driving the public API end to end.
//!
//! These run against a fresh on-disk database (a `tempfile` directory) and the
//! deterministic [`MockEmbedder`], so they exercise the real ingest → index →
//! search path without loading an ONNX model. Run with:
//!
//!   cargo test -p nucleus-core --test engine_integration

use std::collections::BTreeMap;
use std::sync::Arc;

use nucleus_core::embed::{Embedder, MockEmbedder};
use nucleus_core::engine::{IngestBody, QueryInput, SearchRequest};
use nucleus_core::index::IndexKind;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;
use tempfile::TempDir;

/// Build a fresh engine backed by a throwaway database and the mock embedder.
/// Returns the `TempDir` too so the caller keeps the database alive for the test.
fn fresh_engine() -> (Engine, TempDir) {
    let dir = TempDir::new().expect("create temp dir");
    // Keep the machine key file inside the tempdir so the test stays hermetic.
    let keyfile = dir.path().join("test.key");
    let storage = Storage::open_with_options(dir.path().join("test.redb"), None, Some(&keyfile))
        .expect("open storage");
    let embedder: Arc<dyn Embedder> = Arc::new(MockEmbedder::new(64));
    let engine = Engine::open(storage, embedder, IndexKind::Flat, None).expect("open engine");
    (engine, dir)
}

/// Ingest a document as a single text body and return its outcome.
fn ingest(
    engine: &Engine,
    domain_id: nucleus_core::id::DomainId,
    title: &str,
    text: &str,
) -> nucleus_core::engine::IngestOutcome {
    let mut meta = BTreeMap::new();
    meta.insert("filename".to_string(), title.to_string());
    engine
        .ingest_document(
            domain_id,
            title,
            Some(title.to_string()),
            meta,
            vec![],
            IngestBody::Text(text.to_string()),
        )
        .expect("ingest document")
}

fn text_search(
    engine: &Engine,
    domain_id: nucleus_core::id::DomainId,
    query: &str,
    k: usize,
) -> Vec<nucleus_core::engine::SearchHit> {
    engine
        .search(
            domain_id,
            SearchRequest {
                query: QueryInput::Text(query.to_string()),
                k,
                tags: vec![],
                match_all: false,
                document_ids: vec![],
                subdomain: None,
                filter: None,
                diversity: 0.0,
            },
        )
        .expect("search")
}

#[test]
fn create_and_list_domains() {
    let (engine, _dir) = fresh_engine();

    let a = engine.create_domain("legal", None).expect("create legal");
    let b = engine.create_domain("fiscal", None).expect("create fiscal");
    assert_ne!(a.id, b.id, "domains get distinct ids");

    let domains = engine.list_domains().expect("list domains");
    assert_eq!(domains.len(), 2);

    // The dimension must match the embedder's reported dimension (64 here).
    assert_eq!(a.dim, 64);
}

#[test]
fn ingest_chunks_and_count() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    // A body long enough to span more than one chunk.
    let body = "el contrato laboral define las obligaciones de las partes. ".repeat(40);
    let outcome = ingest(&engine, domain.id, "contrato.txt", &body);

    assert!(outcome.chunk_count >= 1, "produced at least one chunk");
    let docs = engine
        .list_documents(domain.id, 0, 10)
        .expect("list documents");
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].title, "contrato.txt");
}

#[test]
fn search_ranks_relevant_document_first() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    ingest(
        &engine,
        domain.id,
        "laboral.txt",
        "el contrato laboral regula la relación entre empresa y trabajador",
    );
    ingest(
        &engine,
        domain.id,
        "cocina.txt",
        "la receta de la pizza con piña divide opiniones en la cocina",
    );

    let hits = text_search(&engine, domain.id, "contrato laboral trabajador", 5);
    assert!(!hits.is_empty(), "search returns results");

    let top = engine
        .get_document(hits[0].chunk.document_id)
        .expect("resolve top doc");
    assert_eq!(
        top.title, "laboral.txt",
        "the labour-contract document outranks the pizza one"
    );
}

#[test]
fn search_respects_k_limit() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    for i in 0..5 {
        ingest(
            &engine,
            domain.id,
            &format!("doc{i}.txt"),
            &format!("documento numero {i} sobre contratos y clausulas varias"),
        );
    }

    let hits = text_search(&engine, domain.id, "contratos clausulas", 2);
    assert!(hits.len() <= 2, "k bounds the number of hits");
}

#[test]
fn delete_document_removes_it_from_listing() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    let outcome = ingest(
        &engine,
        domain.id,
        "borrame.txt",
        "este documento sera eliminado en breve",
    );
    engine
        .delete_document(outcome.document.id)
        .expect("delete document");

    let docs = engine
        .list_documents(domain.id, 0, 10)
        .expect("list documents");
    assert!(docs.is_empty(), "deleted document no longer listed");
}

#[test]
fn search_is_isolated_per_domain() {
    let (engine, _dir) = fresh_engine();
    let legal = engine.create_domain("legal", None).expect("create legal");
    let fiscal = engine.create_domain("fiscal", None).expect("create fiscal");

    ingest(
        &engine,
        legal.id,
        "ley.txt",
        "texto exclusivo del dominio legal",
    );

    // Searching the empty fiscal domain must not see the legal document.
    let hits = text_search(&engine, fiscal.id, "texto legal", 5);
    assert!(hits.is_empty(), "documents do not leak across domains");

    let hits = text_search(&engine, legal.id, "texto legal", 5);
    assert!(
        !hits.is_empty(),
        "the legal domain still finds its own document"
    );
}
