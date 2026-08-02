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

use crate::collab::ws_client::{CollabClient, RemoteCursor};
use crate::components::deck_view::{ensure_presentation_css, render_deck_canvas, render_frame_content};
use crate::editor::yrs_bridge::ydoc_bytes_to_doc;
use crate::presentation::model::{deck_from_doc, Deck, FrameRole, DEFAULT_THEME};
use crate::presentation::nav::{index_of_slide, next_index, prev_index, slide_block_id};

/// Who this viewer can follow: everyone else currently broadcasting a
/// `presenting` slide. Self is excluded (you can't follow yourself),
/// and cursors with no `presenting` value are ordinary editors.
pub(crate) fn presenters<'a>(cursors: &'a [RemoteCursor], me: &str) -> Vec<&'a RemoteCursor> {
    cursors.iter().filter(|c| c.presenting.is_some() && c.user_id != me).collect()
}

/// The slide index a follower should be on, given the presenter's
/// broadcast id. `None` when not following, when the presenter is gone,
/// or when the id names a slide this deck no longer has (a concurrent
/// delete) — in every case the follower simply stays put.
pub(crate) fn followed_index(
    deck: &Deck,
    cursors: &[RemoteCursor],
    following: Option<&str>,
) -> Option<usize> {
    let target = following?;
    let cursor = cursors.iter().find(|c| c.user_id == target)?;
    let block_id = cursor.presenting.as_deref()?;
    index_of_slide(deck, block_id)
}

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

    // Live follow-the-presenter state (Task 8). `remote_cursors` mirrors
    // document.rs's awareness callback; `following` names the user_id
    // this viewer is tracking (`None` = not following anyone); `paused`
    // is set the moment this viewer navigates manually while following,
    // so their own click/keypress doesn't get immediately overwritten by
    // the next presenter broadcast.
    let remote_cursors: RwSignal<Vec<RemoteCursor>> = RwSignal::new(Vec::new());
    let (following, set_following) = signal(None::<String>);
    let (paused, set_paused) = signal(false);
    // `StoredValue` (Copy, unlike a plain `String`) because this id flows
    // through several nested `move` reactive closures below (the follow
    // affordance's two `<Show>`s, the presenter `<For>`, the per-button
    // click handler) — each is `Fn` and gets (re)constructed on every
    // reactive re-run, so a plain owned `String` would be moved out on
    // the first run. Same rationale as `comment_popup.rs`'s `current_uid`.
    let my_user_id: StoredValue<String> = StoredValue::new(
        crate::api::client::get_auth().map(|a| a.user_id).unwrap_or_default(),
    );

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

    // Any manual navigation while following someone else pauses the
    // follow — otherwise the next presenter broadcast (or even this
    // viewer's own idx write racing the follow Effect) would stomp the
    // navigation the viewer just asked for.
    let mark_manual_nav = move || {
        if following.get_untracked().is_some() {
            set_paused.set(true);
        }
    };

    let go_next = move || {
        mark_manual_nav();
        set_idx.set(next_index(idx.get_untracked(), deck.with_untracked(|d| d.slides.len())));
    };
    let go_prev = move || {
        mark_manual_nav();
        set_idx.set(prev_index(idx.get_untracked()));
    };

    // Swipe: left → next, right → previous. 48px threshold, and the
    // gesture must be predominantly horizontal so a vertical scroll in
    // the presenter panel doesn't change slides. `touches()` is empty by
    // the time `touchend` fires (the lifted finger is no longer "on" the
    // surface), so both endpoints are read via `changed_touches()`
    // instead of `crate::touch::first_touch_xy` (which reads `touches()`
    // and only suits touchstart/touchmove). The dominant-axis + threshold
    // decision itself reuses `crate::touch::swipe_direction`, the same
    // primitive spreadsheet touch handling is built on.
    let (touch_start, set_touch_start) = signal::<Option<(f64, f64)>>(None);
    let on_touch_start = move |ev: web_sys::TouchEvent| {
        if let Some(t) = ev.changed_touches().get(0) {
            set_touch_start.set(Some((t.client_x() as f64, t.client_y() as f64)));
        }
    };
    let on_touch_end = move |ev: web_sys::TouchEvent| {
        let Some((start_x, start_y)) = touch_start.get_untracked() else { return };
        set_touch_start.set(None);
        let Some(t) = ev.changed_touches().get(0) else { return };
        let dir = crate::touch::swipe_direction(
            start_x, start_y, t.client_x() as f64, t.client_y() as f64, 48.0,
        );
        match dir {
            Some(crate::touch::SwipeDir::Left) => go_next(),
            Some(crate::touch::SwipeDir::Right) => go_prev(),
            _ => {}
        }
    };

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
                "Home" => { ev.prevent_default(); mark_manual_nav(); set_idx.set(0); }
                "End" => {
                    ev.prevent_default();
                    mark_manual_nav();
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

    // ── Live follow-the-presenter (Task 8) ──────────────────────────
    //
    // This `CollabClient` exists purely for awareness — present mode
    // never edits the doc (content is fetched once via REST above) —
    // so unlike document.rs's client there's no on_remote_update /
    // local_doc_provider wiring, just broadcast-this-viewer's-slide
    // and receive-everyone-else's. Constructed the same way
    // document.rs:919 builds its editing client: fetch a ws-token,
    // then connect.
    let collab_client: std::rc::Rc<std::cell::RefCell<Option<CollabClient>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    // #152 (mirrored from document.rs): disconnect on unmount. The
    // awareness callback and the sync-poll loop below each hold a
    // clone of this `Rc`, so nothing else drops it on a route change —
    // without this the socket + heartbeat would leak past navigating
    // out of present mode, and this viewer's ghost cursor would keep
    // "presenting" for anyone still following.
    let collab_for_unmount = send_wrapper::SendWrapper::new(std::rc::Rc::clone(&collab_client));
    on_cleanup(move || {
        if let Some(client) = collab_for_unmount.borrow().as_ref() {
            client.disconnect();
        }
    });

    // Set true exactly when the socket reaches `Synced`, false again on
    // disconnect — mirrors the `connected_flag` document.rs passes into
    // `connect()`. This is the raw (non-reactive) flag `connect()`
    // requires; `ws_synced` below mirrors it into a signal the broadcast
    // Effect can depend on.
    let ws_synced_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (ws_synced, set_ws_synced) = signal(false);

    {
        let id = doc_id();
        let collab_for_connect = std::rc::Rc::clone(&collab_client);
        let synced_for_connect = std::sync::Arc::clone(&ws_synced_flag);
        leptos::task::spawn_local(async move {
            let client = CollabClient::new(id.clone(), None);
            client.set_on_awareness_update(Box::new(move |cursors| {
                remote_cursors.set(cursors);
            }));
            *collab_for_connect.borrow_mut() = Some(client);

            match crate::api::documents::request_ws_token(&id).await {
                Ok(resp) => {
                    let origin = web_sys::window()
                        .and_then(|w| w.location().origin().ok())
                        .unwrap_or_default();
                    let ws_origin = if origin.starts_with("https") {
                        origin.replacen("https", "wss", 1)
                    } else {
                        let api_origin = origin.replacen("http", "ws", 1);
                        if api_origin.contains(":8080") {
                            api_origin.replace(":8080", ":3000")
                        } else {
                            api_origin
                        }
                    };
                    let ws_url = format!("{ws_origin}/api/v1/documents/{id}/ws");
                    if let Some(ref client) = *collab_for_connect.borrow() {
                        client.connect(&ws_url, &resp.token, synced_for_connect);
                    }
                }
                Err(e) => {
                    crate::editor::debug::warn("collab", &format!("ws-token request failed: {e}"));
                }
            }
        });
    }

    let color_idx = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        my_user_id.with_value(|id| id.hash(&mut h));
        (h.finish() % 12) as u8
    };
    let my_name = crate::api::client::get_auth()
        .map(|a| a.name)
        .unwrap_or_else(|| "Anonymous".to_string());

    // Poll the raw `ws_synced_flag` into the reactive `ws_synced` signal —
    // same trade-off `sync_indicator::poll_sync_state` makes for the
    // save-status badge: the flag flips deep inside a raw WebSocket
    // `onmessage` handler, off the reactive graph, so there's no signal
    // write to hook directly.
    {
        let flag = std::sync::Arc::clone(&ws_synced_flag);
        let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let active_for_cleanup = active.clone();
        on_cleanup(move || active_for_cleanup.store(false, std::sync::atomic::Ordering::Relaxed));
        leptos::task::spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(300).await;
                if !active.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                set_ws_synced.set(flag.load(std::sync::atomic::Ordering::Relaxed));
            }
        });
    }

    // Broadcasts this viewer's current slide as awareness. The Effect
    // depends on `idx`, the deck-loaded/non-empty state, and the polled
    // `ws_synced` signal — three independent async pieces (REST deck
    // fetch, WS handshake, user navigation) that can each finish last —
    // so it necessarily *re-runs* on every one of those changes,
    // including every 300ms `ws_synced` poll tick once synced (the poll
    // writes unconditionally; `reactive_graph`'s `Set::set` has no
    // equality gate, so it notifies every tick regardless of whether the
    // value actually changed). Left alone that would resend an identical
    // frame to the room roughly every 300ms forever. `last_sent_block_id`
    // is the guard: only an Effect run that would send a *different*
    // block_id than last time actually calls `send_awareness` (mirrors
    // `document.rs:1341-1364`'s `prev_sel_hash` change-detection for its
    // own awareness Effect), so in steady state — synced, loaded, no
    // navigation — this sends nothing at all after the first frame.
    {
        let collab = std::rc::Rc::clone(&collab_client);
        let user_id = my_user_id.get_value();
        let name = my_name.clone();
        let last_sent_block_id: StoredValue<Option<String>> = StoredValue::new(None);
        Effect::new(move |_| {
            let i = idx.get();
            let synced = ws_synced.get();
            let ready = loaded.get() && deck.with(|d| !d.slides.is_empty());
            if !synced || !ready {
                return;
            }
            let Some(block_id) = deck.with_untracked(|d| slide_block_id(d, i)) else { return };
            if last_sent_block_id.get_value().as_deref() == Some(block_id.as_str()) {
                return;
            }
            if let Some(ref client) = *collab.borrow() {
                client.send_awareness(
                    &user_id, &name, color_idx,
                    None, None, None, None,
                    Some(block_id.as_str()),
                );
                last_sent_block_id.set_value(Some(block_id));
            }
        });
    }

    // Follow: while following someone and not paused, track their
    // broadcast slide. Reading `idx` untracked (rather than as an Effect
    // dependency) means this never fights a manual navigation — that
    // path pauses first, which this Effect observes before it would
    // otherwise overwrite the just-chosen `idx`.
    Effect::new(move |_| {
        let is_paused = paused.get();
        let follow_target = following.get();
        let cursors = remote_cursors.get();
        if is_paused {
            return;
        }
        let Some(target) = follow_target else { return };
        if let Some(new_idx) = deck.with(|d| followed_index(d, &cursors, Some(target.as_str()))) {
            if idx.get_untracked() != new_idx {
                set_idx.set(new_idx);
            }
        }
    });

    view! {
        <div
            class="deck-present"
            class:deck-present--presenter=is_presenter_view
            on:click=move |_| go_next()
            on:touchstart=on_touch_start
            on:touchend=on_touch_end
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
                <Show when=move || my_user_id.with_value(|id| !presenters(&remote_cursors.get(), id).is_empty())>
                    <div class="deck-present__follow">
                        <Show
                            when=move || following.get().is_some() && paused.get()
                            fallback=move || view! {
                                <For each=move || my_user_id.with_value(|id| {
                                            presenters(&remote_cursors.get(), id)
                                                .into_iter().map(|c| (c.user_id.clone(), c.name.clone())).collect::<Vec<_>>()
                                        })
                                         key=|(id, _)| id.clone()
                                         children=move |(id, name)| {
                                            let id2 = id.clone();
                                            view! {
                                                <button class="deck-present__follow-btn"
                                                    on:click=move |ev: web_sys::MouseEvent| {
                                                        ev.stop_propagation();
                                                        set_following.set(Some(id2.clone()));
                                                        set_paused.set(false);
                                                    }>
                                                    {crate::t!("deck-present-follow", name = name)}
                                                </button>
                                            }
                                         } />
                            }
                        >
                            <button class="deck-present__rejoin"
                                on:click=move |ev: web_sys::MouseEvent| { ev.stop_propagation(); set_paused.set(false); }>
                                {crate::t!("deck-present-rejoin")}
                            </button>
                        </Show>
                    </div>
                </Show>
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
mod follow_tests {
    use super::*;
    use crate::collab::ws_client::RemoteCursor;
    use crate::presentation::model::{DeckSlide, DEFAULT_THEME};

    fn cursor(user: &str, presenting: Option<&str>) -> RemoteCursor {
        RemoteCursor {
            user_id: user.to_string(),
            name: format!("{user}-name"),
            color: "#fff".to_string(),
            cursor_block: None,
            selection_anchor_block: None,
            selection_head_block: None,
            typing_thread_id: None,
            presenting: presenting.map(|s| s.to_string()),
        }
    }

    fn deck(ids: &[&str]) -> Deck {
        Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: ids.iter().map(|id| DeckSlide {
                block_id: (*id).to_string(),
                layout: "blank".to_string(),
                background: None,
                frames: Vec::new(),
            }).collect(),
        }
    }

    #[test]
    fn presenters_excludes_self_and_non_presenters() {
        let cs = vec![cursor("me", Some("s1")), cursor("them", Some("s2")), cursor("editor", None)];
        let p = presenters(&cs, "me");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].user_id, "them");
    }

    #[test]
    fn followed_index_resolves_the_presenters_slide() {
        let d = deck(&["s1", "s2", "s3"]);
        let cs = vec![cursor("them", Some("s3"))];
        assert_eq!(followed_index(&d, &cs, Some("them")), Some(2));
        assert_eq!(followed_index(&d, &cs, None), None, "not following");
        assert_eq!(followed_index(&d, &cs, Some("ghost")), None, "presenter left");
        let cs_gone = vec![cursor("them", Some("deleted-slide"))];
        assert_eq!(followed_index(&d, &cs_gone, Some("them")), None, "unknown slide id");
    }
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
