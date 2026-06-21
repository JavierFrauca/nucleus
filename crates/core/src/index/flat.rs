use std::collections::{HashMap, HashSet};

use super::VectorIndex;
use crate::error::NucleusError;
use crate::id::ChunkId;
use crate::Result;

/// Exact, in-memory brute-force index.
///
/// Vectors are L2-normalised on insertion, so cosine similarity reduces to a dot
/// product. Rows are stored in a single flat `Vec<f32>` for cache-friendly scans;
/// a side map gives O(1) upsert/remove by [`ChunkId`] via swap-remove.
#[derive(Debug, Clone)]
pub struct FlatIndex {
    dim: usize,
    ids: Vec<ChunkId>,
    /// Flattened, row-major: `data[row*dim .. row*dim+dim]` is one normalised vector.
    data: Vec<f32>,
    pos: HashMap<ChunkId, usize>,
}

impl FlatIndex {
    /// Create an empty index for vectors of dimension `dim`.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            ids: Vec::new(),
            data: Vec::new(),
            pos: HashMap::new(),
        }
    }

    fn row(&self, i: usize) -> &[f32] {
        &self.data[i * self.dim..(i + 1) * self.dim]
    }
}

/// L2-normalise `v` into a fresh vector. A zero vector is returned unchanged.
fn normalized(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter().map(|x| x / norm).collect()
    } else {
        v.to_vec()
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

impl VectorIndex for FlatIndex {
    fn dim(&self) -> usize {
        self.dim
    }

    fn len(&self) -> usize {
        self.ids.len()
    }

    fn upsert(&mut self, id: ChunkId, vector: &[f32]) -> Result<()> {
        if vector.len() != self.dim {
            return Err(NucleusError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        let norm = normalized(vector);
        if let Some(&i) = self.pos.get(&id) {
            self.data[i * self.dim..(i + 1) * self.dim].copy_from_slice(&norm);
        } else {
            let i = self.ids.len();
            self.ids.push(id);
            self.data.extend_from_slice(&norm);
            self.pos.insert(id, i);
        }
        Ok(())
    }

    fn remove(&mut self, id: ChunkId) {
        let Some(i) = self.pos.remove(&id) else {
            return;
        };
        let last = self.ids.len() - 1;
        if i != last {
            // Move the last row into the hole, then truncate.
            let (head, tail) = self.data.split_at_mut(last * self.dim);
            head[i * self.dim..(i + 1) * self.dim].copy_from_slice(&tail[..self.dim]);
            let moved = self.ids[last];
            self.ids[i] = moved;
            self.pos.insert(moved, i);
        }
        self.ids.pop();
        self.data.truncate(last * self.dim);
    }

    fn search(
        &self,
        query: &[f32],
        k: usize,
        allowed: Option<&HashSet<ChunkId>>,
    ) -> Vec<(ChunkId, f32)> {
        if k == 0 || self.ids.is_empty() || query.len() != self.dim {
            return Vec::new();
        }
        let q = normalized(query);
        let mut scored: Vec<(ChunkId, f32)> = self
            .ids
            .iter()
            .enumerate()
            .filter(|(_, id)| allowed.is_none_or(|set| set.contains(id)))
            .map(|(i, &id)| (id, dot(&q, self.row(i))))
            .collect();
        scored.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(k);
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> ChunkId {
        ChunkId::new(n)
    }

    #[test]
    fn ranks_by_cosine_similarity() {
        let mut ix = FlatIndex::new(2);
        ix.upsert(id(1), &[1.0, 0.0]).unwrap();
        ix.upsert(id(2), &[0.0, 1.0]).unwrap();
        ix.upsert(id(3), &[1.0, 1.0]).unwrap();

        let hits = ix.search(&[1.0, 0.0], 3, None);
        assert_eq!(hits[0].0, id(1));
        // [1,1] (45°) should outrank [0,1] (90°) for a [1,0] query.
        assert_eq!(hits[1].0, id(3));
        assert_eq!(hits[2].0, id(2));
    }

    #[test]
    fn respects_allowed_filter() {
        let mut ix = FlatIndex::new(2);
        ix.upsert(id(1), &[1.0, 0.0]).unwrap();
        ix.upsert(id(2), &[0.9, 0.1]).unwrap();
        let allowed: HashSet<ChunkId> = [id(2)].into_iter().collect();
        let hits = ix.search(&[1.0, 0.0], 5, Some(&allowed));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, id(2));
    }

    #[test]
    fn upsert_replaces_and_remove_works() {
        let mut ix = FlatIndex::new(2);
        ix.upsert(id(1), &[1.0, 0.0]).unwrap();
        ix.upsert(id(1), &[0.0, 1.0]).unwrap();
        assert_eq!(ix.len(), 1);
        let hits = ix.search(&[0.0, 1.0], 1, None);
        assert!((hits[0].1 - 1.0).abs() < 1e-6);

        ix.upsert(id(2), &[1.0, 0.0]).unwrap();
        ix.remove(id(1));
        assert_eq!(ix.len(), 1);
        assert_eq!(ix.search(&[1.0, 0.0], 1, None)[0].0, id(2));
    }

    #[test]
    fn rejects_wrong_dimension() {
        let mut ix = FlatIndex::new(3);
        assert!(matches!(
            ix.upsert(id(1), &[1.0, 0.0]),
            Err(NucleusError::DimensionMismatch {
                expected: 3,
                got: 2
            })
        ));
    }
}
