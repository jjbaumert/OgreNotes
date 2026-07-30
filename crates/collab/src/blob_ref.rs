// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Durable blob references for `Image.src`.
//!
//! Two layers live here: [`blob_ref`]/[`parse_blob_ref`], the stable
//! `Image.src` form that outlives a presigned S3 URL's TTL, and the
//! doc-level [`collect_blob_refs`]/[`rewrite_blob_refs`] pair the export
//! route uses to resolve those references back into real URLs before
//! handing a document to a format exporter.
//!
//! Blob addressing is general — it is not tied to any one import path.
//! The frontend mirrors the first pair in `frontend/src/editor/blob_ref.rs`
//! (it cannot depend on this crate from its lib target); the two are pinned
//! to the same output by tests on both sides.
//!
//! ## Why a stable `Image.src` form
//!
//! `crates/api/src/routes/documents.rs` mints presigned S3 GET URLs with a
//! 4-hour TTL. Storing that URL verbatim in the CRDT (as the editor's
//! upload path used to) means every inserted image 403s four hours later —
//! true for hand-uploaded images today, and would also be true for every
//! image a Quip import carries over. [`blob_ref`] instead stores an opaque,
//! stable reference; the frontend resolves it to a fresh presigned URL at
//! render time (`frontend/src/editor/view.rs`), re-fetching only when the
//! resolved URL isn't already cached for the page's lifetime.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use yrs::types::xml::{Xml, XmlFragment, XmlOut};
use yrs::{Doc, ReadTxn, Transact, XmlElementRef};

use crate::schema::NodeType;

/// Prefix marking an `Image.src` value as a durable blob reference owned by
/// this workspace, as opposed to a legacy or external absolute URL (which
/// must be used verbatim — see [`parse_blob_ref`]).
pub const BLOB_REF_PREFIX: &str = "ogre-blob:";

/// Build the `Image.src` value for a blob owned by this workspace:
///
/// ```text
/// ogre-blob:<blob_id>/<url-encoded key>
/// ```
///
/// The key is percent-encoded so the whole reference is one unambiguous
/// token (S3 keys may contain `/`, spaces, and other characters that would
/// otherwise make the `<blob_id>/<key>` split ambiguous).
pub fn blob_ref(blob_id: &str, key: &str) -> String {
    let encoded_key: Cow<str> =
        percent_encode(key);
    format!("{BLOB_REF_PREFIX}{blob_id}/{encoded_key}")
}

/// Parse an `Image.src` value produced by [`blob_ref`] back into
/// `(blob_id, key)`. Returns `None` for anything that doesn't start with
/// [`BLOB_REF_PREFIX`] — legacy presigned URLs and external absolute URLs
/// alike — so callers can fall back to using the string verbatim.
///
/// ## Why the shape checks matter
///
/// Callers that presign a key from a reference (the export path in
/// `crates/api/src/routes/documents.rs`, and `request_download_url`)
/// authorize it with a *string* prefix test: does `key` start with
/// `blobs/{doc_id}/{blob_id}/`? A `blob_id` of `..` plus a key of
/// `blobs/{my_doc}/../{victim_doc}/{victim_blob}/x.png` satisfies that
/// test literally while naming a foreign object — the guard would pass
/// and the server would presign someone else's key. Whether S3 then
/// serves it depends on how it normalizes `..` in a signed path; an
/// access-control boundary must not rest on that. So the shape is
/// constrained here, at the single point where every caller derives
/// `(blob_id, key)`:
///
/// - `blob_id` must be `[A-Za-z0-9_-]+` (the charset the upload path
///   already mints), which rules out `.`, `..`, and any separator.
/// - the decoded `key` must contain no `.` or `..` path segment, so a
///   prefix match can't be satisfied by a traversal that resolves
///   elsewhere.
///
/// Dots inside a segment (`photo.tar.gz`) are untouched — only a
/// segment that *is* `.` or `..` is rejected.
pub fn parse_blob_ref(src: &str) -> Option<(String, String)> {
    let rest = src.strip_prefix(BLOB_REF_PREFIX)?;
    let (blob_id, encoded_key) = rest.split_once('/')?;
    if blob_id.is_empty() || encoded_key.is_empty() {
        return None;
    }
    if !is_safe_blob_id(blob_id) {
        return None;
    }
    let key = percent_decode(encoded_key)?;
    if has_dot_segment(&key) {
        return None;
    }
    Some((blob_id.to_string(), key))
}

