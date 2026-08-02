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
/// `presenting` slide. "Self" here means this exact browser tab/window
/// (session), not this user broadly (#211) — a presenter's OTHER
/// window, e.g. a projector window and a separate `?presenter=1`
/// control window, has a different `session_id` and so shows up here as
/// followable. It's expected (and useful) for a presenter to see their
/// own name listed when they have two present windows open.
pub(crate) fn presenters<'a>(cursors: &'a [RemoteCursor], my_session_id: &str) -> Vec<&'a RemoteCursor> {
    cursors.iter().filter(|c| c.presenting.is_some() && c.session_id != my_session_id).collect()
}

/// The slide index a follower should be on, given the presenter's
/// broadcast id. `None` when not following, when the presenter is gone,
/// or when the id names a slide this deck no longer has (a concurrent
/// delete) — in every case the follower simply stays put.
///
/// `following` names a `session_id` (#211), not a `user_id` — so
/// following a specific one of a presenter's two open windows keeps
/// tracking *that* window even if their other window is also presenting
/// a different slide.
pub(crate) fn followed_index(
    deck: &Deck,
    cursors: &[RemoteCursor],
    following: Option<&str>,
) -> Option<usize> {
    let target = following?;
    let cursor = cursors.iter().find(|c| c.session_id == target)?;
    let block_id = cursor.presenting.as_deref()?;
    index_of_slide(deck, block_id)
}

/// True exactly on a false→true transition of the polled `ws_synced`
/// flag — the moment a resync (initial handshake, or a reconnect after a
/// mid-session drop) completes. Pulled out of the awareness-broadcast
/// Effect below as a standalone pure function purely so this one piece of
/// its logic — as opposed to the `Effect`/`StoredValue`/`CollabClient`
/// plumbing around it, which needs a live DOM/WASM environment to
/// exercise — has a unit test.
pub(crate) fn just_resynced(was_synced: bool, synced: bool) -> bool {
    synced && !was_synced
}

/// Whether a DOM event's target is a native interactive control (button,
/// link, form field). Mirrors the tag-name checks other global keydown
/// handlers use to yield to focused controls — e.g.
/// `spreadsheet_view.rs`'s Ctrl+A handler checking `INPUT`/`TEXTAREA` on
/// the active element. Present mode's Follow/Rejoin `<button>`s are the
/// only focusable controls on this page, but the check is written against
/// the general set of interactive tags rather than just `BUTTON` so it
/// keeps working if more controls are added later.
fn is_interactive_target(ev: &web_sys::Event) -> bool {
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .map(|el| {
            matches!(
                el.tag_name().to_uppercase().as_str(),
                "BUTTON" | "INPUT" | "TEXTAREA" | "SELECT" | "A"
            )
        })
        .unwrap_or(false)
}

