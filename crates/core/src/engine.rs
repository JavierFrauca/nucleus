//! The engine façade that ties storage, the vector index and the embedder
//! together. This is the synchronous core; the async job layer (see
//! [`crate::jobs`]) drives `populate_document` on background workers.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::auth::{self, ApiToken, AuthContext, Scope};
use crate::chunking::{Chunker, FixedSizeChunker};
use crate::embed::{Embedder, DEFAULT_MODEL};
use crate::error::NucleusError;
use crate::id::{ChunkId, DocumentId, DomainId, SubdomainId, TagId, TokenId};
use crate::index::{build_index, HnswIndex, IndexKind, LexicalIndex, VectorIndex};
use crate::jobs::Job;
use crate::model::{Chunk, Document, Domain, Subdomain, Tag};
use crate::rerank::Reranker;
use crate::storage::{NewChunk, Storage};
use crate::util::now_millis;
use crate::Result;

/// Hard cap on `k` per search, to bound per-request work and memory.
const MAX_K: usize = 1000;

/// Default number of top fused candidates handed to the reranker. The
/// cross-encoder is costly per `(query, passage)` pair, so this bounds the
/// rerank stage; raise it for quality, lower it for latency.
const DEFAULT_RERANK_CANDIDATES: usize = 20;

/// Reciprocal Rank Fusion of several ranked lists into the top-`k`.
fn rrf_fuse(lists: &[Vec<(ChunkId, f32)>], k: usize) -> Vec<(ChunkId, f32)> {
    const RRF_K: f32 = 60.0;
    let mut scores: HashMap<ChunkId, f32> = HashMap::new();
    for list in lists {
        for (rank, (id, _)) in list.iter().enumerate() {
            *scores.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
    }
    let mut fused: Vec<(ChunkId, f32)> = scores.into_iter().collect();
    fused.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
    fused.truncate(k);
    fused
}

/// Lower-case a char to a single char (best-effort: takes the first scalar of
/// its Unicode lowercase mapping), keeping a 1:1 char alignment with the source
/// so positions found in the lowered copy map back to the original.
fn lower1(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Find the first index at which `needle` occurs in `haystack` (both as char
/// slices). Naive search — needles are short query terms.
fn find_subslice(haystack: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| haystack[i..i + needle.len()] == *needle)
}

/// Build a short **snippet** of `text` centred on the earliest query-term match,
/// padded by `radius` characters and elided with `…`. Case-insensitive. Returns
/// `None` when the query has no usable terms or none of them appear in the text
/// (e.g. a purely semantic match), in which case the caller keeps the full text.
fn snippet(text: &str, query: &str, radius: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| lower1(*c)).collect();
    let terms: Vec<Vec<char>> = query
        .split_whitespace()
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.chars().map(lower1).collect())
        .collect();
    if terms.is_empty() {
        return None;
    }
    let pos = terms
        .iter()
        .filter_map(|t| find_subslice(&lower, t))
        .min()?;
    let start = pos.saturating_sub(radius);
    let end = (pos + radius).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    Some(out)
}

/// Cosine similarity of two vectors. Returns `0.0` if either is empty or
/// zero-norm (degenerate), so it never produces `NaN`.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len().min(b.len()) {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Re-rank `items` with **Maximal Marginal Relevance**, returning the top-`k`.
///
/// At each step it picks the candidate maximising
/// `lambda * relevance - (1 - lambda) * max_similarity_to_already_picked`,
/// where relevance is the incoming score min-max normalised to `[0, 1]` (so it
/// mixes sanely with cosine similarity) and similarity uses the candidate
/// embeddings in `embs` (a missing embedding counts as zero similarity).
/// `lambda == 1` is pure relevance; `lambda == 0` is pure diversity. The
/// reported score stays the original relevance.
fn mmr_select(
    items: Vec<(Chunk, f32)>,
    embs: &HashMap<ChunkId, Vec<f32>>,
    lambda: f32,
    k: usize,
) -> Vec<(Chunk, f32)> {
    let n = items.len();
    let target = k.min(n);
    if target == 0 {
        return Vec::new();
    }
    // Normalise relevance to [0, 1].
    let (min, max) = items
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), (_, s)| {
            (lo.min(*s), hi.max(*s))
        });
    let span = (max - min).max(f32::EPSILON);
    let rel: Vec<f32> = items.iter().map(|(_, s)| (s - min) / span).collect();
    let empty: Vec<f32> = Vec::new();
    let emb = |i: usize| embs.get(&items[i].0.id).unwrap_or(&empty);

    let mut chosen: Vec<usize> = Vec::with_capacity(target);
    let mut remaining: Vec<usize> = (0..n).collect();
    while chosen.len() < target && !remaining.is_empty() {
        let mut best_pos = 0;
        let mut best_score = f32::NEG_INFINITY;
        for (pos, &i) in remaining.iter().enumerate() {
            let max_sim = chosen
                .iter()
                .map(|&j| cosine(emb(i), emb(j)))
                .fold(0.0f32, f32::max);
            let score = lambda * rel[i] - (1.0 - lambda) * max_sim;
            if score > best_score {
                best_score = score;
                best_pos = pos;
            }
        }
        chosen.push(remaining.remove(best_pos));
    }
    // Reassemble in the chosen order, preserving each chunk's original score.
    let mut slots: Vec<Option<(Chunk, f32)>> = items.into_iter().map(Some).collect();
    chosen.into_iter().filter_map(|i| slots[i].take()).collect()
}

/// The body of an ingest request: either raw text (the engine chunks it) or
/// already-split chunks.
pub enum IngestBody {
    Text(String),
    Chunks(Vec<String>),
}

/// How a search expresses its query vector.
pub enum QueryInput {
    /// Text to embed with the domain's model.
    Text(String),
    /// A pre-computed vector (must match the domain dimension). Dense-only: no
    /// BM25 stage, since there is no text.
    Vector(Vec<f32>),
    /// A pre-computed vector **plus** the original text: dense retrieval uses the
    /// vector while BM25 still uses the text, preserving full hybrid search. Used
    /// when the query embedding was computed upstream (e.g. via the batcher).
    Hybrid { text: String, vector: Vec<f32> },
}

/// A search request against a single domain.
pub struct SearchRequest {
    pub query: QueryInput,
    pub k: usize,
    /// Restrict to chunks carrying these tags.
    pub tags: Vec<TagId>,
    /// If true, a chunk must carry *all* `tags`; otherwise any one suffices.
    pub match_all: bool,
    /// Restrict to chunks from these documents.
    pub document_ids: Vec<DocumentId>,
    /// Restrict to a single subdomain.
    pub subdomain: Option<SubdomainId>,
    /// Optional [query-language](crate::query) filter, ANDed with the above.
    pub filter: Option<String>,
    /// Result diversity via Maximal Marginal Relevance, in `[0, 1]`. `0`
    /// (default) disables it (pure relevance order); higher values trade
    /// relevance for less redundancy among the returned chunks.
    pub diversity: f32,
}

