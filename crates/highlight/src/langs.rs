// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use crate::lexer::LexerSpec;
use crate::Language;

/// Spec table for every generically-lexed language. HTML and YAML are
/// bespoke (see `bespoke.rs`) and never reach this function via
/// `highlight`, but return a safe default if they do.
pub(crate) fn spec_for(lang: Language) -> LexerSpec {
    match lang {
        Language::Rust => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            // NOTE: `'` is deliberately NOT a delimiter — lifetimes.
            string_delims: &['"'],
            rust_raw_strings: true,
            hash_meta: true,
            fn_calls: true,
            caps_types: true,
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate",
                "dyn", "else", "enum", "extern", "false", "fn", "for", "if",
                "impl", "in", "let", "loop", "match", "mod", "move", "mut",
                "pub", "ref", "return", "self", "static", "struct", "super",
                "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            types: &[
                "bool", "char", "str", "String", "i8", "i16", "i32", "i64",
                "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
                "f32", "f64", "Vec", "Option", "Result", "Box", "Rc", "Arc",
                "HashMap", "HashSet", "Self",
            ],
            ..LexerSpec::DEFAULT
        },
        // Tasks 3–4 fill these in; until then every other language
        // tokenizes with no rules (single Plain run) — still a valid
        // partition.
        _ => LexerSpec::DEFAULT,
    }
}
