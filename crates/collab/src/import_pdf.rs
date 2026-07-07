// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.6 piece A — PDF → OgreNotes document import.
//!
//! PDF carries no document structure we can faithfully recover — it's
//! a page-description format, not a semantic one. So v1 is **text
//! only**: `pdf-extract` pulls the visible text, and each non-empty
//! line becomes a paragraph. The result lands in the same yrs `Doc`
//! shape every other importer builds.
//!
//! **v1 limitations** (documented for the operator caveats doc, piece E):
//!
//!   - Text only. Tables collapse to their cell text in reading order;
//!     images are dropped; inline marks (bold/italic) are not recovered
//!     (symmetric with the markdown / HTML / DOCX importers).
//!   - No paragraph reconstruction: each extracted line maps to one
//!     paragraph. PDF has no paragraph semantics to group on, so wrapped
//!     lines stay separate. Predictable beats clever here.
//!   - Scanned / image-only PDFs yield no text (OCR is a v2 carry);
//!     such an import produces a single empty paragraph rather than an
//!     error — the conversion "succeeded", it just had nothing to say.

use yrs::{
    types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim},
    Doc, Transact, WriteTxn,
};

use crate::schema::NodeType;

/// Upper bound on the PDF byte length we'll attempt. The import-job
/// route already caps the upload (10 MB), so this is a defense-in-depth
/// guard for direct/worker callers. Note it bounds the *input* only:
/// `pdf-extract` inflates Flate-compressed content streams internally
/// with no exposed ceiling, so a compressed-stream bomb inside a
/// within-limit PDF is a residual risk the upload cap is the real
/// mitigation for (flagged for the security review).
const MAX_PDF_BYTES: u64 = 32 * 1024 * 1024;

/// Upper bound on the extracted text. A small PDF can expand to a very
/// large text body (many pages / repeated content), which would become
/// an oversized yrs `Doc` → S3 snapshot → DynamoDB write. ~16 MiB is
/// thousands of pages of dense prose — generous for real input while
/// bounding abuse. Caps the *output* stage only (see `MAX_PDF_BYTES`).
const MAX_EXTRACTED_TEXT_BYTES: usize = 16 * 1024 * 1024;

/// Parse a PDF byte buffer into a fresh yrs `Doc`. Returns `Err` when
/// the bytes aren't a parseable PDF or exceed a size guard — all
/// signal "not importable" so the worker dead-letters rather than
/// retrying.
pub fn from_pdf(bytes: &[u8]) -> Result<Doc, String> {
    if bytes.len() as u64 > MAX_PDF_BYTES {
        return Err(format!(
            "PDF is {} bytes; limit is {MAX_PDF_BYTES}",
            bytes.len()
        ));
    }
    // pdf-extract panics (unwrap / `panic!`) on a range of malformed
    // PDFs — a missing catalog or pages tree, broken font tables, etc.
    // This runs on the worker, so an uncaught panic would take the
    // consumer task down. Catch it and turn it into an error the worker
    // dead-letters instead. (No profile sets panic=abort, so unwinding
    // is caught here.)
    let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }))
    .map_err(|_| "PDF parsing panicked (malformed or unsupported PDF)".to_string())?
    .map_err(|e| format!("PDF text extraction failed: {e}"))?;

    if extracted.len() > MAX_EXTRACTED_TEXT_BYTES {
        return Err(format!(
            "extracted text is {} bytes; limit is {MAX_EXTRACTED_TEXT_BYTES}",
            extracted.len()
        ));
    }

    Ok(materialize(&extracted))
}

