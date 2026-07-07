// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

use crate::editor::state::{find_block_at, EditorState};
use super::dom_position;

const POPUP_WIDTH: f64 = 420.0;
const BUBBLE_OFFSET: f64 = 32.0;

/// Info about an inline comment thread for rendering highlights.
#[derive(Debug, Clone)]
pub struct InlineThreadInfo {
    pub thread_id: String,
    pub block_id: String,
    /// Text selection start offset within the block (None for block-level comments).
    pub anchor_start: Option<u32>,
    /// Text selection end offset within the block.
    pub anchor_end: Option<u32>,
}

/// Remap inline-thread anchors (stored **block-relative**) through a
/// document swap described by `maps`, mutating `threads` in place and
/// returning the `(thread_id, new_start, new_end)` of anchors that moved.
///
/// Pure — no persistence. The local-edit path persists the returned
/// changes (the editor that made the edit owns the anchor write); the
/// remote-edit path calls this purely *optimistically*, so a peer's edit
/// doesn't visibly drag this client's comment highlights during the
/// window before the editing client's persisted anchors are refetched.
///
/// Because anchors are block-relative, a remap is a no-op unless the
/// anchor's own block changed length before the offset. The per-anchor
/// coordinate math lives in [`crate::editor::state::remap_block_anchor`]
/// (native, unit-tested); this just iterates threads over it.
pub fn remap_thread_anchors(
    threads: &mut [InlineThreadInfo],
    maps: &[crate::editor::transform::StepMap],
    old_doc: &crate::editor::model::Node,
    new_doc: &crate::editor::model::Node,
) -> Vec<(String, u32, u32)> {
    let mut changed = Vec::new();
    for thread in threads.iter_mut() {
        let (Some(start), Some(end)) = (thread.anchor_start, thread.anchor_end) else {
            continue;
        };
        if let Some((new_start, new_end)) = crate::editor::state::remap_block_anchor(
            &thread.block_id, start, end, maps, old_doc, new_doc,
        ) {
            thread.anchor_start = Some(new_start);
            thread.anchor_end = Some(new_end);
            changed.push((thread.thread_id.clone(), new_start, new_end));
        }
    }
    changed
}

/// Overlay that renders highlights for commented text and comment bubbles in the margin.
#[component]
pub fn CommentHighlights(
    /// Inline thread data (block IDs with comments).
    threads: ReadSignal<Vec<InlineThreadInfo>>,
    /// Editor state (triggers re-render when doc changes).
    editor_state: ReadSignal<Option<EditorState>>,
    /// Scroll tick — forces re-render when the editor container scrolls.
    scroll_tick: ReadSignal<u32>,
    /// Callback when a highlight is clicked (fires with (thread_id, left, top) for popup positioning).
    on_click: Callback<(String, f64, f64)>,
) -> impl IntoView {
    view! {
        {move || {
            let _state = editor_state.get();
            let _tick = scroll_tick.get();
            let items = threads.get();

            // Group threads by block_id to count comments per block
            let mut block_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
            for info in &items {
                *block_counts.entry(info.block_id.clone()).or_insert(0) += 1;
            }

            let mut views = Vec::new();

            // Render highlights + margin bubbles
            let mut rendered_bubbles: std::collections::HashSet<String> = std::collections::HashSet::new();
            for info in items {
                let Some(block_el) = dom_position::find_block_element(&info.block_id) else { continue };
                let block_rect = dom_position::element_viewport_rect(&block_el);
                if block_rect.height < 1.0 { continue; }

                let tid = info.thread_id.clone();
                let (popup_left, popup_top) =
                    dom_position::place_left_margin(&block_rect, POPUP_WIDTH, 12.0);

                // Highlight the specific text range if anchors are set,
                // otherwise highlight the whole block.
                let highlight_rects = if let (Some(start), Some(end)) = (info.anchor_start, info.anchor_end) {
                    dom_position::text_range_rects(&block_el, start, end)
                } else {
                    vec![(block_rect.left, block_rect.top, block_rect.width, block_rect.height)]
                };

                for (hl, ht, hw, hh) in highlight_rects {
                    let tid_hl = tid.clone();
                    views.push(view! {
                        <div
                            class="comment-highlight"
                            style:left=format!("{}px", hl)
                            style:top=format!("{}px", ht)
                            style:width=format!("{}px", hw)
                            style:height=format!("{}px", hh)
                            on:click=move |_| on_click.run((tid_hl.clone(), popup_left, popup_top))
                        ></div>
                    }.into_any());
                }

                // Render margin bubble (once per block)
                if !rendered_bubbles.contains(&info.block_id) {
                    let count = block_counts.get(&info.block_id).copied().unwrap_or(1);
                    let bid = info.block_id.clone();
                    let tid_for_bubble = info.thread_id.clone();
                    let bubble_top = block_rect.top + (block_rect.height / 2.0) - 12.0;
                    let bubble_left = (block_rect.left - BUBBLE_OFFSET).max(4.0);

                    rendered_bubbles.insert(bid);
                    views.push(view! {
                        <div
                            class="comment-bubble"
                            style:left=format!("{}px", bubble_left)
                            style:top=format!("{}px", bubble_top)
                            on:click=move |_| on_click.run((tid_for_bubble.clone(), popup_left, popup_top))
                        >
                            <span class="comment-bubble-icon">"\u{1F4AC}"</span>
                            {if count > 1 {
                                Some(view! { <span class="comment-bubble-count">{count}</span> })
                            } else {
                                None
                            }}
                        </div>
                    }.into_any());
                }
            }

            views
        }}
    }
}

