//! Test-only SVG extent checker: approximate bounding boxes of everything
//! a renderer drew, so a test can assert nothing falls outside the
//! viewBox and that labels don't sit on top of nodes.
//!
//! This reads our own output, not arbitrary SVG: attributes are always
//! double-quoted, transforms are only `translate(..)` (other transforms
//! mark the subtree unchecked), and text width comes from the same
//! heuristic table the layouts use.

use crate::measure;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Box2 {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Box2 {
    fn new(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Box2 { x0: x0.min(x1), y0: y0.min(y1), x1: x0.max(x1), y1: y0.max(y1) }
    }
    fn shift(self, dx: f64, dy: f64) -> Self {
        Box2 { x0: self.x0 + dx, y0: self.y0 + dy, x1: self.x1 + dx, y1: self.y1 + dy }
    }
    /// Overlap area, 0 when they merely touch.
    pub fn overlap(&self, o: &Box2) -> f64 {
        let w = self.x1.min(o.x1) - self.x0.max(o.x0);
        let h = self.y1.min(o.y1) - self.y0.max(o.y0);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Drawn {
    /// Element name (`rect`, `text`, …).
    pub tag: String,
    /// Text content for `text` (lines joined by `\n`).
    pub text: String,
    pub bbox: Box2,
}

/// The viewBox and every drawn element's approximate box.
pub(crate) fn scan(svg: &str) -> (Box2, Vec<Drawn>) {
    let vb = attr(open_tag(svg, 0).1, "viewBox").expect("svg has a viewBox");
    let n: Vec<f64> = vb.split_whitespace().map(|v| v.parse().unwrap()).collect();
    let view = Box2::new(n[0], n[1], n[0] + n[2], n[1] + n[3]);

    let mut out = Vec::new();
    // (dx, dy) per open <g>; None = a transform we can't follow.
    let mut stack: Vec<Option<(f64, f64)>> = Vec::new();
    let mut defs_depth = 0usize;
    let mut i = 0;
    while let Some(off) = svg[i..].find('<') {
        let at = i + off;
        let (end, tag) = open_tag(svg, at);
        i = end;
        if let Some(name) = tag.strip_prefix('/') {
            let name = name.trim();
            if name == "g" {
                stack.pop();
            } else if matches!(name, "defs" | "marker" | "linearGradient" | "clipPath" | "mask") {
                defs_depth = defs_depth.saturating_sub(1);
            }
            continue;
        }
        let name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        let self_closing = tag.ends_with('/');
        if matches!(name, "defs" | "marker" | "linearGradient" | "clipPath" | "mask") {
            if !self_closing {
                defs_depth += 1;
            }
            continue;
        }
        if name == "g" {
            let t = attr(tag, "transform").map(|t| parse_translate(&t));
            let parent = stack.last().copied().unwrap_or(Some((0.0, 0.0)));
            let here = match (parent, t) {
                (Some(p), None) => Some(p),
                (Some(p), Some(Some(d))) => Some((p.0 + d.0, p.1 + d.1)),
                _ => None,
            };
            if !self_closing {
                stack.push(here);
            }
            continue;
        }
        if defs_depth > 0 {
            continue;
        }
        let Some((dx, dy)) = stack.last().copied().unwrap_or(Some((0.0, 0.0))) else {
            continue;
        };
        // An element's own translate (e.g. a rotated axis label) is not
        // followed either.
        if attr(tag, "transform").is_some() {
            continue;
        }
        let f = |k: &str| attr(tag, k).and_then(|v| v.trim_end_matches("px").parse::<f64>().ok());
        let bbox = match name {
            "rect" => {
                let (x, y) = (f("x").unwrap_or(0.0), f("y").unwrap_or(0.0));
                Some(Box2::new(x, y, x + f("width").unwrap_or(0.0), y + f("height").unwrap_or(0.0)))
            }
            "circle" => {
                let (cx, cy, r) = (f("cx").unwrap_or(0.0), f("cy").unwrap_or(0.0), f("r").unwrap_or(0.0));
                Some(Box2::new(cx - r, cy - r, cx + r, cy + r))
            }
            "ellipse" => {
                let (cx, cy) = (f("cx").unwrap_or(0.0), f("cy").unwrap_or(0.0));
                let (rx, ry) = (f("rx").unwrap_or(0.0), f("ry").unwrap_or(0.0));
                Some(Box2::new(cx - rx, cy - ry, cx + rx, cy + ry))
            }
            "line" => Some(Box2::new(
                f("x1").unwrap_or(0.0),
                f("y1").unwrap_or(0.0),
                f("x2").unwrap_or(0.0),
                f("y2").unwrap_or(0.0),
            )),
            "polygon" | "polyline" => attr(tag, "points").and_then(|p| points_box(&p)),
            "path" => attr(tag, "d").and_then(|d| path_box(&d)),
            "text" => {
                let close = svg[i..].find("</text>").map(|o| i + o).unwrap_or(i);
                let inner = &svg[i..close];
                let b = text_box(tag, inner);
                let text = b.as_ref().map(|(_, t)| t.clone()).unwrap_or_default();
                i = close;
                if let Some((bb, _)) = b {
                    out.push(Drawn { tag: "text".into(), text, bbox: bb.shift(dx, dy) });
                }
                None
            }
            _ => None,
        };
        if let Some(bb) = bbox {
            out.push(Drawn { tag: name.to_string(), text: String::new(), bbox: bb.shift(dx, dy) });
        }
    }
    (view, out)
}

/// Every drawn element sticking more than 1px outside the viewBox.
pub(crate) fn outside(svg: &str) -> Vec<Drawn> {
    let (v, drawn) = scan(svg);
    drawn
        .into_iter()
        .filter(|d| {
            d.bbox.x0 < v.x0 - 1.0 || d.bbox.y0 < v.y0 - 1.0 || d.bbox.x1 > v.x1 + 1.0 || d.bbox.y1 > v.y1 + 1.0
        })
        .collect()
}

/// Panic with the offenders if anything is drawn outside the viewBox.
#[track_caller]
pub(crate) fn assert_inside(svg: &str) {
    let bad = outside(svg);
    assert!(bad.is_empty(), "drawn outside the viewBox: {bad:#?}\n{svg}");
}

fn text_box(tag: &str, inner: &str) -> Option<(Box2, String)> {
    let size = style_font_size(tag).unwrap_or(measure::FONT_PX);
    let scale = size / measure::FONT_PX;
    let anchor = attr(tag, "text-anchor").unwrap_or_default();
    let middle = matches!(attr(tag, "dominant-baseline").as_deref(), Some("middle" | "central"));
    let x = attr(tag, "x").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let mut y = attr(tag, "y").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);

    // (x, baseline y, text) per line.
    let mut lines: Vec<(f64, f64, String)> = Vec::new();
    if inner.contains("<tspan") {
        let mut j = 0;
        while let Some(o) = inner[j..].find("<tspan") {
            let at = j + o;
            let (end, t) = open_tag(inner, at);
            let close = inner[end..].find("</tspan>").map(|o| end + o).unwrap_or(end);
            let lx = attr(t, "x").and_then(|v| v.parse::<f64>().ok()).unwrap_or(x);
            if let Some(ty) = attr(t, "y").and_then(|v| v.parse::<f64>().ok()) {
                y = ty;
            }
            y += attr(t, "dy").map(|v| parse_len(&v, size)).unwrap_or(0.0);
            lines.push((lx, y, unescape(&inner[end..close])));
            j = close;
        }
    } else {
        lines.push((x, y, unescape(inner)));
    }
    if lines.iter().all(|(_, _, t)| t.trim().is_empty()) {
        return None;
    }
    let mut bb: Option<Box2> = None;
    for (lx, ly, t) in &lines {
        let w = measure::text_size(t).0 * scale;
        let x0 = match anchor.as_str() {
            "middle" => lx - w / 2.0,
            "end" => lx - w,
            _ => *lx,
        };
        let (top, bottom) = if middle { (ly - size * 0.5, ly + size * 0.5) } else { (ly - size * 0.8, ly + size * 0.2) };
        let b = Box2::new(x0, top, x0 + w, bottom);
        bb = Some(match bb {
            None => b,
            Some(a) => Box2::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1)),
        });
    }
    let text = lines.into_iter().map(|(_, _, t)| t).collect::<Vec<_>>().join("\n");
    bb.map(|b| (b, text))
}

