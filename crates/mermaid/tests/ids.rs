// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Two diagrams on one page share a DOM, and `url(#id)` resolves to the
//! *first* definition with that id. So an id that two diagram kinds
//! define differently — or that two instances of one kind define with
//! instance-specific content — silently corrupts whichever renders
//! second. Every kind's golden source is rendered and every `<defs>`
//! child id is checked for a single definition.

use std::collections::{BTreeMap, BTreeSet};

fn golden_sources() -> Vec<(String, String)> {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let mut out: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "mmd"))
        .map(|p| {
            let kind = p.file_stem().unwrap().to_string_lossy().into_owned();
            (kind, std::fs::read_to_string(&p).unwrap())
        })
        .collect();
    out.sort();
    out
}

/// `(id, full element text)` for every element carrying an `id=` inside
/// `<defs>…</defs>`.
fn defs_with_ids(svg: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find("<defs>") {
        let after = &rest[start + 6..];
        let end = after.find("</defs>").unwrap_or(after.len());
        let defs = &after[..end];
        // Split on top-level element starts; each definition is one
        // `<tag ... id="..." ...>…</tag>` run.
        let mut i = 0;
        while let Some(lt) = defs[i..].find('<') {
            let abs = i + lt;
            let tag_end = defs[abs + 1..]
                .find(|c: char| c == ' ' || c == '>' || c == '/')
                .map(|n| abs + 1 + n)
                .unwrap_or(defs.len());
            let tag = &defs[abs + 1..tag_end];
            if tag.is_empty() || tag.starts_with('/') {
                i = tag_end.max(abs + 1);
                continue;
            }
            let close = format!("</{tag}>");
            let elem_end = defs[abs..]
                .find(&close)
                .map(|n| abs + n + close.len())
                .or_else(|| defs[abs..].find("/>").map(|n| abs + n + 2))
                .unwrap_or(defs.len());
            let elem = &defs[abs..elem_end];
            if let Some(idpos) = elem.find(" id=\"") {
                let id_start = idpos + 5;
                let id_end = elem[id_start..].find('"').map(|n| id_start + n).unwrap_or(elem.len());
                out.push((elem[id_start..id_end].to_string(), elem.to_string()));
            }
            i = elem_end.max(abs + 1);
        }
        rest = &after[end..];
    }
    out
}

/// Known collisions, each a real bug whose fix would change an existing
/// test's pinned string (immutable per project policy — surfaced as
/// findings in the 2026-09-26 mermaid output-pinning PR):
///
/// - `mmd-arrow`: sequence defines it with markerWidth/Height 7,
///   flowchart and state with 12. Two of those on one page render the
///   second's arrowheads at the wrong size. Fix = rename the sequence
///   marker (e.g. `mmd-seq-arrow`), which changes the literal
///   `url(#mmd-arrow)` asserted by sequence/svg.rs tests.
///
/// Anything NOT listed here that collides fails this test.
const KNOWN_COLLISIONS: &[&str] = &["mmd-arrow"];

#[test]
fn definition_ids_are_unique_across_diagram_kinds() {
    let mut by_id: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (kind, src) in golden_sources() {
        let svg = ogrenotes_mermaid::render(&src).svg.unwrap_or_else(|| panic!("{kind} renders"));
        for (id, def) in defs_with_ids(&svg) {
            by_id.entry(id.clone()).or_default().insert(def);
            owners.entry(id).or_default().insert(kind.clone());
        }
    }
    assert!(!by_id.is_empty(), "at least the arrow markers must be found");
    let conflicts: Vec<String> = by_id
        .iter()
        .filter(|(id, defs)| defs.len() > 1 && !KNOWN_COLLISIONS.contains(&id.as_str()))
        .map(|(id, defs)| format!("id {id:?} has {} different definitions across {:?}", defs.len(), owners[id]))
        .collect();
    assert!(conflicts.is_empty(), "{}", conflicts.join("\n"));
    for known in KNOWN_COLLISIONS {
        assert!(
            by_id.get(*known).is_some_and(|d| d.len() > 1),
            "{known} no longer collides — remove it from KNOWN_COLLISIONS"
        );
    }
}

// Not asserted: sankey's per-link gradients use `id="sk{n}"`, so two
// sankeys on one page share `sk0`, `sk1`, … and the second takes the
// first's colours. The fix (a per-render prefix) changes the literal
// `<linearGradient id="sk0"` pinned by sankey.rs's own test — surfaced
// as a finding alongside `mmd-arrow` above.
