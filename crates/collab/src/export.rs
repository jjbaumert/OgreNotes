// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use yrs::{
    Any, Doc, Out, ReadTxn, Text, Transact,
    types::xml::{Xml, XmlFragment, XmlOut},
    types::{Attrs, GetString},
};

use crate::schema::NodeType;

/// Export a yrs document to HTML.
/// Cap on export-recursion depth. yrs doesn't stop a malicious client
/// from writing deeply nested elements, and unbounded recursive traversal
/// would overflow the stack and crash the process on export (#7). Far
/// above any legitimate document's nesting; content below the cap is
/// simply not descended into.
const MAX_EXPORT_DEPTH: usize = 256;

thread_local! {
    /// Shared recursion counter for all three export traversals
    /// (`extract_text`, `render_node_html`, `render_node_markdown`). They
    /// share one MAX_EXPORT_DEPTH budget, which is sound only because no
    /// traversal calls another mid-traversal — they compose at the top
    /// level (e.g. `to_html` then `to_markdown`), never nested. If a future
    /// change nests one inside another, the shared budget could trip at a
    /// lower effective depth; split this into per-traversal counters then.
    /// `DepthGuard` always restores on drop (including during panic
    /// unwinding), so the counter is 0 at every top-level entry.
    static EXPORT_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// RAII guard: bumps `EXPORT_DEPTH` on entry and restores it on drop.
/// `enter()` returns `None` once the cap is reached so the caller bails
/// instead of recursing deeper. Used by every recursive node renderer.
struct DepthGuard;

impl DepthGuard {
    fn enter() -> Option<Self> {
        EXPORT_DEPTH.with(|d| {
            let cur = d.get();
            if cur >= MAX_EXPORT_DEPTH {
                None
            } else {
                d.set(cur + 1);
                Some(DepthGuard)
            }
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        EXPORT_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

pub fn to_html(doc: &Doc) -> String {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return String::new();
    };

    let mut html = String::new();
    render_fragment_html(&txn, &fragment, &mut html);
    html
}

/// Export a yrs document to Markdown.
pub fn to_markdown(doc: &Doc) -> String {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return String::new();
    };

    let mut md = String::new();
    render_fragment_markdown(&txn, &fragment, &mut md, 0);
    md
}

/// Export a yrs document to plain text for search indexing.
/// Extracts all text content without formatting, separated by newlines.
pub fn to_plain_text(doc: &Doc) -> String {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return String::new();
    };

    let mut text = String::new();
    let len = fragment.len(&txn);
    for i in 0..len {
        let Some(XmlOut::Element(el)) = fragment.get(&txn, i) else {
            continue;
        };
        let block_text = extract_text(&txn, &el);
        if !block_text.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&block_text);
        }
    }
    text
}

/// Export a yrs document to CSV.
/// Finds the first table in the document and outputs each row as a CSV line.
pub fn to_csv(doc: &Doc) -> String {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return String::new();
    };

    let mut csv = String::new();
    let len = fragment.len(&txn);
    for i in 0..len {
        let Some(XmlOut::Element(el)) = fragment.get(&txn, i) else { continue };
        if el.tag().as_ref() != "table" { continue; }

        // Found a table — iterate rows
        let row_count = el.len(&txn);
        for ri in 0..row_count {
            let Some(XmlOut::Element(row_el)) = el.get(&txn, ri) else { continue };
            let tag = row_el.tag();
            if tag.as_ref() != "table_row" { continue; }

            let col_count = row_el.len(&txn);
            for ci in 0..col_count {
                if ci > 0 { csv.push(','); }
                let cell_text = if let Some(XmlOut::Element(cell_el)) = row_el.get(&txn, ci) {
                    extract_text(&txn, &cell_el)
                } else {
                    String::new()
                };
                csv.push_str(&csv_encode_cell(&cell_text));
            }
            csv.push('\n');
        }
        break; // only first table
    }
    csv
}

/// Encode a single cell for CSV output.
///
/// Two rules:
/// 1. **CSV quoting** (RFC 4180): if the value contains `,`, `\n`, `\r`, or
///    `"`, wrap it in double quotes and double any embedded quote.
/// 2. **Formula-injection guard**: spreadsheet apps (Excel, LibreOffice,
///    Google Sheets) interpret a cell whose first character is `=`, `+`,
///    `-`, `@`, tab, or carriage return as a formula/command. A user
///    typing `=HYPERLINK("https://evil", "Click")` into a cell would then
///    execute on any recipient who opens the CSV. We neutralize that by
///    prepending a single quote, which every major app renders as literal
///    text.
fn csv_encode_cell(cell: &str) -> String {
    let needs_formula_guard = matches!(
        cell.as_bytes().first(),
        Some(b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r')
    );
    let guarded: std::borrow::Cow<'_, str> = if needs_formula_guard {
        std::borrow::Cow::Owned(format!("'{cell}"))
    } else {
        std::borrow::Cow::Borrowed(cell)
    };

    if guarded.contains([',', '\n', '\r', '"']) {
        let mut out = String::with_capacity(guarded.len() + 2);
        out.push('"');
        for ch in guarded.chars() {
            if ch == '"' {
                out.push_str("\"\"");
            } else {
                out.push(ch);
            }
        }
        out.push('"');
        out
    } else {
        guarded.into_owned()
    }
}

/// Recursively extract plain text from a yrs XmlElement.
fn extract_text<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef) -> String {
    // #7: bail out past the recursion cap so a deeply nested blob can't
    // overflow the stack.
    let Some(_depth) = DepthGuard::enter() else {
        return String::new();
    };
    // #148 slice 6 — a Mention leaf carries its display in an
    // attribute rather than a child text run; emit it directly.
    // Matches the frontend `Node::text_content` behavior so
    // LLM prompts, search index, and `plain_text_from_state`
    // see the same "@alice" string across both surfaces.
    if el.tag().as_ref() == NodeType::Mention.tag_name() {
        return el.get_attribute(txn, "display").unwrap_or_default();
    }
    let mut text = String::new();
    let len = el.len(txn);
    for i in 0..len {
        match el.get(txn, i) {
            Some(XmlOut::Element(child)) => {
                text.push_str(&extract_text(txn, &child));
            }
            Some(XmlOut::Text(t)) => {
                text.push_str(&t.get_string(txn));
            }
            _ => {}
        }
    }
    text
}

/// Excel's hard grid limits (xlsx). A table wider/taller than these can't
/// be represented, and writing past them either silently drops the cell
/// (rows) or wraps `ci as u16` and clobbers an earlier column (#7).
#[cfg(feature = "xlsx")]
const XLSX_MAX_ROWS: u32 = 1_048_576;
#[cfg(feature = "xlsx")]
const XLSX_MAX_COLS: u32 = 16_384;

/// Export a yrs document to XLSX (Excel) format.
/// Returns the raw bytes of the .xlsx file.
#[cfg(feature = "xlsx")]
pub fn to_xlsx(doc: &Doc) -> Vec<u8> {
    use rust_xlsxwriter::{Workbook, Format};

    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return Vec::new();
    };

    let mut workbook = Workbook::new();
    let len = fragment.len(&txn);
    let mut sheet_idx = 0u32;

    for i in 0..len {
        let Some(yrs::types::xml::XmlOut::Element(el)) = fragment.get(&txn, i) else { continue };
        if el.tag().as_ref() != "table" { continue; }

        // Get sheet name from attribute
        let sheet_name = el.get_attribute(&txn, ATTR_SHEET_NAME)
            .unwrap_or_else(|| format!("Sheet{}", sheet_idx + 1));

        let worksheet = workbook.add_worksheet();
        let _ = worksheet.set_name(&sheet_name);

        let row_count = el.len(&txn);
        for ri in 0..row_count {
            // #7: Excel's grid is 1,048,576 rows × 16,384 cols. Past those a
            // write is silently dropped (rows) or `ci as u16` wraps and
            // clobbers an earlier cell (cols). Stop at the limit instead.
            if ri >= XLSX_MAX_ROWS {
                break;
            }
            let Some(yrs::types::xml::XmlOut::Element(row_el)) = el.get(&txn, ri) else { continue };
            if row_el.tag().as_ref() != "table_row" { continue; }

            let col_count = row_el.len(&txn);
            for ci in 0..col_count {
                if ci >= XLSX_MAX_COLS {
                    break;
                }
                if let Some(yrs::types::xml::XmlOut::Element(cell_el)) = row_el.get(&txn, ci) {
                    let text = extract_text(&txn, &cell_el);

                    // Apply cell formatting from attributes
                    let mut fmt = Format::new();
                    if cell_el.get_attribute(&txn, "bold").as_deref() == Some("1") {
                        fmt = fmt.set_bold();
                    }
                    if cell_el.get_attribute(&txn, "italic").as_deref() == Some("1") {
                        fmt = fmt.set_italic();
                    }

                    // Write value: try number first, then text
                    if let Ok(num) = text.parse::<f64>() {
                        let _ = worksheet.write_number_with_format(ri, ci as u16, num, &fmt);
                    } else if !text.is_empty() {
                        let _ = worksheet.write_string_with_format(ri, ci as u16, &text, &fmt);
                    }

                }
            }
        }
        sheet_idx += 1;
    }

    workbook.save_to_buffer().unwrap_or_default()
}

/// Export a yrs document to DOCX (Word) format — Phase 6 M-6.5 piece B.
///
/// The mirror image of `import_docx::from_docx`: walks the `content`
/// fragment's top-level blocks and emits them via the docx-rs writer.
/// Paragraphs and headings become `w:p` (headings carry a
/// `Heading{level}` style so `from_docx` recovers the level on a
/// round-trip); tables become `w:tbl`. Inline marks are not emitted —
/// symmetric with the importer's v1 limitation. Any other block kind
/// degrades to a plain paragraph of its text.
///
/// Returns the packed `.docx` bytes, or an empty `Vec` if packing
/// fails (same contract as `to_xlsx`).
#[cfg(feature = "docx")]
pub fn to_docx(doc: &Doc) -> Vec<u8> {
    use docx_rs::{Docx, Paragraph, Run};

    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return Vec::new();
    };

    let mut docx = Docx::new();
    let len = fragment.len(&txn);
    for i in 0..len {
        let Some(XmlOut::Element(el)) = fragment.get(&txn, i) else {
            continue;
        };
        let tag = el.tag();
        let Some(node_type) = NodeType::from_tag(tag.as_ref()) else {
            continue;
        };
        match node_type {
            NodeType::Table => {
                docx = docx.add_table(build_docx_table(&txn, &el));
            }
            NodeType::Heading => {
                let level = el
                    .get_attribute(&txn, "level")
                    .and_then(|l| l.parse::<u8>().ok())
                    .unwrap_or(1)
                    .clamp(1, 6);
                let text = extract_text(&txn, &el);
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .style(&format!("Heading{level}"))
                        .add_run(Run::new().add_text(text)),
                );
            }
            // Paragraph and every other block kind (lists, quotes,
            // code, …) flatten to a plain paragraph of their text in
            // v1 — same scope as the importer.
            _ => {
                let text = extract_text(&txn, &el);
                docx = docx.add_paragraph(Paragraph::new().add_run(Run::new().add_text(text)));
            }
        }
    }

    let mut cursor = std::io::Cursor::new(Vec::new());
    match docx.build().pack(&mut cursor) {
        Ok(_) => cursor.into_inner(),
        Err(e) => {
            tracing::warn!(error = %e, "to_docx: pack failed, returning empty body");
            Vec::new()
        }
    }
}

