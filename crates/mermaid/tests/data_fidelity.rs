//! Renderer-level regressions for the data-diagram audit (#282 and #283).
use ogrenotes_mermaid::render;
use quick_xml::{events::Event, Reader};
use std::collections::HashMap;

fn rendered(source: &str) -> String {
    let output = render(source);
    output
        .svg
        .unwrap_or_else(|| panic!("{source}: {:?}", output.error))
}

fn elements(svg: &str, name: &[u8]) -> Vec<HashMap<String, String>> {
    let mut reader = Reader::from_str(svg);
    let mut found = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(tag) | Event::Empty(tag) if tag.name().as_ref() == name => {
                found.push(
                    tag.attributes()
                        .map(|a| {
                            let a = a.unwrap();
                            (
                                String::from_utf8(a.key.as_ref().to_vec()).unwrap(),
                                a.unescape_value().unwrap().into_owned(),
                            )
                        })
                        .collect(),
                );
            }
            Event::Eof => break,
            _ => {}
        }
    }
    found
}

#[test]
fn malformed_data_is_rejected_at_its_original_line() {
    for (source, line) in [
        ("sankey-beta\na,b,-1", 2),
        ("sankey-beta\na,b,+1", 2),
        ("quadrantChart\nA: [+0.5,0.5]", 2),
        ("xychart-beta\nbar [+1]", 2),
        ("xychart-beta\ny-axis +0 --> 10\nbar [1]", 2),
        ("radar-beta\naxis A,B,C\ncurve c{1,+2,3}", 3),
        ("radar-beta\naxis A,B,C\nmax +3", 3),
        ("treemap-beta\n\"a\":+1", 2),
        ("packet\n0-+7: \"A\"", 2),
        ("packet\n++8: \"A\"", 2),
        ("sankey-beta\na,a,5", 2),
        ("sankey-beta\na,b,5\nb,c,5\nc,a,5", 4),
        ("sankey-beta\na,b,5,extra", 2),
        ("sankey-beta\n\"a,b,5", 2),
        ("packet\n0-7: \"A\"\n16-23: \"B\"", 3),
        ("radar-beta\naxis A,B,C\nmax oops\ncurve c{1,2,3}", 3),
        ("radar-beta\naxis A,B,C\nmin 2\nmax 1", 4),
        ("radar-beta\naxis A,B,C\nticks 0", 3),
        ("radar-beta\naxis A,B,C\nshowLegend maybe", 3),
        ("radar-beta\naxis A,B,C\ngraticule square", 3),
        ("treemap-beta\n\"a\":10:::hot", 2),
        ("treemap-beta\n\"a\":NaN", 2),
        ("treemap-beta\n\"a\":oops", 2),
        ("requirementDiagram\nrequirement r {\nrisk: nonsense\n}", 3),
        (
            "requirementDiagram\nrequirement r {\nverifymethod: magic\n}",
            3,
        ),
        ("requirementDiagram\nrequirement r {\nunknown: value\n}", 3),
        ("C4Context\nSystem(a,\"A\")\nAddElementTag(\"hot\")", 3),
        (
            "C4Context\nSystem(a,\"A\")\nUpdateElementStyle(a,\"red\")",
            3,
        ),
        ("C4Context\nSystem(a,\"A\") trailing", 2),
    ] {
        let output = render(source);
        assert!(output.svg.is_none(), "must reject: {source}");
        assert_eq!(output.error.unwrap().line, Some(line), "{source}");
    }
}

#[test]
fn horizontal_xy_chart_preserves_labels_and_draws_horizontal_bars() {
    let svg = rendered("xychart-beta horizontal\nx-axis \"Month\" [Jan,Feb]\ny-axis Revenue 0 --> 100\nbar [20,40]\nline [30,60]");
    for label in ["Month", "Jan", "Feb", "Revenue", "100"] {
        assert!(svg.contains(&format!(">{label}</text>")), "{svg}");
    }
    let bars = elements(&svg, b"rect");
    assert_eq!(bars.len(), 2);
    let value = |i: usize, key: &str| bars[i][key].parse::<f64>().unwrap();
    assert_eq!(value(0, "x"), value(1, "x"));
    assert_eq!(value(0, "height"), value(1, "height"));
    assert!((value(1, "width") - 2.0 * value(0, "width")).abs() < 0.2);
    assert!(value(1, "y") > value(0, "y"));
    assert!(svg.contains("<polyline"));
    assert!(rendered("xychart horizontal\nline [1,2]").contains("<polyline"));
}