/// `[A-Za-z0-9_-]+` — see [`parse_blob_ref`]. Mirrored byte-for-byte by
/// `frontend/src/editor/blob_ref.rs`.
fn is_safe_blob_id(blob_id: &str) -> bool {
    !blob_id.is_empty()
        && blob_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// True if any `/`-delimited segment of `key` is exactly `.` or `..` —
/// see [`parse_blob_ref`]. Mirrored by the frontend copy.
fn has_dot_segment(key: &str) -> bool {
    key.split('/').any(|seg| seg == "." || seg == "..")
}

/// Every `Image` node in `doc` whose `src` parses as a durable blob
/// reference (see [`parse_blob_ref`]), as `(blob_id, key)` pairs,
/// deduplicated and in document order.
///
/// Used by the export route (`crates/api/src/routes/documents.rs`) to
/// presign fresh download URLs — server-to-S3, not `<img>`-to-server, so
/// none of the Bearer-header-auth problem that rules out a live resolve
/// route applies — before handing the doc to a format exporter. Without
/// this, `export.rs`'s `is_safe_url` gate (by design) rejects the
/// `ogre-blob:` scheme, so an exported Markdown doc silently drops the
/// image (alt text and all) and an exported HTML doc emits an `<img>`
/// with no `src` at all.
pub fn collect_blob_refs(doc: &Doc) -> Vec<(String, String)> {
    let txn = doc.transact();
    let Some(fragment) = txn.get_xml_fragment("content") else {
        return Vec::new();
    };
    let mut image_els = Vec::new();
    collect_image_elements(&txn, &fragment, &mut image_els);

    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out = Vec::new();
    for el in &image_els {
        let Some(src) = el.get_attribute(&txn, "src") else {
            continue;
        };
        let Some(pair) = parse_blob_ref(&src) else {
            continue;
        };
        if seen.insert(pair.clone()) {
            out.push(pair);
        }
    }
    out
}

/// Rewrite every `Image.src` in `doc` that's a durable blob reference
/// (see [`parse_blob_ref`]) whose *exact reference string* (as produced
/// by [`blob_ref`]) is a key in `resolved`, replacing it with the mapped
/// value in place. References not present in `resolved` — and any `src`
/// that isn't a blob reference at all (legacy/external URLs) — are left
/// untouched.
///
/// Two-phase, mirroring [`crate::mail_merge::substitute_ydoc`]: collect
/// the element handles under a read transaction, then mutate them under
/// a single write transaction, so the read and write borrows of `doc`
/// never overlap.
pub fn rewrite_blob_refs(doc: &Doc, resolved: &HashMap<String, String>) {
    if resolved.is_empty() {
        return;
    }

    let image_els: Vec<XmlElementRef> = {
        let txn = doc.transact();
        let Some(fragment) = txn.get_xml_fragment("content") else {
            return;
        };
        let mut out = Vec::new();
        collect_image_elements(&txn, &fragment, &mut out);
        out
    };
    if image_els.is_empty() {
        return;
    }

    let mut txn = doc.transact_mut();
    for el in &image_els {
        let Some(src) = el.get_attribute(&txn, "src") else {
            continue;
        };
        if let Some(new_src) = resolved.get(&src) {
            el.insert_attribute(&mut txn, "src", new_src.as_str());
        }
    }
}

/// Depth-first collection of every `Image` element anywhere in `fragment`
/// (an `Image` can be nested arbitrarily — inside a table cell, list
/// item, blockquote, etc.), shared by [`collect_blob_refs`] and
/// [`rewrite_blob_refs`].
fn collect_image_elements<T: ReadTxn>(
    txn: &T,
    fragment: &yrs::XmlFragmentRef,
    out: &mut Vec<XmlElementRef>,
) {
    for i in 0..fragment.len(txn) {
        if let Some(child) = fragment.get(txn, i) {
            collect_image_elements_from_node(txn, &child, out);
        }
    }
}

fn collect_image_elements_from_node<T: ReadTxn>(
    txn: &T,
    node: &XmlOut,
    out: &mut Vec<XmlElementRef>,
) {
    let XmlOut::Element(el) = node else {
        return;
    };
    if el.tag().as_ref() == NodeType::Image.tag_name() {
        out.push(el.clone());
    }
    for i in 0..el.len(txn) {
        if let Some(child) = el.get(txn, i) {
            collect_image_elements_from_node(txn, &child, out);
        }
    }
}

/// Minimal percent-encoding: escapes everything outside the RFC 3986
/// "unreserved" set (`A-Za-z0-9-_.~`) as `%XX` UTF-8 bytes. No external
/// crate needed for this one call site; kept byte-for-byte mirrored by the
/// frontend copy in `frontend/src/editor/blob_ref.rs` (see the parity test
/// there) since the two sides never share a dependency for this path.
fn percent_encode(input: &str) -> Cow<'_, str> {
    if input
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
    {
        return Cow::Borrowed(input);
    }
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    Cow::Owned(out)
}

