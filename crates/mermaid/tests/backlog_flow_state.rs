//! Rendered-output regressions for issues #32, #276, #278 and #289.
use ogrenotes_mermaid::render;

fn svg(source: &str) -> String {
    let result = render(source);
    result
        .svg
        .unwrap_or_else(|| panic!("{source}: {:?}", result.error))
}

#[test]
fn flowchart_quoted_labels_comments_and_bidirectional_text_preserve_meaning() {
    for source in [
        "flowchart TD\nA -->|\"a|b\"| B %% comment",
        "flowchart TD\nA -- \"a|b\" --> B",
        "flowchart TD\nA <-- \"a|b\" --> B",
    ] {
        let output = svg(source);
        assert!(output.contains(">a|b</text>"), "{output}");
        assert!(!output.contains("&quot;a|b&quot;"), "{output}");
        if source.contains("<--") {
            assert!(output.contains("marker-start=\"url(#mmd-arrow)\""));
        }
    }
    let output = svg("flowchart TD\nsubgraph \"My Title\"\nA[\"50 %% off\"]\nend");
    assert!(output.contains(">My Title</text>"));
    assert!(output.contains("50 %% off"));
}

#[test]
fn literal_percent_pairs_in_labels_do_not_hide_edges_or_trailing_comments() {
    for edge in ["-->", "---", "-.->", "==>"] {
        let source = format!("flowchart TD\nA[50 %% off] {edge}|75 %% off| B[\"90 %% off\"] --> C %% comment\nC --> D");
        let output = svg(&source);
        for text in ["50 %% off", "75 %% off", "90 %% off"] {
            assert!(output.contains(text), "{source}: {output}");
        }
        assert!(!output.contains("comment"));
        assert_eq!(
            output
                .split("</defs>")
                .nth(1)
                .unwrap()
                .matches("<path ")
                .count(),
            3,
            "edges after literal percent labels must survive: {output}"
        );
    }
    let output = svg("flowchart TD\nA -- 50 %% off --> B %% comment\nB --> C");
    assert!(output.contains(">50 %% off</text>"));
    assert!(!output.contains("comment"));
}

#[test]
fn generic_type_ends_are_distinct_from_adjacent_parameter_names() {
    let output = svg("classDiagram\nclass A {\n+find(List~int~ids) bool\n+findBoth(List~int~ids, Map~String,int~counts) bool\n+List~List~int~~nested\n+findNested(List~List~int~~ids) bool\n}");
    for text in [
        "+find(List&lt;int&gt;ids) : bool",
        "+findBoth(List&lt;int&gt;ids, Map&lt;String,int&gt;counts) : bool",
        "+List&lt;List&lt;int&gt;&gt;nested",
        "+findNested(List&lt;List&lt;int&gt;&gt;ids) : bool",
    ] {
        assert!(output.contains(text), "missing {text}: {output}");
    }
}

#[test]
fn flowchart_class_lists_and_default_properties_apply_to_explicit_classes() {
    let output =
        svg("flowchart TD\nA:::a --> B:::b\nclassDef default stroke:blue\nclassDef a,b fill:red");
    assert_eq!(
        output.matches("<g style=\"stroke:blue;fill:red\">").count(),
        2,
        "{output}"
    );
    for source in ["flowchart TD\nend-->B", "flowchart TD\nA-->end"] {
        let error = render(source).error.expect("reserved id must fail");
        assert_eq!(error.line, Some(2));
        assert!(error.message.contains("reserved"));
    }
}

#[test]
fn flowchart_class_definitions_keep_legacy_hyphenated_and_unicode_lookup_keys() {
    let output = svg(
        "flowchart TD\nA --> B\nclassDef my-class,样式 fill:red\nclass A my-class\nclass B 样式",
    );
    assert_eq!(output.matches("<g style=\"fill:red\">").count(), 2);
    for definition in ["classDef , fill:red", "classDef first,,second fill:red"] {
        let error = render(&format!("flowchart TD\nA\n{definition}"))
            .error
            .expect("empty class names rejected");
        assert_eq!(error.line, Some(3));
    }
    for name in ["my-class", "样式"] {
        let output = svg(&format!(
            "stateDiagram-v2\nclassDef {name} fill:red\nclass A {name}\nA --> B"
        ));
        assert!(
            output.contains("<g style=\"fill:red\">"),
            "legacy state class {name} was lost: {output}"
        );
    }
}

#[test]
fn state_pseudostates_are_reused_within_but_not_across_scopes() {
    let output = svg("stateDiagram-v2\n[*] --> A\n[*] --> B\nA --> [*]\nB --> [*]");
    assert_eq!(
        output.matches("<circle ").count(),
        3,
        "one start, two end rings: {output}"
    );
    let output = svg("stateDiagram-v2\n[*] --> Outside\nstate Nested {\n[*] --> A\n[*] --> B\n}");
    assert_eq!(
        output.matches("<circle ").count(),
        2,
        "one start per scope: {output}"
    );
}

