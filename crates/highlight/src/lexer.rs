// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use crate::{Token, TokenKind};

/// Declarative description of a language for the generic lexer.
/// Most supported languages are "C-ish comments + quoted strings +
/// keyword table" and fit this; HTML and YAML get bespoke lexers.
pub(crate) struct LexerSpec {
    pub line_comments: &'static [&'static str],
    pub block_comment: Option<(&'static str, &'static str)>,
    /// String delimiters. A string runs to the matching delimiter,
    /// honoring `\` escapes, and stops (unterminated) at a newline
    /// unless the delimiter is in `multiline_delims`.
    pub string_delims: &'static [char],
    pub multiline_delims: &'static [char],
    /// Python/TOML `"""…"""` / `'''…'''`.
    pub triple_quoted: bool,
    /// Rust `r"…"` / `r#"…"#` / `r##"…"##`.
    pub rust_raw_strings: bool,
    pub keywords: &'static [&'static str],
    pub types: &'static [&'static str],
    /// Identifiers starting with an uppercase letter render as Type.
    pub caps_types: bool,
    /// An identifier immediately followed by `(` renders as Function.
    pub fn_calls: bool,
    /// Keywords match case-insensitively (SQL).
    pub ci_keywords: bool,
    /// `#…` to end of line is Meta (Rust attributes, C preprocessor).
    /// Mutually exclusive with `#` in `line_comments`.
    pub hash_meta: bool,
    /// `@identifier` is Meta (decorators, annotations, CSS at-rules).
    pub at_meta: bool,
    /// `$identifier` and `${…}` are Meta (bash, HCL interpolation).
    pub dollar_meta: bool,
}

impl LexerSpec {
    pub(crate) const DEFAULT: LexerSpec = LexerSpec {
        line_comments: &[],
        block_comment: None,
        string_delims: &[],
        multiline_delims: &[],
        triple_quoted: false,
        rust_raw_strings: false,
        keywords: &[],
        types: &[],
        caps_types: false,
        fn_calls: false,
        ci_keywords: false,
        hash_meta: false,
        at_meta: false,
        dollar_meta: false,
    };
}

/// Tokenize `src` against `spec`.
///
/// Partition-by-construction: the cursor `i` only ever advances, every
/// emitted token is the contiguous slice `&src[start..end]` between
/// cursor positions, and pending plain text is flushed before every
/// non-plain token — so the concatenation of all tokens is exactly
/// `src`. All indices come from `char_indices`/`find` on `src`, so
/// slicing stays on char boundaries.
pub(crate) fn tokenize<'a>(src: &'a str, spec: &LexerSpec) -> Vec<Token<'a>> {
    let mut out: Vec<Token<'a>> = Vec::new();
    let mut plain_start: Option<usize> = None;
    let mut i = 0;

    while i < src.len() {
        let rest = &src[i..];
        // `rest` is non-empty (i < len) and starts on a char boundary.
        let c = rest.chars().next().unwrap();

        // Block comment
        if let Some((open, close)) = spec.block_comment {
            if rest.starts_with(open) {
                flush_plain(&mut out, src, &mut plain_start, i);
                let end = rest[open.len()..]
                    .find(close)
                    .map(|p| i + open.len() + p + close.len())
                    .unwrap_or(src.len());
                out.push(Token { text: &src[i..end], kind: TokenKind::Comment });
                i = end;
                continue;
            }
        }

        // `#…` meta (checked before line comments; specs never set both)
        if spec.hash_meta && c == '#' {
            flush_plain(&mut out, src, &mut plain_start, i);
            let end = rest.find('\n').map(|p| i + p).unwrap_or(src.len());
            out.push(Token { text: &src[i..end], kind: TokenKind::Meta });
            i = end;
            continue;
        }

        // Line comment
        if spec.line_comments.iter().any(|lc| rest.starts_with(lc)) {
            flush_plain(&mut out, src, &mut plain_start, i);
            let end = rest.find('\n').map(|p| i + p).unwrap_or(src.len());
            out.push(Token { text: &src[i..end], kind: TokenKind::Comment });
            i = end;
            continue;
        }

        // Rust raw string
        if spec.rust_raw_strings && c == 'r' {
            if let Some(len) = rust_raw_string_len(rest) {
                flush_plain(&mut out, src, &mut plain_start, i);
                out.push(Token { text: &src[i..i + len], kind: TokenKind::String });
                i += len;
                continue;
            }
        }

        // Triple-quoted string
        if spec.triple_quoted && (rest.starts_with("\"\"\"") || rest.starts_with("'''")) {
            flush_plain(&mut out, src, &mut plain_start, i);
            let delim = &rest[..3];
            let end = rest[3..]
                .find(delim)
                .map(|p| i + 3 + p + 3)
                .unwrap_or(src.len());
            out.push(Token { text: &src[i..end], kind: TokenKind::String });
            i = end;
            continue;
        }

        // String
        if spec.string_delims.contains(&c) {
            flush_plain(&mut out, src, &mut plain_start, i);
            let multiline = spec.multiline_delims.contains(&c);
            let end = string_end(src, i, c, multiline);
            out.push(Token { text: &src[i..end], kind: TokenKind::String });
            i = end;
            continue;
        }

        // @meta / $meta
        if (spec.at_meta && c == '@') || (spec.dollar_meta && c == '$') {
            if let Some(end) = meta_end(src, i, c) {
                flush_plain(&mut out, src, &mut plain_start, i);
                out.push(Token { text: &src[i..end], kind: TokenKind::Meta });
                i = end;
                continue;
            }
            // bare @/$ falls through to plain
        }

        // Number
        if c.is_ascii_digit() {
            flush_plain(&mut out, src, &mut plain_start, i);
            let end = number_end(src, i);
            out.push(Token { text: &src[i..end], kind: TokenKind::Number });
            i = end;
            continue;
        }

        // Identifier / keyword / type / function
        if c.is_alphabetic() || c == '_' {
            flush_plain(&mut out, src, &mut plain_start, i);
            let end = ident_end(src, i);
            let word = &src[i..end];
            let kind = classify_word(word, spec, src[end..].starts_with('('));
            out.push(Token { text: word, kind });
            i = end;
            continue;
        }

        // Anything else accumulates as plain text.
        if plain_start.is_none() {
            plain_start = Some(i);
        }
        i += c.len_utf8();
    }

    flush_plain(&mut out, src, &mut plain_start, src.len());
    out
}

