// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Equation block (frontend). Renders the MathML produced by
//! `ogrenotes-math` from the LaTeX in the `source` attribute, or an
//! error banner + the raw source when it doesn't parse. Editing goes
//! through the equation modal (see `components::math_modal`).

use std::collections::HashMap;

use web_sys::{Document, Node as DomNode};

use super::super::model::{Fragment, Node, NodeType};
use super::{LiveAppBlockInsert, LiveAppBlockView};

pub struct MathBlockView;
pub struct MathBlockInsert;

const STARTER_SOURCE: &str = "e^{i\\pi} + 1 = 0";

impl LiveAppBlockView for MathBlockView {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::MathBlock]
    }

    fn render(
        &self,
        doc: &Document,
        _node_type: NodeType,
        attrs: &HashMap<String, String>,
        _content: &Fragment,
    ) -> Option<DomNode> {
        let source = attrs.get("source").map(String::as_str).unwrap_or("");
        let wrapper = doc.create_element("div").ok()?;
        wrapper.set_attribute("class", "math-block").ok()?;
        wrapper.set_attribute("contenteditable", "false").ok()?;
        // Leaf atom of model size 1 — DOM↔model mapping treats it as opaque.
        wrapper.set_attribute("data-atom-size", "1").ok()?;
        if let Some(bid) = attrs.get("blockId") {
            wrapper.set_attribute("data-block-id", bid).ok()?;
        }
        // Edit hook for the delegated click listener, and the current
        // source to seed the modal (`set_attribute` escapes it).
        wrapper.set_attribute("data-math-action", "edit").ok()?;
        wrapper.set_attribute("data-source", source).ok()?;

        match ogrenotes_math::to_mathml(source, ogrenotes_math::Display::Block) {
            Ok(mathml) => {
                let holder = doc.create_element("div").ok()?;
                holder.set_attribute("class", "math-render").ok()?;
                // Trusted: MathML from our renderer, with the source text
                // XML-escaped and attributes from fixed tables.
                holder.set_inner_html(&mathml);
                wrapper.append_child(&holder).ok()?;
            }
            Err(e) => {
                let banner = doc.create_element("p").ok()?;
                banner.set_attribute("class", "math-error").ok()?;
                banner.set_text_content(Some(&e.to_string()));
                let pre = doc.create_element("pre").ok()?;
                // Untrusted source — text content, never inner_html.
                pre.set_text_content(Some(source));
                wrapper.append_child(&banner).ok()?;
                wrapper.append_child(&pre).ok()?;
            }
        }
        Some(wrapper.into())
    }
}

impl LiveAppBlockInsert for MathBlockInsert {
    fn id(&self) -> &'static str {
        "math"
    }
    fn label_key(&self) -> &'static str {
        "insert-math-label"
    }
    fn description_key(&self) -> &'static str {
        "insert-math-description"
    }
    fn icon(&self) -> &'static str {
        "\u{2211}" // ∑
    }
    fn build_default_node(&self) -> Node {
        let mut attrs = HashMap::new();
        attrs.insert("source".to_string(), STARTER_SOURCE.to_string());
        Node::element_with_attrs(NodeType::MathBlock, attrs, Fragment::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_source_renders() {
        assert!(ogrenotes_math::to_mathml(STARTER_SOURCE, ogrenotes_math::Display::Block).is_ok());
        let node = MathBlockInsert.build_default_node();
        let Node::Element { node_type, attrs, .. } = node else { panic!("element") };
        assert_eq!(node_type, NodeType::MathBlock);
        assert_eq!(attrs.get("source").map(String::as_str), Some(STARTER_SOURCE));
    }
}
