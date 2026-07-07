// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.5 piece A — DOCX → OgreNotes document import.
//!
//! A `.docx` is a ZIP container of OOXML parts; the prose lives in
//! `word/document.xml`. We unzip that one part (deflate is the only
//! compression the spec allows) and stream it through quick-xml,
//! mapping the block grammar onto the same yrs `Doc` shape the other
//! importers build — so once it lands in the editor a DOCX import is
//! indistinguishable from a markdown / HTML / CSV import.
//!
//! Supported block grammar (v1):
//!
//!   - paragraph (`w:p`)
//!   - heading (`w:p` whose `w:pStyle` is `HeadingN` / `Title`),
//!     carrying a `level` attribute like the markdown/HTML importers
//!   - table (`w:tbl` → `w:tr` → `w:tc`), each cell's text wrapped in
//!     a paragraph — identical shape to the CSV/XLSX importers
//!
//! **v1 limitations** (consistent with the markdown/HTML importers):
//!
//!   - Inline marks (bold / italic / underline / …) are dropped: run
//!     text is concatenated into plain characters. Same contract the
//!     `inline_emphasis_drops_marks_keeps_text` import tests pin.
//!   - Lists (`w:numPr`) import as plain paragraphs; mapping Word
//!     numbering definitions to bullet/ordered lists needs
//!     `word/numbering.xml` and is deferred.
//!   - Images, footnotes, headers/footers are dropped. Nested tables
//!     are flattened — an inner table's text folds into the enclosing
//!     cell rather than nesting.
//!
//! Parsing and yrs construction are split: `parse_document` produces
//! a `Vec<DocxBlock>` intermediate so the streaming state machine
//! never holds a live transaction, and the two halves test
//! independently.

use std::io::{Cursor, Read};

use quick_xml::events::Event;
use quick_xml::Reader;
use yrs::{
    types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim},
    Doc, Transact, WriteTxn, Xml,
};

use crate::schema::NodeType;

/// The single OOXML part we read. Headers, footers, footnotes, and
/// numbering definitions live in sibling parts we don't touch in v1.
const DOCUMENT_PART: &str = "word/document.xml";

/// Decompression-bomb ceiling for `word/document.xml`. A few-KB `.docx`
/// can carry a deflate stream that inflates to gigabytes; without a cap
/// the read would OOM the worker. 64 MiB is far above any real
/// document's prose part while bounding a malicious one. Enforced both
/// against the ZIP header's declared size (cheap, pre-read) and against
/// the actual bytes read (defends a spoofed/understated header).
const MAX_DOCUMENT_XML_BYTES: u64 = 64 * 1024 * 1024;

/// Intermediate block, decoupled from yrs so the parser is a pure
/// `bytes -> Vec<DocxBlock>` function.
#[derive(Debug, Clone, PartialEq)]
enum DocxBlock {
    /// `level == None` → paragraph; `Some(n)` → heading at level n.
    Para { level: Option<u8>, text: String },
    /// Rows of cells; each cell holds its combined text (a
    /// multi-paragraph cell is newline-joined in v1).
    Table { rows: Vec<Vec<String>> },
}

/// Parse a `.docx` byte buffer into a fresh yrs `Doc`. Returns `Err`
/// when the input isn't a valid ZIP or is missing the main document
/// part — both signal "not a DOCX" rather than a transient fault, so
/// the worker dead-letters them rather than retrying.
pub fn from_docx(bytes: &[u8]) -> Result<Doc, String> {
    let xml = extract_document_part(bytes)?;
    let blocks = parse_document(&xml)?;
    Ok(materialize(&blocks))
}

