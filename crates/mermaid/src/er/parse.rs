//! ER-diagram parser. Line-oriented, mirrors `class::parse` and
//! `state::parse`: `err()` helper, ASCII id scan with byte-safety
//! comment, `split_once(':')` for labels, exact-match guards for
//! keyword/brace statements, an attribute-block state shaped like
//! class's single-slot member block (nesting one level, opening line
//! tracked, bare `}` closes).
//!
//! Relationship-token strategy: the `:` label split happens FIRST on
//! the original statement (same reasoning as class's operator scan) so
//! label text can never be mistaken for grammar. The pre-colon portion
//! must then be exactly three whitespace-separated tokens: `id token
//! id`. The middle token is the relationship token; per the brief it is
//! validated `is_ascii()` UP FRONT (before any slicing) and errors
//! naming the token otherwise, then sliced as `2 + 2 + 2` bytes (left
//! cardinality symbol, `--`/`..`, right cardinality symbol) — safe
//! because ASCII guarantees every byte offset is a char boundary.

use crate::er::{Cardinality, Entity, ErAttribute, ErGraph, ErRelation};
use crate::ParseError;
use std::collections::HashMap;

/// Maps a 2-char cardinality symbol to its `Cardinality`, independent of
/// which side (left/right) it appears on — the brief's normalization
/// table.
fn symbol_cardinality(sym: &str) -> Option<Cardinality> {
    match sym {
        "||" => Some(Cardinality::ExactlyOne),
        "|o" | "o|" => Some(Cardinality::ZeroOrOne),
        "}|" | "|{" => Some(Cardinality::OneOrMore),
        "}o" | "o{" => Some(Cardinality::ZeroOrMore),
        _ => None,
    }
}

/// Parses one relationship token (e.g. `||--o{`) into
/// `(card_from, identifying, card_to)`. `None` means invalid; the
/// caller attaches the token text and source line to the error.
fn parse_relationship_token(tok: &str) -> Option<(Cardinality, bool, Cardinality)> {
    // Validate ASCII up front: guarantees every byte index below is a
    // char boundary, so the slices that follow can never panic or split
    // a multibyte codepoint.
    if !tok.is_ascii() || tok.len() != 6 {
        return None;
    }
    let left = &tok[0..2];
    let mid = &tok[2..4];
    let right = &tok[4..6];
    let identifying = match mid {
        "--" => true,
        ".." => false,
        _ => return None,
    };
    let card_from = symbol_cardinality(left)?;
    let card_to = symbol_cardinality(right)?;
    Some((card_from, identifying, card_to))
}

struct Parser {
    g: ErGraph,
    ids: HashMap<String, usize>,
    line: usize, // 1-based, for errors
    /// Open attribute block: (entity index, opening line). ER blocks
    /// don't nest, so a single slot suffices — like class's member
    /// block.
    block: Option<(usize, usize)>,
}

