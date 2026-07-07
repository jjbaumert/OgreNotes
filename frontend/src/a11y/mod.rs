// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Accessibility primitives (Phase 5 M-P8).
//!
//! Helpers consumed by every modal/dialog component to meet the
//! WCAG 2.1 AA focus-management bar.

pub mod focus_trap;

pub use focus_trap::{
    defer, defer_close, defer_close_then_run, handle_tab_trap, install_focus_trap,
    is_focus_restore_in_progress,
};
