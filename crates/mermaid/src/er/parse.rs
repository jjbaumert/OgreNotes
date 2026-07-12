//! ER-diagram parser. Line-oriented, mirrors `class::parse` and
//! `state::parse`: `err()` helper, comment/blank skipping, an
//! attribute-block state shaped like class's single-slot member block
//! (nesting one level, opening line tracked, bare `}` closes).
//!
//! Entity names may be bare (`[A-Za-z0-9_-]+`, so Mermaid's canonical
//! hyphenated names like `LINE-ITEM` / `DELIVERY-ADDRESS` parse) or
//! `"double quoted"` (any characters, including spaces). A quoted name
//! keeps its inner text verbatim as the canonical id. Entities may carry
//! a display alias: `CUSTOMER["Customer Account"] { ... }`.
//!
//! Relationship strategy: the `:` label split happens FIRST (on the
//! first colon that is OUTSIDE any quoted span) so label text can never
//! be mistaken for grammar. The pre-colon portion is tokenized
//! quote-aware into `name … cardinality … name`; the FIRST and LAST
//! tokens are the entity endpoints and everything between is the
//! cardinality spec. That spec is either the compact symbol form
//! (`||--o{`, a single ASCII 6-byte token sliced `2+2+2`) or the
//! word-alias form (`only one to zero or more`, `}o..o{` written out),
//! whose line word (`to` / `optionally to`) selects identifying vs not.

use crate::er::{Cardinality, Entity, ErAttribute, ErGraph, ErRelation};
use crate::ParseError;
use std::collections::HashMap;

/// One whitespace-or-quote-delimited token, remembering whether it came
/// from a `"quoted"` span (quoted names bypass the bare-id charset check
/// and may hold spaces / non-ASCII).
struct Tok {
    text: String,
    quoted: bool,
}

/// Splits on whitespace but keeps `"double quoted"` spans intact (quotes
/// stripped, inner spaces preserved). `Err(())` on an unterminated quote.
fn split_ws_quoted(s: &str) -> Result<Vec<Tok>, ()> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut in_bare = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            if in_bare {
                toks.push(Tok { text: std::mem::take(&mut cur), quoted: false });
                in_bare = false;
            }
            let mut q = String::new();
            let mut closed = false;
            for c2 in chars.by_ref() {
                if c2 == '"' {
                    closed = true;
                    break;
                }
                q.push(c2);
            }
            if !closed {
                return Err(());
            }
            toks.push(Tok { text: q, quoted: true });
        } else if c.is_whitespace() {
            if in_bare {
                toks.push(Tok { text: std::mem::take(&mut cur), quoted: false });
                in_bare = false;
            }
        } else {
            cur.push(c);
            in_bare = true;
        }
    }
    if in_bare {
        toks.push(Tok { text: cur, quoted: false });
    }
    Ok(toks)
}

/// Finds the first `:` that is NOT inside a `"quoted"` span and splits
/// there into `(before, Some(after))`; returns `(s, None)` if none. `"`
/// and `:` are ASCII, so the byte indices are always char boundaries.
fn split_label_colon(s: &str) -> (&str, Option<&str>) {
    let mut in_q = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_q = !in_q,
            ':' if !in_q => return (&s[..i], Some(&s[i + 1..])),
            _ => {}
        }
    }
    (s, None)
}

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

/// Maps a spelled-out cardinality alias (Mermaid's word forms) to its
/// `Cardinality`. Case-sensitive, matching Mermaid.
fn alias_cardinality(phrase: &str) -> Option<Cardinality> {
    match phrase {
        "only one" | "1" | "one and only one" => Some(Cardinality::ExactlyOne),
        "zero or one" | "one or zero" => Some(Cardinality::ZeroOrOne),
        "one or more" | "one or many" | "many(1)" | "1+" => Some(Cardinality::OneOrMore),
        "zero or more" | "zero or many" | "many(0)" | "0+" => Some(Cardinality::ZeroOrMore),
        _ => None,
    }
}

