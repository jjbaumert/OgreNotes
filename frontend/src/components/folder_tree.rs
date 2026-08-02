// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! The folder picker's **data**, extracted from its modal shell.
//!
//! `FolderPickerDialog` is a modal. The Quip import wizard is also a modal,
//! and nesting one inside the other risks focus-trap conflicts — that is the
//! documented reason Phase 1 of the importer shipped with a hard-coded Home
//! destination (#236 Unit 3). So the wizard renders a destination *step* of
//! its own inside the modal it already owns, and reuses the picker's data
//! layer from here rather than the picker's `<dialog>`.
//!
//! There are three pieces to that data layer and only the third ever lived
//! inside a component:
//!
//! 1. **The fetches** — `api::folders::get_folder` and the `/users/me`
//!    home-folder lookup. Plain async functions; never needed a component.
//! 2. **The state** — a `HashMap<folder id, FolderResponse>` of everything
//!    visited plus a `HashSet` of expanded ids. Plain values.
//! 3. **The flattening** — turning that map + set into the indented row list
//!    a tree renders as. This was a private `fn render_tree` inside
//!    `folder_picker.rs`; it lives here now so both callers share one
//!    implementation, and because as a pure function it is the one part of a
//!    folder picker that a crate with no DOM harness can actually test.
//!
//! Deliberately *not* here: selection state, the confirm/cancel affordances,
//! and anything that closes a dialog. Those differ between a modal that
//! dismisses itself and a wizard step that returns to the step before it.

use std::collections::{HashMap, HashSet};

use crate::api::folders::{ChildResponse, FolderResponse};

/// One rendered line of the folder tree.
///
/// `depth` drives indentation, not nesting: the tree renders flat with
/// padding so the signal graph stays a single list rather than a component
/// per node.
pub struct FolderTreeRow {
    pub id: String,
    /// Empty when `is_loaded` is false — the caller supplies its own
    /// placeholder wording. Kept out of here so this stays a pure function:
    /// `t!` reads a thread-local bundle that `i18n::init` populates by
    /// touching `document`, which a native test cannot do.
    pub title: String,
    pub depth: u8,
    pub has_children: bool,
    pub is_expanded: bool,
    pub is_loaded: bool,
    /// Trash is never a legal destination — for a restore, and equally for
    /// an import.
    pub is_trash: bool,
}

impl FolderTreeRow {
    /// Whether this row can be chosen. A row that has not loaded yet has no
    /// title to show and no children to trust; Trash is not a destination.
    pub fn is_selectable(&self) -> bool {
        self.is_loaded && !self.is_trash
    }
}

/// Flatten the visited-folder map into indented rows, starting at `root_id`
/// and descending into every expanded folder.
///
/// A folder that is expanded but not yet fetched emits a single
/// `is_loaded: false` placeholder row and no children — the caller has
/// kicked off its fetch and will re-render when it lands.
pub fn flatten_tree(
    root_id: &str,
    folders: &HashMap<String, FolderResponse>,
    expanded: &HashSet<String>,
    out: &mut Vec<FolderTreeRow>,
    depth: u8,
) {
    let Some(folder) = folders.get(root_id) else {
        out.push(FolderTreeRow {
            id: root_id.to_string(),
            title: String::new(),
            depth,
            has_children: false,
            is_expanded: false,
            is_loaded: false,
            is_trash: false,
        });
        return;
    };
    let child_folders: Vec<&ChildResponse> = folder
        .children
        .iter()
        .filter(|c| c.child_type == "folder")
        .collect();
    out.push(FolderTreeRow {
        id: folder.id.clone(),
        title: folder.title.clone(),
        depth,
        has_children: !child_folders.is_empty(),
        is_expanded: expanded.contains(&folder.id),
        is_loaded: true,
        is_trash: folder.is_trash,
    });
    if !expanded.contains(&folder.id) {
        return;
    }
    for child in child_folders {
        flatten_tree(&child.child_id, folders, expanded, out, depth + 1);
    }
}

