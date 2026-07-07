// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Open-or-create flow for a spreadsheet cell's comment thread.
//!
//! A cell comment is a regular `inline` comment thread anchored to a
//! deterministic per-cell block id (`cell-s<sheet>r<row>c<col>`). Two
//! entry points share this flow:
//!
//! - the right-click context menu's "Comment" item, and
//! - the in-grid comment marker the user clicks to open a thread.
//!
//! Both converge here so the create / legacy-migrate / 409-adopt
//! handling lives in exactly one place. The block id is a pure function
//! of `(sheet, row, col)`, so two tabs that act on the same cell propose
//! the same id; the server's per-block uniqueness check lets exactly one
//! POST win and the loser adopts the winner's thread instead of
//! orphaning a competing one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use leptos::prelude::*;

use crate::spreadsheet::eval::SpreadsheetEngine;

use super::CellCommentOpen;

/// Deterministic comment block id for a cell on a given sheet.
pub(super) fn cell_block_id(sheet_idx: usize, row: usize, col: usize) -> String {
    format!("cell-s{sheet_idx}r{row}c{col}")
}

/// Parse a cell comment block id back into `(sheet, row, col)`.
///
/// Inverse of [`cell_block_id`]; returns `None` for any id that isn't the
/// `cell-s<sheet>r<row>c<col>` shape (e.g. a document block id). Used to
/// deep-link a comment notification to the exact cell (issue #50).
pub(crate) fn parse_cell_block_id(id: &str) -> Option<(usize, usize, usize)> {
    let rest = id.strip_prefix("cell-s")?;
    let (sheet, rest) = rest.split_once('r')?;
    let (row, col) = rest.split_once('c')?;
    Some((sheet.parse().ok()?, row.parse().ok()?, col.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::cell_block_id;

    #[test]
    fn block_id_is_deterministic_and_well_formed() {
        // Exact shape — the server stores this string and `document.rs`
        // filters cell threads by the `cell-` prefix, so the format is a
        // cross-component contract, not an implementation detail.
        assert_eq!(cell_block_id(0, 5, 3), "cell-s0r5c3");
        // Same coordinates always produce the same id (the basis for the
        // create-or-adopt dedup across concurrent tabs).
        assert_eq!(cell_block_id(2, 10, 7), cell_block_id(2, 10, 7));
        // Distinct cells never collide.
        assert_ne!(cell_block_id(0, 5, 3), cell_block_id(1, 5, 3));
        assert_ne!(cell_block_id(0, 5, 3), cell_block_id(0, 3, 5));
    }

    #[test]
    fn block_id_round_trips_through_parse() {
        for (s, r, c) in [(0, 5, 3), (2, 0, 0), (9, 1_048_576, 16_383)] {
            let id = cell_block_id(s, r, c);
            assert_eq!(super::parse_cell_block_id(&id), Some((s, r, c)));
        }
    }

    #[test]
    fn parse_rejects_non_cell_ids() {
        assert_eq!(super::parse_cell_block_id("block-abc123"), None);
        assert_eq!(super::parse_cell_block_id("cell-s0r5"), None); // no column
        assert_eq!(super::parse_cell_block_id("cell-sXr5c3"), None); // non-numeric
    }

    #[test]
    fn block_id_fits_the_server_validator() {
        // The comment block_id validator accepts 4-32 chars of
        // alphanumerics + dash. Even an extreme cell stays in range.
        let id = cell_block_id(99, 1_048_576, 16_384);
        assert!(id.len() >= 4 && id.len() <= 32, "len = {}", id.len());
        assert!(id.starts_with("cell-"));
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }
}

/// Open the cell's existing comment thread, or create one (migrating any
/// legacy single-string note into the first message) and then open it.
///
/// `left` / `top` are viewport pixels for the popup anchor. `persist`
/// flushes the engine's cell-style change (the new `comment_thread_id`)
/// back into the CRDT document; `on_open` asks the hosting page to
/// surface its `CommentPopup` in thread mode. `alive` guards the
/// post-await engine write against a mid-flight unmount.
pub(super) fn open_or_create_cell_comment(
    engine: &'static Mutex<SpreadsheetEngine>,
    doc_id: String,
    sheet_idx: usize,
    col: usize,
    row: usize,
    left: f64,
    top: f64,
    persist: impl Fn() + 'static,
    on_open: Callback<CellCommentOpen>,
    alive: Arc<AtomicBool>,
) {
    let (existing_tid, legacy_text) = {
        let eng = engine.lock().unwrap();
        let style = eng.get_style((col, row));
        let tid = style.and_then(|s| s.comment_thread_id.clone());
        let legacy = style
            .and_then(|s| s.comment.clone())
            .filter(|t| !t.is_empty());
        (tid, legacy)
    };

    // State 1: thread already exists — open it directly.
    if let Some(tid) = existing_tid {
        on_open.run(CellCommentOpen {
            thread_id: tid.clone(),
            block_id: tid,
            left,
            top,
        });
        return;
    }

    // States 2 + 3: pre-create the thread (seeding it with the legacy
    // note as the first message when one is present), then open it.
    let new_block_id = cell_block_id(sheet_idx, row, col);
    let block_id_for_open = new_block_id.clone();
    let initial_message = legacy_text;
    leptos::task::spawn_local(async move {
        // Empty string skips the route's seed-message path so a fresh
        // thread starts blank.
        let resp = crate::api::comments::create_thread(
            &doc_id,
            initial_message.as_deref().unwrap_or(""),
            Some(&new_block_id),
            None,
            None,
        )
        .await;
        let server_tid = match resp {
            Ok(r) => r.thread_id,
            Err(crate::api::client::ApiClientError::Http(409, _)) => {
                // A peer already created this cell's thread. Discover and
                // adopt it rather than orphaning a competing thread.
                let Ok(list) = crate::api::comments::list_threads(&doc_id).await else {
                    web_sys::console::warn_1(&"cell comment: 409 adopt failed (list)".into());
                    return;
                };
                let Some(found) = list.threads.iter().find(|t| {
                    t.block_id.as_deref() == Some(new_block_id.as_str()) && t.status == "open"
                }) else {
                    web_sys::console::warn_1(&"cell comment: 409 but no matching thread".into());
                    return;
                };
                found.thread_id.clone()
            }
            Err(_) => {
                web_sys::console::warn_1(&"cell comment: create_thread failed".into());
                return;
            }
        };
        if !alive.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut eng = engine.lock().unwrap();
            let style = eng.style_mut((col, row));
            style.comment_thread_id = Some(server_tid.clone());
            // The legacy note is now the thread's first message; drop the
            // cell-side string so it isn't persisted as a stale duplicate.
            style.comment = None;
        }
        persist();
        on_open.run(CellCommentOpen {
            thread_id: server_tid,
            block_id: block_id_for_open,
            left,
            top,
        });
    });
}
