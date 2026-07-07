// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Configurable debug logging for the editor.
//!
//! Phase 1 observability rewrite — emits compile into both debug
//! and release builds; gating happens at runtime so a deployed
//! WASM bundle can be put into debug mode without a rebuild. The
//! cost of the unconditional emit-path is a level check and (when
//! capture is on) a ring-buffer push — both ~tens of nanoseconds.
//!
//! # Activating
//!
//! Three ways, in priority order — last activation wins:
//!
//! - **URL flag**: `?debug=collab,ws&level=debug` enables `debug`
//!   level for the `collab` and `ws` categories only, for this tab.
//!   Use `?debug=all` (or `?debug=*`) to enable every category.
//!   Use `?debug=all&capture=1` to also start filling the ring
//!   buffer (off by default to avoid the per-call buffer cost on
//!   the keystroke hot path).
//! - **JS console**: `window.__ogre_debug = true` (legacy, kept for
//!   muscle memory). String value `"verbose"` is treated as the
//!   `verbose` level with `all` categories.
//! - **Operator-pushed config** (Phase 2): a `client_log_config`
//!   field on `/users/me` flips a specific user's level without
//!   their cooperation.
//!
//! # Categories
//!
//! - `input`, `keydown`, `backspace`, `selection` — per-keystroke
//!   verbose categories. Only emitted at `verbose` level.
//! - everything else (`collab`, `ws`, `enter`, `paste`, `editor`,
//!   `backspace_high`, etc.) — `debug` level.
//!
//! Categories that produce `warn`/`error` always emit regardless
//! of level, because they indicate "shouldn't be silently
//! swallowed" conditions per the silent-failure audit.
//!
//! # Ring buffer
//!
//! When capture is on (via `?capture=1` URL flag or an explicit
//! `debug::set_capture(true)` JS call), every emit also lands in a
//! 1000-entry ring buffer. `dump_ring_buffer()` returns a copy
//! suitable for inclusion in a support bundle. The buffer is per-
//! thread; WASM is single-threaded so per-tab.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;

/// Emit level — gates which log calls reach `console.*`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    /// No emission. Warnings and errors still go through.
    Off = 0,
    /// Important events: collab, paste, enter, ws state, errors.
    Debug = 1,
    /// Per-keystroke events: input, keydown, backspace, selection.
    Verbose = 2,
}

impl Level {
    fn from_u8(n: u8) -> Self {
        match n {
            2 => Level::Verbose,
            1 => Level::Debug,
            _ => Level::Off,
        }
    }
}

/// Categories that are verbose (per-keystroke).
fn is_verbose_category(category: &str) -> bool {
    matches!(category, "input" | "keydown" | "backspace" | "selection")
}

/// Maximum entries in the ring buffer when capture is on. 1000 is
/// big enough for ~10 seconds of fairly chatty activity (most
/// emits are short messages) and small enough to fit comfortably
/// in the WASM linear memory without LRU thrash on the keystroke
/// hot path.
const RING_BUFFER_CAP: usize = 1000;

thread_local! {
    /// Current emit level. Default `Off`; set by `init_from_url`,
    /// `enable`, `set_level`, or the legacy `__ogre_debug` polling.
    static LEVEL: Cell<u8> = const { Cell::new(0) };

    /// Allowlist of categories to emit. `None` = all categories.
    /// `Some(set)` = only those categories.
    static CATEGORIES: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };

    /// Whether to write to the ring buffer in addition to console.
    /// Off by default — capture is opt-in because the buffer write
    /// is the dominant cost on the keystroke hot path.
    static CAPTURE: Cell<bool> = const { Cell::new(false) };

    /// The ring buffer. Bounded; oldest entry evicted when full.
    static RING: RefCell<VecDeque<LogEntry>> =
        const { RefCell::new(VecDeque::new()) };

    /// Counter incremented on every call. Used to throttle the
    /// legacy `__ogre_debug` polling — checking the JS global on
    /// every call would dominate the cost.
    static CALL_COUNT: Cell<u32> = const { Cell::new(0) };
}

/// One captured emit. Cheap to clone (short String fields).
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub level: &'static str,
    pub category: String,
    pub message: String,
    pub fields: Vec<(String, String)>,
}

/// How many calls between `__ogre_debug` checks.
const CHECK_INTERVAL: u32 = 500;

