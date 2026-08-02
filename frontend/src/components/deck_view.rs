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
//! Frame content normally renders read-only: paragraphs, headings,
//! and lists render as plain HTML (mirrors `diff_block_view.rs`'s
//! block-type match); anything else renders a placeholder box
//! labeled with its node type. The one frame named by `editing_frame`
//! instead mounts a scoped `EditorComponent` over a synthetic Doc
//! wrapping just that frame's content, for real in-place text editing
//! (Task 11).
//!
//! Every deck mutation (add/duplicate/move/delete slide) is a free
//! function taking `&mut Deck` — kept out of the component closure so
//! it's unit-testable without a reactive runtime (see the `tests`
//! module below).

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::editor::model::{generate_block_id, Fragment, Node, NodeType};
use crate::editor::state::EditorState;
use crate::editor::yrs_bridge::doc_to_ydoc_bytes;
use crate::presentation::geometry::{self, Axis, Corner, DragKind, Guide};
use crate::presentation::model::{
    deck_from_doc, deck_to_doc, merge_remote_deck, replace_frame_content, Deck, DeckFrame, DeckSlide, FrameRole, Rect,
};
use crate::presentation::presets::{instantiate, LayoutPreset, LAYOUT_PRESETS};
use crate::presentation::themes::{theme_class, DECK_THEMES};

use super::editor_component::{EditorComponent, EditorProps};
use super::toolbar::ToolbarCommand;

/// DOM attribute carrying a frame's `block_id`, set on every
/// `.deck-frame` div. Two independent mechanisms key off it: the
/// global outside-click listener (below) walks up from
/// `ev.target()` via `.closest("[data-deck-frame-block-id]")` to
/// decide whether a pointerdown landed inside the frame currently
/// being edited, and it doubles as a stable per-frame DOM hook for
/// any future test/automation selector.
const FRAME_BLOCK_ID_ATTR: &str = "data-deck-frame-block-id";

/// Build a `[data-deck-frame-block-id="…"]` selector for locating a
/// frame's DOM element from outside this component — the page-level
/// comment-popup positioning path (`document.rs`'s
/// `request_frame_comment`, Task 12) queries a frame's element the
/// same way `document.rs`'s own `block_id_selector` queries an
/// editor block's, and needs the same escaping for the same reason:
/// a block id containing `"` or `\` must not be able to break out of
/// the selector string and throw a `DOMException`.
pub(crate) fn frame_selector(block_id: &str) -> String {
    let escaped = block_id.replace('\\', "\\\\").replace('"', "\\\"");
    format!("[{FRAME_BLOCK_ID_ATTR}=\"{escaped}\"]")
}

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

/// Re-apply a locally-nudged-but-not-yet-persisted frame's rect onto a
/// freshly merged deck (resync-Effect Finding I1).
///
/// The old sequencing called `flush_nudge` *before* computing
/// `merge_remote_deck`, which persisted the PRE-MERGE local snapshot.
/// That was unsafe two ways: by send time `ws_client`'s
/// `last_synced_doc` had already rebased past the incoming remote
/// update, so the stale doc's sync could delete peer-added slides or
/// revert peer edits wholesale; and it self-defeated the nudge itself,
/// since the merge that followed adopted the remote rect for the very
/// frame just nudged.
///
/// Calling this *after* `merge_remote_deck` instead means the deck
/// that ends up persisted has every peer change AND the local nudge.
/// Returns whether `frame_id` was found in `merged` (and thus the rect
/// applied) — `false` when a concurrent remote delete raced the nudge,
/// which is a legitimate no-op, not an error.
fn carry_nudge(merged: &mut Deck, frame_id: &str, rect: Rect) -> bool {
    for slide in merged.slides.iter_mut() {
        if let Some(frame) = slide.frames.iter_mut().find(|f| f.block_id == frame_id) {
            frame.rect = rect;
            return true;
        }
    }
    false
}

