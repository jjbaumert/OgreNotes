use leptos::prelude::*;

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
) -> impl IntoView {
    let entries = move || {
        editor_state
            .get()
            .map(|state| extract_outline(&state.doc))
            .unwrap_or_default()
    };

    view! {
        <Show when=move || visible.get()>
            <div class="document-outline">
                <div class="outline-header">"Outline"</div>
                <div class="outline-entries">
                    {move || {
                        let items = entries();
                        if items.is_empty() {
                            view! {
                                <div class="outline-empty">"No headings"</div>
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
            </div>
        </Show>
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
}