/// Build a docx-rs `Table` from a `table` element. Each cell's text is
/// emitted as a single paragraph — the inverse of how `from_docx`
/// collapses a cell to text.
#[cfg(feature = "docx")]
fn build_docx_table<T: ReadTxn>(txn: &T, table_el: &yrs::XmlElementRef) -> docx_rs::Table {
    use docx_rs::{Paragraph, Run, Table, TableCell, TableRow};

    let mut rows = Vec::new();
    let rlen = table_el.len(txn);
    for ri in 0..rlen {
        let Some(XmlOut::Element(row_el)) = table_el.get(txn, ri) else {
            continue;
        };
        if row_el.tag().as_ref() != NodeType::TableRow.tag_name() {
            continue;
        }
        let mut cells = Vec::new();
        let clen = row_el.len(txn);
        for ci in 0..clen {
            let Some(XmlOut::Element(cell_el)) = row_el.get(txn, ci) else {
                continue;
            };
            // Accept body and header cells; skip any other element a
            // future schema change might place in a row. docx-rs has no
            // distinct header-cell primitive, so a header's text is
            // preserved but its <th> semantics are not recoverable on a
            // DOCX round-trip — a v1 limitation symmetric with the
            // importer (which collapses every cell to text).
            let cell_tag = cell_el.tag();
            if cell_tag.as_ref() != NodeType::TableCell.tag_name()
                && cell_tag.as_ref() != NodeType::TableHeader.tag_name()
            {
                continue;
            }
            let text = extract_text(txn, &cell_el);
            cells.push(
                TableCell::new()
                    .add_paragraph(Paragraph::new().add_run(Run::new().add_text(text))),
            );
        }
        rows.push(TableRow::new(cells));
    }
    Table::new(rows)
}

/// Export a yrs document to PDF (plain text) — Phase 6 M-6.6 piece B.
///
/// The mirror of `import_pdf::from_pdf`'s lossiness: PDF is a
/// page-description format, so we emit text only. Each top-level block's
/// text becomes a paragraph (blank line between paragraphs), wrapped to
/// the page width and flowed across A4 pages via the built-in Helvetica
/// font (no bundled font asset). Marks, headings styling, images, and
/// table structure are not represented — a table degrades to its
/// concatenated cell text, same low-fidelity contract as the importer.
///
/// Returns the rendered PDF bytes — always a valid, non-empty PDF (an
/// empty document yields a blank single page). Unlike `to_xlsx` /
/// `to_docx` there is no failure path: `printpdf`'s save is infallible
/// for the ops we emit.
#[cfg(feature = "pdf")]
pub fn to_pdf(doc: &Doc) -> Vec<u8> {
    use printpdf::{
        BuiltinFont, Mm, Op, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions, PdfWarnMsg,
        Point, Pt, TextItem,
    };

    // A4 portrait, 20mm margins, 11pt text on 14pt leading. WRAP_CHARS
    // is a width estimate for 11pt Helvetica across the ~170mm text
    // column — proportional fonts make exact wrapping impossible without
    // glyph metrics, and this is documented low-fidelity output.
    const PAGE_W_MM: f32 = 210.0;
    const PAGE_H_MM: f32 = 297.0;
    const MARGIN_MM: f32 = 20.0;
    const FONT_SIZE_PT: f32 = 11.0;
    const LINE_HEIGHT_PT: f32 = 14.0;
    const WRAP_CHARS: usize = 90;

    // Flatten the doc to display lines: each top-level block's text,
    // wrapped, with a blank line between blocks.
    let paragraphs = collect_block_text(doc);
    let mut lines: Vec<String> = Vec::new();
    for (i, para) in paragraphs.iter().enumerate() {
        if i > 0 {
            lines.push(String::new());
        }
        lines.extend(wrap_text(para, WRAP_CHARS));
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    let font = PdfFontHandle::Builtin(BuiltinFont::Helvetica);
    let top_y = Pt::from(Mm(PAGE_H_MM - MARGIN_MM)).0;
    let left_x = Pt::from(Mm(MARGIN_MM)).0;
    let bottom_y = Pt::from(Mm(MARGIN_MM)).0;

    let open_page = |ops: &mut Vec<Op>, font: &PdfFontHandle| {
        ops.push(Op::StartTextSection);
        ops.push(Op::SetFont {
            font: font.clone(),
            size: Pt(FONT_SIZE_PT),
        });
        ops.push(Op::SetLineHeight { lh: Pt(LINE_HEIGHT_PT) });
        ops.push(Op::SetTextCursor {
            pos: Point {
                x: Pt(left_x),
                y: Pt(top_y),
            },
        });
    };

    let mut pages: Vec<PdfPage> = Vec::new();
    let mut ops: Vec<Op> = Vec::new();
    open_page(&mut ops, &font);
    let mut y = top_y;

    for line in &lines {
        if y < bottom_y {
            ops.push(Op::EndTextSection);
            pages.push(PdfPage::new(Mm(PAGE_W_MM), Mm(PAGE_H_MM), std::mem::take(&mut ops)));
            open_page(&mut ops, &font);
            y = top_y;
        }
        if !line.is_empty() {
            ops.push(Op::ShowText {
                items: vec![TextItem::Text(line.clone())],
            });
        }
        ops.push(Op::AddLineBreak);
        y -= LINE_HEIGHT_PT;
    }
    ops.push(Op::EndTextSection);
    pages.push(PdfPage::new(Mm(PAGE_W_MM), Mm(PAGE_H_MM), ops));

    let mut pdf = PdfDocument::new("OgreNotes export");
    pdf.with_pages(pages);
    let mut warnings: Vec<PdfWarnMsg> = Vec::new();
    let bytes = pdf.save(&PdfSaveOptions::default(), &mut warnings);
    for w in &warnings {
        tracing::warn!(warning = ?w, "to_pdf: printpdf serialization warning");
    }
    bytes
}

/// Text of each top-level block, in order. A block's text is its
/// recursively-concatenated leaf text (tables collapse to cell text).
#[cfg(feature = "pdf")]
fn collect_block_text(doc: &Doc) -> Vec<String> {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let len = fragment.len(&txn);
    for i in 0..len {
        if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
            out.push(extract_text(&txn, &el));
        }
    }
    out
}

/// Greedy word-wrap to `max_chars` per line (char-counted, so non-ASCII
/// is safe). Preserves explicit `\n` as line breaks; hard-splits any
/// single word longer than the limit.
#[cfg(feature = "pdf")]
fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    debug_assert!(max_chars > 0, "max_chars must be positive (hard-split would loop forever)");
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let mut cur = String::new();
        for word in raw.split_whitespace() {
            let wlen = word.chars().count();
            if wlen > max_chars {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                let mut chars: Vec<char> = word.chars().collect();
                while chars.len() > max_chars {
                    out.push(chars[..max_chars].iter().collect());
                    chars.drain(..max_chars);
                }
                cur = chars.into_iter().collect();
                continue;
            }
            // cur is bounded by max_chars, so recounting per word is cheap.
            let cur_len = cur.chars().count();
            let sep = usize::from(cur_len > 0);
            if cur_len + sep + wlen > max_chars {
                out.push(std::mem::take(&mut cur));
                cur = word.to_string();
            } else {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(word);
            }
        }
        out.push(cur);
    }
    out
}

pub(crate) const ATTR_SHEET_NAME: &str = "sheetName";

/// True when a simple (boolean) inline mark is present on a text chunk.
fn has_mark(attrs: &Attrs, key: &str) -> bool {
    matches!(attrs.get(key), Some(Any::Bool(true)))
}

/// Extract a link mark's `href`. The editor stores attr-bearing marks
/// (link, colors) as a JSON string; a link's payload is `{"href": "…"}`.
fn link_href(attrs: &Attrs) -> Option<String> {
    let Some(Any::String(json)) = attrs.get("link") else {
        return None;
    };
    let parsed: std::collections::HashMap<String, String> = serde_json::from_str(json).ok()?;
    parsed.get("href").cloned()
}

/// #148: extract a `MarkType::Mention` mark's `user_id`. Same
/// JSON-string encoding as `link` (see `yrs_bridge::marks_to_attrs`
/// on the frontend); export preserves the id so downstream
/// notification / audit consumers can rehydrate the mention.
fn mention_user_id(attrs: &Attrs) -> Option<String> {
    let Some(Any::String(json)) = attrs.get("mention") else {
        return None;
    };
    let parsed: std::collections::HashMap<String, String> = serde_json::from_str(json).ok()?;
    parsed.get("user_id").cloned()
}

/// Render a formatted `XmlText` chunk-by-chunk into HTML, wrapping each
/// run in the tags for its marks (#7). A link nests outermost, with
/// bold / italic / underline / strike / code inside. The href is
/// scheme-checked (`is_safe_url`) and attribute-escaped — closing the
/// latent XSS the ticket flagged for when link export landed.
fn render_text_html<T: ReadTxn>(txn: &T, text: &yrs::XmlTextRef, out: &mut String) {
    for chunk in text.diff(txn, |_| ()) {
        let Out::Any(Any::String(s)) = &chunk.insert else {
            // Non-string embedded values have no text representation; emit
            // nothing rather than raw CRDT data.
            continue;
        };
        let mut open = String::new();
        let mut close = String::new();
        if let Some(attrs) = chunk.attributes.as_deref() {
            if let Some(href) = link_href(attrs).filter(|h| is_safe_url(h)) {
                open.push_str(&format!("<a href=\"{}\">", html_escape_attr(&href)));
                close.insert_str(0, "</a>");
            }
            // #148: mention chip preserves the user_id and the
            // `.mention` class so paste-into-doc round-trips (see
            // `frontend/src/editor/clipboard.rs::tag_to_mark`).
            if let Some(uid) = mention_user_id(attrs) {
                open.push_str(&format!(
                    "<span class=\"mention\" data-user-id=\"{}\">",
                    html_escape_attr(&uid),
                ));
                close.insert_str(0, "</span>");
            }
            for (key, otag, ctag) in [
                ("bold", "<strong>", "</strong>"),
                ("italic", "<em>", "</em>"),
                ("underline", "<u>", "</u>"),
                ("strike", "<s>", "</s>"),
                ("code", "<code>", "</code>"),
                ("subscript", "<sub>", "</sub>"),
                ("superscript", "<sup>", "</sup>"),
            ] {
                if has_mark(attrs, key) {
                    open.push_str(otag);
                    close.insert_str(0, ctag);
                }
            }
        }
        out.push_str(&open);
        out.push_str(&html_escape(s));
        out.push_str(&close);
    }
}

