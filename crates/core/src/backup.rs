//! Engine-level **backup & restore**.
//!
//! A *full* backup is a consistent file snapshot of the redb database (taken
//! while holding the write lock, so no commit lands mid-copy). A *differential*
//! is a **binary delta** (bsdiff) of the current database against the most recent
//! full — full-fidelity (it captures deletes too), and a restore needs only the
//! full plus the latest differential, mirroring SQL Server's model.
//!
//! Indexes are **not** backed up: they are rebuilt from the database on restore.
//!
//! [`BackupManager`] is transport-agnostic and synchronous; the server wraps it
//! with a scheduler and HTTP endpoints, and performs the hot engine swap.

use std::collections::HashSet;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use qbsdiff::{Bsdiff, Bspatch};
use serde::{Deserialize, Serialize};

use crate::error::NucleusError;
use crate::storage::Storage;
use crate::util::{format_utc, now_millis};
use crate::Result;

/// Kind of backup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackupKind {
    /// A complete, standalone snapshot.
    Full,
    /// A binary delta against the most recent full.
    Differential,
}

/// A catalog entry describing one backup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupRecord {
    /// Stable id, also the base of the filename (e.g. `2026-06-21_15-30-12-full`).
    pub id: String,
    pub kind: BackupKind,
    /// Creation time, Unix milliseconds.
    pub created_at: i64,
    /// For differentials, the id of the full they apply on top of.
    pub parent: Option<String>,
    /// File name within the backup directory.
    pub file: String,
    /// Size of the backup file in bytes.
    pub bytes: u64,
}

/// Manages backups in a directory (snapshot files, deltas and a JSON catalog).
pub struct BackupManager {
    dir: PathBuf,
}

