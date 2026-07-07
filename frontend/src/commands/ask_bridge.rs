// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.2 piece C — ask-dialog open bridge.
//!
//! Mirror of `editor_bridge` for the Ask dialog. The Global-scoped
//! `global.ask` palette command lives in the registry at boot time;
//! it can't reach into a per-page `ask_visible` signal directly, so
//! each page that mounts AskDialog installs a callback here on
//! mount and clears it on unmount. The palette's action closure
//! calls `open()` to flip the dialog open.
//!
//! Silent no-op when no page has installed a callback — same
//! defensive shape as editor_bridge. Today the only pages mounting
//! AskDialog are home + document; admin/MFA/auth-complete don't
//! install a callback so a Global Ask command run from those pages
//! does nothing (and the scope filter hides the command there
//! anyway via the editor_bridge's existing matching pattern).

use std::cell::RefCell;

use leptos::prelude::Callable;
use leptos::prelude::Callback;

thread_local! {
    static ASK_OPEN: RefCell<Option<Callback<()>>> = const {
        RefCell::new(None)
    };
}

/// Install (or clear) the page's "open ask dialog" callback.
/// Called by the page in an Effect on mount; cleared via
/// `set_ask_open(None)` on cleanup.
pub fn set_ask_open(cb: Option<Callback<()>>) {
    ASK_OPEN.with(|cell| {
        *cell.borrow_mut() = cb;
    });
}

/// Flip the Ask dialog open. Silent no-op when no page has
/// installed a callback.
pub fn open() {
    ASK_OPEN.with(|cell| {
        if let Some(cb) = *cell.borrow() {
            cb.run(());
        }
    });
}
