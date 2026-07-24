// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Mentions spec §5 (Task 5) — refresh-on-open + the per-viewer
//! degradation overlay for `DocMention` chips.
//!
//! On document open, every `DocMention` in the doc is batch-resolved
//! once (deduping targets first — the resolve endpoint itself does
//! not dedupe). The result drives two independent things:
//!
//! 1. A **per-viewer** [`MentionState`] map, applied to the live DOM
//!    by [`apply_overlay_to_dom`] — missing/dangling chips get a
//!    state class + tooltip, live chips get their title/snippet text
//!    refreshed if stale. This is NEVER persisted: access differs per
//!    viewer, so what one person sees for a chip has no bearing on
//!    what another person should see. The DOM mutation is reapplied
//!    every time the editor re-renders (which rebuilds the chip spans
//!    from the model's cached attrs and would otherwise wipe it) —
//!    same flicker-tolerant precedent as `find_replace_bar`'s
//!    CSS-highlight re-apply.
//! 2. In **editable** sessions only, a silent cache write-back: when
//!    the fresh title/snippet differ from what's cached on the atom,
//!    dispatch a `history: skip` transaction so the CRDT's cached
//!    display text stops drifting further from the truth for every
//!    future viewer. Read-only viewers never write this back — they
//!    have no write authority, and the per-viewer overlay above
//!    already gives them the fresh text regardless.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::documents::MentionResolveResult;
use crate::components::toolbar::ToolbarCommand;
use crate::editor::model::{Node, NodeType};
use crate::editor::state::EditorState;

/// A `DocMention` found while walking the doc: its own render identity
/// (`node_block_id`, i.e. the atom's `blockId` — NOT the target it
/// points at) plus the `doc_id`/`target_block_id` it resolves against.
///
/// `mount`'s resolve Effect needs more per-node context than this
/// (the currently-cached `title`/`snippet`, to know whether a fresh
/// resolve is even worth writing back) and so walks via the richer,
/// private `scan_doc_mentions_full` instead of this type's own
/// `scan_doc_mentions` — hence the `#[allow(dead_code)]`: this is the
/// scan's public, independently-testable pure-walk contract, not
/// currently exercised by production code, only by the tests below.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionRef {
    pub node_block_id: String,
    pub doc_id: String,
    pub target_block_id: Option<String>,
}

/// Per-viewer mention state — NEVER persisted (spec §2): access
/// differs per viewer, so live/dangling/missing exists only in this
/// session, applied straight to the DOM and never written back to the
/// document model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionState {
    /// Resolved, and (for an anchor mention) the target block still
    /// exists. `snippet` may legitimately be empty — `block_found`,
    /// not snippet emptiness, is the liveness signal.
    Live { title: String, snippet: String },
    /// The target document resolved, but the anchored block inside it
    /// no longer exists.
    Dangling { title: String },
    /// The target document doesn't resolve at all — deleted, or the
    /// viewer can't access it. The resolve endpoint serializes both
    /// cases identically, so the client can't (and shouldn't try to)
    /// tell them apart.
    Missing,
}

/// `scan_doc_mentions_full`'s per-node result, carrying the atom's
/// currently-cached `title`/`snippet` alongside the identity/target
/// fields `MentionRef` exposes publicly — used internally to detect
/// whether a fresh resolve actually changed anything worth writing
/// back.
struct MentionNodeInfo {
    node_block_id: String,
    doc_id: String,
    target_block_id: Option<String>,
    title: String,
    snippet: String,
}

fn walk_full(node: &Node, out: &mut Vec<MentionNodeInfo>) {
    if let Node::Element {
        node_type,
        attrs,
        content,
        ..
    } = node
    {
        if *node_type == NodeType::DocMention {
            out.push(MentionNodeInfo {
                node_block_id: attrs.get("blockId").cloned().unwrap_or_default(),
                doc_id: attrs.get("doc_id").cloned().unwrap_or_default(),
                target_block_id: attrs
                    .get("target_block_id")
                    .cloned()
                    .filter(|s| !s.is_empty()),
                title: attrs.get("title").cloned().unwrap_or_default(),
                snippet: attrs.get("snippet").cloned().unwrap_or_default(),
            });
        }
        // DocMention is a leaf atom (empty content) — recursing into it
        // is a harmless no-op; every other element type may nest one
        // arbitrarily deep (list > item > paragraph > DocMention).
        for child in &content.children {
            walk_full(child, out);
        }
    }
}

fn scan_doc_mentions_full(doc: &Node) -> Vec<MentionNodeInfo> {
    let mut out = Vec::new();
    walk_full(doc, &mut out);
    out
}

