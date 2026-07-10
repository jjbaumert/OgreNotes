//! Sequence-diagram parser. Line-oriented; ids are ASCII word chars
//! (char count == byte length — do not relax without a byte-position
//! scan); arrows matched longest-first; caps enforced inline so work
//! is bounded before layout.

use crate::sequence::{
    Event, Head, LineStyle, Participant, SeqDiagram,
    MAX_EVENTS, MAX_PARTICIPANTS,
};
use crate::ParseError;
use std::collections::HashMap;

struct Parser {
    g: SeqDiagram,
    ids: HashMap<String, usize>,
    active_depth: Vec<usize>,
    line: usize,
}

pub(crate) fn parse(source: &str) -> Result<SeqDiagram, ParseError> {
    let mut p = Parser {
        g: SeqDiagram { participants: vec![], events: vec![] },
        ids: HashMap::new(),
        active_depth: vec![],
        line: 0,
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
            if header != "sequenceDiagram" {
                return Err(p.err("sequence diagram must start with `sequenceDiagram`"));
            }
            seen_header = true;
            continue;
        }
        p.parse_statement(line)?;
    }
    if !seen_header {
        return Err(ParseError {
            message: "sequence diagram must start with `sequenceDiagram`".into(),
            line: Some(1),
        });
    }
    Ok(p.g)
}

impl Parser {
    fn err(&self, msg: impl Into<String>) -> ParseError {
        ParseError { message: msg.into(), line: Some(self.line) }
    }

    fn push_event(&mut self, e: Event) -> Result<(), ParseError> {
        if self.g.events.len() >= MAX_EVENTS {
            return Err(self.err(format!(
                "diagram too large: more than {MAX_EVENTS} events"
            )));
        }
        self.g.events.push(e);
        Ok(())
    }

    fn intern(&mut self, id: &str, display: Option<String>, is_actor: bool) -> Result<usize, ParseError> {
        if let Some(&i) = self.ids.get(id) {
            // Explicit declaration after implicit use upgrades display/actor.
            if let Some(d) = display {
                self.g.participants[i].display = d;
                self.g.participants[i].is_actor = is_actor;
            }
            return Ok(i);
        }
        if self.g.participants.len() >= MAX_PARTICIPANTS {
            return Err(self.err(format!(
                "diagram too large: more than {MAX_PARTICIPANTS} participants"
            )));
        }
        let i = self.g.participants.len();
        self.g.participants.push(Participant {
            id: id.to_string(),
            display: display.unwrap_or_else(|| id.to_string()),
            is_actor,
        });
        self.ids.insert(id.to_string(), i);
        self.active_depth.push(0);
        Ok(i)
    }

    fn parse_statement(&mut self, stmt: &str) -> Result<(), ParseError> {
        let first = stmt.split_whitespace().next().unwrap_or("");
        match first {
            "participant" | "actor" => return self.parse_participant(stmt, first == "actor"),
            "autonumber" => return self.push_event(Event::Autonumber),
            // Task 4 adds: activate/deactivate, Note, fragments, end,
            // out-of-scope keywords. Until then:
            _ => {}
        }
        if self.try_parse_message(stmt)? {
            return Ok(());
        }
        Err(self.err(format!("unsupported statement: {first:?}")))
    }

