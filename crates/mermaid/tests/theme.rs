// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn contrast_edge_fixtures_match_the_renderer() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut names = vec![
        "contrast-c4",
        "contrast-c4-narrow",
        "contrast-cli-literal",
        "contrast-quadrant-edges",
        "contrast-quadrant-literal",
        "contrast-quadrant-wide",
        "contrast-quadrant-marker-overlap",
        "contrast-pie-empty",
        "contrast-treemap-short",
        "contrast-treemap-leaf-short",
        "contrast-treemap-wide-m",
        "contrast-c4-long",
        "contrast-c4-unicode",
        "contrast-treemap-numbers",
        "contrast-treemap-wide",
        "contrast-pie-palette",
        "contrast-pie-thin",
        "contrast-pie-adjacent",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    names.extend((0..8).map(|depth| format!("contrast-treemap-depth-{depth}")));
    for name in names {
        let source = std::fs::read_to_string(root.join(format!("{name}.mmd"))).unwrap();
        let svg = ogrenotes_mermaid::render(&source)
            .svg
            .expect("contrast fixture renders");
        let path = root.join(format!("{name}.svg"));
        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(path, svg).unwrap();
        } else {
            assert_eq!(svg, std::fs::read_to_string(path).unwrap());
        }
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
                .args(["--theme", theme, "--bg", "none"])
                .arg(&path)
                .output()
                .unwrap();
            assert!(cli.status.success());
            let resolved = String::from_utf8(cli.stdout).unwrap();
            assert!(
                !resolved.contains("var(--mermaid-"),
                "CLI must resolve every data foreground"
            );
            // Compare paint attributes at corresponding XML elements. A set of
            // colors cannot detect two CLI token mappings being swapped.
            let paints = |xml: &str| {
                let mut reader = quick_xml::Reader::from_str(xml);
                let mut output = Vec::new();
                loop {
                    match reader.read_event().unwrap() {
                        quick_xml::events::Event::Start(tag)
                        | quick_xml::events::Event::Empty(tag) => {
                            for attr in tag.attributes() {
                                let attr = attr.unwrap();
                                if tag.name().as_ref() != b"svg"
                                    && matches!(attr.key.as_ref(), b"fill" | b"stroke")
                                {
                                    output.push(String::from_utf8(attr.value.to_vec()).unwrap());
                                }
                            }
                        }
                        quick_xml::events::Event::Eof => break,
                        _ => {}
                    }
                }
                output
            };
            let raw_paints = paints(&svg);
            let cli_paints = paints(&resolved);
            assert_eq!(raw_paints.len(), cli_paints.len());
            for (raw, actual) in raw_paints.iter().zip(&cli_paints) {
                let Some(property) = raw.strip_prefix("var(--mermaid-") else {
                    continue;
                };
                let (name, fallback) = property.split_once(')').unwrap().0.split_once(',').unwrap();
                if !name.starts_with("data-") && !name.starts_with("quadrant-") {
                    continue;
                }
                let declaration = format!("--mermaid-{name}:");
                let expected = css
                    .lines()
                    .map(str::trim)
                    .find_map(|line| line.strip_prefix(&declaration))
                    .expect("renderer token must have a frontend definition")
                    .split(';')
                    .next()
                    .unwrap()
                    .trim();
                if theme == "light" {
                    assert_eq!(expected, fallback.trim(), "{theme} {name}");
                }
                assert_eq!(
                    actual.to_ascii_lowercase(),
                    expected.to_ascii_lowercase(),
                    "CLI paint must match the frontend token on the same element: {theme} {name}"
                );
            }
        }
    }
}

#[test]
fn cli_theme_preserves_literal_labels() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = root.join("tests/fixtures/contrast-cli-literal.mmd");
    let svg = ogrenotes_mermaid::render(&std::fs::read_to_string(&source).unwrap())
        .svg
        .unwrap();
    let labels = |svg: &str| {
        let mut reader = quick_xml::Reader::from_str(svg);
        let mut labels = Vec::new();
        loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Text(text) => {
                    labels.push(text.unescape().unwrap().into_owned())
                }
                quick_xml::events::Event::Eof => break,
                _ => {}
            }
        }
        labels
    };
    for theme in ["light", "dark"] {
        let output = Command::new(env!("CARGO_BIN_EXE_mermaid_cli"))
            .args(["--theme", theme])
            .arg(&source)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            labels(&svg),
            labels(&String::from_utf8(output.stdout).unwrap()),
            "CLI themes must preserve literal text: {theme}"
        );
    }
}