#[test]
fn sankey_tall_columns_and_every_ribbon_fit_the_canvas() {
    let source = format!(
        "sankey-beta\n{}",
        (0..60)
            .map(|i| format!("a,b{i},1"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let svg = rendered(&source);
    let view = elements(&svg, b"svg")[0]["viewBox"]
        .split_whitespace()
        .map(|n| n.parse::<f64>().unwrap())
        .collect::<Vec<_>>();
    let bars = elements(&svg, b"rect");
    assert_eq!(bars.len(), 61);
    for bar in bars {
        let y = bar["y"].parse::<f64>().unwrap();
        let h = bar["height"].parse::<f64>().unwrap();
        assert!(y >= 0.0 && y + h <= view[3] + 1.0, "{bar:?}");
    }
    for path in elements(&svg, b"path") {
        let coordinates: Vec<f64> = path["d"]
            .split_whitespace()
            .filter_map(|token| token.parse().ok())
            .collect();
        for point in coordinates.chunks_exact(2) {
            assert!(
                point[0] >= 0.0
                    && point[0] <= view[2] + 1.0
                    && point[1] >= 0.0
                    && point[1] <= view[3] + 1.0,
                "{path:?}"
            );
        }
    }
    assert!(!svg.contains("NaN"));
    assert!(!svg.contains("inf"));
}

#[test]
fn c4_relationships_resolve_after_declarations_and_back_arrows_reverse() {
    let declarations = "System(a,\"A\")\nSystem(b,\"B\")";
    let forward = rendered(&format!("C4Context\nRel(a,b,\"uses\")\n{declarations}"));
    let declared_first = rendered(&format!("C4Context\n{declarations}\nRel(a,b,\"uses\")"));
    assert_eq!(forward, declared_first);
    let reverse = rendered(&format!(
        "C4Context\n{declarations}\nRel_Back(a,b,\"uses\")"
    ));
    let explicit = rendered(&format!("C4Context\n{declarations}\nRel(b,a,\"uses\")"));
    assert_eq!(reverse, explicit);
    let indexed = rendered(&format!(
        "C4Dynamic\nRelIndex(7,a,b,\"uses\")\n{declarations}"
    ));
    assert!(indexed.contains(">1: uses</text>"));
}

#[test]
fn c4_descriptions_wrap_without_losing_the_tail() {
    let description = format!("{}finalword", "many descriptive words ".repeat(30));
    let svg = rendered(&format!("C4Context\nSystem(a,\"A\",\"{description}\")"));
    assert!(svg.contains("finalword"));
    assert!(!svg.contains('…'));
    let boxes = elements(&svg, b"rect");
    assert!(boxes.iter().any(|b| b
        .get("height")
        .is_some_and(|h| h.parse::<f64>().unwrap() > 100.0)));
}

#[test]
fn requirement_forward_references_keep_the_declared_stereotype() {
    let svg = rendered("requirementDiagram\na - satisfies -> b\nelement a {\ntype: sim\n}\ndesignConstraint b {\nrisk: High\nverifymethod: Test\n}");
    assert!(svg.contains("«designConstraint»"));
    assert!(svg.contains("risk: High"));
}

#[test]
fn radar_axes_accumulate_and_display_options_control_the_chart() {
    let svg = rendered("radar-beta\naxis A\naxis B,C\nmin 1\nmax 4\nticks 3\ngraticule circle\nshowLegend false\ncurve c{1,2,3}");
    for label in ["A", "B", "C", "2", "3", "4"] {
        assert!(svg.contains(&format!(">{label}</text>")), "{svg}");
    }
    assert_eq!(elements(&svg, b"circle").len(), 3);
    assert_eq!(elements(&svg, b"polygon").len(), 1);
    assert!(!svg.contains(">c</text>"));
}

#[test]
fn treemap_accepts_the_detected_header_and_preserves_colons_in_quoted_names() {
    let svg = rendered("treemap\n\"a:::b\": 10\n\"second\": 20");
    assert!(svg.contains("a:::b"));
    assert!(svg.contains(">10</text>"));
}

#[test]
fn c4_wraps_literal_labels_without_losing_or_splitting_graphemes() {
    use unicode_segmentation::UnicodeSegmentation;
    for name in [
        "literal<br/>caption".to_string(),
        format!("{} \u{0301}B", "W".repeat(15)),
        "A\u{00a0}B".to_string(),
        "A  B".to_string(),
        "A\u{202f}B".to_string(),
        "e\u{301}".repeat(40),
        "क्‍ष".repeat(20),
        "가".repeat(30),
        "가".repeat(30),
        "ДΩ".repeat(25),
    ] {
        let svg = rendered(&format!("C4Context\nPerson(p,\"{name}\")"));
        let mut reader = Reader::from_str(&svg);
        let mut active = false;
        let mut names = Vec::new();
        let mut line = String::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Start(tag) if tag.name().as_ref() == b"text" => {
                    let attrs: HashMap<_, _> = tag
                        .attributes()
                        .map(|a| {
                            let a = a.unwrap();
                            (
                                a.key.as_ref().to_vec(),
                                a.unescape_value().unwrap().into_owned(),
                            )
                        })
                        .collect();
                    active = attrs
                        .get(b"font-weight".as_slice())
                        .is_some_and(|v| v == "700")
                        && !attrs.contains_key(b"data-label-underlay".as_slice());
                    line.clear();
                }
                Event::Text(text) if active => line.push_str(&text.unescape().unwrap()),
                Event::End(tag) if tag.name().as_ref() == b"text" => {
                    if active {
                        names.push(std::mem::take(&mut line));
                    }
                    active = false;
                }
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(names.concat(), name, "C4 source text must remain complete");
        let mut boundaries = std::collections::HashSet::from([0]);
        for (offset, grapheme) in name.grapheme_indices(true) {
            boundaries.insert(offset + grapheme.len());
        }
        let mut offset = 0;
        for line in names {
            offset += line.len();
            assert!(
                boundaries.contains(&offset),
                "line boundary splits a grapheme in {name}"
            );
        }
    }
}

#[test]
fn sankey_cycle_errors_name_a_cycle_edge_not_trailing_or_downstream_rows() {
    for (source, cycle_lines) in [
        (
            "sankey-beta\na,b,1\nb,a,1\nx,y,1\n%% harmless trailing comment",
            vec![2, 3],
        ),
        (
            "sankey-beta\na,b,1\nb,a,1\nb,c,1\nc,d,1\n%% harmless trailing comment",
            vec![2, 3],
        ),
        (
            "sankey-beta\nb,c,1\nc,d,1\na,b,1\nb,a,1\nx,y,1\n%% harmless trailing comment",
            vec![4, 5],
        ),
    ] {
        let result = render(source);
        assert!(result.svg.is_none(), "cyclic flow must not render");
        let error = result.error.unwrap();
        assert!(error.message.contains("cycles"));
        assert!(
            cycle_lines.contains(&error.line.expect("cycle edge source line")),
            "{source}: {error}"
        );
    }
}

#[test]
fn long_horizontal_axis_titles_are_wrapped_inside_the_viewbox() {
    let title = "W".repeat(100);
    for axis in ["x-axis", "y-axis"] {
        let svg = rendered(&format!("xychart horizontal\n{axis} {title}\nbar [1,2]"));
        let view = elements(&svg, b"svg")[0]["viewBox"]
            .split_whitespace()
            .map(|n| n.parse::<f64>().unwrap())
            .collect::<Vec<_>>();
        let mut reader = Reader::from_str(&svg);
        let mut attributes = HashMap::<String, String>::new();
        let mut text = String::new();
        let mut recovered = String::new();
        loop {
            match reader.read_event().unwrap() {
                Event::Start(tag) if tag.name().as_ref() == b"text" => {
                    attributes = tag
                        .attributes()
                        .map(|a| {
                            let a = a.unwrap();
                            (
                                String::from_utf8(a.key.as_ref().to_vec()).unwrap(),
                                a.unescape_value().unwrap().into_owned(),
                            )
                        })
                        .collect();
                    text.clear();
                }
                Event::Text(value) => text.push_str(&value.unescape().unwrap()),
                Event::End(tag)
                    if tag.name().as_ref() == b"text"
                        && !text.is_empty()
                        && text.chars().all(|ch| ch == 'W') =>
                {
                    recovered.push_str(&text);
                    let x = attributes["x"].parse::<f64>().unwrap();
                    let y = attributes["y"].parse::<f64>().unwrap();
                    // An em per W is conservative at the declared 12px size.
                    // Apply the emitted -90-degree rotation for the category title.
                    let width = text.len() as f64 * 12.0;
                    let bounds = if let Some(transform) = attributes.get("transform") {
                        assert!(transform.starts_with("rotate(-90 "));
                        [x - 12.0, y - width / 2.0, x + 3.0, y + width / 2.0]
                    } else {
                        [x - width / 2.0, y - 12.0, x + width / 2.0, y + 3.0]
                    };
                    assert!(
                        bounds[0] >= 0.0
                            && bounds[1] >= 0.0
                            && bounds[2] <= view[2]
                            && bounds[3] <= view[3],
                        "{axis}: {bounds:?} outside {view:?}"
                    );
                }
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(
            recovered, title,
            "wrapping must retain the entire axis title"
        );
    }
}

#[test]
fn tiny_explicit_xy_ranges_reach_the_maximum_tick_in_both_orientations() {
    for (lo, hi, label) in [
        ("0", "1e-12", "1.00e-12"),
        ("-2e-12", "-1e-12", "-1.00e-12"),
        ("0", "5e-324", "4.94e-324"),
    ] {
        for horizontal in [false, true] {
            let header = if horizontal {
                "xychart horizontal"
            } else {
                "xychart"
            };
            let svg = rendered(&format!("{header}\ny-axis {lo} --> {hi}\nbar [{hi}]"));
            assert!(
                svg.contains(&format!(">{label}</text>")),
                "maximum tick missing: {svg}"
            );
            let bars = elements(&svg, b"rect");
            assert_eq!(bars.len(), 1);
            let value = |key: &str| bars[0][key].parse::<f64>().unwrap();
            let grids: Vec<_> = elements(&svg, b"line")
                .into_iter()
                .filter(|line| line.get("opacity").is_some_and(|v| v == "0.2"))
                .collect();
            if horizontal {
                let max_tick_x = grids
                    .iter()
                    .map(|line| line["x1"].parse::<f64>().unwrap())
                    .fold(f64::NEG_INFINITY, f64::max);
                assert_eq!(
                    value("width"),
                    460.0,
                    "the maximum fills the complete plot width"
                );
                assert!((value("x") + value("width") - max_tick_x).abs() < 0.1);
            } else {
                let max_tick_y = grids
                    .iter()
                    .map(|line| line["y1"].parse::<f64>().unwrap())
                    .fold(f64::INFINITY, f64::min);
                assert_eq!(
                    value("height"),
                    300.0,
                    "the maximum fills the complete plot height"
                );
                assert!((value("y") - max_tick_y).abs() < 0.1);
            }
            assert!(!svg.contains("NaN"));
        }
    }
}

#[test]
fn horizontal_scientific_endpoint_ticks_fit_the_canvas() {
    let svg = rendered("xychart horizontal\ny-axis -2e-308 --> -1e-308\nbar [-1e-308]");
    let view = elements(&svg, b"svg")[0]["viewBox"]
        .split_whitespace()
        .map(|value| value.parse::<f64>().unwrap())
        .collect::<Vec<_>>();
    let mut reader = Reader::from_str(&svg);
    let mut attributes = HashMap::<String, String>::new();
    let mut text = String::new();
    let mut count = 0;
    loop {
        match reader.read_event().unwrap() {
            Event::Start(tag) if tag.name().as_ref() == b"text" => {
                attributes = tag
                    .attributes()
                    .map(|a| {
                        let a = a.unwrap();
                        (
                            String::from_utf8(a.key.as_ref().to_vec()).unwrap(),
                            a.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                text.clear();
            }
            Event::Text(value) => text.push_str(&value.unescape().unwrap()),
            Event::End(tag) if tag.name().as_ref() == b"text" && text.contains("e-308") => {
                count += 1;
                let x = attributes["x"].parse::<f64>().unwrap();
                let y = attributes["y"].parse::<f64>().unwrap();
                let font = attributes["font-size"].parse::<f64>().unwrap();
                // Scientific tick glyphs are narrower than three quarters of
                // an em in the emitted sans-serif font. Reserve both halves.
                let half_width = text.chars().count() as f64 * font * 0.75 / 2.0;
                assert!(
                    x - half_width >= view[0] && x + half_width <= view[2],
                    "tick {text} clips horizontally in {view:?}"
                );
                assert!(y - font >= view[1] && y + font * 0.25 <= view[3]);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(count, 3, "check minimum, midpoint, and maximum tick labels");
}

#[test]
fn horizontal_categories_reserve_literal_break_spellings() {
    for spelling in ["<br>", "<br/>", "<br />"] {
        let label = format!("WWWW{spelling}WWWW");
        let svg = rendered(&format!(
            "xychart horizontal\nx-axis [\"{label}\"]\nbar [1]"
        ));
        assert!(svg.contains(&label.replace('<', "&lt;").replace('>', "&gt;")));
        let captions = elements(&svg, b"text");
        let category = captions
            .iter()
            .find(|caption| {
                caption
                    .get("text-anchor")
                    .is_some_and(|anchor| anchor == "end")
            })
            .unwrap();
        let x = category["x"].parse::<f64>().unwrap();
        // Eight W glyphs alone need almost 88px at the emitted 11px
        // font. The literal break spelling needs additional space.
        assert!(x >= 100.0, "literal category clips its left edge: {svg}");
    }
}