fn flush_plain<'a>(
    out: &mut Vec<Token<'a>>,
    src: &'a str,
    start: &mut Option<usize>,
    end: usize,
) {
    if let Some(s) = start.take() {
        if s < end {
            out.push(Token { text: &src[s..end], kind: TokenKind::Plain });
        }
    }
}

/// End of a quoted string starting at `start` (which holds `delim`).
/// Honors `\` escapes. A non-multiline string left open at a newline
/// ends *before* the newline (the newline stays plain).
fn string_end(src: &str, start: usize, delim: char, multiline: bool) -> usize {
    let body = start + delim.len_utf8();
    let mut escaped = false;
    for (off, ch) in src[body..].char_indices() {
        let pos = body + off;
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == '\n' && !multiline {
            return pos;
        } else if ch == delim {
            return pos + ch.len_utf8();
        }
    }
    src.len()
}

/// Length of a Rust raw string (`r"…"`, `r#"…"#`, …) at the start of
/// `rest`, or None if `rest` isn't one. Unterminated → rest of input.
fn rust_raw_string_len(rest: &str) -> Option<usize> {
    let after_r = &rest[1..];
    let hashes = after_r.len() - after_r.trim_start_matches('#').len();
    let quote_at = 1 + hashes;
    if !rest[quote_at..].starts_with('"') {
        return None;
    }
    let closer: String = format!("\"{}", "#".repeat(hashes));
    let body = quote_at + 1;
    Some(
        rest[body..]
            .find(&closer)
            .map(|p| body + p + closer.len())
            .unwrap_or(rest.len()),
    )
}

/// End of `@ident`, `$ident`, or `${…}` starting at `start`; None if
/// the sigil isn't followed by an identifier char or `{`.
fn meta_end(src: &str, start: usize, sigil: char) -> Option<usize> {
    let body = start + sigil.len_utf8();
    let next = src[body..].chars().next()?;
    if next == '{' && sigil == '$' {
        // ${…} to the closing brace (or end of line, whichever first)
        for (off, ch) in src[body..].char_indices() {
            match ch {
                '}' => return Some(body + off + 1),
                '\n' => return Some(body + off),
                _ => {}
            }
        }
        return Some(src.len());
    }
    if next.is_alphabetic() || next == '_' {
        return Some(ident_end(src, body));
    }
    None
}

