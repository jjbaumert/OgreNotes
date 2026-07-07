// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// #142 Phase 2 — mail-merge / template variables.
//
// Placeholder syntax:
//
//   [[key]]                → look up `values.key`
//   [[a.b.c]]              → nested lookup `values.a.b.c` (dot-nested)
//   [[key|Fallback text]]  → use "Fallback text" if `key` is absent
//                            (OgreNotes extension — a useful UX affordance
//                            so a template
//                            with a missing value doesn't render an
//                            obviously-broken raw token)
//
// Allowed key characters: `A-Z a-z 0-9 . _`. The fallback text is
// everything between the pipe and the closing `]]` — arbitrary text
// including spaces and unicode, no interior `]]` allowed.
//
// Value coercion: strings pass through; numbers are stringified; anything
// else (arrays / booleans / null / objects when the key wasn't a leaf)
// falls back to the fallback if provided, otherwise leaves the raw token
// verbatim so the caller can see what wasn't filled in.
//
// The module has two layers:
//
// 1. `scan_str` / `substitute_str` — pure string operations. Zero yrs
//    coupling; heavily tested. This is where all the tricky parsing +
//    lookup logic lives.
// 2. `scan_ydoc` / `substitute_ydoc` — walk a yrs Doc's `content`
//    XmlFragment and apply the string layer to every XmlText. The
//    scanner returns the unique key set (used by `list_templates` so
//    the frontend can decide whether to prompt for values); the
//    substitutor is the copy-time transform.

use std::collections::BTreeSet;

use serde_json::Value;
use yrs::{Doc, GetString, ReadTxn, Text, Transact, WriteTxn, XmlFragment, XmlOut, XmlTextRef};

// ─── String layer ──────────────────────────────────────────────

/// One placeholder occurrence in a string. `default` is Some when the
/// syntax used `[[key|Fallback]]` — including when the fallback is
/// empty (`[[key|]]` yields Some("")), so the caller can distinguish
/// "no default given" from "default is intentionally empty."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub key: String,
    pub default: Option<String>,
}

/// Scan `s` for placeholders in appearance order. Duplicates are kept;
/// callers wanting a unique key set should collect through a `BTreeSet`.
pub fn scan_str(s: &str) -> Vec<Placeholder> {
    let mut out = Vec::new();
    for span in placeholder_spans(s) {
        out.push(span.placeholder);
    }
    out
}

/// Substitute every `[[key]]` / `[[key|Fallback]]` in `s` by looking up
/// `key` in `values`. See module doc for the resolution rules.
pub fn substitute_str(s: &str, values: &Value) -> String {
    // Substitute over `spans` right-to-left so an earlier substitution's
    // length doesn't shift the byte offsets of later ones. `Vec<Range>`
    // is fine at doc scale (placeholder count is small).
    let spans = placeholder_spans(s);
    if spans.is_empty() {
        return s.to_string();
    }
    let mut out = s.to_string();
    for span in spans.into_iter().rev() {
        let replacement = resolve(&span.placeholder, values);
        out.replace_range(span.byte_range.clone(), &replacement);
    }
    out
}

/// True iff `s` contains at least one placeholder. Cheaper than
/// `!scan_str(s).is_empty()` because it can short-circuit on the first hit.
pub fn contains_placeholder(s: &str) -> bool {
    placeholder_spans(s).into_iter().next().is_some()
}

struct Span {
    placeholder: Placeholder,
    byte_range: std::ops::Range<usize>,
}

/// Scanner. Walks the string looking for `[[…]]`. Rejects anything that
/// isn't a well-formed placeholder (unknown chars in the key, no
/// closing `]]`) so a doc with square brackets in other contexts (Rust
/// code samples, footnote markers) doesn't get mis-substituted.
fn placeholder_spans(s: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 2 <= bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(span) = try_parse_placeholder(s, i) {
                let end = span.byte_range.end;
                spans.push(span);
                i = end;
                continue;
            }
        }
        // Advance by one byte. UTF-8 continuation bytes are >= 0x80 so
        // we may land inside a codepoint here — that's fine, we're
        // looking for `[[` which is ASCII and can't overlap a
        // multi-byte sequence's continuation.
        i += 1;
    }
    spans
}

