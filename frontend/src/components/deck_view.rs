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
use crate::presentation::geometry::{self, Axis, Corner, DragKind, Guide};
use crate::presentation::model::{deck_from_doc, deck_to_doc, Deck, DeckFrame, DeckSlide, FrameRole, Rect};
use crate::presentation::presets::{instantiate, LayoutPreset, LAYOUT_PRESETS};
use crate::presentation::themes::{theme_class, DECK_THEMES};

/// Snap-attraction distance in normalized (0..1) slide fractions —
/// how close a frame edge/center has to get to a guide line before
/// `geometry::snap` pulls it the rest of the way.
const SNAP_THRESHOLD: f64 = 0.01;

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

// ─── Frame mutations (Task 10) ─────────────────────────────────
//
// The frame analogue of the slide mutations above: pure functions
// over `&mut DeckSlide`, kept out of the component closure so drag
// commit / delete / duplicate / add are unit-testable without a
// reactive runtime. Every one of them resolves its target frame by
// `block_id` (never a captured index) for the same reason
// `find_slide_index` does — see that function's doc comment.

/// Geometry + seed content for the "Add text frame" toolbar button:
/// `(0.3, 0.3, 0.4, 0.2)` with a single empty paragraph, per
/// `design/presentations.md`.
const TEXT_FRAME_RECT: (f64, f64, f64, f64) = (0.3, 0.3, 0.4, 0.2);

/// Rect for a paste-created frame: the same size as
/// `TEXT_FRAME_RECT`, but centered on the slide rather than placed at
/// a fixed offset — the canvas keymap matrix specifies paste creates
/// a new **centered** frame, distinct from the toolbar button's fixed
/// placement.
fn centered_text_frame_rect() -> Rect {
    let (_, _, w, h) = TEXT_FRAME_RECT;
    Rect::clamped(0.5 - w / 2.0, 0.5 - h / 2.0, w, h)
}

/// Resolve a frame's *current* index within `frames` by its
/// `block_id` — the frame analogue of `find_slide_index`, and for the
/// same reason: an index captured at gesture-start (pointerdown, or
/// the moment a keymap action reads `selected_frame`) can go stale if
/// a concurrent remote edit reorders or deletes a frame before the
/// gesture commits.
fn find_frame_index(frames: &[DeckFrame], block_id: &str) -> Option<usize> {
    frames.iter().position(|f| f.block_id == block_id)
}

/// Insert a fresh frame into `slide`, placed above every existing
/// frame (`z` = current max + 1, so a newly added frame never renders
/// underneath earlier ones), and return its new `block_id`.
fn add_frame(slide: &mut DeckSlide, rect: Rect, role: FrameRole, content: Fragment) -> String {
    let z = slide.frames.iter().map(|f| f.z).max().map_or(0, |m| m + 1);
    let block_id = generate_block_id();
    slide.frames.push(DeckFrame { block_id: block_id.clone(), rect, z, role, content });
    block_id
}

/// Remove the frame `block_id` from `slide`. A no-op if it's already
/// gone (e.g. a concurrent remote delete raced this one, or a stale
/// double-fire of a delete keymap action).
fn delete_frame(slide: &mut DeckSlide, block_id: &str) {
    if let Some(idx) = find_frame_index(&slide.frames, block_id) {
        slide.frames.remove(idx);
    }
}

