// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #152 — client-side navigation bridge for the command palette.
//!
//! Mirror of `commands::ask_bridge`. Palette commands live in
//! the registry from boot time (`register_defaults` runs before mount), so
//! their action closures have no Router context and can't call
//! `use_navigate()` themselves. `AppShell` — which is always mounted while
//! the user is in-app and DOES have Router context — installs a navigate
//! callback here on mount; the palette's nav actions call [`go`].
//!
//! Falls back to a full-page load when no callback is installed (e.g. a
//! command somehow fires before the shell mounts), so navigation never
//! silently does nothing.

use std::cell::RefCell;

use leptos::prelude::Callable;
use leptos::prelude::Callback;

thread_local! {
    static NAVIGATE: RefCell<Option<Callback<String>>> = const {
        RefCell::new(None)
    };
}

/// Install (or clear) the client-side navigate callback. `AppShell` calls
/// this on mount and clears it (`None`) on cleanup.
pub fn set_navigate(cb: Option<Callback<String>>) {
    NAVIGATE.with(|cell| {
        *cell.borrow_mut() = cb;
    });
}

/// Navigate to `path` client-side via the installed callback. Falls back to
/// a full-page load when none is installed.
pub fn go(path: &str) {
    NAVIGATE.with(|cell| {
        if let Some(cb) = *cell.borrow() {
            cb.run(path.to_string());
        } else if let Some(window) = web_sys::window() {
            let _ = window.location().set_href(path);
        }
    });
}