#[test]
fn state_composites_accept_forward_edges_and_quoted_titles_without_phantom_nodes() {
    let output = svg("stateDiagram-v2\n[*] --> First\nstate \"Long Name\" as First {\n[*] --> A\nA --> B\n}\nFirst --> Last");
    assert!(output.contains(">Long Name</text>"));
    assert!(!output.contains(">First</tspan>"));
    assert!(!output.contains("__cluster_"));
    let body = output.split("</defs>").nth(1).unwrap();
    assert_eq!(
        body.matches("<path ").count(),
        4,
        "hidden ordering edges must not render: {body}"
    );
    assert!(body.contains("marker-end=\"url(#mmd-arrow)\""));
}

#[test]
fn malformed_state_classes_and_composite_collisions_keep_source_lines() {
    for (source, line) in [
        ("stateDiagram-v2\nA --> B\nclass A B hot", 3),
        ("stateDiagram-v2\nclass , hot", 2),
        ("stateDiagram-v2\nstate X\nstate X {\nA\n}", 3),
        ("stateDiagram-v2\nstate X {\nA\n}\nstate X {\nB\n}", 5),
        ("stateDiagram-v2\n[*] --> A\nnote right of __start_0: no", 3),
    ] {
        let error = render(source)
            .error
            .unwrap_or_else(|| panic!("must reject {source}"));
        assert_eq!(error.line, Some(line), "{source}: {error}");
    }
    assert!(svg("stateDiagram-v2; A --> B: first; second").contains("first; second"));
}

#[test]
fn composite_style_targets_are_rejected_at_original_lines_in_either_order() {
    for assignment in ["class X hot", "X:::hot", "style X fill:red"] {
        let forward = format!("stateDiagram-v2\n{assignment}\nstate X {{\nA\n}}\nB --> X");
        let after = format!("stateDiagram-v2\nstate X {{\nA\n}}\n{assignment}\nB --> X");
        for (source, line) in [(forward, 2), (after, 5)] {
            let error = render(&source)
                .error
                .unwrap_or_else(|| panic!("must reject {source}"));
            assert_eq!(error.line, Some(line), "{source}: {error}");
            assert!(error.message.contains("composite"), "{error}");
        }
    }
    assert!(
        svg("stateDiagram-v2\nclassDef hot fill:red\nclass A hot\nstate A\nA --> B")
            .contains("fill:red")
    );
}

#[test]
fn class_nested_generics_and_member_classifiers_render_semantically() {
    let output = svg("classDiagram\nclass A {\n+List~List~int~~ nested\n+count$\n+run()* Result\n+factory()$ A\n}");
    assert!(
        output.contains("+List&lt;List&lt;int&gt;&gt; nested"),
        "{output}"
    );
    assert!(output.contains("text-decoration=\"underline\">+count</text>"));
    assert!(output.contains("font-style=\"italic\">+run() : Result</text>"));
    assert!(output.contains("text-decoration=\"underline\">+factory() : A</text>"));
    assert!(!output.contains("+count$"));
    for relation in ["*..", "o..", "*--o", "()--"] {
        let error = render(&format!("classDiagram\nA {relation} B"))
            .error
            .unwrap();
        assert_eq!(error.line, Some(2));
        assert!(error.message.contains(relation), "{error}");
    }
    let output = svg("erDiagram\nPERSON }|..|{ CAR : \"is driver of\"");
    assert!(output.contains(">is driver of</text>"));
    assert!(!output.contains("&quot;is driver of&quot;"));
}

#[test]
fn class_method_return_types_keep_namespace_separators() {
    let output = svg("classDiagram\nclass A {\n+lookup() ::ns::Thing\n+explicit() : ::ns::Thing\n+compact(): ::ns::Thing\n+factory()$ ::ns::Thing\n+simple() Thing\n}");
    for member in [
        "+lookup() : ::ns::Thing",
        "+explicit() : ::ns::Thing",
        "+compact() : ::ns::Thing",
        "+factory() : ::ns::Thing",
        "+simple() : Thing",
    ] {
        assert!(output.contains(member), "missing {member}: {output}");
    }
}

#[test]
fn class_unsupported_operator_checks_respect_ids_and_quoted_multiplicities() {
    for relation in [
        "Foo..>Bar",
        "o..>Bar",
        "Foo\"o..: literal\"-->\"*..\"Bar",
        "A *--order",
        "A*--oBar",
        "A *--o_target",
        "A*--o",
    ] {
        let output = svg(&format!("classDiagram\n{relation}"));
        assert!(
            output.contains("Bar")
                || output.contains("order")
                || output.contains("o_target")
                || output.contains(">o</text>")
        );
        if relation.contains('"') {
            assert!(output.contains("o..: literal"));
            assert!(output.contains("*.."));
        }
    }
    for operator in ["*..", "o..", "*--o", "()--"] {
        let error = render(&format!("classDiagram\nA {operator} B"))
            .error
            .unwrap();
        assert_eq!(error.line, Some(2));
        assert!(error.message.contains(operator));
    }
}

