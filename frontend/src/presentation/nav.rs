// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pure slide-navigation helpers shared by the deck editor and present
//! mode. `active_slide` is a positional index in the UI, but live
//! follow (P2) broadcasts a slide `block_id` — these functions are the
//! only place that mapping lives.

use super::model::Deck;

pub fn next_index(current: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    (current + 1).min(len - 1)
}

pub fn prev_index(current: usize) -> usize {
    current.saturating_sub(1)
}

pub fn index_of_slide(deck: &Deck, block_id: &str) -> Option<usize> {
    deck.slides.iter().position(|s| s.block_id == block_id)
}

pub fn slide_block_id(deck: &Deck, index: usize) -> Option<String> {
    deck.slides.get(index).map(|s| s.block_id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::model::{Deck, DeckSlide, DEFAULT_THEME};

    fn deck_with(ids: &[&str]) -> Deck {
        Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: ids
                .iter()
                .map(|id| DeckSlide {
                    block_id: (*id).to_string(),
                    layout: "blank".to_string(),
                    background: None,
                    frames: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn next_clamps_at_the_last_slide() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(2, 3), 2, "no wrap past the end");
        assert_eq!(next_index(0, 0), 0, "empty deck is inert");
        assert_eq!(next_index(9, 3), 2, "out-of-range clamps into the deck");
    }

    #[test]
    fn prev_clamps_at_the_first_slide() {
        assert_eq!(prev_index(2), 1);
        assert_eq!(prev_index(0), 0, "no wrap before the start");
    }

    #[test]
    fn block_id_and_index_round_trip() {
        let d = deck_with(&["s1", "s2", "s3"]);
        assert_eq!(index_of_slide(&d, "s2"), Some(1));
        assert_eq!(index_of_slide(&d, "missing"), None);
        assert_eq!(slide_block_id(&d, 2).as_deref(), Some("s3"));
        assert_eq!(slide_block_id(&d, 9), None);
        // The mapping must survive a reorder — this is exactly why live
        // follow broadcasts ids, not indices.
        let mut reordered = d.clone();
        reordered.slides.swap(0, 2);
        assert_eq!(index_of_slide(&reordered, "s3"), Some(0));
    }
}
