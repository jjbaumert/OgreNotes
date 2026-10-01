//! Heuristic text measurement — there is no DOM/canvas in pure Rust, so
//! widths come from a char-class table with generous padding downstream.

use unicode_segmentation::UnicodeSegmentation;

pub(crate) const FONT_PX: f64 = 14.0;
pub(crate) const LINE_H: f64 = 19.0;

/// Relative advance width per char class, multiplied by FONT_PX.
fn char_w(c: char) -> f64 {
    match c {
        'i' | 'l' | 'j' | 't' | 'f' | 'r' | '.' | ',' | ':' | ';' | '!'
        | '|' | '\'' | '`' | ' ' | '(' | ')' | '[' | ']' => 0.45,
        'm' | 'w' | 'M' | 'W' | '@' | '%' => 0.95,
        'A'..='Z' | '0'..='9' | '#' | '&' | '$' => 0.72,
        c if (c as u32) > 0x2E7F => 1.05, // CJK & wide scripts
        _ => 0.58,
    }
}

fn split_br(s: &str) -> Vec<&str> {
    // Accept <br/>, <br>, <br /> — case-insensitive is overkill; mermaid
    // docs use lowercase. One pass: only a `<` can start a break, so the
    // cost is linear however many breaks there are (searching the rest of
    // the string for each spelling after every break was quadratic).
    const BREAKS: [&str; 3] = ["<br/>", "<br />", "<br>"];
    let mut out = Vec::new();
    let mut line_start = 0;
    let mut i = 0;
    while let Some(off) = s[i..].find('<') {
        let at = i + off;
        match BREAKS.iter().find(|t| s[at..].starts_with(*t)) {
            Some(t) => {
                out.push(&s[line_start..at]);
                line_start = at + t.len();
                i = line_start;
            }
            None => i = at + 1,
        }
    }
    out.push(&s[line_start..]);
    out
}

pub(crate) fn text_size(s: &str) -> (f64, f64) {
    let lines = split_br(s);
    let w = lines
        .iter()
        .map(|l| l.chars().map(char_w).sum::<f64>() * FONT_PX)
        .fold(0.0, f64::max);
    (w, lines.len() as f64 * LINE_H)
}

/// Lines after <br/> splitting — svg.rs emits one tspan per line.
pub(crate) fn lines(s: &str) -> Vec<&str> {
    split_br(s)
}

/// `s` cut down with a trailing ellipsis so it measures at most `max_w`
/// (unchanged if it already fits). Binary search over char boundaries:
/// re-measuring after dropping one char at a time was quadratic.
pub(crate) fn truncate_to_width(s: &str, max_w: f64) -> String {
    if text_size(s).0 <= max_w {
        return s.to_string();
    }
    let bounds: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    let fits = |n: usize| text_size(&format!("{}…", &s[..bounds[n]])).0 <= max_w;
    // Longest prefix of `n` chars (0 <= n < len) that fits with the ellipsis.
    let (mut lo, mut hi) = (0, bounds.len());
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if fits(mid) { lo = mid } else { hi = mid }
    }
    format!("{}…", &s[..bounds[lo]])
}

include!(concat!(env!("OUT_DIR"), "/compact_unicode.rs"));

fn is_combining_mark(ch: char) -> bool {
    let value = ch as u32;
    let index = COMBINING_RANGES.partition_point(|&range| (range >> 8) + (range & 255) < value);
    COMBINING_RANGES
        .get(index)
        .is_some_and(|&range| (range >> 8) <= value)
}

fn fitting_bases(ch: char, mut visit: impl FnMut(char)) {
    if ('\u{ac00}'..='\u{d7a3}').contains(&ch) {
        let index = ch as u32 - 0xac00;
        visit(char::from_u32(0x1100 + index / 588).unwrap());
        visit(char::from_u32(0x1161 + (index % 588) / 28).unwrap());
        let trailing = index % 28;
        if trailing != 0 {
            visit(char::from_u32(0x11a7 + trailing).unwrap());
        }
    } else if let Ok(index) = CANONICAL_ASCII.binary_search_by_key(&(ch as u32), |&base| base >> 7)
    {
        visit(char::from_u32(CANONICAL_ASCII[index] & 127).unwrap());
    } else if let Ok(index) = CANONICAL_BASES.binary_search_by_key(&ch, |&(base, _)| base) {
        CANONICAL_BASES[index].1.chars().for_each(visit);
    } else {
        visit(ch);
    }
}

include!("font_advances.rs");