#[test]
fn subgraph_titles_preserve_whitespace_brackets_and_literal_percent_pairs() {
    for declaration in [
        "subgraph S [50 %% off]",
        "subgraph S\u{2003}[50 %% off]",
        "subgraph \"Group [A]\"",
        "subgraph \"Group [A] %% off\"",
    ] {
        let output = svg(&format!(
            "flowchart TD\n{declaration} %% trailer\nA --> B\nend\nB --> C"
        ));
        let title = if declaration.contains("Group") {
            "Group [A]"
        } else {
            "50 %% off"
        };
        assert!(output.contains(title), "{output}");
        assert!(!output.contains("trailer"));
        assert_eq!(
            output
                .split("</defs>")
                .nth(1)
                .unwrap()
                .matches("<path ")
                .count(),
            2
        );
    }
}

#[test]
fn sequence_titles_rectangles_and_keyword_case_render_without_raw_markup() {
    let output = svg("sequenceDiagram\nTitle: A <&> title\nRect rgba(10,20,30,0.4)\nLoop once\nactor User as Reader\nUser--xB: cross\nB--)User: async\nEnd\nEnd");
    assert!(output.contains("A &lt;&amp;&gt; title"));
    assert!(output.contains("fill=\"rgba(10,20,30,0.4)\""));
    assert!(!output.contains("[rgba("));
    assert!(output.contains("url(#mmd-cross)"));
    assert!(output.contains("url(#mmd-async)"));
    for color in ["rgb(0,0,999)", "rgba(0,0,0,NaN)", "rgb(0,0,0);evil"] {
        let error = render(&format!("sequenceDiagram\nrect {color}\nA->>B: x\nend"))
            .error
            .unwrap();
        assert_eq!(error.line, Some(2), "{error}");
    }
}

#[test]
fn sequence_rect_color_numbers_are_serialized_as_valid_css() {
    for (source, color) in [
        ("rgba(10,20,30,1.)", "rgba(10,20,30,1)"),
        ("rgba(+1e1,2e1,3e1,+.5)", "rgba(10,20,30,0.5)"),
        ("rgb(1.,+2,3e1)", "rgb(1,2,30)"),
        ("rgba(.1,20.5,30,1e-1)", "rgba(0.1,20.5,30,0.1)"),
    ] {
        let output = svg(&format!(
            "sequenceDiagram\nrect {source}\nA->>B: hello\nend"
        ));
        assert!(
            output.contains(&format!("fill=\"{color}\"")),
            "{source} did not retain its numeric meaning: {output}"
        );
    }
}

#[test]
fn sequence_alias_keyword_is_case_insensitive_without_changing_names() {
    let output = svg("sequenceDiagram\nPARTICIPANT MixedCase AS Alice as herself\nACTOR Other As Bob\nMixedCase->>Other: hello");
    assert!(output.contains("Alice as herself"));
    assert!(output.contains("Bob"));
    assert!(!output.contains("MixedCase</tspan>"));
    let output = svg("sequenceDiagram\nparticipant aS\nPARTICIPANT A\u{2003}aS\u{2003}Name Kept\nA->>aS: names stay distinct");
    assert!(output.contains("Name Kept"));
    assert!(output.contains(">aS</tspan>"));
}

#[test]
fn sequence_messages_from_keyword_named_ids_preserve_spelling() {
    for id in [
        "PARTICIPANT",
        "ACTOR",
        "AUTONUMBER",
        "ACTIVATE",
        "DEACTIVATE",
        "LOOP",
        "ALT",
        "OPT",
        "PAR",
        "CRITICAL",
        "BREAK",
        "RECT",
        "ELSE",
        "AND",
        "OPTION",
        "END",
        "TITLE",
        "NOTE",
    ] {
        let output = svg(&format!(
            "sequenceDiagram\nparticipant {id}\nparticipant B\n{id} ->> B: hello"
        ));
        assert_eq!(
            output.matches("marker-end=\"url(").count(),
            1,
            "message disappeared for {id}: {output}"
        );
        assert_eq!(output.matches(&format!(">{id}</tspan>")).count(), 2);
        assert!(
            !output.contains(&format!(">{}</tspan>", id.to_ascii_lowercase())),
            "id case changed: {output}"
        );
    }
}

