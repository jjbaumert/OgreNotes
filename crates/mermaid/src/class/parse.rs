//! Class-diagram parser. Line-oriented, mirrors `flowchart::parse` and
//! `state::parse`: `err()` helper, ASCII id scan with byte-safety
//! comment, `split_once(':')` for labels, exact-match guards for
//! keyword/brace statements, a member-block state machine shaped like
//! state's composite stack (opening line tracked, bare `}` closes).
//!
//! Relationship parsing strategy (see the operator table below): the
//! `:` label split happens FIRST on the original statement so a label
//! containing operator-looking characters can never confuse the scan.
//! The remainder is then whitespace-tokenized and scanned left to right
//! for the first token that exactly matches an operator; quoted
//! multiplicities are the tokens immediately adjacent to that operator
//! and endpoint ids are the outermost tokens. Direction is normalized
//! per the table so `to` is always the marker end; multiplicities swap
//! along with `from`/`to` when the operator's raw order is reversed.

use crate::class::{ClassBox, ClassGraph, RelKind, Relation};
use crate::ParseError;
use std::collections::HashMap;

/// (operator token, swap, kind, arrow, back_arrow). `swap` = true means the
/// raw left/right order must flip to reach the normalized from/to (the
/// marker sits on the raw-left side); false means raw order already
/// matches normalized order (the marker sits on the raw-right side).
/// `back_arrow` = a second arrowhead on the `from` end (the bidirectional
/// `<-->` / `<..>` forms).
const OPERATORS: &[(&str, bool, RelKind, bool, bool)] = &[
    ("<|--", true, RelKind::Inheritance, false, false),
    ("--|>", false, RelKind::Inheritance, false, false),
    ("<|..", true, RelKind::Realization, false, false),
    ("..|>", false, RelKind::Realization, false, false),
    ("*--", true, RelKind::Composition, false, false),
    ("--*", false, RelKind::Composition, false, false),
    ("o--", true, RelKind::Aggregation, false, false),
    ("--o", false, RelKind::Aggregation, false, false),
    ("<-->", false, RelKind::Association, true, true),
    ("-->", false, RelKind::Association, true, false),
    ("<--", true, RelKind::Association, true, false),
    ("--", false, RelKind::Association, false, false),
    ("<..>", false, RelKind::Dependency, false, true),
    ("..>", false, RelKind::Dependency, false, false),
    ("<..", true, RelKind::Dependency, false, false),
    ("..", false, RelKind::DashedLink, false, false),
];

fn lookup_operator(tok: &str) -> Option<(bool, RelKind, bool, bool)> {
    OPERATORS
        .iter()
        .find(|(s, ..)| *s == tok)
        .map(|&(_, swap, kind, arrow, back)| (swap, kind, arrow, back))
}

/// The LONGEST operator that `sub` starts with, or `None`. Longest-first
/// so that `-->` beats `--`, `..>` beats `..`, `<|--` beats nothing
/// shorter, etc. — the key to splitting glued operators like `A-->B`.
/// All operator tokens are ASCII, so a prefix match is byte-safe.
fn match_operator_at(sub: &str) -> Option<&'static str> {
    OPERATORS
        .iter()
        .map(|&(op, ..)| op)
        .filter(|op| sub.starts_with(op))
        .max_by_key(|op| op.len())
}

