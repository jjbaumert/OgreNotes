// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Shared menu primitive — one implementation of dropdown/context-menu
//! chrome for every menu surface (menu bar, editor and spreadsheet
//! context menus, sheet tabs, account menu).
//!
//! Component layer lands in a follow-up commit; the pure navigation
//! model lives in the lib crate (`ogrenotes_frontend::menu_nav`) so
//! CI's `cargo test --lib` covers it.

pub use ogrenotes_frontend::menu_nav as core;
