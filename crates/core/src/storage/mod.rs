//! Durable storage on top of [`redb`] (embedded, ACID, pure-Rust KV).
//!
//! Keys are the raw `u64` of our typed ids; values are bincode-encoded entities
//! (see [`codec`]). Alongside the primary tables we keep multimap secondary
//! indexes so we can answer "all chunks in a domain", "all chunks for a tag",
//! etc. without scanning. Every mutating method runs in a single write
//! transaction, so multi-table updates are atomic.

pub mod codec;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use redb::{
    Database, MultimapTableDefinition, ReadableMultimapTable, ReadableTable, ReadableTableMetadata,
    TableDefinition,
};

use crate::auth::{ApiToken, Scope};
use crate::error::NucleusError;
use crate::id::{ChunkId, DocumentId, DomainId, JobId, SubdomainId, TagId, TokenId};
use crate::jobs::{Job, JobKind, JobStatus};
use crate::model::{Chunk, Document, Domain, Subdomain, Tag};
use crate::util::now_millis;
use crate::Result;

// Primary tables: id -> bincode(entity).
const DOMAINS: TableDefinition<u64, &[u8]> = TableDefinition::new("domains");
const DOCUMENTS: TableDefinition<u64, &[u8]> = TableDefinition::new("documents");
const CHUNKS: TableDefinition<u64, &[u8]> = TableDefinition::new("chunks");
const EMBEDDINGS: TableDefinition<u64, &[u8]> = TableDefinition::new("embeddings");
const TAGS: TableDefinition<u64, &[u8]> = TableDefinition::new("tags");
// token hash -> bincode(ApiToken).
const TOKENS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("tokens");
// job id -> bincode(Job).
const JOBS: TableDefinition<u64, &[u8]> = TableDefinition::new("jobs");
// set of pending job ids (key only), so claiming doesn't scan all jobs.
const JOBS_PENDING: TableDefinition<u64, ()> = TableDefinition::new("jobs_pending");
// name -> monotonically increasing counter, used to allocate ids.
const SEQ: TableDefinition<&str, u64> = TableDefinition::new("seq");
// engine metadata (e.g. schema_version) for migrations.
const META: TableDefinition<&str, u64> = TableDefinition::new("meta");

/// On-disk schema version. Bump when the layout changes and add a migration.
const SCHEMA_VERSION: u64 = 1;

// Secondary indexes (multimap): parent id -> child id.
const DOCS_BY_DOMAIN: MultimapTableDefinition<u64, u64> =
    MultimapTableDefinition::new("docs_by_domain");
const CHUNKS_BY_DOMAIN: MultimapTableDefinition<u64, u64> =
    MultimapTableDefinition::new("chunks_by_domain");
const CHUNKS_BY_DOC: MultimapTableDefinition<u64, u64> =
    MultimapTableDefinition::new("chunks_by_doc");
const CHUNKS_BY_TAG: MultimapTableDefinition<u64, u64> =
    MultimapTableDefinition::new("chunks_by_tag");
const TAGS_BY_DOMAIN: MultimapTableDefinition<u64, u64> =
    MultimapTableDefinition::new("tags_by_domain");
// "key\u{1f}value" -> chunk id, so metadata filters are index lookups not scans.
const CHUNKS_BY_META: MultimapTableDefinition<&str, u64> =
    MultimapTableDefinition::new("chunks_by_meta");
// subdomain id -> bincode(Subdomain), plus its by-domain and by-name indexes.
const SUBDOMAINS: TableDefinition<u64, &[u8]> = TableDefinition::new("subdomains");
const SUBDOMAINS_BY_DOMAIN: MultimapTableDefinition<u64, u64> =
    MultimapTableDefinition::new("subdomains_by_domain");
const CHUNKS_BY_SUBDOMAIN: MultimapTableDefinition<u64, u64> =
    MultimapTableDefinition::new("chunks_by_subdomain");
// "domain\u{1f}name" -> id, for get-or-create-by-name of subdomains and tags.
const SUBDOMAIN_IDS: TableDefinition<&str, u64> = TableDefinition::new("subdomain_ids");
const TAG_IDS: TableDefinition<&str, u64> = TableDefinition::new("tag_ids");
// "domain\u{1f}content_hash" -> document id, for ingest deduplication.
const DOCS_BY_HASH: TableDefinition<&str, u64> = TableDefinition::new("docs_by_hash");

/// Handle to the on-disk database. Cheap to share behind an `Arc`; all methods
/// take `&self`.
pub struct Storage {
    db: Database,
    path: PathBuf,
}

/// A chunk to persist in a batch: its text and its embedding vector.
pub struct NewChunk<'a> {
    pub text: &'a str,
    pub embedding: &'a [f32],
}