fn number_end(src: &str, start: usize) -> usize {
    let mut end = start;
    let mut chars = src[start..].char_indices().peekable();
    while let Some((off, ch)) = chars.next() {
        let pos = start + off;
        if ch.is_ascii_alphanumeric() || ch == '_' {
            end = pos + ch.len_utf8();
        } else if ch == '.' {
            // consume the dot only when a digit follows (1.5 yes, 1..10 no)
            match chars.peek() {
                Some((_, d)) if d.is_ascii_digit() => end = pos + 1,
                _ => break,
            }
        } else {
            break;
        }
    }
    end
}

fn ident_end(src: &str, start: usize) -> usize {
    src[start..]
        .char_indices()
        .find(|(_, ch)| !ch.is_alphanumeric() && *ch != '_')
        .map(|(off, _)| start + off)
        .unwrap_or(src.len())
}

fn classify_word(word: &str, spec: &LexerSpec, next_is_paren: bool) -> TokenKind {
    let is_kw = if spec.ci_keywords {
        spec.keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
    } else {
        spec.keywords.contains(&word)
    };
    if is_kw {
        return TokenKind::Keyword;
    }
    if spec.types.contains(&word) {
        return TokenKind::Type;
    }
    if spec.caps_types && word.chars().next().is_some_and(|c| c.is_uppercase()) {
        return TokenKind::Type;
    }
    if spec.fn_calls && next_is_paren {
        return TokenKind::Function;
    }
    TokenKind::Plain
}

#[cfg(test)]
mod tests {
    use crate::{highlight, Language, Token, TokenKind};
    use super::{tokenize, LexerSpec};
    use proptest::prelude::*;

    fn kinds_of(src: &str, lang: Language) -> Vec<(String, TokenKind)> {
        highlight(src, lang)
            .into_iter()
            .map(|t| (t.text.to_string(), t.kind))
            .collect()
    }

    fn assert_partition(src: &str, lang: Language) {
        let joined: String = highlight(src, lang).iter().map(|t| t.text).collect();
        assert_eq!(joined, src, "partition violated for {lang:?}");
    }

    #[test]
    fn rust_keywords_types_and_functions() {
        let toks = kinds_of("fn main() { let x: u32 = 5; }", Language::Rust);
        assert!(toks.contains(&("fn".into(), TokenKind::Keyword)));
        assert!(toks.contains(&("let".into(), TokenKind::Keyword)));
        assert!(toks.contains(&("u32".into(), TokenKind::Type)));
        assert!(toks.contains(&("main".into(), TokenKind::Function)));
        assert!(toks.contains(&("5".into(), TokenKind::Number)));
    }

    #[test]
    fn rust_line_and_block_comments() {
        let toks = kinds_of("x // hi\n/* multi\nline */ y", Language::Rust);
        assert!(toks.contains(&("// hi".into(), TokenKind::Comment)));
        assert!(toks.contains(&("/* multi\nline */".into(), TokenKind::Comment)));
        assert_partition("x // hi\n/* multi\nline */ y", Language::Rust);
    }