/// Pure recursive walk — every `DocMention` in `doc`, at any depth,
/// with its own render identity and the target it resolves against.
/// Mirrors `document_outline::extract_outline`'s destructure of
/// `Node::Element { node_type: NodeType::X, attrs, .. }`, generalized
/// to recurse (headings can't nest; mentions can, inside lists/quotes/
/// table cells).
#[allow(dead_code)]
pub fn scan_doc_mentions(doc: &Node) -> Vec<MentionRef> {
    scan_doc_mentions_full(doc)
        .into_iter()
        .map(|info| MentionRef {
            node_block_id: info.node_block_id,
            doc_id: info.doc_id,
            target_block_id: info.target_block_id,
        })
        .collect()
}

/// Map one batch-resolve result to the per-viewer state its chip
/// should show. `has_target` is whether the mention carried a
/// `target_block_id` (i.e. is an anchor mention, not a plain document
/// mention) — only anchor mentions can go Dangling.
///
/// `status != "ok"` covers both "deleted" and "no access": the
/// backend serializes them identically on purpose, so the client must
/// never try to distinguish them (see `MentionResolveResult`'s own
/// doc comment).
fn mention_state_from_result(result: &MentionResolveResult, has_target: bool) -> MentionState {
    if result.status != "ok" {
        return MentionState::Missing;
    }
    let title = result.title.clone().unwrap_or_default();
    if has_target && !result.block_found.unwrap_or(false) {
        return MentionState::Dangling { title };
    }
    // blockFound is the liveness signal — an empty snippet with
    // block_found true (or a plain, non-anchor document mention) is
    // still Live.
    MentionState::Live {
        title,
        snippet: result.snippet.clone().unwrap_or_default(),
    }
}

/// DOM decoration: apply `states` to every rendered `DocMention` chip
/// currently in the editor. Elements whose `node_block_id` has no
/// entry yet (resolve still in flight, or scan found nothing) are
/// left exactly as `view.rs` rendered them from cached attrs.
fn apply_overlay_to_dom(states: &HashMap<String, MentionState>) {
    let Some(window) = web_sys::window() else { return };
    let Some(document) = window.document() else { return };
    let Ok(nodes) = document.query_selector_all("span.doc-mention[data-node-block-id]") else {
        return;
    };

    for i in 0..nodes.length() {
        let Some(raw) = nodes.item(i) else { continue };
        let Ok(el) = raw.dyn_into::<web_sys::HtmlElement>() else { continue };
        let Some(node_block_id) = el.get_attribute("data-node-block-id") else { continue };
        let Some(state) = states.get(&node_block_id) else { continue };

        let class_list = el.class_list();
        // Same glyph convention as view.rs's chip render: anchor
        // (within-doc target) gets ⚓, plain document mention gets 📄.
        let is_anchor = el.has_attribute("data-block-id-target");
        let icon = if is_anchor { "\u{2693} " } else { "\u{1F4C4} " };

        match state {
            MentionState::Missing => {
                let _ = class_list.add_1("doc-mention-missing");
                let _ = class_list.remove_1("doc-mention-dangling");
                let label = crate::t!("doc-mention-missing");
                let text = format!("{icon}{label}");
                if el.text_content().as_deref() != Some(text.as_str()) {
                    el.set_text_content(Some(&text));
                }
                let _ = el.set_attribute("title", &label);
            }
            MentionState::Dangling { title } => {
                let _ = class_list.remove_1("doc-mention-missing");
                let _ = class_list.add_1("doc-mention-dangling");
                let _ = el.set_attribute("title", &crate::t!("doc-block-link-missing"));
                // Spec §3's degradation matrix: a dangling anchor (the
                // target BLOCK is gone, but the document itself still
                // resolves) renders as a plain document mention — the
                // ⚓ glyph promised "this points at a specific block",
                // and that promise is broken, so force the 📄 glyph
                // here rather than `icon` (which is still ⚓, computed
                // from `data-block-id-target` being present). Uses the
                // state's freshly-resolved `title`, not the chip's
                // stale cached anchor snippet — the snippet described
                // a block that no longer exists.
                if !title.is_empty() {
                    let text = format!("\u{1F4C4} {title}");
                    if el.text_content().as_deref() != Some(text.as_str()) {
                        el.set_text_content(Some(&text));
                    }
                }
            }
            MentionState::Live { title, snippet } => {
                let _ = class_list.remove_1("doc-mention-missing");
                let _ = class_list.remove_1("doc-mention-dangling");
                let _ = el.remove_attribute("title");
                let label = if is_anchor && !snippet.is_empty() {
                    snippet.clone()
                } else if !title.is_empty() {
                    title.clone()
                } else {
                    el.get_attribute("data-url").unwrap_or_default()
                };
                if !label.is_empty() {
                    let text = format!("{icon}{label}");
                    if el.text_content().as_deref() != Some(text.as_str()) {
                        el.set_text_content(Some(&text));
                    }
                }
            }
        }
    }
}

