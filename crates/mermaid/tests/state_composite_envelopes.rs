//! End-to-end geometry checks for reserved composite self-loop envelopes.
use ogrenotes_mermaid::render;
use std::collections::{HashMap, HashSet};

type Rect = [f64; 4];
fn number(tag: &str, name: &str) -> f64 {
    format!(" {tag}")
        .split(&format!(" {name}=\""))
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}
fn rectangle(tag: &str) -> Rect {
    [
        number(tag, "x"),
        number(tag, "y"),
        number(tag, "width"),
        number(tag, "height"),
    ]
}
fn ancestor(parent: &HashMap<&str, &str>, candidate: &str, owner: &str) -> bool {
    let mut current = owner;
    while let Some(next) = parent.get(current) {
        if *next == candidate {
            return true;
        }
        current = next;
    }
    false
}
fn overlap(a: Rect, b: Rect) -> bool {
    a[0] < b[0] + b[2] - 0.15
        && a[0] + a[2] > b[0] + 0.15
        && a[1] < b[1] + b[3] - 0.15
        && a[1] + a[3] > b[1] + 0.15
}
fn assert_segment_clear(a: [f64; 2], b: [f64; 2], r: Rect, source: &str) {
    let horizontal = (a[1] - b[1]).abs() < 0.01;
    let vertical = (a[0] - b[0]).abs() < 0.01;
    assert!(
        horizontal || vertical,
        "composite routes must remain orthogonal: {source}"
    );
    let hit = if horizontal {
        a[1] > r[1] + 0.15
            && a[1] < r[1] + r[3] - 0.15
            && a[0].max(b[0]) > r[0] + 0.15
            && a[0].min(b[0]) < r[0] + r[2] - 0.15
    } else {
        a[0] > r[0] + 0.15
            && a[0] < r[0] + r[2] - 0.15
            && a[1].max(b[1]) > r[1] + 0.15
            && a[1].min(b[1]) < r[1] + r[3] - 0.15
    };
    assert!(
        !hit,
        "loop enters composite/node {r:?}, segment {a:?}->{b:?}: {source}"
    );
}

