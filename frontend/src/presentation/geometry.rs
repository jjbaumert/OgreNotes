// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pure canvas-interaction math for the slide-deck editor (Task 10).
//!
//! Deliberately kept free of Leptos/`web_sys` — every function here
//! takes and returns plain values (`Rect`, `f64`, `&[DeckFrame]`) so
//! it's unit-testable without a reactive runtime or a DOM, and so
//! `deck_view.rs`'s pointer/keyboard handlers can stay thin wiring
//! around it: convert pixels to normalized deltas, call into here,
//! write the result back.
//!
//! `apply_drag` and `snap` are the two steps of one pointer-drag
//! gesture: `apply_drag` turns a raw pointer delta into a new
//! (clamped, always-valid) `Rect`; `snap` then nudges that `Rect`
//! onto nearby alignment lines (slide edges, slide center, other
//! frames' edges/centers) and reports which lines it snapped to, so
//! the caller can render them as guides. Both are meant to run on
//! every `pointermove` against a **transient** rect — never write the
//! deck model itself until the gesture ends (`pointerup`), or every
//! frame of a drag becomes its own yrs write.

use crate::presentation::model::{DeckFrame, Rect, MIN_FRAME_DIM};

/// Which corner of a frame a resize handle grabbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    Nw,
    Ne,
    Sw,
    Se,
}

/// What a pointer-drag gesture is doing to a frame's rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    Move,
    Resize(Corner),
}

/// Which axis a [`Guide`] line runs perpendicular to — `X` is a
/// vertical line at a given horizontal position, `Y` is a horizontal
/// line at a given vertical position (mirrors how `left`/`top` CSS
/// percentages are computed from `Rect.x`/`Rect.y`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

/// A snap alignment line to render while dragging, in the same
/// normalized 0..1 slide-fraction space as [`Rect`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    pub axis: Axis,
    pub at: f64,
}

/// Apply one pointer-drag delta (already converted from pixels to
/// normalized 0..1 slide fractions — divide by the canvas
/// `getBoundingClientRect()` size at the call site) to `rect`.
///
/// `dx`/`dy` are the *total* delta since the gesture started, not an
/// incremental step — callers should always re-apply from the rect
/// captured at `pointerdown`, never accumulate deltas across
/// `pointermove` events, or floating-point drift and intermediate
/// clamping would make the final rect depend on how many `pointermove`
/// events fired.
///
/// Always returns a valid rect: `Move` keeps the frame's size fixed
/// and clamps its position so it never crosses a slide edge;
/// `Resize` keeps the corner opposite the dragged one fixed in place
/// and never lets the frame collapse below [`MIN_FRAME_DIM`].
pub fn apply_drag(rect: Rect, kind: DragKind, dx: f64, dy: f64) -> Rect {
    match kind {
        DragKind::Move => apply_move(rect, dx, dy),
        DragKind::Resize(corner) => apply_resize(rect, corner, dx, dy),
    }
}

fn apply_move(rect: Rect, dx: f64, dy: f64) -> Rect {
    let max_x = (1.0 - rect.w).max(0.0);
    let max_y = (1.0 - rect.h).max(0.0);
    Rect {
        x: (rect.x + dx).clamp(0.0, max_x),
        y: (rect.y + dy).clamp(0.0, max_y),
        w: rect.w,
        h: rect.h,
    }
}

fn apply_resize(rect: Rect, corner: Corner, dx: f64, dy: f64) -> Rect {
    // The edge opposite the dragged corner stays fixed in place; the
    // dragged edge moves by the delta, clamped so the frame can
    // never shrink past `MIN_FRAME_DIM` or grow past the slide.
    let left_moves = matches!(corner, Corner::Nw | Corner::Sw);
    let top_moves = matches!(corner, Corner::Nw | Corner::Ne);

    let (x, w) = if left_moves {
        let right = rect.x + rect.w; // fixed
        let new_x = (rect.x + dx).clamp(0.0, right - MIN_FRAME_DIM);
        (new_x, right - new_x)
    } else {
        let left = rect.x; // fixed
        let max_w = (1.0 - left).max(MIN_FRAME_DIM);
        (left, (rect.w + dx).clamp(MIN_FRAME_DIM, max_w))
    };

    let (y, h) = if top_moves {
        let bottom = rect.y + rect.h; // fixed
        let new_y = (rect.y + dy).clamp(0.0, bottom - MIN_FRAME_DIM);
        (new_y, bottom - new_y)
    } else {
        let top = rect.y; // fixed
        let max_h = (1.0 - top).max(MIN_FRAME_DIM);
        (top, (rect.h + dy).clamp(MIN_FRAME_DIM, max_h))
    };

    Rect { x, y, w, h }
}

