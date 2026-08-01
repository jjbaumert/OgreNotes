// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Slide-deck canvas view (Task 9) — the presentation-doc sibling of
//! `SpreadsheetView`. Architecture mirrors it deliberately: a local
//! `RwSignal<Deck>` working model kept in sync with `editor_state` via
//! a doc→model `Effect` (never built inline in the render closure —
//! see the mutex-re-entrancy precedent at
//! `spreadsheet_view.rs:2340`), a `persist_origin` guard signal that
//! stops the resync `Effect` from re-parsing the doc it just wrote
//! (`spreadsheet_view.rs:1357`/`:2363`), and a `persist()` closure
//! that serializes the model back to a `Doc` and hands it to the
//! host page via `on_state_change` (`spreadsheet_view.rs:1686`).
//!
//! Frame content renders read-only in this task: paragraphs,
//! headings, and lists render as plain HTML (mirrors
//! `diff_block_view.rs`'s block-type match); anything else renders a
//! placeholder box labeled with its node type. Real in-canvas editing
//! lands in Task 11.
//!
//! Every deck mutation (add/duplicate/move/delete slide) is a free
//! function taking `&mut Deck` — kept out of the component closure so
//! it's unit-testable without a reactive runtime (see the `tests`
//! module below).

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::editor::model::{generate_block_id, Fragment, Node, NodeType};
use crate::editor::state::EditorState;
use crate::presentation::model::{deck_from_doc, deck_to_doc, Deck, DeckSlide};
use crate::presentation::presets::{instantiate, LayoutPreset, LAYOUT_PRESETS};
use crate::presentation::themes::{theme_class, DECK_THEMES};

const BLANK_LAYOUT_ID: &str = "blank";

fn blank_preset() -> &'static LayoutPreset {
    LAYOUT_PRESETS
        .iter()
        .find(|p| p.id == BLANK_LAYOUT_ID)
        .expect("LAYOUT_PRESETS always carries a \"blank\" entry")
}

// ─── Pure mutations ────────────────────────────────────────────
//
// Each function mutates `deck.slides` in place. None of them ever
// leave a deck with zero slides — `delete_slide` re-seeds a single
// blank slide when it would otherwise empty the deck, since a
// presentation doc with no slides has no valid canvas to show.

/// Insert a fresh slide built from `preset` immediately after index
/// `after` (clamped to the deck's current length). Returns the
/// inserted slide's index.
pub fn add_slide(deck: &mut Deck, after: usize, preset: &LayoutPreset) -> usize {
    let insert_at = (after + 1).min(deck.slides.len());
    deck.slides.insert(insert_at, instantiate(preset));
    insert_at
}

/// Clone the slide at `idx` and insert the copy right after it, with
/// a fresh `blockId` on the slide itself and on every one of its
/// frames — `deck_to_doc` writes `blockId` verbatim, so a duplicate
/// that kept the source ids would collide with the original inside
/// yrs's `find_match`. Returns the duplicate's index; a no-op (idx
/// unchanged) if `idx` is out of range.
pub fn duplicate_slide(deck: &mut Deck, idx: usize) -> usize {
    let Some(source) = deck.slides.get(idx) else {
        return idx;
    };
    let mut dup = source.clone();
    dup.block_id = generate_block_id();
    for frame in &mut dup.frames {
        frame.block_id = generate_block_id();
    }
    let insert_at = idx + 1;
    deck.slides.insert(insert_at, dup);
    insert_at
}

/// Move the slide at `from` to sit at `to` (both clamped to the
/// deck's bounds). A no-op if `from` is out of range.
pub fn move_slide(deck: &mut Deck, from: usize, to: usize) {
    if from >= deck.slides.len() {
        return;
    }
    let slide = deck.slides.remove(from);
    let to = to.min(deck.slides.len());
    deck.slides.insert(to, slide);
}

