//! `nucleus-ffi` — a thin C ABI over [`nucleus_core::Engine`] so Nucleus can be
//! embedded **in-process** as a native library (`nucleus.dll` on Windows) and
//! referenced from C#, C++ or any language with a C FFI. There is no HTTP, no
//! sidecar: the caller links the DLL and calls the engine directly.
//!
//! ## Shape of the API
//!
//! The engine lives behind an opaque handle (`*mut Engine`). Data-carrying calls
//! take a **JSON** input string and write a **JSON** output string — this keeps
//! the ABI boundary tiny (no struct marshalling) and lets us evolve the payloads
//! without breaking the linkage. Every call returns a status code:
//!
//! - `0`  — success; `*out_json` holds the result (caller frees with
//!   [`nucleus_string_free`]).
//! - `<0` — failure; `*out_json` holds `{"error": "..."}` and the same message is
//!   retrievable via [`nucleus_last_error`].
//!
//! ## Memory & threading
//!
//! Every string returned through an out-parameter is owned by the caller and must
//! be released with [`nucleus_string_free`]. The handle from [`nucleus_open`] must
//! be released with [`nucleus_close`]. The engine is `Send + Sync`; a handle may
//! be shared across threads (each method takes `&self`).
//!
//! The engine path used here is fully **synchronous** (no tokio runtime, no job
//! queue): ingest chunks/embeds/persists inline on the calling thread. That is the
//! right model for an embedded library — the host owns its own threading.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use nucleus_core::embed::{Embedder, LocalEmbedder};
use nucleus_core::engine::{IngestBody, QueryInput, SearchRequest};
use nucleus_core::id::{ChunkId, DocumentId, DomainId};
use nucleus_core::index::IndexKind;
use nucleus_core::storage::Storage;
use nucleus_core::Engine;

// ---------------------------------------------------------------------------
// Status codes
// ---------------------------------------------------------------------------

const NUCLEUS_OK: i32 = 0;
const NUCLEUS_ERR_NULL_ARG: i32 = -1;
const NUCLEUS_ERR_UTF8: i32 = -2;
const NUCLEUS_ERR_JSON: i32 = -3;
const NUCLEUS_ERR_ENGINE: i32 = -4;

thread_local! {
    /// Last error message on the current thread, for [`nucleus_last_error`].
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_last_error(msg: &str) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new(msg).ok();
    });
}

// ---------------------------------------------------------------------------
// Small FFI helpers
// ---------------------------------------------------------------------------

/// Borrow a `*const c_char` as `&str`, or `None` for a null/invalid-UTF8 pointer.
///
/// # Safety
/// `ptr` must be null or a valid NUL-terminated C string for the call's duration.
unsafe fn cstr<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    CStr::from_ptr(ptr).to_str().ok()
}

/// Move a Rust `String` into a freshly-allocated C string the caller must free
/// with [`nucleus_string_free`].
fn to_c_string(s: String) -> *mut c_char {
    // NUL bytes can't occur in our JSON, but guard anyway rather than panic.
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Write a successful JSON result to `*out_json` and return [`NUCLEUS_OK`].
fn ok_json(out_json: *mut *mut c_char, value: serde_json::Value) -> i32 {
    if !out_json.is_null() {
        unsafe { *out_json = to_c_string(value.to_string()) };
    }
    NUCLEUS_OK
}

/// Record an error, write `{"error": msg}` to `*out_json`, and return `code`.
fn fail(out_json: *mut *mut c_char, code: i32, msg: String) -> i32 {
    set_last_error(&msg);
    if !out_json.is_null() {
        unsafe { *out_json = to_c_string(json!({ "error": msg }).to_string()) };
    }
    code
}

/// Resolve an opaque handle to a shared `&Engine` reference.
///
/// # Safety
/// `handle` must come from [`nucleus_open`] and not yet be closed.
unsafe fn engine<'a>(handle: *mut Engine) -> Option<&'a Engine> {
    handle.as_ref()
}

// ---------------------------------------------------------------------------
// open / close
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OpenConfig {
    /// Path to the single redb database file (created if absent). When omitted or
    /// empty, defaults to a per-user location ([`default_db_path`]) — embedding a
    /// library, each OS user gets their own database.
    #[serde(default)]
    db_path: Option<String>,
    /// Where fastembed/ONNX models are cached/downloaded. Optional.
    #[serde(default)]
    model_cache: Option<String>,
    /// Directory for persisted (HNSW) index dumps. Optional; omitted = no on-disk
    /// index persistence (flat is rebuilt from storage on open anyway).
    #[serde(default)]
    index_dir: Option<String>,
    /// `"flat"` (default, exact) or `"hnsw"` (approximate).
    #[serde(default)]
    index_kind: Option<String>,
    /// Run embeddings on the GPU (requires the crate's `gpu` feature + a driver).
    #[serde(default)]
    gpu: bool,
}

/// Per-user default database path used when `db_path` is omitted. Embedded in a
/// desktop app, each OS user should get their own store, so we use the user data
/// dir rather than a machine-wide one: `%LOCALAPPDATA%\Nucleus\nucleus.redb` on
/// Windows, `$XDG_DATA_HOME` (or `~/.local/share`)`/nucleus/nucleus.redb` elsewhere.
fn default_db_path() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    #[cfg(not(windows))]
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".local/share"))
            .unwrap_or_else(|| PathBuf::from("."))
    });
    base.join("Nucleus").join("nucleus.redb")
}

