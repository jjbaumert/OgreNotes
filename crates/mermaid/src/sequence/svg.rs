//! SVG document assembly for sequence diagrams. Consumes the parsed
//! `SeqDiagram` plus the bespoke lifeline layout's `SeqLayout` (column
//! x-positions, row y-positions, activation spans, fragment frames) and
//! emits a single `<svg>` string.
//!
//! Z-order matters (see task-6-brief.md for the normative structure this
//! file implements verbatim):
//! 1. `<svg>` open tag.
//! 2. `<defs>` — arrow/cross/async markers.
//! 3. Fragment frames (depth ascending, so nested frames paint on top of
//!    their parent) + dividers.
//! 4. Lifelines.
//! 5. Activation rects.
//! 6. Messages (line/path + text + autonumber prefix).
//! 7. Notes.
//! 8. Participant boxes, top pass then bottom pass (actors get a stick
//!    figure instead of a box).
//! 9. `</svg>`.

use crate::escape_xml;
use crate::measure;
use crate::sequence::layout::{SeqLayout, ACTOR_EXTRA_H, ACT_OFFSET, ACT_W, PAD, SELF_EXTRA, SELF_STUB};
use crate::sequence::{Event, Head, LineStyle, SeqDiagram};

/// `Head::None` has no marker — a test asserts zero `marker-end`
/// occurrences when every message on the diagram is headless.
fn marker_id(h: Head) -> Option<&'static str> {
    match h {
        Head::None => None,
        Head::Arrow => Some("mmd-arrow"),
        Head::Cross => Some("mmd-cross"),
        Head::Async => Some("mmd-async"),
    }
}

/// A box-header participant: centered rect + centered label.
fn draw_box(out: &mut String, cx: f64, top_y: f64, box_w: f64, box_h: f64, label: &str) {
    out.push_str(&format!(
        r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor" rx="4"/>"#,
        cx - box_w / 2.0,
        top_y,
        box_w,
        box_h
    ));
    // Display may contain <br/> line breaks; measure::lines splits the
    // same way the box width/head height were measured. Single-line
    // output is position-identical to the old code (dy = +5 baseline).
    let lines = measure::lines(label);
    let n = lines.len();
    out.push_str(&format!(
        r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">"#,
        top_y + box_h / 2.0
    ));
    for (idx, line) in lines.iter().enumerate() {
        let dy = if idx == 0 {
            -((n as f64 - 1.0) / 2.0) * measure::LINE_H + 5.0
        } else {
            measure::LINE_H
        };
        out.push_str(&format!(
            r#"<tspan x="{cx:.1}" dy="{dy:.1}">{}</tspan>"#,
            escape_xml(line)
        ));
    }
    out.push_str("</text>");
}

/// An actor-header participant: stick figure (head circle + body/arms/legs
/// path) with the label centered below it. `top_y` is the top edge of the
/// figure's head (same semantics as `draw_box`'s `top_y`).
fn draw_actor(out: &mut String, cx: f64, top_y: f64, label: &str) {
    let head_cy = top_y + 7.0;
    out.push_str(&format!(
        r#"<circle cx="{cx:.1}" cy="{head_cy:.1}" r="7" stroke="currentColor" fill="none"/>"#
    ));
    out.push_str(&format!(
        r#"<path d="M {cx:.1} {y7:.1} V {y22:.1} M {xl:.1} {y13:.1} H {xr:.1} M {cx:.1} {y22:.1} L {xll:.1} {y32:.1} M {cx:.1} {y22:.1} L {xrr:.1} {y32:.1}" stroke="currentColor" fill="none"/>"#,
        cx = cx,
        y7 = head_cy + 7.0,
        y22 = head_cy + 22.0,
        xl = cx - 8.0,
        y13 = head_cy + 13.0,
        xr = cx + 8.0,
        xll = cx - 7.0,
        y32 = head_cy + 32.0,
        xrr = cx + 7.0,
    ));
    let lines = measure::lines(label);
    out.push_str(&format!(
        r#"<text x="{cx:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">"#,
        head_cy + 32.0 + 14.0
    ));
    for (idx, line) in lines.iter().enumerate() {
        let dy = if idx == 0 { 0.0 } else { measure::LINE_H };
        out.push_str(&format!(
            r#"<tspan x="{cx:.1}" dy="{dy:.1}">{}</tspan>"#,
            escape_xml(line)
        ));
    }
    out.push_str("</text>");
}

