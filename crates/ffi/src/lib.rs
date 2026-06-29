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
use nucleus_core::extract::extract_text;
use nucleus_core::id::{ChunkId, DocumentId, DomainId, SubdomainId, TagId};
use nucleus_core::index::IndexKind;
use nucleus_core::storage::Storage;
use nucleus_core::util::sha256_hex;
use nucleus_core::{Engine, NucleusError};

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
    /// Optional passphrase for **encryption at rest** (always on). With a
    /// passphrase the key is derived via Argon2id (portable: the same phrase
    /// reopens the DB anywhere). Omitted/empty, a per-machine key is used
    /// automatically. A legacy unencrypted database is migrated transparently on
    /// first open.
    #[serde(default)]
    passphrase: Option<String>,
    /// Optional path to the machine **key file** (used only without a passphrase).
    /// Defaults to `NUCLEUS_KEYFILE`, else an OS per-user config dir — **kept out of
    /// the database directory** so a data backup never carries the key. Back the key
    /// up yourself, separately.
    #[serde(default)]
    keyfile: Option<String>,
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
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
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
        return fail(
            ptr::null_mut(),
            NUCLEUS_ERR_NULL_ARG,
            "out_handle is null".into(),
        );
    }
    *out_handle = ptr::null_mut();

    let Some(cfg_str) = cstr(config_json) else {
        return fail(
            ptr::null_mut(),
            NUCLEUS_ERR_UTF8,
            "config_json is null or not UTF-8".into(),
        );
    };
    let cfg: OpenConfig = match serde_json::from_str(cfg_str) {
        Ok(c) => c,
        Err(e) => {
            return fail(
                ptr::null_mut(),
                NUCLEUS_ERR_JSON,
                format!("invalid config JSON: {e}"),
            )
        }
    };

    let index_kind = match cfg.index_kind.as_deref() {
        None | Some("flat") => IndexKind::Flat,
        Some("hnsw") => IndexKind::Hnsw,
        // int8 scalar quantisation: ~4x less RAM at a negligible recall cost.
        Some("sq") | Some("scalar") => IndexKind::Sq,
        Some(other) => {
            return fail(
                ptr::null_mut(),
                NUCLEUS_ERR_JSON,
                format!("unknown index_kind '{other}' (expected 'flat', 'hnsw' or 'sq')"),
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
            return fail(
                ptr::null_mut(),
                NUCLEUS_ERR_ENGINE,
                format!("create data dir: {e}"),
            );
        }
    }
    if let Some(dir) = &cfg.index_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return fail(
                ptr::null_mut(),
                NUCLEUS_ERR_ENGINE,
                format!("create index dir: {e}"),
            );
        }
    }

    let passphrase = cfg.passphrase.as_deref().filter(|s| !s.is_empty());
    let keyfile = cfg
        .keyfile
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let storage = match Storage::open_with_options(&db_path, passphrase, keyfile.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            return fail(
                ptr::null_mut(),
                NUCLEUS_ERR_ENGINE,
                format!("open storage: {e}"),
            )
        }
    };
    let embedder: Arc<dyn Embedder> = Arc::new(LocalEmbedder::with_options(
        cfg.model_cache.map(PathBuf::from),
        cfg.gpu,
    ));
    let engine = match Engine::open(
        storage,
        embedder,
        index_kind,
        cfg.index_dir.map(PathBuf::from),
    ) {
        Ok(e) => e,
        Err(e) => {
            return fail(
                ptr::null_mut(),
                NUCLEUS_ERR_ENGINE,
                format!("open engine: {e}"),
            )
        }
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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

/// Shared ingest tail: deduplicate by content hash within the domain, then (if new)
/// resolve subdomain/labels by name, create the document, embed/index it and record
/// the hash. Returns `(document_id, chunk_count, duplicate)`; on a duplicate, the
/// existing document id with `chunk_count = 0`. Errors map to `(code, message)`.
#[allow(clippy::too_many_arguments)]
fn dedup_and_ingest(
    eng: &Engine,
    domain_id: DomainId,
    title: &str,
    source: Option<String>,
    metadata: BTreeMap<String, String>,
    labels: &[String],
    subdomain: &Option<String>,
    text: String,
) -> Result<(u64, usize, bool), (i32, String)> {
    let err = |e: NucleusError| (NUCLEUS_ERR_ENGINE, e.to_string());

    let hash = sha256_hex(text.as_bytes());
    match eng.find_document_by_hash(domain_id, &hash) {
        Ok(Some(existing)) => return Ok((existing.get(), 0, true)),
        Ok(None) => {}
        Err(e) => return Err(err(e)),
    }

    let subdomain_id = match subdomain {
        Some(name) => Some(
            eng.get_or_create_subdomain(domain_id, name, "")
                .map_err(err)?
                .id,
        ),
        None => None,
    };
    let mut tags = Vec::with_capacity(labels.len());
    for label in labels {
        tags.push(eng.get_or_create_label(domain_id, label).map_err(err)?.id);
    }

    let doc = eng
        .create_document_record(domain_id, subdomain_id, title, source, metadata, tags)
        .map_err(err)?;
    let chunk_count = eng
        .populate_document(&doc, IngestBody::Text(text))
        .map_err(err)?;
    eng.set_document_hash(domain_id, doc.id, &hash)
        .map_err(err)?;
    Ok((doc.id.get(), chunk_count, false))
}

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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: IngestInput = match parse(input_json) {
        Ok(v) => v,
        Err(code_msg) => return fail(out_json, code_msg.0, code_msg.1),
    };
    let domain_id = DomainId::from(input.domain_id);

    match dedup_and_ingest(
        eng,
        domain_id,
        &input.title,
        input.source,
        input.metadata,
        &input.labels,
        &input.subdomain,
        input.text,
    ) {
        Ok((document_id, chunk_count, duplicate)) => ok_json(
            out_json,
            json!({ "document_id": document_id, "chunk_count": chunk_count, "duplicate": duplicate }),
        ),
        Err((code, msg)) => fail(out_json, code, msg),
    }
}

