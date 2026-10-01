// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::{env, fmt::Write, fs, path::PathBuf};
use unicode_normalization::char::{decompose_canonical, is_combining_mark};

// Fitting needs combining-mark membership and canonical bases with differing
// advances, rather than the complete Unicode normalization engine in WASM.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let mut mappings = String::from("const CANONICAL_BASES: &[(char, &str)] = &[\n");
    let mut ranges = String::from("const COMBINING_RANGES: &[(u32, u32)] = &[\n");
    let mut mark_range: Option<(u32, u32)> = None;
    for value in 0..=0x10ffff {
        let ch = char::from_u32(value);
        if ch.is_some_and(is_combining_mark) {
            mark_range = Some((mark_range.map_or(value, |(start, _)| start), value));
        } else if let Some((start, end)) = mark_range.take() {
            writeln!(ranges, "(0x{start:x}, 0x{end:x}),").unwrap();
        }
        let Some(ch) = ch else { continue };
        // Hangul decomposition is algorithmic and stays algorithmic at runtime.
        if ('\u{ac00}'..='\u{d7a3}').contains(&ch) {
            continue;
        }
        let mut bases = Vec::new();
        decompose_canonical(ch, |base| {
            if !is_combining_mark(base) && !matches!(base, '\u{200c}' | '\u{200d}') {
                bases.push(base);
            }
        });
        if bases == [ch] || (bases.is_empty() && is_combining_mark(ch)) {
            continue;
        }
        // All other non-ASCII bases have a full-em advance, except emoji.
        // Changing one to another therefore needs no runtime mapping.
        let full_em = |c: char| {
            !c.is_ascii()
                && !is_combining_mark(c)
                && !matches!(c, '\u{200c}' | '\u{200d}' | '\u{1f000}'..='\u{1faff}')
        };
        if bases.len() == 1 && full_em(ch) && full_em(bases[0]) {
            continue;
        }
        let escaped: String = bases
            .iter()
            .map(|c| format!("\\u{{{:x}}}", *c as u32))
            .collect();
        writeln!(mappings, "('\\u{{{value:x}}}', \"{escaped}\"),").unwrap();
    }
    if let Some((start, end)) = mark_range {
        writeln!(ranges, "(0x{start:x}, 0x{end:x}),").unwrap();
    }
    mappings.push_str("];\n");
    ranges.push_str("];\n");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("compact_unicode.rs");
    fs::write(
        output,
        format!("// Generated from unicode-normalization data.\n{mappings}{ranges}"),
    )
    .unwrap();
}
