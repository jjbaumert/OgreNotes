use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::editor::state::EditorState;

/// Info about an inline comment thread for rendering highlights.
#[derive(Debug, Clone)]
pub struct InlineThreadInfo {
    pub thread_id: String,
    pub block_id: String,
}

/// Overlay that renders yellow highlights for blocks that have inline comments.
#[component]
pub fn CommentHighlights(
    /// Inline thread data (block IDs with comments).
    threads: ReadSignal<Vec<InlineThreadInfo>>,
    /// Editor state (triggers re-render when doc changes).
    editor_state: ReadSignal<Option<EditorState>>,
    /// Callback when a highlight is clicked (fires with thread_id).
    on_click: Callback<String>,
) -> impl IntoView {
    view! {
        {move || {
            // Read editor_state to trigger re-render when document changes.
            let _state = editor_state.get();

            threads.get().into_iter().filter_map(|info| {
                // Find the DOM element with data-block-id matching this thread's block.
                let rect = block_rect(&info.block_id)?;
                let tid = info.thread_id.clone();

                Some(view! {
                    <div
                        class="comment-highlight"
                        style:left=format!("{}px", rect.0)
                        style:top=format!("{}px", rect.1)
                        style:width=format!("{}px", rect.2)
                        style:height=format!("{}px", rect.3)
                        on:click=move |_| on_click.run(tid.clone())
                    ></div>
                })
            }).collect::<Vec<_>>()
        }}
    }
}

/// Get the bounding rectangle of a block element by its block ID.
/// Returns (left, top, width, height) in viewport pixels.
fn block_rect(block_id: &str) -> Option<(f64, f64, f64, f64)> {
    // Validate block_id before using in CSS selector to prevent injection.
    if !block_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    let window = web_sys::window()?;
    let document = window.document()?;
    let selector = format!("[data-block-id=\"{block_id}\"]");
    let el = document.query_selector(&selector).ok()??;
    let rect = el.get_bounding_client_rect();
    if rect.height() < 1.0 {
        return None;
    }
    Some((rect.left(), rect.top(), rect.width(), rect.height()))
}