#[test]
fn sequence_structural_arrow_labels_keep_precedence_without_a_declared_source() {
    for source in [
        "sequenceDiagram\nloop ->> retry\nA->>B: work\nend",
        "sequenceDiagram\nalt ->> retry\nA->>B: work\nelse ->> alternative\nB->>A: fallback\nend",
        "sequenceDiagram\ntitle ->> retry\nA->>B: work",
        "sequenceDiagram\nparticipant loop\nLoop ->> retry\nA->>B: work\nEND",
    ] {
        let output = svg(source);
        let expected = if source.contains("fallback") { 2 } else { 1 };
        assert_eq!(
            output.matches("marker-end=\"url(").count(),
            expected,
            "structural text became a message: {output}"
        );
        assert!(output.contains("-&gt;&gt; retry"));
    }
    let output =
        svg("sequenceDiagram\nparticipant loop\nparticipant B\nloop ->> B: declared source");
    assert_eq!(output.matches("marker-end=\"url(").count(), 1);
    assert!(!output.contains(">["));
}

#[test]
fn declaration_comments_do_not_open_inline_edges_inside_class_names() {
    for name in ["cool--warm", "cool[--]", "cool--[%%]", "a>b"] {
        let output = svg(&format!(
            "flowchart TD\nA\nclassDef {name} fill:red %% ignored\nclass A {name} %% ignored"
        ));
        assert!(
            output.contains("<g style=\"fill:red\">"),
            "declaration comment corrupted class {name}: {output}"
        );
        assert!(!output.contains("ignored"));
    }
    let output = svg("flowchart TD\nclassDef cool--warm fill:red; A -- 50 %% off --> B %% ignored\nclass A cool--warm %% ignored\nstyle B fill:red %% ignored -- []\nlinkStyle default stroke:blue %% ignored -- []");
    assert_eq!(output.matches("<g style=\"fill:red\">").count(), 2);
    assert!(output.contains(">50 %% off</text>"));
    assert!(output.contains("style=\"stroke:blue\""));
    assert!(!output.contains("ignored"));
    let output = svg("flowchart TD\nsubgraph Group--A %% ignored\nA --> B\nend %% ignored");
    assert!(output.contains(">Group--A</text>"));
    assert!(!output.contains("ignored"));
}

#[test]
fn declaration_lookup_keys_are_opaque_to_all_edge_and_quote_lexers() {
    for name in [
        "a|b",
        "unbalanced|",
        "quoted\"key",
        "open[",
        "open(",
        "open{",
        "a>b",
        "cool--[%%]",
        "ns::key",
        "[a|%%]",
        "\"semi;key\"",
    ] {
        let output = svg(&format!(
            "flowchart TD\nA\nclassDef {name} fill:red %% ignored\nclass A {name} %% ignored"
        ));
        assert!(
            output.contains("<g style=\"fill:red\">"),
            "lookup-key punctuation changed its style: {name}: {output}"
        );
        assert!(!output.contains("ignored"));
    }
    for edge in ["A:::hot-->|50 %% off|B", "A:::hot -- 50 %% off --> B"] {
        let output = svg(&format!(
            "flowchart TD\nclassDef hot fill:red\n{edge} %% ignored"
        ));
        assert!(output.contains(">50 %% off</text>"));
        assert!(output.contains("<g style=\"fill:red\">"));
        assert!(!output.contains("ignored"));
    }
}

#[test]
fn class_method_classifier_uses_balanced_parameters_not_return_parentheses() {
    let source = r#"classDiagram
class A {
+pair()$ (int,int)
+abstractPair()* (int,int)
+callback(cb: fn(int)->(int,int))$ fn(int)->(int,int)
+quoted(value=")")$ (int,int)
+single(value=')')* (int,int)
}"#;
    let output = svg(source);
    for (member, style) in [
        ("+pair() : (int,int)", "text-decoration=\"underline\""),
        ("+abstractPair() : (int,int)", "font-style=\"italic\""),
        (
            "+callback(cb: fn(int)-&gt;(int,int)) : fn(int)-&gt;(int,int)",
            "text-decoration=\"underline\"",
        ),
        (
            "+quoted(value=&quot;)&quot;) : (int,int)",
            "text-decoration=\"underline\"",
        ),
        ("+single(value=')') : (int,int)", "font-style=\"italic\""),
    ] {
        assert!(
            output.contains(&format!("{style}>{member}</text>")),
            "method text/classifier changed: {member}: {output}"
        );
    }
}

#[test]
fn sequence_title_prefixes_preserve_literal_colons() {
    for (declaration, title) in [
        ("title: ::ns::Thing", "::ns::Thing"),
        ("TITLE: :", ":"),
        ("title ::ns::Thing", "::ns::Thing"),
        ("title : ::ns::Thing", "::ns::Thing"),
        ("title: :::", ":::"),
        ("title : :hello", ":hello"),
    ] {
        let output = svg(&format!("sequenceDiagram\n{declaration}\nA->>B: hi"));
        assert!(
            output.contains(&format!(">{title}</text>")),
            "literal title colon lost: {declaration}: {output}"
        );
        assert_eq!(output.matches("marker-end=\"url(").count(), 1);
    }
}

