// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Touch gesture primitives for mobile interactions.
//!
//! Provides pure helpers (`first_touch_xy`, `swipe_direction`) and a stateful
//! `LongPressTracker` that components feed via their own touch event handlers.
//! Mouse-equivalent behavior continues to live in the components themselves —
//! this module only adds the touch path.

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

/// Default delay before a sustained touch counts as a long-press (ms).
pub const LONG_PRESS_MS: i32 = 500;
/// Default movement tolerance — exceeding this px in any direction during a
/// long-press cancels the gesture.
pub const TOUCH_MOVE_THRESHOLD_PX: f64 = 10.0;
/// Default minimum delta for a swipe to register.
pub const SWIPE_THRESHOLD_PX: f64 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwipeDir {
    Left,
    Right,
    Up,
    Down,
}

/// Coordinates of the first touch point, or None for an empty TouchList.
pub fn first_touch_xy(ev: &web_sys::TouchEvent) -> Option<(f64, f64)> {
    let touches = ev.touches();
    if touches.length() == 0 {
        return None;
    }
    let t = touches.item(0)?;
    Some((t.client_x() as f64, t.client_y() as f64))
}

/// Compute swipe direction from start→end coordinates. Returns None if neither
/// axis crosses `threshold_px`. Dominant axis wins on diagonals.
pub fn swipe_direction(
    start_x: f64,
    start_y: f64,
    end_x: f64,
    end_y: f64,
    threshold_px: f64,
) -> Option<SwipeDir> {
    let dx = end_x - start_x;
    let dy = end_y - start_y;
    if dx.abs() > dy.abs() {
        if dx.abs() < threshold_px {
            return None;
        }
        Some(if dx > 0.0 { SwipeDir::Right } else { SwipeDir::Left })
    } else {
        if dy.abs() < threshold_px {
            return None;
        }
        Some(if dy > 0.0 { SwipeDir::Down } else { SwipeDir::Up })
    }
}

struct TrackingState {
    start_x: f64,
    start_y: f64,
    timer_id: i32,
    // The closure must outlive the timer; held here so it isn't dropped early.
    _timer_closure: Closure<dyn FnMut()>,
}

/// Tracks a single long-press gesture across touchstart → touchmove → touchend.
/// Cheap to construct; the same tracker can be reused across multiple touches
/// (only one in flight at a time — multitouch resets state).
pub struct LongPressTracker {
    duration_ms: i32,
    move_threshold_px: f64,
    cb: Rc<RefCell<Box<dyn FnMut()>>>,
    state: Rc<RefCell<Option<TrackingState>>>,
}

impl LongPressTracker {
    pub fn new<F: FnMut() + 'static>(duration_ms: i32, move_threshold_px: f64, cb: F) -> Rc<Self> {
        Rc::new(Self {
            duration_ms,
            move_threshold_px,
            cb: Rc::new(RefCell::new(Box::new(cb))),
            state: Rc::new(RefCell::new(None)),
        })
    }

    /// Call from `touchstart`. Schedules the long-press timer.
    pub fn on_start(self: &Rc<Self>, ev: &web_sys::TouchEvent) {
        self.cancel();
        let Some((x, y)) = first_touch_xy(ev) else {
            return;
        };

        let cb = Rc::clone(&self.cb);
        let state_cell = Rc::clone(&self.state);
        let timer_closure = Closure::<dyn FnMut()>::wrap(Box::new(move || {
            // Clear our own state slot so a stale on_end can't double-cancel.
            *state_cell.borrow_mut() = None;
            (cb.borrow_mut())();
        }));

        let timer_id = web_sys::window()
            .and_then(|w| {
                w.set_timeout_with_callback_and_timeout_and_arguments_0(
                    timer_closure.as_ref().unchecked_ref(),
                    self.duration_ms,
                )
                .ok()
            })
            .unwrap_or(-1);

        *self.state.borrow_mut() = Some(TrackingState {
            start_x: x,
            start_y: y,
            timer_id,
            _timer_closure: timer_closure,
        });
    }

    /// Call from `touchmove`. Cancels the gesture if movement exceeds the
    /// configured threshold.
    pub fn on_move(self: &Rc<Self>, ev: &web_sys::TouchEvent) {
        let Some((x, y)) = first_touch_xy(ev) else {
            return;
        };
        let cancel = self.state.borrow().as_ref().is_some_and(|s| {
            (x - s.start_x).abs() > self.move_threshold_px
                || (y - s.start_y).abs() > self.move_threshold_px
        });
        if cancel {
            self.cancel();
        }
    }

    /// Call from `touchend` / `touchcancel`. Cancels any pending timer.
    pub fn on_end(self: &Rc<Self>) {
        self.cancel();
    }

    /// Cancel any in-flight tracking and clear the timer.
    fn cancel(self: &Rc<Self>) {
        if let Some(state) = self.state.borrow_mut().take() {
            if state.timer_id >= 0 {
                if let Some(w) = web_sys::window() {
                    w.clear_timeout_with_handle(state.timer_id);
                }
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swipe_right_when_dx_dominant_positive() {
        let dir = swipe_direction(0.0, 0.0, 80.0, 10.0, 50.0);
        assert_eq!(dir, Some(SwipeDir::Right));
    }

    #[test]
    fn swipe_left_when_dx_dominant_negative() {
        let dir = swipe_direction(100.0, 0.0, 0.0, 5.0, 50.0);
        assert_eq!(dir, Some(SwipeDir::Left));
    }

    #[test]
    fn swipe_down_when_dy_dominant_positive() {
        let dir = swipe_direction(0.0, 0.0, 5.0, 80.0, 50.0);
        assert_eq!(dir, Some(SwipeDir::Down));
    }

    #[test]
    fn swipe_up_when_dy_dominant_negative() {
        let dir = swipe_direction(0.0, 100.0, 5.0, 0.0, 50.0);
        assert_eq!(dir, Some(SwipeDir::Up));
    }

    #[test]
    fn no_swipe_when_below_threshold_on_dominant_axis() {
        // dx dominant but below threshold
        assert_eq!(swipe_direction(0.0, 0.0, 30.0, 5.0, 50.0), None);
        // dy dominant but below threshold
        assert_eq!(swipe_direction(0.0, 0.0, 5.0, 30.0, 50.0), None);
    }

    #[test]
    fn diagonal_picks_dominant_axis() {
        // dx slightly larger than dy, both above threshold
        let dir = swipe_direction(0.0, 0.0, 80.0, 70.0, 50.0);
        assert_eq!(dir, Some(SwipeDir::Right));
        let dir = swipe_direction(0.0, 0.0, 70.0, 80.0, 50.0);
        assert_eq!(dir, Some(SwipeDir::Down));
    }
}