/// Nudge `rect` by a fixed keyboard-arrow step (`dx`/`dy` already
/// carry the sign and magnitude — 0.01 normally, 0.05 with Shift, per
/// the canvas keymap). Reuses [`apply_drag`]'s `Move` clamping so a
/// nudge can never push a frame off the slide either.
pub fn nudge(rect: Rect, dx: f64, dy: f64) -> Rect {
    apply_drag(rect, DragKind::Move, dx, dy)
}

/// Try to snap `rect` onto nearby alignment lines: the slide's own
/// edges (0.0/1.0) and center (0.5) on each axis, plus every rect in
/// `others`' edges and center. For each axis independently, the
/// candidate snap (rect's left edge, right edge, or center aligning
/// with a target line) with the smallest distance wins, provided
/// that distance is within `threshold`; otherwise the axis is left
/// untouched. Returns the (possibly axis-wise adjusted) rect and the
/// [`Guide`] line(s) it snapped to, if any — an empty `Vec` means no
/// snap happened on either axis.
pub fn snap(rect: Rect, others: &[Rect], threshold: f64) -> (Rect, Vec<Guide>) {
    let mut targets_x = vec![0.0, 0.5, 1.0];
    let mut targets_y = vec![0.0, 0.5, 1.0];
    for o in others {
        targets_x.push(o.x);
        targets_x.push(o.x + o.w);
        targets_x.push(o.x + o.w / 2.0);
        targets_y.push(o.y);
        targets_y.push(o.y + o.h);
        targets_y.push(o.y + o.h / 2.0);
    }

    let (new_x, guide_x) = snap_axis(rect.x, rect.w, &targets_x, threshold);
    let (new_y, guide_y) = snap_axis(rect.y, rect.h, &targets_y, threshold);

    let mut guides = Vec::new();
    if let Some(at) = guide_x {
        guides.push(Guide { axis: Axis::X, at });
    }
    if let Some(at) = guide_y {
        guides.push(Guide { axis: Axis::Y, at });
    }

    (Rect { x: new_x, y: new_y, w: rect.w, h: rect.h }, guides)
}

/// Snap one axis: try the rect's leading edge, trailing edge, and
/// center against every value in `targets`, keeping whichever
/// candidate has the smallest distance. Returns the (possibly
/// snapped) leading-edge coordinate and, if the best candidate was
/// within `threshold`, the guide line position.
fn snap_axis(min: f64, size: f64, targets: &[f64], threshold: f64) -> (f64, Option<f64>) {
    let max = min + size;
    let center = min + size / 2.0;

    // (abs_delta, candidate leading-edge coordinate, guide position)
    let mut best: Option<(f64, f64, f64)> = None;
    let mut consider = |delta: f64, new_min: f64, guide_at: f64| {
        if best.is_none_or(|(best_delta, _, _)| delta < best_delta) {
            best = Some((delta, new_min, guide_at));
        }
    };
    for &t in targets {
        consider((min - t).abs(), t, t); // leading edge -> t
        consider((max - t).abs(), t - size, t); // trailing edge -> t
        consider((center - t).abs(), t - size / 2.0, t); // center -> t
    }

    match best {
        Some((delta, new_min, guide_at)) if delta <= threshold => (new_min, Some(guide_at)),
        _ => (min, None),
    }
}

/// Sort `frames` by `z` ascending, ties broken by their original
/// position in `frames` (stable, so two frames sharing a `z` cycle in
/// the order they were created/appear in the doc). Shared by
/// `next_frame_id` and `previous_frame_id` so Tab and Shift-Tab walk
/// the exact same ring in opposite directions.
fn z_position_order(frames: &[DeckFrame]) -> Vec<&str> {
    let mut order: Vec<(i64, usize, &str)> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| (f.z, i, f.block_id.as_str()))
        .collect();
    order.sort_by_key(|&(z, i, _)| (z, i));
    order.into_iter().map(|(_, _, id)| id).collect()
}

