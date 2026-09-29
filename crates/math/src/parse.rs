// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Recursive-descent LaTeX math parser producing a MathML tree.
//!
//! The grammar is LaTeX's math mode: a sequence of atoms, each optionally
//! followed by `^`/`_`/`'` scripts, where an atom is a character, a braced
//! group, or a command with its arguments. Anything outside the supported
//! set is an error naming the offending command — never a silent drop or
//! a literal render — so a typo is visible instead of quietly wrong.

use crate::fonts::Font;
use crate::mathml::Node;
use crate::symbols::{self, Sym};
use crate::MathError;

/// Deepest nesting of groups, arguments and atoms (a braced argument
/// counts about three). Every level recurses; the source cap alone would
/// allow thousands, enough to exhaust a 1 MB (WASM) stack. Measured at
/// under 7 KB of stack per level in debug builds, so 60 stays well
/// inside it; real equations nest a handful of braces deep.
pub(crate) const MAX_DEPTH: usize = 60;

/// What ended a run of atoms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Eof,
    /// `}` closing a group.
    Brace,
    /// `]` closing an optional argument (`\sqrt[3]{x}`).
    Bracket,
    /// `\right`.
    Right,
    /// `\end`.
    End,
    /// `&` between table cells.
    Amp,
    /// `\\` between table rows.
    Newline,
}

/// How sub/superscripts attach to an atom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Limits {
    /// Beside the atom (`msub`/`msup`).
    Side,
    /// Above/below in display style, beside inline (`\sum`, `\lim`).
    Movable,
    /// Always above/below (`\overbrace`, `\int\limits`).
    Always,
}

struct Atom {
    node: Node,
    limits: Limits,
    /// Whether `\limits`/`\nolimits` may follow (operators only).
    op: bool,
    /// Emitted after the scripts: function application after `\sin`.
    after: Option<Node>,
}

impl Atom {
    fn plain(node: Node) -> Atom {
        Atom { node, limits: Limits::Side, op: false, after: None }
    }
}

pub(crate) struct Parser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
    font: Font,
}

/// `\frac`-family: (display style forced?).
fn frac_style(name: &str) -> Option<Option<bool>> {
    Some(match name {
        "frac" => None,
        "dfrac" | "cfrac" => Some(true),
        "tfrac" => Some(false),
        _ => return None,
    })
}

fn font_command(name: &str) -> Option<Font> {
    Some(match name {
        "mathbf" => Font::Bold,
        "mathit" => Font::Italic,
        "mathrm" | "mathup" => Font::Upright,
        "mathbb" => Font::DoubleStruck,
        "mathcal" | "mathscr" => Font::Script,
        "mathfrak" => Font::Fraktur,
        "mathsf" => Font::SansSerif,
        "mathtt" => Font::Monospace,
        "boldsymbol" | "bm" => Font::BoldItalic,
        "mathnormal" => Font::Normal,
        _ => return None,
    })
}

fn text_font(name: &str) -> Option<Font> {
    Some(match name {
        "text" | "textrm" | "textnormal" | "textup" | "mbox" | "hbox" => Font::Normal,
        "textbf" => Font::Bold,
        "textit" | "emph" => Font::Italic,
        "textsf" => Font::SansSerif,
        "texttt" => Font::Monospace,
        _ => return None,
    })
}

fn space_width(name: &str) -> Option<&'static str> {
    Some(match name {
        "," | "thinspace" => "0.1667em",
        ":" | ">" | "medspace" => "0.2222em",
        ";" | "thickspace" => "0.2778em",
        "enspace" => "0.5em",
        "quad" => "1em",
        "qquad" => "2em",
        _ => return None,
    })
}

/// Rest-of-group switches (`{\bf x}`, `\displaystyle`, `\color{red}`).
enum Switch {
    Font(Font),
    /// `mstyle` attributes.
    Style(&'static [(&'static str, &'static str)]),
    Color(String),
}

fn big_size(name: &str) -> Option<&'static str> {
    let base = name.trim_end_matches(['l', 'r', 'm']);
    if base.len() + 1 < name.len() {
        return None; // at most one l/r/m suffix
    }
    Some(match base {
        "big" => "1.2em",
        "Big" => "1.8em",
        "bigg" => "2.4em",
        "Bigg" => "3em",
        _ => return None,
    })
}

fn primes(n: usize) -> String {
    match n {
        1 => "′".into(),
        2 => "″".into(),
        3 => "‴".into(),
        4 => "⁗".into(),
        _ => "′".repeat(n),
    }
}

fn fence(c: char, form: &'static str) -> Node {
    Node::mo(c).with("fence", "true").with("form", form).with("stretchy", "true")
}

fn is_empty_row(n: &Node) -> bool {
    matches!(n, Node::El { tag: "mrow", children, .. } if children.is_empty())
}

impl Parser {
    pub(crate) fn new(src: &str) -> Parser {
        Parser { chars: src.chars().collect(), pos: 0, depth: 0, font: Font::Normal }
    }