fn parse_len(v: &str, font: f64) -> f64 {
    if let Some(em) = v.strip_suffix("em") {
        em.parse::<f64>().unwrap_or(0.0) * font
    } else {
        v.trim_end_matches("px").parse().unwrap_or(0.0)
    }
}

fn style_font_size(tag: &str) -> Option<f64> {
    if let Some(v) = attr(tag, "font-size") {
        return v.trim_end_matches("px").parse().ok();
    }
    let style = attr(tag, "style")?;
    let at = style.find("font-size:")?;
    let rest = &style[at + "font-size:".len()..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// `(end index past '>', tag text without the angle brackets)`.
fn open_tag(s: &str, at: usize) -> (usize, &str) {
    let end = s[at..].find('>').map(|o| at + o).unwrap_or(s.len());
    (end.saturating_add(1).min(s.len()), s[at + 1..end].trim_end())
}

fn attr(tag: &str, key: &str) -> Option<String> {
    let mut from = 0;
    while let Some(o) = tag[from..].find(key) {
        let at = from + o;
        let before_ok = at == 0 || tag.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = &tag[at + key.len()..];
        if before_ok {
            if let Some(v) = rest.strip_prefix("=\"") {
                return v.find('"').map(|e| v[..e].to_string());
            }
        }
        from = at + key.len();
    }
    None
}

fn parse_translate(t: &str) -> Option<(f64, f64)> {
    let inner = t.trim().strip_prefix("translate(")?.strip_suffix(')')?;
    let mut it = inner.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty());
    let x = it.next()?.parse().ok()?;
    let y = it.next().map(|v| v.parse().ok()).unwrap_or(Some(0.0))?;
    if it.next().is_some() || t.contains(") ") {
        return None;
    }
    Some((x, y))
}

fn numbers(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() || c == '.' || (c == '-' && cur.is_empty()) || c == 'e' && !cur.is_empty() {
            cur.push(c);
        } else {
            if let Ok(v) = cur.parse() {
                out.push(v);
            }
            cur.clear();
            if c == '-' {
                cur.push(c);
            }
        }
    }
    if let Ok(v) = cur.parse() {
        out.push(v);
    }
    out
}