/// Inverse of [`percent_encode`]. Returns `None` on malformed `%XX`
/// escapes or invalid UTF-8 after decoding, rather than panicking or
/// silently dropping bytes.
fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = input.get(i + 1..i + 3)?;
            let byte = u8::from_str_radix(hex, 16).ok()?;
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_ref_round_trips_and_ignores_absolute_urls() {
        let r = blob_ref("b1", "blobs/d1/b1/pic name.png");
        assert!(r.starts_with("ogre-blob:"), "{r}");
        let (id, key) = parse_blob_ref(&r).expect("parses");
        assert_eq!(id, "b1");
        assert_eq!(key, "blobs/d1/b1/pic name.png", "key survives encoding");
        assert!(
            parse_blob_ref("https://example.com/x.png").is_none(),
            "legacy URLs pass through"
        );
    }

    #[test]
    fn blob_ref_rejects_malformed_references() {
        // No prefix at all.
        assert!(parse_blob_ref("blobs/d1/b1/pic.png").is_none());
        // Prefix but no '/' separator between blob_id and key.
        assert!(parse_blob_ref("ogre-blob:b1").is_none());
        // Empty blob_id or empty key.
        assert!(parse_blob_ref("ogre-blob:/key").is_none());
        assert!(parse_blob_ref("ogre-blob:b1/").is_none());
    }

    /// Security pin: a reference whose shape could satisfy a caller's
    /// `blobs/{doc_id}/{blob_id}/` string-prefix authorization check
    /// while naming a *different* document's object must not parse at
    /// all. See the rationale on [`parse_blob_ref`].
    ///
    /// The frontend mirror
    /// (`frontend/src/editor/blob_ref.rs::blob_ref_rejects_traversal_shapes`)
    /// asserts the identical fixtures — relaxing one side alone fails a
    /// test on that side, which is the point of duplicating a security
    /// check across two independently-compiled crates.
    #[test]
    fn blob_ref_rejects_traversal_shapes() {
        // blob_id == ".." — the crafted prefix `blobs/{my_doc}/../`,
        // which a key naming a foreign doc then "matches".
        assert!(parse_blob_ref(
            "ogre-blob:../blobs%2Fmy_doc%2F..%2Fvictim_doc%2Fbv%2Fx.png"
        )
        .is_none());
        // blob_id == "." and other out-of-charset blob_ids.
        assert!(parse_blob_ref("ogre-blob:./k.png").is_none());
        assert!(parse_blob_ref("ogre-blob:a.b/k.png").is_none());
        assert!(parse_blob_ref("ogre-blob:a%2Fb/k.png").is_none());
        assert!(parse_blob_ref("ogre-blob:a b/k.png").is_none());
        // Well-formed blob_id, but the key traverses out of its prefix.
        assert!(parse_blob_ref("ogre-blob:b1/blobs%2Fmy_doc%2Fb1%2F..%2F..%2Fvictim%2Fx.png").is_none());
        assert!(parse_blob_ref("ogre-blob:b1/blobs%2F.%2Fx.png").is_none());
        assert!(parse_blob_ref("ogre-blob:b1/..").is_none());
        // Dots *inside* a segment are ordinary filename characters and
        // must still parse — this is the legitimate-reference guard.
        assert_eq!(
            parse_blob_ref("ogre-blob:b-1_2/blobs%2Fd1%2Fb-1_2%2Fphoto.tar.gz"),
            Some((
                "b-1_2".to_string(),
                "blobs/d1/b-1_2/photo.tar.gz".to_string()
            ))
        );
    }

    #[test]
    fn blob_ref_key_with_special_characters_round_trips() {
        let r = blob_ref("blob-42", "a b/c%d#e?f&g.png");
        let (id, key) = parse_blob_ref(&r).expect("parses");
        assert_eq!(id, "blob-42");
        assert_eq!(key, "a b/c%d#e?f&g.png");
    }

    /// Pins the exact encoded literal so a drift in the encoding scheme is
    /// caught here, not just via round-trip. The frontend mirror
    /// (`frontend/src/editor/blob_ref.rs::blob_ref_matches_backend_form`)
    /// asserts the identical literals for the identical fixtures — that's
    /// the cross-implementation parity contract (the frontend crate can't
    /// depend on this one, so the two copies are pinned by matching test
    /// literals instead of a shared function).
    #[test]
    fn blob_ref_encoded_literal_matches_frontend_mirror() {
        assert_eq!(
            blob_ref("b1", "blobs/d1/b1/pic name.png"),
            "ogre-blob:b1/blobs%2Fd1%2Fb1%2Fpic%20name.png"
        );
        assert_eq!(
            blob_ref("blob-42", "a b/c%d#e?f&g.png"),
            "ogre-blob:blob-42/a%20b%2Fc%25d%23e%3Ff%26g.png"
        );
    }

    // ── collect_blob_refs / rewrite_blob_refs ─────────────────────

    use yrs::types::xml::XmlElementPrelim;
    use yrs::WriteTxn;

    fn doc_with<F: FnOnce(&mut yrs::TransactionMut<'_>, &yrs::XmlFragmentRef)>(f: F) -> Doc {
        let doc = Doc::new();
        {
            let mut txn = doc.transact_mut();
            let fragment = txn.get_or_insert_xml_fragment("content");
            f(&mut txn, &fragment);
        }
        doc
    }

    #[test]
    fn collect_blob_refs_finds_top_level_and_nested_images_and_dedupes() {
        let doc = doc_with(|txn, frag| {
            let img1 = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img1.insert_attribute(txn, "src", &blob_ref("b1", "k1"));

            // Nested: table > tableRow > tableCell > image — exercises the
            // recursive walk, not just top-level fragment children.
            let table = frag.insert(txn, 1, XmlElementPrelim::empty(NodeType::Table.tag_name()));
            let row = table.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableRow.tag_name()));
            let cell = row.insert(txn, 0, XmlElementPrelim::empty(NodeType::TableCell.tag_name()));
            let img2 = cell.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img2.insert_attribute(txn, "src", &blob_ref("b2", "k2"));

            // Duplicate reference to b1/k1 — must be deduped.
            let img3 = frag.insert(txn, 2, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img3.insert_attribute(txn, "src", &blob_ref("b1", "k1"));

            // Legacy absolute URL — must be ignored, not collected.
            let img4 = frag.insert(txn, 3, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img4.insert_attribute(txn, "src", "https://example.com/legacy.png");
        });

        let mut refs = collect_blob_refs(&doc);
        refs.sort();
        assert_eq!(
            refs,
            vec![
                ("b1".to_string(), "k1".to_string()),
                ("b2".to_string(), "k2".to_string()),
            ]
        );
    }

    #[test]
    fn collect_blob_refs_empty_doc_returns_empty() {
        let doc = Doc::new();
        assert!(collect_blob_refs(&doc).is_empty());
    }

    #[test]
    fn rewrite_blob_refs_replaces_matching_refs_and_leaves_the_rest() {
        let ref1 = blob_ref("b1", "k1");
        let ref2 = blob_ref("b2", "k2");
        let doc = doc_with(|txn, frag| {
            let img1 = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img1.insert_attribute(txn, "src", ref1.as_str());

            // Not in the resolved map — must be left untouched.
            let img2 = frag.insert(txn, 1, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img2.insert_attribute(txn, "src", ref2.as_str());

            // Legacy absolute URL — must be left untouched.
            let img3 = frag.insert(txn, 2, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img3.insert_attribute(txn, "src", "https://example.com/legacy.png");
        });

        let mut resolved = HashMap::new();
        resolved.insert(ref1.clone(), "https://s3.example.com/fresh1?sig=1".to_string());

        rewrite_blob_refs(&doc, &resolved);

        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let mut srcs = Vec::new();
        for i in 0..fragment.len(&txn) {
            if let Some(XmlOut::Element(el)) = fragment.get(&txn, i) {
                srcs.push(el.get_attribute(&txn, "src").unwrap());
            }
        }
        assert_eq!(
            srcs,
            vec![
                "https://s3.example.com/fresh1?sig=1".to_string(),
                ref2,
                "https://example.com/legacy.png".to_string(),
            ]
        );
    }

    #[test]
    fn rewrite_blob_refs_empty_map_is_a_noop() {
        let ref1 = blob_ref("b1", "k1");
        let doc = doc_with(|txn, frag| {
            let img = frag.insert(txn, 0, XmlElementPrelim::empty(NodeType::Image.tag_name()));
            img.insert_attribute(txn, "src", ref1.as_str());
        });
        rewrite_blob_refs(&doc, &HashMap::new());

        let txn = doc.transact();
        let fragment = txn.get_xml_fragment("content").unwrap();
        let Some(XmlOut::Element(el)) = fragment.get(&txn, 0) else {
            panic!("image missing");
        };
        assert_eq!(el.get_attribute(&txn, "src"), Some(ref1));
    }
}
