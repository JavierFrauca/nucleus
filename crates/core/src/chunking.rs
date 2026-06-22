//! Splitting document text into chunks.
//!
//! The HTTP API accepts either pre-split `chunks[]` or raw `text`; in the latter
//! case the engine runs a [`Chunker`]. The default [`FixedSizeChunker`] is a
//! character-windowed splitter with overlap that is **boundary-aware**: instead
//! of slicing blindly at `start + size` (which cuts mid-word, hurting both the
//! embedding and any literal match), it backtracks within the tail of the window
//! to the last sentence boundary, falling back to the last whitespace, so chunks
//! end on a natural break whenever one exists nearby. A token-aware splitter can
//! be added later behind the same trait.

/// Splits a document body into retrievable chunks.
pub trait Chunker: Send + Sync {
    fn chunk(&self, text: &str) -> Vec<String>;
}

/// Fixed-size sliding window over Unicode scalar values, with overlap between
/// consecutive windows and **boundary-aware** cutting.
#[derive(Debug, Clone, Copy)]
pub struct FixedSizeChunker {
    /// Window size in characters (target upper bound).
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

    /// How far back from the hard window end we are willing to move the cut to
    /// land on a natural boundary, in characters. Bounded so chunks never shrink
    /// below ~75% of `size` just to find a break.
    fn lookback(&self) -> usize {
        self.size / 4
    }

    /// Given the window `chars[start..hard_end]` (with `hard_end < len`, i.e. not
    /// the final window), pick a cut position in `(start, hard_end]` that lands on
    /// a sentence boundary if one exists within [`lookback`](Self::lookback),
    /// otherwise on the last whitespace there, otherwise the hard end (e.g. a
    /// single very long token with no break nearby).
    fn boundary_end(&self, chars: &[char], start: usize, hard_end: usize) -> usize {
        let floor = hard_end.saturating_sub(self.lookback()).max(start + 1);
        // Prefer a sentence terminator (cut just *after* it, keeping the mark).
        for i in (floor..hard_end).rev() {
            if matches!(chars[i], '.' | '!' | '?' | '\n' | '。' | '！' | '？') {
                return i + 1;
            }
        }
        // Otherwise the last whitespace in the lookback window (cut at it).
        for i in (floor..hard_end).rev() {
            if chars[i].is_whitespace() {
                return i + 1;
            }
        }
        hard_end
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
        let mut out = Vec::new();
        let mut start = 0;
        while start < chars.len() {
            let hard_end = (start + self.size).min(chars.len());
            let end = if hard_end == chars.len() {
                hard_end
            } else {
                self.boundary_end(&chars, start, hard_end)
            };
            let piece: String = chars[start..end].iter().collect();
            let trimmed = piece.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
            if end == chars.len() {
                break;
            }
            // Advance, keeping `overlap` chars of context, but always making
            // progress (the boundary cut can fall close to `start`).
            let next = end.saturating_sub(self.overlap).max(start + 1);
            start = next;
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

    #[test]
    fn breaks_on_sentence_boundary_not_mid_word() {
        // Window of 24 chars; the first sentence ends at char 20, inside the
        // lookback tail (24/4 = 6 → floor 18), so the cut lands on the period.
        let c = FixedSizeChunker::new(24, 4);
        let text = "Primera frase corta. Segunda parte.";
        let chunks = c.chunk(text);
        assert_eq!(chunks[0], "Primera frase corta.");
        // No chunk ends mid-word.
        for ch in &chunks {
            assert!(!ch.ends_with("Segund"));
        }
    }

    #[test]
    fn falls_back_to_whitespace_when_no_sentence_break() {
        let c = FixedSizeChunker::new(12, 2);
        // No sentence punctuation: should cut on a space, never mid-word.
        let chunks = c.chunk("alpha bravo charlie delta");
        for ch in &chunks {
            // Every emitted chunk is whitespace-trimmed whole words.
            assert!(!ch.starts_with(' ') && !ch.ends_with(' '));
        }
        // Reassembling (accounting for overlap) still covers all words.
        assert!(chunks.iter().any(|c| c.contains("alpha")));
        assert!(chunks.iter().any(|c| c.contains("delta")));
    }

    #[test]
    fn long_unbreakable_token_still_progresses() {
        // A single long token with no boundary must not loop forever; it falls
        // back to a hard cut and keeps advancing.
        let c = FixedSizeChunker::new(4, 2);
        let chunks = c.chunk(&"x".repeat(20));
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|s| s.chars().all(|ch| ch == 'x')));
    }
}