fn try_parse_placeholder(s: &str, start: usize) -> Option<Span> {
    debug_assert!(s.as_bytes().get(start..start + 2) == Some(b"[["));
    let after_open = start + 2;
    let rest = &s[after_open..];

    // Key: [A-Za-z0-9._]+. `bytes` is safe here because those chars are
    // all single-byte ASCII, so byte-length = char-length.
    let key_bytes: usize = rest
        .as_bytes()
        .iter()
        .take_while(|&&b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        .count();
    if key_bytes == 0 {
        return None;
    }
    let key = &rest[..key_bytes];
    let after_key = &rest[key_bytes..];

    // Terminator: `]]` (no fallback) or `|<fallback>]]`.
    let (default, closer_offset) = if let Some(after_pipe) = after_key.strip_prefix('|') {
        // Fallback text: everything up to the first `]]`.
        let close = after_pipe.find("]]")?;
        (Some(after_pipe[..close].to_string()), 1 + close + 2)
    } else if after_key.starts_with("]]") {
        (None, 2)
    } else {
        return None;
    };
    let end = after_open + key_bytes + closer_offset;
    Some(Span {
        placeholder: Placeholder {
            key: key.to_string(),
            default,
        },
        byte_range: start..end,
    })
}

/// Look up `p.key` in `values` using dot-nested indexing. Returns the
/// resolved replacement string, falling back through: value → default →
/// verbatim raw token (`[[key]]` / `[[key|Fallback]]`).
fn resolve(p: &Placeholder, values: &Value) -> String {
    if let Some(v) = lookup(values, &p.key) {
        match v {
            Value::String(s) => return s.clone(),
            Value::Number(n) => return n.to_string(),
            // Bool / Null / Array / Object at a leaf key aren't
            // meaningful mail-merge payloads. Fall through to the
            // fallback so the doc doesn't render `true` or `null` at
            // the user.
            _ => {}
        }
    }
    if let Some(default) = &p.default {
        return default.clone();
    }
    // No value, no default — leave the raw token so the reader can see
    // what wasn't filled in.
    let mut raw = String::from("[[");
    raw.push_str(&p.key);
    if let Some(default) = &p.default {
        raw.push('|');
        raw.push_str(default);
    }
    raw.push_str("]]");
    raw
}

/// Key lookup — flat first, then dot-nested navigation. Accepts either
/// `{"a.b": "x"}` or `{"a": {"b": "x"}}` for a `[[a.b]]` placeholder.
/// The flat form is what a simple form UI naturally produces (one field
/// per placeholder keyed by the placeholder string); the nested form
/// suits values that are already structured as an object tree. Both work.
fn lookup<'a>(values: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(obj) = values.as_object() {
        if let Some(v) = obj.get(key) {
            return Some(v);
        }
    }
    let mut cur = values;
    for segment in key.split('.') {
        cur = cur.as_object()?.get(segment)?;
    }
    Some(cur)
}

// ─── Y.Doc layer ───────────────────────────────────────────────

/// Walk `doc`'s `content` XmlFragment and return the unique set of
/// placeholder keys used anywhere in the doc's text. Sorted for stable
/// output — the `list_templates` handler surfaces this to the client so
/// the picker can decide whether to prompt for values.
///
/// Missing fragment → empty set (a doc without content isn't an error).
pub fn scan_ydoc(doc: &Doc) -> Vec<String> {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return Vec::new();
    };
    let mut keys: BTreeSet<String> = BTreeSet::new();
    scan_fragment(&txn, &fragment, &mut keys);
    keys.into_iter().collect()
}

fn scan_fragment<T: ReadTxn>(
    txn: &T,
    fragment: &yrs::XmlFragmentRef,
    keys: &mut BTreeSet<String>,
) {
    for i in 0..fragment.len(txn) {
        let Some(child) = fragment.get(txn, i) else {
            continue;
        };
        match child {
            XmlOut::Element(el) => {
                for j in 0..el.len(txn) {
                    if let Some(grand) = el.get(txn, j) {
                        scan_node(txn, &grand, keys);
                    }
                }
            }
            XmlOut::Text(text) => {
                scan_text(txn, &text, keys);
            }
            _ => {}
        }
    }
}

fn scan_node<T: ReadTxn>(txn: &T, node: &XmlOut, keys: &mut BTreeSet<String>) {
    match node {
        XmlOut::Element(el) => {
            for i in 0..el.len(txn) {
                if let Some(child) = el.get(txn, i) {
                    scan_node(txn, &child, keys);
                }
            }
        }
        XmlOut::Text(text) => {
            scan_text(txn, text, keys);
        }
        _ => {}
    }
}

fn scan_text<T: ReadTxn>(txn: &T, text: &XmlTextRef, keys: &mut BTreeSet<String>) {
    let s = text.get_string(txn);
    for p in scan_str(&s) {
        keys.insert(p.key);
    }
}

