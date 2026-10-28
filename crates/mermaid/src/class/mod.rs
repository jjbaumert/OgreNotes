//! Mermaid class diagrams: model types shared by the parser and the SVG
//! renderer (this slice).

pub(crate) mod parse;
#[cfg(test)]
mod props;
pub(crate) mod svg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RelKind {
    Inheritance, // <|--   marker at `to` end, solid
    Realization, // <|..   marker at `to` end, dashed
    Composition, // *--    filled diamond at `to` end, solid
    Aggregation, // o--    hollow diamond at `to` end, solid
    Association, // --> or --  open arrow at `to` end (--> only), solid
    Dependency,  // ..>    open arrow at `to` end, dashed
    DashedLink,  // ..     no marker, dashed (the dashed peer of `--`)
}

impl RelKind {
    /// Whether the marker (`to`) end is the base class / interface that Mermaid
    /// ranks ABOVE its subclass — i.e. generalization: inheritance and
    /// realization. For these the layout edge is fed reversed (parent first) so
    /// it out-ranks its child, and the hollow triangle is drawn at the path
    /// start instead of the end. Composition/aggregation also put the "whole"
    /// on top in Mermaid, but reversing them perturbs the x-convergence of the
    /// shared layout engine, so that's deliberately left for a follow-up.
    pub(crate) fn ranks_marker_end_on_top(self) -> bool {
        matches!(self, RelKind::Inheritance | RelKind::Realization)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ClassBox {
    pub id: String,
    /// Display label from a `class Name["Label"]` declaration; the
    /// canonical `id` is still what members and relationships reference.
    pub display: Option<String>,
    pub annotation: Option<String>, // <<interface>> etc.
    pub attributes: Vec<String>,    // raw member text, verbatim
    pub methods: Vec<String>,
    pub classes: Vec<String>, // cssClass/`:::`-assigned classDef names, in order
    pub style: Option<String>, // sanitized inline `style` override
    pub namespace: Option<usize>, // enclosing `namespace` block, if any
}

/// Distinguish lifetime identifiers from quoted defaults without treating
/// the displayed member as a language-specific type grammar.
fn lifetime_quote(member: &str, index: usize, in_generic: bool) -> bool {
    let rest = &member[index + 1..];
    let identifier_end = rest
        .char_indices()
        .take_while(|(_, ch)| ch.is_alphanumeric() || *ch == '_')
        .map(|(index, ch)| index + ch.len_utf8())
        .last()
        .unwrap_or(0);
    identifier_end > 0
        && !rest[identifier_end..].starts_with('\'')
        && (member[..index].trim_end().ends_with('&') || in_generic)
}

/// Find the close of the method's actual parameter list. Generic/computed
/// names can contain parentheses; adjacent groups such as operator()(int)
/// end at the final group, while whitespace-delimited returns stay separate.
fn method_parameter_end(member: &str) -> Option<usize> {
    // Pair generic and computed-name spans, ignoring function-type arrows.
    // Only matched spans hide parentheses, so operator< remains usable.
    let mut hidden_ends = vec![None; member.len()];
    let mut generic_starts = Vec::new();
    let mut computed_starts = Vec::new();
    let mut openers = vec![false; member.len()];
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in member.char_indices() {
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if ch == '\'' && lifetime_quote(member, index, !generic_starts.is_empty()) {
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '<' => generic_starts.push(index),
            '>' if !member[..index].ends_with(['-', '=']) => {
                if let Some(start) = generic_starts.pop() {
                    hidden_ends[start] = Some(index);
                }
            }
            '[' => computed_starts.push(index),
            ']' => {
                if let Some(start) = computed_starts.pop() {
                    hidden_ends[start] = Some(index);
                }
            }
            '(' => openers[index] = true,
            _ => {}
        }
    }
    let mut skip_until = 0;
    let mut open = None;
    for (index, _) in member.char_indices() {
        if index < skip_until {
            continue;
        }
        if let Some(end) = hidden_ends[index] {
            skip_until = end + 1;
        } else if openers[index] {
            open = Some(index);
            break;
        }
    }
    let open = open?;
    let mut depth = 1usize;
    let mut generic_depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (relative, ch) in member[open + 1..].char_indices() {
        let index = open + 1 + relative;
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if ch == '\'' && lifetime_quote(member, index, generic_depth > 0) {
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '<' => generic_depth += 1,
            '>' if !member[..index].ends_with(['-', '=']) => {
                generic_depth = generic_depth.saturating_sub(1)
            }
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && !member[index + 1..].starts_with('(') {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// Display form of a member. Keep parser storage intact while sharing the
/// exact text between measurement and SVG emission.
fn member_presentation(member: &str) -> (String, &'static str) {
    let mut value = member.trim();
    let mut static_member = false;
    let mut abstract_member = false;
    if let Some(stripped) = value.strip_suffix('$') {
        static_member = true;
        value = stripped;
    } else if let Some(stripped) = value.strip_suffix('*') {
        abstract_member = true;
        value = stripped;
    }
    let text = if let Some(close) = method_parameter_end(value) {
        let mut suffix = value[close + 1..].trim();
        if let Some(stripped) = suffix.strip_prefix('$') {
            static_member = true;
            suffix = stripped.trim();
        } else if let Some(stripped) = suffix.strip_prefix('*') {
            abstract_member = true;
            suffix = stripped.trim();
        }
        if suffix.is_empty() {
            value[..=close].to_string()
        } else {
            // A single `:` can separate the method from its return type;
            // a leading `::` belongs to a qualified type and must survive.
            let return_type = if suffix.starts_with("::") {
                suffix
            } else {
                suffix.strip_prefix(':').unwrap_or(suffix).trim()
            };
            format!("{} : {}", &value[..=close], return_type)
        }
    } else {
        value.to_string()
    };
    let style = match (static_member, abstract_member) {
        (true, true) => "text-decoration=\"underline\" font-style=\"italic\"",
        (true, false) => "text-decoration=\"underline\"",
        (false, true) => "font-style=\"italic\"",
        (false, false) => "",
    };
    (text, style)
}

#[derive(Debug, Clone)]
pub(crate) struct Relation {
    pub from: usize,
    pub to: usize, // the MARKER end (normalized during parse)
    pub kind: RelKind,
    pub arrow: bool,            // Association `--` (false) vs `-->` (true)
    pub back_arrow: bool,       // second arrowhead on the `from` end (`<-->`, `<..>`)
    pub m_from: Option<String>, // multiplicity near `from`
    pub m_to: Option<String>,
    pub label: Option<String>,
    pub style: Option<String>, // sanitized `linkStyle` override
}

#[derive(Debug, Clone)]
pub(crate) struct ClassNote {
    pub target: Option<usize>, // class index for `note for X`; None = floating
    pub text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ClassGraph {
    pub direction: crate::layout::Direction,
    pub namespaces: Vec<String>, // namespace titles, indexed by ClassBox.namespace
    pub notes: Vec<ClassNote>,
    pub classes: Vec<ClassBox>,
    pub relations: Vec<Relation>,
    pub class_defs: Vec<crate::style::ClassDef>,
}

/// Full class-diagram pipeline: parse -> size each class's compartment
/// box (name [+ annotation] / attributes / methods) -> lay out via the
/// shared `boxgraph` adapter -> SVG. Never panics; a layout failure
/// (diagram too large) surfaces as a `ParseError` with no source line,
/// same as `boxgraph::layout_boxgraph`'s other consumers.
pub(crate) fn render_class(source: &str) -> Result<String, crate::ParseError> {
    let g = parse::parse(source)?;

    let mut sizes = Vec::with_capacity(g.classes.len());
    let mut nodes = Vec::with_capacity(g.classes.len());
    for c in &g.classes {
        // Same line set the renderer draws: annotation (as `«name»`) if
        // present, then the id, then attributes, then methods verbatim.
        let mut lines: Vec<String> = Vec::new();
        if let Some(ann) = &c.annotation {
            lines.push(format!("«{ann}»"));
        }
        lines.push(c.display.clone().unwrap_or_else(|| c.id.clone()));
        lines.extend(c.attributes.iter().map(|m| member_presentation(m).0));
        lines.extend(c.methods.iter().map(|m| member_presentation(m).0));

        let max_w = lines
            .iter()
            .map(|l| crate::measure::text_size(l).0)
            .fold(0.0_f64, f64::max);
        let width = (max_w + 24.0).max(80.0);
        let height = lines.len() as f64 * (crate::measure::LINE_H + 4.0) + 16.0 + 8.0;

        sizes.push((width, height));
        nodes.push(crate::boxgraph::BoxNode {
            width,
            height,
            cluster: c.namespace,
        });
    }

    let clusters: Vec<crate::boxgraph::BoxCluster> = g
        .namespaces
        .iter()
        .map(|title| crate::boxgraph::BoxCluster {
            parent: None,
            title: crate::measure::text_size(title),
        })
        .collect();

    let edges: Vec<crate::boxgraph::BoxEdge> = g
        .relations
        .iter()
        .map(|r| {
            // Rank the parent/whole above its child for generalization-family
            // relations by feeding the layout edge reversed (marker end first);
            // svg::emit compensates by drawing the arrowhead at the path start.
            let (from, to) = if r.kind.ranks_marker_end_on_top() {
                (r.to, r.from)
            } else {
                (r.from, r.to)
            };
            crate::boxgraph::BoxEdge {
                from,
                to,
                label: r.label.as_deref().map(|l| {
                    let (w, h) = crate::measure::text_size(l);
                    (w + 8.0, h + 4.0)
                }),
            }
        })
        .collect();

    let layout = crate::boxgraph::layout_boxgraph(&nodes, &edges, &clusters, g.direction)?;
    Ok(svg::emit(&g, &layout, &sizes))
}

#[cfg(test)]
mod member_tests {
    use super::{member_presentation, method_parameter_end};

    #[test]
    fn tuple_and_function_returns_keep_classifiers_and_return_separator() {
        for (source, expected, style) in [
            (
                "+pair()$ (int, int)",
                "+pair() : (int, int)",
                "text-decoration=\"underline\"",
            ),
            (
                "+pair()* (int, int)",
                "+pair() : (int, int)",
                "font-style=\"italic\"",
            ),
            (
                "+factory()$ fn(int, (bool, int)) -> (int, int)",
                "+factory() : fn(int, (bool, int)) -> (int, int)",
                "text-decoration=\"underline\"",
            ),
            (
                "+pair() (int, int)$",
                "+pair() : (int, int)",
                "text-decoration=\"underline\"",
            ),
        ] {
            assert_eq!(member_presentation(source), (expected.to_string(), style));
        }
    }

    #[test]
    fn nested_callback_parameters_close_before_parenthesized_returns() {
        let source = "+apply(cb: Fn((int, int)) -> (bool, int))$ (bool, int)";
        assert_eq!(
            member_presentation(source),
            (
                "+apply(cb: Fn((int, int)) -> (bool, int)) : (bool, int)".to_string(),
                "text-decoration=\"underline\""
            )
        );
        let source =
            "+apply(cb: for<'a, 'b> fn(&'a str, &'b str) -> (bool, int))* fn(int) -> (bool, int)";
        assert_eq!(member_presentation(source),("+apply(cb: for<'a, 'b> fn(&'a str, &'b str) -> (bool, int)) : fn(int) -> (bool, int)".to_string(),"font-style=\"italic\""));
    }

    #[test]
    fn quoted_and_escaped_default_parentheses_do_not_close_parameters() {
        for parameters in [
            r#"value=")""#,
            r#"value="(nested)""#,
            r#"value="\")""#,
            "ch=')'",
            r"ch='\''",
            "value='multiple ) parentheses ('",
        ] {
            let source = format!("+pair({parameters})$ (int, int)");
            assert_eq!(
                member_presentation(&source),
                (
                    format!("+pair({parameters}) : (int, int)"),
                    "text-decoration=\"underline\""
                )
            );
        }
    }

    #[test]
    fn qualified_names_and_return_types_keep_namespace_separators() {
        for (source, expected) in [
            (
                "+ns::pair()$ ::std::tuple<int, int>",
                "+ns::pair() : ::std::tuple<int, int>",
            ),
            (
                "+ns::pair()$ : std::tuple<int, int>",
                "+ns::pair() : std::tuple<int, int>",
            ),
            (
                "+ns::pair()$ std::tuple<int, int>",
                "+ns::pair() : std::tuple<int, int>",
            ),
        ] {
            assert_eq!(
                member_presentation(source),
                (expected.to_string(), "text-decoration=\"underline\"")
            );
        }
    }

    #[test]
    fn generic_callback_fields_are_not_presented_as_methods() {
        for field in [
            "+List<fn(int)->int> callbacks",
            "+List<fn((int, int))->(bool, int)> callbacks",
            "+List<(int, fn(bool)=>(int, int))> callbacks",
            "+List<void (*)(unsigned int)> values",
        ] {
            assert_eq!(method_parameter_end(field), None);
            assert_eq!(member_presentation(field), (field.to_string(), ""));
        }
        assert_eq!(
            member_presentation("+operator<(Value other)$ bool"),
            (
                "+operator<(Value other) : bool".to_string(),
                "text-decoration=\"underline\""
            )
        );
        assert_eq!(
            member_presentation("+apply(cb: List<fn(int)=>(int, int)>)$ (int, int)"),
            (
                "+apply(cb: List<fn(int)=>(int, int)>) : (int, int)".to_string(),
                "text-decoration=\"underline\""
            )
        );
    }

    #[test]
    fn call_operator_and_computed_names_keep_the_actual_parameter_list() {
        for (source, expected, style) in [
            ("+operator()(int) Foo", "+operator()(int) : Foo", ""),
            (
                "+operator()(int)$ Foo",
                "+operator()(int) : Foo",
                "text-decoration=\"underline\"",
            ),
            (
                "+operator()(int)* (Foo, Foo)",
                "+operator()(int) : (Foo, Foo)",
                "font-style=\"italic\"",
            ),
            (
                "+operator()(int) fn(bool)->(Foo, Foo)$",
                "+operator()(int) : fn(bool)->(Foo, Foo)",
                "text-decoration=\"underline\"",
            ),
            (
                "+curry()(int)(bool)$ Foo",
                "+curry()(int)(bool) : Foo",
                "text-decoration=\"underline\"",
            ),
            (
                "+[Symbol.iterator()]() Foo",
                "+[Symbol.iterator()]() : Foo",
                "",
            ),
            (
                "+[Symbol.iterator()]()$ (Foo, Foo)",
                "+[Symbol.iterator()]() : (Foo, Foo)",
                "text-decoration=\"underline\"",
            ),
            (
                "+[items[indexOf()]](int)* Foo",
                "+[items[indexOf()]](int) : Foo",
                "font-style=\"italic\"",
            ),
            (
                r#"+[factory("]")]()$ Foo"#,
                r#"+[factory("]")]() : Foo"#,
                "text-decoration=\"underline\"",
            ),
        ] {
            assert_eq!(member_presentation(source), (expected.to_string(), style));
        }
        assert_eq!(
            member_presentation("+pair() (int, int)"),
            ("+pair() : (int, int)".to_string(), "")
        );
        assert_eq!(
            member_presentation("+[Symbol.iterator()]"),
            ("+[Symbol.iterator()]".to_string(), "")
        );
    }

    #[test]
    fn unmatched_parameter_lists_or_quotes_have_no_matching_close() {
        for source in [
            "+pair(",
            "+pair(cb: fn(int)",
            r#"+pair(value="unterminated))"#,
        ] {
            assert_eq!(method_parameter_end(source), None);
        }
    }
}
