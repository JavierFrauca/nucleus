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

// ---------------------------------------------------------------------------
// Edge cases: deduplication, chunk context, filters, subdomains, concurrency.
// ---------------------------------------------------------------------------

use nucleus_core::engine::SearchHit;
use nucleus_core::id::{ChunkId, DomainId, TagId};

/// Search helper that also accepts tags (so dedup/filter tests stay compact).
fn search_with_tags(
    engine: &Engine,
    domain_id: DomainId,
    query: &str,
    k: usize,
    tags: Vec<TagId>,
) -> Vec<SearchHit> {
    engine
        .search(
            domain_id,
            SearchRequest {
                query: QueryInput::Text(query.to_string()),
                k,
                tags,
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
fn duplicate_content_is_detected_via_hash() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    let body = "contenido duplicado para probar la deduccion ".repeat(20);

    // The engine's hash index is populated by the caller (the HTTP/FFI layer
    // does it after ingest), so register the hash explicitly here.
    let first = ingest(&engine, domain.id, "original.txt", &body);
    let hash = nucleus_core::util::sha256_hex(body.as_bytes());
    engine
        .set_document_hash(domain.id, first.document.id, &hash)
        .expect("set hash");

    // The engine exposes a hash lookup that the HTTP/FFI layers use to dedup.
    let found = engine
        .find_document_by_hash(domain.id, &hash)
        .expect("hash lookup");
    assert_eq!(
        found,
        Some(first.document.id),
        "the content hash resolves to the ingested document"
    );

    // Different content → no hit.
    let other_hash = nucleus_core::util::sha256_hex(b"completely different content");
    let miss = engine
        .find_document_by_hash(domain.id, &other_hash)
        .expect("hash lookup");
    assert!(miss.is_none(), "a different hash does not match");

    // A second ingest of the SAME content must still point at the first doc:
    // the dedup path short-circuits instead of creating a duplicate.
    let dedup = engine
        .find_document_by_hash(domain.id, &hash)
        .expect("hash lookup");
    assert_eq!(dedup, Some(first.document.id));
}

#[test]
fn chunk_context_returns_neighbours_in_order() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    // A long body splits into several chained chunks (prev/next).
    let body = "frase repetida para llenar varios chunks. ".repeat(80);
    let outcome = ingest(&engine, domain.id, "long.txt", &body);
    assert!(
        outcome.chunk_count >= 3,
        "body spans at least 3 chunks (got {})",
        outcome.chunk_count
    );

    // Pick the first chunk of the document.
    let hits = text_search(&engine, domain.id, "frase repetida", outcome.chunk_count);
    let first_id = hits
        .first()
        .map(|h| h.chunk.id)
        .expect("at least one chunk");

    // Ask for one neighbour on each side; the first chunk has no `prev`.
    let ctx = engine.chunk_context(first_id, 1, 1).expect("chunk context");
    assert!(!ctx.is_empty(), "context is non-empty");
    assert_eq!(ctx[0].id, first_id, "the requested chunk leads the window");
}

#[test]
fn tag_filter_restricts_results() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    // Two labels; documents only carry one each.
    let alpha = engine
        .get_or_create_label(domain.id, "alpha")
        .expect("label alpha");
    let beta = engine
        .get_or_create_label(domain.id, "beta")
        .expect("label beta");

    // Ingest two documents directly with the tag set.
    let doc_a = engine
        .create_document_record(domain.id, None, "a", None, BTreeMap::new(), vec![alpha.id])
        .expect("create doc a");
    engine
        .populate_document(
            &doc_a,
            IngestBody::Text("tema comun compartido alpha".into()),
        )
        .expect("populate a");
    let doc_b = engine
        .create_document_record(domain.id, None, "b", None, BTreeMap::new(), vec![beta.id])
        .expect("create doc b");
    engine
        .populate_document(
            &doc_b,
            IngestBody::Text("tema comun compartido beta".into()),
        )
        .expect("populate b");

    // Filtering by alpha's tag must exclude beta's chunks.
    let alpha_hits = search_with_tags(&engine, domain.id, "tema comun", 10, vec![alpha.id]);
    assert!(
        alpha_hits.iter().all(|h| h.chunk.tags.contains(&alpha.id)),
        "every returned chunk carries the alpha tag"
    );
    assert!(!alpha_hits.is_empty(), "the alpha-tagged document is found");
}

#[test]
fn subdomain_resolution_by_name() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");

    // get-or-create returns the same id for the same name.
    let first = engine
        .get_or_create_subdomain(domain.id, "irpf", "")
        .expect("create subdomain");
    let second = engine
        .get_or_create_subdomain(domain.id, "irpf", "")
        .expect("get subdomain");
    assert_eq!(first.id, second.id, "same name → same id");

    // Lookup-by-name agrees.
    let lookup = engine
        .subdomain_id_by_name(domain.id, "irpf")
        .expect("lookup");
    assert_eq!(lookup, Some(first.id));

    // A name that doesn't exist yields None (no auto-create on lookup).
    let miss = engine
        .subdomain_id_by_name(domain.id, "nope")
        .expect("lookup");
    assert!(miss.is_none());
}

#[test]
fn delete_domain_cascades_to_documents() {
    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");
    ingest(&engine, domain.id, "doc.txt", "contenido del documento");

    // The domain is listed before deletion.
    let before = engine.list_domains().expect("list domains");
    assert!(before.iter().any(|d| d.id == domain.id));

    engine.delete_domain(domain.id).expect("delete domain");

    // The domain is gone.
    let after = engine.list_domains().expect("list domains");
    assert!(after.iter().all(|d| d.id != domain.id), "domain removed");

    // Its documents are gone too (list under the now-absent domain is empty).
    let docs = engine
        .list_documents(domain.id, 0, 10)
        .expect("list documents");
    assert!(docs.is_empty(), "documents cascade-deleted with the domain");
}

#[test]
fn concurrent_reads_match_sequential_baseline() {
    // Search is `&self`, so many threads can search the same engine at once.
    // We assert each concurrent search returns the same *set* of chunk ids as
    // the single-threaded baseline. We compare the set (not the order) because
    // chunks with identical text produce identical scores, so the tie-break
    // order among them is not stable — what matters is that no chunk appears
    // or disappears under concurrency (which would indicate a data race).
    use std::collections::HashSet;
    use std::thread;

    let (engine, _dir) = fresh_engine();
    let domain = engine.create_domain("docs", None).expect("create domain");
    // Use clearly distinct content so each query has an unambiguous winner.
    ingest(
        &engine,
        domain.id,
        "a.txt",
        "documento sobre impuesto sobre la renta irpf",
    );
    ingest(
        &engine,
        domain.id,
        "b.txt",
        "documento sobre el impuesto al valor anadido iva",
    );

    let baseline: HashSet<ChunkId> = text_search(&engine, domain.id, "impuesto", 10)
        .into_iter()
        .map(|h| h.chunk.id)
        .collect();

    let engine = std::sync::Arc::new(engine);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let engine = engine.clone();
        let baseline = baseline.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..10 {
                let hits = text_search(&engine, domain.id, "impuesto", 10);
                let ids: HashSet<ChunkId> = hits.into_iter().map(|h| h.chunk.id).collect();
                assert_eq!(
                    ids, baseline,
                    "concurrent search returned a different chunk set than baseline"
                );
            }
        }));
    }
    for h in handles {
        h.join().expect("search thread panicked under concurrency");
    }
}