#[component]
pub fn PresentPage() -> impl IntoView {
    // Mirrors document.rs:258-263's route guard: an expired/absent
    // session would otherwise fall through to the empty-deck fallback
    // below ("This deck has no slides yet"), which reads as data loss
    // rather than as an auth problem. Server-side gating already denies
    // the content fetch either way — this is purely about giving the
    // same route-guard UX as every other authenticated page.
    if !crate::api::client::is_authenticated() {
        let navigate = use_navigate();
        navigate("/login", Default::default());
        return view! { <div>{crate::t!("common-redirecting-login")}</div> }.into_any();
    }

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
    // document.rs's awareness callback — deliberately NOT deduped by
    // user (unlike document.rs's copy, #211) so a presenter's two open
    // windows both appear as independently followable; `following`
    // names the session_id this viewer is tracking (`None` = not
    // following anyone); `paused` is set the moment this viewer
    // navigates manually while following, so their own click/keypress
    // doesn't get immediately overwritten by the next presenter
    // broadcast.
    let remote_cursors: RwSignal<Vec<RemoteCursor>> = RwSignal::new(Vec::new());
    let (following, set_following) = signal(None::<String>);
    let (paused, set_paused) = signal(false);
    // #211: this window's own session_id, mirrored into a signal from
    // `CollabClient::session_id()` the moment the client is constructed
    // (see the connect Effect below). Empty string (never matches a
    // real session_id) until then, which is fine — `presenters()` just
    // excludes nothing extra in that brief window.
    let (my_session_id, set_my_session_id) = signal(String::new());
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

    // Swipe: left → next, right → previous. Uses the shared
    // `crate::touch::SWIPE_THRESHOLD_PX` (this is the frontend's only
    // swipe consumer, so there's no other call site to stay consistent
    // with, but a page-local magic number would just be a second
    // threshold to keep in sync by hand). The gesture must also be
    // predominantly horizontal so a vertical scroll in
    // the presenter panel doesn't change slides. `touches()` is empty by
    // the time `touchend` fires (the lifted finger is no longer "on" the
    // surface), so both endpoints are read via `changed_touches()`
    // instead of `crate::touch::first_touch_xy` (which reads `touches()`
    // and only suits touchstart/touchmove). The dominant-axis + threshold
    // decision itself reuses `crate::touch::swipe_direction`, the same
    // primitive spreadsheet touch handling is built on.
    let (touch_start, set_touch_start) = signal::<Option<(f64, f64)>>(None);
    // The outer `.deck-present` div also has `on:click=go_next` (tap
    // anywhere to advance, non-presenter view). Mobile browsers replay a
    // handled touch as a compatibility `click` ~shortly after `touchend`
    // unless that default is prevented — belt-and-suspenders here:
    // `prevent_default()` on the touchend that resolves to a swipe asks
    // the browser not to synthesize one, and this flag is the software
    // fallback for engines that fire it anyway. It self-clears on the
    // next click (whether synthetic or a genuine subsequent tap) or,
    // failing that, after 400ms — comfortably past the ~300ms window
    // mobile browsers use for the synthetic click — so a real tap that
    // follows isn't silently swallowed.
    let (suppress_next_click, set_suppress_next_click) = signal(false);
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
            start_x, start_y, t.client_x() as f64, t.client_y() as f64,
            crate::touch::SWIPE_THRESHOLD_PX,
        );
        // Only a horizontal swipe navigates; a vertical one (dy-dominant,
        // e.g. scrolling the presenter panel) is left alone entirely — no
        // navigation, no prevent_default, no click suppression.
        let navigate: Option<bool> = match dir {
            Some(crate::touch::SwipeDir::Left) => Some(true),
            Some(crate::touch::SwipeDir::Right) => Some(false),
            _ => None,
        };
        let Some(is_next) = navigate else { return };
        ev.prevent_default();
        set_suppress_next_click.set(true);
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(400).await;
            set_suppress_next_click.set(false);
        });
        if is_next { go_next() } else { go_prev() }
    };
    let on_stage_click = move |_: web_sys::MouseEvent| {
        if suppress_next_click.get_untracked() {
            set_suppress_next_click.set(false);
            return;
        }
        go_next();
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
                "ArrowRight" | "ArrowDown" | "PageDown" => { ev.prevent_default(); go_next(); }
                " " => {
                    // The Follow/Rejoin buttons live inside this same
                    // full-page keydown scope (there's no container-scoped
                    // handler to yield to, see the comment above), so an
                    // unconditional prevent_default() here would suppress
                    // native Space-activation whenever one of them has
                    // focus. Only treat Space as "advance the deck" when
                    // the event didn't originate on an interactive element.
                    if is_interactive_target(&ev) {
                        return;
                    }
                    ev.prevent_default();
                    go_next();
                }
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

    // #210: bumped by the liveness poll/listener below to re-run the
    // connect Effect after an idle-disconnect or a network blip —
    // mirrors document.rs:337's `reconnect_trigger`. Without this the
    // one-shot `spawn_local` this block used to be could never run
    // again, so a dropped socket stayed dropped for the rest of the
    // session.
    let (reconnect_trigger, set_reconnect_trigger) = signal(0u32);

    {
        let id = doc_id();
        let collab_for_connect = std::rc::Rc::clone(&collab_client);
        let synced_for_connect = std::sync::Arc::clone(&ws_synced_flag);
        Effect::new(move |_| {
            // The only dependency: this Effect's job is purely "(re)connect
            // now", it doesn't need to react to anything else on the page.
            let _trigger = reconnect_trigger.get();

            let has_client = collab_for_connect.borrow().is_some();
            if has_client {
                // Reconnect: reuse the existing CollabClient (mirrors
                // document.rs's same-doc branch) — just disconnect the
                // old WebSocket; the connect below opens a fresh one.
                // `ws_synced_flag` is the same `Arc` every time, so the
                // false it's set to on `onclose` and the true it gets
                // back on the next SyncStep2 both land on the one signal
                // the broadcast Effect's `just_resynced` edge-detector
                // watches.
                if let Some(ref client) = *collab_for_connect.borrow() {
                    client.disconnect();
                }
            } else {
                let client = CollabClient::new(id.clone(), None);
                // #211: capture this window's session_id once, at
                // construction — a fresh `CollabClient` is only built on
                // this (non-reconnect) branch, so this never re-fires on
                // a reconnect and `my_session_id` stays stable for the
                // component's lifetime, matching "once per CollabClient
                // instance / page mount".
                set_my_session_id.set(client.session_id().to_string());
                client.set_on_awareness_update(Box::new(move |cursors| {
                    remote_cursors.set(cursors);
                }));
                *collab_for_connect.borrow_mut() = Some(client);
            }

            let id = id.clone();
            let collab_for_token = std::rc::Rc::clone(&collab_for_connect);
            let synced_for_token = std::sync::Arc::clone(&synced_for_connect);
            leptos::task::spawn_local(async move {
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
                        if let Some(ref client) = *collab_for_token.borrow() {
                            client.connect(&ws_url, &resp.token, synced_for_token);
                        }
                    }
                    Err(e) => {
                        crate::editor::debug::warn("collab", &format!("ws-token request failed: {e}"));
                    }
                }
            });
        });
    }

    // #210 liveness: "visible tab = active" — while the present tab is
    // visible, keep the connection warm unconditionally (a displayed
    // slide IS the live session, whether or not anyone is pressing
    // keys) and ask for a reconnect if the socket isn't up. Deliberately
    // NOT the 300ms `ws_synced` poll above: that cadence exists to keep
    // the *awareness broadcast* Effect's reactive dependency reasonably
    // fresh, but `record_activity()` only matters on a 30-minute
    // horizon (`IDLE_DISCONNECT_MS`), so ticking it that fast would just
    // be needless wakeups for no behavioral gain — a slower, independent
    // interval is enough. The `should_keep_warm`/`should_trigger_reconnect`
    // predicates live in `presentation::liveness` so they're unit-tested
    // without a DOM.
    const LIVENESS_POLL_MS: u32 = 5_000;
    {
        let collab = std::rc::Rc::clone(&collab_client);
        let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let active_for_cleanup = active.clone();
        on_cleanup(move || active_for_cleanup.store(false, std::sync::atomic::Ordering::Relaxed));
        leptos::task::spawn_local(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(LIVENESS_POLL_MS).await;
                if !active.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let visible = web_sys::window()
                    .and_then(|w| w.document())
                    .map(|d| !d.hidden())
                    .unwrap_or(false);
                let connected =
                    collab.borrow().as_ref().map(|c| c.is_connected()).unwrap_or(false);
                if crate::presentation::liveness::should_keep_warm(visible) {
                    if let Some(ref client) = *collab.borrow() {
                        client.record_activity();
                    }
                }
                if crate::presentation::liveness::should_trigger_reconnect(visible, connected) {
                    let _ = set_reconnect_trigger.try_update(|n| *n += 1);
                }
            }
        });
    }

    // #210: a `visibilitychange` listener alongside the poll above gives
    // an immediate reconnect the instant the tab comes back to the
    // foreground, rather than waiting for the next `LIVENESS_POLL_MS`
    // tick — the poll is the backstop that also catches a mid-session
    // drop while the tab stays visible the whole time (a network blip),
    // which a visibility listener alone would never observe. Same
    // `window_event_listener_untyped` + `on_cleanup` pattern as the
    // keydown handler above.
    {
        let collab_for_visibility = std::rc::Rc::clone(&collab_client);
        let handle = window_event_listener_untyped("visibilitychange", move |_ev: web_sys::Event| {
            let visible = web_sys::window()
                .and_then(|w| w.document())
                .map(|d| !d.hidden())
                .unwrap_or(false);
            let connected = collab_for_visibility
                .borrow()
                .as_ref()
                .map(|c| c.is_connected())
                .unwrap_or(false);
            if crate::presentation::liveness::should_trigger_reconnect(visible, connected) {
                let _ = set_reconnect_trigger.try_update(|n| *n += 1);
            }
        });
        on_cleanup(move || handle.remove());
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
        // Tracks the previous `ws_synced` poll value so a false→true
        // transition (initial handshake completing, or a reconnect after
        // a mid-session drop) clears the dedup guard below. Without this,
        // a resync would never re-announce the current slide: the block
        // id hasn't changed, so `last_sent_block_id` would still match it
        // and the Effect would treat it as "already sent" even though the
        // reconnect means the room never actually received it.
        let was_synced: StoredValue<bool> = StoredValue::new(false);
        Effect::new(move |_| {
            let i = idx.get();
            let synced = ws_synced.get();
            if just_resynced(was_synced.get_value(), synced) {
                last_sent_block_id.set_value(None);
            }
            was_synced.set_value(synced);
            let ready = loaded.get() && deck.with(|d| !d.slides.is_empty());
            if !synced || !ready {
                return;
            }
            let Some(block_id) = deck.with_untracked(|d| slide_block_id(d, i)) else { return };
            if last_sent_block_id.get_value().as_deref() == Some(block_id.as_str()) {
                return;
            }
            // `ws_synced` is a 300ms poll of the raw connection flag
            // (see the comment above where it's populated) and can be up
            // to 300ms stale, so a run that reaches this point can still
            // race a disconnect that happened in that window —
            // `send_awareness` re-checks the live state internally and
            // silently no-ops if it's not actually synced. Recording
            // `last_sent_block_id` unconditionally after calling it would
            // poison the dedup guard on exactly that race: the block id
            // would look "sent" forever even though the frame was
            // dropped, and nothing else re-triggers the same id to retry
            // it. `client.is_synced()` reads that same live state
            // synchronously right here (no await between the check and
            // the call, single-threaded WASM), so gating the dedup write
            // on it — rather than on the polled `synced` local — records
            // only when the frame is actually known to have been queued.
            if let Some(ref client) = *collab.borrow() {
                if client.is_synced() {
                    client.send_awareness(
                        &user_id, &name, color_idx,
                        None, None, None, None,
                        Some(block_id.as_str()),
                    );
                    last_sent_block_id.set_value(Some(block_id));
                }
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

    // #210: a real slide change — manual navigation or a follow-driven
    // one, both of which land here through `set_idx` — is itself a
    // liveness signal, so record it too. This is a dedicated Effect
    // rather than folded into the awareness-broadcast Effect above on
    // purpose: that Effect also re-runs on every 300ms `ws_synced` poll
    // tick once synced (see its comment), so recording activity there
    // would re-arm `record_activity()` every 300ms — exactly the "ping
    // too fast" this liveness fix is meant to avoid. `prev_idx` gates on
    // an actual change so mounting at idx 0 counts once, not on every
    // poll-driven re-run of some other Effect.
    {
        let collab = std::rc::Rc::clone(&collab_client);
        let prev_idx: StoredValue<Option<usize>> = StoredValue::new(None);
        Effect::new(move |_| {
            let i = idx.get();
            if prev_idx.get_value() != Some(i) {
                prev_idx.set_value(Some(i));
                if let Some(ref client) = *collab.borrow() {
                    client.record_activity();
                }
            }
        });
    }

    view! {
        <main
            id="main-content"
            tabindex="-1"
            class="deck-present"
            class:deck-present--presenter=is_presenter_view
            on:click=on_stage_click
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
                <Show when=move || !presenters(&remote_cursors.get(), &my_session_id.get()).is_empty()>
                    <div class="deck-present__follow">
                        <Show
                            when=move || following.get().is_some() && paused.get()
                            fallback=move || view! {
                                <For each=move || {
                                            let my_sid = my_session_id.get();
                                            presenters(&remote_cursors.get(), &my_sid)
                                                .into_iter().map(|c| (c.session_id.clone(), c.name.clone())).collect::<Vec<_>>()
                                        }
                                         key=|(session_id, _)| session_id.clone()
                                         children=move |(session_id, name)| {
                                            let target_session_id = session_id.clone();
                                            view! {
                                                <button class="deck-present__follow-btn"
                                                    on:click=move |ev: web_sys::MouseEvent| {
                                                        ev.stop_propagation();
                                                        set_following.set(Some(target_session_id.clone()));
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
        </main>
    }
    .into_any()
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

    /// `session` defaults to `"{user}-sess"` when a test doesn't care
    /// about distinguishing sessions; tests exercising the #211
    /// same-user-multiple-sessions behavior pass explicit session ids
    /// via `cursor_with_session`.
    fn cursor(user: &str, presenting: Option<&str>) -> RemoteCursor {
        cursor_with_session(user, &format!("{user}-sess"), presenting)
    }

    fn cursor_with_session(user: &str, session: &str, presenting: Option<&str>) -> RemoteCursor {
        RemoteCursor {
            user_id: user.to_string(),
            name: format!("{user}-name"),
            color: "#fff".to_string(),
            cursor_block: None,
            selection_anchor_block: None,
            selection_head_block: None,
            typing_thread_id: None,
            presenting: presenting.map(|s| s.to_string()),
            session_id: session.to_string(),
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
        let p = presenters(&cs, "me-sess");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].user_id, "them");
    }

    /// #211: the whole point of session-keyed `presenters()` — a
    /// presenter's OTHER window (same user_id, different session_id)
    /// must be followable, and only the caller's *own* session is
    /// excluded.
    #[test]
    fn presenters_includes_same_user_different_session_excludes_only_own_session() {
        let cs = vec![
            cursor_with_session("me", "sess-projector", Some("s1")),
            cursor_with_session("me", "sess-control", Some("s1")),
            cursor("them", Some("s2")),
        ];
        // Viewing from the projector window: control window (same
        // user, different session) and them are both followable.
        let p = presenters(&cs, "sess-projector");
        let sessions: std::collections::HashSet<_> = p.iter().map(|c| c.session_id.as_str()).collect();
        assert_eq!(sessions, std::collections::HashSet::from(["sess-control", "them-sess"]));
        assert!(!sessions.contains("sess-projector"), "own session must be excluded");
    }

    #[test]
    fn followed_index_resolves_the_presenters_slide() {
        let d = deck(&["s1", "s2", "s3"]);
        let cs = vec![cursor("them", Some("s3"))];
        assert_eq!(followed_index(&d, &cs, Some("them-sess")), Some(2));
        assert_eq!(followed_index(&d, &cs, None), None, "not following");
        assert_eq!(followed_index(&d, &cs, Some("ghost-sess")), None, "presenter left");
        let cs_gone = vec![cursor("them", Some("deleted-slide"))];
        assert_eq!(followed_index(&d, &cs_gone, Some("them-sess")), None, "unknown slide id");
    }

    /// #211: following a SPECIFIC session keeps tracking that window
    /// even when the same user's other window is presenting a
    /// different slide — the two sessions must not be conflated.
    #[test]
    fn followed_index_distinguishes_sessions_of_the_same_user() {
        let d = deck(&["s1", "s2", "s3"]);
        let cs = vec![
            cursor_with_session("presenter", "sess-projector", Some("s1")),
            cursor_with_session("presenter", "sess-control", Some("s3")),
        ];
        assert_eq!(followed_index(&d, &cs, Some("sess-projector")), Some(0));
        assert_eq!(followed_index(&d, &cs, Some("sess-control")), Some(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn just_resynced_fires_only_on_the_false_to_true_edge() {
        assert!(just_resynced(false, true), "false->true is a resync");
        assert!(!just_resynced(true, true), "staying synced is not a resync");
        assert!(!just_resynced(false, false), "staying unsynced is not a resync");
        assert!(!just_resynced(true, false), "dropping is not itself a resync");
    }

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