/// Markdown counterpart of `render_text_html`. Emphasis wraps innermost
/// (code/bold/italic/strike); underline has no Markdown syntax so falls
/// back to inline `<u>`; a link wraps outermost with its label brackets
/// escaped and URL parens encoded (#7).
fn render_text_markdown<T: ReadTxn>(txn: &T, text: &yrs::XmlTextRef, out: &mut String) {
    for chunk in text.diff(txn, |_| ()) {
        let Out::Any(Any::String(s)) = &chunk.insert else {
            // Non-string embedded values (Any::Map/Array/Number, nested
            // shared types) have no text representation; emit nothing
            // rather than raw CRDT data.
            continue;
        };
        let Some(attrs) = chunk.attributes.as_deref() else {
            out.push_str(&escape_markdown_text(s));
            continue;
        };
        let rendered = if let Some(href) = link_href(attrs).filter(|h| is_safe_url(h)) {
            // A link label is inline context, so escape it for the label
            // directly via `escape_md_link_text`. Building it from
            // `escape_markdown_text(s)` instead would double-escape the
            // block-level backslashes that helper inserts at line starts
            // (`\#` → `\\#`), corrupting the rendered label.
            let label = wrap_md_emphasis(escape_md_link_text(s), attrs);
            format!("[{}]({})", label, escape_md_url(&href))
        } else {
            wrap_md_emphasis(escape_markdown_text(s), attrs)
        };
        out.push_str(&rendered);
    }
}

/// Wrap text in the Markdown emphasis delimiters for whatever inline
/// marks are present (innermost first: code, bold, italic, strike; then
/// underline via inline `<u>`, which has no Markdown syntax). Does NOT
/// apply the link mark — the caller nests that outermost so it can build
/// the label in the correct (inline) escaping context.
fn wrap_md_emphasis(mut content: String, attrs: &Attrs) -> String {
    if has_mark(attrs, "code") {
        content = format!("`{content}`");
    }
    if has_mark(attrs, "bold") {
        content = format!("**{content}**");
    }
    if has_mark(attrs, "italic") {
        content = format!("_{content}_");
    }
    if has_mark(attrs, "strike") {
        content = format!("~~{content}~~");
    }
    if has_mark(attrs, "underline") {
        content = format!("<u>{content}</u>");
    }
    // Sub/superscript have no Markdown syntax — fall back to inline HTML, the
    // same approach as underline (round-trips via the HTML tag importer).
    if has_mark(attrs, "subscript") {
        content = format!("<sub>{content}</sub>");
    }
    if has_mark(attrs, "superscript") {
        content = format!("<sup>{content}</sup>");
    }
    // #148: mention chip has no Markdown syntax — fall back to inline
    // HTML mirroring the HTML export path so the paste-import round-
    // trip via `clipboard.rs::tag_to_mark` recovers the mark.
    if let Some(uid) = mention_user_id(attrs) {
        content = format!(
            "<span class=\"mention\" data-user-id=\"{}\">{content}</span>",
            escape_md_link_text(&uid),
        );
    }
    content
}

fn render_fragment_html<T: ReadTxn>(txn: &T, fragment: &yrs::XmlFragmentRef, out: &mut String) {
    let len = fragment.len(txn);
    for i in 0..len {
        if let Some(child) = fragment.get(txn, i) {
            render_node_html(txn, &child, out);
        }
    }
}

fn render_node_html<T: ReadTxn>(txn: &T, node: &XmlOut, out: &mut String) {
    // #7: stop descending past the recursion cap (stack-overflow guard).
    let Some(_depth) = DepthGuard::enter() else {
        return;
    };
    match node {
        XmlOut::Element(el) => {
            let tag = el.tag();
            let Some(node_type) = NodeType::from_tag(&tag) else {
                return;
            };

            // Compute the correct HTML tag (headings need dynamic tags)
            let html_tag = resolve_html_tag(txn, el, node_type);
            let attrs = render_html_attrs(txn, el, node_type);

            // Syntax-highlighted code blocks take a dedicated path so
            // the export matches the live editor DOM
            // (<pre><code class="language-x">…tok spans…</code></pre>).
            // Unknown/empty languages and non-text content fall through
            // to the generic path — byte-identical to the pre-highlight
            // output.
            if node_type == NodeType::CodeBlock {
                if render_code_block_highlighted(txn, el, out) {
                    return;
                }
            }

            if node_type.is_leaf() {
                // #136 — CalendarEvent is a leaf but carries its
                // display text in a `content` attribute; write it
                // as text between the open/close tags rather than
                // self-closing (which would export title-less
                // color chips). Other leaf blocks stay
                // self-closing.
                if matches!(node_type, NodeType::CalendarEvent) {
                    let content = el.get_attribute(txn, "content").unwrap_or_default();
                    out.push_str(&format!(
                        "<{html_tag}{attrs}>{}</{html_tag}>",
                        html_escape(&content),
                    ));
                    return;
                }
                // #148 slice 6 — Mention chip: display name goes
                // between the `<span>` tags, matching the
                // pre-existing text+MarkType::Mention output
                // byte-for-byte so paste round-trip through
                // clipboard.rs::parse_from_html stays stable.
                if matches!(node_type, NodeType::Mention) {
                    let display = el.get_attribute(txn, "display").unwrap_or_default();
                    out.push_str(&format!(
                        "<{html_tag}{attrs}>{}</{html_tag}>",
                        html_escape(&display),
                    ));
                    return;
                }
                if matches!(node_type, NodeType::Mermaid) {
                    let source = el.get_attribute(txn, "source").unwrap_or_default();
                    let out_render = ogrenotes_mermaid::render(&source);
                    match out_render.svg {
                        Some(svg) => {
                            // SVG is generated by our own renderer (no
                            // user HTML passes through); the source is
                            // XML-escaped inside the renderer.
                            out.push_str(&format!(
                                "<div class=\"mermaid-block\">{svg}</div>"
                            ));
                        }
                        None => {
                            let msg = out_render
                                .error
                                .map(|e| e.message)
                                .unwrap_or_else(|| "diagram error".to_string());
                            out.push_str(&format!(
                                "<div class=\"mermaid-error\"><p>{}</p><pre>{}</pre></div>",
                                html_escape(&msg),
                                html_escape(&source),
                            ));
                        }
                    }
                    return;
                }
                out.push_str(&format!("<{html_tag}{attrs} />"));
                return;
            }

            out.push_str(&format!("<{html_tag}{attrs}>"));

            // Render children
            let len = el.len(txn);
            for i in 0..len {
                if let Some(child) = el.get(txn, i) {
                    render_node_html(txn, &child, out);
                }
            }

            out.push_str(&format!("</{html_tag}>"));
        }
        XmlOut::Text(text) => {
            render_text_html(txn, text, out);
        }
        _ => {}
    }
}

fn render_fragment_markdown<T: ReadTxn>(
    txn: &T,
    fragment: &yrs::XmlFragmentRef,
    out: &mut String,
    depth: usize,
) {
    let len = fragment.len(txn);
    for i in 0..len {
        if let Some(child) = fragment.get(txn, i) {
            render_node_markdown(txn, &child, out, depth);
        }
    }
}

fn render_node_markdown<T: ReadTxn>(txn: &T, node: &XmlOut, out: &mut String, depth: usize) {
    // #7: stop descending past the recursion cap (stack-overflow guard).
    // `depth` here is the list-indent level, which only grows on list
    // nesting; this guard counts true recursion across all node types.
    let Some(_rdepth) = DepthGuard::enter() else {
        return;
    };
    match node {
        XmlOut::Element(el) => {
            let tag = el.tag();
            let Some(node_type) = NodeType::from_tag(&tag) else {
                return;
            };

            match node_type {
                NodeType::Paragraph => {
                    render_children_markdown(txn, el, out, depth);
                    out.push_str("\n\n");
                }
                NodeType::Heading => {
                    let level = el
                        .get_attribute(txn, "level")
                        .and_then(|v| v.parse::<u8>().ok())
                        .unwrap_or(1)
                        .clamp(1, 6);
                    let prefix = "#".repeat(level as usize);
                    out.push_str(&format!("{prefix} "));
                    render_children_markdown(txn, el, out, depth);
                    out.push_str("\n\n");
                }
                NodeType::BulletList => {
                    render_list_items_markdown(txn, el, out, depth, "- ");
                }
                NodeType::OrderedList => {
                    render_list_items_markdown(txn, el, out, depth, "1. ");
                }
                NodeType::ListItem | NodeType::TaskItem => {
                    render_children_markdown(txn, el, out, depth);
                }
                NodeType::Blockquote => {
                    out.push_str("> ");
                    render_children_markdown(txn, el, out, depth);
                }
                NodeType::CodeBlock => {
                    let lang = el.get_attribute(txn, "language").unwrap_or_default();
                    out.push_str(&format!("```{lang}\n"));
                    render_children_markdown(txn, el, out, depth);
                    out.push_str("\n```\n\n");
                }
                NodeType::Mermaid => {
                    let source = el.get_attribute(txn, "source").unwrap_or_default();
                    out.push_str("```mermaid\n");
                    out.push_str(&source);
                    out.push_str("\n```\n\n");
                }
                NodeType::HorizontalRule => {
                    out.push_str("---\n\n");
                }
                NodeType::HardBreak => {
                    out.push_str("  \n");
                }
                NodeType::Image => {
                    let alt = el.get_attribute(txn, "alt").unwrap_or_default();
                    let src = el.get_attribute(txn, "src").unwrap_or_default();
                    if is_safe_url(&src) {
                        // #7: escape the label so a `]` in alt can't break out
                        // of the brackets and inject a second link, and encode
                        // parens/space in the URL so they can't terminate it.
                        out.push_str(&format!(
                            "![{}]({})",
                            escape_md_link_text(&alt),
                            escape_md_url(&src)
                        ));
                    }
                }
                NodeType::Embed => {
                    // M-P6 piece C — Markdown can't carry a real
                    // iframe, so the export emits a labelled link
                    // back to the embed source. Title falls back to
                    // the URL when absent so the link text is never
                    // empty / "[Embed: ]".
                    let url = el.get_attribute(txn, "url").unwrap_or_default();
                    if is_safe_url(&url) {
                        let label = el
                            .get_attribute(txn, "title")
                            .filter(|t| !t.is_empty())
                            .unwrap_or_else(|| url.clone());
                        // #7: same link-injection guard as the image arm.
                        out.push_str(&format!(
                            "[Embed: {}]({})\n\n",
                            escape_md_link_text(&label),
                            escape_md_url(&url)
                        ));
                    }
                }
                NodeType::Calendar => {
                    // #136 — placeholder line + numbered event list.
                    // The container renders its own placeholder then
                    // recurses into children (each CalendarEvent
                    // renders its own `- ({color}) ...` line via the
                    // arm below). Attr names live in the plugin
                    // module — see the equivalent comment in
                    // `render_html_attrs`.
                    let mut collected: std::collections::HashMap<String, String> =
                        std::collections::HashMap::new();
                    for name in crate::blocks::calendar::CALENDAR_ATTR_NAMES {
                        if let Some(v) = el.get_attribute(txn, name) {
                            collected.insert((*name).into(), v);
                        }
                    }
                    out.push_str(&crate::blocks::calendar::markdown_placeholder(
                        NodeType::Calendar,
                        &collected,
                    ));
                    render_children_markdown(txn, el, out, depth);
                    out.push('\n');
                }
                NodeType::CalendarEvent => {
                    let mut collected: std::collections::HashMap<String, String> =
                        std::collections::HashMap::new();
                    for name in crate::blocks::calendar::EVENT_ATTR_NAMES {
                        if let Some(v) = el.get_attribute(txn, name) {
                            collected.insert((*name).into(), v);
                        }
                    }
                    out.push_str(&crate::blocks::calendar::markdown_placeholder(
                        NodeType::CalendarEvent,
                        &collected,
                    ));
                }
                NodeType::Kanban => {
                    // #137 — bracket the board with horizontal-rule
                    // markers before and after the children walk so
                    // the columns render between two visible
                    // separators in the markdown export. The plugin
                    // module's `markdown_placeholder(Kanban, ...)`
                    // deliberately returns empty to avoid a duplicate
                    // divider here.
                    out.push_str("\n---\n\n");
                    render_children_markdown(txn, el, out, depth);
                    out.push_str("\n---\n\n");
                }
                NodeType::KanbanColumn => {
                    out.push_str(&crate::blocks::kanban::markdown_placeholder(
                        NodeType::KanbanColumn,
                        &collect_named_attrs(
                            txn,
                            el,
                            crate::blocks::kanban::COLUMN_ATTR_NAMES,
                        ),
                    ));
                    render_children_markdown(txn, el, out, depth);
                    out.push('\n');
                }
                NodeType::KanbanCard => {
                    out.push_str(&crate::blocks::kanban::markdown_placeholder(
                        NodeType::KanbanCard,
                        &collect_named_attrs(
                            txn,
                            el,
                            crate::blocks::kanban::CARD_ATTR_NAMES,
                        ),
                    ));
                }
                NodeType::Mention => {
                    // #148 slice 6 — Markdown has no native mention
                    // shape; emit the display attr verbatim (which
                    // already includes the `@` prefix). Downstream
                    // consumers that need the user_id can read it
                    // out of the HTML export instead.
                    let display = el.get_attribute(txn, "display").unwrap_or_default();
                    out.push_str(&display);
                }
                _ => {
                    render_children_markdown(txn, el, out, depth);
                }
            }
        }
        XmlOut::Text(text) => {
            render_text_markdown(txn, text, out);
        }
        _ => {}
    }
}

