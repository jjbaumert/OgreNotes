// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

use super::menu::{AnchoredMenu, MenuEntry};
use super::toolbar::ToolbarCommand;

/// Document-level actions dispatched from the menu bar.
#[derive(Debug, Clone)]
pub enum DocAction {
    NewDocument,
    Share,
    CopyLink,
    ExportHtml,
    ExportMarkdown,
    Print,
    DocumentHistory,
    DeleteDocument,
    ToggleConversation,
    ToggleOutline,
    ToggleComments,
    ToggleCursors,
    ToggleFocusMode,
    ExportCsv,
    ExportXlsx,
    /// #139: toggle block line numbers in the editor gutter.
    ToggleLineNumbers,
    /// #139: toggle page-break guides in the editor.
    TogglePageBreaks,
    /// #146: open the folder picker to move this document.
    MoveToFolder,
    /// #146: rename this document (prompts for a new title).
    RenameDocument,
    /// #146: duplicate this document (content + "… copy") in the same folder.
    DuplicateDocument,
    /// #147: open the in-app Find & Replace bar.
    OpenFindReplace,
    /// #141: open the read-only Document Details panel.
    DocumentDetails,
    /// #140: toggle the per-document edit lock (owner-only; freezes edits
    /// for everyone when on).
    ToggleLockEdits,
    /// #142: mark this document as a template (or unmark it).
    MarkAsTemplate,
    UnmarkTemplate,
    /// #142: open the template picker modal to start a new doc from a template.
    NewFromTemplate,
}

/// Top-level menu order — drives ArrowLeft/ArrowRight rotation
/// between open menus (the shared primitive's `on_switch` hook).
const MENU_ORDER: [&str; 5] = ["document", "edit", "view", "insert", "format"];

/// The Document menu's entries. Shared between the desktop menu bar
/// and the mobile `⋯` document-actions button in the doc header
/// (`pages/document.rs`) — the menu bar is hidden at the phone
/// breakpoint, and this is where its document-level actions surface
/// instead.
pub fn document_menu_entries(
    on_doc_action: Callback<DocAction>,
    is_template: ReadSignal<bool>,
) -> Vec<MenuEntry> {
    let doc = move |label: String, action: DocAction| {
        MenuEntry::action(label, move || on_doc_action.run(action.clone()))
    };
    vec![
        doc(crate::t!("menubar-doc-new"), DocAction::NewDocument),
        MenuEntry::Separator,
        doc(crate::t!("menubar-doc-share"), DocAction::Share),
        doc(crate::t!("menubar-doc-copy-link"), DocAction::CopyLink),
        doc(crate::t!("menubar-doc-move-folder"), DocAction::MoveToFolder),
        MenuEntry::Separator,
        doc(crate::t!("menubar-doc-duplicate"), DocAction::DuplicateDocument),
        // #142: Mark / Unmark template — single item whose label
        // flips with the current is_template state.
        if is_template.get() {
            doc(crate::t!("menubar-unmark-template"), DocAction::UnmarkTemplate)
        } else {
            doc(crate::t!("menubar-mark-template"), DocAction::MarkAsTemplate)
        },
        doc(crate::t!("menubar-doc-new-template"), DocAction::NewFromTemplate),
        MenuEntry::submenu(
            crate::t!("menubar-doc-export"),
            vec![
                doc(crate::t!("menubar-doc-export-html"), DocAction::ExportHtml),
                doc(crate::t!("menubar-doc-export-markdown"), DocAction::ExportMarkdown),
                doc(crate::t!("menubar-doc-export-csv"), DocAction::ExportCsv),
                doc(crate::t!("menubar-doc-export-excel"), DocAction::ExportXlsx),
            ],
        ),
        MenuEntry::Separator,
        doc(crate::t!("menubar-doc-print"), DocAction::Print).with_shortcut("Ctrl+P"),
        MenuEntry::Separator,
        doc(crate::t!("menubar-doc-history"), DocAction::DocumentHistory)
            .with_shortcut("Ctrl+Shift+H"),
        doc(crate::t!("menubar-doc-details"), DocAction::DocumentDetails),
        MenuEntry::Separator,
        doc(crate::t!("menubar-doc-rename"), DocAction::RenameDocument),
        doc(crate::t!("menubar-doc-delete"), DocAction::DeleteDocument),
    ]
}