#[test]
fn language_neutral_generic_arguments_keep_complete_rendered_text() {
    for (member, expected) in [
        ("+List~unsigned int~ values", "+List<unsigned int> values"),
        (
            "+List~List~unsigned int~~ values",
            "+List<List<unsigned int>> values",
        ),
        (
            "+Map~unsigned int, const char *~ values",
            "+Map<unsigned int, const char *> values",
        ),
        ("+List~? extends Foo~ values", "+List<? extends Foo> values"),
        (
            "+List~const unsigned long *&~ values",
            "+List<const unsigned long *&> values",
        ),
        (
            "+List~std::array<unsigned int, 4>~ values",
            "+List<std::array<unsigned int, 4>> values",
        ),
        (
            "+List~fn(int)->int~ callbacks",
            "+List<fn(int)->int> callbacks",
        ),
        (
            "+List~(int)=>(bool)=>int~ callbacks",
            "+List<(int)=>(bool)=>int> callbacks",
        ),
        (
            "+method(List~unsigned int~ids) bool",
            "+method(List<unsigned int>ids) : bool",
        ),
    ] {
        let output = svg(&format!("classDiagram\nclass A {{\n{member}\n}}"));
        let escaped = expected
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        assert!(
            output.contains(&format!(">{escaped}</text>")),
            "missing complete member {expected}: {output}"
        );
    }
}

#[test]
fn trailing_comments_can_immediately_follow_class_suffixes() {
    for body in ["A:::hot%% comment", "A:::hot;B:::hot%% comment"] {
        let output = svg(&format!("flowchart TD\nclassDef hot fill:red\n{body}"));
        assert!(!output.contains("comment"));
        assert_eq!(
            output.matches("<g style=\"fill:red\">").count(),
            if body.contains(';') { 2 } else { 1 }
        );
    }
}

#[test]
fn operator_and_computed_method_names_keep_parameter_boundaries() {
    for (member, expected) in [
        ("+operator()(int) Foo", "+operator()(int) : Foo"),
        ("+[Symbol.iterator()]() Foo", "+[Symbol.iterator()]() : Foo"),
        ("+[makeName(fn())]() Foo", "+[makeName(fn())]() : Foo"),
    ] {
        let output = svg(&format!("classDiagram\nclass A {{\n{member}\n}}"));
        assert!(output.contains(&format!(">{expected}</text>")), "{output}");
    }
    let output=svg("classDiagram\nclass A {\n+operator()(int)$ (Foo, Bar)\n+[Symbol.iterator()]()* Iterator\n}");
    assert!(output.contains("text-decoration=\"underline\""));
    assert!(output.contains(">+operator()(int) : (Foo, Bar)</text>"));
    assert!(output.contains("font-style=\"italic\""));
    assert!(output.contains(">+[Symbol.iterator()]() : Iterator</text>"));
}

#[test]
fn declaration_comments_keep_opaque_percent_keys_and_require_a_boundary() {
    let output =
        svg("flowchart TD\nA\nclassDef hot%% fill:red %% ignored\nclass A hot%% %% ignored");
    assert!(output.contains("<g style=\"fill:red\">"));
    assert!(!output.contains("ignored"));
    let result = render("flowchart TD\nA\nclassDef hot fill:red\nclass A hot%% comment");
    assert_eq!(result.error.unwrap().line, Some(4));
}

