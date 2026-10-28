//! Shared source decoration handling, preserving original error line numbers.

use crate::{escape_xml, measure, ParseError};

pub(crate) struct Prepared {
    pub source: String,
    pub title: Option<String>,
}

pub(crate) fn prepare(source: &str) -> Result<Prepared, ParseError> {
    let lines: Vec<_> = source.lines().collect();
    let mut output = lines.clone();
    let mut title = None;
    let mut start = 0;
    if lines.first().is_some_and(|line| line.trim() == "---") {
        let close = lines
            .iter()
            .skip(1)
            .position(|line| line.trim() == "---")
            .map(|index| index + 1)
            .ok_or_else(|| error("unterminated front matter: missing closing `---`", 1))?;
        for (index, line) in lines.iter().enumerate().take(close).skip(1) {
            if let Some(value) = line.strip_prefix("title:") {
                if title.is_some() {
                    return Err(error("duplicate front matter title", index + 1));
                }
                let decoded = title_scalar(value.trim(), index + 1)?;
                if decoded.chars().any(|character| {
                    !matches!(character, '\t' | '\n' | '\r')
                        && (character < '\u{20}' || matches!(character, '\u{fffe}' | '\u{ffff}'))
                }) {
                    return Err(error(
                        "front matter title contains a character forbidden in XML",
                        index + 1,
                    ));
                }
                title = Some(
                    decoded
                        .replace("\r\n", "\n")
                        .replace(['\r', '\u{85}', '\u{2028}', '\u{2029}'], "\n"),
                );
            }
        }
        output[..=close].fill("");
        start = close + 1;
    }
    // Configuration directives are comments for this renderer, just as their
    // single-line spelling has always been. Consume their entire comment so
    // embedded JSON cannot be mistaken for a diagram header or node.
    let mut directive = None;
    for (index, line) in lines.iter().enumerate().skip(start) {
        if directive.is_some() || line.trim_start().starts_with("%%{") {
            directive.get_or_insert(index + 1);
            output[index] = "";
            if line.trim_end().ends_with("}%%") {
                directive = None;
            }
        }
    }
    if let Some(line) = directive {
        return Err(error(
            "unterminated configuration comment: missing `}%%`",
            line,
        ));
    }
    Ok(Prepared {
        source: output.join("\n"),
        title,
    })
}

fn error(message: &str, line: usize) -> ParseError {
    ParseError {
        message: message.into(),
        line: Some(line),
    }
}

