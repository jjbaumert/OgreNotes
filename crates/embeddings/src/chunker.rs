// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

/// A single chunk of text extracted from a document.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Zero-based chunk index within the document.
    pub index: usize,
    /// The text content including the title header prefix.
    pub text: String,
}

/// Configuration for text chunking.
pub struct ChunkerConfig {
    /// Target chunk size in characters (~4 chars/token, so 2048 ≈ 512 tokens).
    pub chunk_size: usize,
    /// Overlap size in characters between consecutive chunks.
    pub overlap: usize,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            chunk_size: 2048,
            overlap: 204,
        }
    }
}

/// Split `body` into overlapping chunks, each prefixed with a title header.
///
/// Strategy:
/// - Split body into paragraphs (on double newline)
/// - Accumulate paragraphs into chunks up to `chunk_size`
/// - If a single paragraph exceeds `chunk_size`, split at word boundaries
/// - Overlap by including trailing characters from the previous chunk
pub fn chunk_document(title: &str, body: &str, config: &ChunkerConfig) -> Vec<Chunk> {
    let body = body.trim();
    if body.is_empty() {
        return Vec::new();
    }

    let header = format!("Title: {title}\n\n");
    let content_budget = config.chunk_size.saturating_sub(header.len());
    if content_budget == 0 {
        return vec![Chunk {
            index: 0,
            text: header,
        }];
    }

    // Split into paragraph segments
    let paragraphs = split_paragraphs(body);

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut overlap_text = String::new();

    for para in &paragraphs {
        // If this paragraph alone exceeds budget, split it at word boundaries
        if para.len() > content_budget {
            // Flush current accumulator first
            if !current.is_empty() {
                overlap_text = tail_overlap(&current, config.overlap);
                chunks.push(Chunk {
                    index: chunks.len(),
                    text: format!("{header}{current}"),
                });
                current.clear();
            }

            for word_chunk in split_at_words(para, content_budget) {
                let mut text = overlap_text.clone();
                text.push_str(&word_chunk);
                overlap_text = tail_overlap(&text, config.overlap);
                chunks.push(Chunk {
                    index: chunks.len(),
                    text: format!("{header}{text}"),
                });
            }
            continue;
        }

        // Check if adding this paragraph would exceed budget
        let separator = if current.is_empty() { "" } else { "\n\n" };
        let new_len = current.len() + separator.len() + para.len();

        if new_len > content_budget && !current.is_empty() {
            // Flush current chunk
            overlap_text = tail_overlap(&current, config.overlap);
            chunks.push(Chunk {
                index: chunks.len(),
                text: format!("{header}{current}"),
            });
            // Start new chunk with overlap + current paragraph
            current = format!("{overlap_text}{para}");
        } else {
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(para);
        }
    }

    // Flush remaining
    if !current.is_empty() {
        chunks.push(Chunk {
            index: chunks.len(),
            text: format!("{header}{current}"),
        });
    }

    chunks
}

/// Split text into paragraphs on double-newline boundaries.
fn split_paragraphs(text: &str) -> Vec<String> {
    text.split("\n\n")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split a long string at word boundaries into segments of at most `max_len`.
fn split_at_words(text: &str, max_len: usize) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let separator = if current.is_empty() { "" } else { " " };
        if current.len() + separator.len() + word.len() > max_len && !current.is_empty() {
            segments.push(current);
            current = word.to_string();
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }

    if !current.is_empty() {
        segments.push(current);
    }

    segments
}

/// Get the last `n` characters of `text`, breaking at a word boundary.
fn tail_overlap(text: &str, n: usize) -> String {
    if text.len() <= n {
        return text.to_string();
    }
    let start = text.len() - n;
    // Find the next word boundary after `start`
    match text[start..].find(' ') {
        Some(pos) => text[start + pos..].trim_start().to_string(),
        None => text[start..].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ChunkerConfig {
        ChunkerConfig::default()
    }

    #[test]
    fn empty_body_returns_no_chunks() {
        let chunks = chunk_document("Test", "", &default_config());
        assert!(chunks.is_empty());
    }

    #[test]
    fn whitespace_only_body_returns_no_chunks() {
        let chunks = chunk_document("Test", "   \n\n  ", &default_config());
        assert!(chunks.is_empty());
    }

    #[test]
    fn short_text_single_chunk() {
        let chunks = chunk_document("My Doc", "Hello world", &default_config());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].index, 0);
        assert!(chunks[0].text.starts_with("Title: My Doc\n\n"));
        assert!(chunks[0].text.contains("Hello world"));
    }

    #[test]
    fn title_header_prepended_to_every_chunk() {
        let body = "a ".repeat(2000); // force multiple chunks
        let config = ChunkerConfig {
            chunk_size: 200,
            overlap: 20,
        };
        let chunks = chunk_document("Doc Title", &body, &config);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(
                chunk.text.starts_with("Title: Doc Title\n\n"),
                "Chunk {} missing header: {:?}",
                chunk.index,
                &chunk.text[..40]
            );
        }
    }

    #[test]
    fn chunks_respect_size_limit() {
        let body = (0..50)
            .map(|i| format!("Paragraph number {i} with some content to fill space."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let config = ChunkerConfig {
            chunk_size: 300,
            overlap: 30,
        };
        let chunks = chunk_document("Test", &body, &config);
        for chunk in &chunks {
            assert!(
                chunk.text.len() <= config.chunk_size + config.overlap + 50, // allow margin for header + word boundary
                "Chunk {} too large: {} chars",
                chunk.index,
                chunk.text.len()
            );
        }
    }

    #[test]
    fn chunk_indices_sequential() {
        let body = "para one\n\npara two\n\npara three\n\npara four";
        let config = ChunkerConfig {
            chunk_size: 40,
            overlap: 5,
        };
        let chunks = chunk_document("T", &body, &config);
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i);
        }
    }

    #[test]
    fn paragraphs_split_on_double_newline() {
        let paras = split_paragraphs("one\n\ntwo\n\nthree");
        assert_eq!(paras, vec!["one", "two", "three"]);
    }

    #[test]
    fn split_at_words_respects_limit() {
        let text = "the quick brown fox jumps over the lazy dog";
        let segments = split_at_words(text, 20);
        for seg in &segments {
            assert!(seg.len() <= 20, "Segment too long: {seg}");
        }
        // Reassembled text should match original words
        let reassembled = segments.join(" ");
        assert_eq!(reassembled, text);
    }

    #[test]
    fn tail_overlap_at_word_boundary() {
        let text = "the quick brown fox jumps";
        let overlap = tail_overlap(text, 10);
        // Last 10 chars: "fox jumps", word-boundary adjusted
        assert!(!overlap.starts_with(' '));
        assert!(text.ends_with(&overlap));
    }
}
