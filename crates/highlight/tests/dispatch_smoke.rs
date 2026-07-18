// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Dispatch-level classification smoke: through the PUBLIC API, every
//! language must still color a representative snippet — i.e. produce at
//! least one non-Plain token. The per-language spot tests in
//! `src/{langs,lexer,bespoke}.rs` call internals, so a regression in the
//! `highlight()` → spec/bespoke dispatch (e.g. a language accidentally
//! routed to `LexerSpec::DEFAULT`) could turn a whole language Plain
//! while every unit test stays green. This test closes that seam.

use ogrenotes_highlight::{highlight, Language, TokenKind};

#[test]
fn every_language_colors_a_representative_snippet() {
    let snippets: [(Language, &str); 20] = [
        (Language::Rust, "fn main() { let x = 1; }"),
        (Language::JavaScript, "const x = 1; // c"),
        (Language::TypeScript, "interface F { n: number }"),
        (Language::Python, "def f():\n    return None"),
        (Language::Json, "{\"a\": true, \"n\": 1}"),
        (Language::Toml, "# c\nkey = \"v\""),
        (Language::Yaml, "# c\nname: value"),
        (Language::Bash, "# c\necho \"hi\""),
        (Language::Sql, "SELECT id FROM users;"),
        (Language::Html, "<div class=\"x\">hi</div>"),
        (Language::Css, "/* c */ .a { color: red; }"),
        (Language::Java, "public class A { }"),
        (Language::Kotlin, "fun main() { val x = 1 }"),
        (Language::CSharp, "public class A { }"),
        (Language::C, "int main(void) { return 0; }"),
        (Language::Cpp, "int main() { return 0; }"),
        (Language::Go, "func main() { x := 1 }"),
        (Language::Dockerfile, "FROM alpine\nRUN echo hi"),
        (Language::Hcl, "resource \"a\" \"b\" { x = 1 }"),
        (Language::Protobuf, "message M { int32 n = 1; }"),
    ];

    // Every Language::ALL member must appear exactly once — a new
    // language without a snippet here should fail loudly, not silently
    // skip the smoke.
    assert_eq!(snippets.len(), Language::ALL.len());
    for lang in Language::ALL {
        assert_eq!(
            snippets.iter().filter(|(l, _)| *l == lang).count(),
            1,
            "exactly one snippet for {lang:?}"
        );
    }

    for (lang, src) in snippets {
        let tokens = highlight(src, lang);
        assert!(
            tokens.iter().any(|t| t.kind != TokenKind::Plain),
            "{lang:?} produced only Plain tokens for {src:?} — dispatch or spec regression"
        );
    }
}
