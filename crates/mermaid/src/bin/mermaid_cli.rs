//! Command-line front end for the OgreNotes Mermaid renderer.
//!
//! Renders a Mermaid diagram through the exact same `ogrenotes_mermaid::render`
//! path the app uses, so its output can be compared side-by-side against the
//! upstream mermaid.ai rendering. The `--mermaid-*` theme custom-properties and
//! `currentColor` are resolved to concrete colors for the chosen theme, so the
//! result is faithful in a browser *and* rasterizes correctly to PNG (via
//! librsvg), including clipped labels, masks and glyph halos.
//!
//!   cargo run -p ogrenotes-mermaid --bin mermaid_cli -- [OPTIONS] [INPUT]
//!
//! INPUT   Path to a .mmd/.mermaid file, or `-` / omitted to read stdin.
//!
//! OPTIONS
//!   -o, --out <PATH>   Write here. `.png` uses librsvg or a compatible ImageMagick SVG delegate;
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
    ("var(--mermaid-quadrant-1, #eef4ff)", "#eef4ff", "#253348"),
    ("var(--mermaid-quadrant-2, #fff7e8)", "#fff7e8", "#3d3325"),
    ("var(--mermaid-quadrant-3, #fdeef6)", "#fdeef6", "#402838"),
    ("var(--mermaid-quadrant-4, #eefaf1)", "#eefaf1", "#25382d"),
    ("var(--mermaid-data-text, #1a1a1a)", "#1a1a1a", "#1a1a1a"),
    ("var(--mermaid-data-text-muted, #444)", "#444", "#444"),
    ("var(--mermaid-data-stroke, #444)", "#444", "#444"),
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

    let bg = bg.unwrap_or_else(|| if dark { BG_DARK.into() } else { BG_LIGHT.into() });

    let rendered = ogrenotes_mermaid::render(&source);
    // Faithful to the app: a successful render themes the SVG; a parse error
    // renders the app's error state (banner + source) as an image so a failed
    // diagram is *visible* in a batch instead of silently yielding nothing —
    // and it's also flagged on stderr with a non-zero exit.
    let (image, parse_error) = match rendered.svg {
        Some(svg) => (theme(&svg, dark, &bg), None),
        None => {
            let msg = match rendered.error {
                Some(e) => match e.line {
                    Some(n) => format!("{} (line {n})", e.message),
                    None => e.message,
                },
                None => "renderer produced no SVG".to_string(),
            };
            (error_card(rendered.kind.label(), &msg, &source, dark, &bg), Some(msg))
        }
    };

    match out.as_deref() {
        None => print!("{image}"),
        Some(path) if path.to_ascii_lowercase().ends_with(".png") => rasterize(&image, path),
        Some(path) => std::fs::write(path, &image)
            .unwrap_or_else(|e| die(&format!("failed to write {path}: {e}"))),
    }

    if let Some(msg) = parse_error {
        let where_ = input.as_deref().filter(|s| *s != "-").unwrap_or("stdin");
        eprintln!("mermaid_cli: PARSE ERROR [{where_}] — {msg}");
        std::process::exit(1);
    }
}