/// Open (or create) a Nucleus database and return an engine handle.
///
/// `config_json` example:
/// `{"db_path":"data/nucleus.redb","model_cache":"models","index_kind":"flat"}`
///
/// # Safety
/// `config_json` is a valid C string; `out_handle` is a valid pointer to write to.
#[no_mangle]
pub unsafe extern "C" fn nucleus_open(
    config_json: *const c_char,
    out_handle: *mut *mut Engine,
) -> i32 {
    if out_handle.is_null() {
        return fail(ptr::null_mut(), NUCLEUS_ERR_NULL_ARG, "out_handle is null".into());
    }
    *out_handle = ptr::null_mut();

    let Some(cfg_str) = cstr(config_json) else {
        return fail(ptr::null_mut(), NUCLEUS_ERR_UTF8, "config_json is null or not UTF-8".into());
    };
    let cfg: OpenConfig = match serde_json::from_str(cfg_str) {
        Ok(c) => c,
        Err(e) => return fail(ptr::null_mut(), NUCLEUS_ERR_JSON, format!("invalid config JSON: {e}")),
    };

    let index_kind = match cfg.index_kind.as_deref() {
        None | Some("flat") => IndexKind::Flat,
        Some("hnsw") => IndexKind::Hnsw,
        Some(other) => {
            return fail(
                ptr::null_mut(),
                NUCLEUS_ERR_JSON,
                format!("unknown index_kind '{other}' (expected 'flat' or 'hnsw')"),
            )
        }
    };

    // Resolve the database path (per-user default if unset) and make sure its
    // parent — and the optional index dir — exist before opening.
    let db_path = match cfg.db_path.as_deref().filter(|s| !s.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => default_db_path(),
    };
    if let Some(parent) = db_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return fail(ptr::null_mut(), NUCLEUS_ERR_ENGINE, format!("create data dir: {e}"));
        }
    }
    if let Some(dir) = &cfg.index_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return fail(ptr::null_mut(), NUCLEUS_ERR_ENGINE, format!("create index dir: {e}"));
        }
    }

    let storage = match Storage::open(&db_path) {
        Ok(s) => s,
        Err(e) => return fail(ptr::null_mut(), NUCLEUS_ERR_ENGINE, format!("open storage: {e}")),
    };
    let embedder: Arc<dyn Embedder> =
        Arc::new(LocalEmbedder::with_options(cfg.model_cache.map(PathBuf::from), cfg.gpu));
    let engine = match Engine::open(storage, embedder, index_kind, cfg.index_dir.map(PathBuf::from)) {
        Ok(e) => e,
        Err(e) => return fail(ptr::null_mut(), NUCLEUS_ERR_ENGINE, format!("open engine: {e}")),
    };

    *out_handle = Box::into_raw(Box::new(engine));
    NUCLEUS_OK
}

/// Close a handle returned by [`nucleus_open`]. Safe to call with null.
///
/// # Safety
/// `handle` must come from [`nucleus_open`] and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn nucleus_close(handle: *mut Engine) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// Free a string returned through any out-parameter. Safe to call with null.
///
/// # Safety
/// `s` must have come from this library and not be freed twice.
#[no_mangle]
pub unsafe extern "C" fn nucleus_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// The last error message on the calling thread, or null if none. The pointer is
/// valid until the next FFI call on this thread — copy it if you need to keep it.
#[no_mangle]
pub extern "C" fn nucleus_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| match &*slot.borrow() {
        Some(c) => c.as_ptr(),
        None => ptr::null(),
    })
}