/// Unzip just `word/document.xml`. We don't extract the whole archive
/// — the other parts are irrelevant to v1's block grammar.
fn extract_document_part(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("not a valid DOCX (zip open failed): {e}"))?;
    let part = archive
        .by_name(DOCUMENT_PART)
        .map_err(|_| format!("DOCX missing {DOCUMENT_PART}"))?;
    // Reject upfront on the header's declared uncompressed size — cheap,
    // before any inflation happens.
    if part.size() > MAX_DOCUMENT_XML_BYTES {
        return Err(format!(
            "{DOCUMENT_PART} is {} bytes uncompressed; limit is {MAX_DOCUMENT_XML_BYTES}",
            part.size()
        ));
    }
    // Read through a bounded adapter so a deflate stream that inflates
    // past the cap (e.g. a header understating its size) still can't run
    // away — we stop at the limit + 1 and reject.
    let mut buf = Vec::new();
    part.take(MAX_DOCUMENT_XML_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("failed reading {DOCUMENT_PART}: {e}"))?;
    if buf.len() as u64 > MAX_DOCUMENT_XML_BYTES {
        return Err(format!(
            "{DOCUMENT_PART} exceeds the {MAX_DOCUMENT_XML_BYTES}-byte limit"
        ));
    }
    Ok(buf)
}

/// Accumulator for the paragraph currently being read. A paragraph's
/// heading level (from `w:pStyle`, which precedes the runs) and its
/// run text are both known only at `</w:p>`, so we buffer until then.
#[derive(Default)]
struct ParaAcc {
    level: Option<u8>,
    text: String,
}

