use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::editor::state::EditorState;

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
                let Some(block_el) = find_block_element(&info.block_id) else { continue };
                let block_rect = block_el.get_bounding_client_rect();
                if block_rect.height() < 1.0 { continue; }

                let tid = info.thread_id.clone();
                let popup_width = 420.0;
                let popup_left = (block_rect.left() - popup_width - 12.0).max(4.0);
                let popup_top = block_rect.top();

                // Highlight the text range (or whole block if no anchors)
                let highlight_rect = block_rect.clone();
                views.push(view! {
                    <div
                        class="comment-highlight"
                        style:left=format!("{}px", highlight_rect.left())
                        style:top=format!("{}px", highlight_rect.top())
                        style:width=format!("{}px", highlight_rect.width())
                        style:height=format!("{}px", highlight_rect.height())
                        on:click=move |_| on_click.run((tid.clone(), popup_left, popup_top))
                    ></div>
                }.into_any());

                // Render margin bubble (once per block)
                if !rendered_bubbles.contains(&info.block_id) {
                    let count = block_counts.get(&info.block_id).copied().unwrap_or(1);
                    let bid = info.block_id.clone();
                    let tid_for_bubble = info.thread_id.clone();
                    let bubble_top = block_rect.top() + (block_rect.height() / 2.0) - 12.0;
                    let bubble_left = block_rect.left() - 32.0;

                    rendered_bubbles.insert(bid);
                    views.push(view! {
                        <div
                            class="comment-bubble"
                            style:left=format!("{}px", bubble_left.max(4.0))
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

/// Find the DOM element for a block by its block ID.
fn find_block_element(block_id: &str) -> Option<web_sys::Element> {
    if !block_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    let window = web_sys::window()?;
    let document = window.document()?;
    let selector = format!("[data-block-id=\"{block_id}\"]");
    document.query_selector(&selector).ok()?
}