/// Resolve theme custom-properties and `currentColor` to concrete colors, and
/// stamp a background rect (unless transparent) so any SVG viewer or
/// rasterizer reproduces the app's appearance.
fn theme(svg: &str, dark: bool, bg: &str) -> String {
    use quick_xml::events::{BytesStart, Event};
    let mut reader = quick_xml::Reader::from_str(svg);
    let mut writer = quick_xml::Writer::new(Vec::new());
    let resolve = |value: &str| {
        let mut out = value.to_string();
        for (needle, light, darkv) in VARS {
            out = out.replace(needle, if dark { darkv } else { light });
        }
        out.replace("currentColor", if dark { TEXT_DARK } else { TEXT_LIGHT })
    };
    let mut root = true;
    loop {
        let event = reader.read_event().expect("renderer emits well-formed SVG");
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let name = std::str::from_utf8(tag.name().as_ref())
                    .unwrap()
                    .to_string();
                let mut replacement = BytesStart::new(name);
                for attr in tag.attributes() {
                    let attr = attr.unwrap();
                    let key = std::str::from_utf8(attr.key.as_ref()).unwrap();
                    let value = attr.unescape_value().unwrap();
                    let paint = matches!(key, "fill" | "stroke" | "color" | "stop-color");
                    let value = if paint {
                        resolve(&value)
                    } else if key == "style" {
                        value
                            .split(';')
                            .map(|declaration| {
                                let Some((property, value)) = declaration.split_once(':') else {
                                    return declaration.to_string();
                                };
                                if matches!(
                                    property.trim(),
                                    "fill" | "stroke" | "color" | "stop-color"
                                ) {
                                    format!("{property}:{}", resolve(value))
                                } else {
                                    declaration.to_string()
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(";")
                    } else {
                        value.into_owned()
                    };
                    replacement.push_attribute((key, value.as_str()));
                }
                if root {
                    replacement.push_attribute(("fill", if dark { TEXT_DARK } else { TEXT_LIGHT }));
                }
                writer
                    .write_event(if empty {
                        Event::Empty(replacement)
                    } else {
                        Event::Start(replacement)
                    })
                    .unwrap();
                if root && bg != "none" {
                    let mut rect = BytesStart::new("rect");
                    rect.extend_attributes([
                        ("x", "0"),
                        ("y", "0"),
                        ("width", "100%"),
                        ("height", "100%"),
                        ("fill", bg),
                    ]);
                    writer.write_event(Event::Empty(rect)).unwrap();
                }
                root = false;
            }
            Event::Eof => break,
            other => writer.write_event(other).unwrap(),
        }
    }
    String::from_utf8(writer.into_inner()).unwrap()
}

/// Render the app's parse-error state — a red banner plus the raw source —
/// as an SVG card. Mirrors `frontend/src/editor/blocks/mermaid.rs`, which
/// shows a `.mermaid-error` message and a `<pre>` of the source on failure.
/// This keeps a failed diagram visible in a batch comparison instead of
/// producing no artifact at all.
fn error_card(kind: &str, message: &str, source: &str, dark: bool, bg: &str) -> String {
    let text = if dark { TEXT_DARK } else { TEXT_LIGHT };
    let danger = if dark { "#E87060" } else { "#CC3333" }; // --color-danger
    let muted = if dark { "#B0B0B0" } else { "#6B6B6B" }; // --color-text-secondary
    let title = format!("Parse error — {kind} diagram");
    let src_lines: Vec<&str> = source.lines().collect();
    let cols = src_lines
        .iter()
        .map(|l| l.chars().count())
        .chain([title.chars().count(), message.chars().count()])
        .max()
        .unwrap_or(0);
    let pad = 20.0_f64;
    let line_h = 18.0_f64;
    let w = (cols as f64 * 7.9 + pad * 2.0).max(420.0);
    let h = pad * 2.0 + 24.0 + 26.0 + src_lines.len().max(1) as f64 * line_h;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );
    if bg != "none" {
        out.push_str(&format!(r#"<rect x="0" y="0" width="100%" height="100%" fill="{bg}"/>"#));
    }
    let mut y = pad + 14.0;
    out.push_str(&format!(
        r#"<text x="{pad:.0}" y="{y:.0}" font-weight="bold" fill="{danger}">{}</text>"#,
        escape_xml(&title)
    ));
    y += 24.0;
    out.push_str(&format!(
        r#"<text x="{pad:.0}" y="{y:.0}" fill="{text}">{}</text>"#,
        escape_xml(message)
    ));
    y += 28.0;
    for line in &src_lines {
        out.push_str(&format!(
            r#"<text x="{pad:.0}" y="{y:.0}" font-family="monospace" font-size="13" xml:space="preserve" fill="{muted}">{}</text>"#,
            escape_xml(line)
        ));
        y += line_h;
    }
    out.push_str("</svg>");
    out
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Prefer librsvg. Preserve ImageMagick installations whose SVG delegate
/// supports the renderer's clipping, masks and text paint order.
fn rasterize(svg: &str, out_path: &str) {
    match Command::new("rsvg-convert")
        .args([
            "--dpi-x", "192", "--dpi-y", "192", "--zoom", "2", "--output", out_path,
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => {
            write_image(child, svg, "rsvg-convert");
            return;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => die(&format!("rsvg-convert failed: {e}")),
    }
    let advanced = needs_advanced_svg(svg);
    for tool in ["magick", "convert"] {
        if advanced && !compatible_svg_delegate(tool) {
            continue;
        }
        let Ok(mut input) = tempfile::Builder::new().suffix(".svg").tempfile() else {
            die("failed to create temporary SVG")
        };
        input
            .write_all(svg.as_bytes())
            .unwrap_or_else(|e| die(&format!("failed to write temporary SVG: {e}")));
        match Command::new(tool)
            .args(["-density", "192"])
            .arg(input.path())
            .arg(out_path)
            .stderr(Stdio::inherit())
            .status()
        {
            Ok(status) if status.success() => return,
            Ok(status) => die(&format!("{tool} exited with {status}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => die(&format!("{tool} failed: {e}")),
        }
    }
    die(
        "PNG export needs rsvg-convert (install librsvg2-bin) or ImageMagick with a compatible SVG delegate (librsvg/Inkscape)",
    );
}

fn needs_advanced_svg(svg: &str) -> bool {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(svg);
    let mut roots = 0;
    loop {
        match reader.read_event().expect("well-formed renderer SVG") {
            Event::Start(tag) | Event::Empty(tag) => {
                if tag.name().as_ref() == b"svg" {
                    roots += 1;
                }
                if roots > 1 || matches!(tag.name().as_ref(), b"mask" | b"clipPath") {
                    return true;
                }
                for attr in tag.attributes() {
                    let attr = attr.unwrap();
                    if matches!(attr.key.as_ref(), b"mask" | b"clip-path" | b"paint-order") {
                        return true;
                    }
                    if attr.key.as_ref() == b"style"
                        && attr.unescape_value().unwrap().split(';').any(|d| {
                            d.split_once(':').is_some_and(|(key, _)| {
                                matches!(key.trim(), "mask" | "clip-path" | "paint-order")
                            })
                        })
                    {
                        return true;
                    }
                }
            }
            Event::Eof => return false,
            _ => {}
        }
    }
}

fn write_image(mut child: std::process::Child, svg: &str, tool: &str) {
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(svg.as_bytes()) {
            let _ = child.wait();
            die(&format!("failed to send SVG to {tool}: {e}"));
        }
    }
    match child.wait() {
        Ok(status) if status.success() => {}
        Ok(status) => die(&format!("{tool} exited with {status}")),
        Err(e) => die(&format!("{tool} failed: {e}")),
    }
}

fn compatible_svg_delegate(tool: &str) -> bool {
    // A real pixel probe avoids relying on executable/version names. The
    // four samples check nested viewport clipping, masking and paint order.
    let probe = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="8"><rect width="24" height="8" fill="#fff"/><svg width="8" height="8" overflow="hidden"><rect width="16" height="8" fill="#0f0"/></svg><defs><mask id="probe" maskUnits="userSpaceOnUse" x="8" y="0" width="8" height="8"><rect x="8" width="8" height="8" fill="#fff"/><rect x="8" width="4" height="8" fill="#000"/></mask></defs><rect x="8" width="8" height="8" fill="#f00" mask="url(#probe)"/><svg x="16" width="8" height="8" overflow="hidden"><rect width="8" height="8" fill="#00f" stroke="#f00" stroke-width="10" paint-order="stroke"/></svg></svg>"##;
    let Ok(mut input) = tempfile::Builder::new().suffix(".svg").tempfile() else {
        return false;
    };
    if input.write_all(probe.as_bytes()).is_err() {
        return false;
    }
    let Ok(output) = Command::new(tool)
        .args(["-density", "96"])
        .arg(input.path())
        .args(["-depth", "8", "rgb:-"])
        .stderr(Stdio::null())
        .output()
    else {
        return false;
    };
    if !output.status.success() || output.stdout.len() != 24 * 8 * 3 {
        return false;
    }
    [
        (4, [0, 255, 0]),
        (9, [255, 255, 255]),
        (14, [255, 0, 0]),
        (20, [0, 0, 255]),
    ]
    .iter()
    .all(|(x, color)| {
        let offset = (4 * 24 + x) * 3;
        output.stdout[offset..offset + 3] == *color
    })
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
    -o, --out <PATH>   Output file. `.png` uses librsvg or a compatible ImageMagick SVG delegate; any
                       other extension writes SVG. Omit for SVG on stdout.
    -t, --theme <T>    light (default) | dark.
        --bg <COLOR>   Background (`none` = transparent). Default #ffffff
                       (light) / #1e1e1e (dark).
    -h, --help         Show this help.\n\
\n\
    PNG uses rsvg-convert (Debian/Ubuntu: librsvg2-bin), or ImageMagick\n\
with an SVG delegate supporting clipping, masks and paint order.
";