#[derive(Deserialize)]
struct IngestFileInput {
    domain_id: u64,
    /// Used to detect the format (extension) AND as the default title.
    filename: String,
    /// Defaults to `filename` when omitted.
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    subdomain: Option<String>,
}

/// Ingest a **raw file** (pdf, docx, xlsx, html, md, txt…): the engine extracts
/// the text by format, then chunks/embeds/indexes it. Metadata travels as JSON in
/// `input_json`; the file bytes are passed separately via `bytes`/`bytes_len`.
/// Output: `{"document_id":N,"chunk_count":M,"chars":C}`.
///
/// # Safety
/// `bytes` must point to `bytes_len` readable bytes (or be null with len 0). The
/// usual string-pointer rules apply to `input_json`; see module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_ingest_file(
    handle: *mut Engine,
    input_json: *const c_char,
    bytes: *const u8,
    bytes_len: usize,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    if bytes.is_null() || bytes_len == 0 {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "file bytes are empty".into(),
        );
    }
    let input: IngestFileInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    let data = std::slice::from_raw_parts(bytes, bytes_len);

    // Extract text by format (the engine owns the parsers — pdf/docx/xlsx/html…).
    let text = match extract_text(&input.filename, data) {
        Ok(t) => t,
        Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    };
    let chars = text.chars().count();
    let domain_id = DomainId::from(input.domain_id);
    let title = input.title.unwrap_or_else(|| input.filename.clone());

    match dedup_and_ingest(
        eng,
        domain_id,
        &title,
        input.source,
        input.metadata,
        &input.labels,
        &input.subdomain,
        text,
    ) {
        Ok((document_id, chunk_count, duplicate)) => ok_json(
            out_json,
            json!({ "document_id": document_id, "chunk_count": chunk_count, "chars": chars, "duplicate": duplicate }),
        ),
        Err((code, msg)) => fail(out_json, code, msg),
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
    /// Result diversity via MMR, in `[0, 1]`. `0` (default) = pure relevance;
    /// higher trades relevance for less redundancy among the returned chunks.
    #[serde(default)]
    diversity: f32,
}

fn default_k() -> usize {
    10
}

