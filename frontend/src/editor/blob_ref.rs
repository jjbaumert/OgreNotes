// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Durable blob-reference helpers, mirrored from the backend's
//! `crates/collab/src/blob_ref.rs::{blob_ref, parse_blob_ref}`.
//!
//! The frontend crate doesn't depend on `ogrenotes-collab` for this path
//! (it's a standalone wasm crate — see the module-level note in
//! `editor/view.rs` about the lib/bin split), so the two small functions
//! are duplicated here rather than shared. [`blob_ref_matches_backend_form`]
//! below pins the two sides to the identical output for the same input, in
//! the spirit of the `schema.rs` cross-schema consistency tests
//! (`crates/collab/src/schema.rs`) — if either side's encoding drifts,
//! that test (and this one) needs a matching update on both sides.

use std::borrow::Cow;

/// Must match `crates/collab/src/blob_ref.rs::BLOB_REF_PREFIX`.
pub const BLOB_REF_PREFIX: &str = "ogre-blob:";

/// `Image.src` form for a blob owned by this workspace:
///   `ogre-blob:<blob_id>/<url-encoded key>`
/// Mirrors the backend's `blob_ref` byte-for-byte.
pub fn blob_ref(blob_id: &str, key: &str) -> String {
    let encoded_key: Cow<str> = percent_encode(key);
    format!("{BLOB_REF_PREFIX}{blob_id}/{encoded_key}")
}

/// Inverse of [`blob_ref`]. Returns `None` for anything that doesn't start
/// with [`BLOB_REF_PREFIX`] — legacy presigned URLs and external absolute
/// URLs alike — so the render path can fall back to using the string
/// verbatim (backward compatible with documents written before this
/// change).
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
/// "unreserved" set (`A-Za-z0-9-_.~`) as `%XX` UTF-8 bytes. Kept
/// byte-for-byte identical to the backend copy — see the module doc.
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
/// escapes or invalid UTF-8 after decoding.
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
        assert!(parse_blob_ref("blobs/d1/b1/pic.png").is_none());
        assert!(parse_blob_ref("ogre-blob:b1").is_none());
        assert!(parse_blob_ref("ogre-blob:/key").is_none());
        assert!(parse_blob_ref("ogre-blob:b1/").is_none());
    }

    /// Schema-duality-style parity check: the frontend's `blob_ref` must
    /// produce the exact same literal the backend's
    /// `crates/collab/src/blob_ref.rs::blob_ref` produces for the same
    /// input. The backend test `blob_ref_key_with_special_characters_round_trips`
    /// exercises the same fixture; this hardcoded literal is the pinned
    /// contract between the two independently-compiled implementations.
    #[test]
    fn blob_ref_matches_backend_form() {
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
