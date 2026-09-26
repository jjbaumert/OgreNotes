// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! One pinned SVG per diagram type. Parity with upstream mermaid is
//! checked by the out-of-repo comparison harness; this pins *stability*
//! so a rendering change on main is visible in CI. Re-bless deliberately,
//! after looking at the new output:
//! `UPDATE_GOLDEN=1 cargo test -p ogrenotes-mermaid --test golden`.

use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn every_kind_has_a_golden_and_it_matches() {
    let mut kinds: Vec<String> = std::fs::read_dir(dir())
        .expect("tests/golden")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "mmd"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    kinds.sort();
    assert_eq!(kinds.len(), 22, "one .mmd per diagram kind: {kinds:?}");

    let update = std::env::var("UPDATE_GOLDEN").is_ok();
    let mut failures = Vec::new();
    for kind in &kinds {
        let src = std::fs::read_to_string(dir().join(format!("{kind}.mmd"))).unwrap();
        let out = ogrenotes_mermaid::render(&src);
        if out.kind.label() != kind {
            failures.push(format!("{kind}.mmd detected as {}", out.kind.label()));
            continue;
        }
        let Some(svg) = out.svg else {
            failures.push(format!("{kind}.mmd failed to render: {:?}", out.error));
            continue;
        };
        let golden_path = dir().join(format!("{kind}.svg"));
        if update {
            std::fs::write(&golden_path, &svg).unwrap();
            continue;
        }
        let golden = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|_| panic!("missing {kind}.svg — run with UPDATE_GOLDEN=1 to bless"));
        if golden != svg {
            failures.push(format!(
                "{kind}: output differs from tests/golden/{kind}.svg (review, then UPDATE_GOLDEN=1 to re-bless)"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
