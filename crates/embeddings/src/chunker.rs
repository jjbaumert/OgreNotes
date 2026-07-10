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
    /// Target chunk size in bytes (~4 bytes/token for ASCII text, so
    /// 2048 ≈ 512 tokens; multibyte scripts land fewer chars per chunk).
    pub chunk_size: usize,
    /// Overlap size in bytes between consecutive chunks.
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

/// Get the last `n` bytes of `text` (possibly fewer), breaking at a
/// word boundary.
fn tail_overlap(text: &str, n: usize) -> String {
    if text.len() <= n {
        return text.to_string();
    }
    let mut start = text.len() - n;
    // The byte budget may land inside a multibyte character; advance to
    // the next char boundary so the slice below can't panic (issue #8).
    // Advancing (not retreating) keeps the overlap within `n` bytes.
    while !text.is_char_boundary(start) {
        start += 1;
    }
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

    // ----- added coverage (see also `prop_tests` below) -----

    #[test]
    fn header_only_chunk_when_title_consumes_budget() {
        // chunk_size smaller than the title header leaves a zero content
        // budget; the degenerate single header-only chunk is returned.
        let config = ChunkerConfig {
            chunk_size: 10,
            overlap: 4,
        };
        let title = "A very long title that dwarfs the chunk size";
        let chunks = chunk_document(title, "body text", &config);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].index, 0);
        assert_eq!(chunks[0].text, format!("Title: {title}\n\n"));
    }

    #[test]
    fn oversized_paragraph_split_reassembles_with_zero_overlap() {
        // One paragraph (no double newlines) far larger than the budget
        // takes the word-split path. With overlap = 0 the stripped chunk
        // contents must reassemble to exactly the original body — words
        // are never broken and nothing is dropped or duplicated.
        let body = (0..40)
            .map(|i| format!("word{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let config = ChunkerConfig {
            chunk_size: 80,
            overlap: 0,
        };
        let chunks = chunk_document("T", &body, &config);
        assert!(chunks.len() > 1, "expected multiple chunks");
        let contents: Vec<&str> = chunks
            .iter()
            .map(|c| c.text.strip_prefix("Title: T\n\n").expect("header"))
            .collect();
        assert_eq!(contents.join(" "), body);
    }

    #[test]
    fn zero_overlap_paragraphs_land_intact_in_exactly_one_chunk() {
        let paras: Vec<String> = (0..12)
            .map(|i| format!("paragraph {i} content here"))
            .collect();
        let body = paras.join("\n\n");
        let config = ChunkerConfig {
            chunk_size: 80,
            overlap: 0,
        };
        let chunks = chunk_document("T", &body, &config);
        assert!(chunks.len() > 1, "expected multiple chunks");
        for p in &paras {
            let hits = chunks
                .iter()
                .filter(|c| c.text.contains(p.as_str()))
                .count();
            assert_eq!(hits, 1, "paragraph {p:?} appeared in {hits} chunks");
        }
    }

    #[test]
    fn overlap_tail_of_previous_chunk_leads_the_next() {
        let body = "alpha bravo charlie delta echo\n\nfoxtrot golf hotel india juliet";
        let config = ChunkerConfig {
            chunk_size: 45,
            overlap: 12,
        };
        let chunks = chunk_document("T", body, &config);
        assert_eq!(chunks.len(), 2);
        let c0 = chunks[0].text.strip_prefix("Title: T\n\n").expect("header");
        let c1 = chunks[1].text.strip_prefix("Title: T\n\n").expect("header");
        // The second chunk carries a word-aligned tail of the first as
        // overlap context, then continues with the next paragraph.
        assert!(c0.ends_with("delta echo"), "c0: {c0:?}");
        assert!(c1.starts_with("delta echo"), "c1: {c1:?}");
        assert!(c1.contains("foxtrot"));
    }

    #[test]
    fn split_at_words_keeps_overlong_word_whole() {
        // A single word longer than max_len cannot be split at a word
        // boundary; it is emitted whole as an oversized segment rather
        // than truncated or dropped.
        let word = "x".repeat(50);
        let text = format!("small {word} tail");
        let segments = split_at_words(&text, 10);
        assert!(segments.contains(&word), "segments: {segments:?}");
        assert!(segments.contains(&"small".to_string()));
        assert!(segments.contains(&"tail".to_string()));
    }

    #[test]
    fn split_at_words_collapses_arbitrary_whitespace() {
        let segments = split_at_words("a\tb\n c   d", 100);
        assert_eq!(segments, vec!["a b c d"]);
    }

    #[test]
    fn tail_overlap_returns_whole_text_when_short_enough() {
        assert_eq!(tail_overlap("short", 10), "short");
        assert_eq!(tail_overlap("exact", 5), "exact");
    }

    #[test]
    fn tail_overlap_without_space_returns_raw_tail() {
        assert_eq!(tail_overlap("abcdefghijklmnop", 5), "lmnop");
    }

    #[test]
    fn tail_overlap_zero_budget_returns_empty() {
        assert_eq!(tail_overlap("some words here", 0), "");
    }

    /// Regression: issue #8 — a byte budget landing inside a multibyte
    /// character must not panic on a non-char-boundary slice.
    #[test]
    fn tail_overlap_mid_char_budget_does_not_panic() {
        // 10 × 'α' = 20 bytes; a 5-byte tail starts at byte 15, inside
        // the 8th 'α' (bytes 14..16).
        let text = "α".repeat(10);
        let tail = tail_overlap(&text, 5);
        assert!(tail.len() <= 5);
        assert!(tail.chars().all(|c| c == 'α'));
    }

    /// Regression: issue #8 — multibyte documents long enough to span
    /// chunks must chunk without panicking, end to end.
    #[test]
    fn multibyte_multi_chunk_document_does_not_panic() {
        let config = ChunkerConfig {
            chunk_size: 64,
            overlap: 13, // odd: guaranteed to bisect a 2-byte char
        };
        // One oversized 600-byte Greek "word" forces the word-split path
        // and a tail_overlap call over pure multibyte text.
        let body = "α".repeat(300);
        let chunks = chunk_document("T", &body, &config);

        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(chunk.text.starts_with("Title: T\n\n"));
        }
    }

    #[test]
    fn unicode_body_within_single_chunk_is_preserved() {
        let body = "héllo wörld — 東京 🚀 emoji";
        let chunks = chunk_document("Ünïcode", body, &default_config());
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.starts_with("Title: Ünïcode\n\n"));
        assert!(chunks[0].text.contains(body));
    }

    #[test]
    fn split_paragraphs_drops_blank_and_whitespace_segments() {
        let paras = split_paragraphs("one\n\n\n\n  \n\ntwo");
        assert_eq!(paras, vec!["one", "two"]);
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    /// ASCII words of bounded length, grouped into paragraphs. Word
    /// length stays far below the content budget used by the properties
    /// so the oversized-word escape hatch in `split_at_words` (covered
    /// by a dedicated unit test) does not apply.
    fn paragraphs_strategy() -> impl Strategy<Value = Vec<Vec<String>>> {
        proptest::collection::vec(
            proptest::collection::vec("[a-z]{1,12}", 1..30),
            1..8,
        )
    }

    fn join_body(paragraphs: &[Vec<String>]) -> String {
        paragraphs
            .iter()
            .map(|p| p.join(" "))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    proptest! {
        #[test]
        fn chunks_are_bounded_headed_indexed_and_lossless(
            paragraphs in paragraphs_strategy(),
            chunk_size in 60usize..400,
            overlap in 0usize..40,
        ) {
            let body = join_body(&paragraphs);
            let config = ChunkerConfig { chunk_size, overlap };
            let chunks = chunk_document("T", &body, &config);

            prop_assert!(!chunks.is_empty());
            for (i, chunk) in chunks.iter().enumerate() {
                prop_assert_eq!(chunk.index, i);
                prop_assert!(chunk.text.starts_with("Title: T\n\n"));
                // header + content budget + overlap is the hard ceiling
                // when no single word exceeds the budget.
                prop_assert!(
                    chunk.text.len() <= chunk_size + overlap,
                    "chunk {} is {} bytes; ceiling {}",
                    i,
                    chunk.text.len(),
                    chunk_size + overlap
                );
            }
            // No word from the body is ever lost.
            for word in body.split_whitespace() {
                prop_assert!(
                    chunks.iter().any(|c| c.text.contains(word)),
                    "word {:?} missing from all chunks",
                    word
                );
            }
        }

        #[test]
        fn chunking_is_deterministic(paragraphs in paragraphs_strategy()) {
            let body = join_body(&paragraphs);
            let config = ChunkerConfig { chunk_size: 120, overlap: 20 };
            let a = chunk_document("T", &body, &config);
            let b = chunk_document("T", &body, &config);
            prop_assert_eq!(a.len(), b.len());
            for (x, y) in a.iter().zip(b.iter()) {
                prop_assert_eq!(x.index, y.index);
                prop_assert_eq!(&x.text, &y.text);
            }
        }
    }
}