/// Clone the frame `block_id` and insert the copy right after it,
/// with a fresh `blockId` (same reasoning as `duplicate_slide` — a
/// duplicate that kept the source id would collide with it in yrs's
/// `find_match`) and nudged slightly so it's visibly distinct from
/// its source instead of sitting exactly on top of it. Returns the
/// duplicate's new `block_id`; `None` if `block_id` isn't found.
fn duplicate_frame(slide: &mut DeckSlide, block_id: &str) -> Option<String> {
    let idx = find_frame_index(&slide.frames, block_id)?;
    let mut dup = slide.frames[idx].clone();
    dup.block_id = generate_block_id();
    dup.rect = geometry::nudge(dup.rect, 0.02, 0.02);
    dup.z = slide.frames.iter().map(|f| f.z).max().map_or(0, |m| m + 1);
    let new_id = dup.block_id.clone();
    slide.frames.insert(idx + 1, dup);
    Some(new_id)
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
    // `role=notes` frames are never positioned on the canvas (design
    // doc, "Canvas keymap matrix" section) — they render only in the
    // collapsed notes drawer below the active canvas. This shared
    // renderer backs both the slide-strip thumbnails and (indirectly,
    // by the same filtering rule applied inline below) the active
    // canvas, so a notes frame never leaks into a thumbnail either.
    let mut frames: Vec<_> = slide.frames.iter().filter(|f| f.role == FrameRole::Content).collect();
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

    // ─── Frame drag/resize state (Task 10) ─────────────────
    //
    // Mirrors the slide-strip drag hardening above: the gesture
    // tracks the dragged frame's `block_id` and its rect *as it stood
    // at pointerdown*, never a captured index and never an
    // accumulated delta. `frame_drag` is the gesture descriptor;
    // `drag_preview`/`drag_guides` are the transient, per-pointermove
    // values the canvas renders from — the deck model itself is only
    // written once, at pointerup (`persist()` is one yrs write per
    // gesture, not per mousemove).
    #[derive(Clone)]
    struct FrameDrag {
        block_id: String,
        kind: DragKind,
        start_client_x: f64,
        start_client_y: f64,
        start_rect: Rect,
    }
    let (frame_drag, set_frame_drag) = signal::<Option<FrameDrag>>(None);
    let (drag_preview, set_drag_preview) = signal::<Option<Rect>>(None);
    let (drag_guides, set_drag_guides) = signal::<Vec<Guide>>(Vec::new());
    let canvas_ref = NodeRef::<leptos::html::Div>::new();
    // Arrow-key nudges apply directly to `deck` on every keydown (for
    // instant feedback — the deltas are tiny, no transient-signal
    // indirection needed the way pixel-drag has), but `persist()` is
    // coalesced to fire once on keyup rather than once per repeat
    // event a held-down arrow key generates. This flag tracks whether
    // any nudge happened since the last persist so an unrelated keyup
    // (releasing Shift, or any other key) doesn't trigger a spurious
    // write.
    let (nudge_dirty, set_nudge_dirty) = signal(false);

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

    let add_text_frame = move |_: web_sys::MouseEvent| {
        if readonly {
            return;
        }
        let idx = active_slide.get_untracked();
        let (x, y, w, h) = TEXT_FRAME_RECT;
        let content = Fragment::from(vec![Node::element_with_content(NodeType::Paragraph, Fragment::empty())]);
        let mut new_id: Option<String> = None;
        deck.update(|d| {
            if let Some(slide) = d.slides.get_mut(idx) {
                new_id = Some(add_frame(slide, Rect::clamped(x, y, w, h), FrameRole::Content, content));
            }
        });
        if let Some(id) = new_id {
            set_selected_frame.set(Some(id));
            persist();
        }
    };

    // ─── Frame drag/resize (Task 10) ────────────────────────
    //
    // Shared by the frame body (`DragKind::Move`, on:pointerdown on
    // `.deck-frame`) and each of the four corner handles
    // (`DragKind::Resize(corner)`, on:pointerdown on
    // `.deck-frame-handle`). Both call this with their own `kind`;
    // `ev.stop_propagation()` on a handle's own pointerdown keeps a
    // handle-press from *also* bubbling into the frame's own
    // pointerdown and starting a second, conflicting Move gesture.
    let start_frame_drag = move |block_id: String, kind: DragKind, ev: web_sys::PointerEvent| {
        if readonly {
            return;
        }
        ev.stop_propagation();
        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        let idx = active_slide.get_untracked();
        let Some(start_rect) = deck.with_untracked(|d| {
            d.slides
                .get(idx)
                .and_then(|s| s.frames.iter().find(|f| f.block_id == block_id))
                .map(|f| f.rect)
        }) else {
            return;
        };
        set_selected_frame.set(Some(block_id.clone()));
        set_drag_guides.set(Vec::new());
        set_drag_preview.set(Some(start_rect));
        set_frame_drag.set(Some(FrameDrag {
            block_id,
            kind,
            start_client_x: ev.client_x() as f64,
            start_client_y: ev.client_y() as f64,
            start_rect,
        }));
    };

    // Pixel deltas -> normalized (0..1) slide fractions by dividing
    // through the canvas's own `getBoundingClientRect()` size, per
    // the brief — never the viewport or a fixed constant, so this
    // stays correct at any zoom/canvas width. Applies `apply_drag`
    // then `snap` into the *transient* `drag_preview`/`drag_guides`
    // signals on every move; the deck model is untouched until
    // `commit_frame_drag` runs at pointerup.
    let on_canvas_pointermove = move |ev: web_sys::PointerEvent| {
        if readonly {
            return;
        }
        let Some(state) = frame_drag.get_untracked() else { return };
        let Some(canvas_el) = canvas_ref.get() else { return };
        let bounds = canvas_el.get_bounding_client_rect();
        let (w, h) = (bounds.width(), bounds.height());
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let dx = (ev.client_x() as f64 - state.start_client_x) / w;
        let dy = (ev.client_y() as f64 - state.start_client_y) / h;
        let dragged = geometry::apply_drag(state.start_rect, state.kind, dx, dy);
        let idx = active_slide.get_untracked();
        let others: Vec<Rect> = deck.with_untracked(|d| {
            d.slides
                .get(idx)
                .map(|s| {
                    s.frames
                        .iter()
                        .filter(|f| f.block_id != state.block_id && f.role == FrameRole::Content)
                        .map(|f| f.rect)
                        .collect()
                })
                .unwrap_or_default()
        });
        let (snapped, guides) = geometry::snap(dragged, &others, SNAP_THRESHOLD);
        set_drag_preview.set(Some(snapped));
        set_drag_guides.set(guides);
    };

    // One yrs write per gesture: resolves the dragged frame fresh by
    // `block_id` (never a captured index) so this still lands
    // correctly even if a concurrent remote edit touched the slide's
    // other frames mid-drag; if the dragged frame itself was deleted
    // remotely mid-gesture, `find_frame_index` comes back empty and
    // this is a no-op — no phantom persist of a frame that no longer
    // exists.
    let commit_frame_drag = move || {
        let Some(state) = frame_drag.get_untracked() else { return };
        set_frame_drag.set(None);
        let final_rect = drag_preview.get_untracked();
        set_drag_preview.set(None);
        set_drag_guides.set(Vec::new());
        let Some(final_rect) = final_rect else { return };
        if final_rect == state.start_rect {
            // A plain click (pointerdown immediately followed by
            // pointerup, no intervening pointermove) already selected
            // the frame at pointerdown — nothing actually moved, so
            // skip the write rather than persisting a no-op rect
            // change on every simple click-to-select.
            return;
        }
        let idx = active_slide.get_untracked();
        let mut applied = false;
        deck.update(|d| {
            if let Some(slide) = d.slides.get_mut(idx) {
                if let Some(pos) = find_frame_index(&slide.frames, &state.block_id) {
                    slide.frames[pos].rect = final_rect;
                    applied = true;
                }
            }
        });
        if applied {
            persist();
        }
    };

    let on_canvas_pointerup = move |ev: web_sys::PointerEvent| {
        if let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = el.release_pointer_capture(ev.pointer_id());
        }
        commit_frame_drag();
    };

    let on_canvas_pointercancel = move |ev: web_sys::PointerEvent| {
        if let Some(el) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = el.release_pointer_capture(ev.pointer_id());
        }
        // Cancel discards the gesture instead of committing it —
        // mirrors the slide-strip's pointercancel handling above.
        set_frame_drag.set(None);
        set_drag_preview.set(None);
        set_drag_guides.set(Vec::new());
    };

    // ─── Canvas keymap (Task 10) ─────────────────────────────
    //
    // Implements `design/presentations.md`'s "Canvas keymap matrix"
    // for the "frame selected, not editing" column — real in-frame
    // text editing (and its own key handling) lands in the
    // frame-editing task; until then every frame renders read-only,
    // so this handler never has an "editing" branch to dispatch to.
    let on_canvas_keydown = move |ev: web_sys::KeyboardEvent| {
        if readonly {
            return;
        }
        let ctrl_or_meta = ev.ctrl_key() || ev.meta_key();
        let key = ev.key();

        // Cmd/Ctrl-D: prevent the browser's bookmark-this-page
        // shortcut whenever the canvas has focus, whether or not a
        // frame happens to be selected to actually duplicate.
        if ctrl_or_meta && key.to_lowercase() == "d" {
            ev.prevent_default();
            let Some(block_id) = selected_frame.get_untracked() else { return };
            let idx = active_slide.get_untracked();
            let mut new_id: Option<String> = None;
            deck.update(|d| {
                if let Some(slide) = d.slides.get_mut(idx) {
                    new_id = duplicate_frame(slide, &block_id);
                }
            });
            if let Some(id) = new_id {
                set_selected_frame.set(Some(id));
                persist();
            }
            return;
        }

        match key.as_str() {
            "Escape" => {
                if selected_frame.get_untracked().is_some() {
                    ev.prevent_default();
                    set_selected_frame.set(None);
                }
            }
            // Real edit mode lands in the frame-editing task; for now
            // Enter on a selected frame is a no-op stub (the frame is
            // already selected — nothing further happens yet).
            "Enter" => {
                if selected_frame.get_untracked().is_some() {
                    ev.prevent_default();
                }
            }
            "Delete" | "Backspace" => {
                let Some(block_id) = selected_frame.get_untracked() else { return };
                ev.prevent_default();
                let idx = active_slide.get_untracked();
                deck.update(|d| {
                    if let Some(slide) = d.slides.get_mut(idx) {
                        delete_frame(slide, &block_id);
                    }
                });
                set_selected_frame.set(None);
                persist();
            }
            "Tab" => {
                ev.prevent_default();
                let idx = active_slide.get_untracked();
                let current = selected_frame.get_untracked();
                // `role=notes` frames are never on the canvas (they
                // render only in the notes drawer), so Tab must never
                // land selection on one — only content frames enter
                // the cycle.
                let content_only: Vec<DeckFrame> = deck.with_untracked(|d| {
                    d.slides
                        .get(idx)
                        .map(|s| s.frames.iter().filter(|f| f.role == FrameRole::Content).cloned().collect())
                        .unwrap_or_default()
                });
                let next = if ev.shift_key() {
                    geometry::previous_frame_id(&content_only, current.as_deref())
                } else {
                    geometry::next_frame_id(&content_only, current.as_deref())
                };
                set_selected_frame.set(next);
            }
            "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" => {
                let Some(block_id) = selected_frame.get_untracked() else { return };
                ev.prevent_default();
                let step = if ev.shift_key() { 0.05 } else { 0.01 };
                let (dx, dy) = match key.as_str() {
                    "ArrowUp" => (0.0, -step),
                    "ArrowDown" => (0.0, step),
                    "ArrowLeft" => (-step, 0.0),
                    _ => (step, 0.0),
                };
                let idx = active_slide.get_untracked();
                deck.update(|d| {
                    if let Some(slide) = d.slides.get_mut(idx) {
                        if let Some(frame) = slide.frames.iter_mut().find(|f| f.block_id == block_id) {
                            frame.rect = geometry::nudge(frame.rect, dx, dy);
                        }
                    }
                });
                set_nudge_dirty.set(true);
            }
            _ => {}
        }
    };

    // Coalesces the nudge persist: a held-down arrow key fires many
    // keydowns (each already applied to `deck` above for live visual
    // feedback) but only one keyup, so this is where the single yrs
    // write for the whole nudge streak happens.
    let on_canvas_keyup = move |ev: web_sys::KeyboardEvent| {
        if matches!(ev.key().as_str(), "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight")
            && nudge_dirty.get_untracked()
        {
            set_nudge_dirty.set(false);
            persist();
        }
    };

    // Paste (native ClipboardEvent, same approach as
    // `spreadsheet_view.rs`'s `on_paste`): always creates a new
    // centered frame from the clipboard's plain text, regardless of
    // whether a frame happens to be selected — per the keymap matrix,
    // "with no frame selected, paste on the canvas also creates a new
    // centered frame from the clipboard content."
    let on_canvas_paste = move |ev: web_sys::Event| {
        if readonly {
            return;
        }
        let Ok(ce) = ev.dyn_into::<web_sys::ClipboardEvent>() else { return };
        let Some(data) = ce.clipboard_data() else { return };
        let text = data.get_data("text/plain").unwrap_or_default();
        if text.trim().is_empty() {
            return;
        }
        ce.prevent_default();
        let idx = active_slide.get_untracked();
        let content =
            Fragment::from(vec![Node::element_with_content(NodeType::Paragraph, Fragment::from(vec![Node::text(&text)]))]);
        let mut new_id: Option<String> = None;
        deck.update(|d| {
            if let Some(slide) = d.slides.get_mut(idx) {
                new_id = Some(add_frame(slide, centered_text_frame_rect(), FrameRole::Content, content));
            }
        });
        if let Some(id) = new_id {
            set_selected_frame.set(Some(id));
            persist();
        }
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
                <div class="deck-view__canvas-column">
                    {move || {
                        let d = deck.get();
                        let idx = active_slide.get().min(d.slides.len().saturating_sub(1));
                        let Some(slide) = d.slides.get(idx).cloned() else {
                            return view! { <div class="deck-canvas"></div> }.into_any();
                        };
                        let theme = d.theme.clone();
                        // `role=notes` frames never render on the canvas itself
                        // (design doc, "Canvas keymap matrix") — they surface
                        // only in the collapsed drawer below. Content frames
                        // sort by `z` for paint order (later z on top).
                        let mut frames: Vec<_> =
                            slide.frames.iter().filter(|f| f.role == FrameRole::Content).cloned().collect();
                        frames.sort_by_key(|f| f.z);
                        let canvas_class = format!("deck-canvas {}", theme_class(&theme));
                        // Transient drag state is read here (not just inside a
                        // per-frame reactive prop) so the dragged frame's
                        // geometry — and the snap guides overlay — update on
                        // every pointermove, matching how a plain (non-drag)
                        // geometry mutation already needs this closure to
                        // re-run: `left`/`top`/`width`/`height` are computed
                        // once per run, not as their own reactive closures.
                        let dragging = frame_drag.get();
                        let preview = drag_preview.get();
                        let guides = drag_guides.get();
                        view! {
                            <div
                                class=canvas_class
                                tabindex="0"
                                node_ref=canvas_ref
                                on:keydown=on_canvas_keydown
                                on:keyup=on_canvas_keyup
                                on:paste=on_canvas_paste
                                on:pointermove=on_canvas_pointermove
                                on:pointerup=on_canvas_pointerup
                                on:pointercancel=on_canvas_pointercancel
                            >
                                {frames
                                    .into_iter()
                                    .map(|frame| {
                                        let block_id = frame.block_id.clone();
                                        let block_id_pointerdown = block_id.clone();
                                        let block_id_comment = block_id.clone();
                                        let is_selected = {
                                            let block_id = block_id.clone();
                                            move || selected_frame.get().as_deref() == Some(block_id.as_str())
                                        };
                                        // A second, independently-reactive closure over the
                                        // same block_id (not a plain bool computed once per
                                        // outer rebuild): Escape / Tab-Shift-Tab / a plain
                                        // click-to-select change `selected_frame` without
                                        // touching `deck`/`frame_drag`/`drag_preview`, so the
                                        // outer per-slide closure this frame is built inside
                                        // won't itself re-run — the handles' visibility has to
                                        // track selection via `<Show>`'s own reactivity instead.
                                        let is_selected_for_handles = {
                                            let block_id = block_id.clone();
                                            move || selected_frame.get().as_deref() == Some(block_id.as_str())
                                        };
                                        let has_thread = {
                                            let block_id = block_id_comment.clone();
                                            move || frame_threads.get().iter().any(|t| t == &block_id)
                                        };
                                        let effective_rect = if dragging.as_ref().is_some_and(|s| s.block_id == block_id) {
                                            preview.unwrap_or(frame.rect)
                                        } else {
                                            frame.rect
                                        };
                                        let left = format!("{:.2}%", effective_rect.x * 100.0);
                                        let top = format!("{:.2}%", effective_rect.y * 100.0);
                                        let width = format!("{:.2}%", effective_rect.w * 100.0);
                                        let height = format!("{:.2}%", effective_rect.h * 100.0);
                                        view! {
                                            <div
                                                class="deck-frame"
                                                class:deck-frame--selected=is_selected
                                                class:deck-frame--readonly=readonly
                                                style:left=left
                                                style:top=top
                                                style:width=width
                                                style:height=height
                                                on:pointerdown=move |ev: web_sys::PointerEvent| {
                                                    start_frame_drag(block_id_pointerdown.clone(), DragKind::Move, ev);
                                                }
                                            >
                                                {render_frame_content(&frame.content)}
                                                <button
                                                    class="deck-frame__comment-btn"
                                                    class:deck-frame__comment-btn--active=has_thread
                                                    title=crate::t!("deck-frame-comment")
                                                    aria-label=crate::t!("deck-frame-comment")
                                                    on:pointerdown=|ev: web_sys::PointerEvent| ev.stop_propagation()
                                                    on:click=move |ev: web_sys::MouseEvent| {
                                                        ev.stop_propagation();
                                                        on_request_frame_comment.run(block_id_comment.clone());
                                                    }
                                                >"\u{1F4AC}"</button>
                                                <Show when=move || is_selected_for_handles() && !readonly>
                                                    {
                                                        let block_id = block_id.clone();
                                                        [Corner::Nw, Corner::Ne, Corner::Sw, Corner::Se]
                                                            .into_iter()
                                                            .map(|corner| {
                                                                let block_id = block_id.clone();
                                                                let class = format!(
                                                                    "deck-frame-handle deck-frame-handle--{}",
                                                                    match corner {
                                                                        Corner::Nw => "nw",
                                                                        Corner::Ne => "ne",
                                                                        Corner::Sw => "sw",
                                                                        Corner::Se => "se",
                                                                    }
                                                                );
                                                                view! {
                                                                    <div
                                                                        class=class
                                                                        on:pointerdown=move |ev: web_sys::PointerEvent| {
                                                                            start_frame_drag(
                                                                                block_id.clone(),
                                                                                DragKind::Resize(corner),
                                                                                ev,
                                                                            );
                                                                        }
                                                                    ></div>
                                                                }
                                                            })
                                                            .collect::<Vec<_>>()
                                                    }
                                                </Show>
                                            </div>
                                        }
                                    })
                                    .collect::<Vec<_>>()}
                                {guides
                                    .into_iter()
                                    .map(|g| match g.axis {
                                        Axis::X => {
                                            let left = format!("{:.4}%", g.at * 100.0);
                                            view! { <div class="deck-snap-guide deck-snap-guide--x" style:left=left></div> }
                                                .into_any()
                                        }
                                        Axis::Y => {
                                            let top = format!("{:.4}%", g.at * 100.0);
                                            view! { <div class="deck-snap-guide deck-snap-guide--y" style:top=top></div> }
                                                .into_any()
                                        }
                                    })
                                    .collect::<Vec<_>>()}
                            </div>
                        }
                            .into_any()
                    }}
                    <Show when=move || {
                        deck.with(|d| {
                            let idx = active_slide.get().min(d.slides.len().saturating_sub(1));
                            d.slides.get(idx).is_some_and(|s| s.frames.iter().any(|f| f.role == FrameRole::Notes))
                        })
                    }>
                        <details class="deck-notes-drawer">
                            <summary>{crate::t!("deck-notes-drawer-label")}</summary>
                            <div class="deck-notes-drawer__body">
                                {move || {
                                    deck.with(|d| {
                                        let idx = active_slide.get().min(d.slides.len().saturating_sub(1));
                                        d.slides
                                            .get(idx)
                                            .map(|s| {
                                                s.frames
                                                    .iter()
                                                    .filter(|f| f.role == FrameRole::Notes)
                                                    .map(|f| {
                                                        view! {
                                                            <div class="deck-notes-drawer__frame">
                                                                {render_frame_content(&f.content)}
                                                            </div>
                                                        }
                                                    })
                                                    .collect::<Vec<_>>()
                                            })
                                            .unwrap_or_default()
                                    })
                                }}
                            </div>
                        </details>
                    </Show>
                </div>
            </div>

            <div class="deck-view__pane">
                <Show when=move || !readonly>
                    <button class="deck-add-text-frame-btn" on:click=add_text_frame>
                        {crate::t!("deck-add-text-frame")}
                    </button>
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

    // ─── Frame mutations (Task 10) ─────────────────────────

    #[test]
    fn add_frame_places_above_existing_frames_and_returns_fresh_id() {
        let mut slide = simple_slide(); // one frame, z = 0
        let id = add_frame(&mut slide, Rect::clamped(0.3, 0.3, 0.4, 0.2), FrameRole::Content, Fragment::empty());
        assert_eq!(slide.frames.len(), 2);
        let added = slide.frames.iter().find(|f| f.block_id == id).expect("frame was inserted");
        assert!(added.z > slide.frames[0].z, "new frame paints above the existing one");
        assert_ne!(id, slide.frames[0].block_id);
    }

    #[test]
    fn add_frame_on_empty_slide_starts_at_z_zero() {
        let mut slide = named_slide("s");
        let id = add_frame(&mut slide, Rect::clamped(0.0, 0.0, 0.5, 0.5), FrameRole::Notes, Fragment::empty());
        let added = &slide.frames[0];
        assert_eq!(added.block_id, id);
        assert_eq!(added.z, 0);
        assert_eq!(added.role, FrameRole::Notes);
    }

    #[test]
    fn delete_frame_removes_matching_block_id() {
        let mut slide = simple_slide();
        let target = slide.frames[0].block_id.clone();
        delete_frame(&mut slide, &target);
        assert!(slide.frames.is_empty());
    }

    #[test]
    fn delete_frame_missing_block_id_is_a_no_op() {
        let mut slide = simple_slide();
        let before = slide.frames.len();
        delete_frame(&mut slide, "not-a-real-id");
        assert_eq!(slide.frames.len(), before);
    }

    #[test]
    fn duplicate_frame_gets_fresh_id_and_is_offset_from_source() {
        // Needs headroom to nudge into (`simple_slide`'s frame is
        // full-bleed 0,0,1,1 and would clamp right back to itself).
        let mut slide = named_slide("s");
        slide.frames.push(DeckFrame {
            block_id: generate_block_id(),
            rect: Rect::clamped(0.2, 0.2, 0.3, 0.3),
            z: 0,
            role: FrameRole::Content,
            content: Fragment::empty(),
        });
        let source_id = slide.frames[0].block_id.clone();
        let source_rect = slide.frames[0].rect;
        let dup_id = duplicate_frame(&mut slide, &source_id).expect("source frame exists");
        assert_eq!(slide.frames.len(), 2);
        assert_ne!(dup_id, source_id, "duplicate never reuses the source blockId");
        let dup = slide.frames.iter().find(|f| f.block_id == dup_id).unwrap();
        assert_ne!(dup.rect, source_rect, "duplicate is nudged, not stacked exactly on the source");
        assert!(dup.z > slide.frames.iter().find(|f| f.block_id == source_id).unwrap().z);
    }

    #[test]
    fn duplicate_frame_missing_block_id_returns_none() {
        let mut slide = simple_slide();
        assert_eq!(duplicate_frame(&mut slide, "not-a-real-id"), None);
        assert_eq!(slide.frames.len(), 1, "no-op leaves the slide untouched");
    }

    #[test]
    fn centered_text_frame_rect_is_centered_on_the_slide() {
        let r = centered_text_frame_rect();
        assert!((r.x + r.w / 2.0 - 0.5).abs() < 1e-9);
        assert!((r.y + r.h / 2.0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn find_frame_index_missing_block_id_is_none() {
        let slide = simple_slide();
        assert_eq!(find_frame_index(&slide.frames, "not-a-real-id"), None);
    }
}
