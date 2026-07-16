//! Flowchart node shape library: layout footprint + SVG geometry per
//! `ShapeKind`. `size_for` computes the node's (w, h) box for a given label
//! size (per-shape padding — diamond/hexagon inscribe the label so they
//! inflate more). `emit` renders the shape's geometry only; `svg.rs`
//! overlays the label text on top.

use crate::escape_xml;
use crate::flowchart::ShapeKind;

pub(crate) fn size_for(shape: ShapeKind, tw: f64, th: f64) -> (f64, f64) {
    match shape {
        ShapeKind::Rect | ShapeKind::Rounded => (tw + 24.0, th + 16.0),
        ShapeKind::Subroutine => (tw + 40.0, th + 16.0),
        ShapeKind::Stadium => (tw + 32.0, th + 16.0),
        ShapeKind::Circle => {
            let d = tw.max(th) + 28.0;
            (d, d)
        }
        ShapeKind::DoubleCircle => {
            let d = tw.max(th) + 36.0;
            (d, d)
        }
        ShapeKind::Diamond => {
            // Mermaid sizes a decision node as a 45°-rotated square: one side
            // value drives BOTH axes so the bounding box is always square,
            // regardless of the label's aspect. `s = (labelW + pad) + (labelH +
            // pad)` also guarantees the label inscribes (an axis-aligned rect
            // fits a rhombus when labelW + labelH <= s).
            let s = (tw + DIAMOND_PAD) + (th + DIAMOND_PAD);
            (s, s)
        }
        ShapeKind::Hexagon => (tw + 48.0, th + 16.0),
        ShapeKind::Parallelogram | ShapeKind::ParallelogramRev => (tw + 44.0, th + 16.0),
        ShapeKind::Trapezoid | ShapeKind::TrapezoidRev => (tw + 52.0, th + 16.0),
        ShapeKind::Cylinder => (tw + 28.0, th + 28.0),
        ShapeKind::Flag => (tw + 36.0, th + 16.0),
    }
}

const FILL: &str = "var(--mermaid-node-fill, #ececff)";
/// Mermaid's `nodeBorder` (default theme). Shared by every `shapes::emit`
/// consumer (flowchart, stateDiagram-v2, mindmap).
const BORDER: &str = "var(--mermaid-node-border, #9370DB)";
/// Per-side padding for a decision (diamond) node — Mermaid's flowchart default.
const DIAMOND_PAD: f64 = 15.0;

