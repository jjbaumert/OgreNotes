//! Heuristic text measurement — there is no DOM/canvas in pure Rust, so
//! widths come from a char-class table with generous padding downstream.

use unicode_normalization::char::{decompose_canonical, is_combining_mark};
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

fn literal_advance(grapheme: &str, font_size: f64, bold: bool) -> f64 {
    let weight = if bold { 1.04 } else { 1.0 };
    let mut em: f64 = 0.0;
    for ch in grapheme.chars() {
        decompose_canonical(ch, |base| {
            if is_combining_mark(base)
                || matches!(base, '\u{200c}' | '\u{200d}' | '\u{fe0e}' | '\u{fe0f}')
            {
                return;
            }
            let width = match base {
                'Ш' | 'Щ' | 'Ж' | 'Ю' | 'Ы' | 'Ф' | 'ш' | 'щ' | 'ж' | 'ю' | 'ы' | 'ф' | 'Æ'
                | 'æ' | 'Œ' | 'œ' | '…' => 1.05,
                // Advances for the renderer's sans-serif labels. Layout's
                // generously padded char_w table is unsuitable for fitting:
                // it needlessly drops suffixes from ordinary uppercase names.
                'i' | 'l' | 'j' | 'I' | ' ' => 0.28,
                'f' | 't' | '.' | ',' | ':' | ';' | '!' | '\'' | '`' => 0.30,
                'r' | '(' | ')' | '[' | ']' => 0.36,
                'J' | 'c' | 's' | 'v' | 'x' | 'y' | 'z' => 0.50,
                'E' => 0.67,
                'F' | 'L' | 'T' | 'Z' => 0.61,
                'M' | 'm' => 0.84,
                'W' | '@' | '%' => 0.95,
                'w' => 0.74,
                'A'..='Z' => 0.72,
                'a'..='z' | '0'..='9' => 0.56,
                _ => char_w(base),
            };
            em = em.max(width);
        });
    }
    em * font_size * weight
}

/// Width of literal text, without interpreting break markup.
pub(crate) fn literal_text_width(text: &str, font_size: f64, bold: bool) -> f64 {
    text.graphemes(true)
        .map(|g| literal_advance(g, font_size, bold))
        .sum()
}

/// Fit literal SVG text at its rendered font size, preserving grapheme clusters.
/// Accents use their base glyph's advance; wide letters and scripts get wider
/// estimates. Unlike text_size, <br> is literal text. Callers clip label paint
/// to its background because these advances cannot cover every browser font.
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
    if literal_text_width(text, font_size, bold) <= max_width + 1e-6 {
        return text.to_string();
    }
    let budget = max_width - advance("…");
    if budget < 0.0 {
        return String::new();
    }
    let (mut used, mut end) = (0.0, 0);
    for (offset, grapheme) in text.grapheme_indices(true) {
        let next = used + advance(grapheme);
        if next > budget {
            break;
        }
        used = next;
        end = offset + grapheme.len();
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

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
