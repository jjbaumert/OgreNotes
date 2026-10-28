//! Public-renderer regressions for Gantt and secondary diagram families.
use ogrenotes_mermaid::render;

fn svg(source: &str) -> String {
    let output = render(source);
    output
        .svg
        .unwrap_or_else(|| panic!("{source}: {:?}", output.error))
}

#[test]
fn unsupported_secondary_options_never_disappear_silently() {
    for (source, line) in [
        ("mindmap\nroot\n ::icon(fa fa-book)", 3),
        ("mindmap\nroot\n :::hot", 3),
        ("mindmap\nroot[\"`**bold**`\"]", 2),
        ("gitGraph TB:\ncommit", 1),
        ("gitGraph BT:\ncommit", 1),
        ("gitGraph\ncommit\nbranch b order: 2", 3),
        ("gitGraph\ncommit type: WRONG", 2),
        ("gitGraph\ncommit\nbranch b\ncheckout main\nmerge b", 5),
        ("kanban\nTodo\n t[Task]@{ priority: 'High' }", 3),
        ("kanban\nTodo\n t@{ priority: 'High' }", 3),
        ("architecture-beta\nservice a(server)[A]\na:B -- T:ghost", 3),
        ("architecture-beta\nservice a(server)[A] in ghost", 2),
        ("architecture-beta\nservice a(madeup)[A]", 2),
    ] {
        let output = render(source);
        assert!(output.svg.is_none(), "unexpected render: {source}");
        assert_eq!(output.error.unwrap().line, Some(line), "{source}");
    }
}

#[test]
fn gantt_resolves_forward_dependencies_and_all_duration_units() {
    for duration in ["1.5d", "2w", "1M", "1y", "30m", "30s", "500ms"] {
        svg(&format!(
            "gantt\nA :a, after b, {duration}\nB :b, 2024-01-01, 1d"
        ));
    }
    svg("gantt\nA :a, 2024-01-01, until b c\nB :b, 2024-01-03, 1d\nC :c, 2024-01-04, 1d");
    let critical_done = svg("gantt\nA :crit, done, 2024-01-01, 2d");
    assert!(critical_done.contains("fill=\"var(--mermaid-gantt-done, #b8b8b8)\""));
    assert!(
        critical_done.contains("stroke=\"var(--mermaid-gantt-crit, #ff6b6b)\" stroke-width=\"2\"")
    );
}

#[test]
fn gantt_dependency_cycles_and_reversed_dates_report_source_line() {
    for source in [
        "gantt\nA :a, after b, 1d\nB :b, after a, 1d",
        "gantt\nA :2024-01-03, 2024-01-01",
        "gantt\nA :a, 2024-01-01, 1d\nB :a, 2024-01-02, 1d",
    ] {
        let output = render(source);
        assert!(output.svg.is_none(), "{source}");
        assert!(output.error.unwrap().line.is_some());
    }
}

#[test]
fn gantt_milestone_is_at_the_middle_of_its_duration() {
    let output = svg("gantt\nMilestone :milestone, 2024-01-01, 2d");
    assert!(output.contains("<path d=\"M 390.0 "), "{output}");
}

#[test]
fn mindmap_quotes_and_deep_shapes_keep_their_meaning() {
    let output = svg("mindmap\nroot[\"Root\"]\n child[Child]\n  leaf{{Hex}}\n  other((Circle))");
    assert!(output.contains(">Root</text>"));
    assert!(!output.contains("&quot;Root&quot;"));
    assert!(output.contains("<polygon"), "deep hexagon: {output}");
    assert!(output.contains("<circle"), "deep circle: {output}");
    // The declared rectangular root must not add an implicit circle.
    assert_eq!(output.matches("<circle").count(), 1);
}

#[test]
fn timeline_continuations_stay_with_the_preceding_period() {
    let output = svg("timeline\n2000 : First\n : Second : Third");
    for label in ["2000", "First", "Second", "Third"] {
        assert!(output.contains(&format!(">{label}</text>")));
    }
    let output = render("timeline\nsection One\n2000 : First\nsection Two\n : Orphan");
    assert_eq!(output.error.unwrap().line, Some(5));
}

#[test]
fn architecture_forward_groups_resolve_without_extra_grid_cells() {
    let output = svg("architecture-beta\nservice a(server)[A] in g\ngroup g(cloud)[Group]");
    let forward = architecture_svg_geometry(&output);
    let declared_first = architecture_svg_geometry(&svg(
        "architecture-beta\ngroup g(cloud)[Group]\nservice a(server)[A] in g",
    ));
    assert_eq!(forward.groups, declared_first.groups);
    assert_eq!(forward.centers, declared_first.centers);
    let output = render("architecture-beta\ngroup a(cloud)[A] in b\ngroup b(cloud)[B] in a\nservice s(server)[S] in a");
    assert!(output.error.is_some());
}

#[test]
fn long_kanban_content_wraps_without_losing_words() {
    let label = "A long card with several distinct words that should wrap into multiple lines within its column";
    let output = svg(&format!("kanban\nTodo\n task[{label}]"));
    assert!(!output.contains(&format!(">{label}</text>")));
    for word in label.split_whitespace() {
        assert!(output.contains(word), "lost {word}: {output}");
    }
    assert!(output.matches("<text").count() > 3);
}