/// `style`, when present, is emitted as an inline `style="…"` ON the shape
/// element(s). This matters: the shape carries `fill`/`stroke`
/// *presentation attributes*, which an inherited group style can't
/// override — an inline `style` on the same element can. Callers still
/// wrap the node in a `<g style>` too, so `color:` reaches the label text.
///
/// The style value is XML-escaped here so this helper is self-defending
/// against attribute injection regardless of what a caller passes (the
/// flowchart caller already sanitizes it against an allowlist, but a
/// `pub(crate)` helper shouldn't rely on every caller doing so).
pub(crate) fn emit(shape: ShapeKind, cx: f64, cy: f64, w: f64, h: f64, style: Option<&str>) -> String {
    let (x, y) = (cx - w / 2.0, cy - h / 2.0);
    let style_attr = match style {
        Some(s) if !s.is_empty() => format!(r#" style="{}""#, escape_xml(s)),
        _ => String::new(),
    };
    let common = format!(r#"fill="{FILL}" stroke="{BORDER}" stroke-width="1"{style_attr}"#);
    match shape {
        ShapeKind::Rect => format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" {common}/>"#
        ),
        ShapeKind::Subroutine => {
            let inset = 6.0;
            let (x1, x2, yb) = (x + inset, x + w - inset, y + h);
            format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" {common}/><line x1="{x1:.1}" y1="{y:.1}" x2="{x1:.1}" y2="{yb:.1}" stroke="currentColor" stroke-width="1"/><line x1="{x2:.1}" y1="{y:.1}" x2="{x2:.1}" y2="{yb:.1}" stroke="currentColor" stroke-width="1"/>"#
            )
        }
        ShapeKind::Rounded => format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="8" {common}/>"#
        ),
        ShapeKind::Stadium => {
            let rx = h / 2.0;
            format!(
                r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="{rx:.1}" {common}/>"#
            )
        }
        ShapeKind::Circle => {
            let r = w.min(h) / 2.0;
            format!(r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" {common}/>"#)
        }
        ShapeKind::DoubleCircle => {
            let d = w.min(h);
            let outer = d / 2.0;
            let inner = outer - 5.0;
            format!(
                r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{outer:.1}" {common}/><circle cx="{cx:.1}" cy="{cy:.1}" r="{inner:.1}" {common}/>"#
            )
        }
        ShapeKind::Diamond => {
            let pts = format!(
                "{cx:.1},{y:.1} {:.1},{cy:.1} {cx:.1},{:.1} {:.1},{cy:.1}",
                x + w,
                y + h,
                x
            );
            format!(r#"<polygon points="{pts}" {common}/>"#)
        }
        ShapeKind::Hexagon => {
            let cut = w * 0.25;
            let pts = format!(
                "{x:.1},{cy:.1} {:.1},{y:.1} {:.1},{y:.1} {:.1},{cy:.1} {:.1},{:.1} {:.1},{:.1}",
                x + cut,
                x + w - cut,
                x + w,
                x + w - cut,
                y + h,
                x + cut,
                y + h
            );
            format!(r#"<polygon points="{pts}" {common}/>"#)
        }
        ShapeKind::Parallelogram => {
            let skew = 15.0;
            let pts = format!(
                "{:.1},{y:.1} {:.1},{y:.1} {:.1},{:.1} {x:.1},{:.1}",
                x + skew,
                x + w,
                x + w - skew,
                y + h,
                y + h
            );
            format!(r#"<polygon points="{pts}" {common}/>"#)
        }
        ShapeKind::ParallelogramRev => {
            let skew = 15.0;
            let pts = format!(
                "{x:.1},{y:.1} {:.1},{y:.1} {:.1},{:.1} {:.1},{:.1}",
                x + w - skew,
                x + w,
                y + h,
                x + skew,
                y + h
            );
            format!(r#"<polygon points="{pts}" {common}/>"#)
        }
        ShapeKind::Trapezoid => {
            let inset = 15.0;
            let pts = format!(
                "{:.1},{y:.1} {:.1},{y:.1} {:.1},{:.1} {x:.1},{:.1}",
                x + inset,
                x + w - inset,
                x + w,
                y + h,
                y + h
            );
            format!(r#"<polygon points="{pts}" {common}/>"#)
        }
        ShapeKind::TrapezoidRev => {
            let inset = 15.0;
            let pts = format!(
                "{x:.1},{y:.1} {:.1},{y:.1} {:.1},{:.1} {:.1},{:.1}",
                x + w,
                x + w - inset,
                y + h,
                x + inset,
                y + h
            );
            format!(r#"<polygon points="{pts}" {common}/>"#)
        }
        ShapeKind::Cylinder => {
            let rx = w / 2.0;
            let ry = 7.0;
            let top = y + ry;
            let bottom = y + h - ry;
            let path = format!(
                "M {x:.1} {top:.1} L {x:.1} {bottom:.1} A {rx:.1} {ry:.1} 0 0 0 {:.1} {bottom:.1} L {:.1} {top:.1}",
                x + w,
                x + w
            );
            format!(
                r#"<path d="{path}" {common}/><ellipse cx="{cx:.1}" cy="{top:.1}" rx="{rx:.1}" ry="{ry:.1}" {common}/>"#
            )
        }
        ShapeKind::Flag => {
            let notch = 12.0;
            let pts = format!(
                "{x:.1},{y:.1} {:.1},{y:.1} {:.1},{:.1} {x:.1},{:.1} {:.1},{cy:.1}",
                x + w,
                x + w,
                y + h,
                y + h,
                x + notch
            );
            format!(r#"<polygon points="{pts}" {common}/>"#)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flowchart::ShapeKind;

    const ALL: &[ShapeKind] = &[
        ShapeKind::Rect, ShapeKind::Rounded, ShapeKind::Stadium,
        ShapeKind::Circle, ShapeKind::DoubleCircle, ShapeKind::Diamond,
        ShapeKind::Hexagon, ShapeKind::Parallelogram, ShapeKind::ParallelogramRev,
        ShapeKind::Trapezoid, ShapeKind::TrapezoidRev, ShapeKind::Cylinder,
        ShapeKind::Flag,
    ];

    #[test]
    fn every_shape_fits_its_text() {
        for &s in ALL {
            let (w, h) = size_for(s, 80.0, 19.0);
            assert!(w >= 80.0 && h >= 19.0, "{s:?} smaller than its text");
        }
    }

    #[test]
    fn diamond_inflates_more_than_rect() {
        assert!(size_for(ShapeKind::Diamond, 80.0, 19.0).0
            > size_for(ShapeKind::Rect, 80.0, 19.0).0);
    }

    #[test]
    fn circle_is_square_footprint() {
        let (w, h) = size_for(ShapeKind::Circle, 60.0, 19.0);
        assert_eq!(w, h);
    }

    #[test]
    fn every_shape_emits_geometry() {
        for &s in ALL {
            let svg = emit(s, 100.0, 50.0, 120.0, 40.0, None);
            assert!(
                svg.contains("<rect") || svg.contains("<circle")
                    || svg.contains("<polygon") || svg.contains("<path")
                    || svg.contains("<ellipse"),
                "{s:?} emitted no geometry: {svg}"
            );
            // Theme-aware via `currentColor` (inner lines) or the fill/border
            // CSS vars (the node border is now `var(--mermaid-node-border,…)`).
            assert!(
                svg.contains("currentColor") || svg.contains("var(--mermaid"),
                "{s:?} not theme-aware"
            );
            assert!(!svg.contains("NaN"), "{s:?} produced NaN");
        }
    }

    #[test]
    fn emit_escapes_the_style_value() {
        // Defense-in-depth: even if an unsanitized style reaches `emit`, it
        // must not break out of the `style="…"` attribute.
        let svg = emit(ShapeKind::Rect, 0.0, 0.0, 10.0, 10.0, Some(r#"x"/><script>y"#));
        assert!(!svg.contains("<script>"), "style broke out: {svg}");
        assert!(svg.contains("&quot;") && svg.contains("&lt;script&gt;"), "{svg}");
    }

    #[test]
    fn double_circle_has_two_circles() {
        let svg = emit(ShapeKind::DoubleCircle, 0.0, 0.0, 60.0, 60.0, None);
        assert_eq!(svg.matches("<circle").count(), 2);
    }

    #[test]
    fn polygon_shapes_have_expected_point_counts() {
        let poly_points = |s: ShapeKind| {
            let svg = emit(s, 0.0, 0.0, 100.0, 40.0, None);
            let pts = svg.split("points=\"").nth(1).unwrap()
                .split('"').next().unwrap();
            pts.split_whitespace().count()
        };
        assert_eq!(poly_points(ShapeKind::Diamond), 4);
        assert_eq!(poly_points(ShapeKind::Hexagon), 6);
        assert_eq!(poly_points(ShapeKind::Parallelogram), 4);
        assert_eq!(poly_points(ShapeKind::Trapezoid), 4);
        assert_eq!(poly_points(ShapeKind::Flag), 5);
    }

    #[test]
    fn subroutine_fits_text_and_emits_rect_with_two_bars() {
        let (w, h) = size_for(ShapeKind::Subroutine, 80.0, 19.0);
        assert!(w >= 80.0 && h >= 19.0);
        let svg = emit(ShapeKind::Subroutine, 100.0, 50.0, 120.0, 40.0, None);
        assert!(svg.contains("<rect"), "{svg}");
        assert_eq!(svg.matches("<line").count(), 2, "{svg}");
        assert!(svg.contains("currentColor") && !svg.contains("NaN"));
    }
}