/// One ranked result.
pub struct SearchHit {
    pub chunk: Chunk,
    pub score: f32,
    /// A short excerpt of the chunk centred on the matched query terms, when the
    /// query is text and a term was found; `None` otherwise (keep the full text).
    pub snippet: Option<String>,
}

/// Result of ingesting a document synchronously.
pub struct IngestOutcome {
    pub document: Document,
    pub chunk_count: usize,
}

/// The Nucleus engine. Cheap to share behind an `Arc`; every method takes `&self`.
pub struct Engine {
    storage: Storage,
    embedder: Arc<dyn Embedder>,
    /// One in-memory vector index per domain, rebuilt from storage at startup.
    indexes: RwLock<HashMap<DomainId, Box<dyn VectorIndex>>>,
    /// One in-memory BM25 lexical index per domain (for hybrid search).
    lexical: RwLock<HashMap<DomainId, LexicalIndex>>,
    chunker: FixedSizeChunker,
    index_kind: IndexKind,
    /// Where persistable indexes (HNSW) are dumped/loaded. `None` disables it.
    index_dir: Option<PathBuf>,
    /// Optional cross-encoder reranker (second-stage scoring). `None` disables it.
    reranker: RwLock<Option<Arc<dyn Reranker>>>,
    /// How many top fused candidates the reranker re-scores.
    rerank_candidates: RwLock<usize>,
    /// Last-used timestamps per token (operational telemetry). In memory only —
    /// updating it on the auth hot path must not cost a disk write; it resets on
    /// restart.
    last_used: RwLock<HashMap<TokenId, i64>>,
}

/// A swappable holder for the live [`Engine`], so the engine can be replaced
/// atomically at runtime (used by **restore**) while the job queue and HTTP
/// handlers always observe the current one. Reads are a cheap lock-and-clone.
pub struct EngineHandle {
    inner: RwLock<Arc<Engine>>,
}

impl EngineHandle {
    pub fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(engine),
        })
    }

    /// The engine currently in use.
    pub fn current(&self) -> Arc<Engine> {
        self.inner.read().clone()
    }

    /// Replace the live engine, returning the previous one (which is closed once
    /// the last in-flight reference is dropped).
    pub fn swap(&self, engine: Arc<Engine>) -> Arc<Engine> {
        std::mem::replace(&mut self.inner.write(), engine)
    }
}

impl Engine {
    /// Build an engine using the default (exact) index backend.
    pub fn new(storage: Storage, embedder: Arc<dyn Embedder>) -> Result<Self> {
        Self::open(storage, embedder, IndexKind::default(), None)
    }

    /// Build an engine with a specific index backend (no on-disk index persistence).
    pub fn with_index_kind(
        storage: Storage,
        embedder: Arc<dyn Embedder>,
        index_kind: IndexKind,
    ) -> Result<Self> {
        Self::open(storage, embedder, index_kind, None)
    }

    /// Full constructor. When `index_dir` is set, persistable indexes (HNSW) are
    /// loaded from there at startup (falling back to a rebuild) and can be dumped
    /// via [`persist_indexes`](Engine::persist_indexes).
    pub fn open(
        storage: Storage,
        embedder: Arc<dyn Embedder>,
        index_kind: IndexKind,
        index_dir: Option<PathBuf>,
    ) -> Result<Self> {
        let engine = Self {
            storage,
            embedder,
            indexes: RwLock::new(HashMap::new()),
            lexical: RwLock::new(HashMap::new()),
            chunker: FixedSizeChunker::default(),
            index_kind,
            index_dir,
            reranker: RwLock::new(None),
            rerank_candidates: RwLock::new(DEFAULT_RERANK_CANDIDATES),
            last_used: RwLock::new(HashMap::new()),
        };
        engine.load_all_indexes()?;
        Ok(engine)
    }

    /// Direct access to the persistent store (used by the job layer and admin ops).
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// Install a cross-encoder reranker; enables the rerank stage on text searches.
    pub fn set_reranker(&self, reranker: Arc<dyn Reranker>) {
        *self.reranker.write() = Some(reranker);
    }

    /// Set how many top fused candidates the reranker re-scores per query.
    /// Higher = better ordering but slower; clamped to at least 1.
    pub fn set_rerank_candidates(&self, n: usize) {
        *self.rerank_candidates.write() = n.max(1);
    }

    fn load_all_indexes(&self) -> Result<()> {
        let domains = self.storage.list_domains()?;
        {
            let mut map = self.indexes.write();
            for d in &domains {
                // Try a persisted HNSW dump first; otherwise (re)build from storage.
                let loaded = match (&self.index_dir, self.index_kind) {
                    (Some(dir), IndexKind::Hnsw) => HnswIndex::load(dir, &d.id.to_string())
                        .ok()
                        .filter(|ix| ix.dim() == d.dim)
                        .map(|ix| Box::new(ix) as Box<dyn VectorIndex>),
                    _ => None,
                };
                let ix = match loaded {
                    Some(ix) => ix,
                    None => {
                        let mut ix = build_index(self.index_kind, d.dim);
                        for (cid, vector) in self.storage.embeddings_in_domain(d.id)? {
                            ix.upsert(cid, &vector)?;
                        }
                        ix
                    }
                };
                map.insert(d.id, ix);
            }
        }
        {
            // Rebuild the BM25 lexical index from chunk texts.
            let mut lex = self.lexical.write();
            for d in &domains {
                let mut li = LexicalIndex::new();
                for (cid, text) in self.storage.texts_in_domain(d.id)? {
                    li.add(cid, &text);
                }
                lex.insert(d.id, li);
            }
        }
        Ok(())
    }

    /// Dump every persistable (HNSW) index to `index_dir`. Returns how many were
    /// written. A no-op (returns 0) when persistence is disabled or the backend
    /// is exact (which rebuilds from storage instead).
    pub fn persist_indexes(&self) -> Result<usize> {
        let Some(dir) = &self.index_dir else {
            return Ok(0);
        };
        let indexes = self.indexes.read();
        let mut written = 0;
        for (domain_id, ix) in indexes.iter() {
            if ix.persist(dir, &domain_id.to_string())? {
                written += 1;
            }
        }
        Ok(written)
    }

    // --- domains & tags ----------------------------------------------------