#[test]
fn repeated_state_transitions_attach_to_shapes_and_stay_outside_nodes() {
    #[derive(Clone, Copy, Debug)]
    enum Node {
        Circle(f64, f64, f64),
        Rounded(f64, f64, f64, f64, f64),
    }
    impl Node {
        fn distance(self, p: [f64; 2]) -> f64 {
            match self {
                Self::Circle(x, y, r) => (p[0] - x).hypot(p[1] - y) - r,
                Self::Rounded(x, y, w, h, r) => {
                    let qx = (p[0] - x - w / 2.0).abs() - (w / 2.0 - r);
                    let qy = (p[1] - y - h / 2.0).abs() - (h / 2.0 - r);
                    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
                }
            }
        }
    }
    fn attribute(tag: &str, name: &str) -> f64 {
        tag.split(&format!(" {name}=\""))
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }
    for statements in [
        vec!["[*] --> A"; 10],
        vec!["A --> [*]"; 10],
        vec!["A --> B"; 10],
        [vec!["A --> B"; 5], vec!["B --> A"; 5]].concat(),
        [vec!["A --> B"; 10], vec!["A --> C", "C --> B"]].concat(),
    ] {
        let source = format!("stateDiagram-v2\n{}", statements.join("\n"));
        let output = svg(&source);
        let opening = output.split('>').next().unwrap();
        let width = attribute(opening, "width");
        let height = attribute(opening, "height");
        let body = output.split("</defs>").nth(1).unwrap();
        let mut nodes = Vec::new();
        for tag in body.split('<').filter_map(|part| part.split('>').next()) {
            if tag.starts_with("circle ") && attribute(tag, "r") == 7.0 {
                nodes.push(Node::Circle(
                    attribute(tag, "cx"),
                    attribute(tag, "cy"),
                    7.0,
                ));
            } else if tag.starts_with("rect ") && tag.contains(" rx=\"8\"") {
                nodes.push(Node::Rounded(
                    attribute(tag, "x"),
                    attribute(tag, "y"),
                    attribute(tag, "width"),
                    attribute(tag, "height"),
                    8.0,
                ));
            }
        }
        assert!(nodes.len() >= 2, "all endpoints must be checked: {source}");
        let paths: Vec<_> = body
            .split("<path d=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect();
        assert_eq!(paths.len(), statements.len(), "{source}");
        let distinct: std::collections::HashSet<_> = paths.iter().collect();
        assert_eq!(
            distinct.len(),
            paths.len(),
            "parallel paths collapse: {source}"
        );
        for path in paths {
            let mut tokens = path.split_whitespace();
            let point = |tokens: &mut std::str::SplitWhitespace<'_>| -> [f64; 2] {
                [
                    tokens.next().unwrap().parse().unwrap(),
                    tokens.next().unwrap().parse().unwrap(),
                ]
            };
            assert_eq!(tokens.next(), Some("M"));
            let mut from = point(&mut tokens);
            assert!(
                nodes.iter().any(|n| n.distance(from).abs() < 0.15),
                "detached source {from:?}: {path}; nodes: {nodes:?}"
            );
            while let Some(command) = tokens.next() {
                assert_eq!(command, "C");
                let a = point(&mut tokens);
                let b = point(&mut tokens);
                let to = point(&mut tokens);
                for step in 0..=128 {
                    let t = step as f64 / 128.0;
                    let u = 1.0 - t;
                    let p = std::array::from_fn(|axis| {
                        u * u * u * from[axis]
                            + 3.0 * u * u * t * a[axis]
                            + 3.0 * u * t * t * b[axis]
                            + t * t * t * to[axis]
                    });
                    assert!(
                        p[0] >= -0.15
                            && p[0] <= width + 0.15
                            && p[1] >= -0.15
                            && p[1] <= height + 0.15,
                        "curve outside canvas at {p:?}: {source}: {path}"
                    );
                    for node in &nodes {
                        assert!(
                            node.distance(p) > -0.15,
                            "curve enters node {node:?} at {p:?}: {source}: {path}"
                        );
                    }
                }
                from = to;
            }
            assert!(
                nodes.iter().any(|n| n.distance(from).abs() < 0.15),
                "detached target {from:?}: {path}; nodes: {nodes:?}"
            );
        }
    }
}

#[test]
fn unambiguous_keyword_named_sequence_sources_remain_implicit() {
    for id in [
        "end",
        "End",
        "Actor",
        "Box",
        "participant",
        "Note",
        "Autonumber",
        "activate",
    ] {
        let output = svg(&format!("sequenceDiagram\n{id} ->> B: compatibility"));
        assert!(output.contains(&format!(">{id}</tspan>")), "{id}: {output}");
        assert!(output.contains("compatibility"), "{id}: {output}");
        assert_eq!(output.matches("marker-end=\"url(").count(), 1);
    }
    // Free-text labels still claim arrow-prefixed structural arguments;
    // explicit declarations retain their existing message precedence.
    for keyword in ["loop", "alt", "opt", "par", "critical", "break"] {
        let output = svg(&format!(
            "sequenceDiagram\n{keyword} ->> retry\nA ->> B: body\nend"
        ));
        assert!(output.contains("-&gt;&gt; retry"), "{output}");
        assert!(!output.contains(&format!(">{keyword}</tspan>")), "{output}");
        let output = svg(&format!(
            "sequenceDiagram\nparticipant {keyword}\n{keyword} ->> B: message"
        ));
        assert!(output.contains(&format!(">{keyword}</tspan>")), "{output}");
        assert!(output.contains("message"));
    }
}

#[test]
fn declaration_style_brackets_are_not_reparsed_as_node_labels() {
    let brackets = "[".repeat(18_000);
    let output = svg(&format!(
        "flowchart TD\nA\nclassDef x fill:{brackets},stroke:red %% ignored\nclass A x"
    ));
    assert!(output.contains("<g style=\"stroke:red\">"));
    assert!(
        !output.contains(&brackets),
        "unsafe style value must still be dropped"
    );
    assert!(!output.contains("ignored"));
    for source in [
        "flowchart TD\nsubgraph S [unclosed[[[ %% ignored",
        "flowchart TD\nA[unclosed[[[ %% ignored",
    ] {
        let error = render(source)
            .error
            .expect("unclosed shape must still fail");
        assert_eq!(error.line, Some(2));
        assert!(error.message.contains("unclosed"), "{error:?}");
    }
}