pub(crate) fn emit(d: &SeqDiagram, l: &SeqLayout) -> String {
    let (w, h) = l.size;
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w:.0} {h:.0}" width="{w:.0}" height="{h:.0}" style="font-family:sans-serif;font-size:14px">"#
    );

    // 2. defs: arrow/cross/async markers.
    out.push_str(concat!(
        "<defs>",
        r#"<marker id="mmd-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/></marker>"#,
        r#"<marker id="mmd-cross" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 1 1 L 9 9 M 9 1 L 1 9" stroke="currentColor" stroke-width="1.5" fill="none"/></marker>"#,
        r#"<marker id="mmd-async" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10" stroke="currentColor" stroke-width="1.5" fill="none"/></marker>"#,
        "</defs>",
    ));

    // 3. fragment frames, outer first (depth ascending, stable by index)
    // so nested frames paint on top of their parent.
    let mut frame_order: Vec<usize> = (0..l.frames.len()).collect();
    frame_order.sort_by_key(|&i| l.frames[i].depth);
    for i in frame_order {
        let f = &l.frames[i];
        let r = &f.rect;
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="none" stroke="currentColor" stroke-width="1" rx="3"/>"#,
            r.x, r.y, r.w, r.h
        ));
        let keyword = f.kind.keyword();
        let tab_w = measure::text_size(keyword).0 + 16.0;
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="20" fill="var(--mermaid-cluster-fill, #7773)"/>"#,
            r.x, r.y, tab_w
        ));
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" fill="currentColor" font-weight="bold">{keyword}</text>"#,
            r.x + 8.0,
            r.y + 14.0
        ));
        if !f.label.is_empty() {
            out.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" fill="currentColor">[{}]</text>"#,
                r.x + tab_w + 8.0,
                r.y + 14.0,
                escape_xml(&f.label)
            ));
        }
        for (dy, label) in &f.dividers {
            out.push_str(&format!(
                r#"<line x1="{:.1}" y1="{dy:.1}" x2="{:.1}" y2="{dy:.1}" stroke="currentColor" stroke-dasharray="4 3"/>"#,
                r.x,
                r.x + r.w,
            ));
            out.push_str(&format!(
                r#"<text x="{:.1}" y="{:.1}" text-anchor="middle" fill="currentColor">[{}]</text>"#,
                r.x + r.w / 2.0,
                dy - 4.0,
                escape_xml(label)
            ));
        }
    }

    // 4. lifelines.
    let head_bottom = PAD + l.head_h;
    for &cx in &l.col_x {
        out.push_str(&format!(
            r#"<line x1="{cx:.1}" y1="{head_bottom:.1}" x2="{cx:.1}" y2="{:.1}" stroke="currentColor" stroke-dasharray="3 3" stroke-width="1"/>"#,
            l.body_bottom
        ));
    }

    // 5. activation rects.
    for a in &l.activations {
        let cx = l.col_x.get(a.p).copied().unwrap_or(0.0);
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{ACT_W:.1}" height="{:.1}" fill="var(--mermaid-node-fill, #ececff)" stroke="currentColor"/>"#,
            cx - ACT_W / 2.0 + a.depth as f64 * ACT_OFFSET,
            a.y0,
            a.y1 - a.y0
        ));
    }

    // 6. messages: line/path + text + autonumber prefix.
    for m in &l.messages {
        let Some(Event::Message { from, to, line, head, from_head, text, .. }) = d.events.get(m.event) else {
            continue;
        };
        let self_msg = from == to;
        let fx = l.col_x.get(*from).copied().unwrap_or(0.0);
        let tx = l.col_x.get(*to).copied().unwrap_or(fx);
        let marker_attr = match marker_id(*head) {
            Some(id) => format!(r#" marker-end="url(#{id})""#),
            None => String::new(),
        };
        let marker_start_attr = match marker_id(*from_head) {
            Some(id) => format!(r#" marker-start="url(#{id})""#),
            None => String::new(),
        };
        let dash_attr = if *line == LineStyle::Dotted { r#" stroke-dasharray="4 3""# } else { "" };
        if self_msg {
            // A rounded loop off the lifeline: cubic Bézier from the upper
            // point out to the right and back to the lower point, so the
            // arrowhead re-enters the lifeline pointing left (matching
            // Mermaid's self-message arc rather than a square bracket).
            out.push_str(&format!(
                r#"<path d="M {x0:.1} {y0:.1} C {x1:.1} {y0:.1} {x1:.1} {y1:.1} {x0:.1} {y1:.1}" fill="none" stroke="currentColor"{dash_attr}{marker_attr}{marker_start_attr}/>"#,
                x0 = fx + ACT_W / 2.0,
                y0 = m.y - SELF_EXTRA / 2.0,
                x1 = fx + SELF_STUB,
                y1 = m.y + SELF_EXTRA / 2.0,
            ));
        } else {
            out.push_str(&format!(
                r#"<line x1="{fx:.1}" y1="{y:.1}" x2="{tx:.1}" y2="{y:.1}" stroke="currentColor"{dash_attr}{marker_attr}{marker_start_attr}/>"#,
                y = m.y,
            ));
        }

        let anchor = if self_msg { "start" } else { "middle" };
        let lines = measure::lines(text);
        let n_lines = lines.len();
        out.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="{anchor}" fill="currentColor">"#,
            m.text_anchor.0, m.text_anchor.1
        ));
        for (idx, line_text) in lines.iter().enumerate() {
            let dy = if idx == 0 { -((n_lines as f64 - 1.0) / 2.0) * measure::LINE_H } else { measure::LINE_H };
            let escaped = escape_xml(line_text);
            let content = if idx == 0 {
                match m.number {
                    Some(n) => format!("{n}. {escaped}"),
                    None => escaped,
                }
            } else {
                escaped
            };
            out.push_str(&format!(r#"<tspan x="{:.1}" dy="{dy:.1}">{content}</tspan>"#, m.text_anchor.0));
        }
        out.push_str("</text>");
    }

    // 7. notes.
    for note in &l.notes {
        let Some(Event::Note { text, .. }) = d.events.get(note.event) else {
            continue;
        };
        let r = &note.rect;
        out.push_str(&format!(
            r#"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" fill="var(--mermaid-note-fill, #fff5ad)" stroke="currentColor" rx="2"/>"#,
            r.x, r.y, r.w, r.h
        ));
        let cx = r.x + r.w / 2.0;
        let cy = r.y + r.h / 2.0;
        let lines = measure::lines(text);
        let n_lines = lines.len();
        out.push_str(&format!(
            r#"<text x="{cx:.1}" y="{cy:.1}" text-anchor="middle" fill="var(--mermaid-note-text, #333)">"#
        ));
        for (idx, line_text) in lines.iter().enumerate() {
            let dy = if idx == 0 { -((n_lines as f64 - 1.0) / 2.0) * measure::LINE_H + 4.0 } else { measure::LINE_H };
            out.push_str(&format!(r#"<tspan x="{cx:.1}" dy="{dy:.1}">{}</tspan>"#, escape_xml(line_text)));
        }
        out.push_str("</text>");
    }

    // 8. participant boxes, top pass then bottom pass. Actors get a stick
    // figure instead of a box.
    let any_actor = d.participants.iter().any(|p| p.is_actor);
    let actor_extra = if any_actor { ACTOR_EXTRA_H } else { 0.0 };
    let box_h = (l.head_h - actor_extra).max(0.0);

    for (i, p) in d.participants.iter().enumerate() {
        let cx = l.col_x.get(i).copied().unwrap_or(0.0);
        if p.is_actor {
            draw_actor(&mut out, cx, PAD, &p.display);
        } else {
            let box_w = l.box_w.get(i).copied().unwrap_or(0.0);
            draw_box(&mut out, cx, PAD + actor_extra, box_w, box_h, &p.display);
        }
    }
    let bottom_top_y = l.body_bottom + 8.0;
    for (i, p) in d.participants.iter().enumerate() {
        let cx = l.col_x.get(i).copied().unwrap_or(0.0);
        if p.is_actor {
            draw_actor(&mut out, cx, bottom_top_y, &p.display);
        } else {
            let box_w = l.box_w.get(i).copied().unwrap_or(0.0);
            draw_box(&mut out, cx, bottom_top_y, box_w, box_h, &p.display);
        }
    }

    // 9. close.
    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    use crate::sequence::render_sequence;

    #[test]
    fn self_message_renders_as_a_curved_loop() {
        let svg = render_sequence("sequenceDiagram\nA->>A: think").unwrap();
        // The self-message is a cubic Bézier loop (`C`), not a square
        // `H`/`V` bracket, and still carries its arrowhead.
        assert!(svg.contains(" C "), "self-message not curved: {svg}");
        assert!(!svg.contains(" H "), "self-message still square: {svg}");
        assert!(svg.contains("marker-end"), "{svg}");
    }





    #[test]
    fn basic_exchange_renders() {
        let svg = render_sequence("sequenceDiagram\nparticipant A as Alice\nA->>B: Hello\nB-->>A: Hi").unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Alice") && svg.contains("Hello"));
        assert!(svg.contains("mmd-arrow"));
        assert!(svg.contains("stroke-dasharray=\"3 3\"")); // lifelines
        // top + bottom participant boxes: each display appears twice
        assert!(svg.matches(">Alice<").count() >= 2);
    }

    #[test]
    fn all_heads_get_their_markers() {
        let svg = render_sequence("sequenceDiagram\nA-xB: c\nA-)B: a\nA->>B: n").unwrap();
        assert!(svg.contains("url(#mmd-cross)"));
        assert!(svg.contains("url(#mmd-async)"));
        assert!(svg.contains("url(#mmd-arrow)"));
    }

    #[test]
    fn open_head_has_no_marker_reference_on_its_line() {
        let svg = render_sequence("sequenceDiagram\nA->B: plain").unwrap();
        assert_eq!(svg.matches("marker-end").count(), 0);
    }

    #[test]
    fn bidirectional_message_has_markers_both_ends() {
        let svg = render_sequence("sequenceDiagram\nA<<->>B: both").unwrap();
        assert!(svg.contains(r##"marker-start="url(#mmd-arrow)""##), "{svg}");
        assert!(svg.contains(r##"marker-end="url(#mmd-arrow)""##), "{svg}");
    }

    #[test]
    fn actor_renders_stick_figure() {
        let svg = render_sequence("sequenceDiagram\nactor U as User\nU->>S: go").unwrap();
        assert!(svg.contains("<circle")); // head
    }

    #[test]
    fn note_renders_with_note_fill() {
        let svg = render_sequence("sequenceDiagram\nA->>B: x\nNote over A,B: spanning note").unwrap();
        assert!(svg.contains("--mermaid-note-fill"));
        assert!(svg.contains("spanning note"));
    }

    #[test]
    fn note_over_three_spans_outer_lifelines() {
        let svg = render_sequence(
            "sequenceDiagram\nA->>B: x\nB->>C: y\nNote over A,C: wide",
        )
        .unwrap();
        // Lifelines are the dasharray-"3 3" <line> elements; the note
        // rect (note-fill) must span at least from the first to the
        // last lifeline x.
        let cols: Vec<f64> = svg
            .match_indices("<line ")
            .filter_map(|(i, _)| {
                let seg = &svg[i..i + svg[i..].find("/>").unwrap()];
                if !seg.contains("stroke-dasharray=\"3 3\"") {
                    return None;
                }
                let j = seg.find("x1=\"").unwrap() + 4;
                seg[j..].split('"').next().unwrap().parse().ok()
            })
            .collect();
        assert_eq!(cols.len(), 3, "{svg}");
        let ri = svg.find("--mermaid-note-fill").unwrap();
        let rect = &svg[svg[..ri].rfind("<rect").unwrap()..ri];
        let attr = |name: &str| -> f64 {
            let j = rect.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
            rect[j..].split('"').next().unwrap().parse().unwrap()
        };
        let (x, w) = (attr("x"), attr("width"));
        let cmin = cols.iter().cloned().fold(f64::MAX, f64::min);
        let cmax = cols.iter().cloned().fold(f64::MIN, f64::max);
        assert!(
            x <= cmin && x + w >= cmax,
            "note {x}..{} vs cols {cmin}..{cmax}: {svg}",
            x + w
        );
    }

    #[test]
    fn fragment_frame_and_divider() {
        let svg = render_sequence("sequenceDiagram\nalt good\nA->>B: y\nelse bad\nA-xB: n\nend").unwrap();
        assert!(svg.contains(">alt<") || svg.contains(">alt</"));
        assert!(svg.contains("[good]"));
        assert!(svg.contains("[bad]"));
        assert!(svg.contains("stroke-dasharray=\"4 3\"")); // divider
    }

    #[test]
    fn activation_rect_present() {
        let svg = render_sequence("sequenceDiagram\nA->>+B: go\nB-->>-A: done").unwrap();
        assert!(svg.contains("--mermaid-node-fill"));
    }

    #[test]
    fn autonumber_prefixes() {
        let svg = render_sequence("sequenceDiagram\nautonumber\nA->>B: first\nB->>A: second").unwrap();
        assert!(svg.contains("1. first") || svg.contains(">1. "));
        assert!(svg.contains("2. second") || svg.contains(">2. "));
    }

    #[test]
    fn user_strings_escaped() {
        let svg = render_sequence("sequenceDiagram\nparticipant A as <b>Bad</b>\nA->>B: <script>x</script>").unwrap();
        assert!(!svg.contains("<script>"));
        assert!(!svg.contains("<b>"));
        assert!(svg.contains("&lt;script&gt;"));
    }

    #[test]
    fn parse_error_propagates() {
        assert!(render_sequence("sequenceDiagram\nend").is_err());
    }

    #[test]
    fn participant_display_line_breaks_render_as_tspans() {
        let svg = render_sequence(
            "sequenceDiagram\nparticipant A as Alice<br/>Johnson\nactor B as Bob<br/>Builder\nA->>B: x",
        )
        .unwrap();
        assert!(!svg.contains("&lt;br/&gt;"), "literal <br/> leaked: {svg}");
        // Each display line appears in top AND bottom header passes,
        // box form (Alice/Johnson) and actor form (Bob/Builder).
        for frag in [">Alice<", ">Johnson<", ">Bob<", ">Builder<"] {
            assert!(svg.matches(frag).count() >= 2, "{frag} missing: {svg}");
        }
    }
}
