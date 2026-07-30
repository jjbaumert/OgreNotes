// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Quip HTML → OgreNotes document import.
//!
//! **Current contents:** just the durable blob-reference helpers
//! (Phase 2a Task 3) — the `Image.src` form that survives longer than a
//! presigned S3 URL's TTL. The Quip HTML walker (thread content →
//! block-grammar conversion, mirroring `import_docx`/`import_pdf`) is a
//! later Phase 2a task and lands in this same module.
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
pub fn parse_blob_ref(src: &str) -> Option<(String, String)> {
    let rest = src.strip_prefix(BLOB_REF_PREFIX)?;
    let (blob_id, encoded_key) = rest.split_once('/')?;
    if blob_id.is_empty() || encoded_key.is_empty() {
        return None;
    }
    let key = percent_decode(encoded_key)?;
    Some((blob_id.to_string(), key))
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
}