    /// Create a domain. `model` defaults to [`DEFAULT_MODEL`]; the dimension is
    /// taken from the embedder, so an unknown model fails fast.
    pub fn create_domain(&self, name: &str, model: Option<&str>) -> Result<Domain> {
        let model = model.unwrap_or(DEFAULT_MODEL);
        let dim = self
            .embedder
            .dim(model)
            .ok_or_else(|| NucleusError::ModelNotFound(model.to_string()))?;
        let domain = self.storage.create_domain(name, model, dim)?;
        self.indexes
            .write()
            .insert(domain.id, build_index(self.index_kind, dim));
        self.lexical.write().insert(domain.id, LexicalIndex::new());
        Ok(domain)
    }

    pub fn get_domain(&self, id: DomainId) -> Result<Domain> {
        self.storage.get_domain(id)
    }

    pub fn list_domains(&self) -> Result<Vec<Domain>> {
        self.storage.list_domains()
    }

    pub fn create_tag(
        &self,
        domain_id: DomainId,
        name: &str,
        display_name: &str,
        description: &str,
        parent: Option<TagId>,
    ) -> Result<Tag> {
        self.storage
            .create_tag(domain_id, name, display_name, description, parent)
    }

    pub fn list_tags(&self, domain_id: DomainId) -> Result<Vec<Tag>> {
        self.storage.list_tags(domain_id)
    }

    /// Get a label (tag) by name in a domain, creating it if absent.
    pub fn get_or_create_label(&self, domain_id: DomainId, name: &str) -> Result<Tag> {
        self.storage.get_or_create_tag(domain_id, name)
    }

    pub fn create_subdomain(
        &self,
        domain_id: DomainId,
        name: &str,
        description: &str,
    ) -> Result<Subdomain> {
        self.storage.create_subdomain(domain_id, name, description)
    }

    /// Get a subdomain by name in a domain, creating it if absent.
    pub fn get_or_create_subdomain(
        &self,
        domain_id: DomainId,
        name: &str,
        description: &str,
    ) -> Result<Subdomain> {
        self.storage
            .get_or_create_subdomain(domain_id, name, description)
    }

    pub fn list_subdomains(&self, domain_id: DomainId) -> Result<Vec<Subdomain>> {
        self.storage.list_subdomains(domain_id)
    }

    pub fn subdomain_id_by_name(
        &self,
        domain_id: DomainId,
        name: &str,
    ) -> Result<Option<SubdomainId>> {
        self.storage.subdomain_id_by_name(domain_id, name)
    }

    // --- documents ---------------------------------------------------------

    /// Create the document row only (fast); chunking/embedding happens later in
    /// [`Engine::populate_document`]. The async ingest path uses this so it can
    /// return a document id immediately and finish the heavy work on a worker.
    #[allow(clippy::too_many_arguments)]
    pub fn create_document_record(
        &self,
        domain_id: DomainId,
        subdomain_id: Option<SubdomainId>,
        title: &str,
        source: Option<String>,
        metadata: BTreeMap<String, String>,
        tags: Vec<TagId>,
    ) -> Result<Document> {
        self.storage
            .create_document(domain_id, subdomain_id, title, source, metadata, tags)
    }

    /// Chunk, embed, persist and index the body for an existing document.
    /// Returns the number of chunks produced. **Blocking** (runs inference).
    pub fn populate_document(&self, doc: &Document, body: IngestBody) -> Result<usize> {
        let domain = self.get_domain(doc.domain_id)?;
        let texts = match body {
            IngestBody::Chunks(chunks) => chunks,
            IngestBody::Text(text) => self.chunker.chunk(&text),
        };
        if texts.is_empty() {
            return Ok(0);
        }
        // Embed in bounded windows. fastembed collects the outputs of *every*
        // batch of a single `embed` call before returning, so handing it a whole
        // large document at once makes peak memory scale with the number of
        // chunks. We cap the chunks per call and accumulate the (small) vectors.
        const EMBED_BATCH: usize = 64;
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for window in texts.chunks(EMBED_BATCH) {
            let mut v = self.embedder.embed_documents(&domain.model, window)?;
            if v.len() != window.len() {
                return Err(NucleusError::embedding_msg(
                    "embedder returned a different number of vectors than inputs",
                ));
            }
            vectors.append(&mut v);
        }
        // Persist the whole document in ONE transaction (chunks chained inside).
        let new_chunks: Vec<NewChunk> = texts
            .iter()
            .zip(&vectors)
            .map(|(text, vector)| NewChunk {
                text: text.as_str(),
                embedding: vector.as_slice(),
            })
            .collect();
        let ids = self.storage.insert_chunks(
            doc.domain_id,
            doc.id,
            doc.subdomain_id,
            &doc.tags,
            &doc.metadata,
            &new_chunks,
        )?;
        // Update the in-memory index.
        {
            let kind = self.index_kind;
            let mut idx = self.indexes.write();
            let ix = idx
                .entry(doc.domain_id)
                .or_insert_with(|| build_index(kind, domain.dim));
            for (id, vector) in ids.iter().zip(&vectors) {
                ix.upsert(*id, vector)?;
            }
        }
        {
            let mut lex = self.lexical.write();
            let li = lex.entry(doc.domain_id).or_default();
            for (id, text) in ids.iter().zip(&texts) {
                li.add(*id, text);
            }
        }
        Ok(ids.len())
    }

    /// Synchronous end-to-end ingest: create the document and populate it.
    pub fn ingest_document(
        &self,
        domain_id: DomainId,
        title: &str,
        source: Option<String>,
        metadata: BTreeMap<String, String>,
        tags: Vec<TagId>,
        body: IngestBody,
    ) -> Result<IngestOutcome> {
        let document =
            self.create_document_record(domain_id, None, title, source, metadata, tags)?;
        let chunk_count = self.populate_document(&document, body)?;
        Ok(IngestOutcome {
            document,
            chunk_count,
        })
    }

    pub fn get_document(&self, id: DocumentId) -> Result<Document> {
        self.storage.get_document(id)
    }

