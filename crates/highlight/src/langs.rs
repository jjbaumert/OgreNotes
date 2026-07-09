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
        Language::JavaScript => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            multiline_delims: &['`'],
            fn_calls: true,
            keywords: &[
                "async", "await", "break", "case", "catch", "class", "const",
                "continue", "debugger", "default", "delete", "do", "else",
                "export", "extends", "false", "finally", "for", "function",
                "if", "import", "in", "instanceof", "let", "new", "null",
                "of", "return", "static", "super", "switch", "this", "throw",
                "true", "try", "typeof", "undefined", "var", "void", "while",
                "with", "yield",
            ],
            types: &[
                "Array", "Boolean", "Date", "Error", "JSON", "Map", "Math",
                "Number", "Object", "Promise", "Proxy", "Reflect", "RegExp",
                "Set", "String", "Symbol", "WeakMap", "WeakSet", "console",
                "document", "window",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::TypeScript => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            multiline_delims: &['`'],
            fn_calls: true,
            at_meta: true, // decorators
            keywords: &[
                // JS set
                "async", "await", "break", "case", "catch", "class", "const",
                "continue", "debugger", "default", "delete", "do", "else",
                "export", "extends", "false", "finally", "for", "function",
                "if", "import", "in", "instanceof", "let", "new", "null",
                "of", "return", "static", "super", "switch", "this", "throw",
                "true", "try", "typeof", "undefined", "var", "void", "while",
                "with", "yield",
                // TS additions
                "abstract", "any", "as", "declare", "enum", "implements",
                "interface", "is", "keyof", "module", "namespace", "never",
                "private", "protected", "public", "readonly", "satisfies",
                "type", "unknown",
            ],
            types: &[
                "Array", "Boolean", "Date", "Error", "JSON", "Map", "Math",
                "Number", "Object", "Promise", "Record", "RegExp", "Set",
                "String", "Symbol", "Partial", "Required", "Readonly", "Pick",
                "Omit", "console", "document", "window",
                "boolean", "number", "object", "string", "symbol", "bigint",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Python => LexerSpec {
            line_comments: &["#"],
            string_delims: &['"', '\''],
            triple_quoted: true,
            fn_calls: true,
            at_meta: true, // decorators
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "case",
                "class", "continue", "def", "del", "elif", "else", "except",
                "False", "finally", "for", "from", "global", "if", "import",
                "in", "is", "lambda", "match", "None", "nonlocal", "not",
                "or", "pass", "raise", "return", "True", "try", "while",
                "with", "yield",
            ],
            types: &[
                "bool", "bytes", "dict", "float", "frozenset", "int", "list",
                "object", "set", "str", "tuple", "type", "self", "Exception",
                "ValueError", "TypeError", "KeyError", "RuntimeError",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Json => LexerSpec {
            // JSONC tolerance: comments are illegal in strict JSON but
            // common in config files; harmless to color them.
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"'],
            keywords: &["true", "false", "null"],
            ..LexerSpec::DEFAULT
        },
        Language::Toml => LexerSpec {
            line_comments: &["#"],
            string_delims: &['"', '\''],
            triple_quoted: true,
            keywords: &["true", "false"],
            ..LexerSpec::DEFAULT
        },
        Language::Bash => LexerSpec {
            line_comments: &["#"],
            string_delims: &['"', '\''],
            dollar_meta: true,
            keywords: &[
                "if", "then", "elif", "else", "fi", "for", "while", "until",
                "do", "done", "case", "esac", "in", "function", "select",
                "return", "break", "continue", "local", "export", "readonly",
                "declare", "set", "unset", "shift", "exit", "trap", "source",
                "alias", "eval", "exec",
            ],
            types: &["echo", "printf", "read", "cd", "pwd", "test"],
            ..LexerSpec::DEFAULT
        },
        Language::Sql => LexerSpec {
            line_comments: &["--"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['\''],
            ci_keywords: true,
            fn_calls: true,
            keywords: &[
                "select", "from", "where", "insert", "into", "values",
                "update", "set", "delete", "create", "table", "index",
                "view", "drop", "alter", "add", "primary", "key", "foreign",
                "references", "join", "inner", "left", "right", "full",
                "outer", "on", "as", "and", "or", "not", "null", "is", "in",
                "exists", "between", "like", "limit", "offset", "order",
                "by", "group", "having", "distinct", "union", "all", "case",
                "when", "then", "else", "end", "begin", "commit", "rollback",
                "transaction", "constraint", "unique", "default", "check",
                "if", "returning", "with",
            ],
            types: &[
                "int", "integer", "bigint", "smallint", "decimal", "numeric",
                "float", "real", "double", "varchar", "char", "text",
                "boolean", "date", "time", "timestamp", "timestamptz",
                "blob", "serial", "uuid", "json", "jsonb", "bytea",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Css => LexerSpec {
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            at_meta: true, // @media, @import, @keyframes …
            keywords: &[
                "color", "background", "background-color", "margin",
                "margin-top", "margin-right", "margin-bottom", "margin-left",
                "padding", "padding-top", "padding-right", "padding-bottom",
                "padding-left", "display", "position", "top", "right",
                "bottom", "left", "width", "height", "max-width",
                "max-height", "min-width", "min-height", "border",
                "border-radius", "font", "font-size", "font-family",
                "font-weight", "font-style", "line-height", "text-align",
                "text-decoration", "flex", "flex-direction", "align-items",
                "justify-content", "gap", "grid", "grid-template-columns",
                "grid-template-rows", "overflow", "overflow-x", "overflow-y",
                "opacity", "z-index", "cursor", "transition", "transform",
                "animation", "box-shadow", "outline", "content", "float",
                "clear", "visibility", "white-space", "vertical-align",
                "letter-spacing", "word-break", "box-sizing", "resize",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Java => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            caps_types: true,
            fn_calls: true,
            at_meta: true, // annotations
            keywords: &[
                "abstract", "assert", "break", "case", "catch", "class",
                "const", "continue", "default", "do", "else", "enum",
                "extends", "false", "final", "finally", "for", "goto", "if",
                "implements", "import", "instanceof", "interface", "native",
                "new", "null", "package", "permits", "private", "protected",
                "public", "record", "return", "sealed", "static", "strictfp",
                "super", "switch", "synchronized", "this", "throw", "throws",
                "transient", "true", "try", "var", "void", "volatile",
                "while", "yield",
            ],
            types: &[
                "boolean", "byte", "char", "double", "float", "int", "long",
                "short",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Kotlin => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            multiline_delims: &[], // ("""…""" handled by triple_quoted)
            triple_quoted: true,
            caps_types: true,
            fn_calls: true,
            at_meta: true,
            dollar_meta: true, // string templates outside quotes are rare, harmless
            keywords: &[
                "abstract", "actual", "annotation", "as", "break", "by",
                "catch", "class", "companion", "const", "constructor",
                "continue", "crossinline", "data", "do", "else", "enum",
                "expect", "external", "false", "final", "finally", "for",
                "fun", "get", "if", "import", "in", "infix", "init",
                "inline", "inner", "interface", "internal", "is", "lateinit",
                "noinline", "null", "object", "open", "operator", "out",
                "override", "package", "private", "protected", "public",
                "reified", "return", "sealed", "set", "super", "suspend",
                "tailrec", "this", "throw", "true", "try", "typealias",
                "val", "var", "vararg", "when", "where", "while",
            ],
            types: &[],
            ..LexerSpec::DEFAULT
        },
        Language::CSharp => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            caps_types: true,
            fn_calls: true,
            keywords: &[
                "abstract", "as", "async", "await", "base", "bool", "break",
                "byte", "case", "catch", "char", "checked", "class", "const",
                "continue", "decimal", "default", "delegate", "do", "double",
                "else", "enum", "event", "explicit", "extern", "false",
                "finally", "fixed", "float", "for", "foreach", "goto", "if",
                "implicit", "in", "int", "interface", "internal", "is",
                "lock", "long", "namespace", "new", "null", "object",
                "operator", "out", "override", "params", "private",
                "protected", "public", "readonly", "record", "ref", "return",
                "sbyte", "sealed", "short", "sizeof", "stackalloc", "static",
                "string", "struct", "switch", "this", "throw", "true", "try",
                "typeof", "uint", "ulong", "unchecked", "unsafe", "ushort",
                "using", "var", "virtual", "void", "volatile", "when",
                "where", "while", "yield",
            ],
            types: &[],
            ..LexerSpec::DEFAULT
        },
        Language::C => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            hash_meta: true, // #include, #define, …
            fn_calls: true,
            keywords: &[
                "auto", "break", "case", "char", "const", "continue",
                "default", "do", "double", "else", "enum", "extern", "float",
                "for", "goto", "if", "inline", "int", "long", "register",
                "restrict", "return", "short", "signed", "sizeof", "static",
                "struct", "switch", "typedef", "union", "unsigned", "void",
                "volatile", "while",
            ],
            types: &[
                "bool", "size_t", "ssize_t", "int8_t", "int16_t", "int32_t",
                "int64_t", "uint8_t", "uint16_t", "uint32_t", "uint64_t",
                "FILE", "NULL", "true", "false",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Cpp => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\''],
            hash_meta: true,
            fn_calls: true,
            keywords: &[
                // C subset
                "auto", "break", "case", "char", "const", "continue",
                "default", "do", "double", "else", "enum", "extern", "float",
                "for", "goto", "if", "inline", "int", "long", "register",
                "return", "short", "signed", "sizeof", "static", "struct",
                "switch", "typedef", "union", "unsigned", "void", "volatile",
                "while",
                // C++ additions
                "alignas", "alignof", "bool", "catch", "class", "concept",
                "constexpr", "consteval", "constinit", "const_cast",
                "decltype", "delete", "dynamic_cast", "explicit", "export",
                "false", "final", "friend", "mutable", "namespace", "new",
                "noexcept", "nullptr", "operator", "override", "private",
                "protected", "public", "reinterpret_cast", "requires",
                "static_assert", "static_cast", "template", "this",
                "thread_local", "throw", "true", "try", "typeid", "typename",
                "using", "virtual",
            ],
            types: &[
                "std", "string", "vector", "map", "set", "unordered_map",
                "unique_ptr", "shared_ptr", "size_t", "int32_t", "uint32_t",
                "int64_t", "uint64_t",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Go => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"', '\'', '`'],
            multiline_delims: &['`'], // raw strings
            fn_calls: true,
            keywords: &[
                "break", "case", "chan", "const", "continue", "default",
                "defer", "else", "fallthrough", "for", "func", "go", "goto",
                "if", "import", "interface", "map", "package", "range",
                "return", "select", "struct", "switch", "type", "var",
                "true", "false", "nil", "iota",
            ],
            types: &[
                "bool", "byte", "complex64", "complex128", "error",
                "float32", "float64", "int", "int8", "int16", "int32",
                "int64", "rune", "string", "uint", "uint8", "uint16",
                "uint32", "uint64", "uintptr", "any",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Dockerfile => LexerSpec {
            line_comments: &["#"],
            string_delims: &['"', '\''],
            dollar_meta: true,
            // Instructions are conventionally uppercase; include both
            // cases since the parser is position-blind.
            keywords: &[
                "FROM", "RUN", "CMD", "LABEL", "EXPOSE", "ENV", "ADD",
                "COPY", "ENTRYPOINT", "VOLUME", "USER", "WORKDIR", "ARG",
                "ONBUILD", "STOPSIGNAL", "HEALTHCHECK", "SHELL", "AS",
                "from", "run", "cmd", "label", "expose", "env", "add",
                "copy", "entrypoint", "volume", "user", "workdir", "arg",
                "as",
            ],
            types: &[],
            ..LexerSpec::DEFAULT
        },
        Language::Hcl => LexerSpec {
            line_comments: &["#", "//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"'],
            dollar_meta: true,
            fn_calls: true,
            keywords: &[
                "resource", "variable", "output", "module", "provider",
                "data", "locals", "terraform", "for_each", "count",
                "depends_on", "if", "for", "in", "true", "false", "null",
            ],
            types: &[
                "string", "number", "bool", "list", "map", "set", "object",
                "tuple",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Protobuf => LexerSpec {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            string_delims: &['"'],
            keywords: &[
                "syntax", "package", "import", "option", "message", "enum",
                "service", "rpc", "returns", "oneof", "map", "repeated",
                "optional", "required", "reserved", "extend", "extensions",
                "to", "weak", "public", "true", "false",
            ],
            types: &[
                "double", "float", "int32", "int64", "uint32", "uint64",
                "sint32", "sint64", "fixed32", "fixed64", "sfixed32",
                "sfixed64", "bool", "string", "bytes",
            ],
            ..LexerSpec::DEFAULT
        },
        Language::Html | Language::Yaml => LexerSpec::DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use crate::{highlight, Language, TokenKind};

    fn has(src: &str, lang: Language, text: &str, kind: TokenKind) -> bool {
        highlight(src, lang).iter().any(|t| t.text == text && t.kind == kind)
    }

    #[test]
    fn javascript_and_typescript() {
        assert!(has("const x = `a ${b}\nc`;", Language::JavaScript, "const", TokenKind::Keyword));
        assert!(has("const x = `a\nb`;", Language::JavaScript, "`a\nb`", TokenKind::String));
        assert!(has("interface Foo {}", Language::TypeScript, "interface", TokenKind::Keyword));
        assert!(has("let n: number = 1;", Language::TypeScript, "number", TokenKind::Type));
    }

    #[test]
    fn python() {
        assert!(has("def f():\n  # hi\n  return None", Language::Python, "def", TokenKind::Keyword));
        assert!(has("def f():\n  # hi\n  return None", Language::Python, "# hi", TokenKind::Comment));
        assert!(has("s = '''multi\nline'''", Language::Python, "'''multi\nline'''", TokenKind::String));
        assert!(has("@cached\ndef g(): pass", Language::Python, "@cached", TokenKind::Meta));
    }

    #[test]
    fn json_toml_bash() {
        assert!(has("{\"a\": true}", Language::Json, "true", TokenKind::Keyword));
        assert!(has("{\"a\": 1}", Language::Json, "\"a\"", TokenKind::String));
        assert!(has("# t\nkey = \"v\"", Language::Toml, "# t", TokenKind::Comment));
        assert!(has("if [ -z \"$HOME\" ]; then\nfi", Language::Bash, "if", TokenKind::Keyword));
        assert!(has("echo $HOME ${PATH}", Language::Bash, "$HOME", TokenKind::Meta));
        assert!(has("echo ${PATH}", Language::Bash, "${PATH}", TokenKind::Meta));
    }

    #[test]
    fn sql_case_insensitive_keywords() {
        assert!(has("select * from t;", Language::Sql, "select", TokenKind::Keyword));
        assert!(has("SELECT * FROM t;", Language::Sql, "SELECT", TokenKind::Keyword));
        assert!(has("-- note\nSELECT 1", Language::Sql, "-- note", TokenKind::Comment));
        assert!(has("WHERE name = 'bob'", Language::Sql, "'bob'", TokenKind::String));
    }

    #[test]
    fn jvm_and_dotnet() {
        assert!(has("public class Foo {}", Language::Java, "class", TokenKind::Keyword));
        assert!(has("@Override\nvoid f() {}", Language::Java, "@Override", TokenKind::Meta));
        assert!(has("val x: Int = 1", Language::Kotlin, "val", TokenKind::Keyword));
        assert!(has("using System;", Language::CSharp, "using", TokenKind::Keyword));
    }

    #[test]
    fn c_cpp_go() {
        assert!(has("#include <stdio.h>\nint main() {}", Language::C, "#include <stdio.h>", TokenKind::Meta));
        assert!(has("template<typename T> T f();", Language::Cpp, "template", TokenKind::Keyword));
        assert!(has("func main() { s := `raw\nstring` }", Language::Go, "func", TokenKind::Keyword));
        assert!(has("s := `raw\nstring`", Language::Go, "`raw\nstring`", TokenKind::String));
    }

    #[test]
    fn css_dockerfile_hcl_protobuf() {
        assert!(has("@media screen { color: red; }", Language::Css, "@media", TokenKind::Meta));
        assert!(has("a { color: red; }", Language::Css, "color", TokenKind::Keyword));
        assert!(has("FROM rust:1.79 AS builder", Language::Dockerfile, "FROM", TokenKind::Keyword));
        assert!(has("# comment\nRUN make", Language::Dockerfile, "# comment", TokenKind::Comment));
        assert!(has("resource \"aws_s3_bucket\" \"b\" {}", Language::Hcl, "resource", TokenKind::Keyword));
        // Interpolation sits INSIDE a quoted string, and strings win:
        // the whole quoted region is one String token. (`dollar_meta`
        // still colors interpolation in heredocs/unquoted contexts.)
        assert!(has("bucket = \"${var.name}\"", Language::Hcl, "\"${var.name}\"", TokenKind::String));
        assert!(has("message Foo { int32 id = 1; }", Language::Protobuf, "message", TokenKind::Keyword));
    }
}