    #[test]
    fn rust_strings_with_escapes_and_raw() {
        // Outer literal must be r##…## — the sample code itself
        // contains a r#"…"# raw string.
        let src = r##"let s = "a\"b"; let r = r#"raw"#;"##;
        let toks = kinds_of(src, Language::Rust);
        assert!(toks.contains(&(r#""a\"b""#.into(), TokenKind::String)));
        assert!(toks.iter().any(|(t, k)| t.starts_with("r#\"") && *k == TokenKind::String));
    }

    #[test]
    fn rust_attr_is_meta_and_lifetimes_stay_plain() {
        let toks = kinds_of("#[derive(Debug)]\nfn f<'a>(x: &'a str) {}", Language::Rust);
        assert!(toks.contains(&("#[derive(Debug)]".into(), TokenKind::Meta)));
        // `'` is NOT a Rust string delimiter here — lifetimes must not
        // open a string and swallow the rest of the signature.
        assert!(toks.contains(&("str".into(), TokenKind::Type)));
    }

    #[test]
    fn unterminated_string_and_comment_reach_eof_without_panic() {
        assert_partition("\"never closed", Language::Rust);
        assert_partition("/* never closed", Language::Rust);
        assert_partition("r#\"never closed", Language::Rust);
    }

    #[test]
    fn single_line_string_stops_at_newline() {
        // The newline is NOT part of the string token.
        let toks = kinds_of("\"open\nnext", Language::Rust);
        assert_eq!(toks[0], ("\"open".into(), TokenKind::String));
        assert_partition("\"open\nnext", Language::Rust);
    }

    #[test]
    fn number_dot_only_consumed_before_digit() {
        // `1..10` must not swallow both dots (range syntax).
        let toks = kinds_of("1..10", Language::Rust);
        assert_eq!(toks[0], ("1".into(), TokenKind::Number));
        assert!(toks.contains(&("10".into(), TokenKind::Number)));
        // but a real float works
        let toks = kinds_of("1.5", Language::Rust);
        assert_eq!(toks[0], ("1.5".into(), TokenKind::Number));
    }

    #[test]
    fn multibyte_input_is_sliced_on_char_boundaries() {
        assert_partition("let s = \"héllo → 世界\"; // ünïcode 🎉", Language::Rust);
    }

    // Coverage for five engine features untested by partition.rs property test:
    // triple_quoted, multiline_delims, at_meta, dollar_meta, ci_keywords

    fn all_features_spec() -> LexerSpec {
        LexerSpec {
            line_comments: &["//", "#"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            multiline_delims: &['`'],
            triple_quoted: true,
            rust_raw_strings: true,
            keywords: &["kw", "SELECT"],
            types: &["Ty"],
            caps_types: true,
            fn_calls: true,
            ci_keywords: true,
            hash_meta: false, // '#' is a line comment in this spec; mutually exclusive
            at_meta: true,
            dollar_meta: true,
        }
    }

    fn all_features_with_hash_spec() -> LexerSpec {
        LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            multiline_delims: &['`'],
            triple_quoted: true,
            rust_raw_strings: true,
            keywords: &["kw", "SELECT"],
            types: &["Ty"],
            caps_types: true,
            fn_calls: true,
            ci_keywords: true,
            hash_meta: true,
            at_meta: true,
            dollar_meta: true,
        }
    }

    #[test]
    fn all_features_triple_quoted_double() {
        let src = r#""""x""""#;
        let toks = tokenize(src, &all_features_spec());
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, TokenKind::String);
        assert_eq!(toks[0].text, src);
    }

    #[test]
    fn all_features_triple_quoted_single() {
        let src = "'''x'''";
        let toks = tokenize(src, &all_features_spec());
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, TokenKind::String);
        assert_eq!(toks[0].text, src);
    }

    #[test]
    fn all_features_multiline_delim() {
        let src = "`a\nb`";
        let toks = tokenize(src, &all_features_spec());
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind, TokenKind::String);
        assert_eq!(toks[0].text, src);
    }

    #[test]
    fn all_features_at_meta() {
        let toks = tokenize("@name", &all_features_spec());
        assert!(toks.contains(&Token { text: "@name", kind: TokenKind::Meta }));
    }

    #[test]
    fn all_features_dollar_meta_simple() {
        let toks = tokenize("$var", &all_features_spec());
        assert!(toks.contains(&Token { text: "$var", kind: TokenKind::Meta }));
    }

    #[test]
    fn all_features_dollar_meta_braces() {
        let toks = tokenize("${x}", &all_features_spec());
        assert!(toks.contains(&Token { text: "${x}", kind: TokenKind::Meta }));
    }

    #[test]
    fn all_features_ci_keywords_lowercase() {
        let toks = tokenize("select", &all_features_spec());
        assert!(toks.contains(&Token { text: "select", kind: TokenKind::Keyword }));
    }

    #[test]
    fn all_features_ci_keywords_uppercase() {
        let toks = tokenize("SELECT", &all_features_spec());
        assert!(toks.contains(&Token { text: "SELECT", kind: TokenKind::Keyword }));
    }

    #[test]
    fn all_features_ci_keywords_mixed() {
        let toks = tokenize("SeLeCt", &all_features_spec());
        assert!(toks.contains(&Token { text: "SeLeCt", kind: TokenKind::Keyword }));
    }

    proptest! {
        #[test]
        fn all_features_partition_roundtrip(src in r"[\PC]*") {
            let toks = tokenize(&src, &all_features_spec());
            let joined: String = toks.iter().map(|t| t.text).collect();
            prop_assert_eq!(joined, src, "partition violated");
        }

        #[test]
        fn all_features_with_hash_partition_roundtrip(src in r"[\PC]*") {
            let toks = tokenize(&src, &all_features_with_hash_spec());
            let joined: String = toks.iter().map(|t| t.text).collect();
            prop_assert_eq!(joined, src, "partition violated");
        }
    }
}