/// Decode the single-line YAML scalar subset used for Mermaid titles. Other
/// front-matter configuration remains deliberately ignored. Invalid or complex
/// title values fail instead of rendering a different string from the source.
fn title_scalar(value: &str, line: usize) -> Result<String, ParseError> {
    let Some(quote @ ('\'' | '"')) = value.chars().next() else {
        let end = value
            .char_indices()
            .find_map(|(index, ch)| {
                (ch == '#' && (index == 0 || value[..index].ends_with(char::is_whitespace)))
                    .then_some(index)
            })
            .unwrap_or(value.len());
        let value = value[..end].trim_end();
        if value.starts_with(['|', '>', '[', '{', '!', '&', '*']) || value.contains(": ") {
            return Err(error(
                "front matter title must be a single-line string",
                line,
            ));
        }
        return Ok(value.to_string());
    };
    let mut chars = value[quote.len_utf8()..].chars().peekable();
    let mut decoded = String::new();
    while let Some(ch) = chars.next() {
        if ch == quote {
            if quote == '\'' && chars.peek() == Some(&'\'') {
                chars.next();
                decoded.push('\'');
                continue;
            }
            let rest: String = chars.collect();
            if !rest.trim_start().is_empty() && !rest.trim_start().starts_with('#') {
                return Err(error("unexpected text after front matter title", line));
            }
            return Ok(decoded);
        }
        if quote != '"' || ch != '\\' {
            decoded.push(ch);
            continue;
        }
        let escaped = match chars.next() {
            Some('0') => '\0',
            Some('a') => '\u{7}',
            Some('b') => '\u{8}',
            Some('t' | '\t') => '\t',
            Some('n') => '\n',
            Some('v') => '\u{b}',
            Some('f') => '\u{c}',
            Some('r') => '\r',
            Some('e') => '\u{1b}',
            Some(' ') => ' ',
            Some('"') => '"',
            Some('/') => '/',
            Some('\\') => '\\',
            Some('N') => '\u{85}',
            Some('_') => '\u{a0}',
            Some('L') => '\u{2028}',
            Some('P') => '\u{2029}',
            Some(kind @ ('x' | 'u' | 'U')) => {
                let digits = match kind {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let mut scalar = 0u32;
                for _ in 0..digits {
                    let digit = chars.next().and_then(|ch| ch.to_digit(16)).ok_or_else(|| {
                        error("invalid Unicode escape in front matter title", line)
                    })?;
                    scalar = scalar
                        .checked_mul(16)
                        .and_then(|number| number.checked_add(digit))
                        .ok_or_else(|| {
                            error("invalid Unicode escape in front matter title", line)
                        })?;
                }
                char::from_u32(scalar)
                    .ok_or_else(|| error("invalid Unicode escape in front matter title", line))?
            }
            _ => return Err(error("invalid escape in front matter title", line)),
        };
        decoded.push(escaped);
    }
    Err(error("unterminated front matter title", line))
}

/// Reserve a real title band rather than drawing a title over the diagram.
/// A negative viewBox origin keeps all existing diagram geometry unchanged.
pub(crate) fn with_title(svg: String, title: &str) -> String {
    if title.is_empty() {
        return svg;
    }
    let Some(vb_start) = svg.find("viewBox=\"").map(|at| at + 9) else {
        return svg;
    };
    let Some(vb_end) = svg[vb_start..].find('"').map(|at| at + vb_start) else {
        return svg;
    };
    let values: Vec<f64> = svg[vb_start..vb_end]
        .split_whitespace()
        .filter_map(|value| value.parse().ok())
        .collect();
    let [x, y, width, height] = values.as_slice() else {
        return svg;
    };
    let normalized = title.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<String> = normalized
        .split(['\n', '\u{0085}', '\u{2028}', '\u{2029}'])
        .flat_map(|line| {
            measure::wrap_literal_to_width(line, (width - 40.0).max(80.0), measure::FONT_PX, true)
        })
        .collect();
    let band = 20.0 + lines.len() as f64 * measure::LINE_H;
    let center = x + width / 2.0;
    let canvas_width = width.max(120.0);
    let canvas_x = center - canvas_width / 2.0;
    let mut out = svg[..vb_start].to_string();
    out.push_str(&format!(
        "{canvas_x} {} {canvas_width} {}",
        y - band,
        height + band
    ));
    out.push_str(&svg[vb_end..]);
    if let Some(at) = out.find("width=\"").map(|at| at + 7) {
        if let Some(end) = out[at..].find('"').map(|end| end + at) {
            out.replace_range(at..end, &format!("{canvas_width}"));
        }
    }
    if let Some(at) = out.find("height=\"").map(|at| at + 8) {
        if let Some(end) = out[at..].find('"').map(|end| end + at) {
            out.replace_range(at..end, &format!("{}", height + band));
        }
    }
    let mut heading = String::new();
    for (index, line) in lines.iter().enumerate() {
        heading.push_str(&format!(
            r#"<text xml:space="preserve" x="{center:.1}" y="{:.1}" text-anchor="middle" font-size="14" font-weight="600" fill="currentColor">{}</text>"#,
            y - band + 17.0 + index as f64 * measure::LINE_H, escape_xml(line)
        ));
    }
    if let Some(end) = out.find('>') {
        out.insert_str(end + 1, &heading);
    }
    out
}