pub(crate) fn parse(source: &str) -> Result<ErGraph, ParseError> {
    let mut p = Parser {
        g: ErGraph { entities: vec![], relations: vec![] },
        ids: HashMap::new(),
        line: 0,
        block: None,
    };
    let mut seen_header = false;
    for (idx, raw) in source.lines().enumerate() {
        p.line = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if !seen_header {
            let header = line.strip_suffix(';').unwrap_or(line).trim_end();
            if header != "erDiagram" {
                return Err(p.err("ER diagram must start with `erDiagram`"));
            }
            seen_header = true;
            continue;
        }
        if p.block.is_some() {
            p.handle_block_line(line)?;
        } else {
            p.parse_statement(line)?;
        }
    }
    if !seen_header {
        return Err(ParseError {
            message: "ER diagram must start with `erDiagram`".into(),
            line: Some(1),
        });
    }
    if let Some((idx, opening_line)) = p.block {
        return Err(ParseError {
            message: format!("unclosed attribute block `{}`", p.g.entities[idx].id),
            line: Some(opening_line),
        });
    }
    Ok(p.g)
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { message: msg.into(), line: Some(self.line) }
    }

    /// Top-level (non-block) statement: `ENTITY {` opens an attribute
    /// block, `ENTITY [...]` (alias) and `ENTITY` bare are handled
    /// here, everything else is a relationship.
    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        // ASCII id scan: char count == byte length only because the
        // predicate is ASCII-only; do not relax without a byte-position
        // scan.
        let id_len = stmt.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if id_len == 0 {
            return Err(self.err(format!("expected an entity id, found {stmt:?}")));
        }
        let id = &stmt[..id_len];
        let after = stmt[id_len..].trim_start();
        if let Some(rest) = after.strip_prefix('{') {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Err(self.err(format!("unexpected text after `{{`: {rest:?}")));
            }
            let idx = self.ensure_entity(id)?;
            self.block = Some((idx, self.line));
            return Ok(());
        }
        if after.starts_with('[') {
            return Err(self.err("entity aliases (`ENTITY [\"alias\"]`) are not supported"));
        }
        if after.is_empty() {
            self.ensure_entity(id)?;
            return Ok(());
        }
        self.parse_relationship(stmt)
    }

    /// `A <lcard><line><rcard> B : label` — the pre-colon portion must
    /// be exactly three whitespace-separated tokens; the label after
    /// `:` is required by the grammar.
    fn parse_relationship(&mut self, stmt: &str) -> Result<(), ParseError> {
        let (before, after) = match stmt.split_once(':') {
            Some((b, a)) => (b, Some(a)),
            None => (stmt, None),
        };
        let tokens: Vec<&str> = before.split_whitespace().collect();
        if tokens.len() != 3 {
            return Err(self.err(format!("expected `A <cardinality> B`, found {before:?}")));
        }
        let (left_id, card_tok, right_id) = (tokens[0], tokens[1], tokens[2]);
        self.validate_id(left_id)?;
        self.validate_id(right_id)?;
        let Some((card_from, identifying, card_to)) = parse_relationship_token(card_tok) else {
            return Err(self.err(format!("invalid relationship token {card_tok:?}")));
        };
        let label = match after.map(str::trim) {
            Some(l) if !l.is_empty() => l.to_string(),
            _ => return Err(self.err("relationship needs a `: label`")),
        };
        let from = self.ensure_entity(left_id)?;
        let to = self.ensure_entity(right_id)?;
        self.push_relation(ErRelation { from, to, card_from, card_to, identifying, label })
    }

    /// One line inside an open attribute block: bare `}` closes it;
    /// anything else is an attribute row.
    fn handle_block_line(&mut self, line: &str) -> Result<(), ParseError> {
        if line == "}" {
            self.block = None;
            return Ok(());
        }
        let attr = self.parse_attribute_row(line)?;
        let (idx, _) = self.block.expect("handle_block_line only called while a block is open");
        self.g.entities[idx].attributes.push(attr);
        Ok(())
    }

    /// `type name [PK|FK]` — 2 or 3 whitespace-separated tokens.
    /// `type`/`name` are stored VERBATIM (free text, not id-validated —
    /// the svg renderer escapes them; only entity ids get the
    /// `[A-Za-z0-9_]+` check). A 3rd token other than `PK`/`FK` errors
    /// naming it (`UK` gets its own message naming `UK`); a trailing
    /// quoted comment errors mentioning "comment"; more than 3 tokens
    /// errors.
    fn parse_attribute_row(&self, line: &str) -> Result<ErAttribute, ParseError> {
        let mut tokens = line.split_whitespace();
        let (Some(ty), Some(name)) = (tokens.next(), tokens.next()) else {
            return Err(self.err(format!("expected `type name` attribute row, found {line:?}")));
        };
        let rest: Vec<&str> = tokens.collect();
        let key = match rest.as_slice() {
            [] => None,
            [tok] => match *tok {
                "PK" | "FK" => Some((*tok).to_string()),
                "UK" => return Err(self.err("`UK` (unique key marker) is not supported")),
                other if other.starts_with('"') => {
                    return Err(self.err("attribute comments are not supported"));
                }
                other => return Err(self.err(format!("unsupported attribute key marker {other:?}"))),
            },
            [first, ..] if first.starts_with('"') => {
                return Err(self.err("attribute comments are not supported"));
            }
            _ => return Err(self.err(format!("too many tokens in attribute row: {line:?}"))),
        };
        Ok(ErAttribute { ty: ty.to_string(), name: name.to_string(), key })
    }

    fn validate_id(&self, id: &str) -> Result<(), ParseError> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(self.err(format!("invalid entity id {id:?}")));
        }
        Ok(())
    }

    /// Look up an existing entity by id, or create it (implicit
    /// creation on first reference, whether from a block-open or a
    /// relationship endpoint).
    fn ensure_entity(&mut self, id: &str) -> Result<usize, ParseError> {
        self.validate_id(id)?;
        if let Some(&i) = self.ids.get(id) {
            return Ok(i);
        }
        let idx = self.g.entities.len();
        self.g.entities.push(Entity { id: id.to_string(), attributes: vec![] });
        self.ids.insert(id.to_string(), idx);
        if self.g.entities.len() > crate::layout::MAX_NODES {
            return Err(self.err(format!(
                "diagram too large: too many entities (max {})",
                crate::layout::MAX_NODES
            )));
        }
        Ok(idx)
    }

    fn push_relation(&mut self, rel: ErRelation) -> Result<(), ParseError> {
        self.g.relations.push(rel);
        if self.g.relations.len() > crate::layout::MAX_EDGES {
            return Err(self.err(format!(
                "diagram too large: too many relations (max {})",
                crate::layout::MAX_EDGES
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::er::{Cardinality, ErGraph};

    fn p(src: &str) -> ErGraph {
        parse(src).expect("parse ok")
    }

    #[test]
    fn header_required() {
        assert!(parse("erDiagram\nA ||--o{ B : has").is_ok());
        assert_eq!(parse("A ||--o{ B : has").unwrap_err().line, Some(1));
    }

    #[test]
    fn attribute_blocks() {
        let g = p("erDiagram\nCUSTOMER {\nstring name\nint id PK\nint org_id FK\n}");
        let c = &g.entities[0];
        assert_eq!(c.attributes.len(), 3);
        assert_eq!(c.attributes[0].ty, "string");
        assert_eq!(c.attributes[0].name, "name");
        assert_eq!(c.attributes[0].key, None);
        assert_eq!(c.attributes[1].key.as_deref(), Some("PK"));
        assert_eq!(c.attributes[2].key.as_deref(), Some("FK"));
    }

    #[test]
    fn all_cardinality_symbols() {
        let cases = [
            ("A ||--|| B : r", Cardinality::ExactlyOne, Cardinality::ExactlyOne),
            ("A |o--o| B : r", Cardinality::ZeroOrOne, Cardinality::ZeroOrOne),
            ("A }|--|{ B : r", Cardinality::OneOrMore, Cardinality::OneOrMore),
            ("A }o--o{ B : r", Cardinality::ZeroOrMore, Cardinality::ZeroOrMore),
            ("A ||--o{ B : r", Cardinality::ExactlyOne, Cardinality::ZeroOrMore),
        ];
        for (src, want_from, want_to) in cases {
            let g = p(&format!("erDiagram\n{src}"));
            assert_eq!(g.relations[0].card_from, want_from, "for {src}");
            assert_eq!(g.relations[0].card_to, want_to, "for {src}");
        }
    }

    #[test]
    fn identifying_vs_non() {
        assert!(p("erDiagram\nA ||--|| B : r").relations[0].identifying);
        assert!(!p("erDiagram\nA ||..|| B : r").relations[0].identifying);
    }

    #[test]
    fn label_required() {
        let e = parse("erDiagram\nA ||--o{ B").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("label"));
    }

    #[test]
    fn bad_cardinality_token_errors() {
        let e = parse("erDiagram\nA xx--oo B : r").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn attribute_comment_errors() {
        let e = parse("erDiagram\nA {\nstring name \"the name\"\n}").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains("comment"));
    }

    #[test]
    fn unique_key_errors_named() {
        let e = parse("erDiagram\nA {\nint code UK\n}").unwrap_err();
        assert!(e.message.contains("UK"));
    }

    #[test]
    fn unclosed_block_errors_at_opening() {
        let e = parse("erDiagram\nA {\nstring x").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn multibyte_no_panic() {
        let _ = parse("erDiagram\nA ||--o{ B : héllo\u{2003}🎉\nÉ {\n}");
    }
}
