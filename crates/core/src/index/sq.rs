use std::collections::{HashMap, HashSet};

use super::VectorIndex;
use crate::error::NucleusError;
use crate::id::ChunkId;
use crate::Result;

/// Exact-recall brute-force index with **scalar-quantised** storage.
///
/// Like [`FlatIndex`](super::FlatIndex) it L2-normalises vectors and ranks by dot
/// product (= cosine), but it stores each component as an `i8` instead of an
/// `f32`, cutting the in-memory footprint **4×**. Quantising a unit vector to
/// `round(x * 127)` introduces a tiny per-component error (≤ 1/127), so ranking
/// is effectively unchanged for real embeddings — the trade is more vectors per
/// GB of RAM. The durable copy in storage stays full-precision `f32`; only this
/// in-memory index is compressed, and it is rebuilt from storage on startup.
#[derive(Debug, Clone)]
pub struct SqFlatIndex {
    dim: usize,
    ids: Vec<ChunkId>,
    /// Flattened row-major `i8` codes: `codes[row*dim .. row*dim+dim]` is one vector.
    codes: Vec<i8>,
    pos: HashMap<ChunkId, usize>,
}

impl SqFlatIndex {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            ids: Vec::new(),
            codes: Vec::new(),
            pos: HashMap::new(),
        }
    }

    fn row(&self, i: usize) -> &[i8] {
        &self.codes[i * self.dim..(i + 1) * self.dim]
    }
}

/// Scale factor for symmetric int8 quantisation of a unit vector.
const SCALE: f32 = 127.0;

/// L2-normalise then quantise each component to `i8` (`round(x * 127)`).
fn quantize(v: &[f32]) -> Vec<i8> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let inv = if norm > 0.0 { 1.0 / norm } else { 0.0 };
    v.iter()
        .map(|x| ((x * inv) * SCALE).round().clamp(-SCALE, SCALE) as i8)
        .collect()
}

impl VectorIndex for SqFlatIndex {
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
        let q = quantize(vector);
        if let Some(&i) = self.pos.get(&id) {
            self.codes[i * self.dim..(i + 1) * self.dim].copy_from_slice(&q);
        } else {
            let i = self.ids.len();
            self.ids.push(id);
            self.codes.extend_from_slice(&q);
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
            let (head, tail) = self.codes.split_at_mut(last * self.dim);
            head[i * self.dim..(i + 1) * self.dim].copy_from_slice(&tail[..self.dim]);
            let moved = self.ids[last];
            self.ids[i] = moved;
            self.pos.insert(moved, i);
        }
        self.ids.pop();
        self.codes.truncate(last * self.dim);
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
        // Normalise the query in f32; dequantise stored codes on the fly.
        let norm = query.iter().map(|x| x * x).sum::<f32>().sqrt();
        let inv = if norm > 0.0 { 1.0 / norm } else { 0.0 };
        let q: Vec<f32> = query.iter().map(|x| x * inv).collect();

        let mut scored: Vec<(ChunkId, f32)> = self
            .ids
            .iter()
            .enumerate()
            .filter(|(_, id)| allowed.is_none_or(|set| set.contains(id)))
            .map(|(i, &id)| {
                let row = self.row(i);
                let dot: f32 = q.iter().zip(row).map(|(x, c)| x * (*c as f32)).sum();
                (id, dot / SCALE)
            })
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
    fn ranks_by_cosine_like_flat() {
        let mut ix = SqFlatIndex::new(2);
        ix.upsert(id(1), &[1.0, 0.0]).unwrap();
        ix.upsert(id(2), &[0.0, 1.0]).unwrap();
        ix.upsert(id(3), &[1.0, 1.0]).unwrap();

        let hits = ix.search(&[1.0, 0.0], 3, None);
        assert_eq!(hits[0].0, id(1));
        assert_eq!(hits[1].0, id(3)); // 45° beats 90°
        assert_eq!(hits[2].0, id(2));
        // Score of the identical direction is ~1.0 (quantisation error is tiny).
        assert!((hits[0].1 - 1.0).abs() < 0.02);
    }

    #[test]
    fn respects_allowed_and_remove() {
        let mut ix = SqFlatIndex::new(2);
        ix.upsert(id(1), &[1.0, 0.0]).unwrap();
        ix.upsert(id(2), &[0.9, 0.1]).unwrap();
        let allowed: HashSet<ChunkId> = [id(2)].into_iter().collect();
        assert_eq!(ix.search(&[1.0, 0.0], 5, Some(&allowed))[0].0, id(2));

        ix.remove(id(1));
        assert_eq!(ix.len(), 1);
        assert_eq!(ix.search(&[1.0, 0.0], 1, None)[0].0, id(2));
    }

    #[test]
    fn rejects_wrong_dimension() {
        let mut ix = SqFlatIndex::new(3);
        assert!(matches!(
            ix.upsert(id(1), &[1.0, 0.0]),
            Err(NucleusError::DimensionMismatch { .. })
        ));
    }
}
