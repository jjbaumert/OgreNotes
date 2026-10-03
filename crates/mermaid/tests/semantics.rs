//! Regression cases for syntax that previously rendered with changed meaning.
use ogrenotes_mermaid::render;

#[test]
fn unsupported_and_malformed_statements_keep_original_error_lines() {
    for (source, line) in [
        ("gantt\ndateFormat YYYY-DD-MM\nt :2024-01-02, 2d", 2),
        ("gantt\nexcludes weekends\nt :2024-01-05, 2d", 2),
        ("gantt\naxisFormat %d/%m\nt :2024-01-05, 2d", 2),
        ("xychart-beta\ny-axis 0 --> nope\nbar [1]", 2),
        ("xychart-beta\nbar [1e308]\nline [-1e308]\nline [1]", 3),
        ("xychart-beta\ny-axis 10 --> 0\nbar [1]", 2),
        ("xychart-beta\ny-axis 0 --> 0\nbar [1]", 2),
        ("xychart-beta\ny-axis -1e308 --> 1e308\nbar [1]", 2),
        ("xychart-beta\nx-axis [\"unclosed, A]\nbar [1]", 2),
        ("quadrantChart\nBad: [NaN, 0.5]", 2),
        ("quadrantChart\nBad: [inf, 0.5]", 2),
        ("quadrantChart\nBad: [2, 0.5]", 2),
        ("packet-beta\n0-7: \"A\"\n4-9: \"B\"", 3),
        ("packet-beta\n+0: \"A\"", 2),
        ("graph LR A --> B", 1),
        ("C4Context\nMadeUp(a, \"Lost\")", 2),
        ("treemap-beta\nclassDef wrong fill:red", 2),
        (
            "---\nconfig: {}\n---\nxychart-beta\naccTitle: Chart\ny-axis nope --> 2\nbar [1]",
            6,
        ),
    ] {
        let out = render(source);
        assert!(out.svg.is_none(), "must reject {source}");
        let error = out.error.unwrap();
        assert_eq!(error.line, Some(line), "{source}: {error}");
        assert!(error.to_string().starts_with(&format!("Line {line}:")));
    }
}

#[test]
fn xy_titles_categories_and_numeric_ranges_retain_their_meaning() {
    let out = render(
        "xychart\nx-axis \"Months\" [\"Jan, first\", Feb]\ny-axis Revenue 0 --> 100\nbar [20,40]",
    );
    let svg = out.svg.unwrap();
    for label in ["Months", "Jan, first", "Feb", "Revenue", "100"] {
        assert!(
            svg.contains(&format!(">{label}</text>")),
            "missing {label}: {svg}"
        );
    }
    let svg = render("xychart-beta\nx-axis \"Time\" 10 --> 20\nline [1,2,3]")
        .svg
        .unwrap();
    for label in ["10", "15", "20", "Time"] {
        assert!(svg.contains(&format!(">{label}</text>")), "{svg}");
    }
}

#[test]
fn relative_packet_fields_follow_previous_fields() {
    let svg = render("packet-beta\n+8: \"First\"\n+8: \"Second\"")
        .svg
        .unwrap();
    for label in ["0", "7", "8", "15"] {
        assert!(svg.contains(&format!(">{label}</text>")), "{svg}");
    }
}

#[test]
fn accessibility_metadata_is_escaped_and_does_not_add_shared_ids() {
    for header in [
        "flowchart TD\nA --> B",
        "pie\n\"A\":1",
        "xychart-beta\nbar [1]",
    ] {
        let (header, body) = header.split_once('\n').unwrap();
        let source =
            format!("{header}\naccTitle: Name <&>\naccDescr {{\nDescription <&>\n}}\n{body}");
        let svg = render(&source).svg.unwrap();
        assert!(svg.contains("role=\"img\""));
        assert!(svg.contains("aria-label=\"Name &lt;&amp;&gt;\""));
        assert!(svg.contains("<desc>Description &lt;&amp;&gt;</desc>"));
        assert!(
            !svg.contains("aria-labelledby"),
            "cached instances cannot share title IDs"
        );
    }
    let error = render("pie\naccDescr {\nunfinished").error.unwrap();
    assert_eq!(error.line, Some(2));
}