#[derive(Serialize)]
struct HitOut {
    chunk: nucleus_core::model::Chunk,
    score: f32,
    /// Excerpt centred on the matched query terms, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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
        document_ids: input
            .document_ids
            .into_iter()
            .map(DocumentId::from)
            .collect(),
        subdomain,
        filter: input.filter,
        diversity: input.diversity,
    };
    match eng.search(domain_id, req) {
        Ok(hits) => {
            let out: Vec<HitOut> = hits
                .into_iter()
                .map(|h| HitOut {
                    chunk: h.chunk,
                    score: h.score,
                    snippet: h.snippet,
                })
                .collect();
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    match eng.persist_indexes() {
        Ok(n) => ok_json(out_json, json!({ "persisted": n })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct BackupInput {
    /// Destination file for the snapshot (created/overwritten). Its directory is
    /// created if missing.
    dest_path: String,
}

/// Write a consistent snapshot of the database to a file. Input:
/// `{"dest_path":"backups/nucleus-2026.redb"}`. Output:
/// `{"backed_up":true,"path":"..."}`.
///
/// The snapshot is a self-contained redb file, **encrypted with the same key** as
/// the live database (it reopens with the same passphrase, or the machine key from
/// the same directory). Safe to call while the engine is in use.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_backup(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: BackupInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    // Make sure the destination directory exists before snapshotting.
    let dst = PathBuf::from(&input.dest_path);
    if let Some(parent) = dst.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return fail(
                out_json,
                NUCLEUS_ERR_ENGINE,
                format!("create backup dir: {e}"),
            );
        }
    }
    match eng.backup_to(&dst) {
        Ok(()) => ok_json(
            out_json,
            json!({ "backed_up": true, "path": input.dest_path }),
        ),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct RekeyInput {
    /// Destination file for the rotated database (created/overwritten).
    dest_path: String,
    /// New passphrase. Omitted/empty → use a machine key at `keyfile` (or default).
    #[serde(default)]
    passphrase: Option<String>,
    /// New machine key file (only when no passphrase). Default per `nucleus_open`.
    #[serde(default)]
    keyfile: Option<String>,
}

/// **Rotate the encryption key.** Writes a copy of the database re-encrypted under
/// a new key. Input: `{"dest_path":"...","passphrase":"...","keyfile":"..."}`
/// (passphrase/keyfile optional). Output: `{"rekeyed":true,"path":"..."}`.
/// Activate the result by closing the engine and reopening on `dest_path` with the
/// new key (or swapping it in for the live file). The dedup index resets — see the
/// engine docs.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_rekey(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: RekeyInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    let dst = PathBuf::from(&input.dest_path);
    if let Some(parent) = dst.parent().filter(|p| !p.as_os_str().is_empty()) {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return fail(out_json, NUCLEUS_ERR_ENGINE, format!("create dir: {e}"));
        }
    }
    let passphrase = input.passphrase.as_deref().filter(|s| !s.is_empty());
    let keyfile = input
        .keyfile
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    match eng.rekey_to(&dst, passphrase, keyfile.as_deref()) {
        Ok(()) => ok_json(
            out_json,
            json!({ "rekeyed": true, "path": input.dest_path }),
        ),
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
pub unsafe extern "C" fn nucleus_list_domains(
    handle: *mut Engine,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
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
// edit / delete (cascade)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RenameDomainInput {
    domain_id: u64,
    name: String,
}

/// Rename a domain. Input: `{"domain_id":N,"name":"..."}`. Output: the `Domain`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_rename_domain(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: RenameDomainInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.rename_domain(DomainId::from(input.domain_id), &input.name) {
        Ok(d) => ok_json(out_json, json!(d)),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

/// Delete a domain and everything under it (subdomains, documents, chunks, tags).
/// Input: `{"domain_id":N}`. Output: `{"deleted":true}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_delete_domain(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: DomainRef = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.delete_domain(DomainId::from(input.domain_id)) {
        Ok(()) => ok_json(out_json, json!({ "deleted": true })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct SubdomainRef {
    subdomain_id: u64,
}

/// Delete a subdomain and cascade to its documents/chunks. Input:
/// `{"subdomain_id":N}`. Output: `{"deleted":true}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_delete_subdomain(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: SubdomainRef = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.delete_subdomain(SubdomainId::from(input.subdomain_id)) {
        Ok(()) => ok_json(out_json, json!({ "deleted": true })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct UpdateTagInput {
    tag_id: u64,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

/// Update a label's display name and/or description. Input:
/// `{"tag_id":N,"display_name":"...","description":"..."}` (omit fields to keep).
/// Output: the `Tag`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_update_tag(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: UpdateTagInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.update_tag(
        TagId::from(input.tag_id),
        input.display_name.as_deref(),
        input.description.as_deref(),
    ) {
        Ok(t) => ok_json(out_json, json!(t)),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct TagRef {
    tag_id: u64,
}

/// Delete a label, detaching it from chunks/documents (which survive). Input:
/// `{"tag_id":N}`. Output: `{"deleted":true}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_delete_tag(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: TagRef = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.delete_tag(TagId::from(input.tag_id)) {
        Ok(()) => ok_json(out_json, json!({ "deleted": true })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct UpdateDocumentInput {
    document_id: u64,
    /// New label set (by name, get-or-create). Omit to leave tags unchanged.
    #[serde(default)]
    labels: Option<Vec<String>>,
    /// New subdomain name (get-or-create). Omit + `clear_subdomain:false` leaves
    /// it unchanged; set `clear_subdomain:true` to remove the subdomain.
    #[serde(default)]
    subdomain: Option<String>,
    #[serde(default)]
    clear_subdomain: bool,
}

/// Re-assign a document's labels and/or subdomain (propagated to its chunks).
/// Input: `{"document_id":N,"labels":["a"],"subdomain":"x"}`. Output: `Document`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_update_document(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: UpdateDocumentInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    let doc_id = DocumentId::from(input.document_id);
    // We need the owning domain to resolve label/subdomain names.
    let domain_id = match eng.get_document(doc_id) {
        Ok(d) => d.domain_id,
        Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    };

    let new_tags = match &input.labels {
        Some(names) => {
            let mut ids = Vec::with_capacity(names.len());
            for name in names {
                match eng.get_or_create_label(domain_id, name) {
                    Ok(t) => ids.push(t.id),
                    Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
                }
            }
            Some(ids)
        }
        None => None,
    };

    let change_subdomain = input.clear_subdomain || input.subdomain.is_some();
    let new_subdomain = match &input.subdomain {
        Some(name) if !input.clear_subdomain => {
            match eng.get_or_create_subdomain(domain_id, name, "") {
                Ok(s) => Some(s.id),
                Err(e) => return fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
            }
        }
        _ => None, // clearing, or no change requested
    };

    match eng.update_document(doc_id, new_tags, new_subdomain, change_subdomain) {
        Ok(d) => ok_json(out_json, json!(d)),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct ReindexInput {
    domain_id: u64,
    /// Switch to a new embedding model (changes the dimension). Omit to re-embed
    /// with the current model.
    #[serde(default)]
    model: Option<String>,
}

/// Re-embed every chunk of a domain and rebuild its vector index (blocking).
/// Input: `{"domain_id":N,"model":null}`. Output: `{"reindexed":N}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_reindex_domain(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: ReindexInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    match eng.reindex_domain(DomainId::from(input.domain_id), input.model.as_deref()) {
        Ok(n) => ok_json(out_json, json!({ "reindexed": n })),
        Err(e) => fail(out_json, NUCLEUS_ERR_ENGINE, e.to_string()),
    }
}

#[derive(Deserialize)]
struct SearchMultiInput {
    /// Domains to search together. All must share the same embedding model/dim.
    domain_ids: Vec<u64>,
    query: String,
    #[serde(default = "default_k")]
    k: usize,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    diversity: f32,
}

/// Search several domains at once (they must share a model). Input:
/// `{"domain_ids":[1,2],"query":"...","k":10}`. Output: `{"hits":[...]}`.
///
/// # Safety
/// See module docs.
#[no_mangle]
pub unsafe extern "C" fn nucleus_search_multi(
    handle: *mut Engine,
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    let Some(eng) = engine(handle) else {
        return fail(
            out_json,
            NUCLEUS_ERR_NULL_ARG,
            "engine handle is null".into(),
        );
    };
    let input: SearchMultiInput = match parse(input_json) {
        Ok(v) => v,
        Err(cm) => return fail(out_json, cm.0, cm.1),
    };
    let domain_ids: Vec<DomainId> = input.domain_ids.into_iter().map(DomainId::from).collect();
    let req = SearchRequest {
        query: QueryInput::Text(input.query),
        k: input.k,
        tags: Vec::new(),
        match_all: false,
        document_ids: Vec::new(),
        subdomain: None,
        filter: input.filter,
        diversity: input.diversity,
    };
    match eng.search_multi(&domain_ids, req) {
        Ok(hits) => {
            let out: Vec<HitOut> = hits
                .into_iter()
                .map(|h| HitOut {
                    chunk: h.chunk,
                    score: h.score,
                    snippet: h.snippet,
                })
                .collect();
            ok_json(out_json, json!({ "hits": out }))
        }
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
unsafe fn parse<T: for<'de> Deserialize<'de>>(
    input_json: *const c_char,
) -> Result<T, (i32, String)> {
    let Some(s) = cstr(input_json) else {
        return Err((NUCLEUS_ERR_UTF8, "input_json is null or not UTF-8".into()));
    };
    serde_json::from_str(s).map_err(|e| (NUCLEUS_ERR_JSON, format!("invalid input JSON: {e}")))
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Exercise the C ABI boundary directly: status codes, the JSON in/out
    //! contract, the thread-local last-error, and pointer/string lifetimes — the
    //! `unsafe` surface that the higher-level bindings (C#, C/C++) rely on.
    //!
    //! These use the static `LocalEmbedder::dim()` path (no model download), so
    //! `open` / `create_domain` / `list_domains` run offline. The full
    //! ingest→search round-trip needs the ~450 MB embedding model, so it lives
    //! behind `#[ignore]` (run with `cargo test -p nucleus-ffi -- --ignored`).

    use super::*;
    use serde_json::Value;
    use std::ffi::{CStr, CString};
    use tempfile::TempDir;

    /// Open an engine on a throwaway flat database. Returns the handle plus the
    /// `TempDir` (kept alive so the database file outlives the test).
    fn open_engine() -> (*mut Engine, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("t.redb");
        // Keep the machine key file inside the tempdir so tests stay hermetic and
        // never touch the real per-user key location.
        let key = dir.path().join("t.key");
        let cfg = CString::new(
            json!({
                "db_path": db.to_string_lossy(),
                "keyfile": key.to_string_lossy(),
                "index_kind": "flat",
            })
            .to_string(),
        )
        .unwrap();
        let mut handle: *mut Engine = ptr::null_mut();
        let code = unsafe { nucleus_open(cfg.as_ptr(), &mut handle) };
        assert_eq!(code, NUCLEUS_OK, "open failed: {:?}", last_error_string());
        assert!(!handle.is_null());
        (handle, dir)
    }

    /// The current thread's last error message, if any.
    fn last_error_string() -> Option<String> {
        let p = nucleus_last_error();
        if p.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
        }
    }

    /// Invoke a `(handle, input_json, out_json)` FFI function and return its status
    /// code together with the parsed JSON out-parameter (freed via the library).
    unsafe fn call(
        f: unsafe extern "C" fn(*mut Engine, *const c_char, *mut *mut c_char) -> i32,
        handle: *mut Engine,
        input: &str,
    ) -> (i32, Value) {
        let cin = CString::new(input).unwrap();
        let mut out: *mut c_char = ptr::null_mut();
        let code = f(handle, cin.as_ptr(), &mut out);
        let val = if out.is_null() {
            Value::Null
        } else {
            let s = CStr::from_ptr(out).to_str().unwrap().to_owned();
            nucleus_string_free(out);
            serde_json::from_str(&s).unwrap_or(Value::String(s))
        };
        (code, val)
    }

    // --- open / close -----------------------------------------------------

    #[test]
    fn open_and_close_roundtrip() {
        let (handle, _dir) = open_engine();
        unsafe { nucleus_close(handle) };
    }

    #[test]
    fn open_rejects_null_out_handle() {
        let cfg = CString::new("{}").unwrap();
        let code = unsafe { nucleus_open(cfg.as_ptr(), ptr::null_mut()) };
        assert_eq!(code, NUCLEUS_ERR_NULL_ARG);
    }

    #[test]
    fn open_rejects_null_config() {
        let mut h: *mut Engine = ptr::null_mut();
        let code = unsafe { nucleus_open(ptr::null(), &mut h) };
        assert_eq!(code, NUCLEUS_ERR_UTF8);
        assert!(h.is_null());
    }

    #[test]
    fn open_rejects_invalid_json() {
        let cfg = CString::new("{ not json").unwrap();
        let mut h: *mut Engine = ptr::null_mut();
        let code = unsafe { nucleus_open(cfg.as_ptr(), &mut h) };
        assert_eq!(code, NUCLEUS_ERR_JSON);
        assert!(h.is_null(), "handle stays null on failure");
    }

    #[test]
    fn open_rejects_unknown_index_kind() {
        // Validated before any storage is touched, so it needs no real db path.
        let cfg = CString::new(json!({ "index_kind": "bogus" }).to_string()).unwrap();
        let mut h: *mut Engine = ptr::null_mut();
        let code = unsafe { nucleus_open(cfg.as_ptr(), &mut h) };
        assert_eq!(code, NUCLEUS_ERR_JSON);
        assert!(h.is_null());
    }

    // --- domains ----------------------------------------------------------

    #[test]
    fn create_domain_rejects_null_handle() {
        let (code, _) = unsafe { call(nucleus_create_domain, ptr::null_mut(), r#"{"name":"x"}"#) };
        assert_eq!(code, NUCLEUS_ERR_NULL_ARG);
    }

    #[test]
    fn create_domain_rejects_invalid_json() {
        let (handle, _dir) = open_engine();
        let (code, _) = unsafe { call(nucleus_create_domain, handle, "{ bad") };
        assert_eq!(code, NUCLEUS_ERR_JSON);
        unsafe { nucleus_close(handle) };
    }

    #[test]
    fn create_domain_returns_typed_object() {
        let (handle, _dir) = open_engine();
        let (code, dom) = unsafe { call(nucleus_create_domain, handle, r#"{"name":"legal"}"#) };
        assert_eq!(code, NUCLEUS_OK);
        assert_eq!(dom["name"], "legal");
        // Default model is the 384-dim multilingual e5.
        assert_eq!(dom["dim"], 384);
        unsafe { nucleus_close(handle) };
    }

    #[test]
    fn list_domains_reflects_creation() {
        let (handle, _dir) = open_engine();
        for name in ["legal", "fiscal"] {
            let (code, _) = unsafe {
                call(
                    nucleus_create_domain,
                    handle,
                    &json!({ "name": name }).to_string(),
                )
            };
            assert_eq!(code, NUCLEUS_OK);
        }

        // list_domains takes no input.
        let mut out: *mut c_char = ptr::null_mut();
        let code = unsafe { nucleus_list_domains(handle, &mut out) };
        assert_eq!(code, NUCLEUS_OK);
        let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_owned() };
        unsafe { nucleus_string_free(out) };
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["domains"].as_array().unwrap().len(), 2);
        unsafe { nucleus_close(handle) };
    }

    // --- backup -----------------------------------------------------------

    #[test]
    fn backup_creates_reopenable_snapshot() {
        let (handle, dir) = open_engine();
        let (code, _) = unsafe { call(nucleus_create_domain, handle, r#"{"name":"legal"}"#) };
        assert_eq!(code, NUCLEUS_OK);

        // Snapshot to a separate subdirectory (created on demand). The key lives
        // elsewhere (dir/t.key), so the backup never carries it.
        let backup = dir.path().join("snapshots").join("backup.redb");
        let (code, val) = unsafe {
            call(
                nucleus_backup,
                handle,
                &json!({ "dest_path": backup.to_string_lossy() }).to_string(),
            )
        };
        assert_eq!(code, NUCLEUS_OK, "backup failed: {:?}", last_error_string());
        assert_eq!(val["backed_up"], true);
        assert!(backup.exists());
        unsafe { nucleus_close(handle) };

        // The snapshot is a standalone, encrypted, reopenable database — given the
        // same (separately-managed) machine key.
        let cfg = CString::new(
            json!({
                "db_path": backup.to_string_lossy(),
                "keyfile": dir.path().join("t.key").to_string_lossy(),
                "index_kind": "flat",
            })
            .to_string(),
        )
        .unwrap();
        let mut h2: *mut Engine = ptr::null_mut();
        let code = unsafe { nucleus_open(cfg.as_ptr(), &mut h2) };
        assert_eq!(code, NUCLEUS_OK, "reopen failed: {:?}", last_error_string());
        let mut out: *mut c_char = ptr::null_mut();
        let code = unsafe { nucleus_list_domains(h2, &mut out) };
        assert_eq!(code, NUCLEUS_OK);
        let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_owned() };
        unsafe { nucleus_string_free(out) };
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["domains"].as_array().unwrap().len(), 1);
        assert_eq!(v["domains"][0]["name"], "legal");
        unsafe { nucleus_close(h2) };
    }

    #[test]
    fn backup_rejects_null_handle() {
        let (code, _) =
            unsafe { call(nucleus_backup, ptr::null_mut(), r#"{"dest_path":"x.redb"}"#) };
        assert_eq!(code, NUCLEUS_ERR_NULL_ARG);
    }

    #[test]
    fn rekey_rotates_to_passphrase() {
        let (handle, dir) = open_engine(); // machine-key DB
        let (code, _) = unsafe { call(nucleus_create_domain, handle, r#"{"name":"legal"}"#) };
        assert_eq!(code, NUCLEUS_OK);

        // Rotate to a passphrase-protected copy.
        let dst = dir.path().join("rotated.redb");
        let (code, val) = unsafe {
            call(
                nucleus_rekey,
                handle,
                &json!({ "dest_path": dst.to_string_lossy(), "passphrase": "rk-pass" }).to_string(),
            )
        };
        assert_eq!(code, NUCLEUS_OK, "rekey failed: {:?}", last_error_string());
        assert_eq!(val["rekeyed"], true);
        unsafe { nucleus_close(handle) };

        // The rotated DB opens with the NEW passphrase and keeps the data.
        let cfg = CString::new(
            json!({ "db_path": dst.to_string_lossy(), "passphrase": "rk-pass", "index_kind": "flat" })
                .to_string(),
        )
        .unwrap();
        let mut h2: *mut Engine = ptr::null_mut();
        let code = unsafe { nucleus_open(cfg.as_ptr(), &mut h2) };
        assert_eq!(code, NUCLEUS_OK, "reopen failed: {:?}", last_error_string());
        let mut out: *mut c_char = ptr::null_mut();
        let code = unsafe { nucleus_list_domains(h2, &mut out) };
        assert_eq!(code, NUCLEUS_OK);
        let s = unsafe { CStr::from_ptr(out).to_str().unwrap().to_owned() };
        unsafe { nucleus_string_free(out) };
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["domains"][0]["name"], "legal");
        unsafe { nucleus_close(h2) };
    }

    // --- error reporting & memory hygiene --------------------------------

    #[test]
    fn last_error_is_set_on_failure() {
        let (handle, _dir) = open_engine();
        let (code, val) = unsafe { call(nucleus_create_domain, handle, "definitely not json") };
        assert!(code < 0);
        // The failure also writes {"error": ...} to the out-parameter.
        assert!(
            val.get("error").is_some(),
            "out_json carries the error: {val}"
        );
        assert!(
            last_error_string().is_some(),
            "thread-local last error is populated"
        );
        unsafe { nucleus_close(handle) };
    }

    #[test]
    fn null_pointers_are_safe_to_free_and_close() {
        // Both must tolerate null without UB (documented contract).
        unsafe { nucleus_string_free(ptr::null_mut()) };
        unsafe { nucleus_close(ptr::null_mut()) };
    }

    // --- full pipeline (needs the embedding model; opt-in) ----------------

    #[test]
    #[ignore = "downloads the ~450MB embedding model; run with `-- --ignored`"]
    fn ingest_and_search_roundtrip() {
        let (handle, _dir) = open_engine();
        let (code, dom) = unsafe { call(nucleus_create_domain, handle, r#"{"name":"docs"}"#) };
        assert_eq!(code, NUCLEUS_OK);
        let domain_id = dom["id"].as_u64().unwrap();

        let ingest = json!({
            "domain_id": domain_id,
            "title": "contrato",
            "text": "el contrato laboral regula la relación entre empresa y trabajador",
        })
        .to_string();
        let (code, out) = unsafe { call(nucleus_ingest_text, handle, &ingest) };
        assert_eq!(code, NUCLEUS_OK, "{:?}", last_error_string());
        assert!(out["chunk_count"].as_u64().unwrap() >= 1);

        let query =
            json!({ "domain_id": domain_id, "query": "contrato laboral", "k": 5 }).to_string();
        let (code, out) = unsafe { call(nucleus_search, handle, &query) };
        assert_eq!(code, NUCLEUS_OK);
        assert!(
            !out["hits"].as_array().unwrap().is_empty(),
            "search finds the chunk"
        );
        unsafe { nucleus_close(handle) };
    }
}
