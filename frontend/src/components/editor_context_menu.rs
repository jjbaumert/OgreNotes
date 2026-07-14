// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Document-editor right-click context menu.
//!
//! Visible when `visible` is true; positioned by `x`/`y` (viewport
//! coordinates, clamped to the viewport by the shared primitive).
//! Items dispatch one of `EditorContextCommand` — caller handles the
//! actual work (clipboard ops, format toggles, Comment popup, link
//! prompt).
//!
//! Chrome (backdrop, Escape, keyboard navigation, submenus that open
//! on hover/click/ArrowRight) comes from `components::menu`. The menu
//! runs with `preserve_focus` so the contenteditable keeps DOM focus
//! and its selection — the clipboard / format commands need both when
//! they run.

use leptos::prelude::*;

use super::menu::{ContextMenu, MenuEntry};

/// Commands the editor context menu can dispatch.
#[derive(Debug, Clone)]
pub enum EditorContextCommand {
    Cut,
    Copy,
    Paste,
    Comment,
    ToggleBold,
    ToggleItalic,
    ToggleUnderline,
    ToggleStrike,
    ToggleCode,
    InsertLink,
    // Paragraph-style submenu — each item retypes the current
    // textblock. The dispatch path mirrors the toolbar's block
    // dropdown so behavior stays in sync.
    SetParagraph,
    SetHeading1,
    SetHeading2,
    SetHeading3,
    ToggleBulletList,
    ToggleOrderedList,
    ToggleTaskList,
    ToggleBlockquote,
    SetCodeBlock,
    // Alignment submenu — sets the `align` attr on the current
    // textblock. Left clears the attr (left is the natural default).
    AlignLeft,
    AlignCenter,
    AlignRight,
}

/// Right-click context menu over the document editor.
#[component]
pub fn EditorContextMenu(
    visible: ReadSignal<bool>,
    x: ReadSignal<f64>,
    y: ReadSignal<f64>,
    /// True when the editor's current selection is empty (cursor only).
    /// Used to disable items that require a non-empty range. Accepted
    /// as `Signal<bool>` so callers can pass a `Memo` or a
    /// `Signal::derive` rather than only a `ReadSignal`.
    selection_empty: Signal<bool>,
    on_command: Callback<EditorContextCommand>,
    on_close: Callback<()>,
) -> impl IntoView {
    let item = move |label_key: &'static str, cmd: EditorContextCommand| {
        MenuEntry::action(crate::i18n::translate(label_key, None), move || {
            on_command.run(cmd.clone());
        })
    };

    let entries = Callback::new(move |()| {
        let empty = selection_empty.get();
        vec![
            item("menu-cut", EditorContextCommand::Cut)
                .with_shortcut("Ctrl+X")
                .disabled_when(empty),
            item("menu-copy", EditorContextCommand::Copy)
                .with_shortcut("Ctrl+C")
                .disabled_when(empty),
            item("menu-paste", EditorContextCommand::Paste).with_shortcut("Ctrl+V"),
            MenuEntry::Separator,
            item("menu-comment", EditorContextCommand::Comment)
                .with_shortcut("\u{1F4AC}")
                .disabled_when(empty),
            MenuEntry::Separator,
            MenuEntry::submenu(
                crate::t!("editorctx-paragraph-style"),
                vec![
                    item("toolbar-block-paragraph", EditorContextCommand::SetParagraph),
                    item("toolbar-block-heading-1", EditorContextCommand::SetHeading1),
                    item("toolbar-block-heading-2", EditorContextCommand::SetHeading2),
                    item("toolbar-block-heading-3", EditorContextCommand::SetHeading3),
                    MenuEntry::Separator,
                    item("node-bullet-list", EditorContextCommand::ToggleBulletList),
                    item("node-ordered-list", EditorContextCommand::ToggleOrderedList),
                    item("node-task-list", EditorContextCommand::ToggleTaskList),
                    MenuEntry::Separator,
                    item("toolbar-block-blockquote", EditorContextCommand::ToggleBlockquote),
                    item("node-code-block", EditorContextCommand::SetCodeBlock),
                ],
            ),
            MenuEntry::submenu(
                crate::t!("menu-alignment"),
                vec![
                    item("menu-align-left", EditorContextCommand::AlignLeft),
                    item("menu-align-center", EditorContextCommand::AlignCenter),
                    item("menu-align-right", EditorContextCommand::AlignRight),
                ],
            ),
            MenuEntry::Separator,
            item("menu-bold", EditorContextCommand::ToggleBold).with_shortcut("Ctrl+B"),
            item("menu-italic", EditorContextCommand::ToggleItalic).with_shortcut("Ctrl+I"),
            item("menu-underline", EditorContextCommand::ToggleUnderline).with_shortcut("Ctrl+U"),
            item("menu-strikethrough", EditorContextCommand::ToggleStrike),
            item("menu-code", EditorContextCommand::ToggleCode),
            MenuEntry::Separator,
            item("editorctx-insert-link", EditorContextCommand::InsertLink).with_shortcut("Ctrl+K"),
        ]
    });

    view! {
        <ContextMenu
            visible=visible
            x=x
            y=y
            entries=entries
            on_close=on_close
            preserve_focus=true
        />
    }
}