// ---------------------------------------------------------------------------
// domains
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateDomainInput {
    name: String,
    /// Embedding model id; defaults to the engine's multilingual model if omitted.
    #[serde(default)]
    model: Option<String>,
}

/// Create a domain (namespace). Input: `{"name":"legal","model":null}`.
/// Output: the full `Domain` object as JSON.
///
/// # Safety
/// See module docs: `handle` is live, strings are valid C strings.
#[no_mangle]
pub unsafe extern "C" fn nucleus_create_domain(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: CreateDomainInput = match parse(input_json) {
        Ok(v) => v,
        Err(code_msg) => return fail(out_json, code_msg.0, code_msg.1),
    };
    match eng.create_domain(&input.name, input.model.as_deref()) {
        Ok(domain) => ok_json(out_json, json!(domain)),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// ingest
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct IngestInput {
    domain_id: u64,
    title: String,
    /// Raw text; the engine chunks, embeds and indexes it inline (blocking).
    text: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    /// Label names; resolved (get-or-create) to tag ids within the domain.
    #[serde(default)]
    labels: Vec<String>,
    /// Subdomain name; resolved (get-or-create) within the domain.
    #[serde(default)]
    subdomain: Option<String>,
}

/// Ingest one document synchronously (chunk → embed → persist → index).
/// Output: `{"document_id":N,"chunk_count":M}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_ingest_text(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: IngestInput = match parse(input_json) {
        Ok(v) => v,
        Err(code_msg) => return fail(out_json, code_msg.0, code_msg.1),
    };
    let domain_id = DomainId::from(input.domain_id);

    // Resolve subdomain + labels by name (idempotent get-or-create).
    let subdomain_id = match &input.subdomain {
        Some(name) => match eng.get_or_create_subdomain(domain_id, name, "") {
            Ok(s) => Some(s.id),
            Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
        },
        None => None,
    };
    let mut tags = Vec::with_capacity(input.labels.len());
    for label in &input.labels {
        match eng.get_or_create_label(domain_id, label) {
            Ok(t) => tags.push(t.id),
            Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
        }
    }

    let doc = match eng.create_document_record(
        domain_id,
        subdomain_id,
        &input.title,
        input.source,
        input.metadata,
        tags,
    ) {
        Ok(d) => d,
        Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    };
    match eng.populate_document(&doc, IngestBody::Text(input.text)) {
        Ok(chunk_count) => {
            ok_json(out_json, json!({ "document_id": doc.id, "chunk_count": chunk_count }))
        }
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchInput {
    domain_id: u64,
    /// Query text; embedded with the domain's model (full hybrid + optional rerank).
    query: String,
    #[serde(default = "default_k")]
    k: usize,
    /// Restrict to chunks carrying these labels (resolved by name).
    #[serde(default)]
    labels: Vec<String>,
    /// Require all labels (true) vs any (false).
    #[serde(default)]
    match_all: bool,
    #[serde(default)]
    document_ids: Vec<u64>,
    #[serde(default)]
    subdomain: Option<String>,
    /// Optional query-language filter (`tag:` / `meta.*` / AND·OR·NOT).
    #[serde(default)]
    filter: Option<String>,
}

fn default_k() -> usize {
    10
}

#[derive(Serialize)]
struct HitOut {
    chunk: nucleus_core::model::Chunk,
    score: f32,
}

/// Search a domain by text. Output: `{"hits":[{"chunk":{...},"score":0.87}, ...]}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_search(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: SearchInput = match parse(input_json) {
        Ok(v) => v,
        Err(code_msg) => return fail(out_json, code_msg.0, code_msg.1),
    };
    let domain_id = DomainId::from(input.domain_id);

    // Resolve label/subdomain filters by name without creating them: an unknown
    // name simply yields no matches rather than polluting the domain.
    let mut tags = Vec::with_capacity(input.labels.len());
    for label in &input.labels {
        match eng.list_tags(domain_id) {
            Ok(all) => match all.into_iter().find(|t| &t.name == label) {
                Some(t) => tags.push(t.id),
                // Unknown label: nothing can match all-of, so return no hits early.
                None if input.match_all || input.labels.len() == 1 => {
                    return ok_json(out_json, json!({ "hits": [] }))
                }
                None => {}
            },
            Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
        }
    }
    let subdomain = match &input.subdomain {
        Some(name) => match eng.subdomain_id_by_name(domain_id, name) {
            Ok(Some(id)) => Some(id),
            Ok(None) => return ok_json(out_json, json!({ "hits": [] })),
            Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
        },
        None => None,
    };

    let req = SearchRequest {
        query: QueryInput::Text(input.query),
        k: input.k,
        tags,
        match_all: input.match_all,
        document_ids: input.document_ids.into_iter().map(DocumentId::from).collect(),
        subdomain,
        filter: input.filter,
    };
    match eng.search(domain_id, req) {
        Ok(hits) => {
            let out: Vec<HitOut> =
                hits.into_iter().map(|h| HitOut { chunk: h.chunk, score: h.score }).collect();
            ok_json(out_json, json!({ "hits": out }))
        }
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// maintenance
// ---------------------------------------------------------------------------

/// Persist on-disk indexes (HNSW dumps) to the configured `index_dir`. No-op when
/// the index is flat or no `index_dir` was given. Output: `{"persisted":N}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_persist_indexes(
    handle: *mut Engine,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    match eng.persist_indexes() {
        Ok(n) => ok_json(out_json, json!({ "persisted": n })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// read / browse
// ---------------------------------------------------------------------------

/// List all domains. **No input.** Output: `{"domains":[{...}]}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_list_domains(handle: *mut Engine, out_json: *mut *mut c_char) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    match eng.list_domains() {
        Ok(domains) => ok_json(out_json, json!({ "domains": domains })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct DomainRef {
    domain_id: u64,
}

/// List tags (labels) in a domain. Input: `{"domain_id":N}`. Output: `{"tags":[...]}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_list_tags(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: DomainRef = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.list_tags(DomainId::from(input.domain_id)) {
        Ok(tags) => ok_json(out_json, json!({ "tags": tags })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

/// List subdomains in a domain. Input: `{"domain_id":N}`. Output: `{"subdomains":[...]}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_list_subdomains(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: DomainRef = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.list_subdomains(DomainId::from(input.domain_id)) {
        Ok(subs) => ok_json(out_json, json!({ "subdomains": subs })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct ListDocumentsInput {
    domain_id: u64,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    100
}

/// Paginated document listing. Input: `{"domain_id":N,"offset":0,"limit":100}`.
/// Output: `{"documents":[{...}]}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_list_documents(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: ListDocumentsInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.list_documents(DomainId::from(input.domain_id), input.offset, input.limit) {
        Ok(docs) => ok_json(out_json, json!({ "documents": docs })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct DocumentRef {
    document_id: u64,
}

/// Fetch one document by id. Input: `{"document_id":N}`. Output: the `Document`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_get_document(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: DocumentRef = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.get_document(DocumentId::from(input.document_id)) {
        Ok(doc) => ok_json(out_json, json!(doc)),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

/// Delete a document and its chunks (cascade). Input: `{"document_id":N}`.
/// Output: `{"deleted":true}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_delete_document(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: DocumentRef = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.delete_document(DocumentId::from(input.document_id)) {
        Ok(()) => ok_json(out_json, json!({ "deleted": true })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct ChunkContextInput {
    chunk_id: u64,
    /// Preceding chunks to include (walking the prev chain).
    #[serde(default = "default_ctx")]
    before: usize,
    /// Following chunks to include (walking the next chain).
    #[serde(default = "default_ctx")]
    after: usize,
}

fn default_ctx() -> usize {
    1
}

/// A chunk with up to `before` preceding and `after` following neighbours, in
/// document order. Input: `{"chunk_id":N,"before":1,"after":1}`.
/// Output: `{"chunks":[{...}]}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_chunk_context(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(out_json, NUCLEUS_ERR_NULL_ARG, "engine handle is null".into());
    };
    let input: ChunkContextInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.chunk_context(ChunkId::from(input.chunk_id), input.before, input.after) {
        Ok(chunks) => ok_json(out_json, json!({ "chunks": chunks })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// shared parsing
// ---------------------------------------------------------------------------

/// Parse a JSON input string into `T`, mapping null/utf8/json errors to a
/// `(code, message)` pair the caller turns into a failure.
///
/// # Safety
/// `input_json` must be null or a valid C string.
unsafe fn parse<T: for<'de> Deserialize<'de>>(input_json: *const c_char) -> Result<T, (i32, String)> {
    let Some(s) = cstr(input_json) else {
        return Err((NUCLEUS_ERR_UTF8, "input_json is null or not UTF-8".into()));
    };
    serde_json::from_str(s).map_err(|e| (NUCLEUS_ERR_JSON, format!("invalid input JSON: {e}")))
}