/// Apply `values` to every text node in `doc`. In-place mutation via a
/// write transaction. See module doc for the substitution rules.
///
/// v1 approach: for each XmlText that contains at least one
/// placeholder, replace the entire text with its substituted version.
/// Marks *inside* that text are lost — an acceptable Phase 2
/// simplification because the typical template puts placeholders in
/// plain-text prose. A precision pass (delete_range + insert per
/// placeholder, preserving surrounding marks) is a v2 refinement worth
/// its own commit.
pub fn substitute_ydoc(doc: &mut Doc, values: &Value) {
    // Collect the text refs first, under a read transaction, so we can
    // release the read borrow before opening the write transaction
    // yrs requires exclusive access for.
    let text_refs: Vec<XmlTextRef> = {
        let txn = doc.transact();
        let Some(fragment) = txn.get_xml_fragment("content") else {
            return;
        };
        let mut refs = Vec::new();
        collect_text_refs(&txn, &fragment, &mut refs);
        refs
    };

    if text_refs.is_empty() {
        return;
    }

    let mut txn = doc.transact_mut();
    for text in &text_refs {
        let original = text.get_string(&txn);
        if !contains_placeholder(&original) {
            continue;
        }
        let substituted = substitute_str(&original, values);
        if substituted == original {
            continue;
        }
        // yrs text lengths are in UTF-16 code units — using the char
        // count would be wrong for any BMP-adjacent codepoint. The
        // yrs API takes `u32` and matches yjs's UTF-16 offsets.
        let len_utf16: u32 = original.encode_utf16().count() as u32;
        text.remove_range(&mut txn, 0, len_utf16);
        text.insert(&mut txn, 0, &substituted);
    }
}

fn collect_text_refs<T: ReadTxn>(
    txn: &T,
    fragment: &yrs::XmlFragmentRef,
    out: &mut Vec<XmlTextRef>,
) {
    for i in 0..fragment.len(txn) {
        let Some(child) = fragment.get(txn, i) else {
            continue;
        };
        match child {
            XmlOut::Element(el) => {
                for j in 0..el.len(txn) {
                    if let Some(grand) = el.get(txn, j) {
                        collect_text_from_node(txn, &grand, out);
                    }
                }
            }
            XmlOut::Text(text) => out.push(text),
            _ => {}
        }
    }
}

