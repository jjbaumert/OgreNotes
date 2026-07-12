//! Two-pass lifeline layout: columns (participant x-positions, widened
//! to fit message text / notes / self-loops) then rows (a single
//! top-to-bottom cursor walk placing messages, notes, activation spans,
//! and fragment frames). Infallible — the parser already enforces the
//! size caps (`MAX_PARTICIPANTS`/`MAX_EVENTS`/`MAX_FRAGMENT_DEPTH`), so
//! `run` never fails and never panics (no `[]` indexing on
//! event-derived indices; `get`/`get_mut` + skip-on-mismatch instead).

use crate::measure::text_size;
use crate::sequence::{Event, FragmentKind, NotePlacement, SeqDiagram};

pub(crate) const PAD: f64 = 20.0;
pub(crate) const COL_GAP_MIN: f64 = 110.0;
pub(crate) const BOX_PAD_X: f64 = 24.0;
pub(crate) const BOX_PAD_Y: f64 = 12.0;
pub(crate) const ACTOR_EXTRA_H: f64 = 34.0;
pub(crate) const ROW_GAP: f64 = 10.0;
pub(crate) const MSG_MIN_H: f64 = 24.0;
pub(crate) const SELF_EXTRA: f64 = 16.0;
pub(crate) const SELF_STUB: f64 = 30.0;
pub(crate) const NOTE_PAD: f64 = 8.0;
pub(crate) const FRAME_HEAD: f64 = 26.0;
pub(crate) const FRAME_INSET: f64 = 8.0;
pub(crate) const FRAME_BOTTOM_PAD: f64 = 10.0;
pub(crate) const DIVIDER_H: f64 = 22.0;
pub(crate) const ACT_W: f64 = 10.0;
pub(crate) const ACT_OFFSET: f64 = 6.0;
/// Floor for a fragment frame's width. Deeply-nested frames with no (or
/// narrow) participants can otherwise compute a non-positive width
/// (`canvas_w - PAD - 2*inset`), which would violate the
/// `f.rect.w > 0.0` invariant `sequence/props.rs` asserts on every
/// successful layout. Keeping a small positive floor keeps frames
/// visible instead of collapsing to a sliver.
pub(crate) const FRAME_MIN_W: f64 = 8.0;

pub(crate) struct MsgLayout {
    pub event: usize,
    pub y: f64,
    pub text_anchor: (f64, f64),
    pub number: Option<u32>,
}

pub(crate) struct NoteLayout {
    pub event: usize,
    pub rect: crate::layout::Rect,
}

pub(crate) struct ActRect {
    pub p: usize,
    pub depth: usize,
    pub y0: f64,
    pub y1: f64,
}

pub(crate) struct FrameRect {
    pub kind: FragmentKind,
    pub label: String,
    pub rect: crate::layout::Rect,
    pub depth: usize,
    pub dividers: Vec<(f64, String)>,
}

pub(crate) struct SeqLayout {
    pub col_x: Vec<f64>,
    pub box_w: Vec<f64>,
    pub head_h: f64,
    /// Not read by the render pipeline (svg.rs derives its own vertical
    /// anchors from `head_h`/messages/frames); kept because
    /// `rows_monotonic_and_finite` asserts on it to verify the row
    /// cursor starts below the header strip. Same precedent as
    /// `flowchart::FlowNode.id` — see task-14-report.md.
    #[allow(dead_code)]
    pub body_top: f64,
    pub body_bottom: f64,
    pub messages: Vec<MsgLayout>,
    pub notes: Vec<NoteLayout>,
    pub activations: Vec<ActRect>,
    pub frames: Vec<FrameRect>,
    pub size: (f64, f64),
}

/// Shift columns `from..` right by `deficit` (no-op if `deficit <= 0`).
/// Shifting only ever moves columns further apart, never closer — an
/// earlier pair already satisfied (`col_x[hi] - col_x[lo] >= need`)
/// stays satisfied after a later shift, since both sides of an
/// already-processed pair either both move or the left one stays put
/// while the right one (and everything past it) moves further right.
/// That's why a single left-to-right pass over events, each only ever
/// widening, is sufficient — no fixpoint iteration needed.
fn shift_right(col_x: &mut [f64], from: usize, deficit: f64) {
    if deficit <= 0.0 {
        return;
    }
    for x in col_x.iter_mut().skip(from) {
        *x += deficit;
    }
}

