// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Size and time bounds. The per-kind caps are tested to *reject*
//! over-limit input; nothing bounded how long an at-cap diagram takes,
//! and the layout's recursive cluster expansion had no depth check
//! reachable from user input.

use std::time::{Duration, Instant};

/// 400 nodes / ~600 edges: the layout caps (`layout/mod.rs`). A
/// superlinear regression inside the caps shows up here long before it
/// shows up as a hung request.
#[test]
fn at_cap_flowchart_renders_under_budget() {
    let mut src = String::from("flowchart TD\n");
    for i in 0..400 {
        src.push_str(&format!("    n{i}[Node {i}]\n"));
    }
    for i in 0..399 {
        src.push_str(&format!("    n{i} --> n{}\n", i + 1));
    }
    // Short-range cross edges: long spans cost one dummy waypoint per
    // rank crossed and would trip the 20 000-waypoint cap before the
    // node cap; the point here is the node cap.
    for i in (0..397).step_by(2) {
        src.push_str(&format!("    n{i} --> n{}\n", i + 3));
    }
    let started = Instant::now();
    let out = ogrenotes_mermaid::render(&src);
    let elapsed = started.elapsed();
    assert!(out.svg.is_some(), "at-cap flowchart must render: {:?}", out.error);
    assert!(
        elapsed < Duration::from_secs(5),
        "at-cap flowchart took {elapsed:?} (budget 5s, loose for CI)"
    );
}

/// C4 boundaries nest via `{`; the source-length cap allows ~600
/// levels. The layout expands clusters recursively, so this must come
/// back as an error or an SVG — never a stack overflow, which aborts
/// the process instead of unwinding.
#[test]
fn deeply_nested_c4_boundaries_return_instead_of_overflowing() {
    let depth = 600;
    let mut src = String::from("C4Context\n");
    for i in 0..depth {
        src.push_str(&format!("System_Boundary(b{i}, \"B\") {{\n"));
    }
    src.push_str("System(s, \"S\")\n");
    for _ in 0..depth {
        src.push_str("}\n");
    }
    assert!(src.len() < ogrenotes_mermaid::MAX_SOURCE_LEN, "fixture must be under the source cap");
    let out = ogrenotes_mermaid::render(&src);
    assert!(out.svg.is_some() || out.error.is_some(), "render must return");
}

/// The same for state composites.
#[test]
fn deeply_nested_state_composites_return_instead_of_overflowing() {
    let depth = 600;
    let mut src = String::from("stateDiagram-v2\n");
    for i in 0..depth {
        src.push_str(&format!("state S{i} {{\n"));
    }
    src.push_str("a --> b\n");
    for _ in 0..depth {
        src.push_str("}\n");
    }
    assert!(src.len() < ogrenotes_mermaid::MAX_SOURCE_LEN);
    let out = ogrenotes_mermaid::render(&src);
    assert!(out.svg.is_some() || out.error.is_some(), "render must return");
}
