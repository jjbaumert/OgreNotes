// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #148: consolidated insert registry.
//!
//! Every insert surface (toolbar, menubar, `@`-menu, hover block-
//! menu, command palette) reads from this one catalog so a new
//! insertable shows up everywhere at once. Prior to this module,
//! Image / Link / HR / Table / Embed were hard-coded per-variant
//! in each surface; only Kanban and Calendar went through a real
//! registry (`crate::editor::blocks::BLOCK_INSERTS`).
//!
//! `full_catalog()` folds the built-in entries with the live-app
//! blocks so surfaces don't need to iterate two things.

use crate::components::toolbar::ToolbarCommand;

/// Categorization used by surfaces to filter or section the
/// catalog. The `@`-menu shows all sections; the toolbar's Insert
/// group excludes `Ai` (a live LLM widget in the toolbar row
/// would clutter it); the hover block-menu shows `Block` +
/// `LiveApp` (media inserts have their own toolbar affordance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertSection {
    /// Block-shaped inserts that don't need a picker or async
    /// resolve step: table, horizontal rule, code block.
    Block,
    /// Media inserts that open a file/URL picker: image, embed.
    Media,
    /// Live-app blocks (Kanban, Calendar) via the pre-existing
    /// `BLOCK_INSERTS` registry.
    LiveApp,
    /// AI actions — `@ask` today; `@summarize` / `@translate`
    /// slot in as v2 wrappers.
    Ai,
    /// Cross-doc entries that need runtime data (user picker,
    /// doc search). Not iterated as a static list; the surface
    /// resolves them per query.
    Runtime,
}

/// Static describe of one insertable. `command` returns a
/// concrete `ToolbarCommand` variant to dispatch when the user
/// activates the entry; the surface's existing dispatch chain
/// (toolbar → editor_component → commands::*) handles it.
///
/// `id` is a stable kebab-case string; surfaces may use it as a
/// key. `label_key` / `description_key` are Fluent keys —
/// convention `insert-<id>-label` / `insert-<id>-description`.
/// `keywords` widen the fuzzy match in the `@`-menu (e.g.
/// `divider` for HR, `photo` for image).
pub struct InsertEntry {
    pub id: &'static str,
    pub label_key: &'static str,
    pub description_key: &'static str,
    pub icon: &'static str,
    pub section: InsertSection,
    pub keywords: &'static [&'static str],
    pub command: fn() -> ToolbarCommand,
}

/// The built-in insert entries. Live-app block entries fold in
/// via `full_catalog()`; user + document mentions come from the
/// runtime search paths and aren't in this static list.
///
/// Order here is the display order surfaces use when no query is
/// active.
pub const INSERT_CATALOG: &[InsertEntry] = &[
    InsertEntry {
        id: "table",
        label_key: "insert-table-label",
        description_key: "insert-table-description",
        icon: "\u{1F4CB}", // 📋
        section: InsertSection::Block,
        keywords: &["grid", "spreadsheet"],
        command: || ToolbarCommand::InsertTable,
    },
    InsertEntry {
        id: "image",
        label_key: "insert-image-label",
        description_key: "insert-image-description",
        icon: "\u{1F5BC}", // 🖼
        section: InsertSection::Media,
        keywords: &["photo", "picture"],
        command: || ToolbarCommand::UploadImage,
    },
    InsertEntry {
        id: "horizontal-rule",
        label_key: "insert-horizontal-rule-label",
        description_key: "insert-horizontal-rule-description",
        icon: "\u{2500}", // ─
        section: InsertSection::Block,
        keywords: &["divider", "hr", "line"],
        command: || ToolbarCommand::InsertHorizontalRule,
    },
    InsertEntry {
        id: "code-block",
        label_key: "insert-code-block-label",
        description_key: "insert-code-block-description",
        icon: "\u{1F4BB}", // 💻
        section: InsertSection::Block,
        keywords: &["code", "snippet"],
        command: || ToolbarCommand::SetCodeBlock,
    },
    // Embed and Link don't have a zero-arg command variant
    // (Embed needs a resolve round-trip; Link needs a URL
    // prompt). Surfaces that expose them still route through
    // the existing toolbar-specific flows; the `@`-menu's item
    // producer synthesizes their entries with a dedicated
    // `AtMenuItemKind` so it can trigger the prompt/flow.
];

/// The full catalog: built-ins + live-app blocks folded in as
/// dynamic entries. Called by insert surfaces on render — cheap
/// (constant + a small vec allocation) so no need to cache.
pub fn full_catalog() -> Vec<CatalogItem> {
    let mut items: Vec<CatalogItem> = INSERT_CATALOG
        .iter()
        .map(|e| CatalogItem::Builtin(e))
        .collect();
    for entry in crate::editor::blocks::BLOCK_INSERTS {
        items.push(CatalogItem::LiveApp(*entry));
    }
    items
}

/// Discriminates a catalog item between the built-in
/// `InsertEntry` and a live-app block insert. Surfaces render
/// both the same way (label + icon + description); dispatch
/// forks per variant.
pub enum CatalogItem {
    Builtin(&'static InsertEntry),
    LiveApp(&'static (dyn crate::editor::blocks::LiveAppBlockInsert + 'static)),
}

