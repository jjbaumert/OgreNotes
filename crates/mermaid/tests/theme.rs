// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn c4_all_tiers_have_a_stable_contrast_fixture() {
    let output = ogrenotes_mermaid::render(include_str!("fixtures/contrast-c4.mmd"));
    let svg = output.svg.expect("C4 contrast fixture renders");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/contrast-c4.svg");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(path, svg).unwrap();
    } else {
        assert_eq!(svg, std::fs::read_to_string(path).unwrap());
    }
}

#[test]
fn fixed_palette_foregrounds_agree_between_renderer_frontend_and_cli() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for input in [
        "golden/treemap.mmd",
        "golden/pie.mmd",
        "golden/quadrant-chart.mmd",
        "fixtures/contrast-c4.mmd",
    ] {
        let path = root.join("tests").join(input);
        let rendered = ogrenotes_mermaid::render(&std::fs::read_to_string(&path).unwrap());
        let svg = rendered.svg.unwrap();
        for theme in ["light", "dark"] {
            let css = std::fs::read_to_string(
                root.join(format!("../../frontend/style/tokens-{theme}.css")),
            )
            .unwrap();
            let cli = Command::new(env!("CARGO_BIN_EXE_mermaid_cli"))
                .args(["--theme", theme])
                .arg(&path)
                .output()
                .unwrap();
            assert!(cli.status.success());
            let resolved = String::from_utf8(cli.stdout).unwrap();
            assert!(
                !resolved.contains("var(--mermaid-data-"),
                "CLI must resolve every data foreground"
            );
            for property in svg.split("var(--mermaid-data-").skip(1) {
                let (name, fallback) = property.split_once(')').unwrap().0.split_once(',').unwrap();
                let declaration = format!("--mermaid-data-{name}:");
                let actual = css
                    .lines()
                    .map(str::trim)
                    .find_map(|line| line.strip_prefix(&declaration))
                    .expect("renderer token must have a frontend definition")
                    .split(';')
                    .next()
                    .unwrap()
                    .trim();
                assert_eq!(actual, fallback.trim(), "{theme} {name}");
                assert!(
                    resolved.contains(&format!("\"{}\"", fallback.trim())),
                    "CLI uses the same foreground"
                );
            }
        }
    }
}
