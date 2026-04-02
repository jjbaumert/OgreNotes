use leptos::prelude::*;

use crate::collab::ws_client::RemoteCursor;
use super::dom_position::{dom_position_for_model_pos, range_rect};

/// Overlay that renders remote users' cursors on top of the editor.
#[component]
pub fn CursorOverlay(
    cursors: ReadSignal<Vec<RemoteCursor>>,
) -> impl IntoView {
    view! {
        {move || {
            cursors.get().into_iter().filter_map(|cursor| {
                let pos = cursor.cursor_pos?;
                let (left, top, height) = dom_position_for_model_pos(pos)?;

                let color = cursor.color.clone();
                let color_caret = color.clone();
                let color_label = color.clone();
                let name = cursor.name.clone();

                let selection_style = if let (Some(anchor), Some(head)) = (cursor.selection_anchor, cursor.selection_head) {
                    let from = anchor.min(head);
                    let to = anchor.max(head);
                    range_rect(from, to).map(|(sl, st, sw, sh)| {
                        let bg = format!("{}33", color);
                        (sl, st, sw, sh, bg)
                    })
                } else {
                    None
                };

                Some(view! {
                    {selection_style.map(|(sl, st, sw, sh, bg)| view! {
                        <div
                            class="remote-cursor-selection"
                            style:left=format!("{}px", sl)
                            style:top=format!("{}px", st)
                            style:width=format!("{}px", sw)
                            style:height=format!("{}px", sh)
                            style:background-color=bg
                        ></div>
                    })}
                    <div
                        class="remote-cursor-caret"
                        style:left=format!("{}px", left)
                        style:top=format!("{}px", top)
                        style:height=format!("{}px", height)
                        style:border-left-color=color_caret
                    >
                        <span
                            class="remote-cursor-label"
                            style:background-color=color_label
                        >{name}</span>
                    </div>
                })
            }).collect::<Vec<_>>()
        }}
    }
}