/// #148 v2 slice 4 — which insert surface a caller is rendering.
/// Different surfaces show different subsets of the catalog:
///
/// - `Toolbar` / `Menubar` Insert group: block-shape + media +
///   live-app entries. Code-block is EXCLUDED from these two
///   because the toolbar exposes it through its block-type
///   dropdown; showing it twice would be noise.
/// - `BlockMenu`: only entries that make sense as
///   quick-insert-at-caret from a hovering menu — horizontal
///   rule + live-app blocks. The menu already has its own
///   heading / list / blockquote toggles that aren't
///   catalog-tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertSurface {
    Toolbar,
    Menubar,
    BlockMenu,
}

/// Iterate `full_catalog()` filtered to the entries a given
/// surface should render. Encapsulates the per-surface rules so
/// the three surfaces can share one loop shape.
pub fn catalog_for(surface: InsertSurface) -> Vec<CatalogItem> {
    full_catalog()
        .into_iter()
        .filter(|item| item.visible_in(surface))
        .collect()
}

impl CatalogItem {
    /// Per-surface visibility rule. See `InsertSurface` doc for
    /// the reasoning behind each exclusion.
    pub fn visible_in(&self, surface: InsertSurface) -> bool {
        match (surface, self.section(), self.id()) {
            // Toolbar / Menubar exclude code-block (lives in the
            // block-type dropdown) and Runtime entries (user
            // mention / doc link — those come through the
            // @-menu).
            (
                InsertSurface::Toolbar | InsertSurface::Menubar,
                InsertSection::Runtime,
                _,
            ) => false,
            (
                InsertSurface::Toolbar | InsertSurface::Menubar,
                _,
                "code-block",
            ) => false,
            (InsertSurface::Toolbar | InsertSurface::Menubar, _, _) => true,
            // BlockMenu: HR + live-app blocks only. Media and
            // table have their own toolbar affordance.
            (InsertSurface::BlockMenu, InsertSection::LiveApp, _) => true,
            (InsertSurface::BlockMenu, _, "horizontal-rule") => true,
            (InsertSurface::BlockMenu, _, _) => false,
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            CatalogItem::Builtin(e) => e.id,
            CatalogItem::LiveApp(l) => l.id(),
        }
    }

    pub fn label_key(&self) -> &'static str {
        match self {
            CatalogItem::Builtin(e) => e.label_key,
            CatalogItem::LiveApp(l) => l.label_key(),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            CatalogItem::Builtin(e) => e.icon,
            CatalogItem::LiveApp(l) => l.icon(),
        }
    }

    pub fn section(&self) -> InsertSection {
        match self {
            CatalogItem::Builtin(e) => e.section,
            CatalogItem::LiveApp(_) => InsertSection::LiveApp,
        }
    }

    pub fn command(&self) -> ToolbarCommand {
        match self {
            CatalogItem::Builtin(e) => (e.command)(),
            CatalogItem::LiveApp(l) => {
                ToolbarCommand::InsertLiveApp(l.id())
            }
        }
    }

    /// Cheap case-insensitive substring match against label,
    /// id, and keywords. The `@`-menu narrows the catalog with
    /// this; empty query returns everything.
    pub fn matches_query(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.to_ascii_lowercase();
        if self.id().to_ascii_lowercase().contains(&q) {
            return true;
        }
        // Fluent labels are localized — best-effort match on
        // the resolved translation.
        let label = crate::i18n::translate(self.label_key(), None);
        if label.to_ascii_lowercase().contains(&q) {
            return true;
        }
        if let CatalogItem::Builtin(e) = self {
            for kw in e.keywords {
                if kw.to_ascii_lowercase().contains(&q) {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_ids_are_unique() {
        let mut seen: HashSet<&'static str> = HashSet::new();
        for entry in INSERT_CATALOG {
            assert!(
                seen.insert(entry.id),
                "duplicate insert-catalog id: {}",
                entry.id
            );
        }
    }

    #[test]
    fn full_catalog_includes_live_app_blocks() {
        let ids: HashSet<&'static str> = full_catalog()
            .iter()
            .map(|i| i.id())
            .collect();
        // Kanban and Calendar are registered in BLOCK_INSERTS.
        assert!(ids.contains("kanban"));
        assert!(ids.contains("calendar"));
        // A built-in is also present.
        assert!(ids.contains("table"));
    }

    #[test]
    fn toolbar_excludes_code_block() {
        let ids: HashSet<&'static str> = catalog_for(InsertSurface::Toolbar)
            .iter()
            .map(|i| i.id())
            .collect();
        assert!(!ids.contains("code-block"), "code-block belongs to the block-type dropdown, not the Insert group");
        assert!(ids.contains("table"));
        assert!(ids.contains("image"));
        assert!(ids.contains("horizontal-rule"));
        assert!(ids.contains("kanban"));
    }

    #[test]
    fn menubar_matches_toolbar() {
        let tb: HashSet<&'static str> = catalog_for(InsertSurface::Toolbar)
            .iter()
            .map(|i| i.id())
            .collect();
        let mb: HashSet<&'static str> = catalog_for(InsertSurface::Menubar)
            .iter()
            .map(|i| i.id())
            .collect();
        assert_eq!(tb, mb, "toolbar and menubar Insert lists should match");
    }

    #[test]
    fn block_menu_only_shows_hr_and_live_apps() {
        let ids: HashSet<&'static str> = catalog_for(InsertSurface::BlockMenu)
            .iter()
            .map(|i| i.id())
            .collect();
        assert!(ids.contains("horizontal-rule"));
        assert!(ids.contains("kanban"));
        assert!(ids.contains("calendar"));
        // Media + table stay in the toolbar; the block menu is
        // for at-caret quick inserts only.
        assert!(!ids.contains("image"));
        assert!(!ids.contains("table"));
        assert!(!ids.contains("code-block"));
    }
}