#[test]
fn repeated_cluster_transitions_keep_clipped_borders_and_clear_interiors() {
    fn number(tag: &str, name: &str) -> f64 {
        tag.split(&format!(" {name}=\""))
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .parse()
            .unwrap()
    }
    for state in [false, true] {
        for outgoing in [false, true] {
            for nested in [false, true] {
                for count in [2, 10] {
                    let (header, cluster, outside, member) = if state {
                        ("stateDiagram-v2", "state X {\nA\n}", "B", "X")
                    } else {
                        ("flowchart TD", "subgraph S\nC\nend", "A", "S")
                    };
                    let declarations = if nested {
                        if state {
                            format!("state Outer {{\n{cluster}\n{outside}\n}}")
                        } else {
                            format!("subgraph Outer\n{cluster}\n{outside}\nend")
                        }
                    } else {
                        cluster.to_string()
                    };
                    let edge = if outgoing {
                        format!("{member} --> {outside}")
                    } else {
                        format!("{outside} --> {member}")
                    };
                    let source =
                        format!("{header}\n{declarations}\n{}", vec![edge; count].join("\n"));
                    let output = svg(&source);
                    let opening = output.split('>').next().unwrap();
                    let (width, height) = (number(opening, "width"), number(opening, "height"));
                    let body = output.split("</defs>").nth(1).unwrap();
                    let mut obstacles = Vec::new();
                    let mut clusters = Vec::new();
                    for tag in body.split('<').filter(|part| part.starts_with("rect ")) {
                        let rect = [
                            number(tag, "x"),
                            number(tag, "y"),
                            number(tag, "width"),
                            number(tag, "height"),
                        ];
                        if tag.contains("stroke-dasharray=") {
                            clusters.push(rect);
                        } else {
                            obstacles.push(rect);
                        }
                    }
                    // The explicit endpoint cluster is the inner one. The
                    // parent container legitimately contains the whole route.
                    let cluster = *clusters
                        .iter()
                        .min_by(|a, b| (a[2] * a[3]).total_cmp(&(b[2] * b[3])))
                        .unwrap();
                    obstacles.push(cluster);
                    let paths: Vec<_> = body
                        .split("<path d=\"")
                        .skip(1)
                        .map(|rest| rest.split('"').next().unwrap())
                        .collect();
                    assert_eq!(paths.len(), count, "{source}");
                    let distinct: std::collections::HashSet<_> = paths.iter().collect();
                    assert_eq!(
                        distinct.len(),
                        count,
                        "cluster transitions overlap: {source}"
                    );
                    let boundary = |p: [f64; 2], r: [f64; 4]| {
                        p[0] >= r[0] - 0.15
                            && p[0] <= r[0] + r[2] + 0.15
                            && p[1] >= r[1] - 0.15
                            && p[1] <= r[1] + r[3] + 0.15
                            && (p[0] - r[0])
                                .abs()
                                .min((p[0] - r[0] - r[2]).abs())
                                .min((p[1] - r[1]).abs())
                                .min((p[1] - r[1] - r[3]).abs())
                                < 0.15
                    };
                    let mut anchors = Vec::new();
                    for path in paths {
                        let mut tokens = path.split_whitespace();
                        let point = |tokens: &mut std::str::SplitWhitespace<'_>| -> [f64; 2] {
                            [
                                tokens.next().unwrap().parse().unwrap(),
                                tokens.next().unwrap().parse().unwrap(),
                            ]
                        };
                        assert_eq!(tokens.next(), Some("M"));
                        let mut from = point(&mut tokens);
                        let start = from;
                        assert!(
                            obstacles.iter().any(|r| boundary(start, *r)),
                            "detached source: {source}: {path}"
                        );
                        while let Some(command) = tokens.next() {
                            assert_eq!(command, "C");
                            let a = point(&mut tokens);
                            let b = point(&mut tokens);
                            let to = point(&mut tokens);
                            for step in 0..=128 {
                                let t = step as f64 / 128.0;
                                let u = 1.0 - t;
                                let p: [f64; 2] = std::array::from_fn(|axis| {
                                    u * u * u * from[axis]
                                        + 3.0 * u * u * t * a[axis]
                                        + 3.0 * u * t * t * b[axis]
                                        + t * t * t * to[axis]
                                });
                                assert!(
                                    p[0] >= -0.15
                                        && p[0] <= width + 0.15
                                        && p[1] >= -0.15
                                        && p[1] <= height + 0.15,
                                    "curve leaves canvas: {source}: {path}"
                                );
                                for r in &obstacles {
                                    assert!(!(p[0]>r[0]+0.15 && p[0]<r[0]+r[2]-0.15 && p[1]>r[1]+0.15 && p[1]<r[1]+r[3]-0.15),
                                        "curve enters cluster/node interior {r:?}: {source}: {path}");
                                }
                            }
                            from = to;
                        }
                        assert!(
                            obstacles.iter().any(|r| boundary(from, *r)),
                            "detached target: {source}: {path}"
                        );
                        assert!(
                            boundary(if outgoing { start } else { from }, cluster),
                            "endpoint left cluster border: {source}: {path}"
                        );
                        anchors.push((start, from));
                    }
                    assert!(
                        anchors.windows(2).all(|pair| pair[0] == pair[1]),
                        "parallel border anchors moved: {source}"
                    );
                }
            }
        }
    }
}

