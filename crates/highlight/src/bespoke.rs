// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Hand-written lexers for languages the generic engine can't model:
//! HTML (tag structure) and YAML (line-oriented keys). Same contract
//! as the generic engine: pure partition, never panics.

use crate::{Token, TokenKind};

/// HTML/XML. State machine: outside tags everything is Plain except
/// `<!-- … -->` comments; inside `<…>` the tag name is Keyword,
/// attribute names Type, quoted values String.
pub(crate) fn html(src: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut plain_start: Option<usize> = None;
    let mut i = 0;

    while i < src.len() {
        let rest = &src[i..];

        if let Some(after_open) = rest.strip_prefix("<!--") {
            flush(&mut out, src, &mut plain_start, i);
            let end = after_open.find("-->").map(|p| i + 4 + p + 3).unwrap_or(src.len());
            out.push(Token { text: &src[i..end], kind: TokenKind::Comment });
            i = end;
            continue;
        }

        // A tag opens only when `<` is followed by a letter, `/`, `!`, or `?`.
        if rest.starts_with('<')
            && rest[1..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '/' || c == '!' || c == '?')
        {
            flush(&mut out, src, &mut plain_start, i);
            i = lex_tag(src, i, &mut out);
            continue;
        }

        if plain_start.is_none() {
            plain_start = Some(i);
        }
        i += rest.chars().next().unwrap().len_utf8();
    }
    flush(&mut out, src, &mut plain_start, src.len());
    out
}

/// Lex one `<…>` region starting at `start` (which holds `<`).
/// Returns the index just past the closing `>` (or EOF). Pushes:
/// punctuation as Plain, tag name as Keyword, attr names as Type,
/// quoted values as String.
fn lex_tag<'a>(src: &'a str, start: usize, out: &mut Vec<Token<'a>>) -> usize {
    let mut i = start;
    let mut seen_name = false;
    let mut plain_start: Option<usize> = None;

    while i < src.len() {
        let c = src[i..].chars().next().unwrap();
        match c {
            '>' => {
                flush(out, src, &mut plain_start, i);
                out.push(Token { text: &src[i..i + 1], kind: TokenKind::Plain });
                return i + 1;
            }
            '"' | '\'' => {
                flush(out, src, &mut plain_start, i);
                let end = quoted_end(src, i, c);
                out.push(Token { text: &src[i..end], kind: TokenKind::String });
                i = end;
            }
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' => {
                flush(out, src, &mut plain_start, i);
                let end = src[i..]
                    .char_indices()
                    .find(|(_, ch)| !ch.is_ascii_alphanumeric() && !"-_:.".contains(*ch))
                    .map(|(off, _)| i + off)
                    .unwrap_or(src.len());
                let kind = if seen_name { TokenKind::Type } else { TokenKind::Keyword };
                seen_name = true;
                out.push(Token { text: &src[i..end], kind });
                i = end;
            }
            c => {
                if plain_start.is_none() {
                    plain_start = Some(i);
                }
                i += c.len_utf8();
            }
        }
    }
    flush(out, src, &mut plain_start, src.len());
    src.len()
}

/// YAML, line-oriented: full-line/trailing `#` comments, `key:` as
/// Keyword, quoted scalars as String, bare numbers as Number,
/// true/false/null as Type, `---`/`...` document markers as Meta.
pub(crate) fn yaml(src: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut line_start = 0;

    while line_start <= src.len() {
        let line_end = src[line_start..]
            .find('\n')
            .map(|p| line_start + p)
            .unwrap_or(src.len());
        lex_yaml_line(src, line_start, line_end, &mut out);
        if line_end == src.len() {
            break;
        }
        // The newline itself is a Plain token.
        out.push(Token { text: &src[line_end..line_end + 1], kind: TokenKind::Plain });
        line_start = line_end + 1;
    }
    out
}