fn collect_text_from_node<T: ReadTxn>(txn: &T, node: &XmlOut, out: &mut Vec<XmlTextRef>) {
    match node {
        XmlOut::Element(el) => {
            for i in 0..el.len(txn) {
                if let Some(child) = el.get(txn, i) {
                    collect_text_from_node(txn, &child, out);
                }
            }
        }
        XmlOut::Text(text) => out.push(text.clone()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ─── String layer ────────────────────────────────────────

    #[test]
    fn scan_finds_single_placeholder() {
        let phs = scan_str("Hello, [[name]]!");
        assert_eq!(phs.len(), 1);
        assert_eq!(phs[0].key, "name");
        assert_eq!(phs[0].default, None);
    }

    #[test]
    fn scan_supports_dot_nested_keys() {
        let phs = scan_str("[[user.name]] on [[user.team.slug]]");
        assert_eq!(phs.len(), 2);
        assert_eq!(phs[0].key, "user.name");
        assert_eq!(phs[1].key, "user.team.slug");
    }

    #[test]
    fn scan_supports_fallback_syntax() {
        let phs = scan_str("[[title|Untitled Doc]] on [[dt|]]");
        assert_eq!(phs[0].key, "title");
        assert_eq!(phs[0].default, Some("Untitled Doc".to_string()));
        // Empty fallback is meaningful — distinguishes `[[key|]]` (blank
        // if missing) from `[[key]]` (verbatim token if missing).
        assert_eq!(phs[1].key, "dt");
        assert_eq!(phs[1].default, Some(String::new()));
    }

    #[test]
    fn scan_ignores_malformed_brackets() {
        // A `[[` with no matching `]]`, or contents with disallowed
        // characters, must not be treated as a placeholder — otherwise
        // Rust code samples like `[[cfg(test)]]` would be mangled.
        assert!(scan_str("[[no close").is_empty());
        assert!(scan_str("[[has spaces]]").is_empty());
        assert!(scan_str("[[cfg(test)]]").is_empty());
        assert!(scan_str("[[]] and [[ ]]").is_empty());
    }

    #[test]
    fn substitute_replaces_present_values() {
        let out = substitute_str(
            "Hi [[name]], welcome to [[team.name]].",
            &json!({"name": "Arnie", "team": {"name": "Vector"}}),
        );
        assert_eq!(out, "Hi Arnie, welcome to Vector.");
    }

    #[test]
    fn substitute_accepts_flat_dotted_keys() {
        // A form UI that emits one field per placeholder keyed by the
        // placeholder string produces `{"team.name": "Vector"}`, not the
        // nested form. Both must resolve — the flat lookup wins first.
        let out = substitute_str(
            "Welcome to [[team.name]].",
            &json!({"team.name": "Vector"}),
        );
        assert_eq!(out, "Welcome to Vector.");
    }

    #[test]
    fn substitute_leaves_missing_keys_verbatim() {
        // No value AND no fallback → the raw token stays so the reader
        // knows what wasn't filled in.
        let out = substitute_str("Hi [[name]] from [[team]]", &json!({"name": "Arnie"}));
        assert_eq!(out, "Hi Arnie from [[team]]");
    }

    #[test]
    fn substitute_uses_fallback_when_key_missing() {
        let out = substitute_str(
            "Owner: [[owner|Unassigned]]. Priority: [[p|P3]].",
            &json!({}),
        );
        assert_eq!(out, "Owner: Unassigned. Priority: P3.");
    }

    #[test]
    fn substitute_prefers_value_over_fallback() {
        let out = substitute_str(
            "Owner: [[owner|Unassigned]]",
            &json!({"owner": "Arnie"}),
        );
        assert_eq!(out, "Owner: Arnie");
    }

    #[test]
    fn substitute_coerces_numeric_values_to_string() {
        let out = substitute_str("Age: [[age]]", &json!({"age": 34}));
        assert_eq!(out, "Age: 34");
    }

    #[test]
    fn substitute_falls_through_non_scalar_values() {
        // A key that resolves to an array/bool/object at the leaf isn't
        // a meaningful mail-merge payload — fall back to the default (or
        // to the raw token). Prevents `true` or `null` from leaking
        // into rendered docs.
        let out = substitute_str(
            "[[tags|<none>]] / [[flag|off]]",
            &json!({"tags": ["a", "b"], "flag": false}),
        );
        assert_eq!(out, "<none> / off");
    }

    #[test]
    fn substitute_is_noop_without_placeholders() {
        let out = substitute_str("Plain text with [brackets] but no tokens.", &json!({}));
        assert_eq!(out, "Plain text with [brackets] but no tokens.");
    }

    #[test]
    fn substitute_handles_multibyte_surrounding_text() {
        // Placeholder byte offsets must stay valid when the surrounding
        // text carries multi-byte codepoints. `replace_range` panics
        // on a non-char-boundary — this test guards against that
        // regression if the span logic ever slips into char-count
        // territory.
        let out = substitute_str("Héllo [[name]] 👋", &json!({"name": "Arnie"}));
        assert_eq!(out, "Héllo Arnie 👋");
    }

    #[test]
    fn contains_placeholder_short_circuits() {
        assert!(contains_placeholder("Hi [[name]]"));
        assert!(!contains_placeholder("Plain text"));
        assert!(!contains_placeholder("[[malformed"));
    }

    // ─── Y.Doc layer ─────────────────────────────────────────

    fn build_doc(paragraphs: &[&str]) -> Doc {
        use yrs::types::xml::{XmlElementPrelim, XmlTextPrelim};
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            for (i, text) in paragraphs.iter().enumerate() {
                let p = fragment.insert(&mut txn, i as u32, XmlElementPrelim::empty("paragraph"));
                p.insert(&mut txn, 0, XmlTextPrelim::new(*text));
            }
        }
        doc
    }

    #[test]
    fn scan_ydoc_returns_unique_sorted_keys() {
        let doc = build_doc(&[
            "Hi [[name]] and [[user.email]]",
            "Welcome [[name]] again",
        ]);
        let keys = scan_ydoc(&doc);
        assert_eq!(keys, vec!["name".to_string(), "user.email".to_string()]);
    }

    #[test]
    fn scan_ydoc_empty_when_no_placeholders() {
        let doc = build_doc(&["Just plain text.", "No tokens here."]);
        assert!(scan_ydoc(&doc).is_empty());
    }

    #[test]
    fn substitute_ydoc_rewrites_matching_texts() {
        let mut doc = build_doc(&[
            "Hi [[name]]",
            "Team: [[team|N/A]]",
            "No placeholders here.",
        ]);
        substitute_ydoc(&mut doc, &json!({"name": "Arnie"}));

        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let mut out = Vec::new();
        for i in 0..fragment.len(&txn) {
            if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
                out.push(el.get_string(&txn));
            }
        }
        // `get_string` on an XmlElement includes the wrapping tag; the
        // substituted content is inside.
        assert_eq!(
            out,
            vec![
                "<paragraph>Hi Arnie</paragraph>".to_string(),
                "<paragraph>Team: N/A</paragraph>".to_string(),
                "<paragraph>No placeholders here.</paragraph>".to_string(),
            ]
        );
    }

    #[test]
    fn substitute_ydoc_is_noop_when_no_placeholders() {
        // A no-op substitute must not churn the CRDT — a template with
        // no placeholders should copy as pure byte-passthrough.
        let mut doc = build_doc(&["Just plain text."]);
        let before = doc.transact().encode_state_as_update_v1(&yrs::StateVector::default());
        substitute_ydoc(&mut doc, &json!({}));
        let after = doc.transact().encode_state_as_update_v1(&yrs::StateVector::default());
        assert_eq!(before, after, "no-op substitute must not mutate the doc");
    }
}