/// Convenience wrapper: flatten from `root_id` at depth 0 into a fresh Vec.
pub fn rows_from(
    root_id: &str,
    folders: &HashMap<String, FolderResponse>,
    expanded: &HashSet<String>,
) -> Vec<FolderTreeRow> {
    let mut out = Vec::new();
    flatten_tree(root_id, folders, expanded, &mut out, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(id: &str, kind: &str) -> ChildResponse {
        ChildResponse {
            child_id: id.to_string(),
            child_type: kind.to_string(),
            title: id.to_string(),
            added_at: 0,
            is_deleted: false,
        }
    }

    fn folder(id: &str, title: &str, children: Vec<ChildResponse>) -> FolderResponse {
        FolderResponse {
            id: id.to_string(),
            title: title.to_string(),
            color: 0,
            parent_id: None,
            folder_type: "normal".to_string(),
            created_at: 0,
            updated_at: 0,
            is_trash: false,
            children,
        }
    }

    fn map(folders: Vec<FolderResponse>) -> HashMap<String, FolderResponse> {
        folders.into_iter().map(|f| (f.id.clone(), f)).collect()
    }

    fn expanded(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_collapsed_root_renders_one_row_and_hides_its_children() {
        let folders = map(vec![
            folder("home", "Home", vec![child("work", "folder")]),
            folder("work", "Work", vec![]),
        ]);
        let rows = rows_from("home", &folders, &expanded(&[]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "home");
        assert!(rows[0].has_children, "the chevron must still be offered");
        assert!(!rows[0].is_expanded);
    }

    #[test]
    fn an_expanded_tree_indents_each_generation() {
        let folders = map(vec![
            folder("home", "Home", vec![child("work", "folder")]),
            folder("work", "Work", vec![child("q3", "folder")]),
            folder("q3", "Q3", vec![]),
        ]);
        let rows = rows_from("home", &folders, &expanded(&["home", "work"]));
        let shape: Vec<(&str, u8)> = rows.iter().map(|r| (r.id.as_str(), r.depth)).collect();
        assert_eq!(shape, vec![("home", 0), ("work", 1), ("q3", 2)]);
    }

    #[test]
    fn documents_are_not_folders_and_never_become_rows() {
        let folders = map(vec![
            folder(
                "home",
                "Home",
                vec![child("doc-1", "document"), child("work", "folder")],
            ),
            folder("work", "Work", vec![]),
        ]);
        let rows = rows_from("home", &folders, &expanded(&["home"]));
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["home", "work"]);
        assert!(
            !rows[0].has_children || ids.contains(&"work"),
            "a doc-only folder must not claim a chevron",
        );
    }

    #[test]
    fn a_folder_holding_only_documents_offers_no_chevron() {
        let folders = map(vec![folder(
            "home",
            "Home",
            vec![child("doc-1", "document")],
        )]);
        let rows = rows_from("home", &folders, &expanded(&[]));
        assert!(!rows[0].has_children);
    }

    #[test]
    fn an_unfetched_child_renders_a_placeholder_rather_than_vanishing() {
        let folders = map(vec![folder(
            "home",
            "Home",
            vec![child("work", "folder")],
        )]);
        let rows = rows_from("home", &folders, &expanded(&["home"]));
        assert_eq!(rows.len(), 2, "the pending child still occupies a row");
        assert!(!rows[1].is_loaded);
        assert!(
            rows[1].title.is_empty(),
            "the placeholder's wording belongs to the caller's catalog",
        );
        assert!(
            !rows[1].is_selectable(),
            "a folder we know nothing about is not a destination",
        );
    }

    #[test]
    fn trash_is_never_a_destination() {
        let mut trash = folder("trash", "Trash", vec![]);
        trash.is_trash = true;
        let folders = map(vec![
            folder("home", "Home", vec![child("trash", "folder")]),
            trash,
        ]);
        let rows = rows_from("home", &folders, &expanded(&["home"]));
        assert!(rows[0].is_selectable(), "Home is a destination");
        assert!(rows[1].is_trash);
        assert!(!rows[1].is_selectable(), "Trash is not");
    }
}