/// Remove the slide at `idx`. If that would leave the deck with no
/// slides, a fresh blank slide takes its place instead — a deck
/// always has at least one slide, the same way a spreadsheet always
/// has at least one sheet.
pub fn delete_slide(deck: &mut Deck, idx: usize) {
    if idx >= deck.slides.len() {
        return;
    }
    deck.slides.remove(idx);
    if deck.slides.is_empty() {
        deck.slides.push(instantiate(blank_preset()));
    }
}

/// Decide whether a freshly-decoded, slide-less deck should be
/// self-healed with one blank slide — and build it if so.
///
/// A `readonly` session must never write to the shared document, not
/// even to self-heal an empty deck: a viewer without edit rights who
/// opens a slide-less presentation should just render the empty
/// state, not silently persist a blank slide on the doc's behalf.
/// Returns `None` when no bootstrap should happen (deck already has
/// slides, or the viewer is readonly); `Some(slide)` — a fresh
/// `instantiate(blank_preset())` — otherwise.
fn bootstrap_blank_slide(deck_is_empty: bool, readonly: bool) -> Option<DeckSlide> {
    if !deck_is_empty || readonly {
        return None;
    }
    Some(instantiate(blank_preset()))
}

/// Resolve a slide's *current* index in `slides` by its `block_id`.
///
/// Every per-row action (select / duplicate / delete / drag-drop)
/// resolves its target this way instead of trusting a positional
/// `usize` captured once when `<For>` first ran that row's `children`
/// closure. Leptos's keyed `<For>` never re-invokes `children` for an
/// existing key when the list reorders around it — insert, delete, or
/// drag anywhere else in the deck shifts everyone after it, and a
/// captured index goes stale silently. Resolving by id at the moment
/// of the action is correct even under a reorder that happened after
/// the row was rendered, including one applied by a concurrent remote
/// peer.
fn find_slide_index(slides: &[DeckSlide], block_id: &str) -> Option<usize> {
    slides.iter().position(|s| s.block_id == block_id)
}

// ─── Read-only frame content rendering ─────────────────────────
//
// Mirrors `diff_block_view.rs`'s block-type match: paragraphs as
// `<p>`, headings as `<h1>`..`<h6>`, lists as `<ul>`/`<ol>` of
// `<li>`. Any other node type (tables, code blocks, embeds, kanban,
// calendar, mermaid, …) renders a labeled placeholder box rather
// than attempting a faithful read-only render of every block kind —
// real in-frame editing (Task 11) replaces this wholesale.

fn render_node_readonly(node: &Node) -> AnyView {
    let Node::Element { node_type, attrs, content, .. } = node else {
        return view! { <span>{node.text_content()}</span> }.into_any();
    };
    match *node_type {
        NodeType::Paragraph => view! { <p>{node.text_content()}</p> }.into_any(),
        NodeType::Heading => {
            let level = attrs
                .get("level")
                .and_then(|l| l.parse::<u8>().ok())
                .unwrap_or(1)
                .clamp(1, 6);
            let text = node.text_content();
            match level {
                1 => view! { <h1>{text}</h1> }.into_any(),
                2 => view! { <h2>{text}</h2> }.into_any(),
                3 => view! { <h3>{text}</h3> }.into_any(),
                4 => view! { <h4>{text}</h4> }.into_any(),
                5 => view! { <h5>{text}</h5> }.into_any(),
                _ => view! { <h6>{text}</h6> }.into_any(),
            }
        }
        NodeType::BulletList => {
            let items = content
                .children
                .iter()
                .map(|c| view! { <li>{c.text_content()}</li> })
                .collect::<Vec<_>>();
            view! { <ul>{items}</ul> }.into_any()
        }
        NodeType::OrderedList => {
            let items = content
                .children
                .iter()
                .map(|c| view! { <li>{c.text_content()}</li> })
                .collect::<Vec<_>>();
            view! { <ol>{items}</ol> }.into_any()
        }
        other => {
            let label = format!("{other:?}");
            view! { <div class="deck-frame-placeholder">{label}</div> }.into_any()
        }
    }
}