impl Storage {
    /// Open (creating if needed) the database at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let db = Database::create(&path)?;
        let storage = Self { db, path };
        storage.init_tables()?;
        storage.check_schema_version()?;
        Ok(storage)
    }

    /// Filesystem path of the database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write a **consistent** snapshot of the database to a fresh redb file at
    /// `dst`. This is a *logical* copy (read transaction → new database, table by
    /// table) rather than a raw file copy, because redb holds an OS byte-range
    /// lock on the live file that would make `fs::copy` fail on Windows. The read
    /// transaction gives a consistent MVCC view, so the snapshot is point-in-time.
    pub fn backup_to(&self, dst: impl AsRef<Path>) -> Result<()> {
        let dst = dst.as_ref();
        if dst.exists() {
            std::fs::remove_file(dst)?;
        }
        let src = self.db.begin_read()?;
        let out = Database::create(dst)?;
        let wtx = out.begin_write()?;
        {
            // Copy a primary table (any typed key/value) entry by entry.
            macro_rules! copy_table {
                ($def:expr) => {{
                    let s = src.open_table($def)?;
                    let mut d = wtx.open_table($def)?;
                    for entry in s.iter()? {
                        let (k, v) = entry?;
                        d.insert(k.value(), v.value())?;
                    }
                }};
            }
            // Copy a multimap table (key -> many values).
            macro_rules! copy_multimap {
                ($def:expr) => {{
                    let s = src.open_multimap_table($def)?;
                    let mut d = wtx.open_multimap_table($def)?;
                    for entry in s.iter()? {
                        let (k, vals) = entry?;
                        for v in vals {
                            d.insert(k.value(), v?.value())?;
                        }
                    }
                }};
            }

            copy_table!(DOMAINS);
            copy_table!(DOCUMENTS);
            copy_table!(CHUNKS);
            copy_table!(EMBEDDINGS);
            copy_table!(TAGS);
            copy_table!(TOKENS);
            copy_table!(JOBS);
            copy_table!(JOBS_PENDING);
            copy_table!(SEQ);
            copy_table!(META);
            copy_table!(SUBDOMAINS);
            copy_table!(SUBDOMAIN_IDS);
            copy_table!(TAG_IDS);
            copy_table!(DOCS_BY_HASH);

            copy_multimap!(DOCS_BY_DOMAIN);
            copy_multimap!(CHUNKS_BY_DOMAIN);
            copy_multimap!(CHUNKS_BY_DOC);
            copy_multimap!(CHUNKS_BY_TAG);
            copy_multimap!(TAGS_BY_DOMAIN);
            copy_multimap!(CHUNKS_BY_META);
            copy_multimap!(SUBDOMAINS_BY_DOMAIN);
            copy_multimap!(CHUNKS_BY_SUBDOMAIN);
        }
        wtx.commit()?;
        Ok(())
    }

    /// Stamp/verify the schema version, running migrations if the on-disk version
    /// is older. Refuses to open a database written by a newer Nucleus.
    fn check_schema_version(&self) -> Result<()> {
        let current = {
            let rtx = self.db.begin_read()?;
            let t = rtx.open_table(META)?;
            t.get("schema_version")?.map(|g| g.value())
        };
        match current {
            Some(v) if v == SCHEMA_VERSION => return Ok(()),
            Some(v) if v > SCHEMA_VERSION => {
                return Err(NucleusError::invalid(format!(
                    "database schema v{v} is newer than supported v{SCHEMA_VERSION}; upgrade Nucleus"
                )));
            }
            Some(v) => self.migrate(v, SCHEMA_VERSION)?, // v < current
            None => {}                                   // fresh database
        }
        let wtx = self.db.begin_write()?;
        {
            let mut t = wtx.open_table(META)?;
            t.insert("schema_version", SCHEMA_VERSION)?;
        }
        wtx.commit()?;
        Ok(())
    }

    /// Apply migrations from `from` to `to`. Adding new tables/indexes is handled
    /// by `init_tables` (forward-compatible), so v1 needs no data migration.
    fn migrate(&self, _from: u64, _to: u64) -> Result<()> {
        Ok(())
    }

    /// Materialise every table once, so later read transactions never hit
    /// `TableDoesNotExist` on a fresh database.
    fn init_tables(&self) -> Result<()> {
        let wtx = self.db.begin_write()?;
        wtx.open_table(DOMAINS)?;
        wtx.open_table(DOCUMENTS)?;
        wtx.open_table(CHUNKS)?;
        wtx.open_table(EMBEDDINGS)?;
        wtx.open_table(TAGS)?;
        wtx.open_table(TOKENS)?;
        wtx.open_table(JOBS)?;
        wtx.open_table(JOBS_PENDING)?;
        wtx.open_table(SEQ)?;
        wtx.open_table(META)?;
        wtx.open_multimap_table(DOCS_BY_DOMAIN)?;
        wtx.open_multimap_table(CHUNKS_BY_DOMAIN)?;
        wtx.open_multimap_table(CHUNKS_BY_DOC)?;
        wtx.open_multimap_table(CHUNKS_BY_TAG)?;
        wtx.open_multimap_table(TAGS_BY_DOMAIN)?;
        wtx.open_multimap_table(CHUNKS_BY_META)?;
        wtx.open_table(SUBDOMAINS)?;
        wtx.open_multimap_table(SUBDOMAINS_BY_DOMAIN)?;
        wtx.open_multimap_table(CHUNKS_BY_SUBDOMAIN)?;
        wtx.open_table(SUBDOMAIN_IDS)?;
        wtx.open_table(TAG_IDS)?;
        wtx.open_table(DOCS_BY_HASH)?;
        wtx.commit()?;
        Ok(())
    }

    // --- domains -----------------------------------------------------------

    /// Create a domain pinned to `model`/`dim`.
    pub fn create_domain(&self, name: &str, model: &str, dim: usize) -> Result<Domain> {
        let wtx = self.db.begin_write()?;
        let id = {
            let mut seq = wtx.open_table(SEQ)?;
            DomainId::new(next_seq(&mut seq, "domain")?)
        };
        let domain = Domain {
            id,
            name: name.to_string(),
            model: model.to_string(),
            dim,
            created_at: now_millis(),
        };
        {
            let mut t = wtx.open_table(DOMAINS)?;
            t.insert(id.get(), codec::encode(&domain)?.as_slice())?;
        }
        wtx.commit()?;
        Ok(domain)
    }

    pub fn get_domain(&self, id: DomainId) -> Result<Domain> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(DOMAINS)?;
        let bytes = t.get(id.get())?.ok_or(NucleusError::DomainNotFound(id))?;
        codec::decode(bytes.value())
    }

    pub fn list_domains(&self) -> Result<Vec<Domain>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(DOMAINS)?;
        let mut out = Vec::new();
        for entry in t.iter()? {
            let (_, v) = entry?;
            out.push(codec::decode::<Domain>(v.value())?);
        }
        Ok(out)
    }

    // --- tags --------------------------------------------------------------

    pub fn create_tag(
        &self,
        domain_id: DomainId,
        name: &str,
        display_name: &str,
        description: &str,
        parent: Option<TagId>,
    ) -> Result<Tag> {
        // Validate the domain exists up front.
        self.get_domain(domain_id)?;
        let wtx = self.db.begin_write()?;
        let id = {
            let mut seq = wtx.open_table(SEQ)?;
            TagId::new(next_seq(&mut seq, "tag")?)
        };
        let tag = Tag {
            id,
            domain_id,
            name: name.to_string(),
            display_name: display_name.to_string(),
            description: description.to_string(),
            parent,
            created_at: now_millis(),
        };
        {
            let mut t = wtx.open_table(TAGS)?;
            t.insert(id.get(), codec::encode(&tag)?.as_slice())?;
        }
        {
            let mut idx = wtx.open_multimap_table(TAGS_BY_DOMAIN)?;
            idx.insert(domain_id.get(), id.get())?;
        }
        {
            let mut names = wtx.open_table(TAG_IDS)?;
            names.insert(name_key(domain_id, name).as_str(), id.get())?;
        }
        wtx.commit()?;
        Ok(tag)
    }

    /// Resolve a tag id by name within a domain.
    pub fn tag_id_by_name(&self, domain_id: DomainId, name: &str) -> Result<Option<TagId>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(TAG_IDS)?;
        Ok(t.get(name_key(domain_id, name).as_str())?
            .map(|g| TagId::new(g.value())))
    }

    /// Get the tag (label) with this name in the domain, creating it if absent.
    pub fn get_or_create_tag(&self, domain_id: DomainId, name: &str) -> Result<Tag> {
        if let Some(id) = self.tag_id_by_name(domain_id, name)? {
            return self.get_tag(id);
        }
        self.create_tag(domain_id, name, name, "", None)
    }

    // --- subdomains --------------------------------------------------------

    pub fn create_subdomain(
        &self,
        domain_id: DomainId,
        name: &str,
        description: &str,
    ) -> Result<Subdomain> {
        self.get_domain(domain_id)?;
        let wtx = self.db.begin_write()?;
        let id = {
            let mut seq = wtx.open_table(SEQ)?;
            SubdomainId::new(next_seq(&mut seq, "subdomain")?)
        };
        let sub = Subdomain {
            id,
            domain_id,
            name: name.to_string(),
            description: description.to_string(),
            created_at: now_millis(),
        };
        {
            let mut t = wtx.open_table(SUBDOMAINS)?;
            t.insert(id.get(), codec::encode(&sub)?.as_slice())?;
        }
        {
            let mut idx = wtx.open_multimap_table(SUBDOMAINS_BY_DOMAIN)?;
            idx.insert(domain_id.get(), id.get())?;
        }
        {
            let mut names = wtx.open_table(SUBDOMAIN_IDS)?;
            names.insert(name_key(domain_id, name).as_str(), id.get())?;
        }
        wtx.commit()?;
        Ok(sub)
    }

    pub fn subdomain_id_by_name(
        &self,
        domain_id: DomainId,
        name: &str,
    ) -> Result<Option<SubdomainId>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(SUBDOMAIN_IDS)?;
        Ok(t.get(name_key(domain_id, name).as_str())?
            .map(|g| SubdomainId::new(g.value())))
    }

    /// Get the subdomain with this name in the domain, creating it if absent.
    pub fn get_or_create_subdomain(
        &self,
        domain_id: DomainId,
        name: &str,
        description: &str,
    ) -> Result<Subdomain> {
        if let Some(id) = self.subdomain_id_by_name(domain_id, name)? {
            return self.get_subdomain(id);
        }
        self.create_subdomain(domain_id, name, description)
    }

    pub fn get_subdomain(&self, id: SubdomainId) -> Result<Subdomain> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(SUBDOMAINS)?;
        let bytes = t
            .get(id.get())?
            .ok_or(NucleusError::SubdomainNotFound(id))?;
        codec::decode(bytes.value())
    }

    pub fn list_subdomains(&self, domain_id: DomainId) -> Result<Vec<Subdomain>> {
        let rtx = self.db.begin_read()?;
        let idx = rtx.open_multimap_table(SUBDOMAINS_BY_DOMAIN)?;
        let subs = rtx.open_table(SUBDOMAINS)?;
        let mut out = Vec::new();
        for v in idx.get(domain_id.get())? {
            let sid = v?.value();
            if let Some(bytes) = subs.get(sid)? {
                out.push(codec::decode::<Subdomain>(bytes.value())?);
            }
        }
        Ok(out)
    }

    pub fn get_tag(&self, id: TagId) -> Result<Tag> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(TAGS)?;
        let bytes = t.get(id.get())?.ok_or(NucleusError::TagNotFound(id))?;
        codec::decode(bytes.value())
    }

    pub fn list_tags(&self, domain_id: DomainId) -> Result<Vec<Tag>> {
        let rtx = self.db.begin_read()?;
        let idx = rtx.open_multimap_table(TAGS_BY_DOMAIN)?;
        let tags = rtx.open_table(TAGS)?;
        let mut out = Vec::new();
        for v in idx.get(domain_id.get())? {
            let tag_id = v?.value();
            if let Some(bytes) = tags.get(tag_id)? {
                out.push(codec::decode::<Tag>(bytes.value())?);
            }
        }
        Ok(out)
    }

    // --- documents ---------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn create_document(
        &self,
        domain_id: DomainId,
        subdomain_id: Option<SubdomainId>,
        title: &str,
        source: Option<String>,
        metadata: std::collections::BTreeMap<String, String>,
        tags: Vec<TagId>,
    ) -> Result<Document> {
        self.get_domain(domain_id)?;
        let wtx = self.db.begin_write()?;
        let id = {
            let mut seq = wtx.open_table(SEQ)?;
            DocumentId::new(next_seq(&mut seq, "document")?)
        };
        let doc = Document {
            id,
            domain_id,
            subdomain_id,
            title: title.to_string(),
            source,
            metadata,
            tags,
            created_at: now_millis(),
        };
        {
            let mut t = wtx.open_table(DOCUMENTS)?;
            t.insert(id.get(), codec::encode(&doc)?.as_slice())?;
        }
        {
            let mut idx = wtx.open_multimap_table(DOCS_BY_DOMAIN)?;
            idx.insert(domain_id.get(), id.get())?;
        }
        wtx.commit()?;
        Ok(doc)
    }

    pub fn get_document(&self, id: DocumentId) -> Result<Document> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(DOCUMENTS)?;
        let bytes = t.get(id.get())?.ok_or(NucleusError::DocumentNotFound(id))?;
        codec::decode(bytes.value())
    }

    /// List documents in a domain, paginated (insertion order by id).
    pub fn list_documents(
        &self,
        domain_id: DomainId,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Document>> {
        let rtx = self.db.begin_read()?;
        let by_domain = rtx.open_multimap_table(DOCS_BY_DOMAIN)?;
        let docs = rtx.open_table(DOCUMENTS)?;
        let mut out = Vec::new();
        for entry in by_domain.get(domain_id.get())?.skip(offset).take(limit) {
            let did = entry?.value();
            if let Some(b) = docs.get(did)? {
                out.push(codec::decode::<Document>(b.value())?);
            }
        }
        Ok(out)
    }

    /// List jobs, paginated (by id).
    pub fn list_jobs(&self, offset: usize, limit: usize) -> Result<Vec<Job>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(JOBS)?;
        let mut out = Vec::new();
        for entry in t.iter()?.skip(offset).take(limit) {
            let (_, v) = entry?;
            out.push(codec::decode::<Job>(v.value())?);
        }
        Ok(out)
    }

    /// Look up a document id by content hash within a domain (deduplication).
    pub fn document_id_by_hash(
        &self,
        domain_id: DomainId,
        hash: &str,
    ) -> Result<Option<DocumentId>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(DOCS_BY_HASH)?;
        Ok(t.get(name_key(domain_id, hash).as_str())?
            .map(|g| DocumentId::new(g.value())))
    }

    /// Record a document's content hash for future deduplication.
    pub fn set_document_hash(
        &self,
        domain_id: DomainId,
        document_id: DocumentId,
        hash: &str,
    ) -> Result<()> {
        let wtx = self.db.begin_write()?;
        {
            let mut t = wtx.open_table(DOCS_BY_HASH)?;
            t.insert(name_key(domain_id, hash).as_str(), document_id.get())?;
        }
        wtx.commit()?;
        Ok(())
    }

    /// Delete a document and all of its chunks/embeddings/index entries.
    /// Returns the ids of the chunks that were removed so the caller can update
    /// the in-memory vector index.
    pub fn delete_document(&self, id: DocumentId) -> Result<Vec<ChunkId>> {
        let wtx = self.db.begin_write()?;
        let mut removed = Vec::new();
        {
            let doc_bytes = {
                let docs = wtx.open_table(DOCUMENTS)?;
                let bytes = docs.get(id.get())?.map(|g| g.value().to_vec());
                bytes
            };
            let Some(doc_bytes) = doc_bytes else {
                return Err(NucleusError::DocumentNotFound(id));
            };
            let doc: Document = codec::decode(&doc_bytes)?;

            // Collect this document's chunk ids.
            let chunk_ids: Vec<u64> = {
                let by_doc = wtx.open_multimap_table(CHUNKS_BY_DOC)?;
                let ids = by_doc
                    .get(id.get())?
                    .map(|v| v.map(|g| g.value()))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                ids
            };

            let mut chunks = wtx.open_table(CHUNKS)?;
            let mut embeddings = wtx.open_table(EMBEDDINGS)?;
            let mut by_domain = wtx.open_multimap_table(CHUNKS_BY_DOMAIN)?;
            let mut by_tag = wtx.open_multimap_table(CHUNKS_BY_TAG)?;
            let mut by_doc = wtx.open_multimap_table(CHUNKS_BY_DOC)?;
            let mut by_meta = wtx.open_multimap_table(CHUNKS_BY_META)?;
            let mut by_sub = wtx.open_multimap_table(CHUNKS_BY_SUBDOMAIN)?;

            for cid in &chunk_ids {
                if let Some(cb) = chunks.get(*cid)? {
                    let chunk: Chunk = codec::decode(cb.value())?;
                    by_domain.remove(chunk.domain_id.get(), *cid)?;
                    for tag in &chunk.tags {
                        by_tag.remove(tag.get(), *cid)?;
                    }
                    for (k, v) in &chunk.metadata {
                        by_meta.remove(meta_key(k, v).as_str(), *cid)?;
                    }
                    if let Some(sid) = chunk.subdomain_id {
                        by_sub.remove(sid.get(), *cid)?;
                    }
                }
                chunks.remove(*cid)?;
                embeddings.remove(*cid)?;
                removed.push(ChunkId::new(*cid));
            }
            by_doc.remove_all(id.get())?;

            let mut docs = wtx.open_table(DOCUMENTS)?;
            docs.remove(id.get())?;
            let mut docs_idx = wtx.open_multimap_table(DOCS_BY_DOMAIN)?;
            docs_idx.remove(doc.domain_id.get(), id.get())?;
        }
        wtx.commit()?;
        Ok(removed)
    }

    /// Rename a domain. Returns the updated record.
    pub fn rename_domain(&self, id: DomainId, name: &str) -> Result<Domain> {
        let wtx = self.db.begin_write()?;
        let updated = {
            let mut t = wtx.open_table(DOMAINS)?;
            let bytes = t
                .get(id.get())?
                .map(|g| g.value().to_vec())
                .ok_or(NucleusError::DomainNotFound(id))?;
            let mut domain: Domain = codec::decode(&bytes)?;
            domain.name = name.to_string();
            t.insert(id.get(), codec::encode(&domain)?.as_slice())?;
            domain
        };
        wtx.commit()?;
        Ok(updated)
    }

    /// Delete a domain and **everything** under it: its documents, chunks,
    /// embeddings, tags, subdomains and every secondary-index entry, plus the
    /// by-name and content-hash lookups scoped to the domain. Returns the chunk
    /// ids removed so the caller can drop the in-memory indexes (though the
    /// engine usually just discards the whole per-domain index). All in one
    /// transaction, so a crash leaves the domain either wholly present or wholly
    /// gone.
    pub fn delete_domain(&self, id: DomainId) -> Result<Vec<ChunkId>> {
        let did = id.get();
        let wtx = self.db.begin_write()?;
        let mut removed = Vec::new();
        {
            // Fail fast if the domain doesn't exist (aborts the txn on return).
            {
                let domains = wtx.open_table(DOMAINS)?;
                if domains.get(did)?.is_none() {
                    return Err(NucleusError::DomainNotFound(id));
                }
            }
            // Collect child ids up front (each helper call drops its read handle
            // before we reopen the table for mutation in this same transaction).
            let chunk_ids = collect_multimap(&wtx, CHUNKS_BY_DOMAIN, did)?;
            let doc_ids = collect_multimap(&wtx, DOCS_BY_DOMAIN, did)?;
            let tag_ids = collect_multimap(&wtx, TAGS_BY_DOMAIN, did)?;
            let sub_ids = collect_multimap(&wtx, SUBDOMAINS_BY_DOMAIN, did)?;

            // Chunks + embeddings + their secondary-index entries.
            {
                let mut chunks = wtx.open_table(CHUNKS)?;
                let mut embeddings = wtx.open_table(EMBEDDINGS)?;
                let mut by_tag = wtx.open_multimap_table(CHUNKS_BY_TAG)?;
                let mut by_meta = wtx.open_multimap_table(CHUNKS_BY_META)?;
                let mut by_sub = wtx.open_multimap_table(CHUNKS_BY_SUBDOMAIN)?;
                let mut by_doc = wtx.open_multimap_table(CHUNKS_BY_DOC)?;
                for cid in &chunk_ids {
                    if let Some(cb) = chunks.get(*cid)? {
                        let chunk: Chunk = codec::decode(cb.value())?;
                        for tag in &chunk.tags {
                            by_tag.remove(tag.get(), *cid)?;
                        }
                        for (k, v) in &chunk.metadata {
                            by_meta.remove(meta_key(k, v).as_str(), *cid)?;
                        }
                        if let Some(sid) = chunk.subdomain_id {
                            by_sub.remove(sid.get(), *cid)?;
                        }
                    }
                    chunks.remove(*cid)?;
                    embeddings.remove(*cid)?;
                    removed.push(ChunkId::new(*cid));
                }
                for d in &doc_ids {
                    by_doc.remove_all(*d)?;
                }
            }
            wtx.open_multimap_table(CHUNKS_BY_DOMAIN)?.remove_all(did)?;

            // Documents.
            {
                let mut docs = wtx.open_table(DOCUMENTS)?;
                for d in &doc_ids {
                    docs.remove(*d)?;
                }
            }
            wtx.open_multimap_table(DOCS_BY_DOMAIN)?.remove_all(did)?;

            // Tags.
            {
                let mut tags = wtx.open_table(TAGS)?;
                for t in &tag_ids {
                    tags.remove(*t)?;
                }
            }
            wtx.open_multimap_table(TAGS_BY_DOMAIN)?.remove_all(did)?;

            // Subdomains.
            {
                let mut subs = wtx.open_table(SUBDOMAINS)?;
                for s in &sub_ids {
                    subs.remove(*s)?;
                }
            }
            wtx.open_multimap_table(SUBDOMAINS_BY_DOMAIN)?
                .remove_all(did)?;

            // By-name and content-hash lookups are keyed by "domain\u{1f}…".
            let prefix = format!("{did}\u{1f}");
            remove_keys_with_prefix(&wtx, TAG_IDS, &prefix)?;
            remove_keys_with_prefix(&wtx, SUBDOMAIN_IDS, &prefix)?;
            remove_keys_with_prefix(&wtx, DOCS_BY_HASH, &prefix)?;

            // The domain row itself.
            wtx.open_table(DOMAINS)?.remove(did)?;
        }
        wtx.commit()?;
        Ok(removed)
    }

    /// Delete a subdomain and cascade-delete the documents assigned to it (their
    /// chunks/embeddings/index entries go too). Returns the removed chunk ids.
    /// The document deletes reuse [`delete_document`](Self::delete_document) so
    /// each is atomic; the subdomain row is removed in a final transaction.
    pub fn delete_subdomain(&self, id: SubdomainId) -> Result<Vec<ChunkId>> {
        let sub = self.get_subdomain(id)?;
        // Documents in this subdomain (scan the domain's docs; the set is small
        // relative to chunks and there is no docs-by-subdomain index).
        let doc_ids: Vec<DocumentId> = {
            let rtx = self.db.begin_read()?;
            let by_domain = rtx.open_multimap_table(DOCS_BY_DOMAIN)?;
            let docs = rtx.open_table(DOCUMENTS)?;
            let mut out = Vec::new();
            for v in by_domain.get(sub.domain_id.get())? {
                let did = v?.value();
                if let Some(b) = docs.get(did)? {
                    let doc: Document = codec::decode(b.value())?;
                    if doc.subdomain_id == Some(id) {
                        out.push(DocumentId::new(did));
                    }
                }
            }
            out
        };
        let mut removed = Vec::new();
        for d in doc_ids {
            removed.extend(self.delete_document(d)?);
        }
        // Remove the subdomain row, its by-domain entry and its name lookup.
        let wtx = self.db.begin_write()?;
        {
            wtx.open_table(SUBDOMAINS)?.remove(id.get())?;
            wtx.open_multimap_table(SUBDOMAINS_BY_DOMAIN)?
                .remove(sub.domain_id.get(), id.get())?;
            wtx.open_table(SUBDOMAIN_IDS)?
                .remove(name_key(sub.domain_id, &sub.name).as_str())?;
            // Defensive: drop any lingering chunk-by-subdomain entries.
            wtx.open_multimap_table(CHUNKS_BY_SUBDOMAIN)?
                .remove_all(id.get())?;
        }
        wtx.commit()?;
        Ok(removed)
    }

    /// Update a tag's `display_name` and/or `description` (not its `name`, which
    /// is the lookup key). `None` leaves a field unchanged.
    pub fn update_tag(
        &self,
        id: TagId,
        display_name: Option<&str>,
        description: Option<&str>,
    ) -> Result<Tag> {
        let wtx = self.db.begin_write()?;
        let updated = {
            let mut t = wtx.open_table(TAGS)?;
            let bytes = t
                .get(id.get())?
                .map(|g| g.value().to_vec())
                .ok_or(NucleusError::TagNotFound(id))?;
            let mut tag: Tag = codec::decode(&bytes)?;
            if let Some(d) = display_name {
                tag.display_name = d.to_string();
            }
            if let Some(d) = description {
                tag.description = d.to_string();
            }
            t.insert(id.get(), codec::encode(&tag)?.as_slice())?;
            tag
        };
        wtx.commit()?;
        Ok(updated)
    }

    /// Delete a label (tag), detaching it from every chunk and document that
    /// carries it (documents are **not** deleted — labels are transversal). One
    /// transaction.
    pub fn delete_tag(&self, id: TagId) -> Result<()> {
        let tid = id.get();
        // Read the tag first (404 if missing) to learn its domain and name.
        let tag = self.get_tag(id)?;
        let wtx = self.db.begin_write()?;
        {
            // Chunks carrying the tag.
            let chunk_ids = collect_multimap(&wtx, CHUNKS_BY_TAG, tid)?;
            {
                let mut chunks = wtx.open_table(CHUNKS)?;
                for cid in &chunk_ids {
                    // Copy the bytes out so the read guard is dropped before the
                    // mutating insert (can't hold both borrows of `chunks`).
                    let bytes = chunks.get(*cid)?.map(|g| g.value().to_vec());
                    if let Some(bytes) = bytes {
                        let mut chunk: Chunk = codec::decode(&bytes)?;
                        chunk.tags.retain(|t| *t != id);
                        chunks.insert(*cid, codec::encode(&chunk)?.as_slice())?;
                    }
                }
            }
            wtx.open_multimap_table(CHUNKS_BY_TAG)?.remove_all(tid)?;

            // Documents in the domain that reference the tag.
            let doc_ids = collect_multimap(&wtx, DOCS_BY_DOMAIN, tag.domain_id.get())?;
            {
                let mut docs = wtx.open_table(DOCUMENTS)?;
                for d in &doc_ids {
                    let bytes = docs.get(*d)?.map(|g| g.value().to_vec());
                    if let Some(bytes) = bytes {
                        let mut doc: Document = codec::decode(&bytes)?;
                        if doc.tags.contains(&id) {
                            doc.tags.retain(|t| *t != id);
                            docs.insert(*d, codec::encode(&doc)?.as_slice())?;
                        }
                    }
                }
            }

            // The tag row, its by-domain entry and its name lookup.
            wtx.open_table(TAGS)?.remove(tid)?;
            wtx.open_multimap_table(TAGS_BY_DOMAIN)?
                .remove(tag.domain_id.get(), tid)?;
            wtx.open_table(TAG_IDS)?
                .remove(name_key(tag.domain_id, &tag.name).as_str())?;
        }
        wtx.commit()?;
        Ok(())
    }

    /// Update a domain's pinned `model`/`dim` (used by reindex when the model
    /// changes). The vector index must be rebuilt by the caller afterwards.
    pub fn set_domain_model(&self, id: DomainId, model: &str, dim: usize) -> Result<Domain> {
        let wtx = self.db.begin_write()?;
        let updated = {
            let mut t = wtx.open_table(DOMAINS)?;
            let bytes = t
                .get(id.get())?
                .map(|g| g.value().to_vec())
                .ok_or(NucleusError::DomainNotFound(id))?;
            let mut domain: Domain = codec::decode(&bytes)?;
            domain.model = model.to_string();
            domain.dim = dim;
            t.insert(id.get(), codec::encode(&domain)?.as_slice())?;
            domain
        };
        wtx.commit()?;
        Ok(updated)
    }

    /// Re-assign a document's `tags` and/or `subdomain`, propagating the change to
    /// all of its chunks and the tag/subdomain secondary indexes. `new_tags`
    /// replaces the set when `Some`; `change_subdomain` gates whether
    /// `new_subdomain` is applied (so the subdomain can be set or cleared). The
    /// vector/lexical indexes are untouched (embeddings and text are unchanged).
    /// One transaction.
    pub fn update_document(
        &self,
        id: DocumentId,
        new_tags: Option<Vec<TagId>>,
        new_subdomain: Option<SubdomainId>,
        change_subdomain: bool,
    ) -> Result<Document> {
        let wtx = self.db.begin_write()?;
        let updated = {
            let mut docs = wtx.open_table(DOCUMENTS)?;
            let bytes = docs
                .get(id.get())?
                .map(|g| g.value().to_vec())
                .ok_or(NucleusError::DocumentNotFound(id))?;
            let mut doc: Document = codec::decode(&bytes)?;
            let final_tags = new_tags.unwrap_or_else(|| doc.tags.clone());
            let final_sub = if change_subdomain {
                new_subdomain
            } else {
                doc.subdomain_id
            };

            // Propagate to chunks + their secondary indexes.
            let chunk_ids = collect_multimap(&wtx, CHUNKS_BY_DOC, id.get())?;
            {
                let mut chunks = wtx.open_table(CHUNKS)?;
                let mut by_tag = wtx.open_multimap_table(CHUNKS_BY_TAG)?;
                let mut by_sub = wtx.open_multimap_table(CHUNKS_BY_SUBDOMAIN)?;
                for cid in &chunk_ids {
                    let cb = chunks.get(*cid)?.map(|g| g.value().to_vec());
                    let Some(cb) = cb else { continue };
                    let mut chunk: Chunk = codec::decode(&cb)?;
                    // Tags: swap index entries old -> new.
                    for t in &chunk.tags {
                        by_tag.remove(t.get(), *cid)?;
                    }
                    for t in &final_tags {
                        by_tag.insert(t.get(), *cid)?;
                    }
                    chunk.tags = final_tags.clone();
                    // Subdomain: swap index entry old -> new.
                    if chunk.subdomain_id != final_sub {
                        if let Some(old) = chunk.subdomain_id {
                            by_sub.remove(old.get(), *cid)?;
                        }
                        if let Some(new) = final_sub {
                            by_sub.insert(new.get(), *cid)?;
                        }
                        chunk.subdomain_id = final_sub;
                    }
                    chunks.insert(*cid, codec::encode(&chunk)?.as_slice())?;
                }
            }

            doc.tags = final_tags;
            doc.subdomain_id = final_sub;
            docs.insert(id.get(), codec::encode(&doc)?.as_slice())?;
            doc
        };
        wtx.commit()?;
        Ok(updated)
    }

    // --- chunks & embeddings ----------------------------------------------

    /// Allocate and persist a chunk together with its embedding, updating all
    /// secondary indexes. Returns the assigned [`ChunkId`].
    #[allow(clippy::too_many_arguments)]
    pub fn insert_chunk(
        &self,
        domain_id: DomainId,
        document_id: DocumentId,
        subdomain_id: Option<SubdomainId>,
        ordinal: u32,
        text: &str,
        tags: &[TagId],
        metadata: std::collections::BTreeMap<String, String>,
        embedding: &[f32],
    ) -> Result<ChunkId> {
        let wtx = self.db.begin_write()?;
        let id = {
            let mut seq = wtx.open_table(SEQ)?;
            ChunkId::new(next_seq(&mut seq, "chunk")?)
        };
        let chunk = Chunk {
            id,
            document_id,
            domain_id,
            subdomain_id,
            ordinal,
            text: text.to_string(),
            tags: tags.to_vec(),
            metadata,
            prev: None,
            next: None,
        };
        {
            let mut t = wtx.open_table(CHUNKS)?;
            t.insert(id.get(), codec::encode(&chunk)?.as_slice())?;
        }
        {
            let mut e = wtx.open_table(EMBEDDINGS)?;
            e.insert(id.get(), codec::encode(&embedding.to_vec())?.as_slice())?;
        }
        {
            let mut by_domain = wtx.open_multimap_table(CHUNKS_BY_DOMAIN)?;
            by_domain.insert(domain_id.get(), id.get())?;
            let mut by_doc = wtx.open_multimap_table(CHUNKS_BY_DOC)?;
            by_doc.insert(document_id.get(), id.get())?;
            let mut by_tag = wtx.open_multimap_table(CHUNKS_BY_TAG)?;
            for tag in tags {
                by_tag.insert(tag.get(), id.get())?;
            }
            let mut by_meta = wtx.open_multimap_table(CHUNKS_BY_META)?;
            for (k, v) in &chunk.metadata {
                by_meta.insert(meta_key(k, v).as_str(), id.get())?;
            }
            if let Some(sid) = subdomain_id {
                let mut by_sub = wtx.open_multimap_table(CHUNKS_BY_SUBDOMAIN)?;
                by_sub.insert(sid.get(), id.get())?;
            }
        }
        wtx.commit()?;
        Ok(id)
    }

    /// Persist all chunks of a document in a **single transaction**, chaining
    /// them via `prev`/`next` and updating every secondary index. Returns the
    /// assigned ids in order. This replaces N per-chunk transactions with one.
    pub fn insert_chunks(
        &self,
        domain_id: DomainId,
        document_id: DocumentId,
        subdomain_id: Option<SubdomainId>,
        tags: &[TagId],
        metadata: &std::collections::BTreeMap<String, String>,
        chunks: &[NewChunk<'_>],
    ) -> Result<Vec<ChunkId>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let wtx = self.db.begin_write()?;
        let ids: Vec<ChunkId> = {
            let mut seq = wtx.open_table(SEQ)?;
            let mut v = Vec::with_capacity(chunks.len());
            for _ in 0..chunks.len() {
                v.push(ChunkId::new(next_seq(&mut seq, "chunk")?));
            }
            v
        };
        {
            let mut t = wtx.open_table(CHUNKS)?;
            let mut emb = wtx.open_table(EMBEDDINGS)?;
            let mut by_domain = wtx.open_multimap_table(CHUNKS_BY_DOMAIN)?;
            let mut by_doc = wtx.open_multimap_table(CHUNKS_BY_DOC)?;
            let mut by_tag = wtx.open_multimap_table(CHUNKS_BY_TAG)?;
            let mut by_meta = wtx.open_multimap_table(CHUNKS_BY_META)?;
            let mut by_sub = wtx.open_multimap_table(CHUNKS_BY_SUBDOMAIN)?;
            for (i, nc) in chunks.iter().enumerate() {
                let id = ids[i];
                let chunk = Chunk {
                    id,
                    document_id,
                    domain_id,
                    subdomain_id,
                    ordinal: i as u32,
                    text: nc.text.to_string(),
                    tags: tags.to_vec(),
                    metadata: metadata.clone(),
                    prev: if i > 0 { Some(ids[i - 1]) } else { None },
                    next: ids.get(i + 1).copied(),
                };
                t.insert(id.get(), codec::encode(&chunk)?.as_slice())?;
                emb.insert(id.get(), codec::encode(&nc.embedding.to_vec())?.as_slice())?;
                by_domain.insert(domain_id.get(), id.get())?;
                by_doc.insert(document_id.get(), id.get())?;
                for tag in tags {
                    by_tag.insert(tag.get(), id.get())?;
                }
                for (k, v) in metadata {
                    by_meta.insert(meta_key(k, v).as_str(), id.get())?;
                }
                if let Some(sid) = subdomain_id {
                    by_sub.insert(sid.get(), id.get())?;
                }
            }
        }
        wtx.commit()?;
        Ok(ids)
    }

    /// Link a document's chunks in order via their `prev`/`next` pointers, so a
    /// chunk's neighbours can be fetched for context. `ids` must be in document
    /// order.
    pub fn link_chunks(&self, ids: &[ChunkId]) -> Result<()> {
        if ids.len() < 2 {
            return Ok(());
        }
        let wtx = self.db.begin_write()?;
        {
            let mut chunks = wtx.open_table(CHUNKS)?;
            for (i, cid) in ids.iter().enumerate() {
                let bytes = chunks.get(cid.get())?.map(|g| g.value().to_vec());
                let Some(bytes) = bytes else {
                    continue;
                };
                let mut chunk: Chunk = codec::decode(&bytes)?;
                chunk.prev = if i > 0 { Some(ids[i - 1]) } else { None };
                chunk.next = ids.get(i + 1).copied();
                chunks.insert(cid.get(), codec::encode(&chunk)?.as_slice())?;
            }
        }
        wtx.commit()?;
        Ok(())
    }

    pub fn get_chunk(&self, id: ChunkId) -> Result<Chunk> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(CHUNKS)?;
        let bytes = t.get(id.get())?.ok_or(NucleusError::ChunkNotFound(id))?;
        codec::decode(bytes.value())
    }

    pub fn get_embedding(&self, id: ChunkId) -> Result<Vec<f32>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(EMBEDDINGS)?;
        let bytes = t.get(id.get())?.ok_or(NucleusError::ChunkNotFound(id))?;
        codec::decode(bytes.value())
    }

    /// Overwrite a chunk's embedding vector (used by reindex). One transaction.
    pub fn set_embedding(&self, id: ChunkId, embedding: &[f32]) -> Result<()> {
        let wtx = self.db.begin_write()?;
        {
            let mut t = wtx.open_table(EMBEDDINGS)?;
            t.insert(id.get(), codec::encode(&embedding.to_vec())?.as_slice())?;
        }
        wtx.commit()?;
        Ok(())
    }

    /// All `(chunk id, text)` pairs in a domain — used to (re)build the lexical
    /// (BM25) index on startup.
    pub fn texts_in_domain(&self, domain_id: DomainId) -> Result<Vec<(ChunkId, String)>> {
        let rtx = self.db.begin_read()?;
        let by_domain = rtx.open_multimap_table(CHUNKS_BY_DOMAIN)?;
        let chunks = rtx.open_table(CHUNKS)?;
        let mut out = Vec::new();
        for v in by_domain.get(domain_id.get())? {
            let cid = v?.value();
            if let Some(b) = chunks.get(cid)? {
                let chunk: Chunk = codec::decode(b.value())?;
                out.push((ChunkId::new(cid), chunk.text));
            }
        }
        Ok(out)
    }

    /// All `(chunk id, embedding)` pairs in a domain — used to (re)build a
    /// domain's in-memory vector index on startup or first use.
    pub fn embeddings_in_domain(&self, domain_id: DomainId) -> Result<Vec<(ChunkId, Vec<f32>)>> {
        let rtx = self.db.begin_read()?;
        let by_domain = rtx.open_multimap_table(CHUNKS_BY_DOMAIN)?;
        let emb = rtx.open_table(EMBEDDINGS)?;
        let mut out = Vec::new();
        for v in by_domain.get(domain_id.get())? {
            let cid = v?.value();
            if let Some(bytes) = emb.get(cid)? {
                out.push((ChunkId::new(cid), codec::decode::<Vec<f32>>(bytes.value())?));
            }
        }
        Ok(out)
    }

    /// All chunk ids in a domain (used to evaluate query-language filters).
    pub fn chunk_ids_in_domain(&self, domain_id: DomainId) -> Result<Vec<ChunkId>> {
        let rtx = self.db.begin_read()?;
        let by_domain = rtx.open_multimap_table(CHUNKS_BY_DOMAIN)?;
        let mut out = Vec::new();
        for v in by_domain.get(domain_id.get())? {
            out.push(ChunkId::new(v?.value()));
        }
        Ok(out)
    }

    /// The set of chunk ids belonging to any of `docs`.
    pub fn chunk_ids_for_documents(&self, docs: &[DocumentId]) -> Result<HashSet<ChunkId>> {
        let rtx = self.db.begin_read()?;
        let by_doc = rtx.open_multimap_table(CHUNKS_BY_DOC)?;
        let mut set = HashSet::new();
        for d in docs {
            for v in by_doc.get(d.get())? {
                set.insert(ChunkId::new(v?.value()));
            }
        }
        Ok(set)
    }

    /// The set of chunk ids whose metadata has `key` == `value`.
    pub fn candidates_for_meta(&self, key: &str, value: &str) -> Result<HashSet<ChunkId>> {
        let rtx = self.db.begin_read()?;
        let by_meta = rtx.open_multimap_table(CHUNKS_BY_META)?;
        let mut set = HashSet::new();
        for v in by_meta.get(meta_key(key, value).as_str())? {
            set.insert(ChunkId::new(v?.value()));
        }
        Ok(set)
    }

    /// The set of chunk ids in a subdomain.
    pub fn candidates_for_subdomain(&self, subdomain_id: SubdomainId) -> Result<HashSet<ChunkId>> {
        let rtx = self.db.begin_read()?;
        let by_sub = rtx.open_multimap_table(CHUNKS_BY_SUBDOMAIN)?;
        let mut set = HashSet::new();
        for v in by_sub.get(subdomain_id.get())? {
            set.insert(ChunkId::new(v?.value()));
        }
        Ok(set)
    }

    /// Candidate chunk ids in a domain matching `tags`. With `match_all`, a chunk
    /// must carry every tag; otherwise any one tag suffices. An empty `tags`
    /// slice returns `None` (meaning "no tag restriction").
    pub fn candidates_for_tags(
        &self,
        tags: &[TagId],
        match_all: bool,
    ) -> Result<Option<HashSet<ChunkId>>> {
        if tags.is_empty() {
            return Ok(None);
        }
        let rtx = self.db.begin_read()?;
        let by_tag = rtx.open_multimap_table(CHUNKS_BY_TAG)?;
        let mut acc: Option<HashSet<ChunkId>> = None;
        for tag in tags {
            let mut set = HashSet::new();
            for v in by_tag.get(tag.get())? {
                set.insert(ChunkId::new(v?.value()));
            }
            acc = Some(match acc {
                None => set,
                Some(prev) if match_all => prev.intersection(&set).copied().collect(),
                Some(prev) => prev.union(&set).copied().collect(),
            });
        }
        Ok(acc)
    }

    // --- tokens ------------------------------------------------------------

    pub fn create_token(
        &self,
        name: &str,
        hash: [u8; 32],
        scopes: Vec<Scope>,
        expires_at: Option<i64>,
    ) -> Result<ApiToken> {
        let wtx = self.db.begin_write()?;
        let id = {
            let mut seq = wtx.open_table(SEQ)?;
            TokenId::new(next_seq(&mut seq, "token")?)
        };
        let token = ApiToken {
            id,
            name: name.to_string(),
            hash,
            scopes,
            created_at: now_millis(),
            expires_at,
        };
        {
            let mut t = wtx.open_table(TOKENS)?;
            t.insert(hash.as_slice(), codec::encode(&token)?.as_slice())?;
        }
        wtx.commit()?;
        Ok(token)
    }

    pub fn get_token_by_hash(&self, hash: &[u8; 32]) -> Result<Option<ApiToken>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(TOKENS)?;
        match t.get(hash.as_slice())? {
            Some(bytes) => Ok(Some(codec::decode(bytes.value())?)),
            None => Ok(None),
        }
    }

    pub fn list_tokens(&self) -> Result<Vec<ApiToken>> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(TOKENS)?;
        let mut out = Vec::new();
        for entry in t.iter()? {
            let (_, v) = entry?;
            out.push(codec::decode::<ApiToken>(v.value())?);
        }
        Ok(out)
    }

    /// Replace a token's secret hash (rotation), keeping its id/name/scopes/expiry.
    /// The table is keyed by hash, so this removes the old key and writes the new
    /// one in a single transaction. Returns the updated record, or `None` if no
    /// token has that id.
    pub fn rotate_token(&self, id: TokenId, new_hash: [u8; 32]) -> Result<Option<ApiToken>> {
        let found = {
            let rtx = self.db.begin_read()?;
            let t = rtx.open_table(TOKENS)?;
            let mut f = None;
            for entry in t.iter()? {
                let (k, v) = entry?;
                let tok: ApiToken = codec::decode(v.value())?;
                if tok.id == id {
                    f = Some((k.value().to_vec(), tok));
                    break;
                }
            }
            f
        };
        let Some((old_hash, mut tok)) = found else {
            return Ok(None);
        };
        tok.hash = new_hash;
        let wtx = self.db.begin_write()?;
        {
            let mut t = wtx.open_table(TOKENS)?;
            t.remove(old_hash.as_slice())?;
            t.insert(new_hash.as_slice(), codec::encode(&tok)?.as_slice())?;
        }
        wtx.commit()?;
        Ok(Some(tok))
    }

    pub fn count_tokens(&self) -> Result<u64> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(TOKENS)?;
        Ok(t.len()?)
    }

    /// Delete a token by id. Returns whether a token was removed.
    pub fn delete_token(&self, id: TokenId) -> Result<bool> {
        let target = {
            let rtx = self.db.begin_read()?;
            let t = rtx.open_table(TOKENS)?;
            let mut found = None;
            for entry in t.iter()? {
                let (k, v) = entry?;
                let token: ApiToken = codec::decode(v.value())?;
                if token.id == id {
                    found = Some(k.value().to_vec());
                    break;
                }
            }
            found
        };
        let Some(hash) = target else {
            return Ok(false);
        };
        let wtx = self.db.begin_write()?;
        {
            let mut t = wtx.open_table(TOKENS)?;
            t.remove(hash.as_slice())?;
        }
        wtx.commit()?;
        Ok(true)
    }

    // --- jobs --------------------------------------------------------------

    pub fn create_job(&self, kind: JobKind) -> Result<Job> {
        let wtx = self.db.begin_write()?;
        let id = {
            let mut seq = wtx.open_table(SEQ)?;
            JobId::new(next_seq(&mut seq, "job")?)
        };
        let now = now_millis();
        let job = Job {
            id,
            kind,
            status: JobStatus::Pending,
            attempts: 0,
            error: None,
            created_at: now,
            updated_at: now,
        };
        {
            let mut jobs = wtx.open_table(JOBS)?;
            jobs.insert(id.get(), codec::encode(&job)?.as_slice())?;
        }
        {
            let mut pending = wtx.open_table(JOBS_PENDING)?;
            pending.insert(id.get(), ())?;
        }
        wtx.commit()?;
        Ok(job)
    }

    pub fn get_job(&self, id: JobId) -> Result<Job> {
        let rtx = self.db.begin_read()?;
        let t = rtx.open_table(JOBS)?;
        let bytes = t.get(id.get())?.ok_or(NucleusError::JobNotFound(id))?;
        codec::decode(bytes.value())
    }

    /// Atomically pick the oldest `Pending` job, mark it `Running` (incrementing
    /// `attempts`), and return it. redb serialises write transactions, so two
    /// workers never claim the same job.
    pub fn claim_next_pending(&self) -> Result<Option<Job>> {
        let wtx = self.db.begin_write()?;
        let claimed = {
            let mut pending = wtx.open_table(JOBS_PENDING)?;
            let next_id = pending.first()?.map(|(k, _)| k.value());
            match next_id {
                None => None,
                Some(jid) => {
                    pending.remove(jid)?;
                    let mut jobs = wtx.open_table(JOBS)?;
                    let bytes = jobs.get(jid)?.map(|g| g.value().to_vec());
                    match bytes {
                        None => None, // stale pending entry; skip
                        Some(bytes) => {
                            let mut job: Job = codec::decode(&bytes)?;
                            job.status = JobStatus::Running;
                            job.attempts += 1;
                            job.updated_at = now_millis();
                            jobs.insert(jid, codec::encode(&job)?.as_slice())?;
                            Some(job)
                        }
                    }
                }
            }
        };
        wtx.commit()?;
        Ok(claimed)
    }

    /// Set a job's terminal (or retry) status.
    pub fn finish_job(&self, id: JobId, status: JobStatus, error: Option<String>) -> Result<()> {
        let wtx = self.db.begin_write()?;
        {
            let mut jobs = wtx.open_table(JOBS)?;
            let bytes = jobs.get(id.get())?.map(|g| g.value().to_vec());
            let Some(bytes) = bytes else {
                return Err(NucleusError::JobNotFound(id));
            };
            let mut job: Job = codec::decode(&bytes)?;
            job.status = status;
            job.error = error;
            job.updated_at = now_millis();
            jobs.insert(id.get(), codec::encode(&job)?.as_slice())?;
        }
        {
            // Keep the pending set in sync: re-add on retry, drop otherwise.
            let mut pending = wtx.open_table(JOBS_PENDING)?;
            if matches!(status, JobStatus::Pending) {
                pending.insert(id.get(), ())?;
            } else {
                pending.remove(id.get())?;
            }
        }
        wtx.commit()?;
        Ok(())
    }

    /// Delete terminal (Done/Failed) jobs whose `updated_at` predates `cutoff_ms`.
    /// Returns how many were removed. Used by the periodic retention sweep.
    pub fn purge_finished(&self, cutoff_ms: i64) -> Result<usize> {
        let to_delete: Vec<u64> = {
            let rtx = self.db.begin_read()?;
            let jobs = rtx.open_table(JOBS)?;
            let mut ids = Vec::new();
            for entry in jobs.iter()? {
                let (k, v) = entry?;
                let job: Job = codec::decode(v.value())?;
                if matches!(job.status, JobStatus::Done | JobStatus::Failed)
                    && job.updated_at < cutoff_ms
                {
                    ids.push(k.value());
                }
            }
            ids
        };
        if to_delete.is_empty() {
            return Ok(0);
        }
        let wtx = self.db.begin_write()?;
        {
            let mut jobs = wtx.open_table(JOBS)?;
            let mut pending = wtx.open_table(JOBS_PENDING)?;
            for id in &to_delete {
                jobs.remove(*id)?;
                pending.remove(*id)?;
            }
        }
        wtx.commit()?;
        Ok(to_delete.len())
    }

    /// On startup, return any `Running` jobs to `Pending` (their worker died
    /// mid-flight). Returns how many were requeued.
    pub fn requeue_running(&self) -> Result<usize> {
        let wtx = self.db.begin_write()?;
        let mut count = 0;
        {
            let mut jobs = wtx.open_table(JOBS)?;
            let mut to_update = Vec::new();
            for entry in jobs.iter()? {
                let (k, v) = entry?;
                let job: Job = codec::decode(v.value())?;
                if matches!(job.status, JobStatus::Running) {
                    to_update.push((k.value(), job));
                }
            }
            let mut pending = wtx.open_table(JOBS_PENDING)?;
            for (id, mut job) in to_update {
                job.status = JobStatus::Pending;
                job.updated_at = now_millis();
                jobs.insert(id, codec::encode(&job)?.as_slice())?;
                pending.insert(id, ())?;
                count += 1;
            }
        }
        wtx.commit()?;
        Ok(count)
    }
}