    pub(crate) fn parse(mut self) -> Result<Node, MathError> {
        let (nodes, _) = self.expr(&[Stop::Eof])?;
        Ok(Node::row(nodes))
    }

    fn err<T>(&self, message: impl Into<String>) -> Result<T, MathError> {
        Err(MathError { message: message.into(), offset: self.pos })
    }

    fn err_at<T>(&self, offset: usize, message: impl Into<String>) -> Result<T, MathError> {
        Err(MathError { message: message.into(), offset })
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.pos + n).copied()
    }

    /// Skip whitespace and `%` comments.
    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
            } else if c == '%' {
                while self.peek().is_some_and(|c| c != '\n') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn enter(&mut self) -> Result<(), MathError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return self.err(format!("equation nested too deeply (more than {MAX_DEPTH} levels)"));
        }
        Ok(())
    }

    /// Consume `\name` and return `name`: a run of letters, or one
    /// non-letter character (`\,`, `\{`, `\\`).
    fn command_name(&mut self) -> Result<String, MathError> {
        debug_assert_eq!(self.peek(), Some('\\'));
        self.pos += 1;
        let Some(c) = self.peek() else {
            return self.err("`\\` at the end of the equation");
        };
        if c.is_ascii_alphabetic() {
            let start = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
                self.pos += 1;
            }
            Ok(self.chars[start..self.pos].iter().collect())
        } else {
            self.pos += 1;
            Ok(c.to_string())
        }
    }

    /// Whether `\name` (not followed by another letter) is next.
    fn at_command(&self, name: &str) -> bool {
        if self.peek() != Some('\\') {
            return false;
        }
        let mut i = self.pos + 1;
        for expected in name.chars() {
            if self.chars.get(i) != Some(&expected) {
                return false;
            }
            i += 1;
        }
        !self.chars.get(i).is_some_and(|c| c.is_ascii_alphabetic())
    }

    // ---- sequences --------------------------------------------------------

    fn expr(&mut self, stops: &[Stop]) -> Result<(Vec<Node>, Stop), MathError> {
        self.enter()?;
        let result = self.expr_inner(stops);
        self.depth -= 1;
        result
    }

    fn expr_inner(&mut self, stops: &[Stop]) -> Result<(Vec<Node>, Stop), MathError> {
        let mut nodes = Vec::new();
        loop {
            self.skip_ws();
            let Some(c) = self.peek() else {
                if stops.contains(&Stop::Eof) {
                    return Ok((nodes, Stop::Eof));
                }
                return self.err(if stops.contains(&Stop::Brace) {
                    "missing `}`"
                } else if stops.contains(&Stop::Bracket) {
                    "missing `]`"
                } else if stops.contains(&Stop::Right) {
                    "`\\left` without a matching `\\right`"
                } else {
                    "`\\begin` without a matching `\\end`"
                });
            };
            match c {
                '}' => {
                    if stops.contains(&Stop::Brace) {
                        self.pos += 1;
                        return Ok((nodes, Stop::Brace));
                    }
                    return self.err("unexpected `}`");
                }
                ']' if stops.contains(&Stop::Bracket) => {
                    self.pos += 1;
                    return Ok((nodes, Stop::Bracket));
                }
                '&' => {
                    if stops.contains(&Stop::Amp) {
                        self.pos += 1;
                        return Ok((nodes, Stop::Amp));
                    }
                    return self.err("`&` only works inside an environment such as `aligned` or `matrix`");
                }
                '\\' => {
                    let start = self.pos;
                    let name = self.command_name()?;
                    match name.as_str() {
                        "\\" | "cr" => {
                            if stops.contains(&Stop::Newline) {
                                self.skip_row_spacing();
                                return Ok((nodes, Stop::Newline));
                            }
                            return self.err_at(
                                start,
                                "`\\\\` line breaks only work inside an environment such as `aligned`",
                            );
                        }
                        "right" => {
                            if stops.contains(&Stop::Right) {
                                return Ok((nodes, Stop::Right));
                            }
                            return self.err_at(start, "`\\right` without a matching `\\left`");
                        }
                        "end" => {
                            if stops.contains(&Stop::End) {
                                return Ok((nodes, Stop::End));
                            }
                            return self.err_at(start, "`\\end` without a matching `\\begin`");
                        }
                        _ => {}
                    }
                    self.pos = start;
                    if let Some(switch) = self.switch()? {
                        let (rest, stop) = match switch {
                            Switch::Font(font) => {
                                let saved = self.font;
                                self.font = font;
                                let rest = self.expr(stops);
                                self.font = saved;
                                let (rest, stop) = rest?;
                                (Node::row(rest), stop)
                            }
                            Switch::Style(attrs) => {
                                let (rest, stop) = self.expr(stops)?;
                                let mut style = Node::el("mstyle", rest);
                                for (k, v) in attrs {
                                    style = style.with(k, *v);
                                }
                                (style, stop)
                            }
                            Switch::Color(color) => {
                                let (rest, stop) = self.expr(stops)?;
                                (Node::el("mstyle", rest).with("mathcolor", color), stop)
                            }
                        };
                        nodes.push(rest);
                        return Ok((nodes, stop));
                    }
                }
                _ => {}
            }
            nodes.extend(self.scripted_atom()?);
        }
    }

    /// After `\\`: skip an optional `[length]` row gap.
    fn skip_row_spacing(&mut self) {
        let save = self.pos;
        self.skip_ws();
        if self.peek() == Some('[') {
            while let Some(c) = self.peek() {
                self.pos += 1;
                if c == ']' {
                    return;
                }
            }
        }
        self.pos = save;
    }

    /// A rest-of-group switch, consumed, or `None` (nothing consumed).
    fn switch(&mut self) -> Result<Option<Switch>, MathError> {
        let save = self.pos;
        let name = self.command_name()?;
        let switch = match name.as_str() {
            "displaystyle" => Switch::Style(&[("displaystyle", "true"), ("scriptlevel", "0")]),
            "textstyle" => Switch::Style(&[("displaystyle", "false"), ("scriptlevel", "0")]),
            "scriptstyle" => Switch::Style(&[("displaystyle", "false"), ("scriptlevel", "1")]),
            "scriptscriptstyle" => Switch::Style(&[("displaystyle", "false"), ("scriptlevel", "2")]),
            "rm" => Switch::Font(Font::Upright),
            "bf" => Switch::Font(Font::Bold),
            "it" => Switch::Font(Font::Italic),
            "sf" => Switch::Font(Font::SansSerif),
            "tt" => Switch::Font(Font::Monospace),
            "cal" => Switch::Font(Font::Script),
            "color" => Switch::Color(self.color()?),
            _ => {
                self.pos = save;
                return Ok(None);
            }
        };
        Ok(Some(switch))
    }

    // ---- atoms and scripts -----------------------------------------------

    fn scripted_atom(&mut self) -> Result<Vec<Node>, MathError> {
        let mut atom = match self.peek() {
            // A script with nothing before it attaches to an empty base.
            Some('^' | '_' | '\'') => Atom::plain(Node::row(Vec::new())),
            _ => self.atom()?,
        };
        loop {
            self.skip_ws();
            let start = self.pos;
            if self.at_command("limits") || self.at_command("nolimits") {
                let name = self.command_name()?;
                if !atom.op {
                    return self.err_at(start, format!("`\\{name}` must follow an operator such as `\\sum`"));
                }
                if name == "limits" {
                    atom.limits = Limits::Always;
                    if atom.node.tag() == "mo" {
                        atom.node.set_attr("movablelimits", "false");
                    }
                } else {
                    atom.limits = Limits::Side;
                }
            } else {
                break;
            }
        }
        let node = self.scripts(atom.node, atom.limits)?;
        let mut out = vec![node];
        out.extend(atom.after);
        Ok(out)
    }

    fn scripts(&mut self, base: Node, limits: Limits) -> Result<Node, MathError> {
        let mut sub = None;
        let mut sup = None;
        let mut prime_count = 0;
        loop {
            self.skip_ws();
            match self.peek() {
                Some('^') => {
                    if sup.is_some() {
                        return self.err("double superscript: group with braces, e.g. `x^{ab}`");
                    }
                    self.pos += 1;
                    sup = Some(self.arg()?);
                }
                Some('_') => {
                    if sub.is_some() {
                        return self.err("double subscript: group with braces, e.g. `x_{ab}`");
                    }
                    self.pos += 1;
                    sub = Some(self.arg()?);
                }
                Some('\'') => {
                    if sup.is_some() {
                        return self.err("a prime must come before the superscript, e.g. `f'^2`");
                    }
                    self.pos += 1;
                    prime_count += 1;
                }
                _ => break,
            }
        }
        if prime_count > 0 {
            let p = Node::mo(primes(prime_count));
            sup = Some(match sup {
                None => p,
                Some(s) => Node::row(vec![p, s]),
            });
        }
        let (tag, kids) = match (limits, sub, sup) {
            (_, None, None) => return Ok(base),
            (Limits::Side, Some(b), None) => ("msub", vec![base, b]),
            (Limits::Side, None, Some(p)) => ("msup", vec![base, p]),
            (Limits::Side, Some(b), Some(p)) => ("msubsup", vec![base, b, p]),
            (_, Some(b), None) => ("munder", vec![base, b]),
            (_, None, Some(p)) => ("mover", vec![base, p]),
            (_, Some(b), Some(p)) => ("munderover", vec![base, b, p]),
        };
        Ok(Node::el(tag, kids))
    }

    /// A mandatory argument: a braced group or a single token.
    fn arg(&mut self) -> Result<Node, MathError> {
        self.skip_ws();
        match self.peek() {
            None | Some('}' | '&' | '^' | '_') => self.err("missing argument"),
            Some('{') => {
                self.enter()?;
                self.pos += 1;
                let group = self.expr(&[Stop::Brace]);
                self.depth -= 1;
                Ok(Node::row(group?.0))
            }
            Some('\\') => {
                let atom = self.atom()?;
                Ok(match atom.after {
                    Some(after) => Node::row(vec![atom.node, after]),
                    None => atom.node,
                })
            }
            Some(c) => {
                self.pos += 1;
                self.char_node(c)
            }
        }
    }

    fn atom(&mut self) -> Result<Atom, MathError> {
        self.enter()?;
        let atom = self.atom_inner();
        self.depth -= 1;
        atom
    }

    fn atom_inner(&mut self) -> Result<Atom, MathError> {
        let Some(c) = self.peek() else {
            return self.err("missing argument");
        };
        match c {
            '{' => {
                self.pos += 1;
                let (group, _) = self.expr(&[Stop::Brace])?;
                Ok(Atom::plain(Node::row(group)))
            }
            '\\' => self.command(),
            c if c.is_ascii_digit() || (c == '.' && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())) => {
                let start = self.pos;
                while let Some(d) = self.peek() {
                    let decimal_point = d == '.' && self.peek_at(1).is_some_and(|n| n.is_ascii_digit());
                    if d.is_ascii_digit() || decimal_point {
                        self.pos += 1;
                    } else {
                        break;
                    }
                }
                let digits: String = self.chars[start..self.pos].iter().collect();
                Ok(Atom::plain(Node::mn(self.font.map_str(&digits))))
            }
            c => {
                self.pos += 1;
                Ok(Atom::plain(self.char_node(c)?))
            }
        }
    }

    /// One character of source as a MathML token.
    fn char_node(&self, c: char) -> Result<Node, MathError> {
        let at = self.pos.saturating_sub(1);
        Ok(match c {
            c if c.is_alphabetic() => self.ident(c),
            c if c.is_ascii_digit() => Node::mn(self.font.map(c).to_string()),
            '-' => Node::mo('−'),
            '*' => Node::mo('∗'),
            '\'' => Node::mo('′'),
            '~' => Node::mtext('\u{a0}'),
            '(' | ')' | '[' | ']' | '/' => Node::mo(c).with("stretchy", "false"),
            '|' => bar('|'),
            '$' => return self.err_at(at, "`$` isn't needed inside an equation"),
            '#' => return self.err_at(at, "`#` isn't supported; write `\\#` for the symbol"),
            '{' | '}' | '\\' | '^' | '_' | '&' => {
                return self.err_at(at, format!("unexpected `{c}`"));
            }
            c if c.is_numeric() => Node::mn(c),
            c => Node::mo(c),
        })
    }

    fn ident(&self, c: char) -> Node {
        match self.font {
            Font::Normal => Node::mi(c),
            Font::Upright => Node::mi_upright(c),
            font => Node::mi(font.map(c)),
        }
    }

    // ---- commands ---------------------------------------------------------

    // Command handling is split into small functions on purpose: the
    // parser recurses through `command` for every nested argument, and in
    // unoptimized (debug / dev WASM) builds a function's frame holds every
    // local of every match arm. One big dispatcher cost ~24 KB of stack
    // per nesting level; these keep the recursive path's frames small.

    fn command(&mut self) -> Result<Atom, MathError> {
        let start = self.pos;
        let name = self.command_name()?;
        if let Some(atom) = self.simple_command(&name) {
            return Ok(atom);
        }
        self.structural_command(&name, start)
    }

    /// Commands that take no arguments: symbols, spaces, functions.
    fn simple_command(&self, name: &str) -> Option<Atom> {
        if let Some(width) = space_width(name) {
            return Some(Atom::plain(Node::mspace(width)));
        }
        match name {
            "{" | "}" => return Some(Atom::plain(Node::mo(name).with("stretchy", "false"))),
            "|" => return Some(Atom::plain(bar('‖'))),
            "#" | "$" | "%" | "&" | "_" => return Some(Atom::plain(Node::mo(name))),
            " " => return Some(Atom::plain(Node::mtext('\u{a0}'))),
            "!" => return Some(Atom::plain(Node::mspace("-0.1667em"))),
            "bmod" => return Some(Atom::plain(Node::mo("mod"))),
            "mod" => {
                return Some(Atom::plain(Node::el(
                    "mrow",
                    vec![Node::mspace("1em"), Node::mi_upright("mod"), Node::mspace("0.3333em")],
                )))
            }
            // No-ops: we never number equations.
            "nonumber" | "notag" => return Some(Atom::plain(Node::row(Vec::new()))),
            _ => {}
        }
        if let Some(sym) = symbols::symbol(name) {
            return Some(match sym {
                Sym::Ident(c) => Atom::plain(self.ident(c)),
                Sym::Upright(c) => Atom::plain(Node::mi_upright(c)),
                Sym::Op(c) => Atom::plain(Node::mo(c)),
                Sym::Delim(c @ ('|' | '‖')) => Atom::plain(bar(c)),
                Sym::Delim(c) => Atom::plain(Node::mo(c).with("stretchy", "false")),
                Sym::BigOp(c) => Atom {
                    node: Node::mo(c).with("movablelimits", "true"),
                    limits: Limits::Movable,
                    op: true,
                    after: None,
                },
                Sym::Integral(c) => Atom { node: Node::mo(c), limits: Limits::Side, op: true, after: None },
            });
        }
        if let Some(f) = symbols::function(name) {
            return Some(function_atom(f));
        }
        if let Some(f) = symbols::limit_function(name) {
            return Some(Atom { node: limit_op(f), limits: Limits::Movable, op: true, after: None });
        }
        None
    }

    /// Commands with arguments (or errors). Each arm delegates so this
    /// frame stays small while the argument parse recurses.
    fn structural_command(&mut self, name: &str, start: usize) -> Result<Atom, MathError> {
        if let Some(accent) = symbols::accent(name) {
            return self.cmd_accent(accent).map(Atom::plain);
        }
        if let Some(style) = frac_style(name) {
            return self.cmd_frac(style).map(Atom::plain);
        }
        if let Some(font) = font_command(name) {
            return self.cmd_font(font).map(Atom::plain);
        }
        if let Some(font) = text_font(name) {
            return self.cmd_text(name, font).map(Atom::plain);
        }
        if let Some(size) = big_size(name) {
            return self.cmd_big(name, size).map(Atom::plain);
        }
        match name {
            "binom" | "dbinom" | "tbinom" => self.cmd_binom(name).map(Atom::plain),
            "sqrt" => self.cmd_sqrt().map(Atom::plain),
            "operatorname" => self.cmd_operatorname(),
            "left" => self.cmd_left().map(Atom::plain),
            "middle" => self.cmd_middle().map(Atom::plain),
            "begin" => self.environment().map(Atom::plain),
            "overset" | "stackrel" | "underset" => self.cmd_stack(name).map(Atom::plain),
            "overbrace" | "underbrace" => self.cmd_brace(name),
            "not" => self.cmd_not().map(Atom::plain),
            "pmod" => self.cmd_pmod().map(Atom::plain),
            "phantom" | "hphantom" | "vphantom" => self.cmd_phantom(name).map(Atom::plain),
            "textcolor" => self.cmd_textcolor().map(Atom::plain),
            "limits" | "nolimits" => {
                self.err_at(start, format!("`\\{name}` must follow an operator such as `\\sum`"))
            }
            "displaystyle" | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "rm" | "bf" | "it"
            | "sf" | "tt" | "cal" | "color" => {
                self.err_at(start, format!("`\\{name}` can't be used as an argument; wrap it in braces"))
            }
            "\\" | "cr" | "right" | "end" => self.err_at(start, format!("unexpected `\\{name}`")),
            "tag" | "label" => self.err_at(start, "equation numbers and labels aren't supported"),
            "hline" => self.err_at(start, "`\\hline` isn't supported"),
            _ => self.err_at(start, format!("unknown command `\\{name}`")),
        }
    }

    fn cmd_accent(&mut self, (c, stretchy, below): (char, bool, bool)) -> Result<Node, MathError> {
        let base = self.arg()?;
        let mark = Node::mo(c).with("stretchy", if stretchy { "true" } else { "false" });
        Ok(if below {
            Node::el("munder", vec![base, mark]).with("accentunder", "true")
        } else {
            Node::el("mover", vec![base, mark]).with("accent", "true")
        })
    }

    fn cmd_frac(&mut self, style: Option<bool>) -> Result<Node, MathError> {
        let num = self.arg()?;
        let den = self.arg()?;
        Ok(with_display(Node::el("mfrac", vec![num, den]), style))
    }

    fn cmd_font(&mut self, font: Font) -> Result<Node, MathError> {
        let saved = self.font;
        self.font = font;
        let arg = self.arg();
        self.font = saved;
        arg
    }

    fn cmd_text(&mut self, name: &str, font: Font) -> Result<Node, MathError> {
        let text = self.text_group(name)?;
        Ok(Node::mtext(keep_edge_spaces(&font.map_str(&text))))
    }

    fn cmd_big(&mut self, name: &str, size: &'static str) -> Result<Node, MathError> {
        Ok(match self.delimiter(name)? {
            Some(c) => Node::mo(c)
                .with("stretchy", "true")
                .with("symmetric", "true")
                .with("minsize", size)
                .with("maxsize", size),
            None => Node::row(Vec::new()),
        })
    }

    fn cmd_binom(&mut self, name: &str) -> Result<Node, MathError> {
        let n = self.arg()?;
        let k = self.arg()?;
        let style = match name {
            "dbinom" => Some(true),
            "tbinom" => Some(false),
            _ => None,
        };
        let frac = Node::el("mfrac", vec![n, k]).with("linethickness", "0");
        Ok(with_display(Node::el("mrow", vec![fence('(', "prefix"), frac, fence(')', "postfix")]), style))
    }

    fn cmd_sqrt(&mut self) -> Result<Node, MathError> {
        self.skip_ws();
        if self.peek() == Some('[') {
            self.pos += 1;
            let (index, _) = self.expr(&[Stop::Bracket])?;
            let radicand = self.arg()?;
            Ok(Node::el("mroot", vec![radicand, Node::row(index)]))
        } else {
            Ok(Node::el("msqrt", vec![self.arg()?]))
        }
    }

    fn cmd_operatorname(&mut self) -> Result<Atom, MathError> {
        let starred = self.peek() == Some('*');
        if starred {
            self.pos += 1;
        }
        let text = self.text_group("operatorname")?;
        Ok(if starred {
            Atom { node: limit_op(&text), limits: Limits::Movable, op: true, after: None }
        } else {
            function_atom(&text)
        })
    }

    fn cmd_left(&mut self) -> Result<Node, MathError> {
        let open = self.delimiter("left")?;
        let (body, _) = self.expr(&[Stop::Right])?;
        let close = self.delimiter("right")?;
        let mut kids = Vec::with_capacity(body.len() + 2);
        kids.extend(open.map(|c| fence(c, "prefix")));
        kids.extend(body);
        kids.extend(close.map(|c| fence(c, "postfix")));
        Ok(Node::el("mrow", kids))
    }

    fn cmd_middle(&mut self) -> Result<Node, MathError> {
        Ok(match self.delimiter("middle")? {
            Some(c) => Node::mo(c).with("fence", "true").with("stretchy", "true"),
            None => Node::row(Vec::new()),
        })
    }

    fn cmd_stack(&mut self, name: &str) -> Result<Node, MathError> {
        let script = self.arg()?;
        let base = self.arg()?;
        Ok(Node::el(if name == "underset" { "munder" } else { "mover" }, vec![base, script]))
    }

    fn cmd_brace(&mut self, name: &str) -> Result<Atom, MathError> {
        let base = self.arg()?;
        let (tag, brace) = if name == "overbrace" { ("mover", '⏞') } else { ("munder", '⏟') };
        let node = Node::el(tag, vec![base, Node::mo(brace).with("stretchy", "true")]);
        Ok(Atom { node, limits: Limits::Always, op: false, after: None })
    }

    fn cmd_not(&mut self) -> Result<Node, MathError> {
        self.skip_ws();
        let at = self.pos;
        let target = self.atom()?.node;
        match (target.tag(), target.text()) {
            ("mo" | "mi", Some(t)) if t.chars().count() == 1 => {
                let c = t.chars().next().expect("one char");
                Ok(Node::mo(symbols::negate(c)))
            }
            _ => self.err_at(at, "`\\not` needs a relation after it, e.g. `\\not=`"),
        }
    }

    fn cmd_pmod(&mut self) -> Result<Node, MathError> {
        let n = self.arg()?;
        Ok(Node::el(
            "mrow",
            vec![
                Node::mspace("1em"),
                Node::mo('(').with("stretchy", "false"),
                Node::mi_upright("mod"),
                Node::mspace("0.3333em"),
                n,
                Node::mo(')').with("stretchy", "false"),
            ],
        ))
    }

    fn cmd_phantom(&mut self, name: &str) -> Result<Node, MathError> {
        let phantom = Node::el("mphantom", vec![self.arg()?]);
        Ok(match name {
            "hphantom" => Node::el("mpadded", vec![phantom]).with("height", "0").with("depth", "0"),
            "vphantom" => Node::el("mpadded", vec![phantom]).with("width", "0"),
            _ => phantom,
        })
    }

    fn cmd_textcolor(&mut self) -> Result<Node, MathError> {
        let color = self.color()?;
        let body = self.arg()?;
        Ok(Node::el("mstyle", vec![body]).with("mathcolor", color))
    }

    /// The delimiter after `\left`, `\right`, `\big`, …; `None` for `.`.
    fn delimiter(&mut self, after: &str) -> Result<Option<char>, MathError> {
        self.skip_ws();
        let at = self.pos;
        match self.peek() {
            None => self.err(format!("missing delimiter after `\\{after}`")),
            Some('\\') => {
                let name = self.command_name()?;
                match symbols::delimiter_command(&name) {
                    Some(c) => Ok(Some(c)),
                    None => self.err_at(at, format!("`\\{name}` isn't a delimiter")),
                }
            }
            Some(c) => match symbols::delimiter_char(c) {
                Some(d) => {
                    self.pos += 1;
                    Ok(d)
                }
                None => self.err_at(at, format!("`{c}` isn't a delimiter")),
            },
        }
    }

    /// The literal contents of a `{…}` group (`\text{…}`, environment and
    /// operator names): braces group, `\{` `\}` `\\$` etc. are the
    /// characters themselves, and other commands are errors.
    fn text_group(&mut self, command: &str) -> Result<String, MathError> {
        self.skip_ws();
        if self.peek() != Some('{') {
            return self.err(format!("`\\{command}` needs its text in braces, e.g. `\\{command}{{…}}`"));
        }
        self.pos += 1;
        let mut depth = 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return self.err("missing `}`");
            };
            self.pos += 1;
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(out);
                    }
                }
                '\\' => match self.peek() {
                    Some(e @ ('{' | '}' | '$' | '&' | '#' | '%' | '_' | ' ')) => {
                        self.pos += 1;
                        out.push(e);
                    }
                    // Spacing commands (`\operatorname*{arg\,max}`).
                    Some(',' | ':' | ';') => {
                        self.pos += 1;
                        out.push('\u{2009}');
                    }
                    Some('!') => self.pos += 1,
                    _ => {
                        return self.err(format!("commands aren't supported inside `\\{command}{{…}}`"));
                    }
                },
                '$' => return self.err(format!("math inside `\\{command}{{…}}` isn't supported")),
                '~' => out.push('\u{a0}'),
                c => out.push(c),
            }
        }
    }

    /// `{name}` for `\color`/`\textcolor`: a color name or `#rgb`/`#rrggbb`.
    /// Only these characters reach the `mathcolor` attribute.
    fn color(&mut self) -> Result<String, MathError> {
        let at = self.pos;
        let raw = self.text_group("color")?;
        let color = raw.trim();
        let named = !color.is_empty() && color.len() <= 32 && color.chars().all(|c| c.is_ascii_alphabetic());
        let hex = color.strip_prefix('#').is_some_and(|h| {
            matches!(h.len(), 3 | 6) && h.chars().all(|c| c.is_ascii_hexdigit())
        });
        if named || hex {
            Ok(color.to_string())
        } else {
            self.err_at(at, format!("invalid color {color:?}; use a name like `red` or `#ff0000`"))
        }
    }

    // ---- environments -----------------------------------------------------

    fn environment(&mut self) -> Result<Node, MathError> {
        let at = self.pos;
        let name = self.text_group("begin")?;
        let Some(spec) = self.env_spec(&name, at)? else {
            // `equation`: a plain sequence, no table.
            let (body, _) = self.expr(&[Stop::End])?;
            self.end_environment(&name)?;
            return Ok(Node::row(body));
        };
        let rows = self.table_rows()?;
        self.end_environment(&name)?;
        Ok(build_table(spec, rows))
    }

    /// How an environment lays out its table; `None` for `equation`.
    fn env_spec(&mut self, name: &str, at: usize) -> Result<Option<TableSpec>, MathError> {
        let spec = |open, close, aligns, display, small| TableSpec { open, close, aligns, display, small };
        Ok(Some(match name {
            "matrix" => spec(None, None, Aligns::Center, false, false),
            "smallmatrix" => spec(None, None, Aligns::Center, false, true),
            "pmatrix" => spec(Some('('), Some(')'), Aligns::Center, false, false),
            "bmatrix" => spec(Some('['), Some(']'), Aligns::Center, false, false),
            "Bmatrix" => spec(Some('{'), Some('}'), Aligns::Center, false, false),
            "vmatrix" => spec(Some('|'), Some('|'), Aligns::Center, false, false),
            "Vmatrix" => spec(Some('‖'), Some('‖'), Aligns::Center, false, false),
            "cases" => spec(Some('{'), None, Aligns::Left, false, false),
            "rcases" => spec(None, Some('}'), Aligns::Left, false, false),
            "aligned" | "align" | "align*" | "split" => spec(None, None, Aligns::Alternating, true, false),
            "alignedat" | "alignat" | "alignat*" => {
                // Column-count argument; the alternation is the same.
                self.text_group(name)?;
                spec(None, None, Aligns::Alternating, true, false)
            }
            "gathered" | "gather" | "gather*" => spec(None, None, Aligns::Center, true, false),
            "array" => {
                let columns = self.text_group("begin{array}")?;
                let mut cols = Vec::new();
                for c in columns.chars() {
                    match c {
                        'l' => cols.push("left"),
                        'c' => cols.push("center"),
                        'r' => cols.push("right"),
                        // Vertical rules aren't drawn.
                        '|' | ' ' => {}
                        _ => return self.err_at(at, format!("unsupported array column `{c}`; use l, c or r")),
                    }
                }
                spec(None, None, Aligns::Columns(cols), false, false)
            }
            "equation" | "equation*" | "displaymath" => return Ok(None),
            _ => return self.err_at(at, format!("unknown environment `{name}`")),
        }))
    }

    fn table_rows(&mut self) -> Result<Vec<Vec<Node>>, MathError> {
        let mut rows = Vec::new();
        let mut row = Vec::new();
        loop {
            let (cell, stop) = self.expr(&[Stop::Amp, Stop::Newline, Stop::End])?;
            row.push(Node::row(cell));
            match stop {
                Stop::Amp => {}
                Stop::Newline => rows.push(std::mem::take(&mut row)),
                _ => {
                    rows.push(row);
                    break;
                }
            }
        }
        // A trailing `\\` before `\end` leaves an empty last row.
        if rows.len() > 1 && rows.last().is_some_and(|r| r.len() == 1 && is_empty_row(&r[0])) {
            rows.pop();
        }
        Ok(rows)
    }

    /// After the `\end` stop: `{name}` must match the `\begin`.
    fn end_environment(&mut self, name: &str) -> Result<(), MathError> {
        let at = self.pos;
        let end = self.text_group("end")?;
        if end != name {
            return self.err_at(at, format!("`\\begin{{{name}}}` ended by `\\end{{{end}}}`"));
        }
        Ok(())
    }
}

