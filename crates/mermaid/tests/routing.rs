// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Check the SVG the editor/export actually displays, not only layout points.
use quick_xml::{events::Event, Reader};

#[test]
fn cross_cluster_edge_avoids_nodes_inside_a_cycle() {
    let source = include_str!("fixtures/cluster-port-cycle.mmd");
    let rendered = ogrenotes_mermaid::render(source);
    let svg = rendered.svg.expect("cyclic flowchart renders");
    let mut reader = Reader::from_str(&svg);
    let mut depth = 0;
    let mut paths = Vec::new();
    let mut boxes = Vec::new();
    loop {
        match reader.read_event().unwrap() {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            Event::Empty(e) => {
                let attrs: std::collections::HashMap<_, _> = e
                    .attributes()
                    .map(|a| {
                        let a = a.unwrap();
                        (
                            String::from_utf8(a.key.as_ref().to_vec()).unwrap(),
                            a.unescape_value().unwrap().into_owned(),
                        )
                    })
                    .collect();
                if depth == 1 && e.name().as_ref() == b"path" {
                    paths.push(attrs["d"].clone());
                } else if depth == 2 && e.name().as_ref() == b"rect" {
                    boxes.push(
                        ["x", "y", "width", "height"].map(|k| attrs[k].parse::<f64>().unwrap()),
                    );
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(boxes.len(), 5, "fixture's node rectangles");
    assert_eq!(paths.len(), 6, "fixture's rendered edges");
    // Fifth edge: N4 -> N3. Before the fix its cluster-entry join crosses N2.
    // Sample emitted cubic curves too: SVG rendering can deviate from the
    // layout polyline, and the user sees the curve rather than that polyline.
    let mut tokens = paths[4].split_whitespace();
    assert_eq!(tokens.next(), Some("M"));
    let point = |tokens: &mut std::str::SplitWhitespace<'_>| -> [f64; 2] {
        [
            tokens.next().unwrap().parse().unwrap(),
            tokens.next().unwrap().parse().unwrap(),
        ]
    };
    let mut from = point(&mut tokens);
    while let Some(command) = tokens.next() {
        assert_eq!(command, "C");
        let a = point(&mut tokens);
        let b = point(&mut tokens);
        let to = point(&mut tokens);
        for step in 0..=100 {
            let t = step as f64 / 100.0;
            let u = 1.0 - t;
            let p: [f64; 2] = std::array::from_fn(|k| {
                u * u * u * from[k]
                    + 3.0 * u * u * t * a[k]
                    + 3.0 * u * t * t * b[k]
                    + t * t * t * to[k]
            });
            for (node, &[x, y, w, h]) in boxes.iter().enumerate() {
                if node == 4 || node == 3 {
                    continue;
                }
                assert!(
                    !(p[0] > x + 2.0 && p[0] < x + w - 2.0 && p[1] > y + 2.0 && p[1] < y + h - 2.0),
                    "N4 -> N3 crosses node N{node} at {p:?}: {}",
                    paths[4]
                );
            }
        }
        from = to;
    }
}