/// Wire the refresh-on-open + degradation overlay into a document
/// page. Mount once per page (mirrors how `comment_highlights` is
/// wired in `pages/document.rs`), passing:
///
/// - `editor_state` — the page's live editor state.
/// - `current_id` — the active document's id, used only to detect
///   "a different document just opened" across in-app navigation
///   (the page component itself isn't remounted on `/d/:id` changes,
///   so `editor_state` alone can't tell "new doc" from "same doc,
///   another keystroke").
/// - `editable` — true iff the caller has live write authority right
///   now (not trashed, not view-only, not locked) — gates the
///   cache-refresh write-back only; the DOM overlay itself always
///   applies, editable or not.
/// - `on_command` — the page's standard `ToolbarCommand` dispatch
///   path (the same one every other outside-the-editor mutation in
///   `pages/document.rs` uses), carrying `UpdateDocMentionAttrs`.
pub fn mount(
    editor_state: ReadSignal<Option<EditorState>>,
    current_id: ReadSignal<String>,
    editable: Signal<bool>,
    on_command: Callback<ToolbarCommand>,
) {
    let mention_states: RwSignal<HashMap<String, MentionState>> = RwSignal::new(HashMap::new());

    // Resolve once per doc-open. `editor_state` changes on every
    // keystroke, so the dependency here is deliberately cheap: guard
    // on "have we already resolved for the currently-open doc id"
    // before doing any real work — same idiom as this file's own
    // `spreadsheet_initialized` guard a few hundred lines up.
    let resolved_for_id: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else { return };
        let id = current_id.get();
        if id.is_empty() || *resolved_for_id.borrow() == id {
            return;
        }
        *resolved_for_id.borrow_mut() = id;

        // Eagerly drop the previous doc's states as soon as we know a
        // different doc has opened — before the scan, before the
        // async resolve. Otherwise a doc-with-mentions switch would
        // leave the OLD doc's `mention_states` (and thus its DOM
        // overlay) showing until the new doc's resolve round-trip
        // returns, while the empty-doc case cleared immediately. Both
        // branches now clear at the same point, symmetrically.
        mention_states.set(HashMap::new());

        let infos = scan_doc_mentions_full(&state.doc);
        if infos.is_empty() {
            return;
        }

        // Dedupe targets first — carry-over note: the resolve
        // endpoint does not dedupe server-side, so a doc with the
        // same target mentioned N times would otherwise cost N
        // identical round trips in one request.
        let mut targets: Vec<(String, Option<String>)> = Vec::new();
        let mut index_of: HashMap<(String, Option<String>), usize> = HashMap::new();
        for info in &infos {
            let key = (info.doc_id.clone(), info.target_block_id.clone());
            index_of.entry(key.clone()).or_insert_with(|| {
                targets.push(key);
                targets.len() - 1
            });
        }

        leptos::task::spawn_local(async move {
            let Ok(results) = crate::api::documents::resolve_mentions(&targets).await else {
                return; // network error: leave whatever view.rs already rendered
            };

            let mut states = HashMap::with_capacity(infos.len());
            let mut refreshes: Vec<(String, String, String)> = Vec::new();
            for info in &infos {
                let key = (info.doc_id.clone(), info.target_block_id.clone());
                let Some(&idx) = index_of.get(&key) else { continue };
                let Some(result) = results.get(idx) else { continue };
                let resolved = mention_state_from_result(result, info.target_block_id.is_some());
                if let MentionState::Live {
                    ref title,
                    ref snippet,
                } = resolved
                {
                    if *title != info.title || *snippet != info.snippet {
                        refreshes.push((info.node_block_id.clone(), title.clone(), snippet.clone()));
                    }
                }
                states.insert(info.node_block_id.clone(), resolved);
            }
            mention_states.set(states);

            // Editable sessions only: write the fresher title/snippet
            // back into the CRDT so future viewers stop paying for a
            // stale cache. `update_doc_mention_attrs` (routed through
            // `UpdateDocMentionAttrs`) tags the transaction
            // `history: skip` — this never creates an undo entry.
            //
            // Sent as ONE command carrying the whole batch, not one
            // command per stale mention: `on_command` runs through
            // `set_toolbar_command`, a plain signal — N synchronous
            // `.run()` calls in the same tick each overwrite the
            // last, so only the final mention's refresh would survive
            // (same coalescing hazard `pages/document.rs`'s
            // `clear_at_trigger_then_dispatch` documents).
            if editable.get_untracked() && !refreshes.is_empty() {
                let updates = refreshes
                    .into_iter()
                    .map(|(node_block_id, title, snippet)| {
                        crate::components::toolbar::DocMentionAttrUpdate {
                            node_block_id,
                            title,
                            snippet,
                        }
                    })
                    .collect();
                on_command.run(ToolbarCommand::UpdateDocMentionAttrs { updates });
            }
        });
    });

    // DOM decoration: reapplies every time editor_state OR
    // mention_states changes. The editor's own re-render rebuilds
    // every chip span from the model's cached attrs (view.rs), which
    // would silently wipe any class/text this Effect set — so it must
    // reapply after every render, not just once. Deferred one
    // microtask so the editor's DOM update lands first (same
    // `a11y::defer` precedent `find_replace_bar` uses before mapping
    // model positions to live DOM ranges).
    Effect::new(move |_| {
        // Read `mention_states` FIRST (and unconditionally) so this
        // Effect stays subscribed to it even on the early-return below
        // — otherwise a zero-mention doc would never re-run once the
        // async resolve later populates the map. Only after confirming
        // there's something to decorate do we also track `editor_state`
        // (whose every-keystroke changes are what actually drive the
        // reapply-on-rerender below) and pay for the DOM query — a doc
        // with no mentions has no overlay to maintain, so it shouldn't
        // pay a `querySelectorAll` per keystroke.
        let states = mention_states.get();
        if states.is_empty() {
            return;
        }
        let _ = editor_state.get();
        crate::a11y::defer(move || {
            apply_overlay_to_dom(&states);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::Fragment;

    fn doc_mention(doc_id: &str, target: Option<&str>) -> Node {
        let mut attrs = HashMap::new();
        attrs.insert("doc_id".to_string(), doc_id.to_string());
        if let Some(t) = target {
            attrs.insert("target_block_id".to_string(), t.to_string());
        }
        Node::element_with_attrs(NodeType::DocMention, attrs, Fragment::empty())
    }

    #[test]
    fn scan_finds_mentions_at_any_depth_with_targets() {
        let mention_a = doc_mention("a", None);
        let mention_b = doc_mention("b", Some("x"));
        let mention_a_id = mention_a.block_id().unwrap().to_string();
        let mention_b_id = mention_b.block_id().unwrap().to_string();

        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("hello")]),
                ),
                Node::element_with_content(NodeType::Paragraph, Fragment::from(vec![mention_a])),
                Node::element_with_content(
                    NodeType::BulletList,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![mention_b]),
                        )]),
                    )]),
                ),
            ]),
        );

        let refs = scan_doc_mentions(&doc);
        assert_eq!(refs.len(), 2);

        assert_eq!(refs[0].doc_id, "a");
        assert_eq!(refs[0].target_block_id, None);
        assert_eq!(refs[0].node_block_id, mention_a_id);

        assert_eq!(refs[1].doc_id, "b");
        assert_eq!(refs[1].target_block_id, Some("x".to_string()));
        assert_eq!(refs[1].node_block_id, mention_b_id);
    }

    #[test]
    fn scan_finds_nothing_in_a_doc_with_no_mentions() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("just text")]),
            )]),
        );
        assert!(scan_doc_mentions(&doc).is_empty());
    }

    fn resolve_result(
        status: &str,
        title: Option<&str>,
        block_found: Option<bool>,
        snippet: Option<&str>,
    ) -> MentionResolveResult {
        MentionResolveResult {
            status: status.to_string(),
            title: title.map(str::to_string),
            block_found,
            snippet: snippet.map(str::to_string),
        }
    }

    #[test]
    fn not_ok_status_is_always_missing_regardless_of_target() {
        let r = resolve_result("notFound", None, None, None);
        assert_eq!(mention_state_from_result(&r, false), MentionState::Missing);
        assert_eq!(mention_state_from_result(&r, true), MentionState::Missing);
    }

    #[test]
    fn ok_with_target_and_block_not_found_is_dangling() {
        let r = resolve_result("ok", Some("Target Doc"), Some(false), None);
        assert_eq!(
            mention_state_from_result(&r, true),
            MentionState::Dangling {
                title: "Target Doc".to_string()
            }
        );
    }

    #[test]
    fn ok_with_target_and_empty_snippet_but_block_found_is_live() {
        // Spec contract: blockFound is the liveness signal, not
        // snippet emptiness.
        let r = resolve_result("ok", Some("Target Doc"), Some(true), Some(""));
        assert_eq!(
            mention_state_from_result(&r, true),
            MentionState::Live {
                title: "Target Doc".to_string(),
                snippet: String::new(),
            }
        );
    }

    #[test]
    fn ok_without_target_is_live_even_with_block_found_absent() {
        // Plain document mention (no target_block_id) — block_found
        // is meaningless here and defaults to false, but that must
        // NOT produce Dangling since has_target is false.
        let r = resolve_result("ok", Some("Some Doc"), None, None);
        assert_eq!(
            mention_state_from_result(&r, false),
            MentionState::Live {
                title: "Some Doc".to_string(),
                snippet: String::new(),
            }
        );
    }
}
