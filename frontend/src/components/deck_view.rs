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
    let (dragging_idx, set_dragging_idx) = signal::<Option<usize>>(None);

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
    // that's detected here and immediately backfilled with one blank
    // slide, then persisted so the doc a peer sees over WS already has a
    // canvas to render, not an empty deck. The persist is deferred by a
    // microtask (`a11y::defer`) rather than called synchronously inside
    // this Effect: `persist()` triggers `on_state_change`, which flows
    // back into `editor_state` and would otherwise re-enter this same
    // Effect while it's still on the stack.
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else { return };
        if persist_origin.get_untracked() {
            set_persist_origin.set(false);
            return;
        }

        let mut new_deck = deck_from_doc(&state.doc);
        let needs_bootstrap = new_deck.slides.is_empty();
        if needs_bootstrap {
            new_deck.slides.push(instantiate(blank_preset()));
        }
        deck.set(new_deck);
        if needs_bootstrap {
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

    let duplicate_at = move |idx: usize| {
        deck.update(|d| {
            let dup_idx = duplicate_slide(d, idx);
            set_active_slide.set(dup_idx);
        });
        persist();
    };

    let delete_at = move |idx: usize| {
        deck.update(|d| delete_slide(d, idx));
        let len = deck.with_untracked(|d| d.slides.len());
        if active_slide.get_untracked() >= len {
            set_active_slide.set(len.saturating_sub(1));
        }
        persist();
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
                    let Some(from) = dragging_idx.get_untracked() else { return };
                    let Some(target) = ev
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                        .and_then(|el| el.closest("[data-slide-idx]").ok().flatten())
                        .and_then(|el| el.get_attribute("data-slide-idx"))
                        .and_then(|s| s.parse::<usize>().ok())
                    else {
                        return;
                    };
                    if target != from {
                        deck.update(|d| move_slide(d, from, target));
                        set_dragging_idx.set(Some(target));
                    }
                }
                on:pointerup=move |_| {
                    if dragging_idx.get_untracked().is_some() {
                        set_dragging_idx.set(None);
                        persist();
                    }
                }
                on:pointercancel=move |_| set_dragging_idx.set(None)
            >
                <For
                    each={move || {
                        let items: Vec<(usize, DeckSlide)> =
                            deck.get().slides.into_iter().enumerate().collect();
                        items
                    }}
                    key={|(_, slide): &(usize, DeckSlide)| slide.block_id.clone()}
                    children=move |(i, slide)| {
                        let is_active = move || active_slide.get() == i;
                        // Reactive on `deck` (not just read once at row-creation
                        // time): the `<For>` only re-invokes this closure when
                        // the slide's key (blockId) is fresh, so a theme change
                        // — which touches every existing thumbnail without
                        // touching any key — would otherwise leave already-
                        // rendered thumbnails on the stale theme class.
                        let render_thumb = move || {
                            let theme = deck.with(|d| d.theme.clone());
                            render_deck_canvas(&slide, &theme)
                        };
                        view! {
                            <div
                                class="deck-slide-thumb"
                                class:deck-slide-thumb--active=is_active
                                data-slide-idx=i.to_string()
                                on:click=move |_| set_active_slide.set(i)
                                on:pointerdown=move |_| {
                                    if !readonly { set_dragging_idx.set(Some(i)); }
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
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                ev.stop_propagation();
                                                duplicate_at(i);
                                            }
                                        >"\u{2398}"</button>
                                        <button
                                            class="deck-slide-thumb__delete"
                                            title=crate::t!("deck-delete-slide")
                                            aria-label=crate::t!("deck-delete-slide")
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                ev.stop_propagation();
                                                delete_at(i);
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
}