/// The next frame to select when cycling with Tab, in z-then-position
/// order (see [`z_position_order`]).
///
/// `current = None` (nothing selected) starts the cycle at the first
/// frame. A `current` that names a frame no longer present (deleted
/// out from under a stale selection, e.g. a concurrent remote delete)
/// also restarts at the first frame rather than returning `None` —
/// Tab should always land on *something* as long as the slide has any
/// frames. Returns `None` only when `frames` is empty.
pub fn next_frame_id(frames: &[DeckFrame], current: Option<&str>) -> Option<String> {
    let order = z_position_order(frames);
    if order.is_empty() {
        return None;
    }
    let next_pos = match current.and_then(|cur| order.iter().position(|&id| id == cur)) {
        Some(pos) => (pos + 1) % order.len(),
        None => 0,
    };
    Some(order[next_pos].to_string())
}

/// Shift-Tab's counterpart to [`next_frame_id`] — walks the identical
/// z-then-position ring backwards. `current = None` or a stale
/// `current` both land on the *last* frame in the ring (the mirror of
/// `next_frame_id`'s "restart at the first frame"), so Tab and
/// Shift-Tab from an empty selection move in opposite directions from
/// opposite ends rather than both landing on the same frame.
pub fn previous_frame_id(frames: &[DeckFrame], current: Option<&str>) -> Option<String> {
    let order = z_position_order(frames);
    if order.is_empty() {
        return None;
    }
    let prev_pos = match current.and_then(|cur| order.iter().position(|&id| id == cur)) {
        Some(pos) => (pos + order.len() - 1) % order.len(),
        None => order.len() - 1,
    };
    Some(order[prev_pos].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::Fragment;
    use crate::presentation::model::FrameRole;

    fn frame(block_id: &str, z: i64) -> DeckFrame {
        DeckFrame {
            block_id: block_id.to_string(),
            rect: Rect::clamped(0.1, 0.1, 0.2, 0.2),
            z,
            role: FrameRole::Content,
            content: Fragment::empty(),
        }
    }

    /// Shuffled `z` values, including a tie (two frames share `z = 1`)
    /// to exercise the position tiebreak.
    fn fixture_frames() -> Vec<DeckFrame> {
        vec![frame("f-c", 3), frame("f-a", 1), frame("f-d", 5), frame("f-b", 1)]
    }

    #[test]
    fn drag_move_clamps_inside_slide() {
        let r = Rect::clamped(0.8, 0.8, 0.3, 0.3);
        let out = apply_drag(r, DragKind::Move, 0.5, 0.5);
        assert!(out.x + out.w <= 1.0 + 1e-9 && out.y + out.h <= 1.0 + 1e-9);
    }

    #[test]
    fn drag_move_negative_delta_clamps_to_zero() {
        let r = Rect::clamped(0.1, 0.1, 0.2, 0.2);
        let out = apply_drag(r, DragKind::Move, -5.0, -5.0);
        assert_eq!((out.x, out.y), (0.0, 0.0));
        assert_eq!((out.w, out.h), (0.2, 0.2), "move never changes size");
    }

    #[test]
    fn resize_never_collapses_below_min() {
        let r = Rect::clamped(0.1, 0.1, 0.3, 0.3);
        let out = apply_drag(r, DragKind::Resize(Corner::Se), -0.5, -0.5);
        assert!(out.w >= MIN_FRAME_DIM && out.h >= MIN_FRAME_DIM);
    }

    #[test]
    fn resize_se_keeps_top_left_fixed() {
        let r = Rect::clamped(0.2, 0.2, 0.3, 0.3);
        let out = apply_drag(r, DragKind::Resize(Corner::Se), 0.1, 0.05);
        assert_eq!((out.x, out.y), (0.2, 0.2));
        assert!((out.w - 0.4).abs() < 1e-9);
        assert!((out.h - 0.35).abs() < 1e-9);
    }

    #[test]
    fn resize_nw_moves_origin_and_keeps_bottom_right_fixed() {
        let r = Rect::clamped(0.3, 0.3, 0.3, 0.3); // corners: (0.3,0.3)..(0.6,0.6)
        let out = apply_drag(r, DragKind::Resize(Corner::Nw), 0.1, 0.1);
        assert!((out.x - 0.4).abs() < 1e-9);
        assert!((out.y - 0.4).abs() < 1e-9);
        assert!((out.x + out.w - 0.6).abs() < 1e-9, "bottom-right x fixed");
        assert!((out.y + out.h - 0.6).abs() < 1e-9, "bottom-right y fixed");
    }

    #[test]
    fn resize_never_pushes_past_far_edge() {
        // Se-resize growing far beyond the slide clamps w/h so x+w and
        // y+h never exceed 1.0.
        let r = Rect::clamped(0.7, 0.7, 0.2, 0.2);
        let out = apply_drag(r, DragKind::Resize(Corner::Se), 5.0, 5.0);
        assert!(out.x + out.w <= 1.0 + 1e-9);
        assert!(out.y + out.h <= 1.0 + 1e-9);
    }

    #[test]
    fn snap_attracts_to_slide_center_and_edges() {
        let r = Rect::clamped(0.496, 0.3, 0.2, 0.2);
        let (snapped, guides) = snap(Rect::clamped(0.492, 0.3, 0.2, 0.2), &[], 0.01);
        let (c, g) = snap(Rect::clamped(0.395, 0.3, 0.2, 0.2), &[], 0.01);
        assert!((c.x + c.w / 2.0 - 0.5).abs() < 1e-9);
        assert!(!g.is_empty());
        let _ = (r, snapped, guides);
    }

    #[test]
    fn snap_beyond_threshold_is_a_no_op() {
        let r = Rect::clamped(0.2, 0.2, 0.1, 0.1);
        let (out, guides) = snap(r, &[], 0.01);
        assert_eq!(out, r);
        assert!(guides.is_empty());
    }

    #[test]
    fn snap_attracts_to_other_frame_edges() {
        // Another frame's right edge sits at 0.5. Our rect's left edge
        // at 0.503 is within threshold of it.
        let other = Rect::clamped(0.2, 0.2, 0.3, 0.2); // right edge = 0.5
        let dragged = Rect::clamped(0.503, 0.2, 0.2, 0.2);
        let (out, guides) = snap(dragged, &[other], 0.01);
        assert!((out.x - 0.5).abs() < 1e-9);
        assert_eq!(guides[0].axis, Axis::X);
        assert!((guides[0].at - 0.5).abs() < 1e-9);
    }

    #[test]
    fn tab_cycles_frames_in_z_then_position_order() {
        let frames = fixture_frames();
        let first = next_frame_id(&frames, None).unwrap();
        let mut seen = vec![first.clone()];
        let mut cur = Some(first);
        for _ in 1..frames.len() {
            cur = next_frame_id(&frames, cur.as_deref());
            seen.push(cur.clone().unwrap());
        }
        assert_eq!(next_frame_id(&frames, cur.as_deref()), Some(seen[0].clone()), "wraps");
        let mut sorted = seen.clone();
        sorted.dedup();
        assert_eq!(sorted.len(), frames.len(), "visits every frame once");
    }

    #[test]
    fn next_frame_id_empty_is_none() {
        assert_eq!(next_frame_id(&[], None), None);
        assert_eq!(next_frame_id(&[], Some("anything")), None);
    }

    #[test]
    fn next_frame_id_stale_current_restarts_cycle() {
        let frames = fixture_frames();
        let first = next_frame_id(&frames, None).unwrap();
        assert_eq!(next_frame_id(&frames, Some("not-a-real-id")), Some(first));
    }

    #[test]
    fn previous_frame_id_is_next_frame_id_reversed() {
        let frames = fixture_frames();
        // Walking forward with `next_frame_id` and then immediately
        // back with `previous_frame_id` must land where we started —
        // they walk the identical ring in opposite directions.
        let start = next_frame_id(&frames, None).unwrap();
        let advanced = next_frame_id(&frames, Some(&start)).unwrap();
        assert_eq!(previous_frame_id(&frames, Some(&advanced)), Some(start));
    }

    #[test]
    fn previous_frame_id_empty_is_none() {
        assert_eq!(previous_frame_id(&[], None), None);
    }

    #[test]
    fn previous_frame_id_none_or_stale_lands_on_last() {
        let frames = fixture_frames();
        let last = previous_frame_id(&frames, None).unwrap();
        assert_eq!(previous_frame_id(&frames, Some("not-a-real-id")), Some(last));
    }

    #[test]
    fn nudge_matches_apply_drag_move() {
        let r = Rect::clamped(0.5, 0.5, 0.1, 0.1);
        assert_eq!(nudge(r, 0.01, -0.01), apply_drag(r, DragKind::Move, 0.01, -0.01));
    }
}