/// Copy a subset of a yrs XmlElement's attributes into a plain
/// HashMap for handing off to a plugin module's helper. yrs
/// XmlElementRef doesn't expose a whole-attribute iterator so
/// callers must name the attributes they want.
fn collect_named_attrs<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    names: &[&str],
) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for name in names {
        if let Some(v) = el.get_attribute(txn, name) {
            out.insert((*name).into(), v);
        }
    }
    out
}

fn render_children_markdown<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut String,
    depth: usize,
) {
    let len = el.len(txn);
    for i in 0..len {
        if let Some(child) = el.get(txn, i) {
            render_node_markdown(txn, &child, out, depth);
        }
    }
}

fn render_list_items_markdown<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut String,
    depth: usize,
    prefix: &str,
) {
    let indent = "  ".repeat(depth);
    let len = el.len(txn);
    for i in 0..len {
        if let Some(child) = el.get(txn, i) {
            out.push_str(&format!("{indent}{prefix}"));
            render_node_markdown(txn, &child, out, depth + 1);
            out.push('\n');
        }
    }
    if depth == 0 {
        out.push('\n');
    }
}

/// Resolve the HTML tag for a node type, handling headings dynamically.
fn resolve_html_tag<T: ReadTxn>(txn: &T, el: &yrs::XmlElementRef, nt: NodeType) -> String {
    match nt {
        NodeType::Heading => {
            let level = el
                .get_attribute(txn, "level")
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(1)
                .clamp(1, 6);
            format!("h{level}")
        }
        _ => node_type_to_html_tag(nt).to_string(),
    }
}

fn node_type_to_html_tag(nt: NodeType) -> &'static str {
    match nt {
        NodeType::Doc => "div",
        NodeType::Paragraph => "p",
        NodeType::Heading => "h1", // unreachable -- handled by resolve_html_tag
        NodeType::BulletList => "ul",
        NodeType::OrderedList => "ol",
        NodeType::ListItem => "li",
        NodeType::TaskList => "ul",
        NodeType::TaskItem => "li",
        NodeType::Blockquote => "blockquote",
        NodeType::CodeBlock => "pre",
        NodeType::HorizontalRule => "hr",
        NodeType::HardBreak => "br",
        NodeType::Image => "img",
        NodeType::Table => "table",
        NodeType::TableRow => "tr",
        NodeType::TableCell => "td",
        NodeType::TableHeader => "th",
        // M-P6 embeds: rendered as sandboxed iframes in HTML
        // export. The actual `src`, `sandbox`, `referrerpolicy`,
        // `loading` attributes are written by `render_html_attrs`;
        // here we just return the tag name.
        NodeType::Embed => "iframe",
        // #136 live-app blocks — Calendar/CalendarEvent tag lookup
        // delegates to the plugin module. See
        // design/live-app-blocks.md.
        NodeType::Calendar | NodeType::CalendarEvent => {
            crate::blocks::calendar::html_tag(nt)
        }
        // #137 live-app blocks — Kanban tree tag lookup.
        NodeType::Kanban | NodeType::KanbanColumn | NodeType::KanbanCard => {
            crate::blocks::kanban::html_tag(nt)
        }
        // #148 slice 6 — mention chip rendered as a span with
        // `class="mention"` and `data-user-id` (matches the
        // pre-existing text+MarkType::Mention HTML output so
        // round-trip through paste stays stable). The tag
        // itself is bare; attribute emission happens in
        // `render_html_attrs`, and the inner display text
        // comes from the node's `display` attr.
        NodeType::Mention => "span",
        NodeType::Mermaid => "div",
    }
}

/// Emit a highlighted code block. Returns false (emitting nothing)
/// when the language is unsupported/empty, the content isn't plain
/// text runs, or the text exceeds the highlight size cap — the
/// caller then falls through to the legacy generic path.
fn render_code_block_highlighted<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    out: &mut String,
) -> bool {
    let lang_attr = match el.get_attribute(txn, "language") {
        Some(l) if !l.is_empty() => l,
        _ => return false,
    };
    let Some(lang) = ogrenotes_highlight::Language::from_tag(&lang_attr) else {
        return false;
    };

    // Collect the block's text; bail on any non-text child.
    let mut text = String::new();
    let len = el.len(txn);
    for i in 0..len {
        match el.get(txn, i) {
            Some(XmlOut::Text(t)) => text.push_str(&t.get_string(txn)),
            Some(_) => return false,
            None => {}
        }
    }

    out.push_str(&format!(
        "<pre><code class=\"language-{}\">",
        html_escape_attr(&lang_attr)
    ));
    if text.chars().count() > ogrenotes_highlight::MAX_HIGHLIGHT_CHARS {
        out.push_str(&html_escape(&text));
    } else {
        for token in ogrenotes_highlight::highlight(&text, lang) {
            match ogrenotes_highlight::color_for(token.kind, false) {
                None => out.push_str(&html_escape(token.text)),
                Some(color) => out.push_str(&format!(
                    "<span style=\"color:{}\">{}</span>",
                    color,
                    html_escape(token.text)
                )),
            }
        }
    }
    out.push_str("</code></pre>");
    true
}

fn render_html_attrs<T: ReadTxn>(
    txn: &T,
    el: &yrs::XmlElementRef,
    node_type: NodeType,
) -> String {
    let mut attrs = String::new();

    match node_type {
        NodeType::Heading => {
            // Tag is handled by resolve_html_tag; no extra attrs needed
        }
        NodeType::CodeBlock => {
            if let Some(lang) = el.get_attribute(txn, "language") {
                attrs.push_str(&format!(" class=\"language-{}\"", html_escape_attr(&lang)));
            }
        }
        NodeType::Image => {
            if let Some(src) = el.get_attribute(txn, "src") {
                if is_safe_url(&src) {
                    attrs.push_str(&format!(" src=\"{}\"", html_escape_attr(&src)));
                }
            }
            if let Some(alt) = el.get_attribute(txn, "alt") {
                attrs.push_str(&format!(" alt=\"{}\"", html_escape_attr(&alt)));
            }
            if let Some(title) = el.get_attribute(txn, "title") {
                attrs.push_str(&format!(" title=\"{}\"", html_escape_attr(&title)));
            }
        }
        NodeType::TaskList => {
            attrs.push_str(" data-type=\"taskList\"");
        }
        NodeType::TaskItem => {
            let checked = el
                .get_attribute(txn, "checked")
                .map(|v| v == "true")
                .unwrap_or(false);
            attrs.push_str(&format!(" data-type=\"taskItem\" data-checked=\"{checked}\""));
        }
        NodeType::Embed => {
            // M-P6 piece C — HTML export emits the same sandboxed
            // iframe shape the editor view uses. `url` is the
            // iframe-ready src (already rewritten by the allowlist
            // at insert time, so no extra validation here beyond
            // the scheme check). `height` clamped [200, 1200] so a
            // corrupted attribute can't break the rendered page.
            if let Some(url) = el.get_attribute(txn, "url") {
                if is_safe_url(&url) {
                    attrs.push_str(&format!(" src=\"{}\"", html_escape_attr(&url)));
                }
            }
            if let Some(title) = el.get_attribute(txn, "title") {
                attrs.push_str(&format!(" title=\"{}\"", html_escape_attr(&title)));
            }
            let raw_height = el
                .get_attribute(txn, "height")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(400);
            let clamped = raw_height.clamp(200, 1200);
            attrs.push_str(&format!(
                " sandbox=\"allow-scripts allow-same-origin\" \
                 referrerpolicy=\"no-referrer\" loading=\"lazy\" \
                 frameborder=\"0\" width=\"100%\" height=\"{clamped}\""
            ));
        }
        NodeType::Calendar | NodeType::CalendarEvent => {
            // #136 — yrs XmlElementRef doesn't expose a whole-
            // attribute iterator, so we look up the attr names the
            // block declares. Attr names live in the plugin module
            // (single source of truth), not inline here — see
            // `blocks::calendar::{CALENDAR_ATTR_NAMES, EVENT_ATTR_NAMES}`.
            let names: &[&str] = if node_type == NodeType::Calendar {
                crate::blocks::calendar::CALENDAR_ATTR_NAMES
            } else {
                crate::blocks::calendar::EVENT_ATTR_NAMES
            };
            let mut collected: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for name in names {
                if let Some(v) = el.get_attribute(txn, name) {
                    collected.insert((*name).into(), v);
                }
            }
            attrs.push_str(&crate::blocks::calendar::html_attrs(node_type, &collected));
        }
        NodeType::Kanban | NodeType::KanbanColumn | NodeType::KanbanCard => {
            // #137 — same shape as the Calendar arm; the plugin
            // module owns the attr-name lists.
            let names: &[&str] = match node_type {
                NodeType::Kanban => crate::blocks::kanban::KANBAN_ATTR_NAMES,
                NodeType::KanbanColumn => crate::blocks::kanban::COLUMN_ATTR_NAMES,
                NodeType::KanbanCard => crate::blocks::kanban::CARD_ATTR_NAMES,
                _ => &[],
            };
            let mut collected: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for name in names {
                if let Some(v) = el.get_attribute(txn, name) {
                    collected.insert((*name).into(), v);
                }
            }
            attrs.push_str(&crate::blocks::kanban::html_attrs(node_type, &collected));
        }
        NodeType::Mention => {
            // #148 slice 6 — matches the pre-existing text+mark
            // HTML shape (`<span class="mention"
            // data-user-id="…">display</span>`) so clipboard
            // round-trip and existing stylesheets keep working.
            attrs.push_str(" class=\"mention\"");
            if let Some(uid) = el.get_attribute(txn, "user_id") {
                attrs.push_str(&format!(
                    " data-user-id=\"{}\"",
                    html_escape_attr(&uid),
                ));
            }
        }
        _ => {}
    }

    attrs
}