/// Tokenizes a relationship's pre-colon portion, splitting operators out
/// as their own tokens even when glued to ids (`A-->B` → `[A, -->, B]`)
/// while keeping `"quoted multiplicities"` intact (their inner `..` is
/// never mistaken for an operator). Whitespace-separated input tokenizes
/// identically, so spaced and unspaced forms share one path.
fn tokenize_rel(s: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        if c == '"' {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            let mut q = String::from("\"");
            for (_, c2) in chars.by_ref() {
                q.push(c2);
                if c2 == '"' {
                    break;
                }
            }
            toks.push(q); // an unterminated quote is left for parse_mult to reject
        } else if c.is_whitespace() {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
        } else if let Some(op) = match_operator_at(&s[idx..]) {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            toks.push(op.to_string());
            for _ in 1..op.len() {
                chars.next(); // ASCII operator: 1 byte == 1 char per step
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

struct Parser {
    g: ClassGraph,
    ids: HashMap<String, usize>,
    line: usize, // 1-based, for errors
    /// Open member block: (class index, opening line). Class diagrams
    /// nest at most one level deep (a nested `{` inside a block errors),
    /// so a single slot suffices — unlike flowchart/state's stack.
    block: Option<(usize, usize)>,
}

/// Rewrite Mermaid generic syntax `~Type~` to `<Type>` in a member string.
/// A single leading visibility marker (`+ - # ~`) is peeled off first so its
/// `~` is never mistaken for a generic opener (`~List~int~ x` → `~List<int> x`).
fn generics_to_angle(s: &str) -> String {
    let (vis, rest) = match s.chars().next() {
        Some(c @ ('+' | '-' | '#' | '~')) => (&s[..c.len_utf8()], &s[c.len_utf8()..]),
        _ => ("", s),
    };
    let mut out = String::from(vis);
    let mut cur = rest;
    while let Some(open) = cur.find('~') {
        match cur[open + 1..].find('~') {
            Some(rel) => {
                let close = open + 1 + rel;
                out.push_str(&cur[..open]);
                out.push('<');
                out.push_str(&cur[open + 1..close]);
                out.push('>');
                cur = &cur[close + 1..];
            }
            None => break,
        }
    }
    out.push_str(cur);
    out
}

pub(crate) fn parse(source: &str) -> Result<ClassGraph, ParseError> {
    let mut p = Parser {
        g: ClassGraph { classes: vec![], relations: vec![] },
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
            if header != "classDiagram" && header != "classDiagram-v2" {
                return Err(p.err("class diagram must start with `classDiagram`"));
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
            message: "class diagram must start with `classDiagram`".into(),
            line: Some(1),
        });
    }
    if let Some((idx, opening_line)) = p.block {
        return Err(ParseError {
            message: format!("unclosed class member block `{}`", p.g.classes[idx].id),
            line: Some(opening_line),
        });
    }
    Ok(p.g)
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { message: msg.into(), line: Some(self.line) }
    }

    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        // `:::` is the CSS-class shorthand (`Node:::className`). We don't
        // support styling; reject it explicitly so it can never slip
        // through the `:` label split and be silently stored as a bogus
        // member.
        if stmt.contains(":::") {
            return Err(self.err("CSS class shorthand `:::` is not supported"));
        }
        // Standalone annotation: `<<interface>> ClassName` sets that class's
        // annotation. (The `<<...>>`-on-its-own-line form inside a class block
        // is handled by `apply_member`.)
        if let Some(rest) = stmt.strip_prefix("<<") {
            if let Some(end) = rest.find(">>") {
                let annot = rest[..end].trim().to_string();
                let id = rest[end + 2..].trim();
                self.validate_id(id)?;
                let idx = self.ensure_class(id)?;
                self.g.classes[idx].annotation = Some(annot);
                return Ok(());
            }
        }
        let first = stmt.split_whitespace().next().unwrap_or("");
        match first {
            "namespace" | "click" | "callback" | "style" | "cssClass" | "link" | "note"
            | "classDef" | "direction" => {
                return Err(self.err(format!("`{first}` statements are not supported")));
            }
            "class" => return self.parse_class_stmt(stmt),
            _ => {}
        }
        self.parse_member_or_relationship(stmt)
    }

    /// `class Name` / `class Name {` / `class Name["Label"]` (+ optional
    /// `{`).
    fn parse_class_stmt(&mut self, stmt: &str) -> Result<(), ParseError> {
        let rest = stmt.strip_prefix("class").unwrap().trim_start();
        // ASCII id scan: char count == byte length only because the
        // predicate is ASCII-only; do not relax without a byte-position
        // scan.
        let id_len = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
        if id_len == 0 {
            return Err(self.err("expected a class id after `class`"));
        }
        let id = rest[..id_len].to_string();
        let idx = self.ensure_class(&id)?;
        let mut after = rest[id_len..].trim_start();
        // Generic parameter: `class Square~Shape~` → title "Square<Shape>".
        // Relationships still reference the bare id.
        let mut generic: Option<String> = None;
        if let Some(g_rest) = after.strip_prefix('~') {
            if let Some(end) = g_rest.find('~') {
                generic = Some(g_rest[..end].trim().to_string());
                after = g_rest[end + 1..].trim_start();
            }
        }
        if after.starts_with('[') {
            let (label, tail) = self.parse_class_label(after)?;
            self.g.classes[idx].display = Some(label);
            after = tail;
        } else if let Some(g) = &generic {
            self.g.classes[idx].display = Some(format!("{id}<{g}>"));
        }
        let after = after.trim();
        if after.is_empty() {
            return Ok(());
        }
        if after == "{" {
            self.block = Some((idx, self.line));
            return Ok(());
        }
        Err(self.err(format!("unexpected text after class id: {after:?}")))
    }

    /// `["Label text"]` (or `[Label]`, or a markdown-string
    /// ``["`Label`"]``) starting at the leading `[`. Returns
    /// `(label, trimmed_remainder)`. `[` / `]` are ASCII, so the byte
    /// slices are char-boundary-safe.
    fn parse_class_label<'a>(&self, s: &'a str) -> Result<(String, &'a str), ParseError> {
        let inner = &s[1..];
        match inner.find(']') {
            Some(end) => {
                let raw = inner[..end].trim();
                let unquoted =
                    raw.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(raw);
                let label = unquoted
                    .strip_prefix('`')
                    .and_then(|x| x.strip_suffix('`'))
                    .unwrap_or(unquoted);
                if label.is_empty() {
                    return Err(self.err("empty class label `[]`"));
                }
                Ok((label.to_string(), inner[end + 1..].trim_start()))
            }
            None => Err(self.err("unterminated class label `[...]`")),
        }
    }

    /// One line inside an open member block: bare `}` closes it; a line
    /// containing `{` (any other brace use) errors; anything else is a
    /// member, classified and stored verbatim.
    fn handle_block_line(&mut self, line: &str) -> Result<(), ParseError> {
        if line == "}" {
            self.block = None;
            return Ok(());
        }
        if line.contains('{') {
            return Err(self.err("nested `{` in a class member block is not supported"));
        }
        let (idx, _) = self.block.expect("handle_block_line only called while a block is open");
        Self::apply_member(&mut self.g.classes[idx], line);
        Ok(())
    }

    /// Dispatches a non-`class`, non-keyword statement: a relationship
    /// if a whitespace token in the pre-colon portion exactly matches an
    /// operator, otherwise (requires the colon) the dotted-member form
    /// `Name : member`.
    fn parse_member_or_relationship(&mut self, stmt: &str) -> Result<(), ParseError> {
        // Split the label FIRST, on the ORIGINAL statement, so a label's
        // text can never be mistaken for an operator token during the
        // scan below.
        let (before, after) = match stmt.split_once(':') {
            Some((b, a)) => (b, Some(a)),
            None => (stmt, None),
        };
        let owned = tokenize_rel(before);
        let tokens: Vec<&str> = owned.iter().map(String::as_str).collect();
        if let Some(op_pos) = tokens.iter().position(|t| lookup_operator(t).is_some()) {
            let label = after.map(|s| s.trim().to_string());
            return self.parse_relationship(&tokens, op_pos, label);
        }
        let Some(after) = after else {
            return Err(self.err(format!("unrecognized statement: {stmt:?}")));
        };
        self.parse_dotted_member(before, after)
    }

    /// `A "m1" <op> "m2" B` (multiplicities optional). `tokens` is the
    /// whitespace split of the pre-colon portion; `op_pos` is the index
    /// of the (already located) operator token.
    fn parse_relationship(
        &mut self,
        tokens: &[&str],
        op_pos: usize,
        label: Option<String>,
    ) -> Result<(), ParseError> {
        if op_pos == 0 || op_pos == tokens.len() - 1 {
            return Err(self.err("relationship needs a class id on each side of the operator"));
        }
        let (swap, kind, arrow, back_arrow) = lookup_operator(tokens[op_pos]).unwrap();
        let left_extra = &tokens[1..op_pos];
        let right_extra = &tokens[op_pos + 1..tokens.len() - 1];
        if left_extra.len() > 1 || right_extra.len() > 1 {
            return Err(self.err("unexpected tokens around relationship operator"));
        }
        let raw_left_mult = left_extra.first().map(|t| self.parse_mult(t)).transpose()?;
        let raw_right_mult = right_extra.first().map(|t| self.parse_mult(t)).transpose()?;
        let raw_left_id = tokens[0];
        let raw_right_id = tokens[tokens.len() - 1];
        self.validate_id(raw_left_id)?;
        self.validate_id(raw_right_id)?;
        let left_idx = self.ensure_class(raw_left_id)?;
        let right_idx = self.ensure_class(raw_right_id)?;
        // Multiplicities stay attached to the id they were written next
        // to, so they swap along with from/to.
        let (from, to, m_from, m_to) = if swap {
            (right_idx, left_idx, raw_right_mult, raw_left_mult)
        } else {
            (left_idx, right_idx, raw_left_mult, raw_right_mult)
        };
        self.push_relation(Relation { from, to, kind, arrow, back_arrow, m_from, m_to, label })
    }

    /// `Name : member` — same `(` classification as block members.
    fn parse_dotted_member(&mut self, before: &str, after: &str) -> Result<(), ParseError> {
        let id = before.trim();
        self.validate_id(id)?;
        let member = after.trim();
        if member.is_empty() {
            return Err(self.err("dotted member form needs member text after `:`"));
        }
        let idx = self.ensure_class(id)?;
        Self::apply_member(&mut self.g.classes[idx], member);
        Ok(())
    }

    /// A quoted token like `"0..*"`; strips the quotes, byte-safely
    /// (`strip_prefix`/`strip_suffix` operate on whole `char`s, never
    /// mid-codepoint, so this is safe even if the multiplicity text were
    /// multibyte).
    fn parse_mult(&self, tok: &str) -> Result<String, ParseError> {
        match tok.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            Some(s) if !s.is_empty() => Ok(s.to_string()),
            _ => Err(self.err(format!("expected a quoted multiplicity, found {tok:?}"))),
        }
    }

    fn validate_id(&self, id: &str) -> Result<(), ParseError> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(self.err(format!("invalid class id {id:?}")));
        }
        Ok(())
    }

    /// Classifies and stores one member line/text VERBATIM (after trim):
    /// an `<<annotation>>` line sets the class annotation; a line
    /// containing `(` anywhere is a method; else it's an attribute.
    fn apply_member(cls: &mut ClassBox, text: &str) {
        let t = text.trim();
        if let Some(inner) = t.strip_prefix("<<").and_then(|s| s.strip_suffix(">>")) {
            cls.annotation = Some(inner.trim().to_string());
        } else if t.contains('(') {
            cls.methods.push(generics_to_angle(t));
        } else {
            cls.attributes.push(generics_to_angle(t));
        }
    }

    /// Look up an existing class by id, or create it (implicit
    /// creation on first reference).
    fn ensure_class(&mut self, id: &str) -> Result<usize, ParseError> {
        if let Some(&i) = self.ids.get(id) {
            return Ok(i);
        }
        let idx = self.g.classes.len();
        self.g.classes.push(ClassBox {
            id: id.to_string(),
            display: None,
            annotation: None,
            attributes: vec![],
            methods: vec![],
        });
        self.ids.insert(id.to_string(), idx);
        if self.g.classes.len() > crate::layout::MAX_NODES {
            return Err(self.err(format!(
                "diagram too large: too many classes (max {})",
                crate::layout::MAX_NODES
            )));
        }
        Ok(idx)
    }

    fn push_relation(&mut self, rel: Relation) -> Result<(), ParseError> {
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
    use crate::class::{ClassGraph, RelKind};

    fn p(src: &str) -> ClassGraph {
        parse(src).expect("parse ok")
    }

    fn rel(g: &ClassGraph, i: usize) -> (&str, &str, RelKind) {
        let r = &g.relations[i];
        (&g.classes[r.from].id, &g.classes[r.to].id, r.kind)
    }

    #[test]
    fn header_required() {
        assert!(parse("classDiagram\nclass A").is_ok());
        assert_eq!(parse("class A").unwrap_err().line, Some(1));
    }

    #[test]
    fn member_block_classification() {
        let g = p("classDiagram\nclass Animal {\n<<abstract>>\n+String name\n-int age\n+speak() String\n#walk(int steps)\n}");
        let a = &g.classes[0];
        assert_eq!(a.annotation.as_deref(), Some("abstract"));
        assert_eq!(a.attributes, vec!["+String name", "-int age"]);
        assert_eq!(a.methods, vec!["+speak() String", "#walk(int steps)"]);
    }

    #[test]
    fn dotted_member_form() {
        let g = p("classDiagram\nDuck : +swim()\nDuck : +String beak");
        assert_eq!(g.classes[0].methods, vec!["+swim()"]);
        assert_eq!(g.classes[0].attributes, vec!["+String beak"]);
    }

    #[test]
    fn all_relationship_kinds_normalized() {
        let cases = [
            ("A <|-- B", ("B", "A", RelKind::Inheritance)),
            ("A --|> B", ("A", "B", RelKind::Inheritance)),
            ("A <|.. B", ("B", "A", RelKind::Realization)),
            ("A *-- B", ("B", "A", RelKind::Composition)),
            ("A --* B", ("A", "B", RelKind::Composition)),
            ("A o-- B", ("B", "A", RelKind::Aggregation)),
            ("A --> B", ("A", "B", RelKind::Association)),
            ("A <-- B", ("B", "A", RelKind::Association)),
            ("A -- B", ("A", "B", RelKind::Association)),
            ("A ..> B", ("A", "B", RelKind::Dependency)),
            ("A <.. B", ("B", "A", RelKind::Dependency)),
        ];
        for (src, want) in cases {
            let g = p(&format!("classDiagram\n{src}"));
            let got = rel(&g, 0);
            assert_eq!(got, want, "for {src}");
        }
    }

    #[test]
    fn plain_association_has_no_arrow() {
        let g = p("classDiagram\nA -- B");
        assert!(!g.relations[0].arrow);
        assert!(!g.relations[0].back_arrow);
        let g2 = p("classDiagram\nA --> B");
        assert!(g2.relations[0].arrow);
        assert!(!g2.relations[0].back_arrow);
    }

    #[test]
    fn bidirectional_relations_arrow_both_ends() {
        // `<-->` / `<..>` were previously rejected ("expected a quoted
        // multiplicity, found >"), which broke the canonical intro class
        // example. They must parse and carry an arrowhead on each end.
        let g = p("classDiagram\nClass08 <--> C2: Cool label");
        let r = &g.relations[0];
        assert_eq!(r.kind, RelKind::Association);
        assert!(r.arrow && r.back_arrow, "both ends should have arrowheads");
        assert_eq!(r.label.as_deref(), Some("Cool label"));

        let g2 = p("classDiagram\nA <..> B");
        let r2 = &g2.relations[0];
        assert_eq!(r2.kind, RelKind::Dependency);
        assert!(r2.back_arrow);

        // `<-->` must win the longest-match over `<--` so the trailing `>`
        // is not left dangling.
        let svg = crate::render("classDiagram\nClass08 <--> C2: Cool label")
            .svg
            .expect("intro class example renders");
        assert!(svg.contains("marker-start"), "back arrowhead missing: {svg}");
    }

    #[test]
    fn multiplicities_and_label_follow_normalization() {
        let g = p("classDiagram\nCustomer \"1\" --> \"0..*\" Order : places");
        let r = &g.relations[0];
        assert_eq!(g.classes[r.from].id, "Customer");
        assert_eq!(r.m_from.as_deref(), Some("1"));
        assert_eq!(r.m_to.as_deref(), Some("0..*"));
        assert_eq!(r.label.as_deref(), Some("places"));
        // Reversed operator swaps multiplicities too.
        let g2 = p("classDiagram\nOrder \"0..*\" <-- \"1\" Customer");
        let r2 = &g2.relations[0];
        assert_eq!(g2.classes[r2.from].id, "Customer");
        assert_eq!(r2.m_from.as_deref(), Some("1"));
        assert_eq!(r2.m_to.as_deref(), Some("0..*"));
    }

    #[test]
    fn unclosed_member_block_errors_at_opening() {
        let e = parse("classDiagram\nclass A {\n+x int").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("unclosed"));
    }

    #[test]
    fn out_of_scope_statements_error_named() {
        for stmt in ["namespace N {", "click A call x()", "callback A \"cb\"",
                     "style A fill:#f00", "cssClass \"A\" cls", "link A \"url\"",
                     "note for A \"text\""] {
            let src = format!("classDiagram\nclass A\n{stmt}");
            let e = parse(&src).unwrap_err();
            assert_eq!(e.line, Some(3), "for {stmt}");
            let kw = stmt.split_whitespace().next().unwrap();
            assert!(e.message.contains(kw), "names {kw}: {}", e.message);
        }
    }

    #[test]
    fn css_shorthand_rejected_not_misparsed() {
        // Regression: `A:::styleName` used to silently split on `:` and
        // store `::styleName` as a bogus attribute. It must error instead.
        let e = parse("classDiagram\nclass A\nA:::styleName").unwrap_err();
        assert_eq!(e.line, Some(3));
        assert!(e.message.contains(":::"), "{}", e.message);
    }

    #[test]
    fn classdef_and_direction_rejected_named() {
        for stmt in ["classDef default fill:#f9f", "direction LR"] {
            let e = parse(&format!("classDiagram\nclass A\n{stmt}")).unwrap_err();
            assert_eq!(e.line, Some(3), "for {stmt}");
            let kw = stmt.split_whitespace().next().unwrap();
            assert!(e.message.contains(kw), "names {kw}: {}", e.message);
        }
    }

    #[test]
    fn dashed_link_operator() {
        let g = p("classDiagram\nA .. B");
        assert_eq!(rel(&g, 0), ("A", "B", RelKind::DashedLink));
        assert!(!g.relations[0].arrow);
        // `..>` (dependency) must still win over `..` when glued/adjacent.
        let g2 = p("classDiagram\nA ..> B");
        assert_eq!(g2.relations[0].kind, RelKind::Dependency);
    }

    #[test]
    fn no_space_operators() {
        let cases = [
            ("A-->B", ("A", "B", RelKind::Association)),
            ("A<|--B", ("B", "A", RelKind::Inheritance)),
            ("A*--B", ("B", "A", RelKind::Composition)),
            ("A..>B", ("A", "B", RelKind::Dependency)),
            ("A..B", ("A", "B", RelKind::DashedLink)),
        ];
        for (src, want) in cases {
            let g = p(&format!("classDiagram\n{src}"));
            assert_eq!(rel(&g, 0), want, "for {src}");
        }
        // Glued multiplicities still separate correctly.
        let g = p("classDiagram\nCustomer\"1\"-->\"0..*\"Order : places");
        let r = &g.relations[0];
        assert_eq!(r.m_from.as_deref(), Some("1"));
        assert_eq!(r.m_to.as_deref(), Some("0..*"));
        assert_eq!(r.label.as_deref(), Some("places"));
    }

    #[test]
    fn class_label() {
        let g = p("classDiagram\nclass A[\"Nice Label\"]\nA : +x int");
        assert_eq!(g.classes[0].id, "A");
        assert_eq!(g.classes[0].display.as_deref(), Some("Nice Label"));
        assert_eq!(g.classes[0].attributes, vec!["+x int"]);
        // Label + member block on the same line.
        let g2 = p("classDiagram\nclass B[\"Label B\"] {\n+y int\n}");
        assert_eq!(g2.classes[0].display.as_deref(), Some("Label B"));
        assert_eq!(g2.classes[0].attributes, vec!["+y int"]);
        // Markdown-string label: backticks stripped.
        let g3 = p("classDiagram\nclass C[\"`Markdown`\"]");
        assert_eq!(g3.classes[0].display.as_deref(), Some("Markdown"));
    }

    #[test]
    fn v2_header_accepted() {
        assert!(parse("classDiagram-v2\nclass A").is_ok());
    }

    #[test]
    fn generics_rendered_with_angle_brackets() {
        // Deliberate parity change: Mermaid renders `~T~` generics as `<T>`.
        // (Previously kept verbatim; see `generics_to_angle`.) The leading
        // visibility marker's own context is preserved.
        let g = p("classDiagram\nclass Box {\n+items List~T~\n}");
        assert_eq!(g.classes[0].attributes, vec!["+items List<T>"]);
        // A class-id generic decorates the title, id stays bare.
        let g2 = p("classDiagram\nclass Square~Shape~\nSquare : +area() double");
        assert_eq!(g2.classes[0].id, "Square");
        assert_eq!(g2.classes[0].display.as_deref(), Some("Square<Shape>"));
        // Nested generic after a visibility marker isn't mis-split.
        let g3 = p("classDiagram\nclass M {\n+data Map~String, int~\n}");
        assert_eq!(g3.classes[0].attributes, vec!["+data Map<String, int>"]);
    }

    #[test]
    fn standalone_annotation_sets_class_annotation() {
        // `<<enumeration>> Color` (annotation as its own statement, outside a
        // class block) must set the class annotation, same as the inline form.
        let g = p("classDiagram\nclass Color\n<<enumeration>> Color\nColor : RED");
        assert_eq!(g.classes[0].annotation.as_deref(), Some("enumeration"));
        // Works even when the class is created implicitly by the annotation.
        let g2 = p("classDiagram\n<<interface>> Shape");
        assert_eq!(g2.classes[0].id, "Shape");
        assert_eq!(g2.classes[0].annotation.as_deref(), Some("interface"));
    }

    #[test]
    fn multibyte_no_panic() {
        let _ = parse("classDiagram\nclass Émile\nA\u{2003}--> B : héllo 🎉");
    }

    #[test]
    fn relation_cap_enforced() {
        let mut src = String::from("classDiagram\n");
        for i in 0..=crate::layout::MAX_EDGES {
            src.push_str(&format!("A{} --> B{}\n", i % 100, i % 100));
        }
        assert!(parse(&src).unwrap_err().message.contains("too large"));
    }
}
