use std::collections::HashSet;
use std::path::Path;

use hnsw_rs::prelude::*;
use serde::{Deserialize, Serialize};

use super::VectorIndex;
use crate::error::NucleusError;
use crate::id::ChunkId;
use crate::Result;

/// A loaded HNSW graph borrows from the [`HnswIo`] that produced it
/// (`load_hnsw` is `fn(&'a mut self) -> Hnsw<'b>` with `'a: 'b`), so we keep the
/// two together in this self-referential holder.
#[ouroboros::self_referencing]
struct Loaded {
    io: HnswIo,
    #[borrows(mut io)]
    #[not_covariant]
    hnsw: Hnsw<'this, f32, DistCosine>,
}

/// Either a freshly-built (owned) graph or one reloaded from disk.
enum Graph {
    Fresh(Hnsw<'static, f32, DistCosine>),
    Loaded(Loaded),
}

/// Sidecar persisted next to the graph dump (our bookkeeping that hnsw_rs does
/// not store).
#[derive(Serialize, Deserialize)]
struct Sidecar {
    dim: usize,
    live: usize,
    tombstones: Vec<u64>,
}

/// Approximate nearest-neighbour index backed by `hnsw_rs` (an HNSW graph).
///
/// HNSW has no native deletion, so [`remove`](HnswIndex::remove) records a
/// tombstone and `search` over-fetches and filters them out. With a
/// tag/document/query pre-filter results are *approximate* (HNSW ranks globally
/// and we then intersect). The graph can be [`save`](HnswIndex::save)d and
/// [`load`](HnswIndex::load)ed to skip rebuilding from storage on restart.
pub struct HnswIndex {
    dim: usize,
    graph: Graph,
    tombstones: HashSet<ChunkId>,
    live: usize,
}

fn new_graph() -> Hnsw<'static, f32, DistCosine> {
    // max_nb_connection, max_elements (allocation hint, may be exceeded),
    // max_layer, ef_construction, distance.
    Hnsw::new(24, 10_000, 16, 200, DistCosine)
}

fn sidecar_path(dir: &Path, basename: &str) -> std::path::PathBuf {
    dir.join(format!("{basename}.meta"))
}

impl HnswIndex {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            graph: Graph::Fresh(new_graph()),
            tombstones: HashSet::new(),
            live: 0,
        }
    }

    /// Run a closure with a shared reference to the underlying graph, regardless
    /// of whether it is fresh or reloaded.
    fn with_graph<R>(&self, f: impl FnOnce(&Hnsw<'_, f32, DistCosine>) -> R) -> R {
        match &self.graph {
            Graph::Fresh(h) => f(h),
            Graph::Loaded(loaded) => loaded.with_hnsw(f),
        }
    }

    /// Dump the graph plus our bookkeeping to `dir` under `basename`.
    pub fn save(&self, dir: &Path, basename: &str) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        self.with_graph(|h| h.file_dump(dir, basename))
            .map_err(|e| NucleusError::Io(std::io::Error::other(format!("hnsw dump: {e}"))))?;
        let sidecar = Sidecar {
            dim: self.dim,
            live: self.live,
            tombstones: self.tombstones.iter().map(|c| c.get()).collect(),
        };
        std::fs::write(
            sidecar_path(dir, basename),
            crate::storage::codec::encode(&sidecar)?,
        )?;
        Ok(())
    }

    /// Reload a previously [`save`](HnswIndex::save)d graph.
    pub fn load(dir: &Path, basename: &str) -> Result<Self> {
        let sidecar: Sidecar = {
            let bytes = std::fs::read(sidecar_path(dir, basename))?;
            crate::storage::codec::decode(&bytes)?
        };
        let io = HnswIo::new(dir, basename);
        let loaded = Loaded::try_new(io, |io| {
            io.load_hnsw::<f32, DistCosine>()
                .map_err(|e| NucleusError::Io(std::io::Error::other(format!("hnsw load: {e}"))))
        })?;
        Ok(Self {
            dim: sidecar.dim,
            graph: Graph::Loaded(loaded),
            tombstones: sidecar.tombstones.into_iter().map(ChunkId::new).collect(),
            live: sidecar.live,
        })
    }
}

impl VectorIndex for HnswIndex {
    fn dim(&self) -> usize {
        self.dim
    }

