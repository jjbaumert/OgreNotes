// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! The one invariant everything depends on: `highlight` is a pure
//! partition of its input and never panics, for every language and
//! any input. If this fails, editor caret positions drift — do not
//! weaken it; fix the lexer.

use ogrenotes_highlight::{highlight, Language};
use proptest::prelude::*;

fn assert_partition(src: &str) {
    for lang in Language::ALL {
        let tokens = highlight(src, lang);
        let joined: String = tokens.iter().map(|t| t.text).collect();
        assert_eq!(joined, src, "partition violated for {lang:?} on {src:?}");
        // A zero-length token satisfies the concatenation check trivially
        // but desyncs the editor's caret walk (one DOM node per token —
        // see frontend/src/editor/view.rs). The full contract is
        // "non-empty partition".
        assert!(
            tokens.iter().all(|t| !t.text.is_empty()),
            "empty token emitted for {lang:?} on {src:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_unicode_partitions(src in "\\PC*") {
        assert_partition(&src);
    }

    #[test]
    fn code_shaped_input_partitions(
        src in r#"[ -~\n\t"'`#@$/*\\{}\[\]<>]{0,300}"#
    ) {
        assert_partition(&src);
    }
}

#[test]
fn tricky_fixed_cases_partition() {
    for src in [
        "",
        "\n\n\n",
        "\"",
        "'",
        "r#\"",
        "\\",
        "\"esc\\\"",
        "/*/",
        "// eof no newline",
        "#",
        "@",
        "$",
        "${unclosed",
        "\"\"\"",
        "1.",
        "1..2",
        "0xFFcafe_u32",
        "héllo wörld 世界 🎉",
        "a\u{0}b",       // NUL byte
        "\u{2028}line sep",
    ] {
        assert_partition(src);
    }
}