/// Check that a URL uses a safe protocol.
/// Blocks javascript: URIs, non-image data: URIs, and SVG data: URIs
/// (SVG can contain `<script>` tags and event handlers).
fn is_safe_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with('/')
        || lower.starts_with("data:image/png;")
        || lower.starts_with("data:image/jpeg;")
        || lower.starts_with("data:image/gif;")
        || lower.starts_with("data:image/webp;")
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Escape text placed inside a Markdown link/image label `[...]` so a
/// stray `]` (or newline) can't close the brackets early and inject
/// further markup. Backslash is escaped first so the inserted escapes
/// aren't themselves re-escaped. (#7)
fn escape_md_link_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace(['\n', '\r'], " ")
}

/// Encode the characters that would terminate a Markdown link target
/// inside `(...)` — parens and whitespace — so a crafted URL can't break
/// out and inject trailing markup. The URL has already passed
/// `is_safe_url`. (#7)
fn escape_md_url(s: &str) -> String {
    // Parens/whitespace would terminate the `(...)` target; quotes are
    // encoded as defence-in-depth so a downstream Markdown→HTML renderer
    // that interpolates the href into `href="…"` without its own escaping
    // can't be broken out of (the input is attacker-authorable). These
    // chars are not valid raw URL characters anyway.
    s.replace('(', "%28")
        .replace(')', "%29")
        .replace('"', "%22")
        .replace('\'', "%27")
        .replace([' ', '\t', '\n', '\r'], "%20")
}