    pub fn list_documents(
        &self,
        domain_id: DomainId,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Document>> {
        self.storage.list_documents(domain_id, offset, limit)
    }

    pub fn list_jobs(&self, offset: usize, limit: usize) -> Result<Vec<Job>> {
        self.storage.list_jobs(offset, limit)
    }

    /// Find an existing **live** document by content hash (for deduplication).
    /// Stale index entries (document since deleted) are treated as a miss.
    pub fn find_document_by_hash(
        &self,
        domain_id: DomainId,
        hash: &str,
    ) -> Result<Option<DocumentId>> {
        match self.storage.document_id_by_hash(domain_id, hash)? {
            Some(id) if self.storage.get_document(id).is_ok() => Ok(Some(id)),
            _ => Ok(None),
        }
    }

    /// Record a document's content hash for future deduplication.
    pub fn set_document_hash(
        &self,
        domain_id: DomainId,
        document_id: DocumentId,
        hash: &str,
    ) -> Result<()> {
        self.storage.set_document_hash(domain_id, document_id, hash)
    }

    pub fn get_chunk(&self, id: ChunkId) -> Result<Chunk> {
        self.storage.get_chunk(id)
    }

    /// Return a chunk together with up to `before` preceding and `after`
    /// following chunks (walking the `prev`/`next` chain), in document order.
    pub fn chunk_context(&self, id: ChunkId, before: usize, after: usize) -> Result<Vec<Chunk>> {
        let center = self.storage.get_chunk(id)?;

        let mut preceding = Vec::new();
        let mut cursor = center.prev;
        for _ in 0..before {
            let Some(pid) = cursor else { break };
            let chunk = self.storage.get_chunk(pid)?;
            cursor = chunk.prev;
            preceding.push(chunk);
        }
        preceding.reverse();

        let mut following = Vec::new();
        let mut cursor = center.next;
        for _ in 0..after {
            let Some(nid) = cursor else { break };
            let chunk = self.storage.get_chunk(nid)?;
            cursor = chunk.next;
            following.push(chunk);
        }

        let mut out = preceding;
        out.push(center);
        out.extend(following);
        Ok(out)
    }

    /// Delete a document and drop its chunks from the in-memory index.
    pub fn delete_document(&self, id: DocumentId) -> Result<()> {
        let doc = self.storage.get_document(id)?;
        let removed = self.storage.delete_document(id)?;
        if let Some(ix) = self.indexes.write().get_mut(&doc.domain_id) {
            for cid in &removed {
                ix.remove(*cid);
            }
        }
        if let Some(li) = self.lexical.write().get_mut(&doc.domain_id) {
            for cid in &removed {
                li.remove(*cid);
            }
        }
        Ok(())
    }

    /// Rename a domain.
    pub fn rename_domain(&self, id: DomainId, name: &str) -> Result<Domain> {
        self.storage.rename_domain(id, name)
    }

    /// Delete a domain and everything under it, discarding its in-memory vector
    /// and lexical indexes.
    pub fn delete_domain(&self, id: DomainId) -> Result<()> {
        self.storage.delete_domain(id)?;
        self.indexes.write().remove(&id);
        self.lexical.write().remove(&id);
        Ok(())
    }

    /// Delete a subdomain, cascade-deleting its documents and dropping their
    /// chunks from the in-memory indexes.
    pub fn delete_subdomain(&self, id: SubdomainId) -> Result<()> {
        let sub = self.storage.get_subdomain(id)?;
        let removed = self.storage.delete_subdomain(id)?;
        if let Some(ix) = self.indexes.write().get_mut(&sub.domain_id) {
            for cid in &removed {
                ix.remove(*cid);
            }
        }
        if let Some(li) = self.lexical.write().get_mut(&sub.domain_id) {
            for cid in &removed {
                li.remove(*cid);
            }
        }
        Ok(())
    }

    /// Update a label's display name / description.
    pub fn update_tag(
        &self,
        id: TagId,
        display_name: Option<&str>,
        description: Option<&str>,
    ) -> Result<Tag> {
        self.storage.update_tag(id, display_name, description)
    }

    /// Delete a label, detaching it from chunks and documents (which survive).
    /// No index change: embeddings are untouched.
    pub fn delete_tag(&self, id: TagId) -> Result<()> {
        self.storage.delete_tag(id)
    }

    /// Re-assign a document's tags and/or subdomain, propagated to its chunks.
    /// The in-memory indexes are unaffected (only tags/subdomain change).
    pub fn update_document(
        &self,
        id: DocumentId,
        new_tags: Option<Vec<TagId>>,
        new_subdomain: Option<SubdomainId>,
        change_subdomain: bool,
    ) -> Result<Document> {
        self.storage
            .update_document(id, new_tags, new_subdomain, change_subdomain)
    }

    /// Whether the embedder knows `model` (used to validate a reindex request).
    pub fn supports_model(&self, model: &str) -> bool {
        self.embedder.dim(model).is_some()
    }

    /// Re-embed every chunk of a domain (optionally switching to `new_model`,
    /// which changes the dimension) and rebuild its vector index. **Blocking**
    /// (runs inference). The lexical index is untouched (texts are unchanged).
    /// Returns the number of chunks re-embedded.
    pub fn reindex_domain(&self, domain_id: DomainId, new_model: Option<&str>) -> Result<usize> {
        let domain = self.get_domain(domain_id)?;
        let model = new_model.unwrap_or(&domain.model).to_string();
        let dim = self
            .embedder
            .dim(&model)
            .ok_or_else(|| NucleusError::ModelNotFound(model.clone()))?;
        if model != domain.model || dim != domain.dim {
            self.storage.set_domain_model(domain_id, &model, dim)?;
        }
        // Re-embed all chunk texts in bounded windows, into a fresh index.
        const EMBED_BATCH: usize = 64;
        let texts = self.storage.texts_in_domain(domain_id)?;
        let mut new_index = build_index(self.index_kind, dim);
        let mut count = 0usize;
        for window in texts.chunks(EMBED_BATCH) {
            let inputs: Vec<String> = window.iter().map(|(_, t)| t.clone()).collect();
            let vectors = self.embedder.embed_documents(&model, &inputs)?;
            if vectors.len() != window.len() {
                return Err(NucleusError::embedding_msg(
                    "reindex: embedder returned a different number of vectors than inputs",
                ));
            }
            for ((cid, _), v) in window.iter().zip(&vectors) {
                self.storage.set_embedding(*cid, v)?;
                new_index.upsert(*cid, v)?;
                count += 1;
            }
        }
        // Swap the rebuilt index in for the domain.
        self.indexes.write().insert(domain_id, new_index);
        Ok(count)
    }

    // --- search ------------------------------------------------------------

    /// Retrieve the top-`k` chunks in a domain for a query, applying tag and
    /// document filters.
    pub fn search(&self, domain_id: DomainId, req: SearchRequest) -> Result<Vec<SearchHit>> {
        let domain = self.get_domain(domain_id)?;
        let SearchRequest {
            query,
            k,
            tags,
            match_all,
            document_ids,
            subdomain,
            filter,
            diversity,
        } = req;
        let k = k.min(MAX_K);
        let diversity = diversity.clamp(0.0, 1.0);
        let do_mmr = diversity > 0.0;

        let (query_vec, query_text) = match query {
            QueryInput::Vector(v) => {
                if v.len() != domain.dim {
                    return Err(NucleusError::DimensionMismatch {
                        expected: domain.dim,
                        got: v.len(),
                    });
                }
                (v, None)
            }
            QueryInput::Text(text) => {
                (self.embedder.embed_query(&domain.model, &text)?, Some(text))
            }
            QueryInput::Hybrid { text, vector } => {
                if vector.len() != domain.dim {
                    return Err(NucleusError::DimensionMismatch {
                        expected: domain.dim,
                        got: vector.len(),
                    });
                }
                (vector, Some(text))
            }
        };

        let allowed = self.candidate_set(&tags, match_all, &document_ids, subdomain)?;
        let allowed = self.apply_filter(domain_id, filter, allowed)?;

        // Hybrid retrieval: dense (vector) plus, when the query is text, BM25
        // lexical — fused with Reciprocal Rank Fusion (robust to score scales).
        let fetch = k.saturating_mul(5).max(50);
        let reranker = self.reranker.read().clone();
        let do_rerank = reranker.is_some() && query_text.is_some();
        // When reranking, re-score a bounded candidate window (the cross-encoder
        // is costly per pair): at least `k`, at most what we fetched. MMR also
        // needs a pool wider than `k` to have anything to diversify from.
        let window = if do_rerank {
            (*self.rerank_candidates.read()).clamp(k, fetch)
        } else if do_mmr {
            fetch
        } else {
            k
        };

        let vector_hits = {
            let idx = self.indexes.read();
            match idx.get(&domain_id) {
                Some(ix) => ix.search(&query_vec, fetch, allowed.as_ref()),
                None => Vec::new(),
            }
        };
        let ranked = match &query_text {
            Some(text) => {
                let lexical_hits = {
                    let lex = self.lexical.read();
                    match lex.get(&domain_id) {
                        Some(li) => li.search(text, fetch, allowed.as_ref()),
                        None => Vec::new(),
                    }
                };
                if lexical_hits.is_empty() {
                    let mut v = vector_hits;
                    v.truncate(window);
                    v
                } else {
                    rrf_fuse(&[vector_hits, lexical_hits], window)
                }
            }
            None => {
                let mut v = vector_hits;
                v.truncate(window);
                v
            }
        };

        // Load the candidate chunks (best-effort; skip any deleted since ranking).
        let mut items: Vec<(Chunk, f32)> = Vec::with_capacity(ranked.len());
        for (cid, score) in ranked {
            if let Ok(chunk) = self.storage.get_chunk(cid) {
                items.push((chunk, score));
            }
        }

        // For MMR we need the candidate embeddings; fetch them (aligned by chunk
        // id) before the window is consumed by reranking.
        let cand_embs: HashMap<ChunkId, Vec<f32>> = if do_mmr {
            items
                .iter()
                .filter_map(|(c, _)| self.storage.get_embedding(c.id).ok().map(|e| (c.id, e)))
                .collect()
        } else {
            HashMap::new()
        };

        // Stage 2: optional cross-encoder rerank, which re-scores the window.
        let mut scored: Vec<(Chunk, f32)> = match (do_rerank, reranker, &query_text) {
            (true, Some(reranker), Some(text)) => {
                let docs: Vec<String> = items.iter().map(|(c, _)| c.text.clone()).collect();
                let scores = reranker.rerank(text, &docs)?;
                items
                    .into_iter()
                    .zip(scores)
                    .map(|((chunk, _), score)| (chunk, score))
                    .collect()
            }
            _ => items,
        };
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));

