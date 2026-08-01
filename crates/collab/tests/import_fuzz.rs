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
//! 2. **Materialization stays inside the schema** — no dangerous tag or
//!    event-handler attribute appears in the document tree, for arbitrary
//!    input. Note what that is *not*: it is not a test of the sanitizer.
//!    See [`assert_no_xss`] for exactly what this file can and cannot
//!    observe; the sanitizer boundary is pinned by the `sanitize_*` unit
//!    tests in `crates/collab/src/import.rs`.
//!
//! Neither property can see a **stack overflow** — Rust aborts the process
//! rather than unwinding, so `catch_unwind` has nothing to catch and the
//! runner dies instead of shrinking. The nesting-depth cap therefore gets
//! deterministic tests at the bottom of this file rather than a generator.

use proptest::prelude::*;
use yrs::types::xml::{Xml, XmlElementRef, XmlFragment, XmlOut};
use yrs::{Doc, GetString, ReadTxn, Transact};

use ogrenotes_collab::import::{MAX_NESTING_DEPTH, from_html, from_markdown};
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
/// framing/embedding vectors. Note this is a property of the *materialized
/// tree*, not of the sanitizer: see [`assert_no_xss`], and
/// `allowed_html_tags_admits_no_forbidden_tag` in `import.rs` for the
/// allowlist's own guard.
const FORBIDDEN_TAGS: &[&str] = &["script", "iframe", "object", "embed", "style", "form", "link"];