fn render_frame_content(content: &Fragment) -> Vec<AnyView> {
    content.children.iter().map(render_node_readonly).collect()
}

/// Render one slide's frames (sorted by `z`) as absolutely-positioned
/// `.deck-frame` divs inside a `.deck-canvas` themed root. Shared by
/// the interactive active-slide canvas and the slide-strip thumbnails
/// so both stay pixel-identical (the thumbnail is just the same
/// markup shrunk with `transform: scale()`, per
/// `style/presentation.css`'s `.deck-slide-thumb__scaler` comment).
fn render_deck_canvas(slide: &DeckSlide, theme: &str) -> AnyView {
    let mut frames: Vec<_> = slide.frames.iter().collect();
    frames.sort_by_key(|f| f.z);
    let canvas_class = format!("deck-canvas {}", theme_class(theme));
    view! {
        <div class=canvas_class>
            {frames
                .into_iter()
                .map(|frame| {
                    let left = format!("{:.2}%", frame.rect.x * 100.0);
                    let top = format!("{:.2}%", frame.rect.y * 100.0);
                    let width = format!("{:.2}%", frame.rect.w * 100.0);
                    let height = format!("{:.2}%", frame.rect.h * 100.0);
                    view! {
                        <div
                            class="deck-frame"
                            style:left=left
                            style:top=top
                            style:width=width
                            style:height=height
                        >
                            {render_frame_content(&frame.content)}
                        </div>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
    .into_any()
}

/// Inject the presentation stylesheet on demand — same pattern as
/// `spreadsheet_view.rs:1205`'s `ensure_spreadsheet_css`. `/presentation.css`
/// is a Trunk `copy-file` (see `index.html`), not a bundled `rel="css"`, so a
/// pure-document or pure-spreadsheet session never fetches it. Idempotent via
/// a stable element id.
fn ensure_presentation_css() {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if doc.get_element_by_id("presentation-css").is_some() {
        return;
    }
    let Some(head) = doc.head() else {
        return;
    };
    if let Ok(link) = doc.create_element("link") {
        let _ = link.set_attribute("id", "presentation-css");
        let _ = link.set_attribute("rel", "stylesheet");
        let _ = link.set_attribute("href", "/presentation.css");
        let _ = head.append_child(&link);
    }
}

// ─── Component ─────────────────────────────────────────────────

#[component]
pub fn DeckView(
    editor_state: ReadSignal<Option<EditorState>>,
    on_state_change: Callback<EditorState>,
    /// REST-fallback ping fired after `on_state_change`, same
    /// contract as `SpreadsheetView::on_change`.
    on_change: Callback<()>,
    doc_id: String,
    readonly: bool,
    /// Frame block_id the user wants to comment on. Task 12 wires the
    /// popup on the page side; for now the page passes a no-op.
    on_request_frame_comment: Callback<String>,
    /// block_ids that currently have an open comment thread. Task 12
    /// wires this from the page's `list_threads` fetch; for now the
    /// page passes an empty signal.
    frame_threads: ReadSignal<Vec<String>>,
) -> impl IntoView {
    ensure_presentation_css();
    let _ = doc_id; // not yet consumed directly — plumbed through for parity with SpreadsheetView

    let deck = RwSignal::new(Deck {
        theme: crate::presentation::model::DEFAULT_THEME.to_string(),
        slide_size: "16:9".to_string(),
        slides: Vec::new(),
    });
    let (active_slide, set_active_slide) = signal(0usize);
    let (selected_frame, set_selected_frame) = signal::<Option<String>>(None);
    // Feedback-loop guard: set immediately before persist() emits its
    // editor_state change, cleared by the very next run of the doc→model
    // resync Effect below. Mirrors `spreadsheet_view.rs:1357`/`:2363`.
    let (persist_origin, set_persist_origin) = signal(false);
    let (picker_open, set_picker_open) = signal(false);
    // The dragged slide's identity, not its position — Leptos's keyed
    // `<For>` never re-invokes a row's `children` when the list reorders
    // around it, so a positional index captured at drag-start would go
    // stale the moment the first move lands. `block_id` never goes
    // stale; `find_slide_index` resolves its live position at the
    // moment each move actually needs it (see that function's doc
    // comment).
    let (dragging_block_id, set_dragging_block_id) = signal::<Option<String>>(None);

    // ─── Persist helper ────────────────────────────────────
    //
    // `spreadsheet_view.rs:1686`'s pattern: serialize the model,
    // arm the guard, hand the new doc up.
    let persist = move || {
        let doc = deck.with_untracked(deck_to_doc);
        set_persist_origin.set(true);
        on_state_change.run(EditorState::create_default(doc));
        on_change.run(());
    };

    // ─── Doc → model sync ──────────────────────────────────
    //
    // Runs as an Effect, never inline in the render closure — see the
    // mutex-re-entrancy comment at `spreadsheet_view.rs:2340`. A fresh
    // presentation doc decodes to zero slides (no `Slide` children yet);
    // that's detected here and, for an editable session, immediately
    // backfilled with one blank slide and persisted so the doc a peer
    // sees over WS already has a canvas to render. A `readonly` session
    // never persists (`bootstrap_blank_slide` returns `None`) — it just
    // renders the empty state; self-healing the doc is a write, and a
    // viewer without edit rights must not perform one, even implicitly.
    // The persist is deferred by a microtask (`a11y::defer`) rather than
    // called synchronously inside this Effect: `persist()` triggers
    // `on_state_change`, which flows back into `editor_state` and would
    // otherwise re-enter this same Effect while it's still on the stack.
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else { return };
        if persist_origin.get_untracked() {
            set_persist_origin.set(false);
            return;
        }

        let mut new_deck = deck_from_doc(&state.doc);
        let bootstrap = bootstrap_blank_slide(new_deck.slides.is_empty(), readonly);
        let should_persist = bootstrap.is_some();
        if let Some(slide) = bootstrap {
            new_deck.slides.push(slide);
        }
        deck.set(new_deck);
        if should_persist {
            crate::a11y::defer(persist);
        }
    });

    // ─── Slide-strip mutation handlers ─────────────────────

    let add_with_preset = move |preset_id: &'static str| {
        set_picker_open.set(false);
        let Some(preset) = LAYOUT_PRESETS.iter().find(|p| p.id == preset_id) else { return };
        let after = active_slide.get_untracked();
        deck.update(|d| {
            let idx = add_slide(d, after, preset);
            set_active_slide.set(idx);
        });
        persist();
    };

    // `duplicate_at`/`delete_at`/`select_slide` all take the row's
    // `block_id`, not a positional index — see `find_slide_index`'s doc
    // comment. Resolving the live index right before mutating is what
    // keeps these correct after any reorder that happened since the row
    // was last (re-)rendered.
    let duplicate_at = move |block_id: String| {
        let Some(idx) = deck.with_untracked(|d| find_slide_index(&d.slides, &block_id)) else {
            return;
        };
        deck.update(|d| {
            let dup_idx = duplicate_slide(d, idx);
            set_active_slide.set(dup_idx);
        });
        persist();
    };

    let delete_at = move |block_id: String| {
        let Some(idx) = deck.with_untracked(|d| find_slide_index(&d.slides, &block_id)) else {
            return;
        };
        deck.update(|d| delete_slide(d, idx));
        let len = deck.with_untracked(|d| d.slides.len());
        if active_slide.get_untracked() >= len {
            set_active_slide.set(len.saturating_sub(1));
        }
        persist();
    };

    let select_slide = move |block_id: &str| {
        if let Some(idx) = deck.with_untracked(|d| find_slide_index(&d.slides, block_id)) {
            set_active_slide.set(idx);
        }
    };

    let on_theme_change = move |ev: web_sys::Event| {
        let val = event_target_value(&ev);
        deck.update(|d| d.theme = val);
        persist();
    };

    // ─── Render ─────────────────────────────────────────────

    view! {
        <div class="deck-view">
            <div
                class="deck-view__strip"
                on:pointermove=move |ev: web_sys::PointerEvent| {
                    if readonly { return; }
                    let Some(from_id) = dragging_block_id.get_untracked() else { return };
                    // Not `ev.target()`: once `pointerdown` captures the
                    // pointer (below), every subsequent event for that
                    // pointer id is retargeted to the *capturing* element
                    // regardless of where the pointer physically is, so
                    // `ev.target()` would always report the drag origin,
                    // never whatever thumb is currently under the cursor.
                    // `elementFromPoint` reads real screen geometry instead.
                    let Some(target_id) = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|doc| {
                            doc.element_from_point(ev.client_x() as f32, ev.client_y() as f32)
                        })
                        .and_then(|el| el.closest("[data-slide-block-id]").ok().flatten())
                        .and_then(|el| el.get_attribute("data-slide-block-id"))
                    else {
                        return;
                    };
                    if target_id != from_id {
                        // Resolve both positions fresh from the current deck
                        // at the moment of the move, not from indices cached
                        // anywhere — correct even if a remote peer reordered
                        // slides mid-drag.
                        let positions = deck.with_untracked(|d| {
                            (
                                find_slide_index(&d.slides, &from_id),
                                find_slide_index(&d.slides, &target_id),
                            )
                        });
                        if let (Some(from), Some(to)) = positions {
                            if from != to {
                                deck.update(|d| move_slide(d, from, to));
                            }
                        }
                    }
                }
                on:pointerup=move |ev: web_sys::PointerEvent| {
                    // Pointer capture auto-releases on pointerup per spec,
                    // but release explicitly so cleanup doesn't depend on
                    // that — belt and suspenders across browsers.
                    if let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                        let _ = el.release_pointer_capture(ev.pointer_id());
                    }
                    if dragging_block_id.get_untracked().is_some() {
                        set_dragging_block_id.set(None);
                        persist();
                    }
                }
                on:pointercancel=move |ev: web_sys::PointerEvent| {
                    if let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                        let _ = el.release_pointer_capture(ev.pointer_id());
                    }
                    set_dragging_block_id.set(None);
                }
            >
                <For
                    each=move || deck.get().slides
                    key=|slide: &DeckSlide| slide.block_id.clone()
                    children=move |slide: DeckSlide| {
                        let block_id = slide.block_id.clone();
                        // Reactive on `deck` (not just read once at row-creation
                        // time): the `<For>` only re-invokes this closure when
                        // the slide's key (blockId) is fresh, so a theme change
                        // or a reorder — neither of which touches this row's own
                        // key — would otherwise leave already-rendered
                        // thumbnails on the stale theme class or highlight.
                        // Deriving `is_active` from "does the slide currently at
                        // `active_slide`'s index share my block_id" (rather than
                        // comparing two positional indices) also makes the
                        // active-thumb highlight immune to the same staleness
                        // that motivated `find_slide_index`.
                        let is_active = {
                            let block_id = block_id.clone();
                            move || {
                                deck.with(|d| {
                                    d.slides.get(active_slide.get()).map(|s| s.block_id.as_str())
                                        == Some(block_id.as_str())
                                })
                            }
                        };
                        let render_thumb = {
                            let slide = slide.clone();
                            move || {
                                let theme = deck.with(|d| d.theme.clone());
                                render_deck_canvas(&slide, &theme)
                            }
                        };
                        let block_id_click = block_id.clone();
                        let block_id_pointerdown = block_id.clone();
                        let block_id_duplicate = block_id.clone();
                        let block_id_delete = block_id.clone();
                        view! {
                            <div
                                class="deck-slide-thumb"
                                class:deck-slide-thumb--active=is_active
                                data-slide-block-id=block_id.clone()
                                on:click=move |_| select_slide(&block_id_click)
                                on:pointerdown=move |ev: web_sys::PointerEvent| {
                                    if readonly { return; }
                                    set_dragging_block_id.set(Some(block_id_pointerdown.clone()));
                                    // Capture on `current_target` (the thumb the
                                    // listener is attached to), not `target` (which
                                    // could be a descendant the press landed on) —
                                    // this is what keeps pointermove/up/cancel
                                    // routed here even if the pointer leaves the
                                    // strip entirely before release (Finding 3).
                                    if let Some(el) = ev
                                        .current_target()
                                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                                    {
                                        let _ = el.set_pointer_capture(ev.pointer_id());
                                    }
                                }
                            >
                                <div class="deck-slide-thumb__scaler">
                                    {render_thumb}
                                </div>
                                <Show when=move || !readonly>
                                    <div class="deck-slide-thumb__actions">
                                        <button
                                            class="deck-slide-thumb__duplicate"
                                            title=crate::t!("deck-duplicate-slide")
                                            aria-label=crate::t!("deck-duplicate-slide")
                                            on:click={
                                                // `<Show>`'s children closure can be
                                                // re-invoked every time `when` toggles
                                                // back to true, and each invocation
                                                // needs to build a fresh `on:click`
                                                // closure — cloning into a new local
                                                // here (rather than moving the outer
                                                // `block_id_duplicate` binding directly
                                                // into `on:click`) is what keeps the
                                                // enclosing closure `Fn` instead of
                                                // `FnOnce`.
                                                let block_id_duplicate = block_id_duplicate.clone();
                                                move |ev: web_sys::MouseEvent| {
                                                    ev.stop_propagation();
                                                    duplicate_at(block_id_duplicate.clone());
                                                }
                                            }
                                        >"\u{2398}"</button>
                                        <button
                                            class="deck-slide-thumb__delete"
                                            title=crate::t!("deck-delete-slide")
                                            aria-label=crate::t!("deck-delete-slide")
                                            on:click={
                                                let block_id_delete = block_id_delete.clone();
                                                move |ev: web_sys::MouseEvent| {
                                                    ev.stop_propagation();
                                                    delete_at(block_id_delete.clone());
                                                }
                                            }
                                        >"\u{2715}"</button>
                                    </div>
                                </Show>
                            </div>
                        }
                    }
                />

                <Show when=move || !readonly>
                    <div class="deck-view__add-slide">
                        <button
                            class="deck-add-slide-btn"
                            on:click=move |_| set_picker_open.update(|v| *v = !*v)
                        >
                            {crate::t!("deck-add-slide")}
                        </button>
                        <Show when=move || picker_open.get()>
                            <div class="deck-preset-picker">
                                {LAYOUT_PRESETS
                                    .iter()
                                    .map(|preset| {
                                        let label = crate::i18n::translate(preset.label_key, None);
                                        let id = preset.id;
                                        view! {
                                            <button
                                                class="deck-preset-picker__item"
                                                on:click=move |_| add_with_preset(id)
                                            >
                                                {label}
                                            </button>
                                        }
                                    })
                                    .collect::<Vec<_>>()}
                            </div>
                        </Show>
                    </div>
                </Show>
            </div>

            <div class="deck-view__canvas-wrap">
                {move || {
                    let d = deck.get();
                    let idx = active_slide.get().min(d.slides.len().saturating_sub(1));
                    let Some(slide) = d.slides.get(idx).cloned() else {
                        return view! { <div class="deck-canvas"></div> }.into_any();
                    };
                    let theme = d.theme.clone();
                    let mut frames = slide.frames.to_vec();
                    frames.sort_by_key(|f| f.z);
                    let canvas_class = format!("deck-canvas {}", theme_class(&theme));
                    view! {
                        <div class=canvas_class>
                            {frames
                                .into_iter()
                                .map(|frame| {
                                    let block_id = frame.block_id.clone();
                                    let block_id_click = block_id.clone();
                                    let block_id_comment = block_id.clone();
                                    let is_selected = move || {
                                        selected_frame.get().as_deref() == Some(block_id.as_str())
                                    };
                                    let has_thread = {
                                        let block_id = block_id_comment.clone();
                                        move || frame_threads.get().iter().any(|t| t == &block_id)
                                    };
                                    let left = format!("{:.2}%", frame.rect.x * 100.0);
                                    let top = format!("{:.2}%", frame.rect.y * 100.0);
                                    let width = format!("{:.2}%", frame.rect.w * 100.0);
                                    let height = format!("{:.2}%", frame.rect.h * 100.0);
                                    view! {
                                        <div
                                            class="deck-frame"
                                            class:deck-frame--selected=is_selected
                                            style:left=left
                                            style:top=top
                                            style:width=width
                                            style:height=height
                                            on:click=move |_| {
                                                if !readonly {
                                                    set_selected_frame.set(Some(block_id_click.clone()));
                                                }
                                            }
                                        >
                                            {render_frame_content(&frame.content)}
                                            <button
                                                class="deck-frame__comment-btn"
                                                class:deck-frame__comment-btn--active=has_thread
                                                title=crate::t!("deck-frame-comment")
                                                aria-label=crate::t!("deck-frame-comment")
                                                on:click=move |ev: web_sys::MouseEvent| {
                                                    ev.stop_propagation();
                                                    on_request_frame_comment.run(block_id_comment.clone());
                                                }
                                            >"\u{1F4AC}"</button>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </div>
                    }
                        .into_any()
                }}
            </div>

            <div class="deck-view__pane">
                <Show when=move || !readonly>
                    <label class="deck-theme-picker">
                        {crate::t!("deck-theme-label")}
                        <select
                            class="deck-theme-select"
                            aria-label=crate::t!("deck-theme-label")
                            prop:value=move || deck.with(|d| d.theme.clone())
                            on:change=on_theme_change
                        >
                            {DECK_THEMES
                                .iter()
                                .map(|theme| {
                                    let label = crate::i18n::translate(theme.label_key, None);
                                    view! {
                                        <option value=theme.id>{label}</option>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </select>
                    </label>
                </Show>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::generate_block_id;
    use crate::presentation::model::{DeckFrame, FrameRole, Rect, DEFAULT_THEME};

    fn simple_slide() -> DeckSlide {
        DeckSlide {
            block_id: generate_block_id(),
            layout: "blank".to_string(),
            background: None,
            frames: vec![DeckFrame {
                block_id: generate_block_id(),
                rect: Rect::clamped(0.0, 0.0, 1.0, 1.0),
                z: 0,
                role: FrameRole::Content,
                content: Fragment::empty(),
            }],
        }
    }

    fn fixture_deck_two_slides() -> Deck {
        Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: vec![simple_slide(), simple_slide()],
        }
    }

    fn fixture_deck_one_slide() -> Deck {
        Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: vec![simple_slide()],
        }
    }

    #[test]
    fn slide_ops_add_duplicate_delete_reorder() {
        let mut deck = fixture_deck_two_slides();
        add_slide(&mut deck, 1, &LAYOUT_PRESETS[3]); // insert after index 1
        assert_eq!(deck.slides.len(), 3);
        let dup = duplicate_slide(&mut deck, 0);
        assert_ne!(deck.slides[1].block_id, deck.slides[0].block_id, "dup gets fresh ids");
        assert_eq!(deck.slides[1].frames.len(), deck.slides[0].frames.len());
        assert!(deck.slides[1].frames.iter().zip(&deck.slides[0].frames)
            .all(|(a, b)| a.block_id != b.block_id));
        let _ = dup;
        move_slide(&mut deck, 3, 0);
        delete_slide(&mut deck, 0);
        assert_eq!(deck.slides.len(), 3);
    }

    #[test]
    fn delete_last_slide_leaves_one_blank() {
        let mut deck = fixture_deck_one_slide();
        delete_slide(&mut deck, 0);
        assert_eq!(deck.slides.len(), 1, "a deck always has >= 1 slide");
        assert!(deck.slides[0].frames.is_empty());
    }

    #[test]
    fn add_slide_clamps_after_to_deck_length() {
        let mut deck = fixture_deck_one_slide();
        let idx = add_slide(&mut deck, 99, &LAYOUT_PRESETS[3]);
        assert_eq!(idx, 1);
        assert_eq!(deck.slides.len(), 2);
    }

    #[test]
    fn duplicate_out_of_range_is_a_no_op() {
        let mut deck = fixture_deck_one_slide();
        let idx = duplicate_slide(&mut deck, 5);
        assert_eq!(idx, 5);
        assert_eq!(deck.slides.len(), 1);
    }

    #[test]
    fn move_slide_out_of_range_is_a_no_op() {
        let mut deck = fixture_deck_two_slides();
        move_slide(&mut deck, 99, 0);
        assert_eq!(deck.slides.len(), 2);
    }

    fn named_slide(id: &str) -> DeckSlide {
        DeckSlide {
            block_id: id.to_string(),
            layout: "blank".to_string(),
            background: None,
            frames: vec![],
        }
    }

    fn block_ids(deck: &Deck) -> Vec<&str> {
        deck.slides.iter().map(|s| s.block_id.as_str()).collect()
    }

    #[test]
    fn find_slide_index_missing_block_id_is_none() {
        let deck = fixture_deck_two_slides();
        assert_eq!(find_slide_index(&deck.slides, "not-a-real-id"), None);
    }

    /// Review finding (Important, drag-reorder): a positional index
    /// cached at drag-start goes stale the moment anything else reorders
    /// the deck before the drop. This simulates exactly that — a
    /// concurrent remote reorder lands between "drag start" and "drop" —
    /// and asserts the block-id-based resolution (`find_slide_index`
    /// called fresh at drop time, the fix applied in `DeckView`'s
    /// pointermove handler) still moves the *slide the user actually
    /// grabbed*, landing it at the drop target's *current* position, not
    /// wherever the drag-start indices used to point.
    #[test]
    fn move_by_block_id_resolves_fresh_positions_after_concurrent_reorder() {
        let mut deck = Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: vec![named_slide("a"), named_slide("b"), named_slide("c")],
        };
        // Drag starts on "a" (index 0), intending to drop it after "c".
        // A stale positional-index implementation would cache from=0 here.
        assert_eq!(find_slide_index(&deck.slides, "a"), Some(0));

        // Before the drop lands, a concurrent remote peer reorders the
        // deck: "b" moves to the front.
        move_slide(&mut deck, 1, 0);
        assert_eq!(block_ids(&deck), vec!["b", "a", "c"]);

        // The drop resolves both "a" (the dragged slide) and "c" (the
        // drop target) fresh from the *current* deck — not from indices
        // captured at drag-start (which were 0 and 2, and are now wrong
        // for "a").
        let from = find_slide_index(&deck.slides, "a").expect("a still in deck");
        let to = find_slide_index(&deck.slides, "c").expect("c still in deck");
        move_slide(&mut deck, from, to);

        assert_eq!(
            block_ids(&deck),
            vec!["b", "c", "a"],
            "\"a\" — the slide actually dragged — lands at \"c\"'s live position, \
             unaffected by the earlier concurrent reorder"
        );
    }

    /// Review finding (Critical, readonly bootstrap): a readonly viewer
    /// must never trigger a self-heal write on a slide-less deck.
    #[test]
    fn bootstrap_blank_slide_readonly_never_bootstraps() {
        assert!(
            bootstrap_blank_slide(true, true).is_none(),
            "readonly must never self-heal an empty deck"
        );
        assert!(bootstrap_blank_slide(false, true).is_none());
    }

    #[test]
    fn bootstrap_blank_slide_editable_only_when_empty() {
        assert!(
            bootstrap_blank_slide(false, false).is_none(),
            "a non-empty deck needs no bootstrap"
        );
        let slide = bootstrap_blank_slide(true, false).expect("empty + editable bootstraps");
        assert_eq!(slide.layout, "blank");
        assert!(slide.frames.is_empty());
    }
}