/// "+ comment" affordance rendered in the LEFT margin of the block that
/// currently contains the caret. Replaces a CSS-only `::after` hover hack
/// that put the icon on the right and couldn't receive click events
/// (pseudo-elements aren't in the DOM, and the off-parent placement broke
/// the parent's `:hover` selector as soon as the cursor entered the icon).
///
/// Skips rendering when the caret-containing block already has at least
/// one inline thread — `CommentHighlights` already shows a bubble for
/// those, and stacking two would be visual noise.
#[component]
pub fn AddCommentBubble(
    /// Editor state — used to find the block under the caret.
    editor_state: ReadSignal<Option<EditorState>>,
    /// Existing inline threads — used to suppress this bubble when the
    /// block already has a comment bubble from `CommentHighlights`.
    threads: ReadSignal<Vec<InlineThreadInfo>>,
    /// Re-render trigger when the editor scrolls.
    scroll_tick: ReadSignal<u32>,
    /// Fired with `(block_id, popup_left, popup_top)` when clicked. The
    /// caller opens `CommentPopup` with `is_new=true`,
    /// `anchor_start=None`, `anchor_end=None` (block-level comment).
    on_click: Callback<(String, f64, f64)>,
) -> impl IntoView {
    view! {
        {move || {
            let _tick = scroll_tick.get();
            let state = editor_state.get()?;

            // Where is the caret? Use selection.from() — for a collapsed
            // selection that's the caret position; for a range it's the
            // earlier of the two endpoints, which is fine for picking the
            // block to comment on.
            let pos = state.selection.from();
            let block = find_block_at(&state.doc, pos)?;
            let block_id = block.attrs.get("blockId")?.clone();

            // If an existing-comment bubble is already on this block, skip.
            if threads.get().iter().any(|t| t.block_id == block_id) {
                return None;
            }

            let block_el = dom_position::find_block_element(&block_id)?;
            let block_rect = dom_position::element_viewport_rect(&block_el);
            if block_rect.height < 1.0 {
                return None;
            }

            let bubble_left = (block_rect.left - BUBBLE_OFFSET).max(4.0);
            let bubble_top = block_rect.top + (block_rect.height / 2.0) - 12.0;
            let (popup_left, popup_top) =
                dom_position::place_left_margin(&block_rect, POPUP_WIDTH, 12.0);

            let bid = block_id.clone();
            Some(view! {
                <div
                    class="comment-bubble comment-bubble-add"
                    title=crate::t!("comment-highlights-add")
                    style:left=format!("{}px", bubble_left)
                    style:top=format!("{}px", bubble_top)
                    on:click=move |_| on_click.run((bid.clone(), popup_left, popup_top))
                >
                    // Speech-bubble glyph as the base — matches the
                    // existing-comment bubble's silhouette so the
                    // affordance reads as "comment thing".
                    <span class="comment-bubble-icon">"\u{1F4AC}"</span>
                    // "+" badge overlaid on the bubble. pointer-events
                    // are off so the click bubbles to the parent's handler.
                    <span class="comment-bubble-add-plus">"+"</span>
                </div>
            })
        }}
    }
}