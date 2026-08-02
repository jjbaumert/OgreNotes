//! Regression net over **real** Quip thread bodies.
//!
//! # Why this file exists
//!
//! Seven separate content-import bugs (#169, #173, #175, #176, #184, #187,
//! #189) shared one root cause: every hand-authored fixture in the suite
//! encoded an HTML shape Quip never emits. The most direct instance is
//! `nested_lists_stay_inside_their_item()` in `import_quip.rs`, which asserts
//! on `<ul><li>outer<ul><li>inner</li></ul></li></ul>` — a shape with **zero**
//! occurrences in 56 real documents — while the shape Quip actually emits
//! (a `<ul>` that is a *sibling* of the `<li>`, 470 occurrences) is silently
//! flattened. Every one of those bugs was found by a human opening a
//! document; none by CI.
//!
//! The fixtures in `tests/fixtures/quip/corpus/` are real thread bodies,
//! staged to `s3://test1-ogrenote/imports/` by the import worker and captured
//! on 2026-08-01 — byte-for-byte what `from_quip_html` receives in
//! production. The tests assert **structural counts**, never exact output
//! strings, so a legitimate formatting change does not turn the net red while
//! a structural regression does.
//!
//! # These fixtures are scrubbed, not verbatim
//!
//! The corpus is one person's real Quip account and this repository is
//! public. Each fixture was passed through a text-only substitution:
//!
//! * every word run (`[A-Za-z][A-Za-z0-9]*`) and every digit run (`[0-9]+`)
//!   **inside a text node** was replaced with neutral filler of the
//!   **identical length**, preserving the lower/Title/UPPER case pattern;
//! * the same substitution was applied inside the values of `alt`, `title`,
//!   and `href` — the latter only when the host is not `quip.com`, and never
//!   to the URL scheme;
//! * **nothing else was touched.** Not one tag, attribute name, quote style,
//!   self-closing spelling, or byte of whitespace. Every other attribute
//!   value is byte-exact: `id`, `class`, `style` (including
//!   `--indent0: N`), `value`, `formula`, `src`, `width`, `height`, and
//!   every `data-*`. Entity references (`&lt;` `&gt;` `&amp;`), U+00A0
//!   no-break spaces (Quip indents code with NBSP, not ASCII spaces),
//!   U+200B zero-width spaces (Quip's spelling of an empty paragraph or
//!   cell), smart quotes, en/em dashes and arrows all survive verbatim —
//!   those *are* the test.
//!
//! Because only `[A-Za-z0-9]` code points were substituted, and always
//! length-for-length, the character, word and sentence counts of the prose
//! are unchanged. Every count asserted below was checked to be identical for
//! the scrubbed fixture and the untouched original. The prose itself is
//! meaningless — read the markup, not the words.
//!
//! Quip section ids (`temp:C:AeO6b3a…`, `SSfACAf6Nwn`) are opaque handles and
//! are kept verbatim; capturing them was the subject of #190.
//!
//! # Two staging shapes
//!
//! Objects staged before #171 are JSON envelopes (`{"html": …}`); later ones
//! are raw HTML. That unwrapping happens in the **worker**
//! (`crates/api/src/worker_mode.rs`), not in this crate — `from_quip_html`
//! only ever sees HTML, so the fixtures here are the extracted bodies. Both
//! staged spellings of each of these five threads were confirmed to carry a
//! byte-identical body.
//!
//! # Some assertions below encode CURRENT, BUGGY behaviour
//!
//! This net asserts today's wrong values so that landing a fix makes the
//! change **visible** rather than silent. Every such assertion carries a
//! `// #NNN will …` comment naming the ticket that is expected to change it.
//! Updating one of those numbers alongside its fix is correct; updating one
//! without a corresponding fix is a regression.
//!
//! **No forecast is currently open.** #187 (nesting) and #188 (numbering)
//! landed together in PR #200 and their assertions now record the fixed
//! values, annotated `#NNN (PR #200)`. #189 (trailing breaks) has since
//! landed too and its four assertions now record the achieved value — zero
//! hard breaks in all five fixtures. #190 (section anchors) has now landed
//! as well: the five `sections.len()` assertions went 11→71, 23→170, 44→46,
//! 5→10 and 1→528, and every one of them is now stated as *achieved*.
//!
//! # Known coverage gaps in this fixture set
//!
//! Deliberate, so nobody mistakes a zero for a passing assertion: no fixture
//! here contains `<hr>`, inline `<code>`, `<sup>`, a live-app payload (#191),
//! a nested table, or `colspan`/`rowspan`. The corpus documents richest in
//! images and external links are also the most personal (recruiter
//! correspondence, financial holdings), and scrubbing their URLs would gut
//! exactly what makes a link fixture meaningful — so image coverage here is
//! the single benign image section in `ZaNAAAU4ELc`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ogrenotes_collab::import_quip::{
    from_quip_html, from_quip_html_as, QuipDocument, QuipThreadKind,
};
use yrs::types::xml::{XmlFragment, XmlOut};
use yrs::{Any, Doc, Out, ReadTxn, Text, Transact, Xml, XmlElementRef};

/// The five thread bodies checked in, in the order they are described above.
const CORPUS: &[&str] =
    &["AeOAAAcV1hg", "CVLAAAgSl7Q", "ZaNAAAU4ELc", "SSfAAALs7fy", "QGYAAAjicgG"];

// ─── fixture loading ─────────────────────────────────────────────

