// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P4 piece B — editor-scoped command dispatch bridge.
//!
//! The command palette is mounted globally (one SearchDialog per
//! page) but Editor- and Spreadsheet-scoped commands need to
//! dispatch into the editor's `on_command` callback that lives
//! at the document page level. This module is a thin thread-local
//! pipe: the document page installs its callback on mount; an
//! editor command's action reads from the pipe and dispatches.
//!
//! Why a global pipe rather than passing the callback down through
//! props: the palette registry stores actions as `Box<dyn Fn()>`
//! at module init — long before any page is mounted. Threading
//! a Signal of Callback through every registered action would
//! either require runtime re-registration on every page-mount
//! (fragile) or a per-page registry shadow (complex). The
//! thread-local stays single-tenant (WASM is single-threaded)
//! and the mount/unmount lifecycle keeps it honest.
//!
//! Mount semantics: the editor page calls `set_editor_cmd(Some(cb))`
//! on mount and `set_editor_cmd(None)` on unmount. Dispatch from
//! the palette while no editor is mounted is a silent no-op —
//! defends against the user opening Ctrl+Shift+P from the home
//! page and typing "bold" anyway. Scope filtering in
//! `matching(..., CommandScope::Home)` already hides editor
//! commands there; this is belt-and-braces for any future scope
//! flow that doesn't filter as tightly.

use std::cell::RefCell;

use leptos::prelude::Callback;
use leptos::prelude::Callable;

use crate::components::toolbar::ToolbarCommand;

thread_local! {
    static EDITOR_CMD: RefCell<Option<Callback<ToolbarCommand>>> = const {
        RefCell::new(None)
    };
}

/// Install (or clear) the editor's command callback. Called by the
/// document page in an Effect on mount, with the matching cleanup
/// passing `None` on `on_cleanup`. Replacing a prior install is
/// fine; the second editor wins.
pub fn set_editor_cmd(cb: Option<Callback<ToolbarCommand>>) {
    EDITOR_CMD.with(|cell| {
        *cell.borrow_mut() = cb;
    });
}

/// Dispatch a ToolbarCommand into the active editor. Silent no-op
/// when no editor is mounted. Editor-scoped palette commands wrap
/// their target ToolbarCommand in this call.
pub fn dispatch_editor(cmd: ToolbarCommand) {
    EDITOR_CMD.with(|cell| {
        if let Some(cb) = *cell.borrow() {
            cb.run(cmd);
        }
    });
}