fn base_advance(base: char) -> f64 {
    if !base.is_ascii() {
        if let Ok(index) = FONT_ADVANCES.binary_search_by_key(&(base as u32), |packed| packed >> 11)
        {
            return (FONT_ADVANCES[index] & 0x7ff) as f64 / 1024.0;
        }
    }
    match base {
        '\u{1100}'..='\u{11ff}' | '\u{a960}'..='\u{a97f}' | '\u{d7b0}'..='\u{d7ff}' => 1.05,
        'М' | 'Ш' | 'Щ' | 'Ж' | 'Ю' | 'Ы' | 'Ф' | 'ш' | 'щ' | 'ж' | 'ю' | 'ы' | 'ф' | 'Æ' | 'æ'
        | 'Œ' | 'œ' | '…' => 1.05,
        // Advances for the renderer's sans-serif labels. Layout's
        // generously padded char_w table is unsuitable for fitting:
        // it needlessly drops suffixes from ordinary uppercase names.
        'i' | 'l' | 'j' | 'I' | ' ' => 0.28,
        'f' => 0.45,
        't' => 0.40,
        '.' | ',' | ':' | ';' | '!' | '\'' | '`' => 0.35,
        'r' | '(' | ')' | '[' | ']' => 0.43,
        'J' | 'c' | 's' | 'v' | 'x' | 'y' | 'z' => 0.61,
        'E' => 0.67,
        'F' | 'L' | 'T' | 'Z' => 0.61,
        'M' | 'm' => 0.95,
        'W' | '@' | '%' => 1.00,
        'w' => 0.85,
        'A'..='Z' => 0.72,
        'a'..='z' | '0'..='9' => 0.60,
        // Joined Arabic letters have narrower advances than the full-em
        // fallback. Keep narrow stems distinct from wide seen/sad families.
        'ا' | 'ل' => 0.35,
        'د' | 'ذ' | 'ر' | 'ز' | 'و' => 0.55,
        'س' | 'ش' | 'ص' | 'ض' => 1.05,
        'ط' | 'ظ' | 'ك' => 0.90,
        'ب' | 'ت' | 'ث' | 'ة' | 'ع' | 'غ' | 'م' | 'ن' | 'ه' | 'ي' => 0.65,
        '\u{0600}'..='\u{06ff}' | '\u{0750}'..='\u{077f}' | '\u{08a0}'..='\u{08ff}' => 0.75,
        '\u{1f000}'..='\u{1faff}' => 1.35,
        // Fallback fonts need a conservative full-em estimate. The
        // layout table's Latin average undercounts Cyrillic/Greek.
        c if !c.is_ascii() => 1.05,
        _ => char_w(base),
    }
}

// Share the Unicode lookup path instead of duplicating it at fitting sites.
#[inline(never)]
fn literal_advance(grapheme: &str, font_size: f64, bold: bool) -> f64 {
    let weight = if bold { 1.04 } else { 1.0 };
    // Ordinary grapheme clusters can contain multiple advancing letters
    // (for example Indic conjuncts). Hangul jamo and joined emoji shape
    // into a single glyph instead of adding each component's advance.
    let single_glyph = grapheme.chars().next().is_some_and(|ch| {
            matches!(ch, '\u{1100}'..='\u{11ff}' | '\u{ac00}'..='\u{d7a3}' | '\u{1f000}'..='\u{1faff}')
        });
    let mut em: f64 = 0.0;
    for ch in grapheme.chars() {
        fitting_bases(ch, |base| {
            if is_combining_mark(base)
                || matches!(base, '\u{200c}' | '\u{200d}' | '\u{fe0e}' | '\u{fe0f}')
            {
                return;
            }
            let width = base_advance(base);
            em = if single_glyph {
                em.max(width)
            } else {
                em + width
            };
        });
    }
    em * font_size * weight
}

/// Conservative literal glyph bounds for overlap detection, without HTML breaks.
/// The allowance covers fallback fonts; it changes marker presentation, not layout.
pub(crate) fn literal_overlap_width(text: &str, font_size: f64) -> f64 {
    text.graphemes(true)
        .map(|g| literal_advance(g, font_size, false))
        .sum::<f64>()
        * 1.3
}

