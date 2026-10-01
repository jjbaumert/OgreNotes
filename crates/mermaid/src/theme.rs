// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Text and outlines on the renderer's fixed, light data palettes. These
//! foregrounds stay dark in both document themes, unlike canvas text.

/// Keep fitted labels consistent between browser and standalone viewers.
pub(crate) const LABEL_FONT: &str = "Arial, Helvetica, sans-serif";

pub(crate) const DATA_TEXT: &str = "var(--mermaid-data-text, #1a1a1a)";
pub(crate) const DATA_MUTED_TEXT: &str = "var(--mermaid-data-text-muted, #444)";
pub(crate) const DATA_STROKE: &str = "var(--mermaid-data-stroke, #444)";

/// Stable IDs keep different mask geometry separate across embedded SVGs.
pub(crate) fn paint_mask_id(prefix: &str, geometry: &str) -> String {
    let hash = geometry.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    format!("mmd-{prefix}-{hash:016x}")
}