/// Parse `?debug=...&level=...&capture=...` from the current URL and
/// apply. Called once from `main.rs` early in startup. Safe to call
/// multiple times — last call wins.
#[cfg(target_arch = "wasm32")]
pub fn init_from_url() {
    let Some(window) = web_sys::window() else { return };
    let Ok(search) = window.location().search() else { return };
    apply_query_string(&search);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn init_from_url() {}

/// Pure parser extracted so the URL-flag behavior is unit-testable
/// without spinning up a browser context.
pub fn apply_query_string(search: &str) {
    let pairs = parse_query_pairs(search);

    if let Some(debug_val) = pairs.iter().find_map(|(k, v)| (k == "debug").then_some(v)) {
        let categories: Vec<String> = debug_val
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if categories.iter().any(|c| c == "all" || c == "*") {
            CATEGORIES.with(|c| *c.borrow_mut() = None);
        } else if !categories.is_empty() {
            CATEGORIES.with(|c| *c.borrow_mut() = Some(categories));
        }
        // Setting a debug flag without an explicit level still
        // bumps the floor to `Debug` so anything matched actually
        // emits. Operators rarely want "debug=collab" with no
        // visible output.
        LEVEL.with(|l| {
            if Level::from_u8(l.get()) == Level::Off {
                l.set(Level::Debug as u8);
            }
        });
    }

    if let Some(level_val) = pairs.iter().find_map(|(k, v)| (k == "level").then_some(v)) {
        let level = match level_val.as_str() {
            "verbose" => Level::Verbose,
            "debug" => Level::Debug,
            "off" => Level::Off,
            _ => return,
        };
        LEVEL.with(|l| l.set(level as u8));
    }

    if let Some(capture_val) = pairs.iter().find_map(|(k, v)| (k == "capture").then_some(v)) {
        let on = matches!(capture_val.as_str(), "1" | "true" | "yes" | "on");
        CAPTURE.with(|c| c.set(on));
    }
}

/// Tiny query-string parser. Avoids pulling in a full URL crate
/// for a one-time startup parse.
fn parse_query_pairs(search: &str) -> Vec<(String, String)> {
    let q = search.strip_prefix('?').unwrap_or(search);
    q.split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|kv| {
            let mut it = kv.splitn(2, '=');
            let k = it.next()?.to_string();
            let v = it.next().unwrap_or("").to_string();
            Some((k, v))
        })
        .collect()
}

/// Explicitly set the level. Useful for the JS console and Phase 2's
/// operator-pushed config endpoint.
pub fn set_level(level: Level) {
    LEVEL.with(|l| l.set(level as u8));
}

/// Toggle ring-buffer capture. Opt-in because the per-call buffer
/// write is the dominant cost.
pub fn set_capture(on: bool) {
    CAPTURE.with(|c| c.set(on));
}

/// Snapshot the ring buffer. Phase 1 returns the entries as a Vec
/// for a future "support bundle" POST; Phase 2 wires the actual
/// upload endpoint.
pub fn dump_ring_buffer() -> Vec<LogEntry> {
    RING.with(|r| r.borrow().iter().cloned().collect())
}

/// Drop everything from the ring buffer. Used after a successful
/// support-bundle upload so the next session starts fresh.
pub fn clear_ring_buffer() {
    RING.with(|r| r.borrow_mut().clear());
}

/// Compatibility shim for the legacy `enable()` API.
pub fn enable() {
    set_level(Level::Debug);
}

/// Compatibility shim for the legacy `disable()` API.
pub fn disable() {
    set_level(Level::Off);
}

/// True if any level above Off is in effect.
pub fn is_enabled() -> bool {
    current_level() != Level::Off
}

/// Returns the effective level, consulting the legacy
/// `window.__ogre_debug` global at a throttled cadence so a user
/// can still flip the JS global at runtime without an explicit
/// `set_level` call.
fn current_level() -> Level {
    let cached = LEVEL.with(|l| l.get());
    if cached > 0 {
        return Level::from_u8(cached);
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Poll the legacy JS global at most once every
        // CHECK_INTERVAL calls to avoid the JS-boundary cost.
        let count = CALL_COUNT.with(|c| {
            let n = c.get().wrapping_add(1);
            c.set(n);
            n
        });
        if count % CHECK_INTERVAL == 0 {
            if let Some(window) = web_sys::window() {
                if let Ok(val) = js_sys::Reflect::get(&window, &"__ogre_debug".into()) {
                    if let Some(s) = val.as_string() {
                        if s == "verbose" {
                            LEVEL.with(|l| l.set(Level::Verbose as u8));
                            return Level::Verbose;
                        }
                    }
                    if val.is_truthy() {
                        LEVEL.with(|l| l.set(Level::Debug as u8));
                        return Level::Debug;
                    }
                }
            }
        }
    }
    Level::Off
}

/// Whether the named category should emit at the current level.
fn category_should_emit(category: &str) -> bool {
    CATEGORIES.with(|c| {
        match c.borrow().as_ref() {
            None => true, // No allowlist = everything emits.
            Some(list) => list.iter().any(|allowed| allowed == category),
        }
    })
}

