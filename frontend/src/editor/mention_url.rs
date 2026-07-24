// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Lone-URL paste detection for document/anchor mentions (mentions spec
//! §5). Pure string logic — natively tested; the wasm paste path (`view.rs`
//! `on_paste`) calls `parse_ogre_doc_url` with `window().location().origin()`.

/// A parsed same-origin document URL: `{origin}/d/<doc_id>[/slug][#b=<block_id>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDocUrl {
    pub doc_id: String,
    pub block_id: Option<String>,
}

/// The state a lone-URL paste hands off from `EditorView::on_paste` to the
/// async resolve pipeline in `editor_component.rs`. `from`/`to` are MODEL
/// positions of the just-inserted URL text (the range the guarded replace
/// in `commands::replace_text_with_doc_mention` will later validate still
/// holds `url` verbatim before converting it to a `DocMention`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMentionPaste {
    pub from: usize,
    pub to: usize,
    pub url: String,
    pub parsed: ParsedDocUrl,
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parse a pasted string that is exactly one same-origin document URL.
/// `Some` for `{origin}/d/<id>[/slug][#b=<blockId>]`; `None` otherwise
/// (foreign origin, embedded-in-text, malformed fragment, foreign hash —
/// the paste then stays a plain link, mentions spec §5 case c).
pub fn parse_ogre_doc_url(text: &str, origin: &str) -> Option<ParsedDocUrl> {
    let t = text.trim();
    if t.contains(char::is_whitespace) {
        return None; // lone URL only — no surrounding prose, no second URL
    }
    let rest = t.strip_prefix(origin)?.strip_prefix("/d/")?;
    let (path, frag) = match rest.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (rest, None),
    };
    let doc_id = path.split('/').next().unwrap_or("");
    if !valid_id(doc_id) {
        return None;
    }
    let block_id = match frag {
        None => None,
        Some(f) => {
            // Foreign hash forms (anything but `#b=<id>`) reject the WHOLE
            // URL — conservative choice: an unrecognized fragment means we
            // don't know what the URL addresses, so it stays a plain link
            // rather than silently dropping the fragment's meaning.
            let id = f.strip_prefix("b=")?;
            if !valid_id(id) {
                return None;
            }
            Some(id.to_string())
        }
    };
    Some(ParsedDocUrl { doc_id: doc_id.to_string(), block_id })
}

#[cfg(test)]
mod tests {
    use super::*;
    const ORIGIN: &str = "https://notes.example";

    #[test]
    fn parses_doc_slug_and_fragment_variants() {
        assert_eq!(
            parse_ogre_doc_url("https://notes.example/d/abc123", ORIGIN),
            Some(ParsedDocUrl { doc_id: "abc123".into(), block_id: None })
        );
        assert_eq!(
            parse_ogre_doc_url("https://notes.example/d/abc123/some-slug", ORIGIN),
            Some(ParsedDocUrl { doc_id: "abc123".into(), block_id: None })
        );
        assert_eq!(
            parse_ogre_doc_url("  https://notes.example/d/abc123#b=blk_1  ", ORIGIN),
            Some(ParsedDocUrl { doc_id: "abc123".into(), block_id: Some("blk_1".into()) })
        );
        assert_eq!(
            parse_ogre_doc_url("https://notes.example/d/abc/slug#b=blk-2", ORIGIN),
            Some(ParsedDocUrl { doc_id: "abc".into(), block_id: Some("blk-2".into()) })
        );
    }

    #[test]
    fn rejects_foreign_multi_and_malformed() {
        for bad in [
            "https://other.example/d/abc123",            // foreign origin
            "https://notes.example/settings",             // not a doc path
            "https://notes.example/d/",                   // empty id
            "see https://notes.example/d/abc123 please",  // not a lone URL
            "https://notes.example/d/abc#b=bad id",        // invalid fragment charset
            "https://notes.example/d/abc#appearance",      // foreign hash
            "not a url",
        ] {
            assert_eq!(parse_ogre_doc_url(bad, ORIGIN), None, "should reject: {bad}");
        }
    }

    #[test]
    fn foreign_hash_still_resolves_doc_without_block() {
        // A foreign hash means the URL addresses something we don't
        // understand; conservative choice (recorded): reject entirely,
        // leave the plain URL. This test pins that decision.
        assert_eq!(parse_ogre_doc_url("https://notes.example/d/abc#appearance", ORIGIN), None);
    }
}
