// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Fuzz + invariant net over the import parsers — the crate's untrusted-
//! input surface. `from_markdown`/`from_html` take raw pasted/imported
//! text; `from_xlsx`/`from_docx` take arbitrary uploaded bytes and run on
//! the async worker, where an uncaught panic takes the consumer task down
//! (see `import_pdf::from_pdf`, which already wraps its panic-prone
//! extractor in `catch_unwind`). The example-based suites cover shape;
//! these pin the two properties that hold for *any* input:
//!
//! 1. **Never panic** — malformed input errors or degrades, never crashes.
//! 2. **XSS boundary holds** — no dangerous tag or event-handler attribute
//!    survives into the document tree, for arbitrary input.

use proptest::prelude::*;
use yrs::types::xml::{Xml, XmlElementRef, XmlFragment, XmlOut};
use yrs::{Doc, ReadTxn, Transact};

use ogrenotes_collab::import::{from_html, from_markdown};

/// Recursively collect every element tag name and attribute name in a
/// document's `content` fragment.
fn collect_tags_and_attrs(doc: &Doc) -> (Vec<String>, Vec<String>) {
    let txn = doc.transact();
    let mut tags = Vec::new();
    let mut attrs = Vec::new();
    if let Some(fragment) = txn.get_xml_fragment("content") {
        for i in 0..fragment.len(&txn) {
            if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
                walk(&el, &txn, &mut tags, &mut attrs);
            }
        }
    }
    (tags, attrs)
}

fn walk<T: ReadTxn>(el: &XmlElementRef, txn: &T, tags: &mut Vec<String>, attrs: &mut Vec<String>) {
    tags.push(el.tag().to_string());
    for (key, _val) in el.attributes(txn) {
        attrs.push(key.to_string());
    }
    for i in 0..el.len(txn) {
        if let Some(XmlOut::Element(child)) = el.get(txn, i) {
            walk(&child, txn, tags, attrs);
        }
    }
}

/// Tags that must never appear in an imported document tree — script and
/// framing/embedding vectors. (The importer maps only a block allowlist;
/// this asserts the sanitizer + allowlist keep these out.)
const FORBIDDEN_TAGS: &[&str] = &["script", "iframe", "object", "embed", "style", "form", "link"];

fn assert_no_xss(doc: &Doc, src: &str) -> Result<(), TestCaseError> {
    let (tags, attrs) = collect_tags_and_attrs(doc);
    for t in &tags {
        let lower = t.to_ascii_lowercase();
        prop_assert!(
            !FORBIDDEN_TAGS.contains(&lower.as_str()),
            "forbidden tag {t:?} survived import of {src:?}"
        );
    }
    for a in &attrs {
        prop_assert!(
            !a.to_ascii_lowercase().starts_with("on"),
            "event-handler attribute {a:?} survived import of {src:?}"
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Arbitrary text through the markdown importer never panics.
    #[test]
    fn from_markdown_never_panics(s in "\\PC*") {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_markdown(&s)))
            .map_err(|_| TestCaseError::fail("from_markdown panicked"))?;
    }

    /// Arbitrary text through the HTML importer never panics.
    #[test]
    fn from_html_never_panics(s in "\\PC*") {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_html(&s)))
            .map_err(|_| TestCaseError::fail("from_html panicked"))?;
    }

    /// HTML embedding script / framing / event-handlers must not carry any
    /// of them into the document — the ammonia-sanitized import boundary.
    #[test]
    fn from_html_strips_all_xss_vectors(
        payload in "[a-z0-9 ='\"();]{0,40}",
        tag in prop::sample::select(vec!["script", "iframe", "object", "embed", "style", "form"]),
    ) {
        let html = format!(
            "<p>before</p><{tag}>{payload}</{tag}><img src=x onerror='{payload}'><a href=\"javascript:{payload}\">x</a><p>after</p>"
        );
        let doc = from_html(&html);
        assert_no_xss(&doc, &html)?;
    }

    /// Raw HTML embedded in markdown is dropped in v1 — so the same
    /// vectors can't sneak in through the markdown path either.
    #[test]
    fn from_markdown_drops_embedded_html_vectors(payload in "[a-z0-9 ]{0,30}") {
        let md = format!("text\n\n<script>{payload}</script>\n\n<img src=x onerror='{payload}'>\n\nmore");
        let doc = from_markdown(&md);
        assert_no_xss(&doc, &md)?;
    }
}

// ─── Binary parsers (feature-gated) ────────────────────────────
//
// from_xlsx (calamine) and from_docx (zip + quick-xml) have no
// catch_unwind guard — unlike from_pdf — because these parsers don't
// panic on the inputs fuzzed here. These properties pin that: a future
// calamine/zip/quick-xml bump that introduces a panic on malformed input
// would fail here rather than silently crash the import worker.

#[cfg(all(feature = "xlsx", feature = "docx"))]
mod binary {
    use super::*;
    use ogrenotes_collab::import_docx::from_docx;
    use ogrenotes_collab::import_spreadsheet::from_xlsx;
    use std::io::Write;

    /// A structurally-valid ZIP holding the given parts — malformed part
    /// *contents* inside a valid container is the classic trigger for a
    /// parser panic (the empty-bytes case rarely reaches the vulnerable
    /// code path).
    fn make_zip(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, content) in parts {
                zw.start_file(*name, opts).unwrap();
                zw.write_all(content).unwrap();
            }
            zw.finish().unwrap();
        }
        buf
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1500))]

        #[test]
        fn from_xlsx_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_xlsx(&bytes)))
                .map_err(|_| TestCaseError::fail("from_xlsx panicked on raw bytes"))?;
        }

        #[test]
        fn from_docx_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_docx(&bytes)))
                .map_err(|_| TestCaseError::fail("from_docx panicked on raw bytes"))?;
        }

        /// Valid zip, garbage OOXML parts — the calamine part-navigation
        /// panic surface.
        #[test]
        fn from_xlsx_valid_zip_garbage_parts_never_panics(
            wb in proptest::collection::vec(any::<u8>(), 0..200),
            ss in proptest::collection::vec(any::<u8>(), 0..200),
            sheet in proptest::collection::vec(any::<u8>(), 0..200),
        ) {
            let zip = make_zip(&[
                ("[Content_Types].xml", b"<?xml version=\"1.0\"?><Types/>"),
                ("xl/workbook.xml", &wb),
                ("xl/sharedStrings.xml", &ss),
                ("xl/worksheets/sheet1.xml", &sheet),
            ]);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_xlsx(&zip)))
                .map_err(|_| TestCaseError::fail("from_xlsx panicked on zip-with-garbage-parts"))?;
        }

        /// Valid zip, garbage word/document.xml — the quick-xml surface.
        #[test]
        fn from_docx_valid_zip_garbage_document_never_panics(
            doc in proptest::collection::vec(any::<u8>(), 0..300),
        ) {
            let zip = make_zip(&[
                ("[Content_Types].xml", b"<?xml version=\"1.0\"?><Types/>"),
                ("word/document.xml", &doc),
            ]);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_docx(&zip)))
                .map_err(|_| TestCaseError::fail("from_docx panicked on zip-with-garbage-document"))?;
        }
    }
}