struct TableSpec {
    open: Option<char>,
    close: Option<char>,
    aligns: Aligns,
    display: bool,
    small: bool,
}

fn build_table(spec: TableSpec, rows: Vec<Vec<Node>>) -> Node {
    let alternating = matches!(spec.aligns, Aligns::Alternating);
    let mut trs = Vec::with_capacity(rows.len());
    for row in rows {
        let mut tds = Vec::with_capacity(row.len());
        for (i, mut cell) in row.into_iter().enumerate() {
            if alternating && i % 2 == 1 {
                // `a &= b`: the relation starts the cell, where MathML
                // would treat it as a prefix and drop its spacing.
                mark_leading_operator_infix(&mut cell);
            }
            let style = match &spec.aligns {
                Aligns::Alternating if i % 2 == 0 => Some("text-align:right;padding-right:0".to_string()),
                Aligns::Alternating => Some("text-align:left;padding-left:0".to_string()),
                Aligns::Left => Some("text-align:left".to_string()),
                Aligns::Columns(cols) => match cols.get(i).copied().unwrap_or("center") {
                    "center" => None,
                    a => Some(format!("text-align:{a}")),
                },
                Aligns::Center => None,
            };
            let mut td = Node::el("mtd", vec![cell]);
            if let Some(style) = style {
                td = td.with("style", style);
            }
            tds.push(td);
        }
        trs.push(Node::el("mtr", tds));
    }
    let mut table = Node::el("mtable", trs);
    if spec.display {
        table = table.with("displaystyle", "true");
    }
    if spec.small {
        table = Node::el("mstyle", vec![table]).with("scriptlevel", "1");
    }
    if spec.open.is_none() && spec.close.is_none() {
        return table;
    }
    let mut kids = Vec::with_capacity(3);
    kids.extend(spec.open.map(|c| fence(c, "prefix")));
    kids.push(table);
    kids.extend(spec.close.map(|c| fence(c, "postfix")));
    Node::el("mrow", kids)
}