impl BackupManager {
    /// Open (creating if needed) a backup directory.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn catalog_path(&self) -> PathBuf {
        self.dir.join("catalog.json")
    }

    /// All backups, oldest first.
    pub fn list(&self) -> Result<Vec<BackupRecord>> {
        let p = self.catalog_path();
        if !p.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&p)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| NucleusError::invalid(format!("corrupt backup catalog: {e}")))
    }

    fn save_catalog(&self, recs: &[BackupRecord]) -> Result<()> {
        let bytes =
            serde_json::to_vec_pretty(recs).map_err(|e| NucleusError::invalid(e.to_string()))?;
        let tmp = self.dir.join("catalog.json.tmp");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, self.catalog_path())?; // Rust's rename replaces on all platforms
        Ok(())
    }

    fn append(&self, rec: BackupRecord) -> Result<BackupRecord> {
        let mut recs = self.list()?;
        recs.push(rec.clone());
        self.save_catalog(&recs)?;
        Ok(rec)
    }

    /// Take a **full** snapshot of `storage`.
    pub fn full(&self, storage: &Storage) -> Result<BackupRecord> {
        let ts = now_millis();
        let id = format!("{}-{:03}-full", format_utc(ts), ts.rem_euclid(1000));
        let file = format!("{id}.redb");
        let dst = self.dir.join(&file);
        storage.backup_to(&dst)?;
        let bytes = fs::metadata(&dst)?.len();
        self.append(BackupRecord {
            id,
            kind: BackupKind::Full,
            created_at: ts,
            parent: None,
            file,
            bytes,
        })
    }

    /// Take a **differential** (binary delta) against the most recent full.
    pub fn differential(&self, storage: &Storage) -> Result<BackupRecord> {
        let recs = self.list()?;
        let last_full = recs
            .iter()
            .rev()
            .find(|r| r.kind == BackupKind::Full)
            .ok_or_else(|| {
                NucleusError::invalid("no full backup to diff against; take a full first")
            })?
            .clone();
        let full_bytes = fs::read(self.dir.join(&last_full.file))?;

        // Consistent snapshot of the current state, then diff it against the full.
        let ts = now_millis();
        let tmp = self.dir.join(format!(".snapshot-{ts}.tmp"));
        storage.backup_to(&tmp)?;
        let cur_bytes = fs::read(&tmp)?;
        let _ = fs::remove_file(&tmp);

        let mut patch = Vec::new();
        Bsdiff::new(&full_bytes, &cur_bytes)
            .compare(Cursor::new(&mut patch))
            .map_err(|e| NucleusError::invalid(format!("diff failed: {e}")))?;

        let id = format!("{}-{:03}-diff", format_utc(ts), ts.rem_euclid(1000));
        let file = format!("{id}.patch");
        fs::write(self.dir.join(&file), &patch)?;
        self.append(BackupRecord {
            id,
            kind: BackupKind::Differential,
            created_at: ts,
            parent: Some(last_full.id),
            file,
            bytes: patch.len() as u64,
        })
    }

    /// Reconstruct the database for `backup_id` into the redb file `dst`.
    pub fn restore_to(&self, backup_id: &str, dst: impl AsRef<Path>) -> Result<()> {
        let recs = self.list()?;
        let rec = recs
            .iter()
            .find(|r| r.id == backup_id)
            .ok_or_else(|| NucleusError::invalid(format!("unknown backup: {backup_id}")))?;
        match rec.kind {
            BackupKind::Full => {
                fs::copy(self.dir.join(&rec.file), dst.as_ref())?;
            }
            BackupKind::Differential => {
                let parent_id = rec
                    .parent
                    .as_ref()
                    .ok_or_else(|| NucleusError::invalid("differential has no parent full"))?;
                let parent = recs
                    .iter()
                    .find(|r| &r.id == parent_id)
                    .ok_or_else(|| NucleusError::invalid("parent full is missing from catalog"))?;
                let full_bytes = fs::read(self.dir.join(&parent.file))?;
                let patch = fs::read(self.dir.join(&rec.file))?;
                let mut out = Vec::new();
                Bspatch::new(&patch)
                    .map_err(|e| NucleusError::invalid(format!("bad patch: {e}")))?
                    .apply(&full_bytes, Cursor::new(&mut out))
                    .map_err(|e| NucleusError::invalid(format!("patch apply failed: {e}")))?;
                fs::write(dst.as_ref(), &out)?;
            }
        }
        Ok(())
    }

    /// Keep the newest `keep` full backups (and their differentials); delete the
    /// rest. Returns how many catalog entries were removed.
    pub fn prune(&self, keep: usize) -> Result<usize> {
        let recs = self.list()?;
        let full_ids: Vec<String> = recs
            .iter()
            .filter(|r| r.kind == BackupKind::Full)
            .map(|r| r.id.clone())
            .collect();
        if full_ids.len() <= keep {
            return Ok(0);
        }
        let doomed: HashSet<String> = full_ids
            .into_iter()
            .take(recs.iter().filter(|r| r.kind == BackupKind::Full).count() - keep)
            .collect();

        let mut kept = Vec::new();
        let mut removed = 0;
        for r in &recs {
            let drop = doomed.contains(&r.id)
                || r.parent.as_ref().is_some_and(|p| doomed.contains(p));
            if drop {
                let _ = fs::remove_file(self.dir.join(&r.file));
                removed += 1;
            } else {
                kept.push(r.clone());
            }
        }
        self.save_catalog(&kept)?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::MockEmbedder;
    use crate::engine::{IngestBody, QueryInput, SearchRequest};
    use crate::Engine;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn engine_at(path: &Path) -> Engine {
        let storage = Storage::open(path).unwrap();
        Engine::new(storage, Arc::new(MockEmbedder::new(32))).unwrap()
    }

    fn doc_count(e: &Engine, dom: crate::id::DomainId) -> usize {
        e.list_documents(dom, 0, 1000).unwrap().len()
    }

    #[test]
    fn full_snapshot_restores_point_in_time() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("live.redb");
        let mgr = BackupManager::open(dir.path().join("backups")).unwrap();

        let dom_id;
        let backup_id;
        {
            let e = engine_at(&db);
            let dom = e.create_domain("docs", None).unwrap();
            dom_id = dom.id;
            e.ingest_document(
                dom.id,
                "a",
                None,
                BTreeMap::new(),
                vec![],
                IngestBody::Chunks(vec!["el contrato laboral".into()]),
            )
            .unwrap();
            backup_id = mgr.full(e.storage()).unwrap().id;
            // Mutate AFTER the backup: this must NOT appear in the restore.
            e.ingest_document(
                dom.id,
                "b",
                None,
                BTreeMap::new(),
                vec![],
                IngestBody::Chunks(vec!["pizza con piña".into()]),
            )
            .unwrap();
            assert_eq!(doc_count(&e, dom_id), 2);
        }

        // Restore the snapshot into a fresh file and verify the point-in-time.
        let restored = dir.path().join("restored.redb");
        mgr.restore_to(&backup_id, &restored).unwrap();
        let e = engine_at(&restored);
        assert_eq!(doc_count(&e, dom_id), 1, "restore must reflect backup time");
        let hits = e
            .search(
                dom_id,
                SearchRequest {
                    query: QueryInput::Text("contrato".into()),
                    k: 5,
                    tags: vec![],
                    match_all: false,
                    document_ids: vec![],
                    subdomain: None,
                    filter: None,
                },
            )
            .unwrap();
        assert!(hits.iter().any(|h| h.chunk.text.contains("contrato")));
    }

    #[test]
    fn differential_captures_changes_including_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("live.redb");
        let mgr = BackupManager::open(dir.path().join("backups")).unwrap();

        let dom_id;
        let diff_id;
        {
            let e = engine_at(&db);
            let dom = e.create_domain("docs", None).unwrap();
            dom_id = dom.id;
            let a = e
                .ingest_document(
                    dom.id,
                    "a",
                    None,
                    BTreeMap::new(),
                    vec![],
                    IngestBody::Chunks(vec!["uno".into()]),
                )
                .unwrap();
            mgr.full(e.storage()).unwrap();
            // After the full: add one, delete the original.
            e.ingest_document(
                dom.id,
                "b",
                None,
                BTreeMap::new(),
                vec![],
                IngestBody::Chunks(vec!["dos".into()]),
            )
            .unwrap();
            e.delete_document(a.document.id).unwrap();
            diff_id = mgr.differential(e.storage()).unwrap().id;
        }

        // Restoring the differential must reflect BOTH the add and the delete.
        let restored = dir.path().join("restored.redb");
        mgr.restore_to(&diff_id, &restored).unwrap();
        let e = engine_at(&restored);
        let docs = e.list_documents(dom_id, 0, 1000).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title, "b");
    }

    #[test]
    fn prune_keeps_newest_fulls() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("live.redb");
        let mgr = BackupManager::open(dir.path().join("backups")).unwrap();
        let e = engine_at(&db);
        e.create_domain("docs", None).unwrap();

        // Three fulls (ids embed a second-resolution timestamp; force distinct ids).
        let mut ids = Vec::new();
        for _ in 0..3 {
            ids.push(mgr.full(e.storage()).unwrap().id);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(mgr.list().unwrap().len(), 3);
        let removed = mgr.prune(1).unwrap();
        assert_eq!(removed, 2);
        let remaining = mgr.list().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, *ids.last().unwrap());
    }
}
