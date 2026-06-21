use super::Embedder;
use crate::Result;

/// Deterministic, dependency-free embedder for tests.
///
/// It produces a bag-of-words vector: each whitespace token is hashed (FNV-1a)
/// into one dimension and increments it. Texts that share words get similar
/// vectors, which is enough to exercise search ranking without loading a model.
#[derive(Debug, Clone)]
pub struct MockEmbedder {
    dim: usize,
}

impl MockEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(1) }
    }
}

impl Default for MockEmbedder {
    fn default() -> Self {
        Self::new(32)
    }
}

fn embed_text(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    for token in text.split_whitespace() {
        let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
        for b in token.to_lowercase().bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        v[(h % dim as u64) as usize] += 1.0;
    }
    v
}

impl Embedder for MockEmbedder {
    fn dim(&self, _model: &str) -> Option<usize> {
        Some(self.dim)
    }

    fn embed_documents(&self, _model: &str, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| embed_text(t, self.dim)).collect())
    }

    fn embed_query(&self, _model: &str, text: &str) -> Result<Vec<f32>> {
        Ok(embed_text(text, self.dim))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_words_increase_similarity() {
        let e = MockEmbedder::new(64);
        let a = &e
            .embed_documents("m", &["el contrato laboral".into()])
            .unwrap()[0];
        let q = e.embed_query("m", "contrato laboral").unwrap();
        let unrelated = e.embed_query("m", "pizza con piña").unwrap();

        let dot = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| a * b).sum::<f32>();
        assert!(dot(a, &q) > dot(a, &unrelated));
    }
}
