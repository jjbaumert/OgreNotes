//! Configurable debug logging for the editor.
//!
//! In debug builds (`cfg(debug_assertions)`), logging calls emit to the browser console
//! when the global debug flag is enabled. In release builds, all logging compiles to nothing.
//!
//! # Usage
//!
//! ```rust,ignore
//! use super::debug;
//!
//! debug::enable();  // Turn on logging (e.g., from a dev tools call)
//! debug::log("input", "insertText fired", &[("data", "X"), ("pos", "5")]);
//! debug::warn("paste", "empty clipboard data");
//! ```
//!
//! To enable from the browser console: `window.__ogre_debug = true`

#[cfg(debug_assertions)]
mod inner {
    use std::cell::Cell;

    thread_local! {
        static ENABLED: Cell<bool> = const { Cell::new(false) };
        /// Timestamp of last window.__ogre_debug check (ms since epoch).
        static LAST_CHECK: Cell<f64> = const { Cell::new(0.0) };
    }

    /// Enable debug logging.
    pub fn enable() {
        ENABLED.with(|e| e.set(true));
    }

    /// Disable debug logging.
    pub fn disable() {
        ENABLED.with(|e| e.set(false));
    }

    /// Check if debug logging is enabled.
    /// Checks `window.__ogre_debug` at most once per second to avoid
    /// expensive WASM-JS boundary crossings on every call.
    pub fn is_enabled() -> bool {
        ENABLED.with(|e| {
            if e.get() {
                return true;
            }
            // Throttled check of window.__ogre_debug (once per second)
            LAST_CHECK.with(|lc| {
                let now = js_sys::Date::now();
                if now - lc.get() > 1000.0 {
                    lc.set(now);
                    if let Some(window) = web_sys::window() {
                        if let Ok(val) = js_sys::Reflect::get(&window, &"__ogre_debug".into()) {
                            if val.is_truthy() {
                                e.set(true); // Cache it so subsequent calls are fast
                                return true;
                            }
                        }
                    }
                }
                false
            })
        })
    }

    /// Log a debug message to the browser console.
    /// `category` groups related messages (e.g., "input", "paste", "selection").
    /// `message` is the main log line.
    /// `fields` are key-value pairs for structured data.
    pub fn log(category: &str, message: &str, fields: &[(&str, &str)]) {
        if !is_enabled() {
            return;
        }
        let mut out = format!("[editor:{category}] {message}");
        for (k, v) in fields {
            out.push_str(&format!(" {k}={v}"));
        }
        web_sys::console::log_1(&out.into());
    }

    /// Log a warning to the browser console.
    pub fn warn(category: &str, message: &str) {
        if !is_enabled() {
            return;
        }
        let out = format!("[editor:{category}] {message}");
        web_sys::console::warn_1(&out.into());
    }

    /// Log an error to the browser console (always emitted when debug is enabled).
    pub fn error(category: &str, message: &str) {
        if !is_enabled() {
            return;
        }
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