fn function_atom(name: &str) -> Atom {
    Atom {
        node: Node::mi_upright(name),
        limits: Limits::Side,
        op: true,
        after: Some(Node::mo('\u{2061}')),
    }
}

enum Aligns {
    Center,
    Left,
    /// right, left, right, left, … (`aligned`).
    Alternating,
    Columns(Vec<&'static str>),
}

/// `lim`, `max`, `\operatorname*{…}`: upright operator whose limits go
/// underneath in display style.
fn limit_op(text: &str) -> Node {
    Node::mo(text)
        .with("movablelimits", "true")
        .with("form", "prefix")
        .with("lspace", "0")
        .with("rspace", "0.1667em")
}

/// A plain `|` or `‖` (absolute value, norm): no stretching and no
/// operator spacing, as LaTeX sets it (`|x|`, not `| x |`).
fn bar(c: char) -> Node {
    Node::mo(c).with("stretchy", "false").with("lspace", "0").with("rspace", "0")
}

/// `\text{ times}`: browsers collapse spaces at the edges of `mtext`,
/// so leading/trailing ones become no-break spaces.
fn keep_edge_spaces(text: &str) -> String {
    let body = text.trim_matches(' ');
    let lead = text.len() - text.trim_start_matches(' ').len();
    let trail = text.len() - text.trim_end_matches(' ').len();
    if body.is_empty() {
        return "\u{a0}".repeat(lead.min(1));
    }
    format!("{}{body}{}", "\u{a0}".repeat(lead), "\u{a0}".repeat(trail))
}

fn with_display(node: Node, display: Option<bool>) -> Node {
    match display {
        None => node,
        Some(d) => Node::el("mstyle", vec![node])
            .with("displaystyle", if d { "true" } else { "false" })
            .with("scriptlevel", "0"),
    }
}

fn mark_leading_operator_infix(cell: &mut Node) {
    match cell {
        Node::Leaf { tag: "mo", .. } => cell.set_attr("form", "infix"),
        Node::El { tag: "mrow", children, .. } => {
            if let Some(first @ Node::Leaf { tag: "mo", .. }) = children.first_mut() {
                first.set_attr("form", "infix");
            }
        }
        _ => {}
    }
}
