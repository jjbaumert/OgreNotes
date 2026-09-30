// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Text and outlines on the renderer's fixed, light data palettes. These
//! foregrounds stay dark in both document themes, unlike canvas text.

pub(crate) const DATA_TEXT: &str = "var(--mermaid-data-text, #1a1a1a)";
pub(crate) const DATA_MUTED_TEXT: &str = "var(--mermaid-data-text-muted, #444)";
pub(crate) const DATA_STROKE: &str = "var(--mermaid-data-stroke, #444)";