/// Task 11 review, Finding 2 — whether the frame named by `editing_id`
/// (the embedded editor's currently-open frame, if any) is still part
/// of the *active* slide's frame list. `None` (nothing being edited)
/// is trivially "still visible" — there's nothing to close.
///
/// Every slide-strip mutation that can change which slide is active
/// (`add_with_preset`, `duplicate_at`, `delete_at`, `select_slide`)
/// calls this *after* applying its own mutation and settling
/// `active_slide`, and closes the editor when it comes back `false`.
/// A blanket "always close on any slide-strip action" would be wrong
/// in the opposite direction — re-clicking the *already*-active
/// slide's own thumbnail (a no-op `select_slide`) must not spuriously
/// kick the user out of an open editor — and it would also miss a
/// subtler case: `delete_at` only clamps `active_slide` when it goes
/// *out of bounds*, so deleting a slide *before* the active one shifts
/// every later slide's index down by one without moving
/// `active_slide` itself, silently swapping in a different slide
/// underneath the same index. Re-deriving visibility from the live
/// `Deck` after the mutation (rather than reasoning about *why* the
/// active slide might have changed) handles both cases uniformly.
fn editing_frame_still_visible(deck: &Deck, active_slide_idx: usize, editing_id: Option<&str>) -> bool {
    let Some(id) = editing_id else { return true };
    deck.slides.get(active_slide_idx).is_some_and(|s| s.frames.iter().any(|f| f.block_id == id))
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

// ─── Comment-thread filtering (Task 12) ────────────────────────
//
// Frames are `is_commentable() == true` with ordinary blockIds, so a
// frame's comment threads are just rows in the same doc-wide
// (block_id, thread_id) list the editor's inline-comment highlights
// use — no separate storage or fetch. This is the pure filter behind
// the per-frame comment badge: which of those threads actually
// belong to a frame of the given slide (as opposed to some other
// block elsewhere in the doc whose id happens to be in the list).
// Kept out of the component closure, like the mutation functions
// above, so it's unit-testable without a reactive runtime.

/// (block_id, thread_id) pairs in, thread_ids whose block_id belongs
/// to a frame of `slide` out. An out-of-range `slide` yields an empty
/// result rather than panicking — callers pass a live slide index
/// that can momentarily lag a concurrent slide deletion.
pub fn threads_for_slide(deck: &Deck, slide: usize, threads: &[(String, String)]) -> Vec<String> {
    let Some(slide) = deck.slides.get(slide) else {
        return Vec::new();
    };
    threads
        .iter()
        .filter(|(block_id, _)| slide.frames.iter().any(|f| &f.block_id == block_id))
        .map(|(_, thread_id)| thread_id.clone())
        .collect()
}

// ─── Read-only frame content rendering ─────────────────────────
//
// Mirrors `diff_block_view.rs`'s block-type match: paragraphs as
// `<p>`, headings as `<h1>`..`<h6>`, lists as `<ul>`/`<ol>` of
// `<li>`. Any other node type (tables, code blocks, embeds, kanban,
// calendar, mermaid, …) renders a labeled placeholder box rather
// than attempting a faithful read-only render of every block kind —
// real in-frame editing (Task 11) replaces this wholesale.

/// Mount an imperatively built `web_sys::Node` inside the declarative
/// Leptos tree. The deck renderer needs this for the two DOM-producing
/// paths that can't be expressed as `view!` markup: the shared image
/// builder (`editor::view::build_image_element`, async blob-ref
/// resolution mutates the element after mount) and the live-app block
/// views (`editor::blocks::view_for`, which return ready-made DOM).
/// The whole subtree is rebuilt by the surrounding reactive closure on
/// content change, so a one-shot append is sufficient — the Effect
/// guards against double-append on re-run.
/// Render a delegated live-app block by SERIALIZING the DOM its block
/// view builds and re-parsing it via `inner_html`. Imperative-mount
/// approaches (NodeRef + Effect, NodeRef + rAF) both fail here: this
/// runs inside render closures that re-run on every deck change, whose
/// transient reactive owners dispose effects before they fire and
/// whose NodeRefs never resolve. Serialization is safe — the block
/// views build their DOM with escaped attributes (`set_attribute`) and
/// renderer-generated SVG, and the canvas copy is display-only
/// (`.deck-frame__content` is pointer-events:none), so no live
/// listeners are lost.
fn raw_dom_html(node: Option<web_sys::Node>) -> AnyView {
    let html = node
        .and_then(|n| n.dyn_ref::<web_sys::Element>().map(|el| el.outer_html()))
        .unwrap_or_default();
    view! { <div class="deck-raw-block" inner_html=html></div> }.into_any()
}

/// Bump-signal pair provided by `DeckView` (a STABLE owner) so async
/// image resolution can re-run the canvas's readonly render closures.
/// Per-render signals/effects don't survive here — the render closures
/// re-run on every deck change and dispose whatever transient reactive
/// state the previous run created (the same trap that broke the
/// NodeRef/Effect mounting attempts for delegated blocks).
#[derive(Clone, Copy)]
struct DeckImgRefresh(ReadSignal<u32>, WriteSignal<u32>);

/// Image rendering for the canvas: synchronous blob-ref cache hit →
/// plain-string `src` (same `image_bridge` cache the editor warms). On
/// a cache miss the resolve callback bumps the DeckView-scoped refresh
/// signal — which this fn READS, subscribing the surrounding reactive
/// render closure — so the canvas re-renders once the URL exists and
/// takes the cache-hit path.
fn render_frame_image(attrs: &std::collections::HashMap<String, String>) -> AnyView {
    use crate::editor::view::is_safe_url;
    let refresh = use_context::<DeckImgRefresh>();
    if let Some(r) = &refresh {
        r.0.get(); // subscribe the caller's render closure to bumps
    }
    let alt = attrs.get("alt").cloned().unwrap_or_default();
    let mut src = String::new();
    if let Some(s) = attrs.get("src") {
        if let Some((blob_id, key)) = crate::editor::blob_ref::parse_blob_ref(s) {
            let bump = refresh.map(|r| r.1);
            match crate::editor::image_bridge::resolve(&blob_id, &key, move |url| {
                if is_safe_url(&url) {
                    if let Some(set) = bump {
                        let _ = set.try_update(|v| *v = v.wrapping_add(1));
                    }
                }
            }) {
                Some(url) if is_safe_url(&url) => src = url,
                _ => {}
            }
        } else if is_safe_url(s) {
            src = s.clone();
        }
    }
    view! { <img src=src alt=alt /> }.into_any()
}

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
            let items = render_list_items(content);
            view! { <ul>{items}</ul> }.into_any()
        }
        NodeType::OrderedList => {
            let items = render_list_items(content);
            view! { <ol>{items}</ol> }.into_any()
        }
        NodeType::TaskList => {
            let items = content
                .children
                .iter()
                .map(|c| {
                    let checked = c
                        .attrs()
                        .get("checked")
                        .map(|v| v == "true")
                        .unwrap_or(false);
                    let glyph = if checked { "\u{2611} " } else { "\u{2610} " };
                    let body = render_frame_children(c);
                    view! { <li class="deck-task-item">{glyph}{body}</li> }
                })
                .collect::<Vec<_>>();
            view! { <ul class="deck-task-list">{items}</ul> }.into_any()
        }
        NodeType::Blockquote => {
            let body = render_frame_children(node);
            view! { <blockquote>{body}</blockquote> }.into_any()
        }
        NodeType::CodeBlock => {
            // Text-only on the canvas (no syntax highlighting) per the
            // spec's out-of-scope list.
            let text = node.text_content();
            view! { <pre class="deck-code-block"><code>{text}</code></pre> }.into_any()
        }
        NodeType::HorizontalRule => view! { <hr /> }.into_any(),
        NodeType::HardBreak => view! { <br /> }.into_any(),
        NodeType::Table => {
            let rows = content
                .children
                .iter()
                .map(|row| {
                    let cells = match row {
                        Node::Element { content, .. } => content
                            .children
                            .iter()
                            .map(|cell| {
                                let is_header = cell.node_type() == Some(NodeType::TableHeader);
                                let body = render_frame_children(cell);
                                if is_header {
                                    view! { <th>{body}</th> }.into_any()
                                } else {
                                    view! { <td>{body}</td> }.into_any()
                                }
                            })
                            .collect::<Vec<_>>(),
                        _ => Vec::new(),
                    };
                    view! { <tr>{cells}</tr> }
                })
                .collect::<Vec<_>>();
            view! { <table class="deck-table"><tbody>{rows}</tbody></table> }.into_any()
        }
        NodeType::Embed => {
            // Link-card chip, never an iframe on the canvas (spec).
            let label = embed_chip_label(attrs);
            view! { <div class="deck-embed-chip">{label}</div> }.into_any()
        }
        NodeType::Image => render_frame_image(attrs),
        NodeType::Mermaid | NodeType::Calendar | NodeType::Kanban => {
            // Delegate to the live-app block views — the same DOM the
            // document editor renders (mermaid SVG included), shown
            // static here (see `.deck-frame__content` pointer-events).
            let built = web_sys::window().and_then(|w| w.document()).and_then(|d| {
                crate::editor::blocks::view_for(*node_type)
                    .and_then(|b| b.render(&d, *node_type, attrs, content))
            });
            raw_dom_html(built)
        }
        other => {
            let label = format!("{other:?}");
            view! { <div class="deck-frame-placeholder">{label}</div> }.into_any()
        }
    }
}

/// Recursive list-item rendering so nested lists and non-text item
/// children render instead of being flattened to `text_content()`.
fn render_list_items(content: &Fragment) -> Vec<AnyView> {
    content
        .children
        .iter()
        .map(|c| {
            let body = render_frame_children(c);
            view! { <li>{body}</li> }.into_any()
        })
        .collect()
}

/// Render a node's children (paragraph unwrapped inline where the
/// parent supplies the box, e.g. list items / table cells / quotes).
fn render_frame_children(node: &Node) -> Vec<AnyView> {
    match node {
        Node::Element { content, .. } => {
            content.children.iter().map(render_node_readonly).collect()
        }
        Node::Text { .. } => vec![view! { <span>{node.text_content()}</span> }.into_any()],
    }
}

/// Chip text for an Embed on the canvas: title if present, else the
/// URL, else a generic label.
fn embed_chip_label(attrs: &std::collections::HashMap<String, String>) -> String {
    attrs
        .get("title")
        .filter(|t| !t.trim().is_empty())
        .or_else(|| attrs.get("url").filter(|u| !u.trim().is_empty()))
        .cloned()
        .unwrap_or_else(|| crate::i18n::translate("deck-embed-chip-fallback", None))
}

/// True when a frame's content carries no visible text anywhere —
/// i.e. only empty/whitespace text leaves under any nesting. Drives
/// the render-time placeholder hint: presets seed EMPTY nodes (see
/// `presets::seed_content`) so entering the frame gives a bare caret
/// instead of forcing the user to delete baked-in placeholder prose.
fn fragment_is_visually_empty(content: &Fragment) -> bool {
    fn node_empty(node: &Node) -> bool {
        match node {
            Node::Text { text, .. } => text.trim().is_empty(),
            Node::Element { node_type, content, .. } => match node_type {
                // Attr-driven / intrinsically visible blocks carry no
                // text children but ARE content — a mermaid diagram's
                // source lives in its attrs, an image in `src`, a
                // divider is pure chrome. Classifying these as empty
                // painted the placeholder hint over real content (the
                // frame-blocks launch bug: an inserted mermaid
                // "vanished" behind "Click to add text").
                NodeType::Image
                | NodeType::Mermaid
                | NodeType::Calendar
                | NodeType::Kanban
                | NodeType::Embed
                | NodeType::HorizontalRule
                | NodeType::Table => false,
                _ => content.children.iter().all(node_empty),
            },
        }
    }
    content.children.iter().all(node_empty)
}

