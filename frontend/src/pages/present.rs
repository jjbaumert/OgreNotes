// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Full-screen present mode for slide decks (P2).
//!
//! A sidebar-free route (`/d/:id/present`) that renders the *same*
//! `render_deck_canvas` the editor and thumbnails use — the canvas is
//! already a fixed-aspect, container-queried surface, so presenting is
//! a layout change, not a second renderer. Read-only by construction:
//! the deck is fetched once and never written back.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use wasm_bindgen::JsCast;

use crate::components::deck_view::{ensure_presentation_css, render_deck_canvas};
use crate::editor::yrs_bridge::ydoc_bytes_to_doc;
use crate::presentation::model::{deck_from_doc, Deck, DEFAULT_THEME};
use crate::presentation::nav::{next_index, prev_index};

#[component]
pub fn PresentPage() -> impl IntoView {
    ensure_presentation_css();
    let params = use_params_map();
    let doc_id = move || params.read().get("id").unwrap_or_default();
    let query = use_query_map();
    let is_presenter_view = move || query.read().get("presenter").is_some();

    let deck = RwSignal::new(Deck {
        theme: DEFAULT_THEME.to_string(),
        slide_size: "16:9".to_string(),
        slides: Vec::new(),
    });
    let (idx, set_idx) = signal(0usize);
    let (loaded, set_loaded) = signal(false);

    // Fetch once. Present mode is a read-only view of the deck as it
    // stands when the presenter opens it; live content sync arrives
    // with follow-the-presenter (next task).
    {
        let id = doc_id();
        leptos::task::spawn_local(async move {
            if let Ok(bytes) = crate::api::documents::get_content(&id).await {
                if let Ok(node) = ydoc_bytes_to_doc(&bytes) {
                    deck.set(deck_from_doc(&node));
                }
            }
            set_loaded.set(true);
        });
    }

    let go_next = move || set_idx.set(next_index(idx.get_untracked(), deck.with_untracked(|d| d.slides.len())));
    let go_prev = move || set_idx.set(prev_index(idx.get_untracked()));

    // Window-level keydown: the overlay owns the whole page, and a
    // container-scoped handler would need focus management the browser
    // fullscreen transition can steal. Same listener style as
    // deck_view.rs's outside-click handler.
    {
        let navigate = use_navigate();
        let id = doc_id();
        let handle = window_event_listener_untyped("keydown", move |ev: web_sys::Event| {
            let Ok(ke) = ev.clone().dyn_into::<web_sys::KeyboardEvent>() else { return };
            match ke.key().as_str() {
                "ArrowRight" | "ArrowDown" | " " | "PageDown" => { ev.prevent_default(); go_next(); }
                "ArrowLeft" | "ArrowUp" | "PageUp" => { ev.prevent_default(); go_prev(); }
                "Home" => { ev.prevent_default(); set_idx.set(0); }
                "End" => {
                    ev.prevent_default();
                    let len = deck.with_untracked(|d| d.slides.len());
                    set_idx.set(len.saturating_sub(1));
                }
                "Escape" => { navigate(&format!("/d/{id}"), Default::default()); }
                _ => {}
            }
        });
        on_cleanup(move || handle.remove());
    }

    view! {
        <div
            class="deck-present"
            class:deck-present--presenter=is_presenter_view
            on:click=move |_| go_next()
        >
            <Show when=move || loaded.get() && deck.with(|d| !d.slides.is_empty())
                  fallback=|| view! { <div class="deck-present__empty">{crate::t!("deck-present-empty")}</div> }>
                <div class="deck-present__stage">
                    {move || {
                        let i = idx.get().min(deck.with(|d| d.slides.len().saturating_sub(1)));
                        deck.with(|d| d.slides.get(i).map(|s| render_deck_canvas(s, &d.theme)))
                    }}
                </div>
                <div class="deck-present__counter">
                    {move || format!("{} / {}", idx.get() + 1, deck.with(|d| d.slides.len()))}
                </div>
            </Show>
        </div>
    }
}
