//! Splitting document text into chunks.
//!
//! The HTTP API accepts either pre-split `chunks[]` or raw `text`; in the latter
//! case the engine runs a [`Chunker`]. The MVP ships [`FixedSizeChunker`], a
//! character-windowed splitter with overlap. A token-aware splitter can be added
//! later behind the same trait.

/// Splits a document body into retrievable chunks.
pub trait Chunker: Send + Sync {
    fn chunk(&self, text: &str) -> Vec<String>;
}

/// Fixed-size sliding window over Unicode scalar values, with overlap between
/// consecutive windows.
#[derive(Debug, Clone, Copy)]
pub struct FixedSizeChunker {
    /// Window size in characters.
    pub size: usize,
    /// Overlap in characters between adjacent windows (clamped to `size - 1`).
    pub overlap: usize,
}

impl FixedSizeChunker {
    pub fn new(size: usize, overlap: usize) -> Self {
        let size = size.max(1);
        let overlap = overlap.min(size - 1);
        Self { size, overlap }
    }
}

impl Default for FixedSizeChunker {
    fn default() -> Self {
        Self::new(1000, 200)
    }
}

impl Chunker for FixedSizeChunker {
    fn chunk(&self, text: &str) -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return Vec::new();
        }
        let step = self.size - self.overlap; // size >= 1 and overlap <= size-1 => step >= 1
        let mut out = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let end = (start + self.size).min(chars.len());
            let piece: String = chars[start..end].iter().collect();
            let trimmed = piece.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            if end == chars.len() {
                break;
            }
            start += step;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_yields_no_chunks() {
        assert!(FixedSizeChunker::default().chunk("").is_empty());
        assert!(FixedSizeChunker::default().chunk("   ").is_empty());
    }

    #[test]
    fn windows_with_overlap() {
        let c = FixedSizeChunker::new(4, 2);
        // "abcdefgh" -> [abcd, cdef, efgh, ...]
        let chunks = c.chunk("abcdefgh");
        assert_eq!(chunks[0], "abcd");
        assert_eq!(chunks[1], "cdef");
        assert_eq!(chunks[2], "efgh");
    }

    #[test]
    fn short_text_is_single_chunk() {
        let c = FixedSizeChunker::new(1000, 200);
        assert_eq!(c.chunk("hola mundo"), vec!["hola mundo".to_string()]);
    }
}