/// i18n key for the empty-frame hint, keyed off the first child's
/// node type so a heading frame invites a heading. Falls back to the
/// body hint for anything else (including a fully empty fragment).
fn placeholder_key_for(content: &Fragment) -> &'static str {
    match content.children.first().and_then(|n| n.node_type()) {
        Some(NodeType::Heading) => "deck-placeholder-heading",
        _ => "deck-placeholder-body",
    }
}

pub(crate) fn render_frame_content(content: &Fragment) -> AnyView {
    if fragment_is_visually_empty(content) {
        let hint = crate::i18n::translate(placeholder_key_for(content), None);
        return view! { <div class="deck-frame-placeholder">{hint}</div> }.into_any();
    }
    let children: Vec<AnyView> = content.children.iter().map(render_node_readonly).collect();
    // `.deck-frame__content` is pointer-events:none (presentation.css):
    // delegated live-app blocks (kanban buttons, mermaid click targets)
    // must not intercept frame selection/drag — blocks are interacted
    // with by entering the frame editor, same model as text. The
    // comment button and resize handles are siblings of this wrapper
    // and stay clickable.
    view! { <div class="deck-frame__content">{children}</div> }.into_any()
}

/// Render one slide's frames (sorted by `z`) as absolutely-positioned
/// `.deck-frame` divs inside a `.deck-canvas` themed root. Shared by
/// the interactive active-slide canvas and the slide-strip thumbnails
/// so both stay pixel-identical (the thumbnail is just the same
/// markup shrunk with `transform: scale()`, per
/// `style/presentation.css`'s `.deck-slide-thumb__scaler` comment).
pub(crate) fn render_deck_canvas(slide: &DeckSlide, theme: &str) -> AnyView {
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
pub(crate) fn ensure_presentation_css() {
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
    /// Frame block_id the user wants to comment on — fired by the
    /// per-frame comment button below, never gated on `readonly`
    /// (comment permission is independent of edit permission, same
    /// as the editor's and spreadsheet's own comment affordances).
    /// The page owns deciding whether this reopens an existing
    /// thread on `block_id` or starts a new one (Task 12).
    on_request_frame_comment: Callback<String>,
    /// block_ids that currently have an open comment thread, derived
    /// by the page from its `list_threads` fetch (Task 12) — drives
    /// the comment button's active/badge styling per frame below.
    frame_threads: ReadSignal<Vec<String>>,
    /// Reports the OPEN frame editor's own `EditorState` (frame-local
    /// document + real caret) up to the page on every inner
    /// `on_state_change`, and `None` when the editor closes. The
    /// page's slash/at-menu pipeline computes trigger ranges and
    /// dispatches insert commands in the coordinates of whichever
    /// state it reads — for a presentation the page-level
    /// `editor_state` is the deck-shaped doc, whose positions mean
    /// nothing to the frame editor that actually receives the
    /// commands (via `toolbar_command` forwarding). This channel lets
    /// the page target the editor that owns the caret.
    on_frame_editor_state: Callback<Option<EditorState>>,
    /// Shared page-level toolbar-command channel (Task 11 review,
    /// Finding 3) — the same `toolbar_command`/`set_toolbar_command`
    /// pair `document.rs` already threads into `SpreadsheetView`. The
    /// page's `<Toolbar>` writes into it; DeckView forwards it
    /// straight through as the currently-open frame editor's own
    /// `command_signal` (see `frame_body` below) — nothing else reads
    /// it while no frame is being edited, since no `EditorComponent`
    /// is mounted to consume it.
    toolbar_command: ReadSignal<Option<ToolbarCommand>>,
    set_toolbar_command: WriteSignal<Option<ToolbarCommand>>,
) -> impl IntoView {
    ensure_presentation_css();

    let deck = RwSignal::new(Deck {
        theme: crate::presentation::model::DEFAULT_THEME.to_string(),
        slide_size: "16:9".to_string(),
        slides: Vec::new(),
    });
    let (active_slide, set_active_slide) = signal(0usize);
    let (selected_frame, set_selected_frame) = signal::<Option<String>>(None);
    // Task 11 — the frame currently mounting a scoped `EditorComponent`
    // for in-place text editing, `None` when every frame renders
    // read-only. At most one frame can ever match, since `block_id`s
    // are unique across the whole deck (`generate_block_id()`), so no
    // path can mount two editors at once. Entry points (double-click,
    // Enter on a selected frame) both no-op under `readonly`. Exit
    // points: Escape, a pointerdown outside the editing frame (the
    // window-level listener below), a slide switch, or the frame
    // itself vanishing from a remote update (the resync Effect).
    let (editing_frame, set_editing_frame) = signal::<Option<String>>(None);

    // Unconditionally close the embedded frame editor (Escape, an
    // outside pointerdown, or the edited frame vanishing under a
    // remote delete — every case where we already *know* it should
    // close, not just suspect it). Also drops any toolbar command that
    // arrived for this editor but hasn't been consumed yet (Task 11
    // review, Finding 3): a stale `Some` left in `toolbar_command`
    // here would otherwise get picked up by whichever frame editor
    // opens next, applying a Bold/Italic/etc. the user aimed at a
    // frame that isn't open anymore.
    let close_frame_editor = move || {
        set_editing_frame.set(None);
        set_toolbar_command.set(None);
        // The page's menu pipeline must stop targeting the (now
        // unmounted) frame editor's coordinates immediately.
        on_frame_editor_state.run(None);
    };

    // Task 11 review, Finding 2 — every slide-strip mutation that can
    // change which slide is active calls this *after* applying its own
    // mutation and settling `active_slide`, instead of unconditionally
    // closing the editor the way `select_slide` used to. See
    // `editing_frame_still_visible`'s doc comment for why "did the
    // active slide change" isn't the right question to ask.
    let close_frame_editor_if_hidden = move || {
        let Some(id) = editing_frame.get_untracked() else { return };
        let idx = active_slide.get_untracked();
        let still_visible = deck.with_untracked(|d| editing_frame_still_visible(d, idx, Some(&id)));
        if !still_visible {
            close_frame_editor();
        }
    };

    // Feedback-loop guard: set immediately before persist() emits its
    // editor_state change, cleared by the very next run of the doc→model
    // resync Effect below. Mirrors `spreadsheet_view.rs:1357`/`:2363`.
    let (persist_origin, set_persist_origin) = signal(false);
    // Canvas image resolution refresh channel — see `DeckImgRefresh`.
    let (img_refresh, set_img_refresh) = signal(0u32);
    provide_context(DeckImgRefresh(img_refresh, set_img_refresh));

    // The blob-URL resolver is normally installed by `EditorComponent`
    // and CLEARED (resolver + cache) by its unmount cleanup
    // (`clear_resolver_if`). On a document page the editor lives as
    // long as the page, so that's invisible — but here the frame
    // editor unmounts on every Escape, leaving the CANVAS render with
    // no resolver: every blob image went permanently `src`-less after
    // the first edit session (frame-blocks launch bug). Re-install a
    // deck-scoped resolver whenever no frame editor is open; while one
    // IS open, its own (identical-shape) install wins harmlessly. The
    // Effect lives in DeckView's stable scope, so it runs AFTER the
    // closing editor's cleanup in the same tick ordering.
    {
        let doc_id_for_resolver = doc_id.clone();
        Effect::new(move |_| {
            if editing_frame.get().is_none() {
                let doc_id = doc_id_for_resolver.clone();
                let resolver: crate::editor::image_bridge::Resolver = std::rc::Rc::new(
                    move |blob_id: String,
                          key: String,
                          on_ready: Box<dyn FnOnce(Option<String>)>| {
                        let doc_id = doc_id.clone();
                        leptos::task::spawn_local(async move {
                            let url = crate::api::blobs::request_download_url(
                                &doc_id, &blob_id, &key,
                            )
                            .await
                            .ok();
                            on_ready(url);
                        });
                    },
                );
                crate::editor::image_bridge::set_resolver(Some(resolver));
            }
        });
    }
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

    // Flushes a pending arrow-nudge (Task 10 review, Finding 2). Arrow
    // nudges mutate `deck` directly on every keydown for instant
    // feedback and only persist on the matching keyup — coalescing a
    // held-key's repeat into one write — but a keyup can fail to land
    // on this handler at all (focus loss, a slide switch mid-nudge,
    // component teardown), which would leave `nudge_dirty` true with
    // no write ever scheduled. Every place a nudge gesture can be
    // interrupted before its own keyup calls this instead of
    // duplicating the coalescing check inline.
    //
    // The doc is captured *synchronously* here (not by deferring the
    // whole `persist()` call, which would re-read `deck` — and by
    // then, when called from the resync Effect below, `deck` has
    // already been overwritten with the remote update) — only the
    // actual `on_state_change`/`on_change` send is deferred via
    // `a11y::defer`, for the same synchronous-Effect-reentrancy reason
    // the bootstrap-persist path defers below: `on_state_change` flows
    // back into `editor_state`, which would otherwise re-enter the
    // resync Effect while it's still on the stack.
    let flush_nudge = move || {
        if !nudge_dirty.get_untracked() {
            return;
        }
        set_nudge_dirty.set(false);
        let doc = deck.with_untracked(deck_to_doc);
        crate::a11y::defer(move || {
            set_persist_origin.set(true);
            on_state_change.run(EditorState::create_default(doc));
            on_change.run(());
        });
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

        // A genuine remote update is about to overwrite `deck` below.
        // If an arrow-nudge is mid-gesture (already mutated into
        // `deck` locally, not yet persisted — see `nudge_dirty`'s doc
        // comment on `flush_nudge`), capture the nudged frame's *rect*
        // now, before it's gone, so it can be carried onto the merged
        // deck below (see `carry_nudge`'s doc comment for why calling
        // `flush_nudge` here — persisting the pre-merge snapshot — was
        // unsafe: it raced `ws_client`'s rebase and could delete
        // peer-added slides / revert peer edits wholesale, and it
        // self-defeated the nudge once the merge adopted the remote
        // rect for the same frame).
        let had_pending_nudge = nudge_dirty.get_untracked();
        let pending_nudge = if had_pending_nudge {
            selected_frame.get_untracked().and_then(|id| {
                deck.with_untracked(|d| {
                    d.slides
                        .iter()
                        .flat_map(|s| s.frames.iter())
                        .find(|f| f.block_id == id)
                        .map(|f| (id.clone(), f.rect))
                })
            })
        } else {
            None
        };

        let mut remote_deck = deck_from_doc(&state.doc);
        let bootstrap = bootstrap_blank_slide(remote_deck.slides.is_empty(), readonly);
        let should_persist = bootstrap.is_some();
        if let Some(slide) = bootstrap {
            remote_deck.slides.push(slide);
        }
        // Task 11 — a frame text editor may be open, mid-keystroke,
        // when a genuinely remote update lands. `merge_remote_deck`
        // takes every field of `remote_deck` except the *content* of
        // the frame named by `editing_frame` (if any), which stays
        // `local` so the incoming update can never clobber
        // unpersisted keystrokes. `get_untracked` deliberately: this
        // Effect must re-run when a new `editor_state` arrives, not
        // merely because `editing_frame` toggled.
        let editing = editing_frame.get_untracked();
        let mut merged = deck.with_untracked(|local| merge_remote_deck(local, remote_deck, editing.as_deref()));
        // If the frame being edited was deleted by this same remote
        // update (a concurrent peer's delete raced the open editor),
        // `merged` no longer has it — drop the dangling `editing_frame`
        // so no row can try to keep mounting an editor for a frame
        // that no longer exists.
        if let Some(id) = &editing {
            let still_exists = merged.slides.iter().any(|s| s.frames.iter().any(|f| &f.block_id == id));
            if !still_exists {
                close_frame_editor();
            }
        }
        // Carry the pending nudge onto the now-merged deck (Finding
        // I1) — never onto the pre-merge local/remote decks
        // individually. `carry_nudge` no-ops safely if a concurrent
        // remote delete raced the nudge.
        if let Some((id, rect)) = pending_nudge {
            carry_nudge(&mut merged, &id, rect);
        }
        if had_pending_nudge {
            set_nudge_dirty.set(false);
        }
        deck.set(merged);
        if should_persist || had_pending_nudge {
            crate::a11y::defer(persist);
        }
    });

    // ─── Focus the embedded frame editor on mount (Task 11 review,
    // Finding 2 / I2) ────────────────────────────────────────
    //
    // Mounting the scoped `EditorComponent` (in `frame_body`, below)
    // does not by itself move DOM focus into its contenteditable —
    // without this, focus stays on whatever last had it (typically
    // the canvas div itself), so the very next keydown targets the
    // canvas directly. That skips the `.deck-frame` div's own
    // `on:keydown` listener entirely (it only intercepts events that
    // bubble *through* the frame's DOM subtree) and lands on
    // `on_canvas_keydown` instead, which would delete the frame on
    // Backspace or eat every other keystroke with no visible effect.
    // Deferred by a microtask (`a11y::defer`) so the `frame_body`
    // branch has actually mounted `.editor-content` by the time this
    // queries for it — same pattern as `spreadsheet_view.rs`'s
    // cell-input-focus Effect and `ask_dialog.rs`'s input autofocus.
    Effect::new(move |_| {
        let Some(id) = editing_frame.get() else { return };
        crate::a11y::defer(move || {
            let Some(canvas) = canvas_ref.get_untracked() else { return };
            let selector = format!("{} .editor-content", frame_selector(&id));
            if let Ok(Some(el)) = canvas.query_selector(&selector) {
                if let Ok(html_el) = el.dyn_into::<web_sys::HtmlElement>() {
                    let _ = html_el.focus();
                }
            }
        });
    });

    // Component teardown (navigating away, switching doc types, etc.)
    // is the fourth interruption point for a pending nudge (Task 10
    // review, Finding 2) — without this, closing the tab/route on a
    // frame mid-nudge would drop the last keydown's worth of movement
    // silently, the same way a blur or slide-switch would.
    on_cleanup(move || flush_nudge());

    // ─── Frame-editor outside-click close (Task 11) ─────────
    //
    // Hazard: the canvas's own keydown/paste handlers (below) already
    // stop-propagate while a frame is being edited, so Delete/Backspace
    // typed *inside* the editor can never reach `on_canvas_keydown` and
    // delete the frame out from under the caret. That doesn't cover a
    // pointerdown *outside* the editing frame's DOM, though — the
    // canvas has no single listener that sees every possible outside
    // target (toolbar, theme picker, slide strip, blank canvas). A
    // window-level listener does, mirroring the slide-strip drag's use
    // of `document.element_from_point` elsewhere in this file for the
    // same "read real DOM geometry, don't trust `ev.target()` alone"
    // reason — here it's `.closest()` walking from the real click
    // target up to the nearest `[data-deck-frame-block-id]`, which is
    // `None` (blank canvas / other chrome) or a *different* frame's id
    // whenever the click wasn't inside the editing frame's own
    // subtree. `get_untracked`: this is a native DOM callback, not a
    // reactive computation — it must read the *current* value on each
    // real pointerdown, not re-run because `editing_frame` changed.
    {
        let handle = window_event_listener_untyped("pointerdown", move |ev: web_sys::Event| {
            let Some(editing_id) = editing_frame.get_untracked() else { return };
            let target = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
            // Page-level editor overlays are logically INSIDE the
            // editing session even though they mount outside the frame's
            // DOM subtree: the slash/at-menu (document.rs) dispatches
            // insert commands INTO this frame's editor — closing the
            // editor on the menu click would unmount the command's
            // target mid-dispatch (the frame-blocks launch bug: every
            // slash insert silently no-opped).
            let in_editor_overlay = target
                .as_ref()
                .and_then(|el| el.closest(".at-menu").ok().flatten())
                .is_some();
            if in_editor_overlay {
                return;
            }
            let inside = target
                .and_then(|el| el.closest(&format!("[{FRAME_BLOCK_ID_ATTR}]")).ok().flatten())
                .and_then(|el| el.get_attribute(FRAME_BLOCK_ID_ATTR))
                .is_some_and(|id| id == editing_id);
            if !inside {
                close_frame_editor();
            }
        });
        on_cleanup(move || handle.remove());
    }

    // ─── Slide-strip mutation handlers ─────────────────────

    let add_with_preset = move |preset_id: &'static str| {
        set_picker_open.set(false);
        let Some(preset) = LAYOUT_PRESETS.iter().find(|p| p.id == preset_id) else { return };
        let after = active_slide.get_untracked();
        deck.update(|d| {
            let idx = add_slide(d, after, preset);
            set_active_slide.set(idx);
        });
        // Task 11 review, Finding 2 — this always selects the freshly
        // inserted slide, so any editor open on the *previous* active
        // slide's frame just scrolled out of view.
        close_frame_editor_if_hidden();
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
        // Task 11 review, Finding 2 — same reasoning as `add_with_preset`:
        // this always selects the duplicate, a different slide.
        close_frame_editor_if_hidden();
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
        // Task 11 review, Finding 2 — unlike add/duplicate, `active_slide`
        // often does NOT change here (it's only clamped when it goes out
        // of bounds), so a blanket close would be wrong; but deleting a
        // slide *before* the active one shifts every later slide's index
        // down by one, swapping in a different slide at the same index
        // without moving `active_slide` at all. `close_frame_editor_if_hidden`
        // re-derives visibility from the live deck rather than reasoning
        // about whether `active_slide` itself moved, so it catches that
        // case too — see `editing_frame_still_visible`'s doc comment.
        close_frame_editor_if_hidden();
        persist();
    };

    let select_slide = move |block_id: &str| {
        // A nudge in progress is scoped to the frame selected on the
        // *current* active slide — flush it before switching slides
        // out from under it (Task 10 review, Finding 2), or the
        // keyup that would have persisted it never fires (it lands on
        // the newly-active slide's canvas, which has nothing to do
        // with the pending nudge).
        flush_nudge();
        if let Some(idx) = deck.with_untracked(|d| find_slide_index(&d.slides, block_id)) {
            set_active_slide.set(idx);
        }
        // Task 11 review, Finding 2 — checked *after* resolving the
        // target slide (not unconditionally before, the way this used
        // to work): re-clicking the already-active slide's own thumbnail
        // doesn't actually change anything and must leave an open editor
        // alone; switching to a genuinely different slide closes it,
        // since the edited frame's row just unmounted.
        close_frame_editor_if_hidden();
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
        // `snap` (Move) and `snap_resize` (Resize) are deliberately
        // different functions: `snap` may snap either edge or the
        // center and translate the whole rect, which is only correct
        // when size is fixed. A resize must never do that — see
        // `snap_resize`'s doc comment (Task 10 review, Finding 1).
        let (snapped, guides) = match state.kind {
            DragKind::Move => geometry::snap(dragged, &others, SNAP_THRESHOLD),
            DragKind::Resize(corner) => geometry::snap_resize(dragged, corner, &others, SNAP_THRESHOLD),
        };
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
    // for the "frame selected, not editing" column. The "editing"
    // column's own key handling lives on the `.deck-frame` div's own
    // `on:keydown` (below, "Hazard 1"), which stops propagation before
    // an in-editor keystroke can reach this handler — this guard is
    // defense in depth for the case that mechanism doesn't cover: a
    // keydown whose *target* is the canvas itself (or anything else
    // outside the editing frame's DOM subtree) never touches the frame
    // div's listener at all, so it would otherwise still reach here.
    // Escape is the one key the editing column needs, and it's handled
    // entirely inside the frame's own listener — nothing here.
    let on_canvas_keydown = move |ev: web_sys::KeyboardEvent| {
        if readonly {
            return;
        }
        if editing_frame.get_untracked().is_some() {
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
            // Enter on a selected frame opens the embedded editor —
            // this handler itself stops firing for further keydowns
            // once `editing_frame` is set (see the "Scoped while
            // editing" comment on the frame render below), so a held
            // Enter can't re-trigger this arm mid-edit.
            "Enter" => {
                if let Some(id) = selected_frame.get_untracked() {
                    ev.prevent_default();
                    set_editing_frame.set(Some(id));
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
                // Only claim the keypress when there's actually
                // something to cycle to — a slide with zero content
                // frames (Task 10 review, Finding 3) must let Tab fall
                // through to normal browser focus traversal instead of
                // trapping keyboard users on the canvas.
                if next.is_some() {
                    ev.prevent_default();
                    set_selected_frame.set(next);
                }
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
    // write for the whole nudge streak happens in the common case —
    // `flush_nudge` also covers the cases where this keyup never
    // fires (see its doc comment).
    let on_canvas_keyup = move |ev: web_sys::KeyboardEvent| {
        if matches!(ev.key().as_str(), "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight") {
            flush_nudge();
        }
    };

    // Paste (native ClipboardEvent, same approach as
    // `spreadsheet_view.rs`'s `on_paste`): always creates a new
    // centered frame from the clipboard's plain text, regardless of
    // whether a frame happens to be selected — per the keymap matrix,
    // "with no frame selected, paste on the canvas also creates a new
    // centered frame from the clipboard content."
    //
    // Defense in depth, same reasoning as `on_canvas_keydown` above:
    // the `.deck-frame` div's own `on:paste` (Hazard 1) already stops
    // propagation for a paste that bubbles up through the editing
    // frame's DOM subtree, but a paste whose target is the canvas
    // itself would skip that listener entirely and land here, spawning
    // a spurious extra frame while the user is pasting into the open
    // editor.
    let on_canvas_paste = move |ev: web_sys::Event| {
        if readonly {
            return;
        }
        if editing_frame.get_untracked().is_some() {
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

    let doc_id_for_present = doc_id.clone();

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
                    // Minor M2: a cancel (e.g. the browser reclaiming
                    // the gesture for a system action) still leaves
                    // `deck`'s slide order live-reordered by every
                    // `pointermove` this gesture already processed —
                    // dropping the drag state here without persisting
                    // (the old behavior) left the local order
                    // permanently diverged from the persisted doc.
                    // `pointerup`'s handling is the model: persist
                    // whatever order the gesture landed on.
                    if dragging_block_id.get_untracked().is_some() {
                        set_dragging_block_id.set(None);
                        persist();
                    }
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
                                            // Stop the press BEFORE it reaches the thumb's
                                            // drag handler. Without this the thumb's
                                            // `on:pointerdown` runs and calls
                                            // `set_pointer_capture` on itself, which
                                            // retargets the follow-up `click` to the thumb
                                            // — so this button's `on:click` never fires and
                                            // the button looks dead. `stop_propagation` on
                                            // `click` alone is too late: pointerdown has
                                            // already captured. Same pairing the frame
                                            // comment button uses below.
                                            on:pointerdown=|ev: web_sys::PointerEvent| ev.stop_propagation()
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
                                            // See the duplicate button above: pointerdown
                                            // must be stopped or the thumb captures the
                                            // pointer and swallows this button's click.
                                            on:pointerdown=|ev: web_sys::PointerEvent| ev.stop_propagation()
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
                    // Task 11 hazard 2: this div — and, critically, the `<For>`
                    // of frames inside it — used to live inside a single
                    // `{move || {...}}` closure that re-ran (tearing down and
                    // rebuilding *every* frame's DOM from scratch) on any
                    // change to `deck`, `frame_drag`, `drag_preview`, or
                    // `drag_guides`. That's fine for read-only content, but
                    // fatal for a mounted `EditorComponent`: any unrelated
                    // deck mutation (a remote peer nudging a *different*
                    // frame, a local drag anywhere on the slide) would
                    // remount the editor mid-keystroke, dropping focus, the
                    // caret, and the undo stack. Hoisting the canvas div and
                    // the `<For>` out of that closure — reactive attributes
                    // (`class=`, `node_ref`) instead of a reactive subtree —
                    // is what makes `<For>`'s keyed reconciliation apply:
                    // frames whose `block_id` persists keep their DOM/mounted
                    // component across a `deck` update; only frames that are
                    // actually inserted/removed/reordered get torn down or
                    // moved. The guides overlay and the notes drawer stay as
                    // their own independent reactive fragments (siblings of
                    // the `<For>`, not wrapping it) for the same reason.
                    <div
                        class=move || format!("deck-canvas {}", theme_class(&deck.with(|d| d.theme.clone())))
                        tabindex="0"
                        node_ref=canvas_ref
                        on:keydown=on_canvas_keydown
                        on:keyup=on_canvas_keyup
                        on:blur=move |_| flush_nudge()
                        on:paste=on_canvas_paste
                        on:pointermove=on_canvas_pointermove
                        on:pointerup=on_canvas_pointerup
                        on:pointercancel=on_canvas_pointercancel
                    >
                        <For
                            each=move || {
                                // `role=notes` frames never render on the canvas
                                // itself (design doc, "Canvas keymap matrix") —
                                // they surface only in the collapsed drawer
                                // below. Content frames sort by `z` for paint
                                // order (later z on top); `<For>` reorders
                                // existing rows' DOM position to match rather
                                // than rebuilding them when only `z` changes.
                                let idx = active_slide.get().min(deck.with(|d| d.slides.len().saturating_sub(1)));
                                deck.with(|d| {
                                    d.slides
                                        .get(idx)
                                        .map(|s| {
                                            let mut v: Vec<DeckFrame> =
                                                s.frames.iter().filter(|f| f.role == FrameRole::Content).cloned().collect();
                                            v.sort_by_key(|f| f.z);
                                            v
                                        })
                                        .unwrap_or_default()
                                })
                            }
                            key=|f: &DeckFrame| f.block_id.clone()
                            children=move |frame: DeckFrame| {
                                let block_id = frame.block_id.clone();
                                // `children` is an `Fn` closure invoked once per
                                // row (not consumed after the first row), so
                                // `doc_id` — owned, not `Copy` — has to be
                                // re-cloned from the outer capture on every
                                // invocation rather than moved once; the clone
                                // is what `frame_body` below then moves into its
                                // own `move ||` closure.
                                let doc_id = doc_id.clone();
                                let is_selected = {
                                    let block_id = block_id.clone();
                                    move || selected_frame.get().as_deref() == Some(block_id.as_str())
                                };
                                // A second, independently-reactive closure over the
                                // same block_id (not a plain bool computed once):
                                // Escape / Tab-Shift-Tab / a plain click-to-select
                                // change `selected_frame` without touching `deck` —
                                // the handles' visibility tracks selection via
                                // `<Show>`'s own reactivity.
                                let is_selected_for_handles = {
                                    let block_id = block_id.clone();
                                    move || selected_frame.get().as_deref() == Some(block_id.as_str())
                                };
                                let is_editing = {
                                    let block_id = block_id.clone();
                                    move || editing_frame.get().as_deref() == Some(block_id.as_str())
                                };
                                let has_thread = {
                                    let block_id = block_id.clone();
                                    move || frame_threads.get().iter().any(|t| t == &block_id)
                                };
                                // Reactive per-row geometry: the live rect from
                                // `deck` (tracking this frame's own remote/local
                                // moves even while some *other* row is mid-drag or
                                // mid-edit), overridden by the transient
                                // drag-preview only while *this* frame is the one
                                // being dragged. `fallback` only matters in the
                                // vanishingly unlikely window where this row's
                                // block_id has just been removed from `deck` but
                                // `<For>` hasn't unmounted it yet.
                                let live_rect = {
                                    let block_id = block_id.clone();
                                    let fallback = frame.rect;
                                    move || -> Rect {
                                        let idx = active_slide.get();
                                        let current = deck
                                            .with(|d| {
                                                d.slides
                                                    .get(idx)
                                                    .and_then(|s| s.frames.iter().find(|f| f.block_id == block_id))
                                                    .map(|f| f.rect)
                                            })
                                            .unwrap_or(fallback);
                                        if frame_drag.get().as_ref().is_some_and(|s| s.block_id == block_id) {
                                            drag_preview.get().unwrap_or(current)
                                        } else {
                                            current
                                        }
                                    }
                                };
                                let left = { let live_rect = live_rect.clone(); move || format!("{:.2}%", live_rect().x * 100.0) };
                                let top = { let live_rect = live_rect.clone(); move || format!("{:.2}%", live_rect().y * 100.0) };
                                let width = { let live_rect = live_rect.clone(); move || format!("{:.2}%", live_rect().w * 100.0) };
                                let height = { let live_rect = live_rect.clone(); move || format!("{:.2}%", live_rect().h * 100.0) };

                                // Task 11: either the read-only render (tracks
                                // `deck` reactively, same as before) or a scoped
                                // `EditorComponent` mounted over a synthetic Doc
                                // wrapping just this frame's content. The editing
                                // branch reads `deck` *untracked* — on purpose:
                                // once mounted, this closure must only re-run when
                                // `editing_frame` itself changes (entering or
                                // leaving edit mode), never because `deck` changed
                                // for any other reason, or the editor would remount
                                // mid-keystroke (hazard 2, see the comment above
                                // this `<For>`). Every keystroke already reaches
                                // `deck` via `on_state_change` below without this
                                // closure re-running at all.
                                let frame_body = {
                                    let block_id = block_id.clone();
                                    let fallback_content = frame.content.clone();
                                    move || -> Vec<AnyView> {
                                        if editing_frame.get().as_deref() == Some(block_id.as_str()) {
                                            let idx = active_slide.get_untracked();
                                            let current_content = deck
                                                .with_untracked(|d| {
                                                    d.slides
                                                        .get(idx)
                                                        .and_then(|s| s.frames.iter().find(|f| f.block_id == block_id))
                                                        .map(|f| f.content.clone())
                                                })
                                                .unwrap_or_else(|| fallback_content.clone());
                                            let inner_doc = Node::element_with_content(NodeType::Doc, current_content);
                                            let (inner_remote, _) = signal::<Option<EditorState>>(None);
                                            let on_state_change_block_id = block_id.clone();
                                            vec![
                                                view! {
                                                    <EditorComponent props=EditorProps {
                                                        initial_content: Some(doc_to_ydoc_bytes(&inner_doc)),
                                                        // The outer `persist()` (below) handles transport —
                                                        // the inner editor's own `on_change` (yrs-bytes-only,
                                                        // meant for a standalone doc's REST fallback) has
                                                        // nothing to do here.
                                                        on_change: Callback::new(|_: Vec<u8>| {}),
                                                        on_state_change: Callback::new(move |st: EditorState| {
                                                            // Frame-local state (real caret) up to the
                                                            // page FIRST, so the slash/at-menu trigger
                                                            // Effects see inner coordinates before the
                                                            // deck-shaped `editor_state` write below
                                                            // re-runs them.
                                                            on_frame_editor_state.run(Some(st.clone()));
                                                            let content = match &st.doc {
                                                                Node::Element { content, .. } => content.clone(),
                                                                _ => return,
                                                            };
                                                            deck.update(|d| {
                                                                replace_frame_content(d, &on_state_change_block_id, content);
                                                            });
                                                            persist();
                                                        }),
                                                        // Task 11 review, Finding 3 — the page-level
                                                        // `toolbar_command` signal, forwarded straight
                                                        // through. There's nothing else to gate this on:
                                                        // this `EditorComponent` only exists while
                                                        // `editing_frame == Some(block_id)` in the first
                                                        // place (this whole branch is the mount), so the
                                                        // signal only ever reaches a live consumer while
                                                        // this frame is the one being edited — the same
                                                        // way the page's own top-level `<EditorComponent>`
                                                        // mount (document.rs) wires `toolbar_command`
                                                        // straight into `command_signal` with no extra
                                                        // gating.
                                                        command_signal: toolbar_command,
                                                        // Always `None` — remote merge for this frame is
                                                        // handled at the deck level by `merge_remote_deck`
                                                        // in the resync Effect, not by feeding the inner
                                                        // editor its own remote-state stream.
                                                        remote_state: inner_remote,
                                                        doc_id: doc_id.clone(),
                                                        on_scroll: None,
                                                        on_mapping: None,
                                                        // Frame comments come from the frame chrome's own
                                                        // comment button (Task 12), not the inner editor's
                                                        // right-click menu.
                                                        on_request_comment: None,
                                                        readonly: false,
                                                    } />
                                                }
                                                .into_any(),
                                            ]
                                        } else {
                                            let idx = active_slide.get();
                                            let content = deck
                                                .with(|d| {
                                                    d.slides
                                                        .get(idx)
                                                        .and_then(|s| s.frames.iter().find(|f| f.block_id == block_id))
                                                        .map(|f| f.content.clone())
                                                })
                                                .unwrap_or_else(|| fallback_content.clone());
                                            vec![render_frame_content(&content)]
                                        }
                                    }
                                };

                                let block_id_pointerdown = block_id.clone();
                                let block_id_comment = block_id.clone();
                                let block_id_dblclick = block_id.clone();
                                let block_id_keydown = block_id.clone();
                                let block_id_paste = block_id.clone();
                                let block_id_data_attr = block_id.clone();
                                view! {
                                    <div
                                        class="deck-frame"
                                        class:deck-frame--selected=is_selected
                                        class:deck-frame--editing=is_editing
                                        class:deck-frame--readonly=readonly
                                        style:left=left
                                        style:top=top
                                        style:width=width
                                        style:height=height
                                        // NOTE: plain `data-x=` form, not `attr:data-x=` —
                                        // the `attr:` prefix on a native element silently
                                        // renders nothing in Leptos 0.7 (P1 launch bug:
                                        // every frame lost this attribute, so editing
                                        // closed instantly via the outside-click handler).
                                        data-deck-frame-block-id=block_id_data_attr
                                        // Hazard 1 (keydown): while this frame is
                                        // being edited, a keydown that bubbles up
                                        // from the embedded editor's contenteditable
                                        // — Delete/Backspace, arrows, Tab — must
                                        // never reach `on_canvas_keydown` above,
                                        // which would delete/nudge/reselect the
                                        // very frame being typed in. Stopping
                                        // propagation *here*, at the frame div that
                                        // sits between the editor and the canvas in
                                        // the DOM, is the mechanism; Escape is the
                                        // one key this handler acts on itself
                                        // (closing the editor) before stopping it.
                                        // The `is_none()`/no-op guard covers the case
                                        // where this listener fires from something
                                        // other than the editing frame (shouldn't
                                        // normally happen — nothing here is
                                        // focusable while read-only — but a stray
                                        // event should never accidentally eat a
                                        // canvas-level keydown for a frame that
                                        // isn't being edited).
                                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                                            if editing_frame.get_untracked().as_deref() != Some(block_id_keydown.as_str()) {
                                                return;
                                            }
                                            if ev.key() == "Escape" {
                                                ev.prevent_default();
                                                close_frame_editor();
                                            }
                                            ev.stop_propagation();
                                        }
                                        // Hazard 1 (paste): same reasoning as
                                        // keydown above — the embedded editor's own
                                        // paste handler already reads/consumes the
                                        // clipboard event; without stopping
                                        // propagation here it would go on to reach
                                        // `on_canvas_paste`, which creates a whole
                                        // new centered frame from the same
                                        // clipboard text.
                                        on:paste=move |ev: web_sys::Event| {
                                            if editing_frame.get_untracked().as_deref() != Some(block_id_paste.as_str()) {
                                                return;
                                            }
                                            ev.stop_propagation();
                                        }
                                        on:dblclick=move |ev: web_sys::MouseEvent| {
                                            if readonly {
                                                return;
                                            }
                                            // Task 11 review, Finding 1 — word-select
                                            // (double-click inside the open
                                            // contenteditable's own text, a normal
                                            // editing gesture) fires this same
                                            // `dblclick`. Without this guard it would
                                            // re-set `editing_frame` to the id it's
                                            // already set to, which the row's
                                            // `frame_body` closure can't tell apart
                                            // from a *fresh* open — it would rebuild
                                            // `inner_doc` from `deck` and remount a
                                            // brand-new `EditorComponent`, dropping
                                            // the very selection the double-click was
                                            // trying to make (plus focus and undo
                                            // history). Already-editing-this-frame is
                                            // a no-op here; the browser's own
                                            // word-select still happens untouched.
                                            if editing_frame.get_untracked().as_deref() == Some(block_id_dblclick.as_str()) {
                                                return;
                                            }
                                            ev.stop_propagation();
                                            set_selected_frame.set(Some(block_id_dblclick.clone()));
                                            set_editing_frame.set(Some(block_id_dblclick.clone()));
                                        }
                                        on:pointerdown=move |ev: web_sys::PointerEvent| {
                                            // A click *inside* the frame currently
                                            // being edited (to reposition the caret)
                                            // must not also start a Move-drag
                                            // gesture — `start_frame_drag` captures
                                            // the pointer, which would fight the
                                            // browser's own click-to-place-caret
                                            // handling inside the contenteditable.
                                            if editing_frame.get_untracked().as_deref() == Some(block_id_pointerdown.as_str()) {
                                                return;
                                            }
                                            start_frame_drag(block_id_pointerdown.clone(), DragKind::Move, ev);
                                        }
                                    >
                                        {frame_body}
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
                            }
                        />
                        {move || {
                            drag_guides
                                .get()
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
                                .collect::<Vec<_>>()
                        }}
                    </div>
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
                <button
                    class="deck-present-btn"
                    on:click=move |_| crate::nav_bridge::go(&format!("/d/{}/present", doc_id_for_present))
                >
                    {crate::t!("deck-present")}
                </button>
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

    #[test]
    fn visually_empty_detects_whitespace_and_nesting() {
        assert!(fragment_is_visually_empty(&Fragment::empty()));
        assert!(fragment_is_visually_empty(&Fragment::from(vec![Node::element(
            NodeType::Heading
        )])));
        assert!(fragment_is_visually_empty(&Fragment::from(vec![
            Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("   ")]),
            ),
        ])));
        assert!(!fragment_is_visually_empty(&Fragment::from(vec![
            Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("real text")]),
            ),
        ])));
        // Attr-driven blocks are content even with zero text children
        // (the frame-blocks launch bug: a mermaid "vanished" behind
        // the placeholder hint).
        for nt in [
            NodeType::Mermaid,
            NodeType::Image,
            NodeType::Calendar,
            NodeType::Kanban,
            NodeType::Embed,
            NodeType::HorizontalRule,
            NodeType::Table,
        ] {
            assert!(
                !fragment_is_visually_empty(&Fragment::from(vec![Node::element(nt)])),
                "{nt:?} must not count as visually empty"
            );
        }
    }

    #[test]
    fn embed_chip_prefers_title_then_url() {
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("url".to_string(), "https://example.com/x".to_string());
        assert_eq!(embed_chip_label(&attrs), "https://example.com/x");
        attrs.insert("title".to_string(), "  ".to_string());
        assert_eq!(embed_chip_label(&attrs), "https://example.com/x", "whitespace title ignored");
        attrs.insert("title".to_string(), "Demo Video".to_string());
        assert_eq!(embed_chip_label(&attrs), "Demo Video");
        assert!(!embed_chip_label(&std::collections::HashMap::new()).is_empty(), "fallback label");
    }

    #[test]
    fn placeholder_key_follows_first_node_type() {
        let heading = Fragment::from(vec![Node::element(NodeType::Heading)]);
        assert_eq!(placeholder_key_for(&heading), "deck-placeholder-heading");
        let para = Fragment::from(vec![Node::element(NodeType::Paragraph)]);
        assert_eq!(placeholder_key_for(&para), "deck-placeholder-body");
        assert_eq!(placeholder_key_for(&Fragment::empty()), "deck-placeholder-body");
    }

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
    fn threads_filter_to_slide_frames() {
        let deck = fixture_deck_two_slides();
        let slide0_ids: Vec<String> =
            deck.slides[0].frames.iter().map(|f| f.block_id.clone()).collect();
        let threads = vec![
            (slide0_ids[0].clone(), "t1".to_string()),
            (deck.slides[1].frames[0].block_id.clone(), "t2".to_string()),
            ("orphan".to_string(), "t3".to_string()),
        ];
        let visible = threads_for_slide(&deck, 0, &threads);
        assert_eq!(visible, vec!["t1".to_string()]);
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

    // ─── carry_nudge (resync-Effect Finding I1) ─────────────

    #[test]
    fn carry_nudge_applies_rect_when_frame_still_exists() {
        let mut deck = fixture_deck_one_slide();
        let frame_id = deck.slides[0].frames[0].block_id.clone();
        let new_rect = Rect::clamped(0.4, 0.5, 0.2, 0.2);

        let applied = carry_nudge(&mut deck, &frame_id, new_rect);

        assert!(applied);
        assert_eq!(deck.slides[0].frames[0].rect, new_rect);
    }

    #[test]
    fn carry_nudge_returns_false_when_frame_no_longer_exists() {
        // A concurrent remote delete raced the nudge — the frame named
        // by the captured id is gone from the merged deck. This must
        // be a safe no-op, not a panic or a silent frame re-creation.
        let mut deck = fixture_deck_one_slide();
        let untouched = deck.clone();
        let new_rect = Rect::clamped(0.4, 0.5, 0.2, 0.2);

        let applied = carry_nudge(&mut deck, "deleted-by-a-peer", new_rect);

        assert!(!applied);
        assert_eq!(deck, untouched);
    }

    #[test]
    fn carry_nudge_only_touches_the_named_frame() {
        // Two slides, each with one frame (`fixture_deck_two_slides`).
        // Nudging the first slide's frame must leave the second
        // slide's frame's rect untouched.
        let mut deck = fixture_deck_two_slides();
        let target_id = deck.slides[0].frames[0].block_id.clone();
        let other_rect_before = deck.slides[1].frames[0].rect;
        let new_rect = Rect::clamped(0.4, 0.5, 0.2, 0.2);

        let applied = carry_nudge(&mut deck, &target_id, new_rect);

        assert!(applied);
        assert_eq!(deck.slides[0].frames[0].rect, new_rect);
        assert_eq!(deck.slides[1].frames[0].rect, other_rect_before);
    }

    // ─── Frame-editor lifecycle (Task 11 review, Finding 2) ─
    //
    // `editing_frame_still_visible` is the pure decision `close_frame_editor_if_hidden`
    // (a `DeckView`-internal closure, not testable without the reactive
    // runtime) delegates to. These tests cover the decision function's
    // contract directly; the DOM-level wiring that calls it from
    // `add_with_preset`/`duplicate_at`/`delete_at`/`select_slide` — and
    // Finding 1's dblclick guard, and Finding 3's `command_signal`
    // forwarding — has no pure boundary to test against and is left to
    // manual/browser verification (`trunk build` + the existing
    // `frontend-doctor` scenarios cover presentation docs already).

    #[test]
    fn editing_frame_still_visible_true_when_nothing_is_being_edited() {
        let deck = fixture_deck_two_slides();
        assert!(editing_frame_still_visible(&deck, 0, None));
        // Even an out-of-range slide index is fine — there's nothing to
        // look up when `editing_id` is `None`.
        assert!(editing_frame_still_visible(&deck, 99, None));
    }

    #[test]
    fn editing_frame_still_visible_true_when_frame_is_on_the_active_slide() {
        let deck = fixture_deck_two_slides();
        let id = deck.slides[0].frames[0].block_id.clone();
        assert!(editing_frame_still_visible(&deck, 0, Some(&id)));
    }

    #[test]
    fn editing_frame_still_visible_false_when_frame_is_on_a_different_slide() {
        // The scenario behind `add_with_preset`/`duplicate_at`: the frame
        // being edited lived on slide 0, but the active slide moved to 1.
        let deck = fixture_deck_two_slides();
        let id = deck.slides[0].frames[0].block_id.clone();
        assert!(!editing_frame_still_visible(&deck, 1, Some(&id)));
    }

    #[test]
    fn editing_frame_still_visible_false_when_frame_no_longer_exists() {
        let deck = fixture_deck_two_slides();
        assert!(!editing_frame_still_visible(&deck, 0, Some("deleted-frame-id")));
    }

    #[test]
    fn editing_frame_still_visible_false_when_active_slide_index_is_out_of_range() {
        let deck = fixture_deck_two_slides();
        let id = deck.slides[0].frames[0].block_id.clone();
        assert!(!editing_frame_still_visible(&deck, 99, Some(&id)));
    }

    /// The `delete_at` scenario the review flagged directly: deleting an
    /// *earlier* slide shifts every later slide's index down by one
    /// without `active_slide` (a raw index, unadjusted unless it goes
    /// out of bounds) moving at all — so the same numeric index now
    /// refers to a different slide, one that doesn't have the
    /// previously-edited frame.
    #[test]
    fn editing_frame_still_visible_false_after_earlier_slide_shifts_active_index() {
        let mut deck = Deck {
            theme: DEFAULT_THEME.to_string(),
            slide_size: "16:9".to_string(),
            slides: vec![simple_slide(), simple_slide(), simple_slide()],
        };
        let edited_id = deck.slides[1].frames[0].block_id.clone();
        // Editing a frame on the slide at index 1 (active_slide == 1).
        assert!(editing_frame_still_visible(&deck, 1, Some(&edited_id)));

        // A concurrent delete removes slide 0. `delete_slide` only
        // clamps `active_slide` when it goes *out of bounds*
        // (`delete_at`'s own logic) — deleting an earlier slide doesn't
        // trigger that, so a caller that doesn't separately re-derive
        // visibility would leave `active_slide` sitting at 1, which now
        // names a *different* slide (the one that used to be at index 2).
        delete_slide(&mut deck, 0);
        assert_eq!(deck.slides.len(), 2);
        assert_ne!(
            deck.slides[1].frames[0].block_id, edited_id,
            "index 1 now refers to the slide that used to be at index 2, not the edited frame's slide"
        );
        assert!(
            !editing_frame_still_visible(&deck, 1, Some(&edited_id)),
            "the edited frame's slide shifted to index 0; index 1 no longer has it"
        );
        assert!(
            editing_frame_still_visible(&deck, 0, Some(&edited_id)),
            "the edited frame is still in the deck, just at a different index"
        );
    }
}
