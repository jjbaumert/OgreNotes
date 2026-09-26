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

/// Ids that are allowed to differ between kinds. Empty: the `mmd-arrow`
/// size collision (sequence 7 vs flowchart/state 12) was fixed by giving
/// the sequence marker its own id, and sankey gradients carry a
/// per-diagram tag. Anything that collides fails this test.
const KNOWN_COLLISIONS: &[&str] = &[];

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

/// Instance-specific definitions (sankey's per-link gradients) must not
/// share ids between two different diagrams either.
#[test]
fn instance_specific_ids_differ_between_two_sankeys() {
    let a = ogrenotes_mermaid::render("sankey-beta\n    a,b,5\n    b,c,2\n").svg.unwrap();
    let b = ogrenotes_mermaid::render("sankey-beta\n    x,y,7\n").svg.unwrap();
    let ids_a: BTreeSet<String> = defs_with_ids(&a).into_iter().map(|(id, _)| id).collect();
    let ids_b: BTreeSet<String> = defs_with_ids(&b).into_iter().map(|(id, _)| id).collect();
    assert!(!ids_a.is_empty(), "sankey defines gradients: {a}");
    let shared: Vec<_> = ids_a.intersection(&ids_b).collect();
    assert!(shared.is_empty(), "gradient ids shared between two sankeys: {shared:?}");
}

/// And the sequence arrow marker no longer shares an id (at a different
/// size) with flowchart's.
#[test]
fn sequence_arrow_marker_has_its_own_id() {
    let seq = ogrenotes_mermaid::render("sequenceDiagram\n    A->>B: hi\n").svg.unwrap();
    let flow = ogrenotes_mermaid::render("flowchart TD\n    A --> B\n").svg.unwrap();
    assert!(seq.contains(r#"<marker id="mmd-seq-arrow""#), "{seq}");
    assert!(seq.contains("url(#mmd-seq-arrow)"), "{seq}");
    assert!(!seq.contains(r#"<marker id="mmd-arrow""#), "{seq}");
    assert!(flow.contains(r#"<marker id="mmd-arrow""#), "{flow}");
}