/// Stream `word/document.xml` into the block intermediate. The state
/// is a small set of "what am I inside" flags plus one table builder;
/// `tbl_depth` guards against nested tables corrupting the builder.
fn parse_document(xml: &[u8]) -> Result<Vec<DocxBlock>, String> {
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();

    let mut out: Vec<DocxBlock> = Vec::new();
    let mut cur_para: Option<ParaAcc> = None;
    let mut in_wt = false; // inside a <w:t> text run

    let mut tbl_depth: u32 = 0;
    let mut cur_table: Option<Vec<Vec<String>>> = None;
    let mut cur_row: Option<Vec<String>> = None;
    let mut cur_cell: Option<String> = None;

    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| format!("malformed document.xml: {e}"))?
        {
            Event::Start(e) => match e.local_name().as_ref() {
                b"p" => cur_para = Some(ParaAcc::default()),
                b"t" => in_wt = true,
                b"pStyle" => apply_pstyle(&e, &mut cur_para),
                b"tbl" => {
                    tbl_depth += 1;
                    if tbl_depth == 1 {
                        cur_table = Some(Vec::new());
                    }
                }
                b"tr" if tbl_depth == 1 => cur_row = Some(Vec::new()),
                b"tc" if tbl_depth == 1 => cur_cell = Some(String::new()),
                _ => {}
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                // Self-closing paragraph (an empty line).
                b"p" => finish_paragraph(
                    ParaAcc::default(),
                    tbl_depth,
                    &mut cur_cell,
                    &mut out,
                ),
                b"pStyle" => apply_pstyle(&e, &mut cur_para),
                b"tab" => {
                    if let Some(p) = cur_para.as_mut() {
                        p.text.push('\t');
                    }
                }
                _ => {}
            },
            Event::Text(e) => {
                if in_wt && let Some(p) = cur_para.as_mut() {
                    let t = e
                        .unescape()
                        .map_err(|err| format!("text decode: {err}"))?;
                    p.text.push_str(&t);
                }
            }
            Event::End(e) => match e.local_name().as_ref() {
                b"t" => in_wt = false,
                b"p" => {
                    if let Some(para) = cur_para.take() {
                        finish_paragraph(para, tbl_depth, &mut cur_cell, &mut out);
                    }
                }
                b"tc" if tbl_depth == 1 => {
                    if let (Some(row), Some(cell)) = (cur_row.as_mut(), cur_cell.take()) {
                        row.push(cell);
                    }
                }
                b"tr" if tbl_depth == 1 => {
                    if let (Some(table), Some(row)) = (cur_table.as_mut(), cur_row.take()) {
                        table.push(row);
                    }
                }
                b"tbl" => {
                    if tbl_depth == 1 && let Some(rows) = cur_table.take() {
                        out.push(DocxBlock::Table { rows });
                    }
                    tbl_depth = tbl_depth.saturating_sub(1);
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
}

/// Route a finished paragraph: into the open table cell when inside a
/// table, otherwise as a top-level block. A heading inside a cell
/// loses its level (folded to cell text) — acceptable for v1.
fn finish_paragraph(
    para: ParaAcc,
    tbl_depth: u32,
    cur_cell: &mut Option<String>,
    out: &mut Vec<DocxBlock>,
) {
    if tbl_depth >= 1 {
        if let Some(cell) = cur_cell.as_mut()
            && !para.text.is_empty()
        {
            if !cell.is_empty() {
                cell.push('\n');
            }
            cell.push_str(&para.text);
        }
        return;
    }
    out.push(DocxBlock::Para {
        level: para.level,
        text: para.text,
    });
}

/// Set the current paragraph's heading level from a `w:pStyle`
/// element's `w:val` attribute, if it names a heading style.
fn apply_pstyle(e: &quick_xml::events::BytesStart<'_>, cur_para: &mut Option<ParaAcc>) {
    let Some(para) = cur_para.as_mut() else { return };
    for attr in e.attributes().flatten() {
        // document.xml is always UTF-8; decode then expand any XML
        // entities. We use the standalone `unescape` rather than
        // `Attribute::unescape_value` because docx-rs enables quick-xml's
        // `encoding` feature workspace-wide, which cfg-removes that
        // method — `unescape` is available regardless and entity
        // handling still matters for custom style ids.
        if attr.key.local_name().as_ref() == b"val"
            && let Ok(raw) = std::str::from_utf8(&attr.value)
            && let Ok(val) = quick_xml::escape::unescape(raw)
            && let Some(level) = heading_level(&val)
        {
            para.level = Some(level);
        }
    }
}

/// Map a Word paragraph-style id to a heading level. Recognizes the
/// built-in `Heading1`..`Heading9` ids (and the spaced `Heading 1`
/// variant some producers emit) plus `Title`. Levels clamp to 1..=6
/// to match the editor's heading grammar.
fn heading_level(style: &str) -> Option<u8> {
    let s = style.trim();
    if s.eq_ignore_ascii_case("Title") {
        return Some(1);
    }
    let rest = s
        .strip_prefix("Heading")
        .or_else(|| s.strip_prefix("heading"))?
        .trim();
    if rest.is_empty() {
        return Some(1);
    }
    rest.parse::<u8>().ok().map(|n| n.clamp(1, 6))
}

/// Build the yrs `Doc` from the parsed blocks. Mirrors the table
/// shape the CSV/XLSX importers produce so all four importers feed
/// the editor the same node grammar.
fn materialize(blocks: &[DocxBlock]) -> Doc {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");

        for block in blocks {
            match block {
                DocxBlock::Para { level, text } => {
                    let nt = if level.is_some() {
                        NodeType::Heading
                    } else {
                        NodeType::Paragraph
                    };
                    let pos = fragment.len(&txn);
                    let el = fragment.insert(&mut txn, pos, XmlElementPrelim::empty(nt.tag_name()));
                    if let Some(n) = level {
                        el.insert_attribute(&mut txn, "level", n.to_string());
                    }
                    if !text.is_empty() {
                        el.insert(&mut txn, 0, XmlTextPrelim::new(text));
                    }
                }
                DocxBlock::Table { rows } => {
                    let pos = fragment.len(&txn);
                    let table = fragment.insert(
                        &mut txn,
                        pos,
                        XmlElementPrelim::empty(NodeType::Table.tag_name()),
                    );
                    for row_cells in rows {
                        let rpos = table.len(&txn);
                        let row = table.insert(
                            &mut txn,
                            rpos,
                            XmlElementPrelim::empty(NodeType::TableRow.tag_name()),
                        );
                        for cell_text in row_cells {
                            let cpos = row.len(&txn);
                            let cell = row.insert(
                                &mut txn,
                                cpos,
                                XmlElementPrelim::empty(NodeType::TableCell.tag_name()),
                            );
                            if !cell_text.is_empty() {
                                let p = cell.insert(
                                    &mut txn,
                                    0,
                                    XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
                                );
                                p.insert(&mut txn, 0, XmlTextPrelim::new(cell_text));
                            }
                        }
                    }
                }
            }
        }
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// Wrap a `word/document.xml` body in the minimal ZIP container
    /// `from_docx` reads. Stored (uncompressed) so the test write
    /// path doesn't depend on the deflate feature.
    fn build_docx(document_xml: &str) -> Vec<u8> {
        let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zw.start_file(DOCUMENT_PART, opts).unwrap();
        zw.write_all(document_xml.as_bytes()).unwrap();
        zw.finish().unwrap().into_inner()
    }

    fn doc_body(inner: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>{inner}</w:body></w:document>"#
        )
    }

    #[test]
    fn rejects_non_docx_bytes() {
        let err = from_docx(b"not a zip at all").unwrap_err();
        assert!(err.contains("zip open failed"), "got: {err}");
    }

    #[test]
    fn rejects_decompression_bomb() {
        // A document.xml that inflates past the cap is rejected at the
        // header-size check before any inflation. Spaces compress
        // ~1000:1, so the fixture stays a few KB on the wire while
        // declaring ~65 MiB uncompressed.
        let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zw.start_file(DOCUMENT_PART, opts).unwrap();
        let chunk = vec![b' '; 1024 * 1024];
        let mut written: u64 = 0;
        while written <= MAX_DOCUMENT_XML_BYTES {
            zw.write_all(&chunk).unwrap();
            written += chunk.len() as u64;
        }
        let bytes = zw.finish().unwrap().into_inner();
        assert!(bytes.len() < 1024 * 1024, "bomb fixture should compress tiny");

        let err = from_docx(&bytes).unwrap_err();
        assert!(err.contains("limit"), "expected a size-limit rejection, got: {err}");
    }

    #[test]
    fn rejects_zip_without_document_part() {
        let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zw.start_file("word/other.xml", opts).unwrap();
        zw.write_all(b"<x/>").unwrap();
        let bytes = zw.finish().unwrap().into_inner();

        let err = from_docx(&bytes).unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn parses_paragraph_and_concatenates_runs_dropping_marks() {
        let xml = doc_body(
            r#"<w:p>
                <w:r><w:t xml:space="preserve">Hello </w:t></w:r>
                <w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r>
                <w:r><w:t> world</w:t></w:r>
               </w:p>"#,
        );
        let blocks = parse_document(xml.as_bytes()).unwrap();
        assert_eq!(
            blocks,
            vec![DocxBlock::Para {
                level: None,
                text: "Hello bold world".to_string()
            }]
        );
    }

    #[test]
    fn detects_heading_level_from_pstyle() {
        let xml = doc_body(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr>
                <w:r><w:t>Section</w:t></w:r></w:p>"#,
        );
        let blocks = parse_document(xml.as_bytes()).unwrap();
        assert_eq!(
            blocks,
            vec![DocxBlock::Para {
                level: Some(2),
                text: "Section".to_string()
            }]
        );
    }

    #[test]
    fn title_style_maps_to_level_one() {
        let xml = doc_body(
            r#"<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr>
                <w:r><w:t>Doc title</w:t></w:r></w:p>"#,
        );
        let blocks = parse_document(xml.as_bytes()).unwrap();
        assert_eq!(blocks[0], DocxBlock::Para { level: Some(1), text: "Doc title".into() });
    }

    #[test]
    fn parses_table_into_rows_and_cells() {
        let cell = |t: &str| format!("<w:tc><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc>");
        let row = |a: &str, b: &str| format!("<w:tr>{}{}</w:tr>", cell(a), cell(b));
        let xml = doc_body(&format!("<w:tbl>{}{}</w:tbl>", row("A1", "B1"), row("A2", "B2")));

        let blocks = parse_document(xml.as_bytes()).unwrap();
        assert_eq!(
            blocks,
            vec![DocxBlock::Table {
                rows: vec![
                    vec!["A1".to_string(), "B1".to_string()],
                    vec!["A2".to_string(), "B2".to_string()],
                ]
            }]
        );
    }

    #[test]
    fn multi_paragraph_cell_newline_joins() {
        let xml = doc_body(
            "<w:tbl><w:tr><w:tc>\
             <w:p><w:r><w:t>line one</w:t></w:r></w:p>\
             <w:p><w:r><w:t>line two</w:t></w:r></w:p>\
             </w:tc></w:tr></w:tbl>",
        );
        let blocks = parse_document(xml.as_bytes()).unwrap();
        assert_eq!(
            blocks,
            vec![DocxBlock::Table {
                rows: vec![vec!["line one\nline two".to_string()]]
            }]
        );
    }

    #[test]
    fn empty_self_closing_paragraph_is_kept() {
        let xml = doc_body("<w:p/><w:p><w:r><w:t>after</w:t></w:r></w:p>");
        let blocks = parse_document(xml.as_bytes()).unwrap();
        assert_eq!(
            blocks,
            vec![
                DocxBlock::Para { level: None, text: String::new() },
                DocxBlock::Para { level: None, text: "after".to_string() },
            ]
        );
    }

    #[test]
    fn docx_export_reimport_preserves_structure() {
        // Piece B round-trip: a DOCX → from_docx → to_docx → from_docx
        // should preserve the supported block grammar. Compare via the
        // exported HTML so the assertion is on the yrs node shape, not
        // byte-identical DOCX (which we never promise).
        let xml = doc_body(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Section</w:t></w:r></w:p>
               <w:p><w:r><w:t>Body paragraph</w:t></w:r></w:p>
               <w:tbl>
                 <w:tr>
                   <w:tc><w:p><w:r><w:t>r1c1</w:t></w:r></w:p></w:tc>
                   <w:tc><w:p><w:r><w:t>r1c2</w:t></w:r></w:p></w:tc>
                 </w:tr>
                 <w:tr>
                   <w:tc><w:p><w:r><w:t>r2c1</w:t></w:r></w:p></w:tc>
                   <w:tc><w:p><w:r><w:t>r2c2</w:t></w:r></w:p></w:tc>
                 </w:tr>
               </w:tbl>"#,
        );
        let original = from_docx(&build_docx(&xml)).unwrap();
        let exported = crate::export::to_docx(&original);
        assert!(!exported.is_empty(), "to_docx produced empty bytes");

        let reimported = from_docx(&exported).expect("exported docx should re-parse");

        // Structure survives the round trip.
        assert_eq!(
            crate::export::to_html(&original),
            crate::export::to_html(&reimported),
            "round-trip changed the document structure",
        );
        // And the heading level specifically is preserved.
        let html = crate::export::to_html(&reimported);
        assert!(html.contains("<h2>Section</h2>"), "heading level lost: {html}");
        assert!(html.contains("r2c2"), "table cell lost: {html}");
    }

    #[test]
    fn table_header_round_trip_preserves_cell_text() {
        // A table_header exports to a plain DOCX body cell (docx-rs has
        // no header primitive), so <th> semantics are a known v1 loss —
        // but the cell text must survive the round trip. Pins that
        // behavior and exercises the header branch of build_docx_table.
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            let table = fragment.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::Table.tag_name()),
            );
            let row = table.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::TableRow.tag_name()),
            );
            let th = row.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::TableHeader.tag_name()),
            );
            let p = th.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
            );
            p.insert(&mut txn, 0, XmlTextPrelim::new("Header A"));
        }
        let exported = crate::export::to_docx(&doc);
        assert!(!exported.is_empty());
        let reimported = from_docx(&exported).expect("header table round-trip");
        let html = crate::export::to_html(&reimported);
        assert!(html.contains("Header A"), "header cell text lost: {html}");
    }

    #[test]
    fn materialize_builds_expected_yrs_shape() {
        // End-to-end through the ZIP container: a heading, a paragraph,
        // and a 1x2 table. Assert against the exported HTML so we're
        // checking the real yrs node grammar, not the intermediate.
        let xml = doc_body(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>
               <w:p><w:r><w:t>Body text</w:t></w:r></w:p>
               <w:tbl><w:tr>
                 <w:tc><w:p><w:r><w:t>c1</w:t></w:r></w:p></w:tc>
                 <w:tc><w:p><w:r><w:t>c2</w:t></w:r></w:p></w:tc>
               </w:tr></w:tbl>"#,
        );
        let bytes = build_docx(&xml);
        let doc = from_docx(&bytes).unwrap();
        let html = crate::export::to_html(&doc);

        assert!(html.contains("<h1>Title</h1>"), "got: {html}");
        assert!(html.contains("Body text"), "got: {html}");
        assert!(html.contains("<table>"), "got: {html}");
        assert!(html.contains("c1") && html.contains("c2"), "got: {html}");
    }
}