/// Escape Markdown structural characters at line start to prevent
/// text content from being interpreted as Markdown structure.
fn escape_markdown_text(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for line in s.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#')
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("> ")
            || trimmed.starts_with("```")
            || trimmed.starts_with("---")
        {
            // Insert backslash before the structural character, after any leading whitespace
            let leading = line.len() - trimmed.len();
            result.push_str(&line[..leading]);
            result.push('\\');
            result.push_str(trimmed);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    // Remove trailing newline added by the loop
    if result.ends_with('\n') && !s.ends_with('\n') {
        result.pop();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::OgreDoc;
    use yrs::{Transact, WriteTxn, types::xml::{XmlElementPrelim, XmlTextPrelim}};
    use crate::schema::NodeType;

    // ── Helpers ────────────────────────────────────────────────────

    /// Build a doc with content populated by a closure.
    fn doc_with<F: FnOnce(&mut yrs::TransactionMut<'_>, &yrs::XmlFragmentRef)>(f: F) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            f(&mut txn, &fragment);
        }
        doc
    }

    /// Insert text into an XmlElement at position 0.
    fn insert_text(txn: &mut yrs::TransactionMut<'_>, el: &yrs::XmlElementRef, text: &str) {
        el.insert(txn, 0, XmlTextPrelim::new(text));
    }

    // ── Security / escaping (existing) ─────────────────────────────

    #[test]
    fn export_html_empty_doc() {
        let doc = OgreDoc::new();
        let html = to_html(doc.inner());
        assert!(html.contains("<p"));
    }

    #[test]
    fn export_markdown_empty_doc() {
        let doc = OgreDoc::new();
        let md = to_markdown(doc.inner());
        assert!(md.trim().is_empty() || md.contains('\n'));
    }

    #[test]
    fn html_escape_special_chars() {
        assert_eq!(
            html_escape("<script>alert('xss')</script>"),
            "&lt;script&gt;alert('xss')&lt;/script&gt;"
        );
    }

    #[test]
    fn html_escape_attr_quotes() {
        assert_eq!(
            html_escape_attr("value \"with\" quotes"),
            "value &quot;with&quot; quotes"
        );
        assert_eq!(
            html_escape_attr("it's here"),
            "it&#x27;s here"
        );
    }

    #[test]
    fn safe_url_allows_http() {
        assert!(is_safe_url("https://example.com/img.png"));
        assert!(is_safe_url("http://example.com/img.png"));
        assert!(is_safe_url("/images/photo.jpg"));
        assert!(is_safe_url("data:image/png;base64,abc123"));
        assert!(is_safe_url("data:image/jpeg;base64,abc123"));
        assert!(is_safe_url("data:image/gif;base64,abc123"));
        assert!(is_safe_url("data:image/webp;base64,abc123"));
    }

    #[test]
    fn safe_url_blocks_javascript() {
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("JAVASCRIPT:alert(1)"));
        assert!(!is_safe_url(" javascript:alert(1)"));
    }

    #[test]
    fn safe_url_blocks_data_non_image() {
        assert!(!is_safe_url("data:text/html,<script>alert(1)</script>"));
    }

    #[test]
    fn safe_url_blocks_svg_data_uri() {
        // SVG can contain <script> tags and event handlers — XSS vector
        assert!(!is_safe_url("data:image/svg+xml;base64,PHN2Zy..."));
        assert!(!is_safe_url("data:image/svg+xml,<svg onload='alert(1)'>"));
    }

    #[test]
    fn markdown_escape_structural_chars() {
        assert_eq!(escape_markdown_text("# Not a heading"), "\\# Not a heading");
        assert_eq!(escape_markdown_text("- Not a list"), "\\- Not a list");
        assert_eq!(escape_markdown_text("Normal text"), "Normal text");
        // Backslash must go before the structural char, not before leading whitespace
        assert_eq!(escape_markdown_text("  # indented"), "  \\# indented");
        assert_eq!(escape_markdown_text("   - indented list"), "   \\- indented list");
    }

    // ── HTML rendering tests ───────────────────────────────────────

    #[test]
    fn html_paragraph_with_text() {
        let doc = doc_with(|txn, frag| {
            let para = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &para, "Hello world");
        });
        let html = to_html(&doc);
        assert!(html.contains("<p>Hello world</p>"), "got: {html}");
    }

    #[test]
    fn html_heading_levels() {
        for level in 1..=6u8 {
            let doc = doc_with(|txn, frag| {
                let h = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Heading.tag_name()));
                h.insert_attribute(txn, "level", level.to_string());
                insert_text(txn, &h, "Title");
            });
            let html = to_html(&doc);
            let open = format!("<h{level}>Title</h{level}>");
            assert!(html.contains(&open), "level {level}: got {html}");
        }
    }

    #[test]
    fn html_heading_defaults_to_h1() {
        let doc = doc_with(|txn, frag| {
            let h = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Heading.tag_name()));
            // No "level" attribute set
            insert_text(txn, &h, "No level");
        });
        let html = to_html(&doc);
        assert!(html.contains("<h1>No level</h1>"), "got: {html}");
    }

    #[test]
    fn html_bullet_list() {
        let doc = doc_with(|txn, frag| {
            let ul = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::BulletList.tag_name()));
            let li = ul.insert(txn, 0, XmlElementPrelim::empty(NodeType::ListItem.tag_name()));
            let p = li.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Item 1");
        });
        let html = to_html(&doc);
        assert!(html.contains("<ul>"), "got: {html}");
        assert!(html.contains("<li>"), "got: {html}");
        assert!(html.contains("Item 1"), "got: {html}");
        assert!(html.contains("</li>"), "got: {html}");
        assert!(html.contains("</ul>"), "got: {html}");
    }

    #[test]
    fn html_ordered_list() {
        let doc = doc_with(|txn, frag| {
            let ol = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::OrderedList.tag_name()));
            let li = ol.insert(txn, 0, XmlElementPrelim::empty(NodeType::ListItem.tag_name()));
            let p = li.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Step 1");
        });
        let html = to_html(&doc);
        assert!(html.contains("<ol>"), "got: {html}");
        assert!(html.contains("Step 1"), "got: {html}");
    }

    #[test]
    fn html_blockquote() {
        let doc = doc_with(|txn, frag| {
            let bq = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Blockquote.tag_name()));
            let p = bq.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Quoted text");
        });
        let html = to_html(&doc);
        assert!(html.contains("<blockquote>"), "got: {html}");
        assert!(html.contains("Quoted text"), "got: {html}");
        assert!(html.contains("</blockquote>"), "got: {html}");
    }

    // Updated for syntax-highlighted export (2026-07-09 spec): token spans split the literal text; class moved to the <code> wrapper.
    #[test]
    fn html_code_block_with_language() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "rust");
            insert_text(txn, &cb, "fn main() {}");
        });
        let html = to_html(&doc);
        assert!(html.contains("class=\"language-rust\""), "got: {html}");
        assert!(html.contains("main"), "got: {html}");
    }

    #[test]
    fn html_horizontal_rule() {
        let doc = doc_with(|txn, frag| {
            frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::HorizontalRule.tag_name()));
        });
        let html = to_html(&doc);
        assert!(html.contains("<hr"), "got: {html}");
    }

    #[test]
    fn html_image_with_attrs() {
        let doc = doc_with(|txn, frag| {
            let img = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img.insert_attribute(txn, "src", "https://example.com/photo.jpg");
            img.insert_attribute(txn, "alt", "A photo");
            img.insert_attribute(txn, "title", "Photo title");
        });
        let html = to_html(&doc);
        assert!(html.contains("src=\"https://example.com/photo.jpg\""), "got: {html}");
        assert!(html.contains("alt=\"A photo\""), "got: {html}");
        assert!(html.contains("title=\"Photo title\""), "got: {html}");
    }

    #[test]
    fn html_image_blocks_unsafe_src() {
        let doc = doc_with(|txn, frag| {
            let img = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img.insert_attribute(txn, "src", "javascript:alert(1)");
            img.insert_attribute(txn, "alt", "bad");
        });
        let html = to_html(&doc);
        assert!(!html.contains("javascript:"), "unsafe src in output: {html}");
    }

    #[test]
    fn html_embed_renders_sandboxed_iframe() {
        let doc = doc_with(|txn, frag| {
            let em = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Embed.tag_name()));
            em.insert_attribute(txn, "url", "https://www.youtube.com/embed/dQw4w9WgXcQ");
            em.insert_attribute(txn, "provider", "youtube");
            em.insert_attribute(txn, "height", "315");
            em.insert_attribute(txn, "title", "Demo video");
        });
        let html = to_html(&doc);
        assert!(html.contains("<iframe"), "got: {html}");
        assert!(html.contains("src=\"https://www.youtube.com/embed/dQw4w9WgXcQ\""), "got: {html}");
        assert!(
            html.contains("sandbox=\"allow-scripts allow-same-origin\""),
            "got: {html}"
        );
        assert!(html.contains("referrerpolicy=\"no-referrer\""), "got: {html}");
        assert!(html.contains("loading=\"lazy\""), "got: {html}");
        assert!(html.contains("title=\"Demo video\""), "got: {html}");
        assert!(html.contains("height=\"315\""), "got: {html}");
    }

    #[test]
    fn html_embed_clamps_extreme_height() {
        let doc = doc_with(|txn, frag| {
            let em = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Embed.tag_name()));
            em.insert_attribute(txn, "url", "https://player.vimeo.com/video/76979871");
            em.insert_attribute(txn, "height", "99999"); // out-of-range
        });
        let html = to_html(&doc);
        // Clamps to 1200, the upper bound.
        assert!(html.contains("height=\"1200\""), "got: {html}");
        assert!(!html.contains("99999"), "got: {html}");
    }

    #[test]
    fn html_calendar_event_writes_content_between_tags() {
        // #136 review-fix regression guard: CalendarEvent is a
        // leaf, but its display text lives in `content` and must
        // ride between the span tags — not self-close and lose
        // the title.
        let doc = doc_with(|txn, frag| {
            let cal = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Calendar.tag_name()));
            cal.insert_attribute(txn, "view", "month");
            cal.insert_attribute(txn, "timezone", "UTC");
            let ev = cal.insert(
                txn,
                0,
                XmlElementPrelim::empty(NodeType::CalendarEvent.tag_name()),
            );
            ev.insert_attribute(txn, "color", "blue");
            ev.insert_attribute(txn, "allDay", "true");
            ev.insert_attribute(txn, "startDate", "2026-07-15");
            ev.insert_attribute(txn, "endDate", "2026-07-15");
            ev.insert_attribute(txn, "content", "Team standup");
        });
        let html = to_html(&doc);
        assert!(
            html.contains(">Team standup</span>"),
            "content missing from HTML export; got: {html}"
        );
        assert!(
            !html.contains("<span class=\"calendar-event calendar-event--blue\" data-all-day=\"true\" data-start-date=\"2026-07-15\" data-end-date=\"2026-07-15\" />"),
            "CalendarEvent must NOT self-close; got: {html}"
        );
    }

    #[test]
    fn html_calendar_event_escapes_content() {
        // Regression guard: content is HTML-escaped so a rogue
        // event title cannot inject markup into the export.
        let doc = doc_with(|txn, frag| {
            let cal = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Calendar.tag_name()));
            let ev = cal.insert(
                txn,
                0,
                XmlElementPrelim::empty(NodeType::CalendarEvent.tag_name()),
            );
            ev.insert_attribute(txn, "color", "red");
            ev.insert_attribute(txn, "allDay", "true");
            ev.insert_attribute(txn, "startDate", "2026-07-15");
            ev.insert_attribute(txn, "content", "<script>alert(1)</script>");
        });
        let html = to_html(&doc);
        assert!(!html.contains("<script>"), "unescaped script tag; got: {html}");
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "expected escaped content; got: {html}"
        );
    }

    #[test]
    fn html_embed_drops_unsafe_url() {
        let doc = doc_with(|txn, frag| {
            let em = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Embed.tag_name()));
            em.insert_attribute(txn, "url", "javascript:alert(1)");
        });
        let html = to_html(&doc);
        // Iframe still emits (it's a structural element of the doc),
        // but the unsafe src is stripped via `is_safe_url`.
        assert!(html.contains("<iframe"), "got: {html}");
        assert!(!html.contains("javascript:"), "got: {html}");
        assert!(!html.contains("src=\""), "got: {html}");
    }

    #[test]
    fn html_task_list() {
        let doc = doc_with(|txn, frag| {
            let tl = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::TaskList.tag_name()));
            let ti = tl.insert(txn, 0, XmlElementPrelim::empty(NodeType::TaskItem.tag_name()));
            ti.insert_attribute(txn, "checked", "true");
            let p = ti.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Done task");
        });
        let html = to_html(&doc);
        assert!(html.contains("data-type=\"taskList\""), "got: {html}");
        assert!(html.contains("data-type=\"taskItem\""), "got: {html}");
        assert!(html.contains("data-checked=\"true\""), "got: {html}");
    }

    #[test]
    fn html_table() {
        let doc = doc_with(|txn, frag| {
            let table = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let row = table.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let cell = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p = cell.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Cell 1");
        });
        let html = to_html(&doc);
        assert!(html.contains("<table>"), "got: {html}");
        assert!(html.contains("<tr>"), "got: {html}");
        assert!(html.contains("<td>"), "got: {html}");
        assert!(html.contains("Cell 1"), "got: {html}");
    }

    #[test]
    fn html_table_header() {
        let doc = doc_with(|txn, frag| {
            let table = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let row = table.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let th = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableHeader.tag_name()));
            let p = th.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Header");
        });
        let html = to_html(&doc);
        assert!(html.contains("<th>"), "got: {html}");
        assert!(html.contains("Header"), "got: {html}");
    }

    #[test]
    fn html_escapes_text_content() {
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "a < b & c > d");
        });
        let html = to_html(&doc);
        assert!(html.contains("a &lt; b &amp; c &gt; d"), "got: {html}");
    }

    // ── Markdown rendering tests ───────────────────────────────────

    #[test]
    fn markdown_paragraph() {
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Simple paragraph");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("Simple paragraph"), "got: {md}");
        assert!(md.contains("\n\n"), "paragraph should end with double newline, got: {md}");
    }

    #[test]
    fn markdown_heading_levels() {
        for level in 1..=3u8 {
            let doc = doc_with(|txn, frag| {
                let h = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Heading.tag_name()));
                h.insert_attribute(txn, "level", level.to_string());
                insert_text(txn, &h, "Title");
            });
            let md = to_markdown(&doc);
            let prefix = "#".repeat(level as usize);
            assert!(md.contains(&format!("{prefix} Title")), "level {level}: got {md}");
        }
    }

    #[test]
    fn markdown_bullet_list() {
        let doc = doc_with(|txn, frag| {
            let ul = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::BulletList.tag_name()));
            let li = ul.insert(txn, 0, XmlElementPrelim::empty(NodeType::ListItem.tag_name()));
            let p = li.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Bullet");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("- "), "got: {md}");
        assert!(md.contains("Bullet"), "got: {md}");
    }

    #[test]
    fn markdown_ordered_list() {
        let doc = doc_with(|txn, frag| {
            let ol = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::OrderedList.tag_name()));
            let li = ol.insert(txn, 0, XmlElementPrelim::empty(NodeType::ListItem.tag_name()));
            let p = li.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Step");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("1. "), "got: {md}");
        assert!(md.contains("Step"), "got: {md}");
    }

    #[test]
    fn markdown_blockquote() {
        let doc = doc_with(|txn, frag| {
            let bq = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Blockquote.tag_name()));
            let p = bq.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Quoted");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("> "), "got: {md}");
        assert!(md.contains("Quoted"), "got: {md}");
    }

    #[test]
    fn markdown_code_block() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "python");
            insert_text(txn, &cb, "print('hi')");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("```python\n"), "got: {md}");
        assert!(md.contains("print('hi')"), "got: {md}");
        assert!(md.contains("\n```"), "got: {md}");
    }

    #[test]
    fn markdown_code_block_no_language() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            insert_text(txn, &cb, "code");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("```\n"), "got: {md}");
    }

    #[test]
    fn markdown_horizontal_rule() {
        let doc = doc_with(|txn, frag| {
            frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::HorizontalRule.tag_name()));
        });
        let md = to_markdown(&doc);
        assert!(md.contains("---"), "got: {md}");
    }

    #[test]
    fn markdown_hard_break() {
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Line one");
            p.insert(txn, 1, XmlElementPrelim::empty(NodeType::HardBreak.tag_name()));
        });
        let md = to_markdown(&doc);
        assert!(md.contains("  \n"), "hard break should produce trailing spaces, got: {md}");
    }

    #[test]
    fn markdown_image() {
        let doc = doc_with(|txn, frag| {
            let img = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img.insert_attribute(txn, "src", "https://example.com/img.png");
            img.insert_attribute(txn, "alt", "Photo");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("![Photo](https://example.com/img.png)"), "got: {md}");
    }

    #[test]
    fn markdown_image_blocks_unsafe_src() {
        let doc = doc_with(|txn, frag| {
            let img = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img.insert_attribute(txn, "src", "javascript:alert(1)");
            img.insert_attribute(txn, "alt", "bad");
        });
        let md = to_markdown(&doc);
        assert!(!md.contains("javascript:"), "unsafe src in markdown: {md}");
    }

    #[test]
    fn markdown_embed_emits_labelled_link() {
        let doc = doc_with(|txn, frag| {
            let em = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Embed.tag_name()));
            em.insert_attribute(txn, "url", "https://www.youtube.com/embed/dQw4w9WgXcQ");
            em.insert_attribute(txn, "title", "Demo video");
        });
        let md = to_markdown(&doc);
        assert!(
            md.contains("[Embed: Demo video](https://www.youtube.com/embed/dQw4w9WgXcQ)"),
            "got: {md}",
        );
    }

    #[test]
    fn markdown_embed_falls_back_to_url_when_no_title() {
        let doc = doc_with(|txn, frag| {
            let em = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Embed.tag_name()));
            em.insert_attribute(txn, "url", "https://player.vimeo.com/video/76979871");
        });
        let md = to_markdown(&doc);
        assert!(
            md.contains("[Embed: https://player.vimeo.com/video/76979871]"),
            "got: {md}",
        );
    }

    #[test]
    fn markdown_embed_blocks_unsafe_url() {
        let doc = doc_with(|txn, frag| {
            let em = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Embed.tag_name()));
            em.insert_attribute(txn, "url", "javascript:alert(1)");
        });
        let md = to_markdown(&doc);
        assert!(!md.contains("javascript:"), "got: {md}");
        // The embed paragraph is fully suppressed when its URL is
        // unsafe — the markdown export refuses to emit a link to
        // an attacker-controlled scheme even with a "[Embed: ...]"
        // wrapper around it.
        assert!(!md.contains("[Embed:"), "got: {md}");
    }

    // ── CSV export tests ───────────────────────────────────────────

    #[test]
    fn csv_simple_table() {
        let doc = doc_with(|txn, frag| {
            let table = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let row = table.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let c1 = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p1 = c1.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p1, "A");
            let c2 = row.insert(txn, 1, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p2 = c2.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p2, "B");
        });
        let csv = to_csv(&doc);
        assert_eq!(csv.trim(), "A,B");
    }

    #[test]
    fn csv_quotes_commas() {
        let doc = doc_with(|txn, frag| {
            let table = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let row = table.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let c1 = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p1 = c1.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p1, "hello, world");
        });
        let csv = to_csv(&doc);
        assert_eq!(csv.trim(), "\"hello, world\"");
    }

    #[test]
    fn csv_multiple_rows() {
        let doc = doc_with(|txn, frag| {
            let table = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            for val in &["R1", "R2"] {
                let row = table.insert(txn, table.len(txn), XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
                let c = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
                let p = c.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
                insert_text(txn, &p, val);
            }
        });
        let csv = to_csv(&doc);
        let lines: Vec<&str> = csv.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "R1");
        assert_eq!(lines[1], "R2");
    }

    #[test]
    fn csv_no_table_returns_empty() {
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "No table here");
        });
        let csv = to_csv(&doc);
        assert!(csv.is_empty());
    }

    #[test]
    fn csv_only_first_table() {
        let doc = doc_with(|txn, frag| {
            // Table 1
            let t1 = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let r1 = t1.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let c1 = r1.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p1 = c1.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p1, "First");
            // Table 2
            let t2 = frag.insert(txn, 1, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let r2 = t2.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let c2 = r2.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p2 = c2.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p2, "Second");
        });
        let csv = to_csv(&doc);
        assert!(csv.contains("First"));
        assert!(!csv.contains("Second"));
    }

    // ── extract_text tests ─────────────────────────────────────────

    #[test]
    fn extract_text_nested() {
        // Build a table cell containing a paragraph with text
        let doc = doc_with(|txn, frag| {
            let cell = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p = cell.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "nested text");
        });

        let txn = doc.transact();
        let frag = txn.get_xml_fragment("content").unwrap();
        if let Some(yrs::types::xml::XmlOut::Element(el)) = frag.get(&txn, 0) {
            assert_eq!(extract_text(&txn, &el), "nested text");
        } else {
            panic!("expected element");
        }
    }

    // ── Early-return / no content fragment ──────────────────────────

    #[test]
    fn html_no_content_fragment() {
        let doc = Doc::new(); // raw Doc, no "content" fragment
        assert_eq!(to_html(&doc), "");
    }

    #[test]
    fn markdown_no_content_fragment() {
        let doc = Doc::new();
        assert_eq!(to_markdown(&doc), "");
    }

    #[test]
    fn csv_no_content_fragment() {
        let doc = Doc::new();
        assert_eq!(to_csv(&doc), "");
    }

    // ── Unknown XML tags ───────────────────────────────────────────

    #[test]
    fn html_unknown_tag_skipped() {
        let doc = doc_with(|txn, frag| {
            frag.insert(txn, 0, XmlElementPrelim::empty("unknown_widget"));
            let p = frag.insert(txn, 1, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "visible");
        });
        let html = to_html(&doc);
        assert!(!html.contains("unknown_widget"), "unknown tag should be skipped: {html}");
        assert!(html.contains("visible"), "known content should render: {html}");
    }

    #[test]
    fn markdown_unknown_tag_skipped() {
        let doc = doc_with(|txn, frag| {
            frag.insert(txn, 0, XmlElementPrelim::empty("unknown_widget"));
            let p = frag.insert(txn, 1, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "visible");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("visible"), "known content should render: {md}");
    }

    // ── Markdown: table falls through to default arm ───────────────

    #[test]
    fn markdown_table_renders_cell_text() {
        let doc = doc_with(|txn, frag| {
            let table = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let row = table.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let cell = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p = cell.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Cell content");
        });
        let md = to_markdown(&doc);
        // Table/TableRow/TableCell hit the _ => default arm, which recurses into children
        assert!(md.contains("Cell content"), "got: {md}");
    }

    // ── HTML: HardBreak as leaf ────────────────────────────────────

    #[test]
    fn html_hard_break() {
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Before");
            p.insert(txn, 1, XmlElementPrelim::empty(NodeType::HardBreak.tag_name()));
            p.insert(txn, 2, XmlTextPrelim::new("After"));
        });
        let html = to_html(&doc);
        assert!(html.contains("<br"), "hard break should render as <br: {html}");
    }

    // ── HTML: code block without language ───────────────────────────

    #[test]
    fn html_code_block_no_language() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            insert_text(txn, &cb, "plain code");
        });
        let html = to_html(&doc);
        assert!(html.contains("<pre>"), "got: {html}");
        assert!(!html.contains("class="), "no language class without attr: {html}");
        assert!(html.contains("plain code"), "got: {html}");
    }

    #[test]
    fn html_code_block_highlights_supported_language() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "rust");
            insert_text(txn, &cb, "fn main() {}");
        });
        let html = to_html(&doc);
        assert!(html.contains("<pre><code class=\"language-rust\">"), "got: {html}");
        // `fn` is a keyword token with the light-palette keyword color.
        assert!(
            html.contains("<span style=\"color:#cf222e\">fn</span>"),
            "got: {html}"
        );
        assert!(html.contains("</code></pre>"), "got: {html}");
        // Reassembling the visible text must reproduce the source.
        assert!(html.contains("main"), "got: {html}");
    }

    #[test]
    fn html_code_block_escapes_hostile_content_per_token() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "rust");
            insert_text(txn, &cb, "let x = \"</code><script>alert(1)</script>\";");
        });
        let html = to_html(&doc);
        assert!(!html.contains("<script>"), "raw script must never appear: {html}");
        assert!(html.contains("&lt;script&gt;"), "got: {html}");
    }

    #[test]
    fn html_code_block_unknown_language_keeps_legacy_shape() {
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "mermaid");
            insert_text(txn, &cb, "pie title x");
        });
        let html = to_html(&doc);
        // Exactly today's output: class on <pre>, no <code>, no spans.
        assert!(html.contains("<pre class=\"language-mermaid\">"), "got: {html}");
        assert!(!html.contains("<code"), "got: {html}");
        assert!(!html.contains("tok-"), "got: {html}");
    }

    #[test]
    fn html_code_block_oversized_renders_unhighlighted() {
        let big = "x".repeat(ogrenotes_highlight::MAX_HIGHLIGHT_CHARS + 1);
        let doc = doc_with(|txn, frag| {
            let cb = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::CodeBlock.tag_name()));
            cb.insert_attribute(txn, "language", "rust");
            insert_text(txn, &cb, &big);
        });
        let html = to_html(&doc);
        assert!(html.contains("<pre><code class=\"language-rust\">"), "got: {html}");
        assert!(!html.contains("<span"), "no spans over the size cap: {html}");
    }

    // ── HTML: task item unchecked ───────────────────────────────────

    #[test]
    fn html_task_item_unchecked() {
        let doc = doc_with(|txn, frag| {
            let tl = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::TaskList.tag_name()));
            let ti = tl.insert(txn, 0, XmlElementPrelim::empty(NodeType::TaskItem.tag_name()));
            // No "checked" attribute set → defaults to false
            let p = ti.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Pending");
        });
        let html = to_html(&doc);
        assert!(html.contains("data-checked=\"false\""), "got: {html}");
    }

    // ── HTML: image without optional attrs ─────────────────────────

    #[test]
    fn html_image_src_only() {
        let doc = doc_with(|txn, frag| {
            let img = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img.insert_attribute(txn, "src", "https://example.com/x.png");
            // No alt, no title
        });
        let html = to_html(&doc);
        assert!(html.contains("src="), "got: {html}");
        assert!(!html.contains("alt="), "no alt attr when missing: {html}");
        assert!(!html.contains("title="), "no title attr when missing: {html}");
    }

    // ── CSV: cell with embedded quotes ─────────────────────────────

    #[test]
    fn csv_quotes_in_cell() {
        let doc = doc_with(|txn, frag| {
            let table = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let row = table.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let c = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p = c.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "say \"hello\"");
        });
        let csv = to_csv(&doc);
        // Quotes should be doubled and cell wrapped in quotes
        assert_eq!(csv.trim(), "\"say \"\"hello\"\"\"");
    }

    // ── csv_encode_cell: formula-injection guard ────────────────────

    #[test]
    fn csv_encode_neutralizes_formula_prefix() {
        // Classic CSV-injection payload. A spreadsheet app would execute
        // HYPERLINK on open; the quote prefix makes it render as literal text.
        assert_eq!(
            csv_encode_cell("=HYPERLINK(\"https://evil\",\"x\")"),
            // contains `"` and `,` → wrapped in quotes, embedded quotes doubled
            "\"'=HYPERLINK(\"\"https://evil\"\",\"\"x\"\")\""
        );
        assert_eq!(csv_encode_cell("+cmd|' /c calc'!A0"), "'+cmd|' /c calc'!A0");
        assert_eq!(csv_encode_cell("-2+3"), "'-2+3");
        assert_eq!(csv_encode_cell("@SUM(A1:A9)"), "'@SUM(A1:A9)");
    }

    #[test]
    fn csv_encode_guards_tab_and_cr_prefixes() {
        // Excel also honours TAB and CR as command prefixes.
        // TAB doesn't trigger RFC-4180 quoting on its own, CR does.
        assert_eq!(csv_encode_cell("\t=CMD"), "'\t=CMD");
        assert_eq!(csv_encode_cell("\rbad"), "\"'\rbad\"");
    }

    #[test]
    fn csv_encode_leaves_safe_cells_alone() {
        // Content that merely contains `=` but doesn't start with it is safe.
        assert_eq!(csv_encode_cell("a=b"), "a=b");
        assert_eq!(csv_encode_cell("hello"), "hello");
        assert_eq!(csv_encode_cell(""), "");
        // Leading digit, negative number via word, etc. — safe.
        assert_eq!(csv_encode_cell("1=2"), "1=2");
    }

    #[test]
    fn csv_encode_combines_formula_guard_with_quoting() {
        // A formula-like cell that also contains a comma must get BOTH
        // the `'` prefix and RFC-4180 quoting.
        assert_eq!(csv_encode_cell("=A1,B1"), "\"'=A1,B1\"");
        // A formula-like cell with embedded double-quote.
        assert_eq!(csv_encode_cell("=\"x\""), "\"'=\"\"x\"\"\"");
    }

    #[test]
    fn csv_to_csv_rejects_formula_injection_end_to_end() {
        // End-to-end: a user-typed formula in a table cell must not leak
        // into the CSV output as an executable formula.
        let doc = doc_with(|txn, frag| {
            let table = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let row = table.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let c = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let p = c.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "=1+1");
        });
        let csv = to_csv(&doc);
        assert!(csv.starts_with("'="), "formula prefix must be guarded: {csv:?}");
    }

    // ── node_type_to_html_tag: Doc node ────────────────────────────

    #[test]
    fn html_doc_node_renders_as_div() {
        // Exercising the Doc arm in node_type_to_html_tag
        assert_eq!(node_type_to_html_tag(NodeType::Doc), "div");
        assert_eq!(node_type_to_html_tag(NodeType::HardBreak), "br");
        assert_eq!(node_type_to_html_tag(NodeType::Heading), "h1");
    }

    // ── render_html_attrs: paragraph (hits _ => {} arm) ────────────

    #[test]
    fn html_paragraph_has_no_special_attrs() {
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Just text");
        });
        let html = to_html(&doc);
        // Paragraph should render as <p> with no extra attributes
        assert!(html.contains("<p>Just text</p>"), "got: {html}");
    }

    // ── Nested bullet list in markdown (depth > 0) ─────────────────

    #[test]
    fn markdown_nested_list() {
        let doc = doc_with(|txn, frag| {
            let ul = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::BulletList.tag_name()));
            let li = ul.insert(txn, 0, XmlElementPrelim::empty(NodeType::ListItem.tag_name()));
            let p = li.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "Parent");
            // Nested list
            let inner_ul = li.insert(txn, 1, XmlElementPrelim::empty(NodeType::BulletList.tag_name()));
            let inner_li = inner_ul.insert(txn, 0, XmlElementPrelim::empty(NodeType::ListItem.tag_name()));
            let inner_p = inner_li.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &inner_p, "Child");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("- "), "got: {md}");
        assert!(md.contains("  - "), "nested list should be indented: {md}");
        assert!(md.contains("Parent"), "got: {md}");
        assert!(md.contains("Child"), "got: {md}");
    }

    // ── #7: export hardening ───────────────────────────────────────

    #[test]
    fn markdown_image_escapes_link_injection() {
        // #7d: a `]` in alt or `)` in src must not break out of the
        // `![..](..)` syntax and inject a second link.
        let doc = doc_with(|txn, frag| {
            let img = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img.insert_attribute(txn, "src", "https://e.com/a(b).png");
            img.insert_attribute(txn, "alt", "x](javascript:alert(1)) ![");
        });
        let md = to_markdown(&doc);
        assert!(md.contains("x\\]"), "alt ] must be backslash-escaped: {md}");
        assert!(md.contains("\\["), "alt [ must be backslash-escaped: {md}");
        assert!(
            md.contains("a%28b%29"),
            "src parens must be percent-encoded so they can't close the link: {md}"
        );
    }

    #[test]
    fn export_recursion_is_depth_capped() {
        // #7b: a maliciously deep element tree must not overflow the
        // stack. Nest past the cap with a marker at the bottom; the
        // traversal must return (no panic) and never reach the marker.
        let depth = MAX_EXPORT_DEPTH + 50;
        let doc = doc_with(|txn, frag| {
            let mut current =
                frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Blockquote.tag_name()));
            for _ in 0..depth {
                current = current
                    .insert(txn, 0, XmlElementPrelim::empty(NodeType::Blockquote.tag_name()));
            }
            let p = current.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            insert_text(txn, &p, "DEEP_MARKER");
        });
        let html = to_html(&doc);
        let md = to_markdown(&doc);
        let plain = to_plain_text(&doc);
        assert!(!html.contains("DEEP_MARKER"), "html must cap recursion: {}", html.len());
        assert!(!md.contains("DEEP_MARKER"), "markdown must cap recursion: {}", md.len());
        assert!(!plain.contains("DEEP_MARKER"), "plain-text (extract_text) must cap recursion");
    }

    #[test]
    fn export_renders_inline_marks() {
        // #7a: bold/italic on a run must survive into HTML and Markdown
        // (previously dropped — exported as plain text). Tag order is
        // fixed by the renderer, so the output is deterministic.
        use std::sync::Arc;
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            let t = p.insert(txn, 0, XmlTextPrelim::new("hi"));
            let mut attrs = Attrs::new();
            attrs.insert(Arc::from("bold"), Any::Bool(true));
            attrs.insert(Arc::from("italic"), Any::Bool(true));
            t.format(txn, 0, 2, attrs);
        });
        assert!(
            to_html(&doc).contains("<strong><em>hi</em></strong>"),
            "html: {}",
            to_html(&doc)
        );
        assert!(to_markdown(&doc).contains("_**hi**_"), "md: {}", to_markdown(&doc));
    }

    #[test]
    fn export_renders_safe_link_mark() {
        // #7a: a link mark renders as <a>/[..](..) with the href
        // scheme-checked and attribute-escaped.
        use std::sync::Arc;
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            let t = p.insert(txn, 0, XmlTextPrelim::new("click"));
            let mut attrs = Attrs::new();
            attrs.insert(
                Arc::from("link"),
                Any::String(Arc::from(r#"{"href":"https://ok.example/x"}"#)),
            );
            t.format(txn, 0, 5, attrs);
        });
        assert!(
            to_html(&doc).contains(r#"<a href="https://ok.example/x">click</a>"#),
            "html: {}",
            to_html(&doc)
        );
        assert!(
            to_markdown(&doc).contains("[click](https://ok.example/x)"),
            "md: {}",
            to_markdown(&doc)
        );
    }

    #[test]
    fn export_renders_mention_mark_with_user_id() {
        // #148: a `mention` mark (text carrying `MarkType::Mention`)
        // survives HTML + Markdown export with the user_id and
        // `.mention` class intact so paste-import (via
        // `frontend/src/editor/clipboard.rs::tag_to_mark`) can
        // rehydrate the mark. Before this fix export silently
        // dropped the mark and only the plain display name survived.
        use std::sync::Arc;
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            let t = p.insert(txn, 0, XmlTextPrelim::new("Alice"));
            let mut attrs = Attrs::new();
            attrs.insert(
                Arc::from("mention"),
                Any::String(Arc::from(r#"{"user_id":"u-42"}"#)),
            );
            t.format(txn, 0, 5, attrs);
        });
        let html = to_html(&doc);
        assert!(
            html.contains(r#"<span class="mention" data-user-id="u-42">Alice</span>"#),
            "html: {html}"
        );
        let md = to_markdown(&doc);
        assert!(
            md.contains(r#"<span class="mention" data-user-id="u-42">Alice</span>"#),
            "md: {md}"
        );
    }

    #[test]
    fn export_renders_mention_node_bytewise_matches_mark_output() {
        // #148 slice 6 — a `NodeType::Mention` leaf renders the
        // same `<span class="mention" data-user-id="…">Display</span>`
        // HTML the legacy text+MarkType::Mention pair produced.
        // Byte-match guarantees paste round-trip through
        // `clipboard.rs::parse_from_html` stays stable across the
        // migration; downstream stylesheets and tests keyed to the
        // exact tag shape don't need to change.
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            let m = p.insert(txn, 0, XmlElementPrelim::empty(NodeType::Mention.tag_name()));
            m.insert_attribute(txn, "user_id", "u-42");
            m.insert_attribute(txn, "display", "Alice");
        });
        let html = to_html(&doc);
        assert!(
            html.contains(r#"<span class="mention" data-user-id="u-42">Alice</span>"#),
            "html: {html}"
        );
        // Markdown emits the display verbatim (no wire shape;
        // the user_id survives through the HTML export).
        let md = to_markdown(&doc);
        assert!(md.contains("Alice"), "md: {md}");
    }

    #[test]
    fn extract_text_walks_mention_node_as_display() {
        // #148 slice 6 — `extract_text` (used by the LLM prompt
        // embed path in ask.rs::execute_get_document) sees a
        // Mention node's `display` attr rather than an empty
        // gap. Matches the frontend `Node::text_content` behavior.
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            let _ = p.insert(txn, 0, XmlTextPrelim::new("Hi "));
            let m = p.insert(txn, 1, XmlElementPrelim::empty(NodeType::Mention.tag_name()));
            m.insert_attribute(txn, "user_id", "u-1");
            m.insert_attribute(txn, "display", "@alice");
        });
        let txn = doc.transact();
        let frag = txn.get_xml_fragment("content").expect("content fragment");
        let mut text = String::new();
        for i in 0..frag.len(&txn) {
            if let Some(yrs::types::xml::XmlOut::Element(el)) = frag.get(&txn, i) {
                text.push_str(&extract_text(&txn, &el));
            }
        }
        assert_eq!(text, "Hi @alice");
    }

    #[test]
    fn export_link_label_does_not_double_escape_structural_char() {
        // #7a review: a run starting with a Markdown structural char (`#`)
        // that also carries a link mark must render the label as
        // `[# heading]`, not `[\# heading]` — the link label is inline
        // context, so escape_markdown_text's block-level backslash must not
        // be applied (and then doubled by escape_md_link_text).
        use std::sync::Arc;
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            let t = p.insert(txn, 0, XmlTextPrelim::new("# heading"));
            let mut attrs = Attrs::new();
            attrs.insert(
                Arc::from("link"),
                Any::String(Arc::from(r#"{"href":"https://example.com/"}"#)),
            );
            t.format(txn, 0, 9, attrs);
        });
        let md = to_markdown(&doc);
        assert!(md.contains("[# heading]"), "label must not escape #: {md}");
        assert!(!md.contains(r"[\# heading]"), "label must not double-escape: {md}");
    }

    #[test]
    fn export_drops_unsafe_link_href() {
        // #7a: the latent XSS the ticket warned about — a javascript:
        // href must never reach the <a> tag (just render the text).
        use std::sync::Arc;
        let doc = doc_with(|txn, frag| {
            let p = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
            let t = p.insert(txn, 0, XmlTextPrelim::new("x"));
            let mut attrs = Attrs::new();
            attrs.insert(
                Arc::from("link"),
                Any::String(Arc::from(r#"{"href":"javascript:alert(1)"}"#)),
            );
            t.format(txn, 0, 1, attrs);
        });
        let html = to_html(&doc);
        assert!(!html.contains("javascript:"), "unsafe href dropped: {html}");
        assert!(!html.contains("<a "), "no anchor for unsafe href: {html}");
        assert!(html.contains('x'), "text still rendered: {html}");
    }

    #[cfg(feature = "xlsx")]
    #[test]
    fn xlsx_grid_limits_are_lossless_and_match_excel() {
        // #7c: the column cap must keep `ci as u16` from wrapping, and
        // both caps must be Excel's real grid bounds.
        assert_eq!(XLSX_MAX_ROWS, 1_048_576);
        assert_eq!(XLSX_MAX_COLS, 16_384);
        assert!(
            XLSX_MAX_COLS <= u16::MAX as u32,
            "col cap must keep the `as u16` cast lossless"
        );
    }

    // ── Mermaid export (Task 6) ──────────────────────────────────────

    /// Build a doc with a single `mermaid` leaf node carrying `source`,
    /// and export it to HTML.
    fn to_html_of_single_mermaid(source: &str) -> String {
        let doc = doc_with(|txn, frag| {
            let m = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Mermaid.tag_name()));
            m.insert_attribute(txn, "source", source);
        });
        to_html(&doc)
    }

    /// Same as above, exported to Markdown.
    fn to_markdown_of_single_mermaid(source: &str) -> String {
        let doc = doc_with(|txn, frag| {
            let m = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Mermaid.tag_name()));
            m.insert_attribute(txn, "source", source);
        });
        to_markdown(&doc)
    }

    #[test]
    fn mermaid_html_inlines_svg_for_valid_pie() {
        // Build a doc with a single Mermaid node whose `source` attr is a
        // valid pie. (Reuse this file's existing doc-building helper/pattern.)
        let html = to_html_of_single_mermaid("pie\n\"A\" : 1\n\"B\" : 1");
        assert!(html.contains("<svg"), "expected inlined SVG, got: {html}");
        assert!(!html.contains("mermaid-error"));
    }

    #[test]
    fn mermaid_html_falls_back_to_raw_source_on_error() {
        let html = to_html_of_single_mermaid("sequenceDiagram\nAlice->>Bob: hi");
        assert!(html.contains("mermaid-error"));
        // raw source preserved and escaped
        assert!(html.contains("Alice-&gt;&gt;Bob") || html.contains("Alice->>Bob"));
        assert!(!html.contains("<svg"));
    }

    #[test]
    fn mermaid_markdown_emits_fenced_block() {
        let md = to_markdown_of_single_mermaid("pie\n\"A\" : 1");
        assert!(md.contains("```mermaid"));
        assert!(md.contains("\"A\" : 1"));
    }
}
