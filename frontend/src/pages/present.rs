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

use crate::components::deck_view::{ensure_presentation_css, render_deck_canvas, render_frame_content};
use crate::editor::yrs_bridge::ydoc_bytes_to_doc;
use crate::presentation::model::{deck_from_doc, Deck, FrameRole, DEFAULT_THEME};
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

    // Presenter timer: ticks once a second while the presenter panel is
    // shown. Guarded with a cancellation flag (same pattern as
    // notification_bell.rs's poll loop) so the interval doesn't keep
    // firing — and touching a disposed `set_elapsed` — after the
    // presenter navigates away from this page.
    let (elapsed, set_elapsed) = signal(0u64);
    if is_presenter_view() {
        let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let active_for_cleanup = active.clone();
        on_cleanup(move || active_for_cleanup.store(false, std::sync::atomic::Ordering::Relaxed));
        leptos::task::spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                if !active.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                set_elapsed.update(|v| *v += 1);
            }
        });
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
                <Show when=is_presenter_view>
                    <aside class="deck-present__panel" on:click=move |ev: web_sys::MouseEvent| ev.stop_propagation()>
                        <div class="deck-present__timer">{move || format_elapsed(elapsed.get())}</div>
                        <div class="deck-present__next">
                            <h3>{crate::t!("deck-present-next")}</h3>
                            {move || {
                                let n = idx.get() + 1;
                                deck.with(|d| d.slides.get(n).map(|s| render_deck_canvas(s, &d.theme)))
                            }}
                        </div>
                        <div class="deck-present__notes">
                            <h3>{crate::t!("deck-present-notes")}</h3>
                            {move || {
                                let i = idx.get();
                                deck.with(|d| {
                                    d.slides.get(i).map(|s| {
                                        s.frames
                                            .iter()
                                            .filter(|f| f.role == FrameRole::Notes)
                                            .map(|f| view! {
                                                <div class="deck-present__note">
                                                    {render_frame_content(&f.content)}
                                                </div>
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                })
                            }}
                        </div>
                    </aside>
                </Show>
            </Show>
        </div>
    }
}

/// Elapsed wall-clock as `M:SS` (or `H:MM:SS` past an hour) for the
/// presenter timer.
pub(crate) fn format_elapsed(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_formats_as_a_clock() {
        assert_eq!(format_elapsed(0), "0:00");
        assert_eq!(format_elapsed(9), "0:09");
        assert_eq!(format_elapsed(75), "1:15");
        assert_eq!(format_elapsed(3599), "59:59");
        assert_eq!(format_elapsed(3600), "1:00:00");
        assert_eq!(format_elapsed(3725), "1:02:05");
    }
}
