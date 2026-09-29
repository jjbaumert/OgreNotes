// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

#![forbid(unsafe_code)]

//! LaTeX math → MathML Core.
//!
//! [`to_mathml`] turns the math-mode LaTeX people type into equations
//! (`\frac{a}{b}`, `x^2`, `\sum_{i=1}^n`, `\begin{pmatrix}…`) into a
//! `<math>` element that browsers lay out natively, so the editor and
//! server-side HTML export render the same markup with no JavaScript or
//! font files. Pure `std`; compiles to wasm32.
//!
//! Contract (shared with `ogrenotes-mermaid`):
//! - never panics, whatever the input;
//! - every failure is an error naming what went wrong — unknown commands
//!   and unsupported syntax are never dropped or rendered literally;
//! - all source text reaching the output is XML-escaped, and attribute
//!   values come from fixed tables (colors are validated), so the output
//!   is safe to insert as HTML;
//! - the original LaTeX rides along as an `application/x-tex`
//!   annotation, for copy/paste and assistive technology.

mod fonts;
mod mathml;
mod parse;
mod symbols;

/// Longest equation source accepted, in characters. Shared with the
/// write-gate validator (`crates/collab`) and the editor, so client and
/// server agree on what can be saved.
pub const MAX_SOURCE_LEN: usize = 10_000;

/// A parse failure: what went wrong and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MathError {
    pub message: String,
    /// Character offset into the source where the problem was found.
    pub offset: usize,
}

impl std::fmt::Display for MathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (at character {})", self.message, self.offset + 1)
    }
}

impl std::error::Error for MathError {}

/// Display equations sit on their own line in display style; inline
/// equations flow with the text in the more compact text style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Block,
    Inline,
}

/// Render LaTeX math source to a `<math>` element.
pub fn to_mathml(source: &str, display: Display) -> Result<String, MathError> {
    if source.chars().count() > MAX_SOURCE_LEN {
        return Err(MathError {
            message: format!("equation too long (max {MAX_SOURCE_LEN} characters)"),
            offset: MAX_SOURCE_LEN,
        });
    }
    if source.trim().is_empty() {
        return Err(MathError { message: "equation is empty".into(), offset: 0 });
    }
    let body = parse::Parser::new(source).parse()?;
    // `<semantics>` takes exactly one presentation child.
    let body = match body {
        n @ mathml::Node::El { tag: "mrow", .. } => n,
        n => mathml::Node::el("mrow", vec![n]),
    };

    let mut out = String::with_capacity(source.len() * 12 + 160);
    out.push_str(r#"<math xmlns="http://www.w3.org/1998/Math/MathML" display=""#);
    out.push_str(match display {
        Display::Block => "block",
        Display::Inline => "inline",
    });
    out.push_str(r#""><semantics>"#);
    body.write(&mut out);
    out.push_str(r#"<annotation encoding="application/x-tex">"#);
    mathml::escape_into(&mut out, source);
    out.push_str("</annotation></semantics></math>");
    Ok(out)
}

#[cfg(test)]
mod tests;
