// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

pub mod blob_ref;
pub mod blocks;
pub mod clipboard;
pub mod debug;
pub mod commands;
pub mod find;
pub mod find_highlight;
pub mod image_bridge;
pub mod input_rules;
pub mod keymap;
pub mod markdown;
pub mod mention_url;
pub mod model;
pub mod plugins;
pub mod position;
pub mod schema;
pub mod selection;
pub mod state;
pub mod transform;
pub mod view;
pub mod yrs_bridge;

// Native-only: proptest doesn't build for wasm32 (see frontend/Cargo.toml).
#[cfg(all(test, not(target_arch = "wasm32")))]
mod structural_props;
