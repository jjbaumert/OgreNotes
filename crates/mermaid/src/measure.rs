//! Heuristic text measurement — there is no DOM/canvas in pure Rust, so
//! widths come from a char-class table with generous padding downstream.

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
    // docs use lowercase.
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        let hit = ["<br/>", "<br />", "<br>"]
            .iter()
            .filter_map(|t| rest.find(t).map(|i| (i, t.len())))
            .min();
        match hit {
            Some((i, tl)) => {
                out.push(&rest[..i]);
                rest = &rest[i + tl..];
            }
            None => {
                out.push(rest);
                return out;
            }
        }
    }
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
    fn empty_is_one_line_high() {
        assert_eq!(text_size(""), (0.0, LINE_H));
    }
}