/// Classic menu bar (Document | Edit | View | Insert | Format).
///
/// Dropdown chrome (backdrop, Escape, keyboard navigation, real
/// submenus for Export / Paragraph Style / Alignment / List) comes
/// from `components::menu`. This component keeps the bar-specific
/// interaction rules: click opens/toggles, hovering a sibling name
/// while a menu is open switches to it, and a hover-opened menu is
/// *committed* (not toggled shut) by the follow-up click.
#[component]
pub fn MenuBar(
    on_command: Callback<ToolbarCommand>,
    on_doc_action: Callback<DocAction>,
    /// Whether the conversation pane is visible.
    conversation_visible: ReadSignal<bool>,
    /// Whether the outline panel is visible.
    outline_visible: ReadSignal<bool>,
    /// Whether comment highlights/bubbles are visible.
    comments_visible: ReadSignal<bool>,
    /// Whether remote collaborators' cursors are rendered (#99).
    cursors_visible: ReadSignal<bool>,
    /// Whether focus mode (chrome hidden) is active (#100).
    focus_mode: ReadSignal<bool>,
    /// #139: whether block line numbers are shown in the editor gutter.
    line_numbers_visible: ReadSignal<bool>,
    /// #139: whether page-break guides are shown in the editor.
    page_breaks_visible: ReadSignal<bool>,
    /// #140: whether this document is locked for editing.
    locked: ReadSignal<bool>,
    /// #140: whether the caller may toggle the lock (owner-only). The
    /// "Lock Edits" control is rendered only when true.
    can_manage_lock: ReadSignal<bool>,
    /// #142: whether this doc is marked as a template. Drives the Document-menu
    /// item label (Mark vs Unmark).
    is_template: ReadSignal<bool>,
) -> impl IntoView {
    let (open_menu, set_open_menu) = signal::<Option<&'static str>>(None);
    // True when the currently-open menu was opened by *hover* (mouseenter
    // switching from an already-open menu) and is awaiting a possible click to
    // commit it. Without this, hovering a sibling menu opens it and the
    // follow-up click immediately toggled it back shut — so clicking a
    // different top-level menu while one was open closed it instead of
    // switching (the menu-switch doctor regression).
    let (hover_opened, set_hover_opened) = signal(false);

    let close = Callback::new(move |()| {
        set_open_menu.set(None);
        set_hover_opened.set(false);
    });

    // Click a top-level menu name. Opens it (switching from any other), or
    // closes it when it's already open — unless a hover just opened it, in
    // which case the click *commits* it (keeps it open).
    let toggle_menu = move |name: &'static str| {
        if open_menu.get_untracked() == Some(name) {
            if hover_opened.get_untracked() {
                set_hover_opened.set(false);
            } else {
                set_open_menu.set(None);
            }
        } else {
            set_open_menu.set(Some(name));
            set_hover_opened.set(false);
        }
    };

    // Hover-to-switch: when another menu is already open, moving over a
    // sibling opens it. Flagged as hover-opened so a following click commits
    // rather than toggles it closed.
    let hover_menu = move |name: &'static str| {
        let cur = open_menu.get_untracked();
        if cur.is_some() && cur != Some(name) {
            set_open_menu.set(Some(name));
            set_hover_opened.set(true);
        }
    };

    // ArrowLeft/ArrowRight at a dropdown's root panel rotates between
    // the top-level menus, wrapping at the ends (menubar keyboard
    // convention).
    let switch = Callback::new(move |dir: i32| {
        let Some(cur) = open_menu.get_untracked() else { return };
        let Some(pos) = MENU_ORDER.iter().position(|n| *n == cur) else { return };
        let next = (pos as i32 + dir).rem_euclid(MENU_ORDER.len() as i32) as usize;
        set_open_menu.set(Some(MENU_ORDER[next]));
        set_hover_opened.set(false);
    });

    let on_mousedown = |ev: web_sys::MouseEvent| {
        ev.prevent_default();
    };

    // Entry-builder helpers. `doc`/`cmd` dispatch and let the shared
    // chrome close the menu after activation.
    let doc = move |label: String, action: DocAction| {
        MenuEntry::action(label, move || on_doc_action.run(action.clone()))
    };
    let cmd = move |label: String, command: ToolbarCommand| {
        MenuEntry::action(label, move || on_command.run(command.clone()))
    };

    let document_entries =
        Callback::new(move |()| document_menu_entries(on_doc_action, is_template));

    // Cut/Copy/Paste and "Copy Anchor Link" are deliberately absent:
    // the clipboard rows were no-ops that only closed the menu (the
    // browser shortcuts and the editor's right-click menu do the real
    // work), and Copy Anchor Link dispatched the identical CopyLink
    // action the Document menu already offers.
    let edit_entries = Callback::new(move |()| {
        vec![
            cmd(crate::t!("menubar-edit-undo"), ToolbarCommand::Undo).with_shortcut("Ctrl+Z"),
            cmd(crate::t!("menubar-edit-redo"), ToolbarCommand::Redo)
                .with_shortcut("Ctrl+Shift+Z"),
            MenuEntry::Separator,
            doc(crate::t!("menubar-edit-find"), DocAction::OpenFindReplace)
                .with_shortcut("Ctrl+F"),
        ]
    });

    let view_entries = Callback::new(move |()| {
        let toggle = move |label: String, active: ReadSignal<bool>, action: DocAction| {
            MenuEntry::toggle(label, active, move || on_doc_action.run(action.clone()))
        };
        vec![
            toggle(crate::t!("menubar-view-comments"), comments_visible, DocAction::ToggleComments),
            toggle(
                crate::t!("menubar-view-conversation"),
                conversation_visible,
                DocAction::ToggleConversation,
            ),
            toggle(crate::t!("menubar-view-cursors"), cursors_visible, DocAction::ToggleCursors),
            toggle(crate::t!("menubar-view-focus"), focus_mode, DocAction::ToggleFocusMode),
            toggle(
                crate::t!("menubar-view-line-numbers"),
                line_numbers_visible,
                DocAction::ToggleLineNumbers,
            ),
            toggle(
                crate::t!("menubar-view-page-breaks"),
                page_breaks_visible,
                DocAction::TogglePageBreaks,
            ),
            MenuEntry::Separator,
            toggle(crate::t!("menubar-view-outline"), outline_visible, DocAction::ToggleOutline),
            // "Keep Outline Expanded" (a TODO no-op) and a duplicate
            // "Document History" row were removed — History lives in
            // the Document menu.
        ]
    });

    let insert_entries = Callback::new(move |()| {
        // #148 v2 slice 4 — catalog swap. Link remains hard-coded
        // because it takes a URL prompt at click time, not a zero-arg
        // dispatch; every other entry rides the shared catalog so a
        // new insertable added to `INSERT_CATALOG` shows up here for
        // free.
        let mut entries = vec![
            cmd(
                crate::t!("menubar-insert-link"),
                ToolbarCommand::ToggleLink(String::new()),
            )
            .with_shortcut("Ctrl+K"),
        ];
        entries.extend(
            crate::inserts::catalog_for(crate::inserts::InsertSurface::Menubar)
                .into_iter()
                .map(|item| {
                    cmd(crate::i18n::translate(item.label_key(), None), item.command())
                        .with_icon(item.icon())
                }),
        );
        entries
    });

    let format_entries = Callback::new(move |()| {
        let mut entries = vec![
            cmd(crate::t!("menu-bold"), ToolbarCommand::ToggleBold)
                .with_icon("B")
                .with_shortcut("Ctrl+B"),
            cmd(crate::t!("menu-italic"), ToolbarCommand::ToggleItalic)
                .with_icon("I")
                .with_shortcut("Ctrl+I"),
            cmd(crate::t!("menu-underline"), ToolbarCommand::ToggleUnderline)
                .with_icon("U")
                .with_shortcut("Ctrl+U"),
            cmd(crate::t!("menu-strikethrough"), ToolbarCommand::ToggleStrike)
                .with_icon("S\u{0336}")
                .with_shortcut("Ctrl+Shift+X"),
            cmd(crate::t!("menubar-format-subscript"), ToolbarCommand::ToggleSubscript)
                .with_icon("x\u{2082}")
                .with_shortcut("Ctrl+,"),
            cmd(crate::t!("menubar-format-superscript"), ToolbarCommand::ToggleSuperscript)
                .with_icon("x\u{00B2}")
                .with_shortcut("Ctrl+."),
            // Text Color / Highlight rows were no-op stubs — the real
            // pickers live in the toolbar and can't be triggered from
            // here without opening them there anyway.
            cmd(crate::t!("menu-code"), ToolbarCommand::ToggleCode)
                .with_icon("</>")
                .with_shortcut("Ctrl+Shift+K"),
            MenuEntry::Separator,
            MenuEntry::submenu(
                crate::t!("menubar-format-paragraph-style"),
                vec![
                    cmd(crate::t!("toolbar-block-paragraph"), ToolbarCommand::SetParagraph),
                    cmd(crate::t!("toolbar-block-heading-1"), ToolbarCommand::SetHeading(1)),
                    cmd(crate::t!("toolbar-block-heading-2"), ToolbarCommand::SetHeading(2)),
                    cmd(crate::t!("toolbar-block-heading-3"), ToolbarCommand::SetHeading(3)),
                    cmd(crate::t!("toolbar-block-code-block"), ToolbarCommand::SetCodeBlock),
                    cmd(crate::t!("node-blockquote"), ToolbarCommand::ToggleBlockquote),
                ],
            ),
            MenuEntry::submenu(
                crate::t!("menu-alignment"),
                vec![
                    cmd(
                        crate::t!("menu-align-left"),
                        ToolbarCommand::SetAlignment("left".to_string()),
                    ),
                    cmd(
                        crate::t!("menu-align-center"),
                        ToolbarCommand::SetAlignment("center".to_string()),
                    ),
                    cmd(
                        crate::t!("menu-align-right"),
                        ToolbarCommand::SetAlignment("right".to_string()),
                    ),
                ],
            ),
            MenuEntry::submenu(
                crate::t!("menubar-format-list"),
                vec![
                    cmd(
                        crate::t!("toolbar-block-bulleted-list"),
                        ToolbarCommand::ToggleBulletList,
                    ),
                    cmd(
                        crate::t!("toolbar-block-numbered-list"),
                        ToolbarCommand::ToggleOrderedList,
                    ),
                    cmd(crate::t!("toolbar-block-checklist"), ToolbarCommand::ToggleTaskList),
                ],
            ),
            MenuEntry::Separator,
            cmd(crate::t!("menubar-format-clear"), ToolbarCommand::ClearFormatting)
                .with_shortcut("Ctrl+\\"),
        ];
        // #140: owner-only edit lock. Hidden for non-owners so only
        // someone who can actually toggle it sees it.
        if can_manage_lock.get() {
            entries.push(MenuEntry::Separator);
            entries.push(MenuEntry::toggle(
                crate::t!("menubar-format-lock"),
                locked,
                move || on_doc_action.run(DocAction::ToggleLockEdits),
            ));
        }
        entries
    });

    // One trigger + anchored dropdown per top-level menu.
    let menu = move |name: &'static str,
                     label: String,
                     entries: Callback<(), Vec<MenuEntry>>| {
        view! {
            <div class="menu-bar-item-wrapper">
                <button class="menu-bar-item"
                    class:open=move || open_menu.get() == Some(name)
                    on:click=move |_| toggle_menu(name)
                    on:mouseenter=move |_| hover_menu(name)
                >{label}</button>
                <AnchoredMenu
                    open=Signal::derive(move || open_menu.get() == Some(name))
                    entries=entries
                    on_close=close
                    preserve_focus=true
                    on_switch=switch
                />
            </div>
        }
    };

    view! {
        <div class="menu-bar" on:mousedown=on_mousedown>
            {menu("document", crate::t!("menubar-document"), document_entries)}
            {menu("edit", crate::t!("menubar-edit"), edit_entries)}
            {menu("view", crate::t!("menubar-view"), view_entries)}
            {menu("insert", crate::t!("menubar-insert"), insert_entries)}
            {menu("format", crate::t!("menubar-format"), format_entries)}
        </div>
    }
}