        // Stage 3: either diversify with MMR or take a plain top-`k`.
        let selected = if do_mmr {
            mmr_select(scored, &cand_embs, 1.0 - diversity, k)
        } else {
            scored.truncate(k);
            scored
        };
        let qt = query_text.as_deref();
        Ok(selected
            .into_iter()
            .map(|(chunk, score)| {
                let snippet = qt.and_then(|q| snippet(&chunk.text, q, 120));
                SearchHit {
                    chunk,
                    score,
                    snippet,
                }
            })
            .collect())
    }

    /// Search several domains of the **same model/dimension** in one call,
    /// merging their results by score into the global top-`k`. Per-domain id
    /// filters (tags, document_ids, subdomain) don't generalise across domains,
    /// so only `query`, `k`, `filter` (by tag name) and `diversity` are honoured.
    pub fn search_multi(
        &self,
        domain_ids: &[DomainId],
        req: SearchRequest,
    ) -> Result<Vec<SearchHit>> {
        if domain_ids.is_empty() {
            return Ok(Vec::new());
        }
        // All domains must agree on model/dimension.
        let mut model: Option<(String, usize)> = None;
        for id in domain_ids {
            let d = self.get_domain(*id)?;
            match &model {
                None => model = Some((d.model.clone(), d.dim)),
                Some((m, dim)) => {
                    if *m != d.model || *dim != d.dim {
                        return Err(NucleusError::invalid(
                            "multi-domain search requires all domains to share the same model",
                        ));
                    }
                }
            }
        }
        let (model, _dim) = model.expect("non-empty domain_ids");

        let SearchRequest {
            query,
            k,
            filter,
            diversity,
            ..
        } = req;
        // Materialise the query once (embed any text query a single time).
        let (text, vector) = match query {
            QueryInput::Text(t) => {
                let v = self.embedder.embed_query(&model, &t)?;
                (Some(t), v)
            }
            QueryInput::Vector(v) => (None, v),
            QueryInput::Hybrid { text, vector } => (Some(text), vector),
        };

        let mut all: Vec<SearchHit> = Vec::new();
        for id in domain_ids {
            let q = match &text {
                Some(t) => QueryInput::Hybrid {
                    text: t.clone(),
                    vector: vector.clone(),
                },
                None => QueryInput::Vector(vector.clone()),
            };
            all.extend(self.search(
                *id,
                SearchRequest {
                    query: q,
                    k,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: filter.clone(),
                    diversity,
                },
            )?);
        }
        all.sort_by(|a, b| b.score.total_cmp(&a.score));
        all.truncate(k.min(MAX_K));
        Ok(all)
    }

    // --- tokens / auth -----------------------------------------------------

    /// Validate a bearer token, returning the caller's [`AuthContext`].
    pub fn authenticate(&self, bearer: &str) -> Result<AuthContext> {
        let hash = auth::hash_token(bearer);
        let token = self
            .storage
            .get_token_by_hash(&hash)?
            .ok_or(NucleusError::Unauthorized)?;
        if let Some(exp) = token.expires_at {
            if now_millis() > exp {
                return Err(NucleusError::Unauthorized);
            }
        }
        // Record last-used (in memory; cheap, no disk write on the hot path).
        self.last_used.write().insert(token.id, now_millis());
        Ok(AuthContext {
            token_id: token.id,
            scopes: token.scopes,
        })
    }

    /// Last time a token authenticated successfully (in-memory; `None` if it
    /// hasn't been used since the server started).
    pub fn token_last_used(&self, id: TokenId) -> Option<i64> {
        self.last_used.read().get(&id).copied()
    }

    /// Create a token, returning the stored record and the plaintext (shown once).
    pub fn create_token(
        &self,
        name: &str,
        scopes: Vec<Scope>,
        expires_at: Option<i64>,
    ) -> Result<(ApiToken, String)> {
        let (plaintext, hash) = auth::generate_token();
        let token = self.storage.create_token(name, hash, scopes, expires_at)?;
        Ok((token, plaintext))
    }

    /// Rotate a token's secret: mint a fresh plaintext/hash for the same id,
    /// scopes and expiry, invalidating the old secret. Returns the updated record
    /// and the new plaintext (shown once).
    pub fn rotate_token(&self, id: TokenId) -> Result<(ApiToken, String)> {
        let (plaintext, hash) = auth::generate_token();
        match self.storage.rotate_token(id, hash)? {
            Some(token) => {
                self.last_used.write().remove(&id);
                Ok((token, plaintext))
            }
            None => Err(NucleusError::invalid("token not found")),
        }
    }

    pub fn list_tokens(&self) -> Result<Vec<ApiToken>> {
        self.storage.list_tokens()
    }

    pub fn delete_token(&self, id: TokenId) -> Result<bool> {
        self.storage.delete_token(id)
    }

    /// If no tokens exist yet, mint a global-admin bootstrap token and return its
    /// plaintext (to be printed once at startup). Returns `None` otherwise.
    pub fn bootstrap_admin_token(&self) -> Result<Option<String>> {
        if self.storage.count_tokens()? > 0 {
            return Ok(None);
        }
        let (_token, plaintext) =
            self.create_token("bootstrap-admin", vec![Scope::admin_all()], None)?;
        Ok(Some(plaintext))
    }

    /// Combine tag and document filters into a single allow-set, or `None` for
    /// "no restriction".
    fn candidate_set(
        &self,
        tags: &[TagId],
        match_all: bool,
        document_ids: &[DocumentId],
        subdomain: Option<SubdomainId>,
    ) -> Result<Option<HashSet<ChunkId>>> {
        let mut acc: Option<HashSet<ChunkId>> = None;
        let mut fold = |set: HashSet<ChunkId>| {
            acc = Some(match acc.take() {
                None => set,
                Some(prev) => prev.intersection(&set).copied().collect(),
            });
        };
        if let Some(by_tag) = self.storage.candidates_for_tags(tags, match_all)? {
            fold(by_tag);
        }
        if !document_ids.is_empty() {
            fold(self.storage.chunk_ids_for_documents(document_ids)?);
        }
        if let Some(sid) = subdomain {
            fold(self.storage.candidates_for_subdomain(sid)?);
        }
        Ok(acc)
    }

    /// Apply an optional query-language filter, intersecting it with `base`.
    ///
    /// The predicate is resolved with **set algebra over the secondary indexes**
    /// (tag/doc/meta lookups combined with ∩/∪/∖) rather than scanning and
    /// decoding every chunk. The domain's chunk-id set is used as the universe so
    /// `NOT` is well-defined.
    fn apply_filter(
        &self,
        domain_id: DomainId,
        filter: Option<String>,
        base: Option<HashSet<ChunkId>>,
    ) -> Result<Option<HashSet<ChunkId>>> {
        let Some(filter) = filter else {
            return Ok(base);
        };
        let expr = crate::query::parse(&filter)?;
        let names = self.tag_name_map(domain_id)?;
        let universe: HashSet<ChunkId> = self
            .storage
            .chunk_ids_in_domain(domain_id)?
            .into_iter()
            .collect();
        let matched = self.eval_filter(&expr, &names, &universe)?;
        Ok(Some(match base {
            Some(base) => &matched & &base,
            None => matched,
        }))
    }

    /// Resolve a filter [`Expr`](crate::query::Expr) to the set of matching chunk
    /// ids using the secondary indexes.
    fn eval_filter(
        &self,
        expr: &crate::query::Expr,
        names: &HashMap<String, TagId>,
        universe: &HashSet<ChunkId>,
    ) -> Result<HashSet<crate::id::ChunkId>> {
        use crate::query::Expr;
        Ok(match expr {
            Expr::And(a, b) => {
                &self.eval_filter(a, names, universe)? & &self.eval_filter(b, names, universe)?
            }
            Expr::Or(a, b) => {
                &self.eval_filter(a, names, universe)? | &self.eval_filter(b, names, universe)?
            }
            Expr::Not(a) => universe - &self.eval_filter(a, names, universe)?,
            Expr::Tag(name) => match names.get(name) {
                Some(id) => self
                    .storage
                    .candidates_for_tags(&[*id], false)?
                    .unwrap_or_default(),
                None => HashSet::new(),
            },
            Expr::Doc(id) => self
                .storage
                .chunk_ids_for_documents(&[DocumentId::new(*id)])?,
            Expr::Meta(key, value) => self.storage.candidates_for_meta(key, value)?,
        })
    }

    fn tag_name_map(&self, domain_id: DomainId) -> Result<HashMap<String, TagId>> {
        Ok(self
            .storage
            .list_tags(domain_id)?
            .into_iter()
            .map(|t| (t.name, t.id))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedder;

    fn engine() -> (Engine, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path().join("n.redb")).unwrap();
        let embedder = Arc::new(MockEmbedder::new(64));
        (Engine::new(storage, embedder).unwrap(), dir)
    }

    #[test]
    fn ingest_then_search_returns_relevant_chunk() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();

        e.ingest_document(
            dom.id,
            "doc",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec![
                "el contrato laboral indefinido".into(),
                "la receta de la pizza con piña".into(),
            ]),
        )
        .unwrap();

        let hits = e
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato laboral".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].chunk.text.contains("contrato"));
    }

    #[test]
    fn search_respects_tag_filter() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        let legal = e.create_tag(dom.id, "legal", "Legal", "", None).unwrap();

        e.ingest_document(
            dom.id,
            "tagged",
            None,
            BTreeMap::new(),
            vec![legal.id],
            IngestBody::Chunks(vec!["contrato laboral".into()]),
        )
        .unwrap();
        e.ingest_document(
            dom.id,
            "untagged",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["contrato mercantil".into()]),
        )
        .unwrap();

        let hits = e
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 10,
                    tags: vec![legal.id],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].chunk.tags.contains(&legal.id));
    }

    #[test]
    fn search_with_query_language_filter() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        let legal = e.create_tag(dom.id, "legal", "Legal", "", None).unwrap();
        e.ingest_document(
            dom.id,
            "a",
            None,
            BTreeMap::new(),
            vec![legal.id],
            IngestBody::Chunks(vec!["contrato laboral".into()]),
        )
        .unwrap();
        e.ingest_document(
            dom.id,
            "b",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["contrato mercantil".into()]),
        )
        .unwrap();

        let with_tag = |filter: &str| {
            e.search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 10,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: Some(filter.to_string()),
                    diversity: 0.0,
                },
            )
            .unwrap()
        };

        let legal_hits = with_tag("tag:legal");
        assert_eq!(legal_hits.len(), 1);
        assert!(legal_hits[0].chunk.tags.contains(&legal.id));

        let not_legal = with_tag("NOT tag:legal");
        assert_eq!(not_legal.len(), 1);
        assert!(!not_legal[0].chunk.tags.contains(&legal.id));

        // A malformed filter surfaces as an error.
        assert!(e
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("x".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: Some("doc:abc".into()),
                    diversity: 0.0,
                },
            )
            .is_err());
    }

    #[test]
    fn search_filter_by_metadata() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        let mut es = BTreeMap::new();
        es.insert("lang".to_string(), "es".to_string());
        let mut en = BTreeMap::new();
        en.insert("lang".to_string(), "en".to_string());
        e.ingest_document(
            dom.id,
            "a",
            None,
            es,
            vec![],
            IngestBody::Chunks(vec!["contrato".into()]),
        )
        .unwrap();
        e.ingest_document(
            dom.id,
            "b",
            None,
            en,
            vec![],
            IngestBody::Chunks(vec!["contract".into()]),
        )
        .unwrap();

        let run = |filter: &str| {
            e.search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 10,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: Some(filter.to_string()),
                    diversity: 0.0,
                },
            )
            .unwrap()
        };

        let es_hits = run("meta.lang:es");
        assert_eq!(es_hits.len(), 1);
        assert_eq!(
            es_hits[0].chunk.metadata.get("lang").map(String::as_str),
            Some("es")
        );

        let not_es = run("NOT meta.lang:es");
        assert_eq!(not_es.len(), 1);
        assert_eq!(
            not_es[0].chunk.metadata.get("lang").map(String::as_str),
            Some("en")
        );
    }

    #[test]
    fn chunk_context_walks_neighbors() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        e.ingest_document(
            dom.id,
            "d",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["c0".into(), "c1".into(), "c2".into(), "c3".into()]),
        )
        .unwrap();

        let mut ids = e.storage().chunk_ids_in_domain(dom.id).unwrap();
        ids.sort_by_key(|c| c.get());

        let ctx = e.chunk_context(ids[1], 1, 1).unwrap();
        assert_eq!(
            ctx.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(),
            vec!["c0", "c1", "c2"]
        );

        // Edges clamp instead of failing.
        let head = e.chunk_context(ids[0], 5, 1).unwrap();
        assert_eq!(head[0].text, "c0");
        assert_eq!(head.len(), 2);
    }

    #[test]
    fn hybrid_search_finds_exact_term() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        e.ingest_document(
            dom.id,
            "a",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["el artículo 14 regula las vacaciones".into()]),
        )
        .unwrap();
        e.ingest_document(
            dom.id,
            "b",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["disposiciones generales del convenio".into()]),
        )
        .unwrap();

        let hits = e
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("artículo 14".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].chunk.text.contains("artículo 14"));
    }

    #[test]
    fn reranker_reorders_results() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        e.ingest_document(
            dom.id,
            "a",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["contrato laboral indefinido".into()]),
        )
        .unwrap();
        e.ingest_document(
            dom.id,
            "b",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["contrato mercantil".into()]),
        )
        .unwrap();
        e.set_reranker(std::sync::Arc::new(crate::rerank::MockReranker));

        let hits = e
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato laboral".into()),
                    k: 2,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].chunk.text.contains("laboral"));
    }

    #[test]
    fn mmr_diversifies_results() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        e.ingest_document(
            dom.id,
            "d",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec![
                // Two highly-relevant near-duplicates plus one distinct chunk.
                "contrato laboral indefinido".into(),
                "contrato laboral temporal".into(),
                "contrato de obras".into(),
            ]),
        )
        .unwrap();

        let run = |diversity: f32| {
            e.search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato laboral".into()),
                    k: 2,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity,
                },
            )
            .unwrap()
        };

        // Pure relevance keeps the two near-duplicates (both share "laboral").
        let plain = run(0.0);
        assert_eq!(plain.len(), 2);
        assert!(!plain.iter().any(|h| h.chunk.text.contains("obras")));

        // High diversity promotes the distinct chunk into the top-2.
        let diverse = run(1.0);
        assert_eq!(diverse.len(), 2);
        assert!(diverse.iter().any(|h| h.chunk.text.contains("obras")));
    }

    #[test]
    fn search_returns_snippet() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        let long = format!(
            "{}contrato laboral indefinido {}",
            "a ".repeat(200),
            "b ".repeat(200)
        );
        e.ingest_document(
            dom.id,
            "d",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec![long]),
        )
        .unwrap();
        let hits = e
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        let snip = hits[0].snippet.as_ref().expect("snippet present");
        assert!(snip.contains("contrato"));
        // It's a trimmed excerpt of the (much longer) chunk, elided with `…`.
        assert!(snip.chars().count() < hits[0].chunk.text.chars().count());
        assert!(snip.starts_with('…') || snip.ends_with('…'));
    }

    #[test]
    fn multi_domain_search_merges_same_model() {
        let (e, _d) = engine();
        let a = e.create_domain("a", None).unwrap();
        let b = e.create_domain("b", None).unwrap();
        e.ingest_document(
            a.id,
            "da",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["contrato laboral en A".into()]),
        )
        .unwrap();
        e.ingest_document(
            b.id,
            "db",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["contrato laboral en B".into()]),
        )
        .unwrap();
        let hits = e
            .search_multi(
                &[a.id, b.id],
                SearchRequest {
                    query: QueryInput::Text("contrato laboral".into()),
                    k: 10,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 2);
        let domains: HashSet<DomainId> = hits.iter().map(|h| h.chunk.domain_id).collect();
        assert!(domains.contains(&a.id) && domains.contains(&b.id));
    }

    #[test]
    fn update_document_retags_and_moves_subdomain() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        let a = e.create_tag(dom.id, "a", "A", "", None).unwrap();
        let b = e.create_tag(dom.id, "b", "B", "", None).unwrap();
        let out = e
            .ingest_document(
                dom.id,
                "d",
                None,
                BTreeMap::new(),
                vec![a.id],
                IngestBody::Chunks(vec!["contrato laboral".into()]),
            )
            .unwrap();
        let sub = e.get_or_create_subdomain(dom.id, "irpf", "").unwrap();

        e.update_document(out.document.id, Some(vec![b.id]), Some(sub.id), true)
            .unwrap();

        let by = |tags: Vec<TagId>, subdomain: Option<SubdomainId>| {
            e.search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 5,
                    tags,
                    match_all: false,
                    document_ids: vec![],
                    subdomain,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap()
        };
        assert_eq!(by(vec![b.id], None).len(), 1, "retagged to b");
        assert!(by(vec![a.id], None).is_empty(), "old tag a detached");
        assert_eq!(by(vec![], Some(sub.id)).len(), 1, "moved into subdomain");
        // The document row reflects the change too.
        let doc = e.get_document(out.document.id).unwrap();
        assert_eq!(doc.tags, vec![b.id]);
        assert_eq!(doc.subdomain_id, Some(sub.id));
    }

    #[test]
    fn reindex_reembeds_and_updates_model() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        e.ingest_document(
            dom.id,
            "d",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["contrato laboral".into(), "pizza con piña".into()]),
        )
        .unwrap();

        let n = e.reindex_domain(dom.id, Some("bge-small-en-v1.5")).unwrap();
        assert_eq!(n, 2);
        assert_eq!(e.get_domain(dom.id).unwrap().model, "bge-small-en-v1.5");

        // Search still works against the rebuilt index.
        let hits = e
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].chunk.text.contains("contrato"));
    }

    #[test]
    fn rotate_token_invalidates_old_secret() {
        let (e, _d) = engine();
        let (tok, plain) = e.create_token("t", vec![Scope::admin_all()], None).unwrap();
        assert!(e.authenticate(&plain).is_ok());
        assert!(e.token_last_used(tok.id).is_some());

        let (tok2, plain2) = e.rotate_token(tok.id).unwrap();
        assert_eq!(tok2.id, tok.id, "same id, new secret");
        assert!(e.authenticate(&plain2).is_ok(), "new secret works");
        assert!(
            matches!(e.authenticate(&plain), Err(NucleusError::Unauthorized)),
            "old secret is rejected"
        );
    }

    #[test]
    fn dedupe_by_content_hash() {
        let (e, _d) = engine();
        let dom = e.create_domain("docs", None).unwrap();
        let out = e
            .ingest_document(
                dom.id,
                "d",
                None,
                BTreeMap::new(),
                vec![],
                IngestBody::Chunks(vec!["hola".into()]),
            )
            .unwrap();
        e.set_document_hash(dom.id, out.document.id, "abc123")
            .unwrap();
        assert_eq!(
            e.find_document_by_hash(dom.id, "abc123").unwrap(),
            Some(out.document.id)
        );
        assert_eq!(e.find_document_by_hash(dom.id, "nope").unwrap(), None);

        // A deleted document leaves a stale index entry; treat it as a miss.
        e.delete_document(out.document.id).unwrap();
        assert_eq!(e.find_document_by_hash(dom.id, "abc123").unwrap(), None);
    }

    #[test]
    fn token_bootstrap_and_auth() {
        let (e, _d) = engine();
        let plaintext = e.bootstrap_admin_token().unwrap().unwrap();
        // Tokens now exist, so a second bootstrap is a no-op.
        assert!(e.bootstrap_admin_token().unwrap().is_none());

        let ctx = e.authenticate(&plaintext).unwrap();
        assert!(ctx.is_admin());
        assert!(matches!(
            e.authenticate("nuc_bogus"),
            Err(NucleusError::Unauthorized)
        ));
    }

    #[test]
    fn search_with_hnsw_backend() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path().join("n.redb")).unwrap();
        let e = Engine::with_index_kind(
            storage,
            Arc::new(MockEmbedder::new(64)),
            crate::index::IndexKind::Hnsw,
        )
        .unwrap();
        let dom = e.create_domain("docs", None).unwrap();
        e.ingest_document(
            dom.id,
            "d",
            None,
            BTreeMap::new(),
            vec![],
            IngestBody::Chunks(vec!["el contrato laboral".into(), "pizza con piña".into()]),
        )
        .unwrap();

        let hits = e
            .search(
                dom.id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].chunk.text.contains("contrato"));
    }

    #[test]
    fn hnsw_index_persists_across_reopen() {
        use crate::index::IndexKind;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("n.redb");
        let idx_dir = dir.path().join("idx");
        let dom_id;
        {
            let storage = Storage::open(&db).unwrap();
            let e = Engine::open(
                storage,
                Arc::new(MockEmbedder::new(64)),
                IndexKind::Hnsw,
                Some(idx_dir.clone()),
            )
            .unwrap();
            let dom = e.create_domain("docs", None).unwrap();
            dom_id = dom.id;
            e.ingest_document(
                dom.id,
                "d",
                None,
                BTreeMap::new(),
                vec![],
                IngestBody::Chunks(vec!["contrato laboral".into()]),
            )
            .unwrap();
            assert_eq!(e.persist_indexes().unwrap(), 1);
        }
        // Reopen with the same index dir: the HNSW graph is loaded from disk.
        let storage = Storage::open(&db).unwrap();
        let e = Engine::open(
            storage,
            Arc::new(MockEmbedder::new(64)),
            IndexKind::Hnsw,
            Some(idx_dir),
        )
        .unwrap();
        let hits = e
            .search(
                dom_id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn index_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("n.redb");
        let dom_id;
        {
            let storage = Storage::open(&path).unwrap();
            let e = Engine::new(storage, Arc::new(MockEmbedder::new(64))).unwrap();
            let dom = e.create_domain("docs", None).unwrap();
            dom_id = dom.id;
            e.ingest_document(
                dom.id,
                "d",
                None,
                BTreeMap::new(),
                vec![],
                IngestBody::Chunks(vec!["contrato laboral".into()]),
            )
            .unwrap();
        }
        // Reopen: the index must be rebuilt from storage.
        let storage = Storage::open(&path).unwrap();
        let e = Engine::new(storage, Arc::new(MockEmbedder::new(64))).unwrap();
        let hits = e
            .search(
                dom_id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 1,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                    diversity: 0.0,
                },
            )
            .unwrap();
        assert_eq!(hits.len(), 1);
    }
}