    fn parse_participant(&mut self, stmt: &str, is_actor: bool) -> Result<(), ParseError> {
        let rest = stmt
            .split_once(char::is_whitespace)
            .map(|(_, r)| r.trim())
            .unwrap_or("");
        if rest.is_empty() {
            return Err(self.err("participant needs an id"));
        }
        let (id, display) = match rest.split_once(" as ") {
            Some((id, d)) => (id.trim(), Some(d.trim().to_string())),
            None => (rest, None),
        };
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(self.err(format!("invalid participant id {id:?}")));
        }
        if matches!(&display, Some(d) if d.is_empty()) {
            return Err(self.err("participant alias must not be empty"));
        }
        self.intern(id, display, is_actor)?;
        Ok(())
    }

    /// Try `IDENT ARROW [+|-] IDENT [: text]`. Returns Ok(false) if the
    /// statement doesn't start with an id followed by an arrow.
    fn try_parse_message(&mut self, stmt: &str) -> Result<bool, ParseError> {
        // Leading id: ASCII-only predicate, so char count == byte length.
        let id_len = stmt
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if id_len == 0 {
            return Ok(false);
        }
        let from_id = &stmt[..id_len];
        let rest = stmt[id_len..].trim_start();
        // Arrows longest-first; each maps to (line, head).
        const ARROWS: &[(&str, LineStyle, Head)] = &[
            ("-->>", LineStyle::Dotted, Head::Arrow),
            ("-->", LineStyle::Dotted, Head::None),
            ("->>", LineStyle::Solid, Head::Arrow),
            ("->", LineStyle::Solid, Head::None),
            ("--x", LineStyle::Dotted, Head::Cross),
            ("-x", LineStyle::Solid, Head::Cross),
            ("--)", LineStyle::Dotted, Head::Async),
            ("-)", LineStyle::Solid, Head::Async),
        ];
        let Some((arrow, line_style, head)) = ARROWS
            .iter()
            .find(|(a, _, _)| rest.starts_with(a))
            .map(|(a, l, h)| (*a, *l, *h))
        else {
            return Ok(false);
        };
        let mut after = rest[arrow.len()..].trim_start();
        let mut activate_target = false;
        let mut deactivate_source = false;
        if let Some(r) = after.strip_prefix('+') {
            activate_target = true;
            after = r;
        } else if let Some(r) = after.strip_prefix('-') {
            deactivate_source = true;
            after = r;
        }
        let to_len = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        if to_len == 0 {
            return Err(self.err("expected a target participant after the arrow"));
        }
        let to_id = &after[..to_len];
        let tail = after[to_len..].trim_start();
        let text = match tail.strip_prefix(':') {
            Some(t) => t.trim().to_string(),
            None if tail.is_empty() => String::new(),
            None => {
                return Err(self.err(format!(
                    "unexpected text after message target: {tail:?}"
                )))
            }
        };
        let from = self.intern(from_id, None, false)?;
        let to = self.intern(to_id, None, false)?;
        if activate_target {
            self.active_depth[to] += 1;
        }
        if deactivate_source {
            if self.active_depth[from] == 0 {
                let id = self.g.participants[from].id.clone();
                return Err(self.err(format!("cannot deactivate {id:?}: not active")));
            }
            self.active_depth[from] -= 1;
        }
        self.push_event(Event::Message {
            from,
            to,
            line: line_style,
            head,
            text,
            activate_target,
            deactivate_source,
        })?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::{Event, Head, LineStyle};

    fn p(src: &str) -> crate::sequence::SeqDiagram {
        parse(src).expect("parse ok")
    }

    fn msg(e: &Event) -> (usize, usize, LineStyle, Head, &str) {
        match e {
            Event::Message { from, to, line, head, text, .. } => (*from, *to, *line, *head, text.as_str()),
            other => panic!("expected message, got {other:?}"),
        }
    }

    #[test]
    fn header_required() {
        let e = parse("A->>B: hi").unwrap_err();
        assert_eq!(e.line, Some(1));
        assert!(parse("sequenceDiagram\nA->>B: hi").is_ok());
        assert!(parse("sequenceDiagram;\nA->>B: hi").is_ok()); // trailing ;
    }

    #[test]
    fn participant_declarations() {
        let g = p("sequenceDiagram\nparticipant A\nparticipant B as Bob Smith\nactor C as Carol");
        assert_eq!(g.participants.len(), 3);
        assert_eq!(g.participants[0].display, "A");
        assert_eq!(g.participants[1].display, "Bob Smith");
        assert!(!g.participants[1].is_actor);
        assert!(g.participants[2].is_actor);
        assert_eq!(g.participants[2].id, "C");
    }

    #[test]
    fn implicit_participants_in_declaration_order() {
        let g = p("sequenceDiagram\nZed->>Amy: hi\nAmy-->>Zed: yo");
        assert_eq!(g.participants[0].id, "Zed"); // first appearance wins
        assert_eq!(g.participants[1].id, "Amy");
        assert_eq!(g.participants.len(), 2);
    }

    #[test]
    fn all_arrow_forms() {
        let cases: &[(&str, LineStyle, Head)] = &[
            ("A->B: t", LineStyle::Solid, Head::None),
            ("A-->B: t", LineStyle::Dotted, Head::None),
            ("A->>B: t", LineStyle::Solid, Head::Arrow),
            ("A-->>B: t", LineStyle::Dotted, Head::Arrow),
            ("A-xB: t", LineStyle::Solid, Head::Cross),
            ("A--xB: t", LineStyle::Dotted, Head::Cross),
            ("A-)B: t", LineStyle::Solid, Head::Async),
            ("A--)B: t", LineStyle::Dotted, Head::Async),
        ];
        for (src, want_line, want_head) in cases {
            let g = p(&format!("sequenceDiagram\n{src}"));
            let (_, _, line, head, text) = msg(&g.events[0]);
            assert_eq!(line, *want_line, "for {src}");
            assert_eq!(head, *want_head, "for {src}");
            assert_eq!(text, "t", "for {src}");
        }
    }

    #[test]
    fn message_without_text() {
        let g = p("sequenceDiagram\nA->>B");
        assert_eq!(msg(&g.events[0]).4, "");
    }

    #[test]
    fn self_message() {
        let g = p("sequenceDiagram\nA->>A: think");
        let (from, to, ..) = msg(&g.events[0]);
        assert_eq!(from, to);
    }

    #[test]
    fn activation_shorthand() {
        let g = p("sequenceDiagram\nA->>+B: go\nB-->>-A: done");
        match &g.events[0] {
            Event::Message { activate_target, deactivate_source, .. } => {
                assert!(*activate_target && !*deactivate_source);
            }
            _ => unreachable!(),
        }
        match &g.events[1] {
            Event::Message { activate_target, deactivate_source, .. } => {
                assert!(!*activate_target && *deactivate_source);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn deactivate_shorthand_without_active_errors() {
        let e = parse("sequenceDiagram\nB-->>-A: done").unwrap_err();
        assert_eq!(e.line, Some(2));
        assert!(e.message.contains("not active"), "got: {}", e.message);
    }

    #[test]
    fn autonumber_event() {
        let g = p("sequenceDiagram\nautonumber\nA->>B: hi");
        assert!(matches!(g.events[0], Event::Autonumber));
    }

    #[test]
    fn comments_and_blanks_skipped() {
        let g = p("sequenceDiagram\n%% c\n\nA->>B: hi");
        assert_eq!(g.events.len(), 1);
    }

    #[test]
    fn participant_cap_enforced() {
        let mut src = String::from("sequenceDiagram\n");
        for i in 0..=crate::sequence::MAX_PARTICIPANTS {
            src.push_str(&format!("participant p{i}\n"));
        }
        let e = parse(&src).unwrap_err();
        assert!(e.message.contains("too large"));
    }

    #[test]
    fn event_cap_enforced() {
        let mut src = String::from("sequenceDiagram\n");
        for _ in 0..=crate::sequence::MAX_EVENTS {
            src.push_str("A->>B: x\n");
        }
        let e = parse(&src).unwrap_err();
        assert!(e.message.contains("too large"));
    }

    #[test]
    fn unknown_statement_errors_with_line() {
        let e = parse("sequenceDiagram\nA->>B: ok\nwibble wobble").unwrap_err();
        assert_eq!(e.line, Some(3));
    }

    #[test]
    fn multibyte_input_no_panic() {
        // Multi-byte whitespace and emoji in display/text must not panic.
        let _ = parse("sequenceDiagram\nparticipant A as Émile 🎭\nA->>B: héllo 🎉");
        let _ = parse("sequenceDiagram\nA\u{2003}->>B: x");
    }
}
