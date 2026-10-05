//! Emit deterministic CRDT content for the mocked browser i18n regression suite.
use base64::{Engine, engine::general_purpose::STANDARD};
use ogrenotes_frontend::editor::{
    model::{Fragment, Mark, MarkType, Node, NodeType},
    yrs_bridge::doc_to_ydoc_bytes,
};

fn element(kind: NodeType, attrs: &[(&str, &str)], children: Vec<Node>) -> Node {
    Node::element_with_attrs(
        kind,
        attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        Fragment::from(children),
    )
}

fn main() {
    let calendar = element(
        NodeType::Calendar,
        &[
            ("blockId", "calendar"),
            ("view", "month"),
            ("cursor", "2026-05"),
            ("timezone", "UTC"),
        ],
        vec![element(
            NodeType::CalendarEvent,
            &[
                ("blockId", "event"),
                ("allDay", "true"),
                ("startDate", "2026-05-04"),
                ("endDate", "2026-05-04"),
                ("content", ""),
            ],
            vec![],
        )],
    );
    let code = element(
        NodeType::CodeBlock,
        &[("blockId", "code"), ("language", "")],
        vec![Node::text("example")],
    );
    let directional = element(
        NodeType::Paragraph,
        &[("blockId", "rtl-paragraph"), ("dir", "rtl")],
        vec![Node::text("مرحبا Hello 123")],
    );
    let left_aligned = element(
        NodeType::Paragraph,
        &[
            ("blockId", "rtl-left-paragraph"),
            ("dir", "rtl"),
            ("align", "left"),
        ],
        vec![Node::text("مرحبا left")],
    );
    let mixed = element(
        NodeType::Paragraph,
        &[("blockId", "mixed-selection"), ("dir", "rtl")],
        vec![
            Node::text("مرحبا "),
            Node::text_with_marks("😀Hello", vec![Mark::new(MarkType::Bold)]),
            Node::text(" עולם 123"),
        ],
    );
    let doc = element(
        NodeType::Doc,
        &[],
        vec![directional, left_aligned, calendar, code, mixed],
    );
    let cells = ["1234.5", "2"]
        .iter()
        .map(|v| {
            element(
                NodeType::TableCell,
                &[],
                vec![element(NodeType::Paragraph, &[], vec![Node::text(v)])],
            )
        })
        .collect();
    let sheet = element(
        NodeType::Doc,
        &[],
        vec![element(
            NodeType::Table,
            &[("sheetName", "Sheet1")],
            vec![element(NodeType::TableRow, &[], cells)],
        )],
    );
    println!(
        "{}",
        serde_json::json!({"document":STANDARD.encode(doc_to_ydoc_bytes(&doc)),"spreadsheet":STANDARD.encode(doc_to_ydoc_bytes(&sheet))})
    );
}
