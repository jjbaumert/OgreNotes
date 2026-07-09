//! Flowchart node shape library: layout footprint + SVG geometry per
//! `ShapeKind`. `size_for` computes the node's (w, h) box for a given label
//! size (per-shape padding — diamond/hexagon inscribe the label so they
//! inflate more). `emit` renders the shape's geometry only; `svg.rs`
//! overlays the label text on top.

use crate::flowchart::ShapeKind;

pub(crate) fn size_for(shape: ShapeKind, tw: f64, th: f64) -> (f64, f64) {
    match shape {
        ShapeKind::Rect | ShapeKind::Rounded => (tw + 24.0, th + 16.0),
        ShapeKind::Stadium => (tw + 32.0, th + 16.0),
        ShapeKind::Circle => {
            let d = tw.max(th) + 28.0;
            (d, d)
        }
        ShapeKind::DoubleCircle => {
            let d = tw.max(th) + 36.0;
            (d, d)
        }
        ShapeKind::Diamond => (tw * 1.7 + 24.0, th * 2.2 + 12.0),
        ShapeKind::Hexagon => (tw + 48.0, th + 16.0),
        ShapeKind::Parallelogram | ShapeKind::ParallelogramRev => (tw + 44.0, th + 16.0),
        ShapeKind::Trapezoid | ShapeKind::TrapezoidRev => (tw + 52.0, th + 16.0),
        ShapeKind::Cylinder => (tw + 28.0, th + 28.0),
        ShapeKind::Flag => (tw + 36.0, th + 16.0),
    }
}

const FILL: &str = "var(--mermaid-node-fill, #ececff)";

pub(crate) fn emit(shape: ShapeKind, cx: f64, cy: f64, w: f64, h: f64) -> String {
    let (x, y) = (cx - w / 2.0, cy - h / 2.0);
    let common = format!(r#"fill="{FILL}" stroke="currentColor" stroke-width="1""#);
    match shape {
        ShapeKind::Rect => format!(
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" {common}/>"#
        ),
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
            let svg = emit(s, 100.0, 50.0, 120.0, 40.0);
            assert!(
                svg.contains("<rect") || svg.contains("<circle")
                    || svg.contains("<polygon") || svg.contains("<path")
                    || svg.contains("<ellipse"),
                "{s:?} emitted no geometry: {svg}"
            );
            assert!(svg.contains("currentColor"), "{s:?} not theme-aware");
            assert!(!svg.contains("NaN"), "{s:?} produced NaN");
        }
    }

    #[test]
    fn double_circle_has_two_circles() {
        let svg = emit(ShapeKind::DoubleCircle, 0.0, 0.0, 60.0, 60.0);
        assert_eq!(svg.matches("<circle").count(), 2);
    }

    #[test]
    fn polygon_shapes_have_expected_point_counts() {
        let poly_points = |s: ShapeKind| {
            let svg = emit(s, 0.0, 0.0, 100.0, 40.0);
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
}