fn points_box(p: &str) -> Option<Box2> {
    pairs_box(&numbers(p))
}

fn pairs_box(n: &[f64]) -> Option<Box2> {
    let mut it = n.chunks_exact(2);
    let first = it.next()?;
    let mut b = Box2::new(first[0], first[1], first[0], first[1]);
    for p in it {
        b = Box2::new(b.x0.min(p[0]), b.y0.min(p[1]), b.x1.max(p[0]), b.y1.max(p[1]));
    }
    Some(b)
}

/// Absolute-command paths only (what our renderers emit). Arc commands
/// contribute their end point plus radius slack around it; relative
/// commands make the path unchecked.
fn path_box(d: &str) -> Option<Box2> {
    if d.chars().any(|c| matches!(c, 'm' | 'l' | 'c' | 'q' | 'h' | 'v' | 'a' | 's' | 't')) {
        return None;
    }
    let mut pts: Vec<f64> = Vec::new();
    let mut cmd = ' ';
    let mut args = String::new();
    let flush = |cmd: char, args: &str, pts: &mut Vec<f64>| {
        let n = numbers(args);
        match cmd {
            'A' => {
                for a in n.chunks_exact(7) {
                    pts.extend([a[5], a[6]]);
                }
            }
            'H' | 'V' => {}
            _ => pts.extend(n),
        }
    };
    for c in d.chars() {
        if c.is_ascii_alphabetic() && c != 'e' {
            flush(cmd, &args, &mut pts);
            args.clear();
            cmd = c;
        } else {
            args.push(c);
        }
    }
    flush(cmd, &args, &mut pts);
    pairs_box(&pts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_elements_outside_the_viewbox() {
        let svg = r#"<svg viewBox="0 0 100 50"><defs><marker><path d="M 0 0 L 900 900"/></marker></defs><rect x="10" y="10" width="20" height="20"/><g transform="translate(90,0)"><rect x="0" y="0" width="20" height="5"/></g><text x="5" y="40" style="font-size:14px">hi</text></svg>"#;
        let bad = outside(svg);
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert_eq!(bad[0].bbox.x1, 110.0);
    }

    #[test]
    fn tspan_lines_accumulate_dy() {
        let svg = r#"<svg viewBox="0 0 100 100"><text x="50" y="10" text-anchor="middle"><tspan x="50" dy="0">a</tspan><tspan x="50" dy="19">b</tspan></text></svg>"#;
        let (_, d) = scan(svg);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].text, "a\nb");
        assert!((d[0].bbox.y1 - (29.0 + 14.0 * 0.2)).abs() < 1e-9);
    }
}
