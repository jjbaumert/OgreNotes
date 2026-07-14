// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

// `a11y` is referenced from `editor::view` for the focus-restore
// flag — has to be visible to both the wasm-pack lib target (which
// compiles `editor`) and the binary target.
pub mod a11y;
pub mod collab;
pub mod editor;
// #137 Phase 4d — i18n must be lib-visible so editor block
// renderers (blocks/kanban.rs, blocks/calendar.rs, and any
// future live-app widget) can call translate() without
// hardcoding English. The `t!` macro's `$crate::i18n::translate`
// expansion resolves against whichever crate the caller sits
// in, so it needs `i18n` at the lib crate root too.
pub mod i18n;
// Pure keyboard-navigation model for the shared menu primitive
// (components/menu in the binary). Lib-visible so `cargo test --lib`
// (the CI tier-1 command) runs its unit tests.
pub mod menu_nav;
pub mod observability;
pub mod touch;