/// Parses the compact symbol token (e.g. `||--o{`) into
/// `(card_from, identifying, card_to)`. `None` means invalid.
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
        g: ErGraph { entities: vec![], relations: vec![], class_defs: vec![] },
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

    /// Peels a leading entity name — `"quoted"` (any inner text) or bare
    /// (`[A-Za-z0-9_-]+`) — off the front, returning
    /// `(canonical_name, was_quoted, trimmed_remainder)`. The bare scan
    /// is ASCII-only, so `char count == byte length` and `s[..len]` is a
    /// valid slice; the quoted scan cuts on the ASCII `"` bytes.
    fn take_entity_name<'a>(&self, s: &'a str) -> Result<(String, bool, &'a str), ParseError> {
        if let Some(rest) = s.strip_prefix('"') {
            match rest.find('"') {
                Some(end) => Ok((rest[..end].to_string(), true, rest[end + 1..].trim_start())),
                None => Err(self.err("unterminated quoted entity name")),
            }
        } else {
            let len = s
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .count();
            if len == 0 {
                return Err(self.err(format!("expected an entity id, found {s:?}")));
            }
            Ok((s[..len].to_string(), false, s[len..].trim_start()))
        }
    }

    /// Parses `[alias]` (optionally `["quoted alias"]`) starting at the
    /// leading `[`, returning `(alias_text, trimmed_remainder)`. `[` and
    /// `]` are ASCII, so the byte slices are char-boundary-safe.
    fn parse_alias<'a>(&self, s: &'a str) -> Result<(String, &'a str), ParseError> {
        let inner = &s[1..];
        match inner.find(']') {
            Some(end) => {
                let raw = inner[..end].trim();
                let alias = raw
                    .strip_prefix('"')
                    .and_then(|x| x.strip_suffix('"'))
                    .unwrap_or(raw);
                if alias.is_empty() {
                    return Err(self.err("empty entity alias `[]`"));
                }
                Ok((alias.to_string(), inner[end + 1..].trim_start()))
            }
            None => Err(self.err("unterminated entity alias `[...]`")),
        }
    }

    /// Top-level (non-block) statement: `ENTITY {` / `ENTITY [alias] {`
    /// opens an attribute block, `ENTITY` / `ENTITY [alias]` bare declare
    /// an entity, everything else is a relationship.
    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        // Standalone `ENTITY:::className` — attach a style class. Checked
        // before the keyword dispatch so `:::` never gets mistaken for
        // anything else, and before the entity-name scan so hyphens/etc.
        // inside `id` don't need special-casing here.
        if let Some((id, cls)) = stmt.split_once(":::") {
            let id = id.trim();
            let cls = cls.trim();
            if !id.is_empty() && !cls.is_empty() && !cls.contains(char::is_whitespace) {
                self.validate_id(id)?;
                let idx = self.ensure_entity(id)?;
                self.g.entities[idx].classes.push(cls.to_string());
                return Ok(());
            }
        }
        let first = stmt.split_whitespace().next().unwrap_or("");
        match first {
            "classDef" => return self.parse_class_def(stmt),
            "class" => return self.parse_class_assign(stmt),
            "style" => return self.parse_style(stmt),
            "linkStyle" => return self.parse_link_style(stmt),
            _ => {}
        }
        let (name, quoted, after) = self.take_entity_name(stmt)?;
        if !quoted {
            self.validate_id(&name)?;
        } else if name.is_empty() {
            return Err(self.err("empty quoted entity name"));
        }

        if after.starts_with('[') {
            let (alias, tail) = self.parse_alias(after)?;
            let idx = self.ensure_entity(&name)?;
            self.g.entities[idx].display = Some(alias);
            return self.open_block_or_end(idx, tail);
        }
        if after.starts_with('{') {
            let idx = self.ensure_entity(&name)?;
            return self.open_block_or_end(idx, after);
        }
        if after.is_empty() {
            self.ensure_entity(&name)?;
            return Ok(());
        }
        self.parse_relationship(stmt)
    }

    /// `classDef name prop:val,...`
    fn parse_class_def(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("classDef").unwrap().trim();
        let Some((name, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("classDef needs a name and styles"));
        };
        self.g.class_defs.push(crate::style::ClassDef {
            name: name.trim().to_string(),
            style: crate::style::sanitize_style(styles),
        });
        Ok(())
    }

    /// `class A,B className` — attach a style class to one or more
    /// entities.
    fn parse_class_assign(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("class").unwrap().trim();
        let Some((ids, name)) = rest.rsplit_once(char::is_whitespace) else {
            return Err(self.err("class needs an entity list and a class name"));
        };
        let name = name.trim();
        for id in ids.trim().trim_matches('"').split(',') {
            let id = id.trim();
            self.validate_id(id)?;
            let idx = self.ensure_entity(id)?;
            self.g.entities[idx].classes.push(name.to_string());
        }
        Ok(())
    }

    /// `style <id> prop:val,...`
    fn parse_style(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("style").unwrap().trim();
        let Some((id, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("style needs an entity id and styles"));
        };
        let idx = self.ensure_entity(id.trim())?;
        let s = crate::style::sanitize_style(styles);
        if !s.is_empty() {
            self.g.entities[idx].style = Some(s);
        }
        Ok(())
    }

    /// `linkStyle <index[,index...]|default> prop:val,...` — styles one or
    /// more relations, addressed by declaration index.
    fn parse_link_style(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("linkStyle").unwrap().trim();
        let Some((sel, styles)) = rest.split_once(char::is_whitespace) else {
            return Err(self.err("linkStyle needs an index and styles"));
        };
        let s = crate::style::sanitize_style(styles);
        if s.is_empty() {
            return Ok(());
        }
        let edges = &mut self.g.relations;
        if sel.trim() == "default" {
            for e in edges.iter_mut() {
                e.style = Some(s.clone());
            }
        } else {
            for tok in sel.split(',') {
                if let Ok(i) = tok.trim().parse::<usize>() {
                    if let Some(e) = edges.get_mut(i) {
                        e.style = Some(s.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// After an entity name (+ optional alias), the remainder is either
    /// empty (bare declaration) or `{` opening an attribute block; any
    /// other trailing text is an error.
    fn open_block_or_end(&mut self, idx: usize, tail: &str) -> Result<(), ParseError> {
        let tail = tail.trim();
        if tail.is_empty() {
            return Ok(());
        }
        if let Some(rest) = tail.strip_prefix('{') {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Err(self.err(format!("unexpected text after `{{`: {rest:?}")));
            }
            self.block = Some((idx, self.line));
            return Ok(());
        }
        Err(self.err(format!("unexpected text after entity: {tail:?}")))
    }

    /// `A <cardinality-spec> B : label` — first/last pre-colon tokens are
    /// the endpoints, the middle is the cardinality spec, and the label
    /// after `:` is required by the grammar.
    fn parse_relationship(&mut self, stmt: &str) -> Result<(), ParseError> {
        let (before, after) = split_label_colon(stmt);
        let toks = split_ws_quoted(before)
            .map_err(|()| self.err("unterminated quoted entity name"))?;
        if toks.len() < 3 {
            return Err(self.err(format!("expected `A <cardinality> B`, found {before:?}")));
        }
        let left = &toks[0];
        let right = &toks[toks.len() - 1];
        let middle: Vec<&Tok> = toks[1..toks.len() - 1].iter().collect();
        let (card_from, identifying, card_to) = self.parse_cardinality_spec(&middle)?;
        let label = match after.map(str::trim) {
            Some(l) if !l.is_empty() => l.to_string(),
            _ => return Err(self.err("relationship needs a `: label`")),
        };
        self.validate_entity_token(left)?;
        self.validate_entity_token(right)?;
        let from = self.ensure_entity(&left.text)?;
        let to = self.ensure_entity(&right.text)?;
        self.push_relation(ErRelation {
            from,
            to,
            card_from,
            card_to,
            identifying,
            label,
            style: None,
        })
    }

    /// The tokens between the two endpoints: either one compact symbol
    /// token (`||--o{`) or a word-alias phrase (`only one to zero or
    /// more`). The line word `to` / `optionally to` picks identifying vs
    /// non-identifying and splits the left/right alias phrases.
    fn parse_cardinality_spec(
        &self,
        middle: &[&Tok],
    ) -> Result<(Cardinality, bool, Cardinality), ParseError> {
        if let [only] = middle {
            if !only.quoted {
                if let Some(t) = parse_relationship_token(&only.text) {
                    return Ok(t);
                }
            }
        }
        if middle.is_empty() || middle.iter().any(|t| t.quoted) {
            return Err(self.err("invalid relationship cardinality"));
        }
        let phrase = middle.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
        let (identifying, lp, rp) = if let Some((l, r)) = phrase.split_once(" optionally to ") {
            (false, l, r)
        } else if let Some((l, r)) = phrase.split_once(" to ") {
            (true, l, r)
        } else {
            return Err(self.err(format!("invalid relationship cardinality {phrase:?}")));
        };
        let card_from = alias_cardinality(lp.trim())
            .ok_or_else(|| self.err(format!("unknown cardinality {:?}", lp.trim())))?;
        let card_to = alias_cardinality(rp.trim())
            .ok_or_else(|| self.err(format!("unknown cardinality {:?}", rp.trim())))?;
        Ok((card_from, identifying, card_to))
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

    /// `type name [keys] ["comment"]` — `type`/`name` are stored VERBATIM
    /// (free text; the svg renderer escapes them). `keys` is any
    /// comma-or-space-separated combination of `PK` / `FK` / `UK`; a
    /// trailing `"..."` is the comment (verbatim, may contain spaces).
    fn parse_attribute_row(&self, line: &str) -> Result<ErAttribute, ParseError> {
        // Peel a trailing quoted comment first: the first `"` opens it and
        // it must run to a closing `"` with nothing after. `"` is ASCII,
        // so `line[..q]` splits on a char boundary.
        let (main, comment) = match line.find('"') {
            Some(q) => {
                let inner = &line[q + 1..];
                match inner.find('"') {
                    Some(e) => {
                        let after = inner[e + 1..].trim();
                        if !after.is_empty() {
                            return Err(
                                self.err(format!("unexpected text after attribute comment: {after:?}"))
                            );
                        }
                        (line[..q].trim_end(), Some(inner[..e].to_string()))
                    }
                    None => return Err(self.err("unterminated attribute comment")),
                }
            }
            None => (line, None),
        };

        let mut tokens = main.split_whitespace();
        let (Some(ty), Some(name)) = (tokens.next(), tokens.next()) else {
            return Err(self.err(format!("expected `type name` attribute row, found {line:?}")));
        };

        let key_str = tokens.collect::<Vec<_>>().join(" ");
        let mut keys = Vec::new();
        for k in key_str.split(|c: char| c == ',' || c.is_whitespace()) {
            let k = k.trim();
            if k.is_empty() {
                continue;
            }
            match k {
                "PK" | "FK" | "UK" => keys.push(k.to_string()),
                other => {
                    return Err(self.err(format!(
                        "unsupported attribute key marker {other:?} (expected PK, FK, or UK)"
                    )))
                }
            }
        }
        Ok(ErAttribute { ty: ty.to_string(), name: name.to_string(), keys, comment })
    }

    fn validate_id(&self, id: &str) -> Result<(), ParseError> {
        if id.is_empty()
            || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(self.err(format!("invalid entity id {id:?}")));
        }
        Ok(())
    }

    /// A relationship endpoint: quoted tokens are free text (any non-empty
    /// content); bare tokens must satisfy the bare-id charset.
    fn validate_entity_token(&self, t: &Tok) -> Result<(), ParseError> {
        if t.quoted {
            if t.text.is_empty() {
                return Err(self.err("empty quoted entity name"));
            }
            Ok(())
        } else {
            self.validate_id(&t.text)
        }
    }

    /// Look up an existing entity by id, or create it (implicit creation
    /// on first reference). The caller is responsible for having
    /// validated the id — quoted names legitimately fall outside the
    /// bare-id charset.
    fn ensure_entity(&mut self, id: &str) -> Result<usize, ParseError> {
        if let Some(&i) = self.ids.get(id) {
            return Ok(i);
        }
        let idx = self.g.entities.len();
        self.g.entities.push(Entity {
            id: id.to_string(),
            display: None,
            attributes: vec![],
            classes: vec![],
            style: None,
        });
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
        assert!(c.attributes[0].keys.is_empty());
        assert_eq!(c.attributes[1].keys, vec!["PK"]);
        assert_eq!(c.attributes[2].keys, vec!["FK"]);
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

    // --- new coverage: parity gaps closed against the Mermaid ER spec ---

    #[test]
    fn hyphenated_entity_names() {
        // Mermaid's canonical docs example uses hyphenated names.
        let g = p("erDiagram\n\
                   CUSTOMER ||--o{ ORDER : places\n\
                   ORDER ||--|{ LINE-ITEM : contains\n\
                   CUSTOMER }|..|{ DELIVERY-ADDRESS : uses");
        let ids: Vec<&str> = g.entities.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"LINE-ITEM"));
        assert!(ids.contains(&"DELIVERY-ADDRESS"));
        assert_eq!(g.relations.len(), 3);
    }

    #[test]
    fn quoted_entity_names_with_spaces() {
        let g = p("erDiagram\n\"Order Detail\" ||--|| CUSTOMER : x");
        assert_eq!(g.entities[0].id, "Order Detail");
        assert_eq!(g.entities[1].id, "CUSTOMER");
        // Same quoted name refers to the same entity.
        let g2 = p("erDiagram\n\"Order Detail\" {\nstring sku\n}\n\"Order Detail\" ||--|| A : x");
        assert_eq!(g2.entities.len(), 2);
        assert_eq!(g2.entities[0].attributes.len(), 1);
    }

    #[test]
    fn entity_alias() {
        let g = p("erDiagram\np[\"Customer Account\"] {\nstring firstName\n}\np ||--|| q : x");
        assert_eq!(g.entities[0].id, "p");
        assert_eq!(g.entities[0].display.as_deref(), Some("Customer Account"));
        assert_eq!(g.entities[0].attributes.len(), 1);
        // Unquoted alias too.
        let g2 = p("erDiagram\nCUSTOMER[Person]");
        assert_eq!(g2.entities[0].display.as_deref(), Some("Person"));
    }

    #[test]
    fn word_form_cardinalities() {
        let g = p("erDiagram\nCUSTOMER only one to zero or more ORDER : places");
        assert_eq!(g.relations[0].card_from, Cardinality::ExactlyOne);
        assert_eq!(g.relations[0].card_to, Cardinality::ZeroOrMore);
        assert!(g.relations[0].identifying);

        let g2 = p("erDiagram\nA one or more optionally to zero or one B : r");
        assert_eq!(g2.relations[0].card_from, Cardinality::OneOrMore);
        assert_eq!(g2.relations[0].card_to, Cardinality::ZeroOrOne);
        assert!(!g2.relations[0].identifying); // `optionally to` = non-identifying

        // Short aliases.
        let g3 = p("erDiagram\nA 1 to 1+ B : r");
        assert_eq!(g3.relations[0].card_from, Cardinality::ExactlyOne);
        assert_eq!(g3.relations[0].card_to, Cardinality::OneOrMore);
    }

    #[test]
    fn unknown_word_cardinality_errors() {
        let e = parse("erDiagram\nA several to many B : r").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn unique_key_supported() {
        let g = p("erDiagram\nA {\nint code UK\n}");
        assert_eq!(g.entities[0].attributes[0].keys, vec!["UK"]);
    }

    #[test]
    fn combined_keys() {
        let g = p("erDiagram\nA {\nint id PK, FK\nint x PK,UK\n}");
        assert_eq!(g.entities[0].attributes[0].keys, vec!["PK", "FK"]);
        assert_eq!(g.entities[0].attributes[1].keys, vec!["PK", "UK"]);
    }

    #[test]
    fn attribute_comment_kept() {
        let g = p("erDiagram\nA {\nstring name \"the display name\"\nint id PK \"primary\"\n}");
        assert_eq!(g.entities[0].attributes[0].comment.as_deref(), Some("the display name"));
        assert_eq!(g.entities[0].attributes[0].name, "name");
        assert_eq!(g.entities[0].attributes[1].keys, vec!["PK"]);
        assert_eq!(g.entities[0].attributes[1].comment.as_deref(), Some("primary"));
    }

    #[test]
    fn unsupported_key_marker_errors_named() {
        let e = parse("erDiagram\nA {\nint code XX\n}").unwrap_err();
        assert!(e.message.contains("XX"));
    }

    #[test]
    fn unclosed_block_errors_at_opening() {
        let e = parse("erDiagram\nA {\nstring x").unwrap_err();
        assert_eq!(e.line, Some(2));
    }

    #[test]
    fn er_styling_parses() {
        let g = parse("erDiagram\nclassDef warm fill:#f80\nCAR\nCAR:::warm\nstyle CAR fill:#333").unwrap();
        let car = g.entities.iter().find(|e| e.id == "CAR").unwrap();
        assert_eq!(car.classes, vec!["warm".to_string()]);
        assert_eq!(car.style.as_deref(), Some("fill:#333"));
        assert_eq!(g.class_defs.iter().find(|d| d.name == "warm").unwrap().style, "fill:#f80");
    }

    #[test]
    fn multibyte_no_panic() {
        let _ = parse("erDiagram\nA ||--o{ B : héllo\u{2003}🎉\nÉ {\n}");
        let _ = parse("erDiagram\n\"Café Ordér\" ||--|| A : naïve\nB { int \u{e9}x PK }");
        let _ = parse("erDiagram\n\"unterminated ||--|| A : x");
    }
}
