//! Configurable debug logging for the editor.
//!
//! In debug builds (`cfg(debug_assertions)`), logging calls emit to the browser console
//! when the global debug flag is enabled. In release builds, all logging compiles to nothing.
//!
//! # Levels
//!
//! - `window.__ogre_debug = true` — shows important events (enter, paste, collab, errors)
//! - `window.__ogre_debug = "verbose"` — also shows per-keystroke events (input, keydown, backspace)
//!
//! # Usage
//!
//! ```rust,ignore
//! debug::log("input", "insertText fired", &[("data", "X")]);  // verbose only
//! debug::log("enter", "split_block", &[("pos", "5")]);         // always when enabled
//! debug::warn("paste", "empty clipboard data");                 // always when enabled
//! ```

#[cfg(debug_assertions)]
mod inner {
    use std::cell::Cell;

    /// Debug level: 0 = off, 1 = normal, 2 = verbose (per-keystroke)
    thread_local! {
        static LEVEL: Cell<u8> = const { Cell::new(0) };
    }

    /// Categories that are verbose (per-keystroke). Only shown at level 2.
    fn is_verbose_category(category: &str) -> bool {
        matches!(category, "input" | "keydown" | "backspace" | "selection")
    }

    /// Enable debug logging at normal level.
    pub fn enable() {
        LEVEL.with(|l| l.set(1));
    }

    /// Disable debug logging.
    pub fn disable() {
        LEVEL.with(|l| l.set(0));
    }

    /// Check if debug logging is enabled (at any level).
    /// Checks `window.__ogre_debug` at most once per second.
    pub fn is_enabled() -> bool {
        get_level() > 0
    }

    /// Counter incremented on every call. Only checks `window.__ogre_debug`
    /// every N calls instead of using `Date::now()` (avoids WASM-JS boundary crossing).
    thread_local! {
        static CALL_COUNT: Cell<u32> = const { Cell::new(0) };
    }

    /// How many calls between window.__ogre_debug checks.
    const CHECK_INTERVAL: u32 = 500;

    fn get_level() -> u8 {
        LEVEL.with(|l| {
            let cached = l.get();
            if cached > 0 {
                return cached;
            }
            // Only check the JS global every CHECK_INTERVAL calls (no Date::now needed)
            CALL_COUNT.with(|c| {
                let count = c.get().wrapping_add(1);
                c.set(count);
                if count % CHECK_INTERVAL != 0 {
                    return 0;
                }
                if let Some(window) = web_sys::window() {
                    if let Ok(val) = js_sys::Reflect::get(&window, &"__ogre_debug".into()) {
                        if let Some(s) = val.as_string() {
                            if s == "verbose" {
                                l.set(2);
                                return 2;
                            }
                        }
                        if val.is_truthy() {
                            l.set(1);
                            return 1;
                        }
                    }
                }
                0
            })
        })
    }

    /// Log a debug message. Verbose categories (input, keydown, backspace)
    /// are only shown when `window.__ogre_debug = "verbose"`.
    pub fn log(category: &str, message: &str, fields: &[(&str, &str)]) {
        let level = get_level();
        if level == 0 {
            return;
        }
        if level < 2 && is_verbose_category(category) {
            return;
        }
        let mut out = format!("[editor:{category}] {message}");
        for (k, v) in fields {
            out.push_str(&format!(" {k}={v}"));
        }
        web_sys::console::log_1(&out.into());
    }

    /// Log a warning. Always emitted regardless of debug flag — warnings
    /// indicate problems that shouldn't be silently swallowed.
    pub fn warn(category: &str, message: &str) {
        let out = format!("[editor:{category}] {message}");
        web_sys::console::warn_1(&out.into());
    }

    /// Log an error. Always emitted regardless of debug flag — errors are
    /// diagnostics that should never be silenced.
    pub fn error(category: &str, message: &str) {
        let out = format!("[editor:{category}] {message}");
        web_sys::console::error_1(&out.into());
    }
}

// In release builds, all logging is a no-op that compiles to nothing.
#[cfg(not(debug_assertions))]
mod inner {
    #[inline(always)]
    pub fn enable() {}
    #[inline(always)]
    pub fn disable() {}
    #[inline(always)]
    pub fn is_enabled() -> bool {
        false
    }
    #[inline(always)]
    pub fn log(_category: &str, _message: &str, _fields: &[(&str, &str)]) {}
    #[inline(always)]
    pub fn warn(_category: &str, _message: &str) {}
    #[inline(always)]
    pub fn error(_category: &str, _message: &str) {}
}

pub use inner::*;