#[test]
fn literal_quotes_inside_plain_edge_labels_do_not_hide_their_delimiters() {
    for operator in ["-->", "---", "-.->", "==>", "<-->"] {
        let output = svg(&format!(
            "flowchart TD\nA {operator}|6\" [draft] tall| B %% trailer"
        ));
        assert!(output.contains(">6&quot; [draft] tall</text>"), "{output}");
        assert!(!output.contains("trailer"));
    }
    for (open, close) in [("--", "-->"), ("==", "==>"), ("-.", ".->"), ("<--", "-->")] {
        let output = svg(&format!(
            "flowchart TD\nA {open} 12\" ruler {close} B %% trailer"
        ));
        assert!(output.contains(">12&quot; ruler</text>"), "{output}");
        assert!(!output.contains("trailer"));
    }
    for source in [
        "flowchart TD\nA -->|\"a|b\"| B %% trailer",
        "flowchart TD\nA -- \"a--b\" --> B %% trailer",
        "flowchart TD\nA -->|\"50 %% off | [draft]\"| B %% trailer",
    ] {
        let output = svg(source);
        assert!(!output.contains("trailer"));
        assert!(
            !output.contains("&quot;"),
            "whole-label quotes must still be removed: {output}"
        );
    }
}

#[test]
fn bare_subgraph_titles_do_not_open_node_shapes_or_midtext_quote_guards() {
    for (declaration, title) in [
        ("Q1 > Q2", "Q1 &gt; Q2"),
        ("Plan (draft", "Plan (draft"),
        ("Plan {draft", "Plan {draft"),
        ("6\" tall", "6&quot; tall"),
        ("6\" [draft]", "6&quot; [draft]"),
        ("S [[draft]", "[draft"),
        ("S [Plan (draft]", "Plan (draft"),
        ("S [50 %% off]", "50 %% off"),
        ("S [\"Group [A] %% literal\"]", "Group [A] %% literal"),
        ("\"Group [A] %% literal\"", "Group [A] %% literal"),
        ("\"Q1 > Q2 %% literal\"", "Q1 &gt; Q2 %% literal"),
    ] {
        let output = svg(&format!(
            "flowchart TD\nsubgraph {declaration} %% trailer\nA\nend"
        ));
        assert!(
            output.contains(&format!(">{title}</text>")),
            "{declaration}: {output}"
        );
        assert!(!output.contains("trailer"), "{declaration}: {output}");
    }
    let output = svg("flowchart TD\nsubgraph Q1 > Q2; A; end %% trailer");
    assert!(output.contains(">Q1 &gt; Q2</text>"));
    assert!(!output.contains("trailer"));
}

#[test]
fn unquoted_subgraph_nested_brackets_and_percent_pairs_share_title_boundaries() {
    for (title, expected) in [
        ("S [Group [A] %% literal]", "Group [A] %% literal"),
        ("S [Group [A [B]] %% literal]", "Group [A [B]] %% literal"),
        (
            "S [Group [A] %% literal] [tail]",
            "Group [A] %% literal] [tail",
        ),
        ("S [[draft]", "[draft"),
        ("S [\"Group [A] %% literal\"]", "Group [A] %% literal"),
    ] {
        for trailer in ["", " %% trailer [ignored]"] {
            let output = svg(&format!("flowchart TD\nsubgraph {title}{trailer}\nB\nend"));
            assert!(
                output.contains(&format!(">{expected}</text>")),
                "{title}: {output}"
            );
            assert!(!output.contains("trailer"));
        }
    }
}

#[test]
fn subgraph_title_scanning_stops_at_its_quote_aware_statement_boundary() {
    for (declaration, title) in [
        ("S [Group]", "Group"),
        ("S [Group [A] %% literal]", "Group [A] %% literal"),
        ("S [\"Group; [A] %% literal\"]", "Group; [A] %% literal"),
        ("\"Group; [A] %% literal\"", "Group; [A] %% literal"),
    ] {
        for (node, label) in [
            ("B[\"x ] %% literal\"]", "x ] %% literal"),
            ("B[\"x ]; %% literal\"]", "x ]; %% literal"),
            ("B[6\" tall]", "6&quot; tall"),
        ] {
            let output = svg(&format!(
                "flowchart TD\nsubgraph {declaration}; {node}\nend"
            ));
            assert!(output.contains(&format!(">{title}</text>")), "{output}");
            assert!(
                output.contains(&format!(">{label}</tspan>")),
                "following node was truncated: {output}"
            );
        }
    }
}