fn lex_yaml_line<'a>(src: &'a str, start: usize, end: usize, out: &mut Vec<Token<'a>>) {
    let line = &src[start..end];
    if line.is_empty() {
        return;
    }
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();

    if trimmed == "---" || trimmed == "..." {
        if indent_len > 0 {
            out.push(Token { text: &src[start..start + indent_len], kind: TokenKind::Plain });
        }
        out.push(Token { text: &src[start + indent_len..end], kind: TokenKind::Meta });
        return;
    }

    // Walk the line: indent / `- ` prefix as plain, then optional
    // `key:` then scalars. Trailing comments start at an unquoted `#`.
    let mut i = start;
    let mut plain_start: Option<usize> = None;
    let mut key_possible = true;

    while i < end {
        let c = src[i..].chars().next().unwrap();

        if c == '#' {
            flush(out, src, &mut plain_start, i);
            out.push(Token { text: &src[i..end], kind: TokenKind::Comment });
            return;
        }
        if c == '"' || c == '\'' {
            flush(out, src, &mut plain_start, i);
            let str_end = quoted_end(src, i, c).min(end);
            out.push(Token { text: &src[i..str_end], kind: TokenKind::String });
            i = str_end;
            key_possible = false;
            continue;
        }
        if c.is_ascii_digit() {
            flush(out, src, &mut plain_start, i);
            let num_end = src[i..end]
                .char_indices()
                .find(|(_, ch)| !ch.is_ascii_alphanumeric() && *ch != '.' && *ch != '_')
                .map(|(off, _)| i + off)
                .unwrap_or(end);
            out.push(Token { text: &src[i..num_end], kind: TokenKind::Number });
            i = num_end;
            key_possible = false;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            flush(out, src, &mut plain_start, i);
            let word_end = src[i..end]
                .char_indices()
                .find(|(_, ch)| !ch.is_alphanumeric() && !"-_".contains(*ch))
                .map(|(off, _)| i + off)
                .unwrap_or(end);
            let word = &src[i..word_end];
            // `key:` (colon followed by space/EOL) → Keyword
            let colon_next = src[word_end..end].starts_with(':')
                && src[word_end + 1..end]
                    .chars()
                    .next()
                    .is_none_or(|n| n == ' ' || n == '\t');
            let kind = if key_possible && colon_next {
                TokenKind::Keyword
            } else if matches!(word, "true" | "false" | "null" | "yes" | "no") {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };
            out.push(Token { text: word, kind });
            i = word_end;
            if kind == TokenKind::Keyword {
                key_possible = false;
            }
            continue;
        }

        // `- ` sequence dashes, colons, whitespace, everything else: plain
        if plain_start.is_none() {
            plain_start = Some(i);
        }
        i += c.len_utf8();
    }
    flush(out, src, &mut plain_start, end);
}

/// Shared: end of a `"…"`/`'…'` region with backslash escapes,
/// clamped to EOF. (No newline clamp — HTML attr values may wrap.)
fn quoted_end(src: &str, start: usize, delim: char) -> usize {
    let body = start + delim.len_utf8();
    let mut escaped = false;
    for (off, ch) in src[body..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if ch == delim {
            return body + off + ch.len_utf8();
        }
    }
    src.len()
}

fn flush<'a>(out: &mut Vec<Token<'a>>, src: &'a str, start: &mut Option<usize>, end: usize) {
    if let Some(s) = start.take() {
        if s < end {
            out.push(Token { text: &src[s..end], kind: TokenKind::Plain });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{highlight, Language, TokenKind};

    fn has(src: &str, lang: Language, text: &str, kind: TokenKind) -> bool {
        highlight(src, lang).iter().any(|t| t.text == text && t.kind == kind)
    }

    fn assert_partition(src: &str, lang: Language) {
        let joined: String = highlight(src, lang).iter().map(|t| t.text).collect();
        assert_eq!(joined, src);
    }

    #[test]
    fn html_tags_attrs_strings_comments() {
        let src = "<!-- c --><div class=\"box\" id='x'>text</div>";
        assert!(has(src, Language::Html, "<!-- c -->", TokenKind::Comment));
        assert!(has(src, Language::Html, "div", TokenKind::Keyword));
        assert!(has(src, Language::Html, "class", TokenKind::Type));
        assert!(has(src, Language::Html, "\"box\"", TokenKind::String));
        assert!(has(src, Language::Html, "'x'", TokenKind::String));
        assert_partition(src, Language::Html);
    }

    #[test]
    fn html_text_between_tags_stays_plain_and_unterminated_is_safe() {
        assert!(has("<p>hello</p>", Language::Html, "hello", TokenKind::Plain));
        assert_partition("<div class=\"open", Language::Html);
        assert_partition("<!-- never closed", Language::Html);
        assert_partition("< 5 and > 3", Language::Html); // bare angle brackets
    }

    #[test]
    fn yaml_keys_comments_values() {
        let src = "# top\nname: ogre\ncount: 3\nflag: true\nitems:\n  - \"quoted\"\n";
        assert!(has(src, Language::Yaml, "# top", TokenKind::Comment));
        assert!(has(src, Language::Yaml, "name", TokenKind::Keyword));
        assert!(has(src, Language::Yaml, "count", TokenKind::Keyword));
        assert!(has(src, Language::Yaml, "3", TokenKind::Number));
        assert!(has(src, Language::Yaml, "true", TokenKind::Type));
        assert!(has(src, Language::Yaml, "\"quoted\"", TokenKind::String));
        assert_partition(src, Language::Yaml);
    }

    #[test]
    fn yaml_document_marker_and_no_key_lines() {
        let src = "---\n- plain item\nurl: http://x/y:z\n";
        assert!(has(src, Language::Yaml, "---", TokenKind::Meta));
        assert_partition(src, Language::Yaml);
    }
}
