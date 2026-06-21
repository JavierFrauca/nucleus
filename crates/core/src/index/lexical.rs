//! In-memory BM25 lexical index, kept per domain alongside the vector index.
//!
//! Dense (cosine) retrieval misses exact terms — law numbers, "art. 14", years.
//! BM25 nails those. The engine fuses both with Reciprocal Rank Fusion, which is
//! robust to the very different score scales (and to e5's anisotropic cosines).
//!
//! Like the vector index it lives in memory and is rebuilt from storage on
//! startup; deletions are tombstoned (BM25 stats drift negligibly until rebuild).

use std::collections::{HashMap, HashSet};

use crate::id::ChunkId;

const K1: f32 = 1.2;
const B: f32 = 0.75;

/// Tokenise into lowercased alphanumeric terms (Unicode-aware, keeps accents).
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .map(|t| t.to_lowercase())
        .collect()
}

/// BM25 over chunk texts.
#[derive(Default)]
pub struct LexicalIndex {
    /// term -> postings (chunk id, term frequency).
    postings: HashMap<String, Vec<(ChunkId, u32)>>,
    /// chunk id -> document length (token count).
    doc_len: HashMap<ChunkId, u32>,
    tombstones: HashSet<ChunkId>,
    total_len: u64,
}

impl LexicalIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of live documents indexed.
    pub fn len(&self) -> usize {
        self.doc_len.len() - self.tombstones.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn add(&mut self, id: ChunkId, text: &str) {
        self.tombstones.remove(&id);
        let tokens = tokenize(text);
        let dl = tokens.len() as u32;
        let mut tf: HashMap<String, u32> = HashMap::new();
        for t in tokens {
            *tf.entry(t).or_insert(0) += 1;
        }
        for (term, freq) in tf {
            self.postings.entry(term).or_default().push((id, freq));
        }
        self.total_len += dl as u64;
        self.doc_len.insert(id, dl);
    }

    pub fn remove(&mut self, id: ChunkId) {
        if self.doc_len.contains_key(&id) {
            self.tombstones.insert(id);
        }
    }

    /// Rank live chunks by BM25 for `query`, returning up to `k` `(chunk, score)`
    /// pairs, highest first. `allowed`, if set, restricts the candidates.
    pub fn search(
        &self,
        query: &str,
        k: usize,
        allowed: Option<&HashSet<ChunkId>>,
    ) -> Vec<(ChunkId, f32)> {
        let live = self.len();
        if k == 0 || live == 0 {
            return Vec::new();
        }
        let n = live as f32;
        let avgdl = (self.total_len as f32) / (self.doc_len.len() as f32).max(1.0);

        let mut terms: Vec<String> = tokenize(query);
        terms.sort();
        terms.dedup();

        let mut scores: HashMap<ChunkId, f32> = HashMap::new();
        for term in &terms {
            let Some(postings) = self.postings.get(term) else {
                continue;
            };
            let df = postings.len() as f32;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for &(id, freq) in postings {
                if self.tombstones.contains(&id) {
                    continue;
                }
                if let Some(set) = allowed {
                    if !set.contains(&id) {
                        continue;
                    }
                }
                let dl = *self.doc_len.get(&id).unwrap_or(&0) as f32;
                let tf = freq as f32;
                let norm = tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * dl / avgdl));
                *scores.entry(id).or_insert(0.0) += idf * norm;
            }
        }

        let mut ranked: Vec<(ChunkId, f32)> = scores.into_iter().collect();
        ranked.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        ranked.truncate(k);
        ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> ChunkId {
        ChunkId::new(n)
    }

    #[test]
    fn ranks_and_filters() {
        let mut ix = LexicalIndex::new();
        ix.add(id(1), "contrato laboral indefinido");
        ix.add(id(2), "receta de pizza con piña");
        ix.add(id(3), "contrato mercantil de servicios");

        let hits = ix.search("contrato laboral", 5, None);
        assert_eq!(hits[0].0, id(1)); // matches both terms
        assert!(hits.iter().any(|(c, _)| *c == id(3))); // matches "contrato"
        assert!(hits.iter().all(|(c, _)| *c != id(2))); // no shared terms

        let allowed: HashSet<ChunkId> = [id(3)].into_iter().collect();
        let scoped = ix.search("contrato", 5, Some(&allowed));
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].0, id(3));
    }

    #[test]
    fn tombstone_excludes() {
        let mut ix = LexicalIndex::new();
        ix.add(id(1), "contrato laboral");
        ix.add(id(2), "contrato mercantil");
        ix.remove(id(1));
        assert_eq!(ix.len(), 1);
        let hits = ix.search("contrato", 5, None);
        assert!(hits.iter().all(|(c, _)| *c != id(1)));
    }
}