/// Assert the *materialized* document carries no forbidden element and no
/// event-handler attribute.
///
/// **What this cannot observe (#160).** It walks the yrs document, whose
/// element names are written by the importer from the closed
/// `schema::NodeType` enum and whose attributes are the handful the importer
/// writes itself (`level`, `language`). Materialization has no way to emit an
/// `<iframe>` or an `onclick` no matter what the sanitizer let through — so a
/// green result here says the walker and materializer cannot be talked into
/// emitting a forbidden element, and says *nothing* about the allowlist in
/// front of them. Adding `iframe`/`object`/`embed`/`form`/`link` to
/// `allowed_html_tags()` leaves every assertion below green.
///
/// The sanitizer boundary is pinned instead by `sanitize_*` and
/// `allowed_html_tags_admits_no_forbidden_tag` in
/// `crates/collab/src/import.rs`, which can see the sanitized *string* and
/// therefore the thing that actually varies.
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

    /// Handed XSS-shaped markup, the walker + materializer emit no forbidden
    /// element and no event-handler attribute — for arbitrary payloads.
    ///
    /// **Renamed from `from_html_strips_all_xss_vectors` (#160), which
    /// overpromised.** "Strips" is the sanitizer's job and this test cannot
    /// see the sanitizer: it inspects the yrs tree, where element names come
    /// from the closed `NodeType` enum and an `<iframe>` is unreachable by
    /// construction. The `<a href="javascript:…">` arm is decorative for the
    /// same reason — hrefs become text, and no assertion here reads them.
    ///
    /// What it does pin is still worth having: that the mapping layer stays
    /// inside the schema even when the DOM it is handed does not, so a future
    /// pass-through branch in `walk_html` (say, one that copied an unknown
    /// tag's name straight through) fails here. See [`assert_no_xss`].
    #[test]
    fn from_html_materialization_emits_no_forbidden_tag_or_handler(
        payload in "[a-z0-9 ='\"();]{0,40}",
        tag in prop::sample::select(vec!["script", "iframe", "object", "embed", "style", "form"]),
    ) {
        let html = format!(
            "<p>before</p><{tag}>{payload}</{tag}><img src=x onerror='{payload}'><a href=\"javascript:{payload}\">x</a><p>after</p>"
        );
        let doc = from_html(&html);
        assert_no_xss(&doc, &html)?;
    }

    /// Nested wrappers through the HTML importer never panic.
    ///
    /// Generated depth is bounded **under** `MAX_NESTING_DEPTH` on purpose.
    /// Past the cap the interesting failure mode is not a panic but a stack
    /// overflow, which aborts the process — proptest would kill the runner
    /// instead of shrinking a case, a useless CI signal. The cap gets the two
    /// deterministic tests at the bottom of this file; this property stays in
    /// panic-land, where `catch_unwind` can actually report.
    #[test]
    fn from_html_never_panics_on_bounded_nesting(
        depth in 0usize..(MAX_NESTING_DEPTH / 2),
        tag in prop::sample::select(vec!["div", "span", "blockquote", "ul", "li", "p", "pre"]),
        text in "[a-z <>&\"/=]{0,24}",
    ) {
        let html = format!(
            "{}{text}{}",
            format!("<{tag}>").repeat(depth),
            format!("</{tag}>").repeat(depth),
        );
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| from_html(&html)))
            .map_err(|_| TestCaseError::fail(format!("from_html panicked at depth {depth}")))?;
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

    /// Pins the **materialization path**, NOT the sanitizer allowlist.
    ///
    /// Read what this can actually observe before trusting it: `assert_no_xss`
    /// walks the materialized yrs document, whose element names come from the
    /// closed `NodeType` enum. It therefore cannot fail on anything the
    /// ammonia allowlist admits — widening `allowed_tags()` with `iframe` and
    /// `allowed_attributes()` with `onclick` leaves this test green, because
    /// materialization was never going to emit either one. The `javascript:`
    /// href arm is weaker still: hrefs become text *marks*, which this never
    /// inspects.
    ///
    /// What it does pin is real but narrow — that the block walker and
    /// `materialize` cannot be talked into emitting a forbidden element or an
    /// `on*` attribute even when handed one. **The allowlist itself is
    /// guarded by `import_quip`'s own unit tests**
    /// (`the_allowlist_admits_no_script_framing_or_event_handler` and
    /// `sanitize_strips_script_framing_and_event_handlers`), which live beside
    /// `sanitize()` because that is the only place it is reachable.
    #[test]
    fn from_quip_html_materialization_emits_no_xss_vectors(
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

// ─── Nesting depth: an abort, not a panic (#158) ────────────────
//
// Every property above wraps the parser in `catch_unwind`, which is the right
// net for a panic (an unwrap, an index, a bad slice boundary) and is *no* net
// at all for stack exhaustion: Rust aborts the process on stack overflow, so
// there is nothing to catch. Deeply-nested HTML is therefore a liveness bug
// the fuzz net structurally cannot see, and it needs deterministic tests with
// concrete depths rather than a generator.
//
// Before `MAX_NESTING_DEPTH`, `from_html` aborted at 3 000 levels of nested
// `<div>` / `<span>` / `<blockquote>` on the 2 MiB stack that tokio worker
// threads and Rust test threads both get (2 400 still parsed). `from_html` is
// reachable from `POST /documents/import` with `format: "html"` behind
// nothing but an `AuthUser` check, and a ~30 KB body was enough — so any
// authenticated user could kill the API process, taking every in-flight
// request and open WebSocket on it down with them.
//
// The same abort reached `from_quip_html` on the shared import worker, which
// died at ~1 050 levels of the same nesting: it killed every concurrent job,
// and the retry reached the same document and died again — a permanent wedge
// that charged no attempt and marked no thread failed, because the process
// never got to run that code.
//
// `<li>` and `<table>` are deliberately absent from these tests: html5ever
// routes them through non-recursive paths (a stray `<li>` gets wrapped, a
// `<table>` with no `<tr>` has no children to descend into), so they never
// reproduced the abort and asserting on them would pin the wrong thing.

/// Past the cap, but deliberately **below** the 3 000-level depth at which
/// the uncapped walker aborted — and below the 2 400 that still parsed. A
/// regression here therefore fails *cleanly* on the truncation assertion
/// instead of killing the runner, which is the signal you want in CI for the
/// ordinary mistake (cap raised, cap removed).
const PAST_CAP: usize = 500;

/// Past the depth at which the uncapped walker exhausted a 2 MiB stack, and
/// far below `ammonia::clean`'s own limit (measured surviving 50 000, where
/// our walker was already long dead).
const PAST_STACK_LIMIT: usize = 4_000;

/// The deepest cap this test will accept, written as a **literal rather than
/// in terms of `MAX_NESTING_DEPTH`**. An assertion phrased against the
/// constant would be self-approving: raising the cap to 4 000 would move the
/// bound along with it and stay green right up to the abort. This number is
/// the independent claim — 2 400 levels was the last depth the uncapped
/// walker survived on a 2 MiB stack, and anything within an order of
/// magnitude of that is not a safety margin.
const SAFE_DEPTH_CEILING: usize = 256;

fn nested(tag: &str, depth: usize) -> String {
    format!(
        "<p>top</p>{}deep-content{}",
        format!("<{tag}>").repeat(depth),
        format!("</{tag}>").repeat(depth),
    )
}

/// Nesting past the cap is flattened, not descended into — and the text
/// inside it survives.
///
/// This is the behavioral half, pinned at a depth the uncapped walker
/// survived, so removing or raising the cap fails this assertion cleanly
/// rather than aborting the process.
#[test]
// clippy would rather this be a `const` block. Kept a runtime assertion so
// the failure message can name the offending value — "raised to 2000" is the
// whole diagnostic, and a const assertion cannot interpolate it.
#[allow(clippy::assertions_on_constants)]
fn nesting_past_the_cap_is_truncated() {
    assert!(
        MAX_NESTING_DEPTH <= SAFE_DEPTH_CEILING,
        "MAX_NESTING_DEPTH was raised to {MAX_NESTING_DEPTH}; the uncapped walker \
         aborted the process at 3 000 levels on a 2 MiB stack, so a cap above \
         {SAFE_DEPTH_CEILING} is not a safety margin",
    );

    // `<blockquote>` is the observable case: it maps to a `NodeType`, so the
    // materialized tree records how deep the walker actually went. `<div>`
    // and `<span>` are transparent — for those, surviving the call and
    // keeping the text is the whole assertion.
    let doc = from_html(&nested("blockquote", PAST_CAP));
    let depth = tree_depth(&doc);
    assert!(
        depth <= SAFE_DEPTH_CEILING,
        "blockquote nested {PAST_CAP} deep materialized {depth} levels; the depth \
         cap did not bound the walk — at ~3 000 levels this aborts the process \
         instead of failing a test",
    );

    for tag in ["div", "span", "blockquote"] {
        // Failing SOFT is the point: dropping the subtree would silently
        // delete the deepest content of a document, and rejecting the import
        // would turn a formatting quirk into a failed request.
        let text = doc_text(&from_html(&nested(tag, PAST_CAP)));
        assert!(text.contains("top"), "<{tag}>: content above the cap must survive: {text:?}");
        assert!(
            text.contains("deep-content"),
            "<{tag}>: the text below the cap must survive the flattening: {text:?}",
        );
    }
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
        let doc = from_html(&nested(tag, PAST_STACK_LIMIT));
        assert!(
            doc_text(&doc).contains("deep-content"),
            "<{tag}>: the text must survive even at {PAST_STACK_LIMIT} levels",
        );

        let out = from_quip_html(&nested(tag, PAST_STACK_LIMIT));
        assert!(out.deep_nesting_truncated > 0, "<{tag}> must be truncated");
        assert!(
            doc_text(&out.doc).contains("deep-content"),
            "the text must survive even at {PAST_STACK_LIMIT} levels",
        );
    }
}