/// Encode a metadata `key`/`value` pair into a single multimap key. The unit
/// separator (`\u{1f}`) keeps `a`+`bc` distinct from `ab`+`c`.
fn meta_key(key: &str, value: &str) -> String {
    format!("{key}\u{1f}{value}")
}

/// Build a `"domain\u{1f}name"` key for the by-name indexes (subdomains, tags).
fn name_key(domain_id: DomainId, name: &str) -> String {
    format!("{}\u{1f}{}", domain_id.get(), name)
}

/// Collect all values a `u64 -> u64` multimap holds for `key` into a `Vec`.
/// Done in its own helper so the read handle is dropped before the caller
/// reopens the table for mutation within the same transaction.
fn collect_multimap(
    wtx: &redb::WriteTransaction,
    def: MultimapTableDefinition<u64, u64>,
    key: u64,
) -> Result<Vec<u64>> {
    let t = wtx.open_multimap_table(def)?;
    let mut out = Vec::new();
    for v in t.get(key)? {
        out.push(v?.value());
    }
    Ok(out)
}

/// Remove every entry of a `&str`-keyed table whose key starts with `prefix`.
/// Used to drop a domain's by-name / content-hash lookups on cascade delete.
/// Keys are collected first (dropping the read handle) before removal, so the
/// table is never iterated and mutated at once.
fn remove_keys_with_prefix(
    wtx: &redb::WriteTransaction,
    def: TableDefinition<&'static str, u64>,
    prefix: &str,
) -> Result<()> {
    let keys: Vec<String> = {
        let t = wtx.open_table(def)?;
        let mut out = Vec::new();
        for entry in t.iter()? {
            let (k, _) = entry?;
            if k.value().starts_with(prefix) {
                out.push(k.value().to_string());
            }
        }
        out
    };
    let mut t = wtx.open_table(def)?;
    for k in &keys {
        t.remove(k.as_str())?;
    }
    Ok(())
}