fn fixture(thread_id: &str) -> String {
    let path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/quip/corpus")
        .join(format!("{thread_id}.html"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

// ─── the census ──────────────────────────────────────────────────

/// A structural census of one imported document: what the walker actually
/// produced, counted by kind. Deliberately count-based — the point is to
/// notice a structural change, not to freeze a serialization.
#[derive(Debug, Default)]
struct Census {
    /// `heading` elements by their `level` attribute.
    headings: BTreeMap<String, usize>,
    paragraphs: usize,
    bullet_lists: usize,
    ordered_lists: usize,
    task_lists: usize,
    list_items: usize,
    task_items: usize,
    /// Deepest list nesting reached anywhere in the output. Quip's own
    /// nesting reaches 2–3, and since #187 (PR #200) so does the output;
    /// this field used to measure exactly the gap between the two.
    max_list_depth: usize,
    /// Items in each top-level `ordered_list`, in document order.
    ordered_list_item_counts: Vec<usize>,
    /// Lengths of maximal runs of consecutive top-level `ordered_list`
    /// blocks. A `bullet_list` between two of them does **not** end a run:
    /// that is Quip's sub-bullet section, and #188 (PR #200) merged the
    /// numbered sections it separates. Post-fix every entry should be 1 —
    /// a run longer than that is a numbered sequence that failed to merge.
    ordered_list_runs: Vec<usize>,
    hard_breaks: usize,
    /// `hard_break` elements that are the LAST child of their paragraph or
    /// heading — the artifact #189 removed. Zero everywhere since that fix;
    /// it is kept because it is the shape that distinguishes a terminator
    /// from an authored break, so a regression shows up here specifically.
    trailing_hard_breaks: usize,
    code_blocks: usize,
    /// Total newline-separated lines across all `code_block` elements.
    /// One line per block would mean `<br>` was dropped rather than turned
    /// into a newline (#184, fixed at 89050d8).
    code_lines: usize,
    tables: usize,
    table_rows: usize,
    table_cells: usize,
    table_headers: usize,
    blockquotes: usize,
    horizontal_rules: usize,
    images: usize,
    mentions: usize,
    doc_mentions: usize,
    /// Mark runs by attribute name, as `Text::diff` reports them.
    marks: BTreeMap<String, usize>,
}

impl Census {
    fn lists(&self) -> usize {
        self.bullet_lists + self.ordered_lists + self.task_lists
    }

    fn mark(&self, name: &str) -> usize {
        self.marks.get(name).copied().unwrap_or(0)
    }
}

fn census(doc: &Doc) -> Census {
    let mut c = Census::default();
    let txn = doc.transact();
    let Some(frag) = txn.get_xml_fragment("content") else { return c };

    let mut run = 0usize;
    for i in 0..frag.len(&txn) {
        let Some(XmlOut::Element(el)) = frag.get(&txn, i) else { continue };
        match el.tag().as_ref() {
            "ordered_list" => {
                c.ordered_list_item_counts.push(el.len(&txn) as usize);
                run += 1;
            }
            // Quip splits a numbered sequence with sub-bullet sections; they
            // sit between two numbered sections without ending the sequence.
            "bullet_list" => {}
            _ => {
                if run > 0 {
                    c.ordered_list_runs.push(run);
                    run = 0;
                }
            }
        }
        census_element(&txn, &el, &mut c, 0);
    }
    if run > 0 {
        c.ordered_list_runs.push(run);
    }
    c
}

fn census_element<T: ReadTxn>(txn: &T, el: &XmlElementRef, c: &mut Census, list_depth: usize) {
    let tag = el.tag().to_string();
    let depth = match tag.as_str() {
        "heading" => {
            let level = el.get_attribute(txn, "level").unwrap_or_else(|| "?".into());
            *c.headings.entry(level).or_default() += 1;
            list_depth
        }
        "paragraph" => {
            c.paragraphs += 1;
            list_depth
        }
        "bullet_list" => {
            c.bullet_lists += 1;
            list_depth + 1
        }
        "ordered_list" => {
            c.ordered_lists += 1;
            list_depth + 1
        }
        "task_list" => {
            c.task_lists += 1;
            list_depth + 1
        }
        "list_item" => {
            c.list_items += 1;
            list_depth
        }
        "task_item" => {
            c.task_items += 1;
            list_depth
        }
        "hard_break" => {
            c.hard_breaks += 1;
            list_depth
        }
        "code_block" => {
            c.code_blocks += 1;
            c.code_lines += code_block_lines(txn, el);
            list_depth
        }
        "table" => {
            c.tables += 1;
            list_depth
        }
        "table_row" => {
            c.table_rows += 1;
            list_depth
        }
        "table_cell" => {
            c.table_cells += 1;
            list_depth
        }
        "table_header" => {
            c.table_headers += 1;
            list_depth
        }
        "blockquote" => {
            c.blockquotes += 1;
            list_depth
        }
        "horizontal_rule" => {
            c.horizontal_rules += 1;
            list_depth
        }
        "image" => {
            c.images += 1;
            list_depth
        }
        "mention" => {
            c.mentions += 1;
            list_depth
        }
        "doc_mention" => {
            c.doc_mentions += 1;
            list_depth
        }
        _ => list_depth,
    };
    c.max_list_depth = c.max_list_depth.max(depth);

    let n = el.len(txn);
    if matches!(tag.as_str(), "paragraph" | "heading")
        && n > 0
        && let Some(XmlOut::Element(last)) = el.get(txn, n - 1)
        && last.tag().as_ref() == "hard_break"
    {
        c.trailing_hard_breaks += 1;
    }

    for i in 0..n {
        match el.get(txn, i) {
            Some(XmlOut::Element(child)) => census_element(txn, &child, c, depth),
            Some(XmlOut::Text(text)) => {
                for delta in text.diff(txn, yrs::types::text::YChange::identity) {
                    if let Some(attrs) = delta.attributes {
                        for (name, _) in attrs.iter() {
                            *c.marks.entry(name.to_string()).or_default() += 1;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn code_block_lines<T: ReadTxn>(txn: &T, el: &XmlElementRef) -> usize {
    let mut body = String::new();
    for i in 0..el.len(txn) {
        match el.get(txn, i) {
            Some(XmlOut::Text(text)) => {
                for delta in text.diff(txn, yrs::types::text::YChange::identity) {
                    if let Out::Any(Any::String(s)) = &delta.insert {
                        body.push_str(s.as_ref());
                    }
                }
            }
            // A `hard_break` inside a code block would mean #184 regressed to
            // an element rather than a newline; count it as a line break so
            // the number still reflects what a reader sees.
            Some(XmlOut::Element(child)) if child.tag().as_ref() == "hard_break" => body.push('\n'),
            _ => {}
        }
    }
    body.lines().count()
}

// ─── source-side count: how many anchors were there to capture? ───

/// Number of elements carrying an `id` attribute in the source HTML.
///
/// This is the denominator for #190. It counts *elements*, not distinct
/// ids, and Quip repeats an item's id on its inner `<span>` — so the
/// captured count is never expected to equal this number. What it is
/// expected to approach is the count of **distinct** ids, which
/// [`source_ids_by_tag`] separates out.
fn source_id_count(html: &str) -> usize {
    source_ids_by_tag(html).len()
}

/// Every `(tag, id)` pair in the source, in document order — the shape
/// needed to say *which* anchors a change stopped capturing, not merely
/// how many.
fn source_ids_by_tag(html: &str) -> Vec<(String, String)> {
    use html5ever::tendril::TendrilSink;
    use markup5ever_rcdom::{Handle, NodeData, RcDom};

    let dom = html5ever::parse_document(RcDom::default(), Default::default())
        .from_utf8()
        .read_from(&mut html.as_bytes())
        .expect("html5ever parse is infallible");

    fn walk(node: &Handle, out: &mut Vec<(String, String)>) {
        if let NodeData::Element { name, attrs, .. } = &node.data {
            for a in attrs.borrow().iter() {
                if a.name.local.as_ref().eq_ignore_ascii_case("id") {
                    out.push((name.local.to_string(), a.value.to_string()));
                }
            }
        }
        for child in node.children.borrow().iter() {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(&dom.document, &mut out);
    out
}

fn import(thread_id: &str) -> (String, QuipDocument, Census) {
    let html = fixture(thread_id);
    let quip = from_quip_html(&html);
    let c = census(&quip.doc);
    (html, quip, c)
}

// ─── grid readout (#230) ─────────────────────────────────────────
//
// The census counts cells; it cannot say *where* a value landed, which is
// the whole of #230. These read the first table back as a grid so a test
// can assert a position.

/// All text under `el`, descendants included, in document order.
fn element_text<T: ReadTxn>(txn: &T, el: &XmlElementRef) -> String {
    let mut body = String::new();
    for i in 0..el.len(txn) {
        match el.get(txn, i) {
            Some(XmlOut::Text(text)) => {
                for delta in text.diff(txn, yrs::types::text::YChange::identity) {
                    if let Out::Any(Any::String(s)) = &delta.insert {
                        body.push_str(s.as_ref());
                    }
                }
            }
            Some(XmlOut::Element(child)) => body.push_str(&element_text(txn, &child)),
            _ => {}
        }
    }
    body
}

fn find_table<T: ReadTxn>(txn: &T, el: &XmlElementRef) -> Option<XmlElementRef> {
    if el.tag().as_ref() == "table" {
        return Some(el.clone());
    }
    for i in 0..el.len(txn) {
        if let Some(XmlOut::Element(child)) = el.get(txn, i)
            && let Some(found) = find_table(txn, &child)
        {
            return Some(found);
        }
    }
    None
}

/// The document's first `table`, row-major, as `(is_header_cell, text)`.
fn first_table_grid(doc: &Doc) -> Vec<Vec<(bool, String)>> {
    let txn = doc.transact();
    let Some(frag) = txn.get_xml_fragment("content") else { return Vec::new() };
    let mut found = None;
    for i in 0..frag.len(&txn) {
        let Some(XmlOut::Element(el)) = frag.get(&txn, i) else { continue };
        if let Some(t) = find_table(&txn, &el) {
            found = Some(t);
            break;
        }
    }
    let Some(table) = found else { return Vec::new() };
    let mut grid = Vec::new();
    for r in 0..table.len(&txn) {
        let Some(XmlOut::Element(row)) = table.get(&txn, r) else { continue };
        let mut cells = Vec::new();
        for c in 0..row.len(&txn) {
            let Some(XmlOut::Element(cell)) = row.get(&txn, c) else { continue };
            cells.push((cell.tag().as_ref() == "table_header", element_text(&txn, &cell)));
        }
        grid.push(cells);
    }
    grid
}

/// Column `index` of a grid, top to bottom.
fn column(grid: &[Vec<(bool, String)>], index: usize) -> Vec<&str> {
    grid.iter().filter_map(|r| r.get(index)).map(|(_, t)| t.as_str()).collect()
}

// ─── per-fixture assertions ──────────────────────────────────────

/// `AeOAAAcV1hg` — Quip's nested-list spelling, plus two table sections.
///
/// The source has one `data-section-style='5'` bullet section whose `<ul>`
/// contains an `<li class='parent'>` followed by a **sibling** `<ul>`. That
/// is the only nesting spelling Quip emits (470 sites across 24 documents);
/// the standard `<ul>` inside `<li>` occurs zero times in the whole corpus.
#[test]
fn corpus_nested_lists_and_tables() {
    let (html, quip, c) = import("AeOAAAcV1hg");

    assert_eq!(c.headings.get("1"), Some(&1), "h1");
    assert_eq!(c.headings.get("2"), Some(&2), "h2");
    assert_eq!(c.headings.get("3"), Some(&1), "h3");
    assert_eq!(c.paragraphs, 47, "paragraph blocks (7 <p> + 38 <td> + 2 <li>)");

    // One bullet section holding the `class='parent'` item, plus the nested
    // list built from the sibling `<ul>` beneath it. #187 (PR #200) raised
    // this from 1: the sibling `<ul>` is now a real nested `bullet_list`
    // inside the preceding `<li>` rather than having its item hoisted into
    // the outer list.
    assert_eq!(c.bullet_lists, 2, "bullet_list blocks — outer plus the nested sibling <ul>");
    assert_eq!(c.ordered_lists, 0);
    // Nesting moves an item, it never creates or destroys one.
    assert_eq!(c.list_items, 2, "list_item elements");
    // #187 (PR #200) raised both of these from 1: the output now reproduces
    // the two levels the source nests.
    assert_eq!(c.max_list_depth, 2, "output list depth — matches the source's 2 levels");
    assert_eq!(c.lists(), 2, "total list blocks");

    assert_eq!(c.tables, 2, "data-section-style='13' sections");
    assert_eq!(c.table_rows, 17, "table_row");
    assert_eq!(c.table_cells, 38, "table_cell");
    assert_eq!(c.table_headers, 0, "this document uses <td> for its header row");

    assert_eq!(c.code_blocks, 0);
    assert_eq!(c.images, 0);
    assert_eq!(quip.images.len(), 0);
    assert_eq!(c.horizontal_rules, 0);
    assert_eq!(c.blockquotes, 0);

    // Every `<li>` and every `<td>` in the source ends in `<br/>` — 40 of
    // them, which the walker used to keep as 40 trailing `hard_break`
    // leaves. #189 drops the terminator; this document has no mid-content
    // `<br>` at all, so nothing authored is left behind and both counts are
    // 0. The 38 `table_cell` and 2 `list_item` counts above are unchanged by
    // that, which is the point: a terminator went away, no content did.
    assert_eq!(c.hard_breaks, 0, "hard_break leaves — every <br> here was a terminator (#189)");
    assert_eq!(c.trailing_hard_breaks, 0, "no trailing hard_break survives (#189)");

    assert_eq!(c.mark("bold"), 18, "bold runs");
    assert_eq!(c.mark("italic"), 0);
    assert_eq!(c.mark("link"), 0);
    assert_eq!(c.mark("code"), 0);

    // #190 (achieved): every id-bearing element that becomes a block is now
    // recorded. 11 → 71, which is **every distinct id in the document**:
    // 4 headings + 7 `<p>` + 1 `<ul>` + 2 `<li>` + 2 `<table>` + 17 `<tr>`
    // + 38 `<td>`. The 111 − 71 = 40 remaining `id` attributes are the
    // `<span>` inside each `<li>`/`<td>`, and every one of them repeats its
    // parent's id byte for byte — so those 40 anchors resolve through the
    // 40 entries already recorded rather than needing their own.
    assert_eq!(source_id_count(&html), 111, "id attributes present in the source");
    assert_eq!(quip.sections.len(), 71, "section ids captured — every distinct id (#190)");

    assert_eq!(quip.deep_nesting_truncated, 0, "nothing exceeded MAX_NESTING_DEPTH");
}

/// `CVLAAAgSl7Q` — numbered procedures and code blocks.
///
/// Twelve `data-section-style='6'` sections, each holding exactly one step of
/// a numbered procedure and separated by `data-section-style='5'` sub-bullet
/// sections; several carry `class="list-numbering-restart-at"` and
/// `style="--indent0: N"`. Four `<pre class='prettyprint'>` blocks whose line
/// separator is `<br>` and whose indentation is U+00A0.
#[test]
fn corpus_numbered_sequences_and_code_blocks() {
    let (html, quip, c) = import("CVLAAAgSl7Q");

    assert_eq!(c.headings.get("1"), Some(&1), "h1");
    assert_eq!(c.headings.get("2"), Some(&5), "h2");
    assert_eq!(c.headings.get("3"), Some(&10), "h3");
    assert_eq!(c.paragraphs, 126, "paragraph blocks");

    // #187 (PR #200) raised this from 22. The source nests a `<ul>` directly
    // inside a `<ul>` in 20 places; in 8 of them the nested list follows an
    // `<li class='parent'>` as a sibling, and each of those 8 used to have
    // its items hoisted into the outer list instead of becoming a list of
    // its own. 22 + 8 = 30.
    assert_eq!(c.bullet_lists, 30, "data-section-style='5' sections plus 8 nested sibling <ul>");
    // #188 (PR #200) lowered this from 12: the twelve single-step
    // `data-section-style='6'` sections merge into the two numbered runs the
    // reader actually sees.
    assert_eq!(c.ordered_lists, 2, "merged numbered runs — the source has 12 sections");
    // Neither nesting nor merging creates or destroys an item.
    assert_eq!(c.list_items, 119, "list_item elements");
    // #187 (PR #200) raised this from 1. Three levels, not the two the
    // ticket predicted: the source has `<div><ul><ul>…</ul></ul></div>`
    // indent wrappers, and inside one of those a further sibling `<ul>`
    // hangs off an `<li class='parent'>`.
    assert_eq!(c.max_list_depth, 3, "output list depth — the source nests 3 deep");

    // #188 (PR #200): each of the twelve numbered steps used to be its own
    // single-item `ordered_list`, so the reader saw "1." twelve times
    // instead of 1–7 and 1–5. `[1; 12]` is now `[7, 5]` — the same twelve
    // steps, gathered into the two runs the author wrote. The split is 7/5
    // and not 7/2/3 because Quip's continues-class spans the intervening
    // `<pre>`: a code block between two numbered steps does not end the run
    // (pinned directly by `a_code_block_between_two_numbered_items_does_
    // not_end_the_run` in `import_quip.rs`).
    assert_eq!(
        c.ordered_list_item_counts,
        vec![7, 5],
        "items per ordered_list — the twelve steps merged into two runs (#188)"
    );
    // Each run is now a single `ordered_list` block, so no run is longer
    // than one; #188 (PR #200) collapsed `[7, 2, 3]` to `[1, 1]`.
    assert_eq!(
        c.ordered_list_runs,
        vec![1, 1],
        "one ordered_list per numbered run — nothing left to merge"
    );
    // The merge redistributes items between lists; it must not lose one.
    assert_eq!(
        c.ordered_list_item_counts.iter().sum::<usize>(),
        12,
        "the source's twelve numbered steps all survive"
    );

    // #184 (fixed at 89050d8): a `<br>` inside `<pre>` became a newline.
    // A revert would collapse `code_lines` to 4.
    assert_eq!(c.code_blocks, 4, "<pre> blocks");
    assert_eq!(c.code_lines, 63, "lines across all code blocks");

    assert_eq!(c.tables, 0);
    assert_eq!(c.images, 0);
    assert_eq!(c.blockquotes, 0);
    assert_eq!(c.horizontal_rules, 0);

    // The source has 178 `<br>`: 119 terminate an `<li>` and the other 59
    // are code-block line separators, which are newlines rather than leaves.
    // #189 drops the 119 terminators and leaves the 59 alone — `code_lines`
    // above stays at 63 across the same 4 blocks, which is the proof that
    // the two rules did not get unified. The `<br>` inside a `<pre>` is
    // reached through `raw_text` (#184) and never through this path.
    assert_eq!(c.hard_breaks, 0, "hard_break leaves — the 119 <li> terminators are gone (#189)");
    assert_eq!(c.trailing_hard_breaks, 0, "no trailing hard_break survives (#189)");

    assert_eq!(c.mark("bold"), 73, "bold runs");
    assert_eq!(c.mark("link"), 1, "the one external <a href>");
    assert_eq!(c.mark("code"), 0, "Quip emits no inline <code> here");

    // #190 (achieved): 23 → 170. The document holds 180 distinct ids (299
    // attributes less the 119 `<span>`s that repeat their `<li>`'s), so 170
    // is 10 short — and those 10 are named and expected. They are the `<ul>`
    // elements of the ten `'6'` continuation sections that #188 (PR #200)
    // merges away: `merge_numbered_sections` empties each into the
    // accumulating list and drops the now-contentless section before the
    // walker runs, so its section-level anchor has no block left to name.
    // Their *items* all survive with their own anchors — the 119 `<li>` ids
    // are all here. `every_uncaptured_source_id_is_one_of_the_two_known_
    // residues` below pins this exactly, so a new drop cannot hide in it.
    assert_eq!(source_id_count(&html), 299, "id attributes present in the source");
    assert_eq!(quip.sections.len(), 170, "section ids captured (#190)");

    assert_eq!(quip.pending_links.len(), 0, "the only anchor is external");
    assert_eq!(quip.person_mentions.len(), 0);
    assert_eq!(quip.deep_nesting_truncated, 0);
}

/// `ZaNAAAU4ELc` — an image section, flex columns, a blockquote, italics.
///
/// The image is a bare `<img>` inside `<div data-section-style='11'>`; Quip
/// never wraps it in a `<p>`. The `display: flex` wrappers are the shape Quip
/// uses for image-plus-caption pairing, flattened into document order by the
/// walker (audit F-9 — content-preserving and accepted).
///
/// This document contains no `<li>` and no `<td>`, so it is also the control
/// for #189: the trailing-break fix must leave it at zero hard breaks.
#[test]
fn corpus_image_section_and_blockquote() {
    let (html, quip, c) = import("ZaNAAAU4ELc");

    assert_eq!(c.headings.get("1"), Some(&1), "h1");
    assert_eq!(c.paragraphs, 44, "paragraph blocks, most of them U+200B spacers");

    assert_eq!(c.images, 1, "image blocks");
    assert_eq!(quip.images.len(), 1, "image side-table entries");
    assert_eq!(
        quip.images[0].src, "https://quip.com/blob/ZaNAAAU4ELc/jG4ISoLLsz9JZ2nahGsoSg",
        "the blob URL the caller must side-load"
    );
    assert_eq!(quip.images[0].block_id.len(), 10, "the image is keyed by a minted blockId");

    assert_eq!(c.blockquotes, 1);
    assert_eq!(c.lists(), 0, "no lists in this document");
    assert_eq!(c.tables, 0);
    assert_eq!(c.code_blocks, 0);

    assert_eq!(c.hard_breaks, 0, "the source contains no <br> at all");
    assert_eq!(c.trailing_hard_breaks, 0, "#189 must leave this at zero");

    assert_eq!(c.mark("italic"), 1, "italic run");
    assert_eq!(c.mark("bold"), 0);
    assert_eq!(c.mark("link"), 0);

    // #190 (achieved): 44 → 46. This document has no `<span>` id and no
    // `<li>`/`<td>`, so source ids and distinct ids coincide; the two that
    // used to be missed were the `<img>` (audit F-4 counts one per image
    // section) and the `<blockquote>`, and both are now recorded. Every
    // anchor in this document is addressable.
    assert_eq!(source_id_count(&html), 46, "id attributes present in the source");
    assert_eq!(quip.sections.len(), 46, "section ids captured — all of them (#190)");

    assert_eq!(quip.deep_nesting_truncated, 0);
}

/// `SSfAAALs7fy` — the stock "Welcome to Quip" thread.
///
/// The corpus's only checklist and its only `<control>` wrappers. The
/// checklist is a plain `<ul>` inside `<div data-section-style='7'>`: no
/// `<input>`, no `checked`, no check-ish class token anywhere — which is why
/// #183 (checked state) is blocked on measurement rather than on code.
#[test]
fn corpus_checklist_and_control_wrappers() {
    let (html, quip, c) = import("SSfAAALs7fy");

    assert_eq!(c.headings.get("1"), Some(&1), "h1");
    assert_eq!(c.paragraphs, 8, "4 <p> plus one per checklist item");

    // #173: the checklist is spelled `data-section-style='7'`, never
    // `class='checklist'`. A regression here degrades it to a bullet list.
    assert_eq!(c.task_lists, 1, "data-section-style='7' becomes a task_list");
    assert_eq!(c.task_items, 4, "task_item elements");
    assert_eq!(c.bullet_lists, 0, "the checklist must not degrade to a bullet list");
    assert_eq!(c.ordered_lists, 0);
    assert_eq!(c.list_items, 0);

    // #175: `<control data-remapped="true">` is transparent. Three of them
    // here — two wrapping a `quip.com` anchor, one empty (Quip's
    // client-rendered date, which the export genuinely does not carry).
    // Neither anchor has a `#temp:C:…` fragment, so both are provisionally
    // person mentions; `resolve_person_mentions` is what later degrades the
    // one that turns out to be a folder rather than a person.
    assert_eq!(c.mentions, 2, "both control-wrapped anchors become mention leaves");
    assert_eq!(c.doc_mentions, 0, "neither anchor carries a section fragment");
    assert_eq!(quip.person_mentions.len(), 2);
    let mut quip_user_ids: Vec<&str> =
        quip.person_mentions.iter().map(|m| m.quip_user_id.as_str()).collect();
    quip_user_ids.sort_unstable();
    assert_eq!(quip_user_ids, ["JAdAOAxYGcQ", "XYJAEA0Sgev"], "ids taken from the anchor hrefs");
    assert_eq!(quip.pending_links.len(), 0, "no anchor reached the Phase-2b table");
    assert!(
        quip.person_mentions.iter().all(|m| !m.block_id.is_empty()),
        "every mention leaf is addressable"
    );

    // The empty `<control>` must contribute nothing and disturb nothing.
    assert!(html.contains("<control data-remapped=\"true\" id=\"SSfACAsTxeJ\"></control>"));

    // #189 zeroed both: the source's one trailing `<br/>` per checklist item
    // is a terminator, and a `task_item` is terminated exactly like an `<li>`
    // — `task_items` above stays at 4.
    assert_eq!(c.hard_breaks, 0, "the 4 per-item terminators are gone (#189)");
    assert_eq!(c.trailing_hard_breaks, 0, "no trailing hard_break survives (#189)");

    assert_eq!(c.tables, 0);
    assert_eq!(c.images, 0);
    assert_eq!(c.code_blocks, 0);

    // #190 (achieved): 5 → 10 = h1 + 4 `<p>` + the `<ul>` + its 4 `<li>`.
    // Of the 17 − 10 = 7 remaining `id` attributes, 4 are the per-item
    // `<span>`s repeating their `<li>`'s id and 3 are `<control>` wrappers.
    // A `<control>` is an inline entity, not a section: two of these become
    // `Mention` leaves and the third is the empty client-rendered date,
    // which materializes nothing at all — so there is no block to name and
    // no corpus anchor that targets one.
    assert_eq!(source_id_count(&html), 17, "id attributes present in the source");
    assert_eq!(quip.sections.len(), 10, "section ids captured (#190)");

    assert_eq!(quip.deep_nesting_truncated, 0);
}

/// `QGYAAAjicgG` — a Quip spreadsheet thread.
///
/// The widest table in the corpus and the densest concentration of section
/// anchors: 1008 `id` attributes, nearly all on `<td>` and `<span>`. It also
/// carries two `formula` attributes, which the import now counts but still
/// does not carry (#192), and 469 U+200B-only cells.
#[test]
fn corpus_spreadsheet_section_id_density() {
    let (html, quip, c) = import("QGYAAAjicgG");

    assert_eq!(c.headings.get("1"), Some(&1), "h1");
    assert_eq!(c.paragraphs, 527, "one per cell, plus the spacer paragraphs");

    assert_eq!(c.tables, 1, "one data-section-style='13' section");
    assert_eq!(c.table_rows, 31, "table_row");
    assert_eq!(c.table_cells, 510, "table_cell");
    assert_eq!(c.table_headers, 17, "<th> column headers — <thead> is transparent");

    assert_eq!(c.lists(), 0);
    assert_eq!(c.code_blocks, 0);
    assert_eq!(c.images, 0);
    assert_eq!(c.marks.len(), 0, "a spreadsheet carries no inline marks");

    // #189 zeroed both. 496 terminators, not 527: the 30 row-number `<td>`
    // and the corner `<th class='empty' style='width: 2em'/>` carry none —
    // which is itself the discriminator working, since a cell without a
    // terminating `<br/>` must come through unchanged. `table_cells` and
    // `table_headers` above are untouched, and so is the paragraph count
    // pinned by `zero_width_spacers_survive_as_paragraphs`.
    assert_eq!(c.hard_breaks, 0, "hard_break leaves — 496 cell terminators dropped (#189)");
    assert_eq!(c.trailing_hard_breaks, 0, "no trailing hard_break survives (#189)");

    // #192, source side. Two — not the "28–30" the remediation brief
    // predicted, and not the issue's "30 across 2 documents" either: that
    // number is over the 56-document audit corpus, of which these five are
    // the checked-in sample. This is the whole formula population available
    // in-repo, and it is what any fix here can be pinned against.
    assert_eq!(html.matches("formula='").count(), 2, "formulas present in the source");

    // #192, import side. `formula` now clears the sanitizer, so the import
    // can *count* what it does not carry (`import_quip::FORMULA_ATTR`) — the
    // difference between a lossy import and a silent one. It still does not
    // carry it: the cells hold the values Quip last computed, nothing
    // recalculates, and the number below is what the worker turns into the
    // user's report note.
    //
    // Landing the formulas looks closer than it may read. A native sheet has
    // no `formula` attribute — the formula **is** the cell's text, re-parsed
    // on load (`spreadsheet_view/persistence.rs` hydrates each cell with
    // `engine.set_cell(addr, &cell_node.text_content())`), and both formulas
    // here are inside the native grammar (`*`, `SUM`, A1 ranges). Quip
    // reports this thread's type as `spreadsheet` and `worker_mode` already
    // maps that to `DocType::Spreadsheet`, so writing the formula as the
    // cell's text — rather than the cached value — is the shape of the fix,
    // and it drives this assertion to 0.
    //
    // What still has to be settled first, and why this is a separate unit:
    // the sheet attrs (`sheetName`, the grid bounds) are not emitted by this
    // walker, Quip's cell geometry has to be mapped onto `(col, row)` past
    // the row-number `<td>` and corner `<th>`, and a formula *outside* the
    // native grammar needs the literal-text-plus-note fallback rather than a
    // silently different answer. Note also that the same attribute can ride
    // in a table embedded in an ordinary `document` thread, where there is no
    // sheet to be live in and the cached value is the better import — so the
    // fix is conditioned on the thread's type, not on the attribute alone.
    assert_eq!(quip.formulas_dropped, 2, "both source formulas are reported as lost");
    assert_eq!(quip.live_apps_dropped, 0, "a spreadsheet is not a live app");

    // The sharpest statement of #190 in the suite, now the other way round:
    // 1 → 528. Where only the `<h1>` used to be captured — so an anchor
    // pointing at any cell of this sheet could never be resolved by the
    // Phase-2b back-patch — every distinct id in the document is now
    // recorded: 1 `<h1>` + 1 `<table>` + 30 `<tr>` + 480 `<td>` + 16 `<th>`.
    // The other 480 `id` attributes are the per-cell `<span>`s repeating
    // their `<td>`'s id, so every one of the 1008 anchors resolves.
    //
    // 528 entries is also the corpus's worst case for the `SECMAP#` rows:
    // one chunk (`SECMAP_CHUNK_ENTRIES` = 2000), one write, ~62 KB against
    // DynamoDB's 400 KB item cap.
    assert_eq!(source_id_count(&html), 1008, "id attributes present in the source");
    assert_eq!(quip.sections.len(), 528, "section ids captured — every distinct id (#190)");

    assert_eq!(quip.deep_nesting_truncated, 0);
}

/// **#230.** The same fixture, imported as the *spreadsheet* Quip says it is.
///
/// Quip renders a sheet as an HTML table that includes its own rulers: a
/// `<thead>` row of column letters, and a leading `<td>` per body row holding
/// the row number. Importing those as cells shifts every value down one row
/// and right one column — `a1` lands at B2, and column A fills with 1…30.
///
/// This is the positional statement the census cannot make: `table_cells`
/// counts the same 510 cells whether or not they are in the right places.
#[test]
fn corpus_spreadsheet_grid_chrome_is_not_imported_as_data() {
    let html = fixture("QGYAAAjicgG");
    let quip = from_quip_html_as(&html, QuipThreadKind::Spreadsheet);
    let grid = first_table_grid(&quip.doc);

    // 30 rows × 16 columns of data. The source table is 31 × 17; the extra
    // row is the column-letter `<thead>` and the extra column is the gutter.
    assert_eq!(grid.len(), 30, "body rows only — the column-letter row is chrome");
    let widths: Vec<usize> = grid.iter().map(Vec::len).collect();
    assert_eq!(widths, vec![16; 30], "data columns only — the row-number gutter is chrome");

    // Every `<th>` in this document was a column letter, so once the header
    // row is gone there is no header cell left.
    assert!(
        grid.iter().flatten().all(|(header, _)| !header),
        "a stripped sheet keeps no header cell",
    );

    // The top-left data cell. The fixture is content-scrubbed — every word
    // run became filler of the identical length — so `a1 b1 c1 d1` reads as
    // four 2-character strings. Their *values* are meaningless; that they are
    // the first four cells of the first row, rather than cells 2-5 of row 2,
    // is the entire assertion.
    assert_eq!(
        grid[0][..4].iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
        vec!["as", "as", "no", "it"],
        "row 1, columns A-D — Quip's a1..d1, unshifted",
    );

    // The shift was diagonal, so pin both axes. Column A must be the sheet's
    // own first column, not the gutter: under the bug every one of its 30
    // cells was a row number.
    let col_a = column(&grid, 0);
    assert!(
        !col_a.iter().all(|t| t.bytes().all(|b| b.is_ascii_digit())),
        "column A is data, not the row-number ruler: {col_a:?}",
    );

    // The other axis, pinned along the whole of column D — the one column of
    // this sheet with content spread down it. Quip has a value at D1 and D6
    // and a run of four numbers at D8-D11 (the last two computed by the
    // formulas of #192); every other cell of the column is one of the 469
    // U+200B-only spacers. Under the bug all of it read one row lower, in
    // column E. `occupied` is the shape, not the values: the fixture's digits
    // are scrubbed, so `1, 2, 2, 5` is not recoverable — where they sit is.
    let col_d = column(&grid, 3);
    let occupied: Vec<usize> = col_d
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.trim_matches('\u{200b}').is_empty())
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(occupied, vec![1, 6, 8, 9, 10, 11], "column D's filled rows, 1-based: {col_d:?}");
    assert!(
        col_d[7..11].iter().all(|t| t.bytes().all(|b| b.is_ascii_digit())),
        "D8-D11 are the numeric run: {:?}",
        &col_d[7..11],
    );

    // The cost of the strip, stated rather than left silent: the 16
    // column-letter `<th>` elements each carried an `id`, and dropping the
    // cells drops those anchors. 528 - 16 = 512. A Phase-2b link pointing at
    // a column letter no longer resolves; a link pointing at any of the 480
    // data cells still does, and those are all 480 of them.
    assert_eq!(quip.sections.len(), 512, "every anchor except the 16 column letters");

    assert_eq!(quip.formulas_dropped, 2, "the strip does not disturb the #192 count");
    assert_eq!(quip.deep_nesting_truncated, 0);
}

/// **#230, the negative control.** Byte-for-byte the same markup, imported as
/// an ordinary document: the header row and the leading column survive as
/// content.
///
/// This is the discriminator under test, isolated — same input, one bit
/// different, opposite outcome. It matters because Quip wraps *prose* tables
/// in exactly this chrome too: 17 of the 47 tables across the staged
/// 56-document corpus carry a `<thead>` whose first cell is the empty 2em
/// corner and a `#f0f0f0` numeric gutter on every body row, and 16 of those
/// 17 are ordinary document tables whose `<th>` cells hold real headings.
/// Any fix keyed on that markup instead of on the thread type would delete
/// those headings. This test fails the moment such a fix is attempted.
#[test]
fn the_same_grid_markup_imported_as_a_document_keeps_its_header_row() {
    let html = fixture("QGYAAAjicgG");
    let quip = from_quip_html_as(&html, QuipThreadKind::Document);
    let grid = first_table_grid(&quip.doc);

    assert_eq!(grid.len(), 31, "the header row is content here");
    assert_eq!(grid.iter().map(Vec::len).collect::<Vec<_>>(), vec![17; 31], "17 cells per row");
    assert_eq!(
        grid[0].iter().filter(|(header, _)| *header).count(),
        17,
        "the whole first row is <th>",
    );
    assert!(
        column(&grid, 0)[1..].iter().all(|t| t.bytes().all(|b| b.is_ascii_digit())),
        "the leading column survives as content",
    );

    // `QuipThreadKind::Document` is what plain `from_quip_html` means, so the
    // counts the rest of this file pins are the counts asserted here.
    assert_eq!(census(&quip.doc).table_cells, 510);
    assert_eq!(census(&quip.doc).table_headers, 17);
    assert_eq!(quip.sections.len(), 528, "no anchor is lost on the document path");
}

// ─── cross-fixture invariants ────────────────────────────────────

/// Whatever else changes, the corpus must keep parsing, must not truncate,
/// and must keep recording at least the anchors it records today.
#[test]
fn every_corpus_fixture_parses_without_truncation() {
    for thread_id in CORPUS {
        let (_html, quip, _c) = import(thread_id);
        assert_eq!(
            quip.deep_nesting_truncated, 0,
            "{thread_id}: nothing in the real corpus reaches MAX_NESTING_DEPTH"
        );
        assert!(!quip.sections.is_empty(), "{thread_id}: every document has at least one anchor");
    }
}

/// The known-coverage-gap paragraph at the top of this file states that no
/// fixture here carries a live-app payload (#191). That is a claim about the
/// corpus, so it belongs in the corpus net rather than only in prose — and
/// it is the *only* statement about #191 these five documents can make.
///
/// It earns its place twice over. It is the false-positive guard for
/// `import_quip::LIVE_APP_ATTR_PREFIX`: five real documents, 166 KB of
/// third-party markup, and the detector must fire on none of it. And it is
/// the tripwire for the gap itself — the day a fixture with a real board is
/// added, this assertion fails and whoever adds it is told, at exactly the
/// right moment, that #191 finally has ground truth to be pinned against.
#[test]
fn no_corpus_fixture_carries_a_live_app_and_the_detector_agrees() {
    for thread_id in CORPUS {
        let (html, quip, _c) = import(thread_id);
        assert!(
            !html.contains(ogrenotes_collab::import_quip::LIVE_APP_ATTR_PREFIX),
            "{thread_id}: a fixture with a live app has appeared — #191 now has a real \
             payload to be designed against, and this assertion is the wrong shape for it"
        );
        assert_eq!(quip.live_apps_dropped, 0, "{thread_id}: nothing here is a live app");
    }
}

/// Formulas are the one #192 signal these fixtures carry, and only one
/// document carries any. Stated across the whole corpus so a detector that
/// started counting `<td>`s, or `value=`, or every `<span>`, fails here
/// rather than inflating a user's report by two orders of magnitude.
#[test]
fn only_the_spreadsheet_fixture_reports_a_dropped_formula() {
    for thread_id in CORPUS {
        let (html, quip, _c) = import(thread_id);
        assert_eq!(
            quip.formulas_dropped,
            html.matches("formula='").count(),
            "{thread_id}: every source formula is counted exactly once"
        );
    }
}

/// Every captured section id must be one the source actually contained, and
/// every minted blockId must have the shape the frontend expects — a guard
/// against a future change inventing anchors.
#[test]
fn captured_section_ids_all_occur_in_the_source() {
    for thread_id in CORPUS {
        let (html, quip, _c) = import(thread_id);
        for (section_id, block_id) in &quip.sections {
            assert!(
                html.contains(section_id.as_str()),
                "{thread_id}: captured section id {section_id} is not in the source"
            );
            assert_eq!(block_id.len(), 10, "{thread_id}: blockId shape");
        }
    }
}

/// #190 stated as an exhaustive fact rather than five totals: after the fix,
/// **every id a Quip anchor could name is a key in the section map**, bar
/// thirteen in the whole corpus, both groups named and understood.
///
/// The totals asserted per fixture say *how many* anchors are recorded; they
/// cannot say *which*, so a change that started dropping ten `<td>` ids while
/// capturing ten others would keep them green. This one asks the question the
/// back-patch will actually ask — "is this source id resolvable?" — for all
/// 1481 of them, and reports the misses by tag.
///
/// Note what that makes of the 643 `<span>` ids: **none of them is a miss.**
/// Quip repeats the enclosing `<li>`/`<td>` id verbatim on the inner
/// `<span>`, so a span's id is already a key — it maps to the item's block,
/// which is where the anchor should land. They need no entry of their own,
/// only the parent's. `spans_repeat_their_parents_id` below keeps that
/// premise honest, and
/// `a_cell_span_repeats_its_parents_anchor_so_needs_no_entry_of_its_own` in
/// `import_quip.rs` pins the behaviour.
///
/// The thirteen genuine misses:
///
/// * **`ul` — 10, `CVLAAAgSl7Q` only.** The `'6'` continuation sections #188
///   (PR #200) merges away: emptied into the accumulating list and dropped
///   before the walker runs, so the section-level anchor has no block left.
/// * **`control` — 3, `SSfAAALs7fy` only.** An inline entity wrapper, not a
///   section; two become `Mention` leaves, one is empty.
///
/// Both are explained in full at their fixture above.
#[test]
fn every_uncaptured_source_id_is_one_of_the_two_known_residues() {
    let expected: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::from([
        ("AeOAAAcV1hg", BTreeMap::new()),
        ("CVLAAAgSl7Q", BTreeMap::from([("ul", 10)])),
        ("ZaNAAAU4ELc", BTreeMap::new()),
        ("SSfAAALs7fy", BTreeMap::from([("control", 3)])),
        ("QGYAAAjicgG", BTreeMap::new()),
    ]);

    for thread_id in CORPUS {
        let (html, quip, _c) = import(thread_id);
        let captured: BTreeSet<&str> = quip.sections.iter().map(|(s, _)| s.as_str()).collect();
        let mut missing: BTreeMap<&str, usize> = BTreeMap::new();
        for (tag, id) in source_ids_by_tag(&html) {
            if !captured.contains(id.as_str()) {
                // Leaked as a `&'static str` only to keep the expected map
                // above readable; the test process is the only owner.
                let tag: &'static str = Box::leak(tag.into_boxed_str());
                *missing.entry(tag).or_default() += 1;
            }
        }
        assert_eq!(
            missing, expected[*thread_id],
            "{thread_id}: uncaptured ids by tag changed — a new tag here is an anchor \
             that stopped being addressable"
        );
    }
}

/// The premise the `<span>` residue rests on: a `<span>` bearing an id
/// always repeats its enclosing `<li>`/`<td>`'s id byte for byte. If Quip
/// ever gives a span a distinct id, the residue above stops being harmless
/// and spans need capturing in their own right.
///
/// Asserted on the raw bytes rather than through the parser, because it is a
/// claim about Quip's markup, not about the walker.
#[test]
fn spans_repeat_their_parents_id() {
    let mut checked = 0usize;
    for thread_id in CORPUS {
        let html = fixture(thread_id);
        // `<li id='X' …><span id='X'>` and `<td id='X' …><span id='X'>` are
        // the only two shapes; walk each id-bearing span back to the
        // nearest preceding `<li`/`<td`/`<th` open tag.
        for (idx, _) in html.match_indices("<span id='") {
            let rest = &html[idx + "<span id='".len()..];
            let span_id = &rest[..rest.find('\'').expect("closing quote")];
            let before = &html[..idx];
            let owner = ["<li id='", "<td id='", "<th id='"]
                .iter()
                .filter_map(|open| before.rfind(open).map(|at| (at, *open)))
                .max_by_key(|(at, _)| *at);
            let Some((at, open)) = owner else { continue };
            let tail = &html[at + open.len()..];
            let owner_id = &tail[..tail.find('\'').expect("closing quote")];
            assert_eq!(
                span_id, owner_id,
                "{thread_id}: a <span> id that does not repeat its cell's id"
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 643, "every id-bearing <span> in the corpus was checked");
}

/// Quip's nesting spelling is a `<ul>` that is a **sibling** of the `<li>`.
/// The hand-authored fixtures use the standard `<ul>`-inside-`<li>` spelling,
/// which does not occur once in 56 real documents. This pins the real shape
/// so #187's fix (PR #200) had something to turn green — and so nobody
/// "fixes" these fixtures into the shape the parser already handled.
#[test]
fn the_corpus_nests_lists_as_a_sibling_ul_never_inside_the_li() {
    for thread_id in ["AeOAAAcV1hg", "CVLAAAgSl7Q"] {
        let html = fixture(thread_id);
        assert!(
            html.contains("</li><ul>"),
            "{thread_id}: expected Quip's sibling-<ul> nesting spelling"
        );
        assert!(
            html.contains("class='parent'"),
            "{thread_id}: the <li> that owns a nested list is marked class='parent'"
        );
        assert!(
            !html.contains("</ul></li>"),
            "{thread_id}: Quip never closes a nested list inside its <li>"
        );
    }
}

/// Quip terminates every `<li>`, `<td>` and `<th>` with a `<br/>` before the
/// closing tag. That is a line terminator, not authored content — #189, which
/// dropped it. A `<br>` in the middle of a cell IS authored content and must
/// survive, so the fix discriminates by position, not by presence.
///
/// **What this test can and cannot show.** Every single `<br>` outside a
/// `<pre>` in all five fixtures is a terminator — 659 of them, zero authored
/// mid-content breaks anywhere in this corpus. So these five prove the
/// terminators go, and prove nothing about the mid-content breaks surviving.
/// That half of the contract is pinned on verbatim markup by
/// `a_mid_item_break_survives_while_the_terminator_goes` and its neighbours
/// in `import_quip.rs`; if this file ever gains a fixture with an authored
/// break, assert it here too.
#[test]
fn every_list_item_and_cell_terminator_is_dropped() {
    for thread_id in ["AeOAAAcV1hg", "SSfAAALs7fy", "QGYAAAjicgG"] {
        let html = fixture(thread_id);
        assert!(
            html.contains("<br/></li>") || html.contains("<br/></td>"),
            "{thread_id}: the source must still carry the terminators being tested"
        );
        let (_h, _q, c) = import(thread_id);
        assert_eq!(c.hard_breaks, 0, "{thread_id}: every <br> here is a terminator and goes");
    }
}

/// The corpus half of the discrimination rule, stated as a source fact so it
/// cannot silently stop being true: no `<br>` in these five documents sits
/// anywhere but immediately before `</li>`, `</td>`, `</th>` — or inside a
/// `<pre>`, where #184's opposite rule owns it. Should a future fixture break
/// this, the assertion above stops being a valid statement of the fix and the
/// new document's authored breaks need their own assertion.
#[test]
fn no_corpus_break_outside_a_pre_is_anything_but_a_terminator() {
    for thread_id in CORPUS {
        let html = fixture(thread_id);
        // Strip comments (the fixture headers themselves discuss `<br/>`)
        // and `<pre>` bodies, then every remaining `<br>` must terminate a
        // cell.
        let mut rest = String::new();
        let mut tail = html.as_str();
        while let Some(open) = tail.find("<!--") {
            rest.push_str(&tail[..open]);
            tail = &tail[open + 4..];
            tail = tail.split_once("-->").map(|(_, t)| t).unwrap_or("");
        }
        rest.push_str(tail);
        let outside_pre: String = rest
            .split("<pre")
            .enumerate()
            .map(|(i, chunk)| {
                if i == 0 { chunk } else { chunk.split_once("</pre>").map_or("", |(_, t)| t) }
            })
            .collect();

        let total = outside_pre.matches("<br").count();
        let terminators = outside_pre.matches("<br/></li>").count()
            + outside_pre.matches("<br/></td>").count()
            + outside_pre.matches("<br/></th>").count();
        assert_eq!(
            total, terminators,
            "{thread_id}: {} <br> outside <pre>, {terminators} of them terminators",
            total
        );
    }
}

/// Quip indents code with no-break spaces, not ASCII spaces, and separates
/// code lines with `<br>` rather than a newline. A fixture that normalises
/// either one silently stops testing the real thing (#184).
#[test]
fn code_blocks_keep_nbsp_indentation_and_br_separators() {
    let html = fixture("CVLAAAgSl7Q");
    assert!(html.contains('\u{a0}'), "the source indents code with U+00A0");
    assert!(html.contains("<br>"), "the source separates code lines with <br>");
    assert!(!html.contains("<pre><code"), "Quip emits a bare <pre>, never <pre><code>");
    assert!(html.contains("class='prettyprint'"), "Quip's code-block class");

    let quip = from_quip_html(&html);
    let c = census(&quip.doc);
    assert!(
        c.code_lines > c.code_blocks,
        "code blocks must be multi-line: {} lines across {} blocks",
        c.code_lines,
        c.code_blocks
    );
}

/// U+200B is how Quip spells an empty paragraph, an empty list item and an
/// empty table cell — 1105 text nodes across 40 documents. The walker's
/// "is this paragraph empty?" logic turns on it, and no hand-authored fixture
/// contains one.
#[test]
fn zero_width_spacers_survive_as_paragraphs() {
    let html = fixture("QGYAAAjicgG");
    assert!(html.contains('\u{200b}'), "the source uses U+200B for empty cells");
    let (_h, _q, c) = import("QGYAAAjicgG");
    // 510 cells + 17 headers = 527 paragraphs: no cell collapsed away, even
    // though 469 of the source spans hold nothing but a zero-width space.
    assert_eq!(c.paragraphs, c.table_cells + c.table_headers, "no empty cell was dropped");
}