#[test]
fn composite_loops_reserve_space_before_sibling_and_ancestor_layout() {
    let flat = "state X {\nA\n}\nstate Y {\nB\n}\nstate Z {\nC\n}";
    let nested = "state Outer {\nstate X {\nA\n}\nstate Y {\nB\n}\nD\n}\nstate Z {\nC\n}";
    let cases = vec![
        (flat, vec!["X"; 10], false, "", vec![]),
        (flat, vec!["Y"; 10], true, "", vec![]),
        (
            flat,
            [vec!["X"; 6], vec!["Y"; 6]].concat(),
            true,
            "",
            vec![],
        ),
        (flat, vec!["X"; 10], true, "\nX --> Y", vec![]),
        (flat, vec!["X"; 10], true, "\nX --> Below", vec![]),
        (flat, vec!["X"; 10], true, "\nX --> Y\nY --> X", vec![]),
        (flat, vec!["X"; 3], true, "\nX --> Y\nY --> X", vec![]),
        (
            nested,
            vec!["X"; 10],
            true,
            "",
            vec![("X", "Outer"), ("Y", "Outer")],
        ),
        (
            nested,
            [vec!["X"; 5], vec!["Outer"; 5]].concat(),
            true,
            "",
            vec![("X", "Outer"), ("Y", "Outer")],
        ),
        (flat, vec!["X"; 990], false, "", vec![]),
    ];
    for (declarations, owners, labeled, tail, parents) in cases {
        let parent: HashMap<_, _> = parents.into_iter().collect();
        let mut source = format!("stateDiagram-v2\n{declarations}\n");
        for (index, owner) in owners.iter().enumerate() {
            source.push_str(&format!("{owner} --> {owner}"));
            if labeled {
                let detail = if owners.len() == 3 {
                    "long label ".repeat(40)
                } else {
                    "with detail".to_string()
                };
                source.push_str(&format!(": loop {index}<br/>{detail}"));
            }
            source.push('\n');
        }
        source.push_str(tail);
        let result = render(&source);
        let svg = result.svg.unwrap_or_else(|| panic!("{:?}", result.error));
        let opening = svg.split('>').next().unwrap();
        let (width, height) = (number(opening, "width"), number(opening, "height"));
        let body = svg.split("</defs>").nth(1).unwrap();
        let mut clusters = HashMap::new();
        let mut nodes = Vec::new();
        let mut masks = Vec::new();
        for section in body.split("<rect ").skip(1) {
            let tag = section.split('>').next().unwrap();
            let r = rectangle(tag);
            assert!(
                r[0] >= -0.15
                    && r[1] >= -0.15
                    && r[0] + r[2] <= width + 0.15
                    && r[1] + r[3] <= height + 0.15
            );
            if tag.contains("stroke-dasharray=") {
                let text = section
                    .split("</text>")
                    .next()
                    .unwrap()
                    .rsplit('>')
                    .next()
                    .unwrap();
                clusters.insert(text, r);
            } else if tag.contains("rx=\"8\"") {
                nodes.push(r);
            } else {
                masks.push(r);
            }
        }
        let paths: Vec<_> = body
            .split("<path d=\"")
            .skip(1)
            .map(|s| s.split('"').next().unwrap())
            .collect();
        assert_eq!(paths.len(), owners.len() + tail.matches("-->").count());
        assert_eq!(
            paths[..owners.len()].iter().collect::<HashSet<_>>().len(),
            owners.len(),
            "loop routes collapsed"
        );
        // A cubic lies inside its control-point hull. These conservative
        // hull checks also cover normal/feedback connectors through the
        // reserved decoration region, rather than only the self-loops.
        for path in &paths {
            let mut tokens = path.split_whitespace();
            let point = |tokens: &mut std::str::SplitWhitespace<'_>| -> [f64; 2] {
                [
                    tokens.next().unwrap().parse().unwrap(),
                    tokens.next().unwrap().parse().unwrap(),
                ]
            };
            assert_eq!(tokens.next(), Some("M"));
            let mut from = point(&mut tokens);
            while let Some(command) = tokens.next() {
                assert_eq!(command, "C");
                let a = point(&mut tokens);
                let b = point(&mut tokens);
                let to = point(&mut tokens);
                let xs = [from[0], a[0], b[0], to[0]];
                let ys = [from[1], a[1], b[1], to[1]];
                let x = xs.into_iter().fold(f64::INFINITY, f64::min);
                let y = ys.into_iter().fold(f64::INFINITY, f64::min);
                let hull = [
                    x,
                    y,
                    xs.into_iter().fold(f64::NEG_INFINITY, f64::max) - x,
                    ys.into_iter().fold(f64::NEG_INFINITY, f64::max) - y,
                ];
                for label in &masks {
                    assert!(
                        !overlap(hull, *label),
                        "connector control hull crosses loop label: {path}"
                    );
                }
                from = to;
            }
        }
        for (index, owner) in owners.iter().enumerate() {
            let r = clusters[owner];
            let path = paths[index];
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
                (from[0] - r[0] - r[2]).abs() < 0.15 && from[1] > r[1] && from[1] < r[1] + r[3]
            );
            while let Some(command) = tokens.next() {
                assert_eq!(command, "C");
                let a = point(&mut tokens);
                let b = point(&mut tokens);
                let to = point(&mut tokens);
                // These orthogonal cubics are exact line segments. Checking
                // their entire intervals cannot jump over a narrow sibling.
                assert!(
                    [a, b, to].iter().all(|p| (p[0] - from[0]).abs() < 0.01)
                        || [a, b, to].iter().all(|p| (p[1] - from[1]).abs() < 0.01)
                );
                for p in [a, b] {
                    for axis in 0..2 {
                        assert!(
                            p[axis] >= from[axis].min(to[axis]) - 0.01
                                && p[axis] <= from[axis].max(to[axis]) + 0.01
                        );
                    }
                }
                for label in &masks {
                    assert_segment_clear(from, to, *label, &source);
                }
                for p in [from, a, b, to] {
                    assert!(
                        p[0] >= -0.15
                            && p[0] <= width + 0.15
                            && p[1] >= -0.15
                            && p[1] <= height + 0.15
                    );
                }
                for node in &nodes {
                    assert_segment_clear(from, to, *node, &source);
                }
                for (name, cluster) in &clusters {
                    if !ancestor(&parent, name, owner) {
                        assert_segment_clear(from, to, *cluster, &source);
                    }
                }
                from = to;
            }
            assert!(
                (from[0] - r[0] - r[2]).abs() < 0.15 && from[1] > r[1] && from[1] < r[1] + r[3]
            );
            if labeled {
                let label = masks[index];
                assert!(label[0] > r[0] + r[2], "label must stay outside its owner");
                for node in &nodes {
                    assert!(!overlap(label, *node), "label overlaps node");
                }
                for (name, cluster) in &clusters {
                    if !ancestor(&parent, name, owner) {
                        assert!(!overlap(label, *cluster), "label overlaps {name}");
                    }
                }
                for other in &masks[index + 1..] {
                    assert!(!overlap(label, *other), "loop labels overlap");
                }
            }
        }
    }
}

#[test]
fn nested_decoration_joins_respect_the_aggregate_waypoint_budget() {
    let mut source = String::from("stateDiagram-v2\n");
    for index in 0..60 {
        source.push_str(&format!("state C{index} {{\n"));
    }
    source.push_str("A\n");
    for _ in 0..60 {
        source.push_str("}\n");
    }
    source.push_str("B\nC0 --> C0\n");
    source.push_str(&"A --> B\n".repeat(500));
    let error = render(&source)
        .error
        .expect("aggregate nested join count must be bounded");
    assert!(error.message.contains("waypoints"), "{error:?}");
}