#[test]
fn crowded_mindmap_shapes_do_not_overlap() {
    use quick_xml::{events::Event, Reader};
    let mut source = String::from("mindmap\nroot((Root))\n");
    for i in 0..12 {
        source.push_str(&format!(" child{i}[Twenty character label {i}]\n"));
    }
    let output = svg(&source);
    let mut reader = Reader::from_str(&output);
    let mut boxes: Vec<[f64; 4]> = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Empty(node) if matches!(node.name().as_ref(), b"rect" | b"circle") => {
                let attrs: std::collections::HashMap<_, _> = node
                    .attributes()
                    .map(|attr| {
                        let attr = attr.unwrap();
                        (
                            String::from_utf8(attr.key.as_ref().to_vec()).unwrap(),
                            attr.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                let number = |key: &str| attrs[key].parse::<f64>().unwrap();
                boxes.push(if node.name().as_ref() == b"circle" {
                    let (x, y, r) = (number("cx"), number("cy"), number("r"));
                    [x - r, y - r, x + r, y + r]
                } else {
                    let (x, y, w, h) =
                        (number("x"), number("y"), number("width"), number("height"));
                    [x, y, x + w, y + h]
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(boxes.len(), 13);
    for (i, b) in boxes.iter().enumerate() {
        for a in &boxes[..i] {
            let width = b[2].min(a[2]) - b[0].max(a[0]);
            let height = b[3].min(a[3]) - b[1].max(a[1]);
            assert!(
                width <= 1.0 || height <= 1.0,
                "overlapping rendered nodes: {a:?}, {b:?}"
            );
        }
    }
}

#[test]
fn gantt_until_only_reads_the_referenced_start() {
    svg("gantt\nA :a, 2024-01-01, until b\nB :b, 2024-01-03, until c\nC :c, after a, 1d");
    // This cycle involves the date values themselves and must still fail.
    assert!(
        render("gantt\nA :a, after b, 2024-01-04\nB :b, 2024-01-01, until a")
            .error
            .is_some()
    );
}

#[test]
fn architecture_parent_groups_enclose_child_groups_and_reserve_titles() {
    use quick_xml::{events::Event, Reader};
    for declarations in [
        "group outer(cloud)[Outer]\ngroup inner(cloud)[Inner] in outer",
        "group inner(cloud)[Inner] in outer\ngroup outer(cloud)[Outer]",
    ] {
        let output = svg(&format!(
            "architecture-beta\n{declarations}\nservice s(server)[Service] in inner"
        ));
        let mut reader = Reader::from_str(&output);
        let mut boxes = Vec::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Empty(node) if node.name().as_ref() == b"rect" => {
                    let attrs: std::collections::HashMap<_, _> = node
                        .attributes()
                        .map(|attr| {
                            let attr = attr.unwrap();
                            (
                                String::from_utf8(attr.key.as_ref().to_vec()).unwrap(),
                                attr.unescape_value().unwrap().into_owned(),
                            )
                        })
                        .collect();
                    if attrs.get("stroke-opacity").map(String::as_str) != Some("0.4") {
                        continue;
                    }
                    boxes.push(
                        ["x", "y", "width", "height"].map(|key| attrs[key].parse::<f64>().unwrap()),
                    );
                }
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(boxes.len(), 2);
        let [outer, inner] = boxes.as_slice() else {
            unreachable!()
        };
        assert!(outer[0] < inner[0] && outer[0] + outer[2] > inner[0] + inner[2]);
        assert!(
            outer[1] + 24.0 <= inner[1],
            "parent title must be above child: {boxes:?}"
        );
        assert!(outer[1] + outer[3] > inner[1] + inner[3]);
    }
}

#[test]
fn kanban_metadata_is_distinguished_from_literal_card_text() {
    for label in ["Literal @{ value }", "\"Literal @{ value }\""] {
        let output = svg(&format!("kanban\nTodo\n task[{label}]"));
        assert!(output.contains("@{"));
        assert!(!output.contains("&quot;"));
    }
    for line in ["task[Lost] trailing", "task[Unclosed"] {
        assert_eq!(
            render(&format!("kanban\nTodo\n {line}"))
                .error
                .unwrap()
                .line,
            Some(3)
        );
    }
}

#[test]
fn git_graph_consumes_every_statement_argument() {
    for (source, line) in [
        ("gitGraph\ncommit unsupported: value", 2),
        ("gitGraph\ncommit id: known unsupported: value", 2),
        ("gitGraph\ncommit id: one id: two", 2),
        ("gitGraph\ncommit id: \"unclosed", 2),
        ("gitGraph\ncommit\ncheckout main trailing", 3),
        (
            "gitGraph\ncommit\nbranch dev\ncommit\ncheckout main\nmerge dev unsupported: value",
            6,
        ),
        (
            "gitGraph\ncommit id: first\ncherry-pick id: first parent: other",
            3,
        ),
    ] {
        let output = render(source);
        assert!(output.svg.is_none(), "must reject {source}");
        assert_eq!(output.error.unwrap().line, Some(line), "{source}");
    }
    let output = svg("gitGraph\ncommit id: \"first id\" tag: \"literal type: WRONG\" type: NORMAL\nbranch \"my branch\"\ncommit\ncheckout main\nmerge \"my branch\" id: merged tag: \"tag: value\" type: NORMAL");
    for label in [
        "first id",
        "literal type: WRONG",
        "my branch",
        "merged",
        "tag: value",
    ] {
        assert!(output.contains(label), "lost {label}");
    }
}

#[test]
fn mindmap_cloud_and_bang_keep_distinct_geometry_at_root_and_depth() {
    for source in [
        "mindmap\nroot)Cloud(\n rounded(Rounded)\n  bang))Bang((",
        "mindmap\nroot))Bang((\n rounded(Rounded)\n  cloud)Cloud(",
    ] {
        let output = svg(source);
        assert!(output.contains("<path data-mindmap-shape=\"cloud\""));
        assert!(output.contains(" Q "), "cloud outline must be scalloped");
        assert!(output.contains("<polygon data-mindmap-shape=\"bang\""));
        assert!(
            output.contains("rx=\"8\""),
            "ordinary rounded node stays rounded"
        );
        assert_eq!(output.matches("data-mindmap-shape=\"cloud\"").count(), 1);
        assert_eq!(output.matches("data-mindmap-shape=\"bang\"").count(), 1);
    }
}

#[test]
fn kanban_matches_the_outer_delimiter_and_rejects_trailing_statements() {
    for line in [
        "task[First] other[Second]",
        "task(First) other(Second)",
        "task[\"Literal ] text\"] other[Second]",
    ] {
        let output = render(&format!("kanban\nTodo\n {line}"));
        assert!(output.svg.is_none(), "must reject {line}");
        assert_eq!(output.error.unwrap().line, Some(3));
    }
    for line in [
        "task(\"Literal [text] @{ value }\")",
        "task[\"Literal (text) @{ value }\"]",
        "task[\"Literal ] [ brackets\"]",
        "task(\"Literal ) ( parentheses\")",
    ] {
        let output = svg(&format!("kanban\nTodo\n {line}"));
        assert!(output.contains("Literal"));
        assert!(!output.contains("&quot;"));
    }
    assert!(svg("kanban\nTodo\n task(\"Literal [text] @{ value }\")")
        .contains("Literal [text] @{ value }"));
}

#[test]
fn gantt_calendar_months_preserve_pre_epoch_fractional_dates() {
    svg("gantt\nStart :start, 1969-01-01, 12h\nMonth :month, after start, 1M\nZero :zero, after start, 0M");
}

#[test]
fn mindmap_inline_decorators_are_not_plain_labels() {
    for node in [
        "child[Child]:::hot",
        "child[Child]::icon(fa fa-book)",
        "child:::hot",
        "child)Cloud(:::hot",
        "child))Bang((::icon(fa fa-book)",
    ] {
        let output = render(&format!("mindmap\nroot\n {node}"));
        assert!(output.svg.is_none(), "must reject {node}");
        assert_eq!(output.error.unwrap().line, Some(3));
    }
    for node in [
        "child[\"Literal :::hot ::icon(fa fa-book)\"]",
        "child[Literal :::hot]",
    ] {
        let output = svg(&format!("mindmap\nroot\n {node}"));
        assert!(output.contains("Literal :::hot"));
    }
}

struct ArchitectureSvgGeometry {
    groups: Vec<[f64; 4]>,
    centers: std::collections::HashMap<String, (f64, f64)>,
    paths: Vec<Vec<(f64, f64)>>,
    captions: Vec<(String, f64, f64)>,
}

fn architecture_svg_geometry(output: &str) -> ArchitectureSvgGeometry {
    use quick_xml::{events::Event, Reader};
    let mut reader = Reader::from_str(output);
    let mut geometry = ArchitectureSvgGeometry {
        groups: Vec::new(),
        centers: std::collections::HashMap::new(),
        paths: Vec::new(),
        captions: Vec::new(),
    };
    loop {
        match reader.read_event().unwrap() {
            Event::Empty(node) | Event::Start(node) => {
                let attrs: std::collections::HashMap<_, _> = node
                    .attributes()
                    .map(|attr| {
                        let attr = attr.unwrap();
                        (
                            String::from_utf8(attr.key.as_ref().to_vec()).unwrap(),
                            attr.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                match node.name().as_ref() {
                    b"rect" if attrs.get("stroke-opacity").map(String::as_str) == Some("0.4") => {
                        geometry.groups.push(
                            ["x", "y", "width", "height"].map(|key| attrs[key].parse().unwrap()),
                        );
                    }
                    b"text" if attrs.get("font-weight").map(String::as_str) == Some("600") => {
                        let name = reader.read_text(node.name()).unwrap().into_owned();
                        geometry.captions.push((
                            name,
                            attrs["x"].parse().unwrap(),
                            attrs["y"].parse().unwrap(),
                        ));
                    }
                    b"text" if attrs.get("stroke-width").map(String::as_str) == Some("3") => {
                        let name = reader.read_text(node.name()).unwrap().into_owned();
                        geometry.centers.insert(
                            name,
                            (
                                attrs["x"].parse().unwrap(),
                                attrs["y"].parse::<f64>().unwrap() - 33.0,
                            ),
                        );
                    }
                    b"path" if attrs.get("stroke").map(String::as_str) == Some("currentColor") => {
                        let values: Vec<f64> = attrs["d"]
                            .split_whitespace()
                            .filter_map(|v| v.parse().ok())
                            .collect();
                        geometry
                            .paths
                            .push(values.chunks_exact(2).map(|p| (p[0], p[1])).collect());
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    geometry
}

fn architecture_assert_edges_clear_icons(geometry: &ArchitectureSvgGeometry) {
    for path in &geometry.paths {
        for segment in path.windows(2) {
            let (a, b) = (segment[0], segment[1]);
            assert!(
                (a.0 - b.0).abs() < 0.01 || (a.1 - b.1).abs() < 0.01,
                "non-orthogonal segment {segment:?}"
            );
            for (name, &(x, y)) in &geometry.centers {
                let (l, t, r, bottom) = (x - 23.0, y - 29.0, x + 23.0, y + 17.0);
                let intersects = if (a.1 - b.1).abs() < 0.01 {
                    a.1 > t && a.1 < bottom && a.0.min(b.0) < r && a.0.max(b.0) > l
                } else {
                    a.0 > l && a.0 < r && a.1.min(b.1) < bottom && a.1.max(b.1) > t
                };
                assert!(!intersects, "edge {path:?} crosses icon {name} at {x},{y}");
            }
        }
    }
}

#[test]
fn architecture_unrelated_nested_groups_do_not_overlap() {
    let variants = [
        "group ga(cloud)[GA]\ngroup ia(cloud)[IA] in ga\ngroup gb(cloud)[GB]\ngroup ib(cloud)[IB] in gb\nservice a(server)[A] in ia\nservice b(server)[B] in ib\na:R -- L:b",
        "group ga(cloud)[GA]\ngroup ia(cloud)[IA] in ga\ngroup gb(cloud)[GB]\ngroup ib(cloud)[IB] in gb\ngroup iib(cloud)[IIB] in ib\nservice a(server)[A] in ia\nservice b(server)[B] in iib\na:B -- T:b",
        "group ga(cloud)[GA]\ngroup ia(cloud)[IA] in ga\ngroup gb(cloud)[GB]\ngroup ib(cloud)[IB] in gb\nservice a(server)[A] in ia\nservice b(server)[B] in ib\nservice c(server)[C] in ia\na:R -- L:b\nb:R -- L:c",
        "group ga(cloud)[GA]\ngroup ia(cloud)[IA] in ga\ngroup gb(cloud)[GB]\ngroup ib(cloud)[IB] in gb\nservice c(server)[C] in ia\nservice b(server)[B] in ib\nservice a(server)[A] in ia\na:R -- L:b\nb:R -- L:c",
        "group ga(cloud)[GA]\ngroup gb(cloud)[GB]\nservice a(server)[A] in ga\nservice b(server)[B] in gb\nservice c(server)[C] in ga\na:R -- L:b\nb:R -- L:c",
    ];
    for (variant, source) in variants.iter().enumerate() {
        let output = svg(&format!("architecture-beta\n{source}"));
        let geometry = architecture_svg_geometry(&output);
        let (a, b) = (geometry.groups[0], geometry.groups[1]);
        assert!(
            a[0] + a[2] <= b[0]
                || b[0] + b[2] <= a[0]
                || a[1] + a[3] <= b[1]
                || b[1] + b[3] <= a[1],
            "unrelated groups overlap: {a:?}, {b:?}\n{source}"
        );
        if variant == 1 {
            assert!(geometry.centers["A"].1 < geometry.centers["B"].1);
        } else {
            assert!(geometry.centers["A"].0 < geometry.centers["B"].0);
        }
        if variant >= 2 {
            assert!(geometry.centers["B"].0 < geometry.centers["C"].0);
        }
        architecture_assert_edges_clear_icons(&geometry);
    }
}

#[test]
fn architecture_nested_layout_preserves_all_port_directions_and_routes() {
    let groups="architecture-beta\ngroup outer(cloud)[Outer]\ngroup inner(cloud)[Inner] in outer\nservice b(server)[B] in inner\nservice a(server)[A] in inner\n";
    for (edge, dx, dy) in [
        ("a:R --> L:b", 1.0, 0.0),
        ("a:L --> R:b", -1.0, 0.0),
        ("a:B --> T:b", 0.0, 1.0),
        ("a:T --> B:b", 0.0, -1.0),
        ("a:R --> T:b", 1.0, 1.0),
        ("a:T --> L:b", 1.0, -1.0),
        ("a:R --> R:b", 1.0, 0.0),
    ] {
        let output = svg(&format!("{groups}{edge}"));
        let geometry = architecture_svg_geometry(&output);
        let (a, b) = (geometry.centers["A"], geometry.centers["B"]);
        if dx != 0.0 {
            assert!((b.0 - a.0) * dx > 0.0, "{edge}");
        }
        if dy != 0.0 {
            assert!((b.1 - a.1) * dy > 0.0, "{edge}");
        }
        architecture_assert_edges_clear_icons(&geometry);
        let path = &geometry.paths[0];
        assert!(path.len() >= 2);
        let source_side = edge.split(':').nth(1).unwrap().chars().next().unwrap();
        let target_side = edge
            .split_whitespace()
            .last()
            .unwrap()
            .chars()
            .next()
            .unwrap();
        for (side, p, q) in [
            (source_side, path[0], path[1]),
            (target_side, *path.last().unwrap(), path[path.len() - 2]),
        ] {
            let delta = match side {
                'L' => (-1.0, 0.0),
                'R' => (1.0, 0.0),
                'T' => (0.0, -1.0),
                'B' => (0.0, 1.0),
                _ => unreachable!(),
            };
            assert!(
                (q.0 - p.0) * delta.0 + (q.1 - p.1) * delta.1 > 0.0,
                "wrong port direction: {edge}: {path:?}"
            );
        }
    }
    let geometry = architecture_svg_geometry(&svg(&format!("{groups}a:R --> B:a")));
    architecture_assert_edges_clear_icons(&geometry);
    let output = render(&format!("{groups}a:R -- L:b\nb:R -- L:a"));
    assert!(output.svg.is_none());
    let error = output.error.unwrap();
    assert_eq!(error.line, Some(7));
    assert!(error
        .message
        .contains("inconsistent architecture port directions"));
}

#[test]
fn architecture_ungrouped_edges_use_the_same_obstacle_aware_router() {
    for edge in [
        "a:R --> R:b",
        "a:L --> L:b",
        "a:B --> B:b",
        "a:T --> T:b",
        "a:R --> L:a",
        "a:R --> B:a",
    ] {
        for declarations in [
            "service a(server)[A]\nservice b(server)[B]",
            "service b(server)[B]\nservice a(server)[A]",
        ] {
            let output = svg(&format!("architecture-beta\n{declarations}\n{edge}"));
            let geometry = architecture_svg_geometry(&output);
            architecture_assert_edges_clear_icons(&geometry);
            assert!(!geometry.paths.is_empty());
        }
    }
    let simple = architecture_svg_geometry(&svg(
        "architecture-beta\nservice a(server)[A]\nservice b(server)[B]\na:R --> L:b",
    ));
    assert_eq!(simple.centers["B"].0 - simple.centers["A"].0, 150.0);
    assert_eq!(simple.centers["A"].1, simple.centers["B"].1);
    assert_eq!(
        simple.paths[0].len(),
        2,
        "clear aligned edges stay straight"
    );
}

#[test]
fn architecture_routes_around_clipped_group_captions() {
    let output = svg("architecture-beta\ngroup outer(cloud)[Outer]\ngroup inner(cloud)[WWWWWWWWWWWWWWWWWWWW] in outer\nservice a(server)[A] in outer\nservice b(server)[B] in inner\na:B --> T:b");
    let geometry = architecture_svg_geometry(&output);
    let (caption, x, baseline) = geometry
        .captions
        .iter()
        .find(|(name, _, _)| name.starts_with('W'))
        .unwrap();
    assert!(
        caption.ends_with('…'),
        "test must exercise the rendered clipped caption"
    );
    let width = caption.chars().filter(|&c| c == 'W').count() as f64 * 11.4 + 12.0;
    for path in &geometry.paths {
        for segment in path.windows(2) {
            let (a, b) = (segment[0], segment[1]);
            let crosses = if (a.1 - b.1).abs() < 0.01 {
                a.1 > baseline - 12.0
                    && a.1 < baseline + 3.0
                    && a.0.min(b.0) < x + width
                    && a.0.max(b.0) > *x
            } else {
                a.0 > *x
                    && a.0 < x + width
                    && a.1.min(b.1) < baseline + 3.0
                    && a.1.max(b.1) > baseline - 12.0
            };
            assert!(
                !crosses,
                "edge {path:?} crosses rendered group caption {caption}"
            );
        }
    }
    assert!(geometry.centers["A"].1 < geometry.centers["B"].1);
    architecture_assert_edges_clear_icons(&geometry);
}

#[test]
fn git_graph_rejects_unrendered_merge_types_explicitly() {
    let prefix = "gitGraph\ncommit\nbranch dev\ncommit\ncheckout main\n";
    for kind in ["HIGHLIGHT", "REVERSE"] {
        let output = render(&format!("{prefix}merge dev type: {kind}"));
        assert!(output.svg.is_none());
        let error = output.error.unwrap();
        assert_eq!(error.line, Some(6));
        assert!(error.message.contains("only type: NORMAL"));
        // These types remain supported for ordinary commits.
        svg(&format!("gitGraph\ncommit type: {kind}"));
    }
    svg(&format!("{prefix}merge dev type: NORMAL"));
    svg(&format!("{prefix}merge dev"));
}

#[test]
fn architecture_connections_meet_junction_edges_in_every_direction() {
    use quick_xml::{events::Event, Reader};
    for (side, opposite, dx, dy) in [
        ("L", "R", -1.0, 0.0),
        ("R", "L", 1.0, 0.0),
        ("T", "B", 0.0, -1.0),
        ("B", "T", 0.0, 1.0),
    ] {
        for from_junction in [true, false] {
            let edge = if from_junction {
                format!("j:{side} --> {opposite}:a")
            } else {
                format!("a:{opposite} --> {side}:j")
            };
            let output = svg(&format!(
                "architecture-beta\njunction j\nservice a(server)[A]\n{edge}"
            ));
            let mut reader = Reader::from_str(&output);
            let mut junction = None;
            let mut paths = Vec::new();
            loop {
                match reader.read_event().unwrap() {
                    Event::Empty(node) => {
                        let attrs: std::collections::HashMap<_, _> = node
                            .attributes()
                            .map(|attr| {
                                let attr = attr.unwrap();
                                (
                                    String::from_utf8(attr.key.as_ref().to_vec()).unwrap(),
                                    attr.unescape_value().unwrap().into_owned(),
                                )
                            })
                            .collect();
                        if node.name().as_ref() == b"circle"
                            && attrs.get("r").map(String::as_str) == Some("5")
                        {
                            junction = Some(
                                ["cx", "cy", "r"].map(|key| attrs[key].parse::<f64>().unwrap()),
                            );
                        }
                        if node.name().as_ref() == b"path" && attrs.contains_key("marker-end") {
                            paths.push(attrs["d"].clone());
                        }
                    }
                    Event::Eof => break,
                    _ => {}
                }
            }
            let [cx, cy, radius] = junction.expect("junction dot");
            assert_eq!(
                paths.len(),
                2,
                "stem and marker repaint must share connection geometry"
            );
            assert_eq!(paths[0], paths[1], "both paint passes must share routing");
            for path in paths {
                let coordinates: Vec<f64> = path
                    .split_whitespace()
                    .filter_map(|value| value.parse().ok())
                    .collect();
                assert!(coordinates.len() >= 4 && coordinates.len().is_multiple_of(2));
                let index = if from_junction {
                    0
                } else {
                    coordinates.len() - 2
                };
                assert!(
                    (coordinates[index] - (cx + dx * radius)).abs() < 0.05,
                    "{edge}: {path}"
                );
                assert!(
                    (coordinates[index + 1] - (cy + dy * radius)).abs() < 0.05,
                    "{edge}: {path}"
                );
            }
        }
    }
}

#[test]
fn gantt_literal_break_spelling_is_retained_and_fits_the_canvas() {
    use quick_xml::{events::Event, Reader};
    let label = "A<br/>".repeat(100);
    for source in [
        format!("gantt\n{label} :2024-01-01, 1d"),
        format!("gantt\ntitle {label}\nTask :2024-01-01, 1d"),
        format!("gantt\nsection {label}\nTask :2024-01-01, 1d"),
    ] {
        let output = svg(&source);
        let mut reader = Reader::from_str(&output);
        let mut width = None;
        let mut literal_found = false;
        loop {
            match reader.read_event().unwrap() {
                Event::Start(node) if node.name().as_ref() == b"svg" => {
                    let view = node
                        .attributes()
                        .map(Result::unwrap)
                        .find(|attr| attr.key.as_ref() == b"viewBox")
                        .unwrap()
                        .unescape_value()
                        .unwrap()
                        .into_owned();
                    width = Some(
                        view.split_whitespace()
                            .nth(2)
                            .unwrap()
                            .parse::<f64>()
                            .unwrap(),
                    );
                }
                Event::Text(text) if text.unescape().unwrap() == label => literal_found = true,
                Event::Eof => break,
                _ => {}
            }
        }
        assert!(literal_found, "literal label must remain complete");
        assert!(
            width.unwrap() > 3000.0,
            "600 literal ASCII glyphs cannot fit the old 760px canvas"
        );
    }
}

#[test]
fn git_graph_separates_long_tags_on_the_same_baseline() {
    use quick_xml::{events::Event, Reader};
    let first = "M".repeat(80);
    let second = "W".repeat(80);
    let output = svg(&format!(
        "gitGraph\ncommit tag: \"{first}\"\ncommit\ncommit tag: \"{second}\""
    ));
    let mut reader = Reader::from_str(&output);
    let mut current = None;
    let mut anchors = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(node) if node.name().as_ref() == b"text" => {
                let attrs: std::collections::HashMap<_, _> = node
                    .attributes()
                    .map(|attr| {
                        let attr = attr.unwrap();
                        (
                            String::from_utf8(attr.key.as_ref().to_vec()).unwrap(),
                            attr.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                current = Some(["x", "y"].map(|key| attrs[key].parse::<f64>().unwrap()));
            }
            Event::Text(text) => {
                let text = text.unescape().unwrap();
                if text == first || text == second {
                    anchors.push(current.expect("tag anchor"));
                }
            }
            Event::End(node) if node.name().as_ref() == b"text" => current = None,
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(anchors.len(), 2);
    assert_eq!(anchors[0][1], anchors[1][1]);
    assert!(
        anchors[1][0] - anchors[0][0] >= 800.0,
        "long tag footprints require more than the two original 50px gaps: {anchors:?}"
    );
}

#[test]
fn git_graph_literal_branch_and_id_text_get_complete_bounds() {
    use quick_xml::{events::Event, Reader};
    let literal = "A<br/>".repeat(50);
    let output = svg(&format!(
        "gitGraph\ncommit id: \"{literal}\"\nbranch \"{literal}\"\ncommit"
    ));
    let mut reader = Reader::from_str(&output);
    let mut canvas = None;
    let mut wide_branch = false;
    let mut literal_count = 0;
    loop {
        match reader.read_event().unwrap() {
            Event::Start(node) if node.name().as_ref() == b"svg" => {
                let attr = node
                    .attributes()
                    .map(Result::unwrap)
                    .find(|attr| attr.key.as_ref() == b"viewBox")
                    .unwrap();
                canvas = Some(
                    attr.unescape_value()
                        .unwrap()
                        .split_whitespace()
                        .map(|number| number.parse::<f64>().unwrap())
                        .collect::<Vec<_>>(),
                );
            }
            Event::Empty(node) if node.name().as_ref() == b"rect" => {
                let attrs: std::collections::HashMap<_, _> = node
                    .attributes()
                    .map(|attr| {
                        let attr = attr.unwrap();
                        (
                            String::from_utf8(attr.key.as_ref().to_vec()).unwrap(),
                            attr.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                if attrs.get("ry").map(String::as_str) == Some("4")
                    && attrs["width"].parse::<f64>().unwrap() > 1500.0
                {
                    wide_branch = true;
                }
            }
            Event::Text(text) if text.unescape().unwrap() == literal => literal_count += 1,
            Event::Eof => break,
            _ => {}
        }
    }
    let canvas = canvas.unwrap();
    assert!(
        canvas[2] > 1500.0 && canvas[3] > 800.0,
        "full literal branch and rotated ID need room: {canvas:?}"
    );
    assert!(
        wide_branch,
        "branch backing must use the same complete width as its gutter"
    );
    assert!(
        literal_count >= 2,
        "branch and ID captions must remain complete"
    );
}

#[test]
fn timeline_literal_labels_reserve_their_complete_single_line_width() {
    use quick_xml::{events::Event, Reader};
    let literal = "A<br/>".repeat(100);
    for source in [
        format!("timeline\ntitle {literal}\n2000 : Event"),
        format!("timeline\nsection {literal}\n2000 : Event"),
        format!("timeline\n{literal} : Event"),
        format!("timeline\n2000 : {literal}"),
    ] {
        let output = svg(&source);
        let mut reader = Reader::from_str(&output);
        let mut wide_canvas = false;
        let mut in_text = false;
        let mut retained = false;
        loop {
            match reader.read_event().unwrap() {
                Event::Start(node) if node.name().as_ref() == b"svg" => {
                    let attr = node
                        .attributes()
                        .map(Result::unwrap)
                        .find(|attr| attr.key.as_ref() == b"viewBox")
                        .unwrap();
                    wide_canvas = attr
                        .unescape_value()
                        .unwrap()
                        .split_whitespace()
                        .nth(2)
                        .unwrap()
                        .parse::<f64>()
                        .unwrap()
                        > 3000.0;
                }
                Event::Start(node) if node.name().as_ref() == b"text" => in_text = true,
                Event::Text(text) if in_text && text.unescape().unwrap() == literal => {
                    retained = true
                }
                Event::End(node) if node.name().as_ref() == b"text" => in_text = false,
                Event::Eof => break,
                _ => {}
            }
        }
        assert!(
            retained && wide_canvas,
            "full literal timeline caption must fit"
        );
    }
}

#[test]
fn mindmap_literal_sibling_labels_have_separate_single_line_footprints() {
    use quick_xml::{events::Event, Reader};
    let label = "A<br/>".repeat(50);
    let mut source = String::from("mindmap\nroot((Root))\n");
    for i in 0..4 {
        source.push_str(&format!(" child{i}[{label}]\n"));
    }
    let output = svg(&source);
    let mut reader = Reader::from_str(&output);
    let mut current = None;
    let mut anchors = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(node) if node.name().as_ref() == b"text" => {
                let attrs: std::collections::HashMap<_, _> = node
                    .attributes()
                    .map(|attr| {
                        let attr = attr.unwrap();
                        (
                            String::from_utf8(attr.key.as_ref().to_vec()).unwrap(),
                            attr.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                current = Some(["x", "y"].map(|key| attrs[key].parse::<f64>().unwrap()));
            }
            Event::Text(text) if current.is_some() && text.unescape().unwrap() == label => {
                anchors.push(current.unwrap())
            }
            Event::End(node) if node.name().as_ref() == b"text" => current = None,
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(anchors.len(), 4);
    for (i, a) in anchors.iter().enumerate() {
        for b in &anchors[..i] {
            assert!(
                (a[0] - b[0]).abs() >= 1600.0 || (a[1] - b[1]).abs() >= 20.0,
                "long sibling labels would overlap: {a:?}, {b:?}"
            );
        }
    }
}

#[test]
fn architecture_literal_break_spelling_is_fitted_as_text() {
    use quick_xml::{events::Event, Reader};
    let literal = "A<br/>".repeat(100);
    let output = svg(&format!(
        "architecture-beta\ngroup g(cloud)[{literal}]\nservice a(server)[{literal}] in g"
    ));
    let mut reader = Reader::from_str(&output);
    let mut in_text = false;
    let mut in_defs = 0;
    let mut captions = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(node) if node.name().as_ref() == b"defs" => in_defs += 1,
            Event::End(node) if node.name().as_ref() == b"defs" => in_defs -= 1,
            Event::Start(node) if node.name().as_ref() == b"text" => in_text = true,
            Event::End(node) if node.name().as_ref() == b"text" => in_text = false,
            Event::Text(text) if in_text && in_defs == 0 => {
                captions.push(text.unescape().unwrap().into_owned())
            }
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(captions.len(), 2);
    assert!(
        captions
            .iter()
            .all(|caption| caption.ends_with('…') && caption.chars().count() < 100),
        "literal names must fit within bounded service/group space: {captions:?}"
    );
}

#[test]
fn gantt_section_captions_reserve_a_separate_plot_gutter() {
    use quick_xml::{events::Event, Reader};
    let section = "M".repeat(80);
    let output = svg(&format!("gantt\nsection {section}\nTask :2024-01-01, 1d"));
    let mut reader = Reader::from_str(&output);
    let mut bars = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Empty(node) if node.name().as_ref() == b"rect" => {
                let attrs: std::collections::HashMap<_, _> = node
                    .attributes()
                    .map(|attr| {
                        let attr = attr.unwrap();
                        (
                            String::from_utf8(attr.key.as_ref().to_vec()).unwrap(),
                            attr.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                if attrs.get("rx").map(String::as_str) == Some("3") {
                    bars.push(["x", "width"].map(|key| attrs[key].parse::<f64>().unwrap()));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(bars.len(), 1);
    assert!(
        bars[0][0] > 800.0,
        "80 wide literal section glyphs must precede the plot: {bars:?}"
    );
    assert_eq!(
        bars[0][1], 620.0,
        "caption gutter must retain the plot's data scale"
    );
}

#[test]
fn git_graph_rotated_ids_reserve_space_before_lower_branch_captions() {
    use quick_xml::{events::Event, Reader};
    let id = "M".repeat(80);
    let branch = "M".repeat(20);
    let output = svg(&format!(
        "gitGraph\ncommit id: \"{id}\"\nbranch \"{branch}\"\ncommit tag: \"Lower tag\""
    ));
    let mut reader = Reader::from_str(&output);
    let mut current_y = None;
    let mut lower_y = None;
    loop {
        match reader.read_event().unwrap() {
            Event::Start(node) if node.name().as_ref() == b"text" => {
                let attr = node
                    .attributes()
                    .map(Result::unwrap)
                    .find(|attr| attr.key.as_ref() == b"y")
                    .unwrap();
                current_y = Some(attr.unescape_value().unwrap().parse::<f64>().unwrap());
            }
            Event::Text(text) if current_y.is_some() && text.unescape().unwrap() == branch => {
                lower_y = current_y
            }
            Event::End(node) if node.name().as_ref() == b"text" => current_y = None,
            Event::Eof => break,
            _ => {}
        }
    }
    assert!(
        lower_y.unwrap() > 600.0,
        "long rotated IDs need more than the original fixed lane gap"
    );
}

#[test]
fn secondary_outer_single_quotes_protect_literals_and_keep_apostrophes() {
    for (card, label) in [
        ("task['Literal ] text']", "Literal ] text"),
        ("task('Literal ) text')", "Literal ) text"),
        ("task['It's ] literal']", "It's ] literal"),
        ("task('It's ) literal')", "It's ) literal"),
        ("task[Don't lose this]", "Don't lose this"),
        ("task['Twas a note]", "'Twas a note"),
        ("'Literal [text]'", "Literal [text]"),
        ("task[\"It's ] literal\"]", "It's ] literal"),
    ] {
        let output = svg(&format!("kanban\nTodo\n {card}"));
        assert!(output.contains(label), "{card}: {output}");
    }
    for (node, label) in [
        ("'Literal :::hot'", "Literal :::hot"),
        ("'It's :::hot'", "It's :::hot"),
        ("child['It's ] :::hot']", "It's ] :::hot"),
        (
            "child['Literal ::icon(fa fa-book)']",
            "Literal ::icon(fa fa-book)",
        ),
        ("Don't lose this", "Don't lose this"),
        ("child[Don't lose this]", "Don't lose this"),
        ("'Twas a note", "'Twas a note"),
        ("\"It's :::hot\"", "It's :::hot"),
    ] {
        let output = svg(&format!("mindmap\nroot\n {node}"));
        assert!(output.contains(label), "{node}: {output}");
    }
    for source in [
        "kanban\nTodo\n task['It's ] literal'] other[bad]",
        "kanban\nTodo\n task['It's literal'] @{ priority: 'High' }",
        "mindmap\nroot\n 'It's literal':::hot",
        "mindmap\nroot\n child['It's :::hot']:::bad",
        "mindmap\nroot\n Don't :::hot",
    ] {
        let output = render(source);
        assert!(output.svg.is_none(), "must reject outside syntax: {source}");
        assert_eq!(output.error.unwrap().line, Some(3));
    }
}

#[test]
fn kanban_plain_prose_retains_brackets_parentheses_and_prefixes() {
    let output = svg("kanban\nIn Progress (WIP)\n Write docs (v2)\n Review [PR 12]\n Release/v2 [notes]\n task[Display]\n next(Display too)");
    for label in [
        "In Progress (WIP)",
        "Write docs (v2)",
        "Review [PR 12]",
        "Release/v2 [notes]",
        "Display",
        "Display too",
    ] {
        assert!(
            output.contains(label),
            "lost literal label {label}: {output}"
        );
    }
    assert!(!output.contains("task["));
    assert!(!output.contains("next("));
}

#[test]
fn git_graph_distinguishes_empty_branch_merges_from_equal_nonempty_heads() {
    let empty = render("gitGraph\nbranch dev\nmerge main");
    let error = empty.error.unwrap();
    assert_eq!(error.line, Some(3));
    assert!(error.message.contains("without commits"));
    let same = render("gitGraph\ncommit\nbranch dev\nmerge main");
    let error = same.error.unwrap();
    assert_eq!(error.line, Some(4));
    assert!(error.message.contains("same head commit"));
}

#[test]
fn architecture_reports_unseparable_group_declarations_not_unrelated_edges() {
    let source="architecture-beta\ngroup ga(cloud)[GA]\ngroup gb(cloud)[GB]\nservice a(server)[A] in ga\nservice b(server)[B] in gb\nservice c(server)[C] in ga\nservice d(server)[D] in gb\nservice e(server)[E]\nservice f(server)[F]\na:R -- L:b\nb:R -- L:c\na:B -- T:d\nd:B -- T:c\ne:R -- L:f";
    let output = render(source);
    assert!(output.svg.is_none());
    let error = output.error.unwrap();
    assert!(matches!(error.line, Some(2 | 3)), "{error:?}");
    assert!(error.message.contains("groups cannot be separated"));
}

#[test]
fn architecture_edge_limit_has_an_original_source_line() {
    let prefix = "architecture-beta\nservice a(server)[A]\nservice b(server)[B]\n";
    let allowed = format!("{prefix}{}", "a:R -- L:b\n".repeat(1000));
    svg(&allowed);
    let output = render(&format!("{allowed}a:R -- L:b\n"));
    assert!(output.svg.is_none());
    let error = output.error.unwrap();
    assert_eq!(error.line, Some(1004));
    assert!(error.message.contains("1000 edges"));
}

#[test]
fn mindmap_bare_backticks_remain_literal_but_markdown_strings_reject() {
    for (node, label) in [
        ("Run `ls`", "Run `ls`"),
        ("child[Use `cargo`]", "Use `cargo`"),
        ("`literal`", "`literal`"),
        ("child[`literal`]", "`literal`"),
        ("child[\"Use `cargo`\"]", "Use `cargo`"),
        ("child['`literal`']", "`literal`"),
    ] {
        let output = svg(&format!("mindmap\nroot\n {node}"));
        assert!(output.contains(label), "{node}: {output}");
    }
    for node in ["child[\"`**bold**`\"]", "\"`**bold**`\""] {
        let output = render(&format!("mindmap\nroot\n {node}"));
        assert!(output.svg.is_none());
        let error = output.error.unwrap();
        assert_eq!(error.line, Some(3));
        assert!(error.message.contains("markdown labels"));
    }
}

#[test]
fn git_graph_rejects_merges_into_an_empty_current_branch() {
    let output = render("gitGraph\nbranch dev\ncommit\ncheckout main\nmerge dev");
    assert!(output.svg.is_none());
    let error = output.error.unwrap();
    assert_eq!(error.line, Some(5));
    assert!(error.message.contains("into a branch without commits"));
    svg("gitGraph\ncommit\nbranch dev\ncommit\ncheckout main\nmerge dev");
}

#[test]
fn architecture_hyphenated_ids_resolve_groups_junctions_and_arrows() {
    for arrow in ["--", "-->", "<--", "<-->"] {
        let output=svg(&format!("architecture-beta\nservice api--gw(server)[Gateway] in private--api\ngroup private--api(cloud)[Private] in public-api\ngroup public-api(cloud)[API]\njunction edge-junction in private--api\nservice db-main(database)[Database] in public-api\napi--gw:R {arrow} L:edge-junction\nedge-junction:B {arrow} T:db-main"));
        let geometry = architecture_svg_geometry(&output);
        assert_eq!(geometry.groups.len(), 2);
        assert_eq!(
            geometry.paths.len(),
            2,
            "must retain both hyphenated-id connections"
        );
        assert!(
            geometry.centers.contains_key("Gateway") && geometry.centers.contains_key("Database")
        );
        architecture_assert_edges_clear_icons(&geometry);
    }
    for declaration in [
        "service -api(server)[Bad]",
        "service api-(server)[Bad]",
        "group -group(cloud)[Bad]",
        "junction j-",
    ] {
        let output = render(&format!("architecture-beta\n{declaration}"));
        assert!(output.svg.is_none());
        assert_eq!(output.error.unwrap().line, Some(2));
    }
    let output = render("architecture-beta\nservice api-gw(server)[Gateway] in unknown-group");
    let error = output.error.unwrap();
    assert_eq!(error.line, Some(2));
    assert!(error
        .message
        .contains("unknown architecture group `unknown-group`"));
}

#[test]
fn architecture_renders_declared_group_icons_and_empty_group_hierarchy() {
    let output=svg("architecture-beta\ngroup outer(cloud)[Outer]\ngroup inner(disk)[Inner] in outer\ngroup solo(server)[Solo]\nservice a(server)[A]");
    let geometry = architecture_svg_geometry(&output);
    assert_eq!(geometry.groups.len(), 3);
    assert_eq!(
        output
            .matches("data-architecture-group-icon=\"true\"")
            .count(),
        3
    );
    for title in ["Outer", "Inner", "Solo"] {
        assert!(
            geometry.captions.iter().any(|(text, _, _)| text == title),
            "missing {title}: {output}"
        );
    }
    let (outer, inner) = (geometry.groups[0], geometry.groups[2]);
    assert!(
        outer[0] < inner[0]
            && outer[1] + 24.0 <= inner[1]
            && outer[0] + outer[2] > inner[0] + inner[2]
            && outer[1] + outer[3] > inner[1] + inner[3]
    );
    for bounds in geometry.groups {
        assert!(bounds[2] >= 64.0 && bounds[3] >= 40.0);
    }
    let group_only = svg("architecture-beta\ngroup g(cloud)[G]");
    assert_eq!(architecture_svg_geometry(&group_only).groups.len(), 1);
    let all=svg("architecture-beta\ngroup a(cloud)[A]\ngroup b(server)[B]\ngroup c(disk)[C]\ngroup d(database)[D]\ngroup e(internet)[E]");
    assert_eq!(
        all.matches("data-architecture-group-icon=\"true\"").count(),
        5
    );
}

#[test]
fn kanban_html_break_spelling_remains_literal_text() {
    let output = svg("kanban\nHeader<br/>literal\n task[Card<br/>literal]");
    assert!(output.contains("Header&lt;br/&gt;literal"));
    assert!(output.contains("Card&lt;br/&gt;literal"));
}

#[test]
fn gantt_section_background_and_title_follow_the_complete_canvas() {
    use quick_xml::{events::Event, Reader};
    for section in ["", "section S\n"] {
        let output = svg(&format!(
            "gantt\ntitle Wide chart\n{section}{} :2024-01-01, 1d",
            "W".repeat(160)
        ));
        let mut reader = Reader::from_str(&output);
        let mut view = None;
        let mut title = None;
        let mut bands = Vec::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Start(node) | Event::Empty(node) => {
                    let attrs: std::collections::HashMap<_, _> = node
                        .attributes()
                        .map(|a| {
                            let a = a.unwrap();
                            (
                                String::from_utf8(a.key.as_ref().to_vec()).unwrap(),
                                a.unescape_value().unwrap().into_owned(),
                            )
                        })
                        .collect();
                    if node.name().as_ref() == b"svg" {
                        view = Some(
                            attrs["viewBox"]
                                .split_whitespace()
                                .map(|v| v.parse::<f64>().unwrap())
                                .collect::<Vec<_>>(),
                        );
                    }
                    if node.name().as_ref() == b"text"
                        && attrs.get("y").map(String::as_str) == Some("24")
                    {
                        title = Some(attrs["x"].parse::<f64>().unwrap());
                    }
                    if node.name().as_ref() == b"rect"
                        && attrs
                            .get("fill")
                            .is_some_and(|fill| fill.contains("mermaid-gantt-section"))
                    {
                        bands.push((
                            attrs["x"].parse::<f64>().unwrap(),
                            attrs["width"].parse::<f64>().unwrap(),
                        ));
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        let view = view.unwrap();
        assert!((title.unwrap() - (view[0] + view[2] / 2.0)).abs() < 0.1);
        assert!(!bands.is_empty());
        for (x, width) in bands {
            assert!((x - view[0]).abs() < 0.1 && (width - view[2]).abs() < 0.1);
        }
    }
}

#[test]
fn git_graph_quoted_values_preserve_literal_backslashes() {
    for value in [r"C:\", r"C:\work\", r"two\\slashes", r"C:\new\test"] {
        let output = svg(&format!(
            "gitGraph\ncommit id: \"{value}\" tag: \"{value}\""
        ));
        assert!(output.contains(value), "lost literal {value}: {output}");
    }
    let output =
        svg("gitGraph\ncommit\nbranch \"C:\\work\\\"\ncommit\ncheckout \"C:\\work\\\"\ncommit");
    assert!(output.contains(r"C:\work\"));
}

#[test]
fn architecture_same_port_self_edges_reject_without_changing_other_self_loops() {
    for declaration in ["service a(server)[A]", "junction a"] {
        for side in ["L", "R", "T", "B"] {
            for arrow in ["--", "-->", "<--", "<-->"] {
                let source = format!("architecture-beta\n%% source-line control\n{declaration}\n\na:{side} {arrow} {side}:a");
                let output = render(&source);
                assert!(
                    output.svg.is_none(),
                    "must reject coincident ports: {source}"
                );
                let error = output.error.unwrap();
                assert_eq!(error.line, Some(5));
                assert!(error.message.contains("self edges using the same port"));
                for target in ["L", "R", "T", "B"]
                    .into_iter()
                    .filter(|&target| target != side)
                {
                    let output = svg(&format!(
                        "architecture-beta\n{declaration}\na:{side} {arrow} {target}:a"
                    ));
                    architecture_assert_edges_clear_icons(&architecture_svg_geometry(&output));
                }
            }
        }
    }
}

#[test]
fn kanban_columns_expand_for_indivisible_graphemes_but_keep_ordinary_caps() {
    use quick_xml::{events::Event, Reader};
    use unicode_segmentation::UnicodeSegmentation;
    let cluster = format!("{}क", "क्".repeat(100));
    assert_eq!(cluster.graphemes(true).count(), 1);
    for header in [false, true] {
        let source = if header {
            format!("kanban\n{cluster}\n task[Normal]")
        } else {
            format!("kanban\nTodo\n task[{cluster}]")
        };
        let output = svg(&source);
        assert!(
            output.contains(&format!(">{cluster}</text>")),
            "must retain the whole grapheme in one rendered text run"
        );
        let mut reader = Reader::from_str(&output);
        let mut column_width = None;
        loop {
            match reader.read_event().unwrap() {
                Event::Empty(node) if node.name().as_ref() == b"rect" => {
                    let attrs: std::collections::HashMap<_, _> = node
                        .attributes()
                        .map(|a| {
                            let a = a.unwrap();
                            (
                                String::from_utf8(a.key.as_ref().to_vec()).unwrap(),
                                a.unescape_value().unwrap().into_owned(),
                            )
                        })
                        .collect();
                    if attrs.get("fill-opacity").map(String::as_str) == Some("0.3") {
                        column_width = Some(attrs["width"].parse::<f64>().unwrap());
                        break;
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        assert!(column_width.unwrap() > 240.0);
    }
    let output = svg(&format!(
        "kanban\n{}\n task[{}]",
        "W".repeat(200),
        "W".repeat(200)
    ));
    assert!(
        output.contains("viewBox=\"0 0 280 "),
        "ordinary long words retain the capped column: {output}"
    );
    assert!(
        !output.contains(&format!(">{}</text>", "W".repeat(200))),
        "ordinary long words continue to wrap"
    );
}
