// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

#![forbid(unsafe_code)]

//! Pure-Rust syntax tokenizer shared by the WASM frontend (editor
//! rendering) and the server (HTML export) — same shared-crate shape
//! as `ogrenotes-mermaid`. std-only, no dependencies, wasm-clean.
//!
//! Hard invariant: `highlight` is a pure partition of its input —
//! the concatenation of every token's `text` equals `source`
//! byte-for-byte, and it never panics. The editor's caret mapping
//! depends on this (see `frontend/src/editor/view.rs` DOM walkers).

mod lexer;
mod langs;
mod bespoke;

/// Blocks longer than this render unhighlighted (linear lexing is
/// cheap, but the editor re-renders per keystroke — don't gamble on
/// pathological blocks).
pub const MAX_HIGHLIGHT_CHARS: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    Type,
    String,
    Comment,
    Number,
    Function,
    Meta,
    Plain,
}

impl TokenKind {
    /// CSS class the editor stamps on the token's `<span>`.
    /// `None` (Plain) renders as a bare text node.
    pub fn css_class(self) -> Option<&'static str> {
        match self {
            TokenKind::Keyword => Some("tok-keyword"),
            TokenKind::Type => Some("tok-type"),
            TokenKind::String => Some("tok-string"),
            TokenKind::Comment => Some("tok-comment"),
            TokenKind::Number => Some("tok-number"),
            TokenKind::Function => Some("tok-function"),
            TokenKind::Meta => Some("tok-meta"),
            TokenKind::Plain => None,
        }
    }
}

/// Single source of truth for token colors. The CSS custom
/// properties in `frontend/style/main.css` mirror these values;
/// HTML export inlines them directly.
pub fn color_for(kind: TokenKind, dark: bool) -> Option<&'static str> {
    Some(match (kind, dark) {
        (TokenKind::Keyword, false) => "#cf222e",
        (TokenKind::Keyword, true) => "#ff7b72",
        (TokenKind::Type, false) => "#953800",
        (TokenKind::Type, true) => "#ffa657",
        (TokenKind::String, false) => "#0a3069",
        (TokenKind::String, true) => "#a5d6ff",
        (TokenKind::Comment, false) => "#6e7781",
        (TokenKind::Comment, true) => "#8b949e",
        (TokenKind::Number, false) => "#0550ae",
        (TokenKind::Number, true) => "#79c0ff",
        (TokenKind::Function, false) => "#8250df",
        (TokenKind::Function, true) => "#d2a8ff",
        (TokenKind::Meta, false) => "#116329",
        (TokenKind::Meta, true) => "#7ee787",
        (TokenKind::Plain, _) => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Python,
    Json,
    Toml,
    Yaml,
    Bash,
    Sql,
    Html,
    Css,
    Java,
    Kotlin,
    CSharp,
    C,
    Cpp,
    Go,
    Dockerfile,
    Hcl,
    Protobuf,
}

impl Language {
    pub const ALL: [Language; 20] = [
        Language::Rust,
        Language::JavaScript,
        Language::TypeScript,
        Language::Python,
        Language::Json,
        Language::Toml,
        Language::Yaml,
        Language::Bash,
        Language::Sql,
        Language::Html,
        Language::Css,
        Language::Java,
        Language::Kotlin,
        Language::CSharp,
        Language::C,
        Language::Cpp,
        Language::Go,
        Language::Dockerfile,
        Language::Hcl,
        Language::Protobuf,
    ];

    /// Resolve a code block's `language` attribute (as written by the
    /// UI, markdown import, or pasted `language-*` classes) to a
    /// supported language. Case-insensitive, alias-tolerant. `None`
    /// means render plain.
    pub fn from_tag(tag: &str) -> Option<Language> {
        Some(match tag.trim().to_ascii_lowercase().as_str() {
            "rust" | "rs" => Language::Rust,
            "javascript" | "js" | "jsx" | "mjs" | "node" => Language::JavaScript,
            "typescript" | "ts" | "tsx" => Language::TypeScript,
            "python" | "py" | "python3" => Language::Python,
            "json" | "jsonc" => Language::Json,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "bash" | "sh" | "shell" | "zsh" | "console" => Language::Bash,
            "sql" | "mysql" | "postgres" | "postgresql" | "sqlite" => Language::Sql,
            "html" | "htm" | "xml" | "svg" => Language::Html,
            "css" | "scss" | "less" => Language::Css,
            "java" => Language::Java,
            "kotlin" | "kt" | "kts" => Language::Kotlin,
            "csharp" | "cs" | "c#" => Language::CSharp,
            "c" | "h" => Language::C,
            "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hh" => Language::Cpp,
            "go" | "golang" => Language::Go,
            "dockerfile" | "docker" | "containerfile" => Language::Dockerfile,
            "hcl" | "terraform" | "tf" | "tfvars" => Language::Hcl,
            "protobuf" | "proto" => Language::Protobuf,
            _ => return None,
        })
    }

    /// Canonical tag written to the `language` attr by the selector.
    pub fn tag(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Python => "python",
            Language::Json => "json",
            Language::Toml => "toml",
            Language::Yaml => "yaml",
            Language::Bash => "bash",
            Language::Sql => "sql",
            Language::Html => "html",
            Language::Css => "css",
            Language::Java => "java",
            Language::Kotlin => "kotlin",
            Language::CSharp => "csharp",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Go => "go",
            Language::Dockerfile => "dockerfile",
            Language::Hcl => "hcl",
            Language::Protobuf => "protobuf",
        }
    }

    /// Human label for the selector chip.
    pub fn label(self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Python => "Python",
            Language::Json => "JSON",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Bash => "Bash",
            Language::Sql => "SQL",
            Language::Html => "HTML",
            Language::Css => "CSS",
            Language::Java => "Java",
            Language::Kotlin => "Kotlin",
            Language::CSharp => "C#",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Go => "Go",
            Language::Dockerfile => "Dockerfile",
            Language::Hcl => "HCL",
            Language::Protobuf => "Protobuf",
        }
    }
}