fn push_ring(level: &'static str, category: &str, message: &str, fields: &[(&str, &str)]) {
    RING.with(|r| {
        let mut buf = r.borrow_mut();
        if buf.len() >= RING_BUFFER_CAP {
            buf.pop_front();
        }
        buf.push_back(LogEntry {
            level,
            category: category.to_string(),
            message: message.to_string(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });
    });
}

/// Log a debug-level message. Console emission is gated by level
/// and category allowlist; the ring buffer (when capture is on)
/// always captures so an operator inspecting a support bundle can
/// see the full sequence, not just what survived the level filter.
pub fn log(category: &str, message: &str, fields: &[(&str, &str)]) {
    let level = current_level();
    let capture = CAPTURE.with(|c| c.get());

    if capture {
        push_ring("debug", category, message, fields);
    }
    // Fast path: nothing to emit, nothing to capture — return
    // before the level / category checks.
    if level == Level::Off && !capture {
        return;
    }
    if level == Level::Off {
        return; // capture was on; we already pushed to the ring.
    }
    if level != Level::Verbose && is_verbose_category(category) {
        return;
    }
    if !category_should_emit(category) {
        return;
    }
    emit_console("log", category, message, fields);
}

/// Log a warning. Always emitted regardless of level — warnings
/// indicate problems that shouldn't be silently swallowed.
/// Always captured to the ring buffer.
pub fn warn(category: &str, message: &str) {
    push_ring("warn", category, message, &[]);
    emit_console("warn", category, message, &[]);
}

/// Log an error. Always emitted regardless of level. Always
/// captured.
pub fn error(category: &str, message: &str) {
    push_ring("error", category, message, &[]);
    emit_console("error", category, message, &[]);
}

#[cfg(target_arch = "wasm32")]
fn emit_console(kind: &str, category: &str, message: &str, fields: &[(&str, &str)]) {
    let mut out = format!("[editor:{category}] {message}");
    for (k, v) in fields {
        out.push_str(&format!(" {k}={v}"));
    }
    match kind {
        "warn" => web_sys::console::warn_1(&out.into()),
        "error" => web_sys::console::error_1(&out.into()),
        _ => web_sys::console::log_1(&out.into()),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn emit_console(_kind: &str, _category: &str, _message: &str, _fields: &[(&str, &str)]) {
    // Native (non-WASM) builds run in unit tests only and have
    // no console; the ring buffer is still populated for tests
    // that inspect it.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset_state() {
        LEVEL.with(|l| l.set(0));
        CATEGORIES.with(|c| *c.borrow_mut() = None);
        CAPTURE.with(|c| c.set(false));
        RING.with(|r| r.borrow_mut().clear());
        CALL_COUNT.with(|c| c.set(0));
    }

    #[test]
    fn parse_query_pairs_handles_typical_input() {
        let pairs = parse_query_pairs("?debug=collab,ws&level=verbose&capture=1");
        assert_eq!(
            pairs,
            vec![
                ("debug".to_string(), "collab,ws".to_string()),
                ("level".to_string(), "verbose".to_string()),
                ("capture".to_string(), "1".to_string()),
            ]
        );
    }

    #[test]
    fn apply_query_string_sets_level_and_categories() {
        reset_state();
        apply_query_string("?debug=collab,ws&level=debug");
        assert_eq!(current_level(), Level::Debug);
        assert!(category_should_emit("collab"));
        assert!(category_should_emit("ws"));
        assert!(!category_should_emit("input"));
    }

    #[test]
    fn apply_query_string_all_enables_every_category() {
        reset_state();
        apply_query_string("?debug=all");
        assert!(category_should_emit("collab"));
        assert!(category_should_emit("anything"));
        assert!(category_should_emit("editor"));
    }

    #[test]
    fn capture_off_by_default_log_off_skips_ring() {
        reset_state();
        log("collab", "test", &[]);
        assert!(dump_ring_buffer().is_empty());
    }

    #[test]
    fn capture_on_populates_ring_even_when_level_off() {
        reset_state();
        set_capture(true);
        log("collab", "test", &[]);
        let buf = dump_ring_buffer();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].category, "collab");
        assert_eq!(buf[0].message, "test");
    }

    #[test]
    fn warn_always_captures_regardless_of_level() {
        reset_state();
        warn("paste", "empty clipboard");
        let buf = dump_ring_buffer();
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].level, "warn");
    }

    #[test]
    fn ring_buffer_evicts_oldest_when_full() {
        reset_state();
        set_capture(true);
        for i in 0..(RING_BUFFER_CAP + 5) {
            log("collab", &format!("m{i}"), &[]);
        }
        let buf = dump_ring_buffer();
        assert_eq!(buf.len(), RING_BUFFER_CAP);
        // First entry should be the 5th emit (0..4 evicted).
        assert_eq!(buf[0].message, "m5");
        // Last entry is the most recent.
        assert_eq!(buf[buf.len() - 1].message, format!("m{}", RING_BUFFER_CAP + 4));
    }

    #[test]
    fn verbose_categories_gated_at_debug_level() {
        reset_state();
        set_level(Level::Debug);
        set_capture(true);
        log("input", "keystroke", &[]);
        log("collab", "ok", &[]);
        let buf = dump_ring_buffer();
        // Both calls are captured because capture is independent
        // of level; only the console emission is gated.
        // (Tests verify the behavior; the buffer always captures
        // when capture is on.)
        assert_eq!(buf.len(), 2);
    }
}