/// Widen the gap between columns `i` and `j` (order-independent) to at
/// least `need`, by shifting the higher-indexed column (and everything
/// right of it) further right.
fn widen_gap(col_x: &mut [f64], i: usize, j: usize, need: f64) {
    if i == j || i >= col_x.len() || j >= col_x.len() {
        return;
    }
    let (lo, hi) = if i < j { (i, j) } else { (j, i) };
    let cur = col_x[hi] - col_x[lo];
    if cur < need {
        shift_right(col_x, hi, need - cur);
    }
}

/// Pass 1: participant column x-positions and box widths, widened in
/// event order to fit message text, notes, and self-message stubs.
fn pass1_columns(d: &SeqDiagram) -> (Vec<f64>, Vec<f64>, f64, f64) {
    let n = d.participants.len();
    let box_w: Vec<f64> = d
        .participants
        .iter()
        .map(|p| (text_size(&p.display).0 + BOX_PAD_X * 2.0).max(60.0))
        .collect();
    let max_text_h = d
        .participants
        .iter()
        .map(|p| text_size(&p.display).1)
        .fold(0.0_f64, f64::max);
    let any_actor = d.participants.iter().any(|p| p.is_actor);
    let head_h = max_text_h + BOX_PAD_Y * 2.0 + if any_actor { ACTOR_EXTRA_H } else { 0.0 };

    let mut col_x: Vec<f64> = Vec::with_capacity(n);
    if n > 0 {
        col_x.push(PAD + box_w[0] / 2.0);
        for i in 1..n {
            let gap = ((box_w[i - 1] + box_w[i]) / 2.0 + 20.0).max(COL_GAP_MIN);
            col_x.push(col_x[i - 1] + gap);
        }
    }

    let mut overhang_right = 0.0_f64;

    for ev in &d.events {
        match ev {
            Event::Message { from, to, text, .. } => {
                if from != to {
                    let need = text_size(text).0 + 24.0;
                    widen_gap(&mut col_x, *from, *to, need);
                } else {
                    let p = *from;
                    if p < col_x.len() {
                        let need_right = SELF_STUB + text_size(text).0 + 12.0;
                        if p + 1 >= col_x.len() {
                            overhang_right = overhang_right.max(need_right);
                        } else if let Some(&next_w) = box_w.get(p + 1) {
                            widen_gap(&mut col_x, p, p + 1, need_right + next_w / 2.0);
                        }
                    }
                }
            }
            Event::Note { placement, text } => {
                let note_w = text_size(text).0 + NOTE_PAD * 2.0;
                match placement {
                    NotePlacement::Over(a, Some(b)) => {
                        widen_gap(&mut col_x, *a, *b, note_w + 12.0);
                    }
                    NotePlacement::Over(_, None) => {}
                    NotePlacement::LeftOf(p) => {
                        if *p == 0 {
                            if let Some(&x0) = col_x.first() {
                                let need_margin = note_w + 12.0;
                                let cur_margin = x0 - PAD;
                                if cur_margin < need_margin {
                                    shift_right(&mut col_x, 0, need_margin - cur_margin);
                                }
                            }
                        } else if *p > 0 && *p < col_x.len() {
                            widen_gap(&mut col_x, p - 1, *p, note_w + 12.0);
                        }
                    }
                    NotePlacement::RightOf(p) => {
                        if *p + 1 >= col_x.len() {
                            if *p < col_x.len() {
                                overhang_right = overhang_right.max(note_w + 12.0);
                            }
                        } else {
                            widen_gap(&mut col_x, *p, *p + 1, note_w + 12.0);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    (col_x, box_w, head_h, overhang_right)
}

/// Pass 2: a single top-to-bottom cursor walk placing messages, notes,
/// activation spans, and fragment frames.
#[allow(clippy::too_many_arguments)]
fn pass2_rows(
    d: &SeqDiagram,
    col_x: &[f64],
    box_w: &[f64],
    body_top: f64,
    canvas_w: f64,
) -> (Vec<MsgLayout>, Vec<NoteLayout>, Vec<ActRect>, Vec<FrameRect>, f64) {
    let mut cursor = body_top;
    // `autonum` is the next number to assign (None = numbering disabled);
    // `autonum_step` is the increment applied after each numbered message.
    let mut autonum: Option<u32> = None;
    let mut autonum_step: u32 = 1;
    // (kind, label, top, depth-at-open, dividers)
    #[allow(clippy::type_complexity)]
    let mut frame_stack: Vec<(FragmentKind, String, f64, usize, Vec<(f64, String)>)> = Vec::new();
    // one open-span stack per participant: (depth, y0)
    let mut act_stacks: Vec<Vec<(usize, f64)>> = vec![Vec::new(); d.participants.len()];

    let mut messages = Vec::new();
    let mut notes = Vec::new();
    let mut activations = Vec::new();
    let mut frames = Vec::new();

    for (idx, ev) in d.events.iter().enumerate() {
        match ev {
            Event::Autonumber { start, step } => {
                autonum = Some(*start);
                autonum_step = *step;
            }
            Event::AutonumberOff => {
                autonum = None;
            }
            Event::Message { from, to, text, activate_target, deactivate_source, .. } => {
                let self_msg = from == to;
                let text_h = if text.is_empty() { 0.0 } else { text_size(text).1 };
                // Base footprint (arrow + label). Self-messages additionally
                // reserve SELF_EXTRA of vertical room below the line for the
                // loop-back stub; that extra room is spent AFTER the line
                // (it pushes the next row down) rather than baked into the
                // line's own y, or it would cancel out of the gap to the
                // next message entirely (cursor and line-y would both
                // shift by the same amount) — see
                // `self_message_taller_than_normal` in the test module and
                // task-5-report.md for the derivation.
                let base_row_h = MSG_MIN_H.max(text_h + 14.0);
                let row_h = base_row_h + if self_msg { SELF_EXTRA } else { 0.0 };
                let line_y = cursor + base_row_h - 6.0;
                let text_anchor = if self_msg {
                    let fx = col_x.get(*from).copied().unwrap_or(0.0);
                    (fx + ACT_W + SELF_STUB + 6.0, cursor + text_h / 2.0 + 6.0)
                } else {
                    let fx = col_x.get(*from).copied().unwrap_or(0.0);
                    let tx = col_x.get(*to).copied().unwrap_or(fx);
                    ((fx + tx) / 2.0, line_y - 6.0)
                };
                let number = autonum;
                if let Some(n) = autonum {
                    autonum = Some(n.saturating_add(autonum_step));
                }
                messages.push(MsgLayout { event: idx, y: line_y, text_anchor, number });

                if *activate_target {
                    if let Some(stack) = act_stacks.get_mut(*to) {
                        let depth = stack.len();
                        stack.push((depth, line_y));
                    }
                }
                if *deactivate_source {
                    if let Some(stack) = act_stacks.get_mut(*from) {
                        if let Some((depth, y0)) = stack.pop() {
                            activations.push(ActRect { p: *from, depth, y0, y1: line_y });
                        }
                    }
                }
                cursor += row_h + ROW_GAP;
            }
            Event::Note { placement, text } => {
                let (tw, th) = text_size(text);
                let note_w = tw + NOTE_PAD * 2.0;
                let note_h = th + NOTE_PAD * 2.0;
                let (x, w) = match placement {
                    NotePlacement::LeftOf(p) => {
                        let px = col_x.get(*p).copied().unwrap_or(0.0);
                        (px - ACT_W - note_w, note_w)
                    }
                    NotePlacement::RightOf(p) => {
                        let px = col_x.get(*p).copied().unwrap_or(0.0);
                        (px + ACT_W, note_w)
                    }
                    NotePlacement::Over(a, None) => {
                        let ax = col_x.get(*a).copied().unwrap_or(0.0);
                        (ax - note_w / 2.0, note_w)
                    }
                    NotePlacement::Over(a, Some(b)) => {
                        let (lo, hi) = if a < b { (*a, *b) } else { (*b, *a) };
                        let lo_x = col_x.get(lo).copied().unwrap_or(0.0);
                        let hi_x = col_x.get(hi).copied().unwrap_or(lo_x);
                        let lo_w = box_w.get(lo).copied().unwrap_or(0.0);
                        let hi_w = box_w.get(hi).copied().unwrap_or(0.0);
                        let left = lo_x - lo_w / 2.0 - NOTE_PAD;
                        let right = hi_x + hi_w / 2.0 + NOTE_PAD;
                        (left, (right - left).max(note_w))
                    }
                };
                notes.push(NoteLayout {
                    event: idx,
                    rect: crate::layout::Rect { x, y: cursor, w, h: note_h },
                });
                cursor += note_h + ROW_GAP;
            }
            Event::FragmentOpen { kind, label } => {
                let depth = frame_stack.len();
                frame_stack.push((*kind, label.clone(), cursor, depth, Vec::new()));
                cursor += FRAME_HEAD;
            }
            Event::FragmentDivider { label } => {
                if let Some(top) = frame_stack.last_mut() {
                    top.4.push((cursor + 4.0, label.clone()));
                }
                cursor += DIVIDER_H;
            }
            Event::FragmentClose => {
                if let Some((kind, label, top, depth, dividers)) = frame_stack.pop() {
                    let inset = depth as f64 * FRAME_INSET;
                    let rect = crate::layout::Rect {
                        x: PAD / 2.0 + inset,
                        y: top,
                        w: (canvas_w - PAD - 2.0 * inset).max(FRAME_MIN_W),
                        h: cursor + 6.0 - top,
                    };
                    cursor += FRAME_BOTTOM_PAD;
                    frames.push(FrameRect { kind, label, rect, depth, dividers });
                }
            }
            Event::Activate { p } => {
                if let Some(stack) = act_stacks.get_mut(*p) {
                    let depth = stack.len();
                    stack.push((depth, cursor));
                }
            }
            Event::Deactivate { p } => {
                if let Some(stack) = act_stacks.get_mut(*p) {
                    if let Some((depth, y0)) = stack.pop() {
                        activations.push(ActRect { p: *p, depth, y0, y1: cursor });
                    }
                }
            }
        }
    }

    let body_bottom = cursor + 6.0;
    // Force-close any activation spans still open (unbalanced
    // `activate`/`+` with no matching close) at the body bottom.
    for (p, stack) in act_stacks.iter_mut().enumerate() {
        for (depth, y0) in stack.drain(..) {
            activations.push(ActRect { p, depth, y0, y1: body_bottom });
        }
    }

    (messages, notes, activations, frames, body_bottom)
}

pub(crate) fn run(d: &SeqDiagram) -> SeqLayout {
    let (col_x, box_w, head_h, overhang_right) = pass1_columns(d);
    let width = match (col_x.last(), box_w.last()) {
        (Some(&last_x), Some(&last_w)) => last_x + last_w / 2.0 + overhang_right + PAD,
        _ => PAD * 2.0,
    };
    let body_top = PAD + head_h + 14.0;
    let (messages, notes, activations, frames, body_bottom) =
        pass2_rows(d, &col_x, &box_w, body_top, width);

    SeqLayout {
        col_x,
        box_w,
        head_h,
        body_top,
        body_bottom,
        messages,
        notes,
        activations,
        frames,
        size: (width, body_bottom + head_h + PAD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sequence::parse::parse;

    fn lay(src: &str) -> SeqLayout {
        run(&parse(src).expect("parse"))
    }

    #[test]
    fn columns_ordered_with_min_gap() {
        let l = lay("sequenceDiagram\nA->>B: x\nB->>C: y");
        assert!(l.col_x[0] < l.col_x[1] && l.col_x[1] < l.col_x[2]);
        assert!(l.col_x[1] - l.col_x[0] >= COL_GAP_MIN - 1e-6);
    }

    #[test]
    fn long_message_widens_its_pair() {
        let short = lay("sequenceDiagram\nA->>B: x");
        let long = lay(&format!("sequenceDiagram\nA->>B: {}", "wide ".repeat(20)));
        assert!(long.col_x[1] - long.col_x[0] > short.col_x[1] - short.col_x[0]);
    }

    #[test]
    fn spanning_note_widens() {
        let base = lay("sequenceDiagram\nA->>B: x");
        let noted = lay(&format!("sequenceDiagram\nA->>B: x\nNote over A,B: {}", "n".repeat(60)));
        assert!(noted.col_x[1] - noted.col_x[0] > base.col_x[1] - base.col_x[0]);
    }

    #[test]
    fn left_note_extends_left_margin() {
        let base = lay("sequenceDiagram\nA->>B: x");
        let noted = lay(&format!("sequenceDiagram\nA->>B: x\nNote left of A: {}", "n".repeat(40)));
        assert!(noted.col_x[0] > base.col_x[0]);
    }

    #[test]
    fn rows_monotonic_and_finite() {
        let l = lay("sequenceDiagram\nA->>B: one\nB-->>A: two\nNote over A: n\nA->>A: self");
        let mut prev = l.body_top;
        for m in &l.messages {
            assert!(m.y > prev - 1e-6, "monotone");
            assert!(m.y.is_finite());
            prev = m.y;
        }
        assert!(l.size.0.is_finite() && l.size.1.is_finite());
        assert!(l.body_bottom > l.body_top);
    }

    #[test]
    fn self_message_taller_than_normal() {
        let normal = lay("sequenceDiagram\nA->>B: x\nA->>B: y");
        let selfy = lay("sequenceDiagram\nA->>A: x\nA->>B: y");
        let dn = normal.messages[1].y - normal.messages[0].y;
        let ds = selfy.messages[1].y - selfy.messages[0].y;
        assert!(ds > dn);
    }

    #[test]
    fn activation_spans_wellformed_and_stacked() {
        let l = lay("sequenceDiagram\nA->>+B: a\nB->>+B: nest\nB-->>-B: unnest\nB-->>-A: done");
        assert_eq!(l.activations.len(), 2);
        for a in &l.activations {
            assert!(a.y1 > a.y0);
        }
        let depths: Vec<usize> = l.activations.iter().map(|a| a.depth).collect();
        assert!(depths.contains(&0) && depths.contains(&1));
    }

    #[test]
    fn unclosed_activation_force_closed_at_bottom() {
        let l = lay("sequenceDiagram\nA->>B: x\nactivate B");
        assert_eq!(l.activations.len(), 1);
        assert!((l.activations[0].y1 - l.body_bottom).abs() < 1e-6);
    }

    #[test]
    fn frames_contain_their_rows_and_nest() {
        let l = lay("sequenceDiagram\nloop outer\nA->>B: one\nalt inner\nB-->>A: two\nelse other\nA-xB: three\nend\nend");
        assert_eq!(l.frames.len(), 2);
        let (inner, outer) = {
            let a = &l.frames[0];
            let b = &l.frames[1];
            if a.depth > b.depth { (a, b) } else { (b, a) }
        };
        assert!(inner.rect.x > outer.rect.x);
        assert!(inner.rect.y > outer.rect.y);
        assert!(inner.rect.y + inner.rect.h < outer.rect.y + outer.rect.h + 1e-6);
        // every message row inside the outer frame's y-range
        for m in &l.messages {
            assert!(m.y > outer.rect.y && m.y < outer.rect.y + outer.rect.h);
        }
        assert_eq!(inner.dividers.len(), 1);
    }

    #[test]
    fn autonumber_numbers_messages_from_activation_point() {
        let l = lay("sequenceDiagram\nA->>B: zero\nautonumber\nA->>B: one\nB-->>A: two");
        assert_eq!(l.messages[0].number, None);
        assert_eq!(l.messages[1].number, Some(1));
        assert_eq!(l.messages[2].number, Some(2));
    }

    #[test]
    fn autonumber_start_step_and_off() {
        // start=10 step=5, then `off` stops numbering the last message.
        let l = lay("sequenceDiagram\nautonumber 10 5\nA->>B: a\nA->>B: b\nautonumber off\nA->>B: c");
        assert_eq!(l.messages[0].number, Some(10));
        assert_eq!(l.messages[1].number, Some(15));
        assert_eq!(l.messages[2].number, None);
    }

    #[test]
    fn actor_strip_taller_than_plain() {
        let plain = lay("sequenceDiagram\nA->>B: x");
        let actor = lay("sequenceDiagram\nactor A\nA->>B: x");
        assert!(actor.head_h > plain.head_h);
    }

    #[test]
    fn frame_width_floored_when_no_participants() {
        // No participants + deep fragment nesting drives
        // `canvas_w - PAD - 2*inset` non-positive; the width must be
        // floored at `FRAME_MIN_W` so frames stay visible AND the
        // `f.rect.w > 0.0` property in `sequence/props.rs` holds.
        let l = lay("sequenceDiagram\nloop a\nloop b\nloop c\nend\nend\nend");
        assert_eq!(l.frames.len(), 3);
        for f in &l.frames {
            assert!(f.rect.w >= FRAME_MIN_W, "frame width {} below floor", f.rect.w);
            assert!(f.rect.w.is_finite() && f.rect.h.is_finite());
        }
        assert!(l.size.0.is_finite() && l.size.1.is_finite());
    }

    #[test]
    fn deterministic() {
        let src = "sequenceDiagram\nloop l\nA->>+B: x\nNote over A,B: n\nB-->>-A: y\nend";
        let a = lay(src);
        let b = lay(src);
        assert_eq!(a.col_x, b.col_x);
        assert_eq!(a.size, b.size);
    }
}