#[test]
fn axis_edge_cases_do_not_overlap_or_lose_error_lines() {
    let svg = render("xychart-beta\nx-axis 0 --> 100\nbar [5]")
        .svg
        .unwrap();
    for x in ["76.0", "306.0", "536.0"] {
        assert!(
            svg.contains(&format!("<text x=\"{x}\" y=\"336.0\"")),
            "missing numeric tick at {x}: {svg}"
        );
    }
    let error = render("xychart-beta\nline [-1e308, 1e308]").error.unwrap();
    assert_eq!(error.line, Some(2));
    assert!(
        render("xychart-beta\ny-axis Infinity\nbar [1]")
            .svg
            .is_some()
    );
    let svg = render("xychart-beta\ny-axis -0.1 --> 0.10000000000000001\nbar [0]")
        .svg
        .unwrap();
    assert!(!svg.contains("5.55e-17"));
    assert!(svg.contains(">0</text>"));
    let svg = render("flowchart TD\naccTitle : Spaced\naccDescr { First\nSecond }\nA --> B")
        .svg
        .unwrap();
    assert!(svg.contains("<title>Spaced</title>"));
    assert!(svg.contains("<desc>First\nSecond</desc>"));
    let svg = render("flowchart TD\nA -->|uses| B").svg.unwrap();
    assert!(svg.contains("<desc>uses; A; B</desc>"));
}

#[test]
fn descriptions_include_visible_labels_once_and_preserve_metadata_named_nodes() {
    for (source, label) in [
        (
            "C4Context\nPerson(p, \"Reader\", \"description\")",
            "Reader",
        ),
        (
            "quadrantChart\nquadrant-1 Unique\nPoint: [0.5,0.5]",
            "Unique",
        ),
        (
            "architecture-beta\nservice db(database)[Database]",
            "Database",
        ),
    ] {
        let svg = render(source).svg.unwrap();
        let description = svg
            .split("<desc>")
            .nth(1)
            .unwrap()
            .split("</desc>")
            .next()
            .unwrap();
        assert_eq!(description.matches(label).count(), 1, "{description}");
    }
    let svg = render("flowchart TD\naccDescr{Decision}\naccDescr --> Next")
        .svg
        .unwrap();
    assert!(svg.contains("Decision"), "{svg}");
    assert!(svg.contains("Next"), "{svg}");
    let svg = render("flowchart TD\naccTitle:::highlight\naccTitle --> Next")
        .svg
        .unwrap();
    assert!(svg.contains("accTitle"), "{svg}");
}

#[test]
fn numeric_x_range_ticks_match_the_data_endpoints() {
    for series in ["line [1,2,3]", "bar [1,2,3]"] {
        let svg = render(&format!("xychart-beta\nx-axis 10 --> 20\n{series}"))
            .svg
            .unwrap();
        let expected = if series.starts_with("line") {
            ["76.0", "306.0", "536.0"]
        } else {
            ["152.7", "306.0", "459.3"]
        };
        for (x, label) in expected.into_iter().zip(["10", "15", "20"]) {
            assert!(svg.contains(&format!("<text x=\"{x}\" y=\"336.0\" text-anchor=\"middle\" font-size=\"11\" fill=\"currentColor\">{label}</text>")), "{svg}");
        }
        if series.starts_with("line") {
            assert!(svg.contains("points=\"76.0,"), "{svg}");
            assert!(svg.contains(" 536.0,"), "{svg}");
        }
    }
}

#[test]
fn supported_gantt_directives_and_inline_header_comments_render() {
    let out = render(
        "gantt\ndateFormat YYYY-MM-DD\naxisFormat %Y-%m-%d\ntodayMarker off\nt :2024-01-02, 2d",
    );
    assert!(out.svg.is_some(), "{:?}", out.error);
    let out = render("flowchart TD %% inline comment\nA --> B");
    assert!(out.svg.is_some(), "{:?}", out.error);
}