/// The cap must not touch documents that stay under it. A real document (Quip
/// or otherwise) nests around a dozen levels at most, so a cap that truncated
/// ordinary content would be a worse bug than the one it fixes.
#[test]
fn ordinary_nesting_is_not_truncated() {
    let depth = 16;
    let html = format!(
        "{}<p>nested</p>{}",
        "<blockquote>".repeat(depth),
        "</blockquote>".repeat(depth),
    );
    let doc = from_html(&html);
    assert_eq!(
        tree_depth(&doc),
        depth + 1,
        "{depth} levels of blockquote plus a paragraph is ordinary document \
         structure and must pass through untouched",
    );
    assert!(doc_text(&doc).contains("nested"));

    let out = from_quip_html(&html);
    assert_eq!(
        out.deep_nesting_truncated, 0,
        "{depth} levels is ordinary document structure and must pass through untouched",
    );
    assert!(doc_text(&out.doc).contains("nested"));
}

/// Deepest element chain in a document's `content` fragment. Iterative: it
/// runs against trees this file deliberately builds too deep to recurse over.
fn tree_depth(doc: &Doc) -> usize {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return 0;
    };
    let mut max = 0usize;
    let mut stack: Vec<(XmlElementRef, usize)> = Vec::new();
    for i in 0..fragment.len(&txn) {
        if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
            stack.push((el, 1));
        }
    }
    while let Some((el, d)) = stack.pop() {
        max = max.max(d);
        for i in 0..el.len(&txn) {
            if let Some(XmlOut::Element(child)) = el.get(&txn, i) {
                stack.push((child, d + 1));
            }
        }
    }
    max
}

/// Every text run in a document's `content` fragment, space-separated.
/// Iterative for the same reason [`tree_depth`] is. Document order is not
/// preserved and no caller depends on it — these assertions read membership.
fn doc_text(doc: &Doc) -> String {
    let txn = doc.transact();
    let mut out = String::new();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return out;
    };
    let mut stack: Vec<XmlElementRef> = Vec::new();
    for i in 0..fragment.len(&txn) {
        if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
            stack.push(el);
        }
    }
    while let Some(el) = stack.pop() {
        for i in 0..el.len(&txn) {
            match el.get(&txn, i) {
                Some(XmlOut::Element(child)) => stack.push(child),
                Some(XmlOut::Text(t)) => {
                    out.push(' ');
                    out.push_str(&t.get_string(&txn));
                }
                _ => {}
            }
        }
    }
    out
}
