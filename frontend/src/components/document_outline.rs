// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

use crate::a11y;
use crate::editor::model::{Node, NodeType};
use crate::editor::state::EditorState;

/// An entry in the document outline.
#[derive(Debug, Clone)]
pub struct OutlineEntry {
    /// Heading text.
    pub text: String,
    /// Heading level (1-3).
    pub level: u8,
    /// Model position of the heading's content start (for scroll-to).
    pub position: usize,
}

/// Extract outline entries from a document.
pub fn extract_outline(doc: &Node) -> Vec<OutlineEntry> {
    let Node::Element { content, .. } = doc else {
        return Vec::new();
    };

    let mut entries = Vec::new();
    let mut offset = 0;

    for child in &content.children {
        if let Node::Element {
            node_type: NodeType::Heading,
            attrs,
            ..
        } = child
        {
            let level = attrs
                .get("level")
                .and_then(|l| l.parse::<u8>().ok())
                .unwrap_or(1)
                .clamp(1, 3);
            let text = child.text_content();
            // content_start = offset + 1 (heading open boundary)
            entries.push(OutlineEntry {
                text,
                level,
                position: offset + 1,
            });
        }
        offset += child.node_size();
    }

    entries
}

/// Document outline sidebar component.
/// Shows a clickable table of contents generated from headings.
#[component]
pub fn DocumentOutline(
    /// Current editor state (for extracting headings).
    editor_state: ReadSignal<Option<EditorState>>,
    /// Whether the outline is visible.
    visible: ReadSignal<bool>,
    /// Callback when a heading is clicked (receives the model position).
    on_navigate: Callback<usize>,
    /// Optional close callback. When provided, a mobile-only close button
    /// renders inside the outline header (see responsive.css `.outline-close`).
    #[prop(optional)]
    on_close: Option<Callback<()>>,
    /// Phase 6 M-6.2 piece D: current doc id. When present, the
    /// outline drawer also hosts the RelationshipPanel below the
    /// heading list. Absent on the few rare callers that mount
    /// the outline outside a real doc context (none today).
    #[prop(into, optional)]
    doc_id: Option<Signal<String>>,
) -> impl IntoView {
    let entries = move || {
        editor_state
            .get()
            .map(|state| extract_outline(&state.doc))
            .unwrap_or_default()
    };

    view! {
        // Always-rendered outer wrapper so CSS transitions on `.is-open`
        // actually fire when the visibility signal flips. The inner content
        // is still gated on `visible` to skip the expensive iteration when
        // the drawer is hidden.
        <div class="document-outline" class:is-open=move || visible.get()>
            <Show when=move || visible.get()>
                <div class="outline-header">
                    <span>{crate::t!("outline-title")}</span>
                    {on_close.map(|cb| view! {
                        <button
                            class="outline-close"
                            aria-label=crate::t!("outline-aria-close")
                            on:click=move |_| a11y::defer_close(cb)
                        >"\u{00D7}"</button>
                    })}
                </div>
                <div class="outline-entries">
                    {move || {
                        let items = entries();
                        if items.is_empty() {
                            view! {
                                <div class="outline-empty">{crate::t!("outline-empty")}</div>
                            }.into_any()
                        } else {
                            view! {
                                <ul class="outline-list">
                                    {items.into_iter().map(|entry| {
                                        let pos = entry.position;
                                        let indent_class = match entry.level {
                                            1 => "outline-h1",
                                            2 => "outline-h2",
                                            _ => "outline-h3",
                                        };
                                        view! {
                                            <li
                                                class=format!("outline-item {indent_class}")
                                                on:click=move |_| on_navigate.run(pos)
                                            >
                                                {entry.text}
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
                                </ul>
                            }.into_any()
                        }
                    }}
                </div>
                {doc_id.map(|id| view! {
                    <crate::components::relationship_panel::RelationshipPanel
                        doc_id=id
                    />
                })}
            </Show>
        </div>
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{Fragment, Node, NodeType};
    use std::collections::HashMap;

    #[test]
    fn extract_outline_from_headings() {
        let mut attrs1 = HashMap::new();
        attrs1.insert("level".to_string(), "1".to_string());
        let mut attrs2 = HashMap::new();
        attrs2.insert("level".to_string(), "2".to_string());

        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs1,
                    Fragment::from(vec![Node::text("Title")]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Some text")]),
                ),
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs2,
                    Fragment::from(vec![Node::text("Subtitle")]),
                ),
            ]),
        );

        let entries = extract_outline(&doc);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].text, "Title");
        assert_eq!(entries[0].level, 1);
        assert_eq!(entries[0].position, 1);
        assert_eq!(entries[1].text, "Subtitle");
        assert_eq!(entries[1].level, 2);
        // Title heading size: 1 + 5 + 1 = 7. Paragraph size: 1 + 9 + 1 = 11.
        // Subtitle starts at offset 7 + 11 = 18. Position = 18 + 1 = 19.
        assert_eq!(entries[1].position, 19);
    }

    #[test]
    fn extract_outline_no_headings() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Just a paragraph")]),
            )]),
        );
        let entries = extract_outline(&doc);
        assert!(entries.is_empty());
    }

    #[test]
    fn extract_outline_empty_doc() {
        let doc = Node::empty_doc();
        let entries = extract_outline(&doc);
        assert!(entries.is_empty());
    }

    #[test]
    fn extract_outline_clamps_level() {
        // Level 0 should clamp to 1, level 5 should clamp to 3
        let mut attrs_low = HashMap::new();
        attrs_low.insert("level".to_string(), "0".to_string());
        let mut attrs_high = HashMap::new();
        attrs_high.insert("level".to_string(), "5".to_string());
        let mut attrs_missing = HashMap::new();
        // No "level" key at all — should default to 1

        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs_low,
                    Fragment::from(vec![Node::text("Low")]),
                ),
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs_high,
                    Fragment::from(vec![Node::text("High")]),
                ),
                Node::element_with_attrs(
                    NodeType::Heading,
                    attrs_missing,
                    Fragment::from(vec![Node::text("Missing")]),
                ),
            ]),
        );

        let entries = extract_outline(&doc);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].level, 1, "level 0 should clamp to 1");
        assert_eq!(entries[1].level, 3, "level 5 should clamp to 3");
        assert_eq!(entries[2].level, 1, "missing level should default to 1");
    }
}