    fn len(&self) -> usize {
        self.live
    }

    fn upsert(&mut self, id: ChunkId, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dim {
            return Err(NucleusError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        self.tombstones.remove(&id);
        self.with_graph(|h| h.insert((vector, id.get() as usize)));
        self.live += 1;
        Ok(())
    }

    fn remove(&mut self, id: ChunkId) {
        if self.tombstones.insert(id) && self.live > 0 {
            self.live -= 1;
        }
    }

    fn search(
        &self,
        query: &[f32],
        k: usize,
        allowed: Option<&HashSet<ChunkId>>,
    ) -> Vec<(ChunkId, f32)> {
        let total = self.with_graph(|h| h.get_nb_point());
        if k == 0 || query.len() != self.dim || total == 0 {
            return Vec::new();
        }
        // Over-fetch to absorb tombstones and the post-filter intersection.
        let want = (k.saturating_mul(4) + self.tombstones.len()).clamp(1, total);
        let ef = want.max(64);
        let neighbours = self.with_graph(|h| h.search(query, want, ef));

        let mut out = Vec::with_capacity(k);
        for neighbour in neighbours {
            let id = ChunkId::new(neighbour.d_id as u64);
            if self.tombstones.contains(&id) {
                continue;
            }
            if let Some(set) = allowed {
                if !set.contains(&id) {
                    continue;
                }
            }
            out.push((id, 1.0 - neighbour.distance)); // distance = 1 - cosine
            if out.len() >= k {
                break;
            }
        }
        out
    }

    fn persist(&self, dir: &Path, name: &str) -> Result<bool> {
        self.save(dir, name)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> ChunkId {
        ChunkId::new(n)
    }

    #[test]
    fn ranks_nearest_first() {
        let mut ix = HnswIndex::new(2);
        ix.upsert(id(1), &[1.0, 0.0]).unwrap();
        ix.upsert(id(2), &[0.0, 1.0]).unwrap();
        ix.upsert(id(3), &[0.92, 0.1]).unwrap();
        assert_eq!(ix.len(), 3);
        assert_eq!(ix.search(&[1.0, 0.0], 2, None)[0].0, id(1));
    }

    #[test]
    fn tombstones_are_excluded() {
        let mut ix = HnswIndex::new(2);
        ix.upsert(id(1), &[1.0, 0.0]).unwrap();
        ix.upsert(id(2), &[0.9, 0.1]).unwrap();
        ix.remove(id(1));
        assert_eq!(ix.len(), 1);
        assert!(ix
            .search(&[1.0, 0.0], 5, None)
            .iter()
            .all(|(c, _)| *c != id(1)));
    }

    #[test]
    fn rejects_wrong_dimension() {
        let mut ix = HnswIndex::new(3);
        assert!(matches!(
            ix.upsert(id(1), &[1.0, 0.0]),
            Err(NucleusError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn save_and_reload_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut ix = HnswIndex::new(2);
            ix.upsert(id(1), &[1.0, 0.0]).unwrap();
            ix.upsert(id(2), &[0.0, 1.0]).unwrap();
            ix.upsert(id(3), &[0.92, 0.1]).unwrap();
            ix.remove(id(2));
            ix.save(dir.path(), "dom1").unwrap();
        }
        let reloaded = HnswIndex::load(dir.path(), "dom1").unwrap();
        assert_eq!(reloaded.dim(), 2);
        assert_eq!(reloaded.len(), 2); // 3 inserted - 1 tombstoned
        let hits = reloaded.search(&[1.0, 0.0], 3, None);
        assert_eq!(hits[0].0, id(1));
        assert!(hits.iter().all(|(c, _)| *c != id(2))); // tombstone survived reload
    }

    #[test]
    fn insert_after_reload() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut ix = HnswIndex::new(2);
            ix.upsert(id(1), &[1.0, 0.0]).unwrap();
            ix.save(dir.path(), "dom2").unwrap();
        }
        let mut reloaded = HnswIndex::load(dir.path(), "dom2").unwrap();
        reloaded.upsert(id(5), &[0.0, 1.0]).unwrap();
        let hits = reloaded.search(&[0.0, 1.0], 1, None);
        assert_eq!(hits[0].0, id(5));
    }
}