/// Fit literal SVG text at its rendered font size, preserving grapheme clusters.
/// Accents use their base glyph's advance; wide letters and scripts get wider
/// estimates. Unlike text_size, <br> is literal text. Callers back label glyphs with palette halos
/// because these advances cannot cover every browser font.
pub(crate) fn truncate_literal_to_width(
    text: &str,
    max_width: f64,
    font_size: f64,
    bold: bool,
) -> String {
    if text.is_empty() || max_width <= 0.0 {
        return String::new();
    }
    let advance = |grapheme: &str| literal_advance(grapheme, font_size, bold);
    let budget = max_width - advance("…");
    let (mut total, mut end, mut fitting) = (0.0, 0, true);
    // Measure the full label and its fitting prefix together. Keep checking
    // the full width after the prefix fills: an untruncated label may fit
    // even when there is no room for the additional ellipsis.
    for grapheme in text.graphemes(true) {
        total += advance(grapheme);
        if fitting && total <= budget {
            end += grapheme.len();
        } else {
            fitting = false;
        }
    }
    if total <= max_width + 1e-6 {
        return text.to_string();
    }
    if budget < 0.0 {
        return String::new();
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arabic_names_that_fit_remain_complete() {
        assert_eq!(
            truncate_literal_to_width("السلام عليكم", 95.6, 13.0, true),
            "السلام عليكم"
        );
        let wide = "س".repeat(40);
        let fitted = truncate_literal_to_width(&wide, 95.6, 13.0, true);
        assert!(fitted.ends_with('…'));
        assert!(fitted.len() < wide.len());
    }

    #[test]
    fn compact_fitting_data_matches_full_unicode_normalization() {
        use unicode_normalization::char::{decompose_canonical, is_combining_mark as full_mark};

        fn reference(text: &str) -> f64 {
            let single = text.chars().next().is_some_and(|ch| {
                matches!(ch, '\u{1100}'..='\u{11ff}' | '\u{ac00}'..='\u{d7a3}' | '\u{1f000}'..='\u{1faff}')
            });
            let mut width: f64 = 0.0;
            for ch in text.chars() {
                decompose_canonical(ch, |base| {
                    if !full_mark(base)
                        && !matches!(base, '\u{200c}' | '\u{200d}' | '\u{fe0e}' | '\u{fe0f}')
                    {
                        let advance = base_advance(base);
                        width = if single {
                            width.max(advance)
                        } else {
                            width + advance
                        };
                    }
                });
            }
            width
        }

        for value in 0..=0x10ffff {
            let Some(ch) = char::from_u32(value) else {
                continue;
            };
            assert_eq!(is_combining_mark(ch), full_mark(ch), "mark U+{value:04X}");
            // A prepended character also exercises decomposed Hangul when
            // it is not the first character of the grapheme cluster.
            for text in [ch.to_string(), format!("\u{0600}{ch}")] {
                assert_eq!(
                    literal_advance(&text, 1.0, false),
                    reference(&text),
                    "advance U+{value:04X}"
                );
            }
        }
    }

    #[test]
    fn wider_text_measures_wider() {
        assert!(text_size("wide text here").0 > text_size("hi").0);
    }

    #[test]
    fn narrow_chars_narrower_than_wide() {
        // 4 narrow chars vs 4 normal-width chars.
        assert!(text_size("ilil").0 < text_size("wood").0);
    }

    #[test]
    fn cjk_wider_than_ascii() {
        assert!(text_size("图表").0 > text_size("ab").0);
    }

    #[test]
    fn br_splits_lines() {
        let (w1, h1) = text_size("hello world");
        let (w2, h2) = text_size("hello<br/>world");
        assert!(h2 > h1);
        assert!(w2 < w1);
        assert_eq!(h2, 2.0 * LINE_H);
        // <br> and <br /> variants also split.
        assert_eq!(text_size("a<br>b").1, 2.0 * LINE_H);
        assert_eq!(text_size("a<br />b").1, 2.0 * LINE_H);
    }

    #[test]
    fn br_splitting_is_exact_and_linear() {
        assert_eq!(lines("a<br/>b<br />c<br>d"), ["a", "b", "c", "d"]);
        assert_eq!(lines("<br>"), ["", ""]);
        assert_eq!(lines("a < b <b> <br"), ["a < b <b> <br"]);
        // ~20k chars of breaks: formerly ~1.5s, now instant.
        let many = "<br>".repeat(5000);
        let started = std::time::Instant::now();
        assert_eq!(lines(&many).len(), 5001);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn truncation_fits_and_keeps_char_boundaries() {
        assert_eq!(truncate_to_width("short", 1000.0), "short");
        for s in ["a fairly long label that will not fit", "图表图表图表图表图表图表"] {
            let t = truncate_to_width(s, 60.0);
            assert!(t.ends_with('…') && t.len() < s.len() + 3, "{t}");
            assert!(text_size(&t).0 <= 60.0, "{t}");
            // The next longer prefix would not have fit.
            let kept = t.trim_end_matches('…').chars().count();
            let longer: String = s.chars().take(kept + 1).collect();
            assert!(text_size(&format!("{longer}…")).0 > 60.0, "{t}");
        }
        let long = "x".repeat(19_000);
        let started = std::time::Instant::now();
        truncate_to_width(&long, 100.0);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn empty_is_one_line_high() {
        assert_eq!(text_size(""), (0.0, LINE_H));
    }
}
