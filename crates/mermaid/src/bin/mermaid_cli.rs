//! Command-line front end for the OgreNotes Mermaid renderer.
//!
//! Renders a Mermaid diagram through the exact same `ogrenotes_mermaid::render`
//! path the app uses, so its output can be compared side-by-side against the
//! upstream mermaid.ai rendering. The `--mermaid-*` theme custom-properties and
//! `currentColor` are resolved to concrete colors for the chosen theme, so the
//! result is faithful in a browser *and* rasterizes correctly to PNG (via
//! ImageMagick) without a CSS-aware SVG renderer.
//!
//!   cargo run -p ogrenotes-mermaid --bin mermaid_cli -- [OPTIONS] [INPUT]
//!
//! INPUT   Path to a .mmd/.mermaid file, or `-` / omitted to read stdin.
//!
//! OPTIONS
//!   -o, --out <PATH>   Write here. `.png` rasterizes via ImageMagick;
//!                      any other extension (or none) writes SVG. Omit for
//!                      SVG on stdout.
//!   -t, --theme <T>    `light` (default) or `dark`.
//!   --bg <COLOR>       Background override (`none` for transparent). Default
//!                      `#ffffff` (light) / `#1e1e1e` (dark).
//!   -h, --help         This help.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

/// (emitted `var(...)` string, light value, dark value). Light values are the
/// fallbacks the renderer already bakes in; dark values mirror
/// `frontend/style/tokens-dark.css`. Keep in sync with both.
const VARS: &[(&str, &str, &str)] = &[
    ("var(--mermaid-node-fill, #ececff)", "#ececff", "#2F2F45"),
    ("var(--mermaid-cluster-fill, #7773)", "#7773", "#ffffff14"),
    ("var(--mermaid-note-fill, #fff5ad)", "#fff5ad", "#4A4636"),
    ("var(--mermaid-note-text, #333)", "#333", "#E8E8E8"),
    ("var(--mermaid-gantt-task, #8a90dd)", "#8a90dd", "#6B74C9"),
    ("var(--mermaid-gantt-active, #bfc7ff)", "#bfc7ff", "#8F99E6"),
    ("var(--mermaid-gantt-done, #b8b8b8)", "#b8b8b8", "#5A5A5A"),
    ("var(--mermaid-gantt-crit, #ff6b6b)", "#ff6b6b", "#C95A5A"),
    ("var(--mermaid-gantt-band, #00000010)", "#00000010", "#ffffff10"),
    // Edge/relationship-label mask (ER, flowchart). Dark mirrors --surface
    // in tokens-dark.css so labels stay legible on the dark canvas.
    ("var(--surface, #fff)", "#ffffff", "#2A2A2A"),
];

const TEXT_LIGHT: &str = "#1A1A1A";
const TEXT_DARK: &str = "#E8E8E8";
const BG_LIGHT: &str = "#ffffff";
const BG_DARK: &str = "#1e1e1e";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut input: Option<String> = None;
    let mut out: Option<String> = None;
    let mut dark = false;
    let mut bg: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-h" | "--help" => {
                print!("{}", HELP);
                return;
            }
            "-o" | "--out" => {
                i += 1;
                out = Some(args.get(i).cloned().unwrap_or_else(|| die("--out needs a path")));
            }
            "-t" | "--theme" => {
                i += 1;
                match args.get(i).map(String::as_str) {
                    Some("dark") => dark = true,
                    Some("light") => dark = false,
                    _ => die("--theme must be light or dark"),
                }
            }
            "--bg" => {
                i += 1;
                bg = Some(args.get(i).cloned().unwrap_or_else(|| die("--bg needs a color")));
            }
            other if other.starts_with('-') && other != "-" => {
                die(&format!("unknown option: {other}"));
            }
            _ => {
                if input.is_some() {
                    die("more than one input given");
                }
                input = Some(a.clone());
            }
        }
        i += 1;
    }

    let source = match input.as_deref() {
        None | Some("-") => {
            let mut s = String::new();
            if std::io::stdin().read_to_string(&mut s).is_err() {
                die("failed to read stdin");
            }
            s
        }
        Some(path) => std::fs::read_to_string(path)
            .unwrap_or_else(|e| die(&format!("failed to read {path}: {e}"))),
    };

    let rendered = ogrenotes_mermaid::render(&source);
    let svg = match rendered.svg {
        Some(svg) => svg,
        None => {
            let msg = rendered
                .error
                .map(|e| format!("{e:?}"))
                .unwrap_or_else(|| "render produced no SVG".into());
            die(&format!("{} diagram did not render: {msg}", rendered.kind.label()));
        }
    };

    let bg = bg.unwrap_or_else(|| if dark { BG_DARK.into() } else { BG_LIGHT.into() });
    let themed = theme(&svg, dark, &bg);

    match out.as_deref() {
        None => {
            print!("{themed}");
        }
        Some(path) if path.to_ascii_lowercase().ends_with(".png") => {
            rasterize(&themed, path);
        }
        Some(path) => {
            std::fs::write(path, &themed)
                .unwrap_or_else(|e| die(&format!("failed to write {path}: {e}")));
        }
    }
}

/// Resolve theme custom-properties and `currentColor` to concrete colors, and
/// stamp a background rect (unless transparent) so any SVG viewer or
/// rasterizer reproduces the app's appearance.
fn theme(svg: &str, dark: bool, bg: &str) -> String {
    let mut out = svg.to_string();
    for (needle, light, darkv) in VARS {
        out = out.replace(needle, if dark { darkv } else { light });
    }
    out = out.replace("currentColor", if dark { TEXT_DARK } else { TEXT_LIGHT });

    if bg != "none" {
        if let Some(pos) = out.find('>') {
            let rect = format!(r#"<rect x="0" y="0" width="100%" height="100%" fill="{bg}"/>"#);
            out.insert_str(pos + 1, &rect);
        }
    }
    out
}

/// Pipe the SVG to ImageMagick (`magick`, falling back to `convert`) to write a
/// PNG. The SVG already carries a real background rect and concrete colors, so
/// no CSS-aware delegate is required.
fn rasterize(svg: &str, out_path: &str) {
    for tool in ["magick", "convert"] {
        let mut child = match Command::new(tool)
            .args(["-density", "192", "svg:-", out_path])
            .stdin(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => continue, // tool not installed — try the next
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(svg.as_bytes());
        }
        match child.wait() {
            Ok(status) if status.success() => return,
            Ok(status) => die(&format!("{tool} exited with {status}")),
            Err(e) => die(&format!("{tool} failed: {e}")),
        }
    }
    die("no PNG rasterizer found (install ImageMagick, or output .svg instead)");
}

fn die(msg: &str) -> ! {
    eprintln!("mermaid_cli: {msg}");
    std::process::exit(1);
}

const HELP: &str = "\
mermaid_cli — render a Mermaid diagram through the OgreNotes renderer

USAGE:
    mermaid_cli [OPTIONS] [INPUT]

    INPUT   .mmd/.mermaid file, or `-`/omitted to read stdin.

OPTIONS:
    -o, --out <PATH>   Output file. `.png` rasterizes via ImageMagick; any
                       other extension writes SVG. Omit for SVG on stdout.
    -t, --theme <T>    light (default) | dark.
        --bg <COLOR>   Background (`none` = transparent). Default #ffffff
                       (light) / #1e1e1e (dark).
    -h, --help         Show this help.
";