/// Read-modify-write a counter in the `SEQ` table, returning the new value.
fn next_seq(seq: &mut redb::Table<&str, u64>, key: &str) -> Result<u64> {
    let current = seq.get(key)?.map(|g| g.value()).unwrap_or(0);
    let next = current + 1;
    seq.insert(key, next)?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Storage, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path().join("nucleus.redb")).unwrap();
        (storage, dir)
    }

    #[test]
    fn domain_roundtrip_and_list() {
        let (s, _d) = temp_db();
        let dom = s
            .create_domain("docs", "multilingual-e5-small", 384)
            .unwrap();
        assert_eq!(s.get_domain(dom.id).unwrap(), dom);
        assert_eq!(s.list_domains().unwrap(), vec![dom]);
    }

    #[test]
    fn chunk_insert_and_tag_candidates() {
        let (s, _d) = temp_db();
        let dom = s.create_domain("docs", "m", 2).unwrap();
        let t1 = s.create_tag(dom.id, "legal", "Legal", "", None).unwrap();
        let t2 = s.create_tag(dom.id, "hr", "HR", "", None).unwrap();
        let doc = s
            .create_document(dom.id, None, "d", None, Default::default(), vec![])
            .unwrap();

        let c1 = s
            .insert_chunk(
                dom.id,
                doc.id,
                None,
                0,
                "a",
                &[t1.id],
                Default::default(),
                &[1.0, 0.0],
            )
            .unwrap();
        let c2 = s
            .insert_chunk(
                dom.id,
                doc.id,
                None,
                1,
                "b",
                &[t1.id, t2.id],
                Default::default(),
                &[0.0, 1.0],
            )
            .unwrap();

        // any-of {legal} -> both via t1
        let any = s.candidates_for_tags(&[t1.id], false).unwrap().unwrap();
        assert_eq!(any.len(), 2);
        // all-of {legal, hr} -> only c2
        let all = s
            .candidates_for_tags(&[t1.id, t2.id], true)
            .unwrap()
            .unwrap();
        assert_eq!(all, [c2].into_iter().collect());
        // no tags -> no restriction
        assert!(s.candidates_for_tags(&[], false).unwrap().is_none());

        assert_eq!(s.embeddings_in_domain(dom.id).unwrap().len(), 2);
        let _ = c1;
    }

    #[test]
    fn delete_document_cleans_up() {
        let (s, _d) = temp_db();
        let dom = s.create_domain("docs", "m", 2).unwrap();
        let t1 = s.create_tag(dom.id, "x", "X", "", None).unwrap();
        let doc = s
            .create_document(dom.id, None, "d", None, Default::default(), vec![])
            .unwrap();
        let _c = s
            .insert_chunk(
                dom.id,
                doc.id,
                None,
                0,
                "a",
                &[t1.id],
                Default::default(),
                &[1.0, 0.0],
            )
            .unwrap();

        let removed = s.delete_document(doc.id).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(matches!(
            s.get_document(doc.id),
            Err(NucleusError::DocumentNotFound(_))
        ));
        assert!(s.embeddings_in_domain(dom.id).unwrap().is_empty());
        assert!(s
            .candidates_for_tags(&[t1.id], false)
            .unwrap()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn rename_and_delete_domain_cascades() {
        let (s, _d) = temp_db();
        let dom = s.create_domain("docs", "m", 2).unwrap();
        let tag = s.create_tag(dom.id, "legal", "Legal", "", None).unwrap();
        let sub = s.create_subdomain(dom.id, "irpf", "").unwrap();
        let doc = s
            .create_document(
                dom.id,
                Some(sub.id),
                "d",
                None,
                Default::default(),
                vec![tag.id],
            )
            .unwrap();
        s.insert_chunk(
            dom.id,
            doc.id,
            Some(sub.id),
            0,
            "a",
            &[tag.id],
            Default::default(),
            &[1.0, 0.0],
        )
        .unwrap();
        s.set_document_hash(dom.id, doc.id, "h1").unwrap();

        // Rename works.
        assert_eq!(s.rename_domain(dom.id, "renamed").unwrap().name, "renamed");

        // Cascade delete wipes everything scoped to the domain.
        let removed = s.delete_domain(dom.id).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(matches!(
            s.get_domain(dom.id),
            Err(NucleusError::DomainNotFound(_))
        ));
        assert!(s.list_tags(dom.id).unwrap().is_empty());
        assert!(s.list_subdomains(dom.id).unwrap().is_empty());
        assert!(s.list_documents(dom.id, 0, 100).unwrap().is_empty());
        assert!(s.embeddings_in_domain(dom.id).unwrap().is_empty());
        assert_eq!(s.tag_id_by_name(dom.id, "legal").unwrap(), None);
        assert_eq!(s.subdomain_id_by_name(dom.id, "irpf").unwrap(), None);
        assert_eq!(s.document_id_by_hash(dom.id, "h1").unwrap(), None);

        // A second delete is a clean 404, not a panic.
        assert!(matches!(
            s.delete_domain(dom.id),
            Err(NucleusError::DomainNotFound(_))
        ));
    }

    #[test]
    fn delete_tag_detaches_without_deleting_docs() {
        let (s, _d) = temp_db();
        let dom = s.create_domain("docs", "m", 2).unwrap();
        let tag = s.create_tag(dom.id, "legal", "Legal", "", None).unwrap();
        let doc = s
            .create_document(dom.id, None, "d", None, Default::default(), vec![tag.id])
            .unwrap();
        let cid = s
            .insert_chunk(
                dom.id,
                doc.id,
                None,
                0,
                "a",
                &[tag.id],
                Default::default(),
                &[1.0, 0.0],
            )
            .unwrap();

        s.delete_tag(tag.id).unwrap();

        // Tag gone, name lookup gone.
        assert!(matches!(
            s.get_tag(tag.id),
            Err(NucleusError::TagNotFound(_))
        ));
        assert_eq!(s.tag_id_by_name(dom.id, "legal").unwrap(), None);
        // Document and chunk survive, detached from the tag.
        assert!(s.get_document(doc.id).unwrap().tags.is_empty());
        assert!(s.get_chunk(cid).unwrap().tags.is_empty());
        // The by-tag index no longer points anywhere.
        assert!(s
            .candidates_for_tags(&[tag.id], false)
            .unwrap()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn delete_subdomain_cascades_its_documents() {
        let (s, _d) = temp_db();
        let dom = s.create_domain("docs", "m", 2).unwrap();
        let sub = s.create_subdomain(dom.id, "irpf", "").unwrap();
        let in_sub = s
            .create_document(dom.id, Some(sub.id), "in", None, Default::default(), vec![])
            .unwrap();
        s.insert_chunk(
            dom.id,
            in_sub.id,
            Some(sub.id),
            0,
            "a",
            &[],
            Default::default(),
            &[1.0, 0.0],
        )
        .unwrap();
        let other = s
            .create_document(dom.id, None, "out", None, Default::default(), vec![])
            .unwrap();
        s.insert_chunk(
            dom.id,
            other.id,
            None,
            0,
            "b",
            &[],
            Default::default(),
            &[0.0, 1.0],
        )
        .unwrap();

        let removed = s.delete_subdomain(sub.id).unwrap();
        assert_eq!(removed.len(), 1);
        assert!(matches!(
            s.get_subdomain(sub.id),
            Err(NucleusError::SubdomainNotFound(_))
        ));
        // The subdomain's document is gone; the other one survives.
        assert!(matches!(
            s.get_document(in_sub.id),
            Err(NucleusError::DocumentNotFound(_))
        ));
        assert!(s.get_document(other.id).is_ok());
        assert_eq!(s.subdomain_id_by_name(dom.id, "irpf").unwrap(), None);
    }
}