/// Build the yrs `Doc`: one paragraph per non-empty extracted line.
/// Guarantees at least one paragraph so an image-only / text-free PDF
/// still produces a valid (blank) document rather than an empty
/// fragment.
fn materialize(text: &str) -> Doc {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let fragment = txn.get_or_insert_xml_fragment("content");

        let mut emitted = false;
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let pos = fragment.len(&txn);
            let p = fragment.insert(
                &mut txn,
                pos,
                XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
            );
            p.insert(&mut txn, 0, XmlTextPrelim::new(trimmed));
            emitted = true;
        }

        if !emitted {
            fragment.insert(
                &mut txn,
                0,
                XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
            );
        }
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal single-page PDF whose content stream shows each
    /// line of `lines` with the standard Helvetica font. lopdf produces
    /// a valid xref + trailer, so `pdf-extract` can parse it — far more
    /// robust than hand-writing byte offsets.
    fn build_text_pdf(lines: &[&str]) -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();

        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });

        let mut ops = vec![Operation::new("BT", vec![])];
        ops.push(Operation::new("Tf", vec!["F1".into(), 14.into()]));
        // Start near the top and move down one line per entry.
        ops.push(Operation::new("Td", vec![72.into(), 720.into()]));
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                // Move to the next line (negative y leading).
                ops.push(Operation::new("Td", vec![0.into(), (-18).into()]));
            }
            ops.push(Operation::new(
                "Tj",
                vec![Object::string_literal(*line)],
            ));
        }
        ops.push(Operation::new("ET", vec![]));

        let content = Content { operations: ops };
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            content.encode().expect("encode content"),
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);

        let mut buf = Vec::new();
        doc.save_to(&mut buf).expect("save pdf");
        buf
    }

    /// Extract the text of every top-level paragraph from a doc, in order.
    fn paragraph_texts(doc: &Doc) -> Vec<String> {
        use yrs::types::xml::{XmlOut, XmlTextRef};
        use yrs::{GetString, ReadTxn};
        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").expect("content fragment");
        let mut out = Vec::new();
        for i in 0..fragment.len(&txn) {
            let Some(XmlOut::Element(el)) = fragment.get(&txn, i) else {
                continue;
            };
            if el.tag().as_ref() != NodeType::Paragraph.tag_name() {
                continue;
            }
            let mut text = String::new();
            for j in 0..el.len(&txn) {
                if let Some(XmlOut::Text(t)) = el.get(&txn, j) {
                    let t: XmlTextRef = t;
                    text.push_str(&t.get_string(&txn));
                }
            }
            out.push(text);
        }
        out
    }

    #[test]
    fn rejects_non_pdf_bytes() {
        let err = from_pdf(b"this is not a pdf at all").unwrap_err();
        assert!(err.contains("extraction failed"), "got: {err}");
    }

    #[test]
    fn rejects_oversize_input() {
        // Covers the input-size guard only. A within-limit PDF carrying a
        // compressed-stream bomb is NOT defended here — pdf-extract has no
        // decompression ceiling, so the upload cap is that case's mitigation.
        let big = vec![b'%'; (MAX_PDF_BYTES + 1) as usize];
        let err = from_pdf(&big).unwrap_err();
        assert!(err.contains("limit"), "got: {err}");
    }

    #[test]
    fn pdf_export_reimport_preserves_text() {
        // Piece B round-trip: build a doc, to_pdf, then from_pdf, and
        // confirm the text survives. PDF is plain-text-only, so we assert
        // on content, not structure.
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let frag = txn.get_or_insert_xml_fragment("content");
            for (i, text) in ["Quarterly report", "Revenue grew this quarter."]
                .iter()
                .enumerate()
            {
                let p = frag.insert(
                    &mut txn,
                    i as u32,
                    XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
                );
                p.insert(&mut txn, 0, XmlTextPrelim::new(*text));
            }
        }
        let pdf = crate::export::to_pdf(&doc);
        assert!(!pdf.is_empty(), "to_pdf produced empty bytes");
        assert_eq!(&pdf[..4], b"%PDF", "output should be a PDF");

        let reimported = from_pdf(&pdf).expect("exported pdf should reimport");
        let text = paragraph_texts(&reimported).join("\n");
        assert!(text.contains("Quarterly report"), "got: {text}");
        assert!(text.contains("Revenue grew this quarter."), "got: {text}");
    }

    #[test]
    fn extracts_lines_as_paragraphs() {
        let pdf = build_text_pdf(&["First paragraph line", "Second paragraph line"]);
        let doc = from_pdf(&pdf).expect("import should succeed");
        let paras = paragraph_texts(&doc);
        // pdf-extract may add surrounding whitespace/blank lines; assert
        // on content, not exact paragraph count.
        let joined = paras.join("\n");
        assert!(joined.contains("First paragraph line"), "got: {paras:?}");
        assert!(joined.contains("Second paragraph line"), "got: {paras:?}");
        // Require at least two paragraphs so we'd catch a regression where
        // pdf-extract's line-break behavior folds both lines into one.
        assert!(paras.len() >= 2, "expected >= 2 paragraphs, got: {paras:?}");
    }

    #[test]
    fn text_free_pdf_yields_one_empty_paragraph() {
        // Exercises `materialize` directly. The from_pdf path for a truly
        // text-free PDF (scanned/image-only) depends on pdf-extract
        // returning Ok("") rather than Err, which isn't asserted here.
        let doc = materialize("   \n\n  ");
        let paras = paragraph_texts(&doc);
        assert_eq!(paras, vec![String::new()], "expected a single empty paragraph");
    }
}