/// Tokenize `source`. Total function: never panics; the concatenation
/// of all returned token texts equals `source` exactly.
pub fn highlight(source: &str, lang: Language) -> Vec<Token<'_>> {
    match lang {
        Language::Html => bespoke::html(source),
        Language::Yaml => bespoke::yaml(source),
        other => lexer::tokenize(source, &langs::spec_for(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_tag_resolves_canonical_and_aliases() {
        assert_eq!(Language::from_tag("rust"), Some(Language::Rust));
        assert_eq!(Language::from_tag("rs"), Some(Language::Rust));
        assert_eq!(Language::from_tag("TypeScript"), Some(Language::TypeScript));
        assert_eq!(Language::from_tag("  yml "), Some(Language::Yaml));
        assert_eq!(Language::from_tag("c++"), Some(Language::Cpp));
        assert_eq!(Language::from_tag("c#"), Some(Language::CSharp));
        assert_eq!(Language::from_tag("tf"), Some(Language::Hcl));
    }

    #[test]
    fn from_tag_rejects_unknown() {
        assert_eq!(Language::from_tag(""), None);
        assert_eq!(Language::from_tag("mermaid"), None, "mermaid fences stay plain");
        assert_eq!(Language::from_tag("brainfuck"), None);
    }

    #[test]
    fn every_language_round_trips_its_canonical_tag() {
        for lang in Language::ALL {
            assert_eq!(Language::from_tag(lang.tag()), Some(lang), "tag {}", lang.tag());
        }
    }

    #[test]
    fn plain_has_no_class_and_no_color_others_have_both() {
        for kind in [TokenKind::Keyword, TokenKind::Type, TokenKind::String,
                     TokenKind::Comment, TokenKind::Number, TokenKind::Function,
                     TokenKind::Meta] {
            assert!(kind.css_class().is_some());
            assert!(color_for(kind, false).is_some());
            assert!(color_for(kind, true).is_some());
        }
        assert_eq!(TokenKind::Plain.css_class(), None);
        assert_eq!(color_for(TokenKind::Plain, false), None);
    }
}
