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
use yrs::{Doc, GetString, ReadTxn, Transact};

use ogrenotes_collab::import::{from_html, from_markdown};
use ogrenotes_collab::import_quip::from_quip_html;

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

    /// Arbitrary text through the **Quip** importer never panics.
    ///
    /// This parser is the newest of the four and the only one whose input is
    /// authored entirely by a third party — the worker feeds it whatever
    /// `/2/threads/{id}/html` returns, with no opportunity for a user to
    /// notice the document is malformed first. The content pass now catches a
    /// panic here and charges it to the thread's attempt budget rather than
    /// dead-lettering the import, but that is a containment net; this is the
    /// property that says the net should stay unused.
    #[test]
    fn from_quip_html_never_panics(s in "\\PC*") {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_quip_html(&s)))
            .map_err(|_| TestCaseError::fail("from_quip_html panicked"))?;
    }

    /// The same property against input actually shaped like markup.
    ///
    /// `\\PC*` almost never produces a well-formed tag, so it exercises the
    /// sanitizer and little of the walker behind it. This generator emits
    /// nested Quip-shaped elements — the tables, images, checkbox inputs and
    /// section-anchored headings that `from_quip_html` handles and
    /// `from_html` deliberately does not — so the recursive block walker,
    /// `enforce_containment` and `materialize` are the code under test.
    #[test]
    fn from_quip_html_never_panics_on_quip_shaped_markup(
        tags in prop::collection::vec(
            prop::sample::select(vec![
                "h1", "h6", "p", "ul", "ol", "li", "table", "tr", "td", "th",
                "blockquote", "pre", "code", "b", "a", "img", "input", "div", "span",
            ]),
            0..12,
        ),
        attrs in prop::collection::vec(
            prop::sample::select(vec![
                "", " id=\"sec-1\"", " href=\"https://acme.quip.com/t1/X#sec-2\"",
                " src=\"/blob/t1/b9\"", " type=\"checkbox\" checked",
                " data-section-id=\"s\"", " class=\"\"", " alt=\"\"",
            ]),
            0..12,
        ),
        text in "[a-z <>&\"/=]{0,24}",
    ) {
        // Generated nesting is bounded well under `MAX_NESTING_DEPTH` on
        // purpose. Unbounded nesting does not *panic* the walker — it
        // exhausts the stack, and stack exhaustion ABORTS the process rather
        // than unwinding, so proptest would kill the test runner instead of
        // shrinking a failing case. The depth cap gets its own deterministic
        // test below; this property stays in panic-land where catch_unwind
        // can actually report.
        // Interleave opens, text and (deliberately unbalanced) closes: real
        // Quip HTML is well-formed, so mismatched nesting is exactly the case
        // no example-based test covers.
        let mut html = String::new();
        for (i, t) in tags.iter().enumerate() {
            let a = attrs.get(i).copied().unwrap_or("");
            html.push_str(&format!("<{t}{a}>{text}"));
            if i % 3 == 2 {
                html.push_str(&format!("</{t}>"));
            }
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_quip_html(&html)))
            .map_err(|_| TestCaseError::fail(format!("from_quip_html panicked on {html:?}")))?;
    }

    /// The Quip importer's ammonia allowlist is *wider* than
    /// `import::from_html`'s — it has to admit tables, images and checkbox
    /// inputs — so the XSS boundary is a genuinely different one and needs
    /// its own property rather than inheriting the narrower parser's.
    #[test]
    fn from_quip_html_strips_all_xss_vectors(
        payload in "[a-z0-9 ='\"();]{0,40}",
        tag in prop::sample::select(vec!["script", "iframe", "object", "embed", "style", "form", "link"]),
    ) {
        let html = format!(
            "<p>before</p><{tag}>{payload}</{tag}><img src=x onerror='{payload}'>\
             <table><tr><td onclick='{payload}'>c</td></tr></table>\
             <a href=\"javascript:{payload}\">x</a><p>after</p>"
        );
        let out = from_quip_html(&html);
        assert_no_xss(&out.doc, &html)?;
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

// ─── Nesting depth: an abort, not a panic ───────────────────────
//
// The properties above all wrap the parser in `catch_unwind`, which is the
// right net for a panic (an unwrap, an index, a bad slice boundary) and is
// *no* net at all for stack exhaustion: Rust aborts the process on stack
// overflow, so there is nothing to catch. Deeply-nested third-party HTML is
// therefore a liveness bug that the fuzz net above cannot see, and it needs a
// deterministic test with a concrete depth rather than a generator.
//
// Before `MAX_NESTING_DEPTH`, `from_quip_html` aborted at ~1 050 levels of
// nested `<div>`/`<span>`/`<blockquote>` on the 2 MiB stack that tokio worker
// threads and Rust test threads both get. On the shared import worker that
// killed every concurrent job, and the retry reached the same document and
// died again — a permanent wedge that charged no attempt and marked no thread
// failed, because the process never got to run that code.

/// Past the cap, but deliberately **below** the ~1 050-level depth at which
/// the uncapped walker aborted. A regression here therefore fails *cleanly*
/// on the truncation assertion instead of killing the runner — which is the
/// signal you want in CI for the ordinary mistake (cap raised, cap removed).
const PAST_CAP: usize = 500;

/// Past the depth at which the uncapped walker exhausted a 2 MiB stack, and
/// far below `ammonia::clean`'s own (upstream, unfixed) limit around 120 000.
const PAST_STACK_LIMIT: usize = 4_000;

fn nested(tag: &str, depth: usize) -> String {
    format!(
        "<p>top</p>{}deep-content{}",
        format!("<{tag}>").repeat(depth),
        format!("</{tag}>").repeat(depth),
    )
}

/// Nesting past the cap is flattened and *recorded*, not silently accepted.
///
/// This is the behavioral half, pinned at a depth the uncapped walker
/// survived, so removing or raising the cap fails this assertion cleanly
/// rather than aborting the process.
#[test]
fn nesting_past_the_cap_is_truncated_and_reported() {
    for tag in ["div", "span", "blockquote"] {
        let out = from_quip_html(&nested(tag, PAST_CAP));
        assert!(
            out.deep_nesting_truncated > 0,
            "<{tag}> nested {PAST_CAP} deep must be recorded as truncated, not silently accepted",
        );
        // Failing SOFT is the point: dropping the subtree would silently
        // delete the deepest content of a document, and erroring would wedge
        // the thread on every retry.
        let text = doc_text(&out.doc);
        assert!(text.contains("top"), "content above the cap must survive: {text:?}");
        assert!(
            text.contains("deep-content"),
            "the text below the cap must survive the flattening: {text:?}",
        );
    }
}

/// The liveness half: nesting past the *stack* limit does not abort.
///
/// There is no way to make this one fail gracefully — if the cap is gone the
/// process dies here rather than returning a failure — but the runner names
/// the test in `fatal runtime error: stack overflow`, so the signal is
/// unambiguous even at its ugliest. Kept separate from the behavioral test
/// above precisely so the common regression is caught by *that* one first.
#[test]
fn nesting_past_the_stack_limit_does_not_abort_the_process() {
    for tag in ["div", "span", "blockquote"] {
        let out = from_quip_html(&nested(tag, PAST_STACK_LIMIT));
        assert!(out.deep_nesting_truncated > 0, "<{tag}> must be truncated");
        assert!(
            doc_text(&out.doc).contains("deep-content"),
            "the text must survive even at {PAST_STACK_LIMIT} levels",
        );
    }
}

/// The cap must not touch documents that stay under it — a real Quip document
/// nests around a dozen levels, so a cap that truncated ordinary content
/// would be a worse bug than the one it fixes.
#[test]
fn ordinary_nesting_is_not_truncated() {
    let depth = 16;
    let html = format!(
        "{}<p>nested</p>{}",
        "<blockquote>".repeat(depth),
        "</blockquote>".repeat(depth),
    );
    let out = from_quip_html(&html);
    assert_eq!(
        out.deep_nesting_truncated, 0,
        "{depth} levels is ordinary document structure and must pass through untouched",
    );
    assert!(doc_text(&out.doc).contains("nested"));
}

/// Flattened text of a document's `content` fragment.
fn doc_text(doc: &Doc) -> String {
    fn walk_text<T: ReadTxn>(el: &XmlElementRef, txn: &T, out: &mut String) {
        for i in 0..el.len(txn) {
            match el.get(txn, i) {
                Some(XmlOut::Element(child)) => walk_text(&child, txn, out),
                Some(XmlOut::Text(t)) => {
                    out.push(' ');
                    out.push_str(&t.get_string(txn));
                }
                _ => {}
            }
        }
    }
    let txn = doc.transact();
    let mut out = String::new();
    if let Some(fragment) = txn.get_xml_fragment("content") {
        for i in 0..fragment.len(&txn) {
            if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
                walk_text(&el, &txn, &mut out);
            }
        }
    }
    out
}
