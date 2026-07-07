// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

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

/// Classic menu bar (Document | Edit | View | Insert | Format).
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

    let close = move || {
        set_open_menu.set(None);
        set_hover_opened.set(false);
    };

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

    let on_mousedown = |ev: web_sys::MouseEvent| {
        ev.prevent_default();
    };

    view! {
        <div class="menu-bar" on:mousedown=on_mousedown>
            // ─── Document ───
            <div class="menu-bar-item-wrapper">
                <button class="menu-bar-item"
                    class:open=move || open_menu.get() == Some("document")
                    on:click=move |_| toggle_menu("document")
                    on:mouseenter=move |_| hover_menu("document")
                >{crate::t!("menubar-document")}</button>
                <Show when=move || open_menu.get() == Some("document")>
                    <div class="menu-bar-backdrop" on:click=move |_| close()></div>
                    <div class="menu-bar-dropdown">
                        {menu_action("New", "", move || {
                            on_doc_action.run(DocAction::NewDocument); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Share\u{2026}", "", move || {
                            on_doc_action.run(DocAction::Share); close();
                        })}
                        {menu_action("Copy Link", "", move || {
                            on_doc_action.run(DocAction::CopyLink); close();
                        })}
                        {menu_action("Move to Folder\u{2026}", "", move || {
                            on_doc_action.run(DocAction::MoveToFolder); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Duplicate\u{2026}", "", move || {
                            on_doc_action.run(DocAction::DuplicateDocument); close();
                        })}
                        // #142: Mark / Unmark template — single item whose label
                        // flips with the current is_template state. Inlined
                        // (not via menu_action) because menu_action takes a
                        // &'static str label.
                        <button class="menu-bar-action" on:click=move |_| {
                            let action = if is_template.get_untracked() {
                                DocAction::UnmarkTemplate
                            } else {
                                DocAction::MarkAsTemplate
                            };
                            on_doc_action.run(action); close();
                        }>
                            <span class="menu-bar-action-label">{move || if is_template.get() {
                                crate::t!("menubar-unmark-template")
                            } else {
                                crate::t!("menubar-mark-template")
                            }}</span>
                        </button>
                        {menu_action("New from Template\u{2026}", "", move || {
                            on_doc_action.run(DocAction::NewFromTemplate); close();
                        })}
                        {menu_action_sub("Export", move || {
                            // Submenu would go here; for now show export options inline
                        })}
                        {menu_action("  HTML", "", move || {
                            on_doc_action.run(DocAction::ExportHtml); close();
                        })}
                        {menu_action("  Markdown (copy)", "", move || {
                            on_doc_action.run(DocAction::ExportMarkdown); close();
                        })}
                        {menu_action("  CSV", "", move || {
                            on_doc_action.run(DocAction::ExportCsv); close();
                        })}
                        {menu_action("  Excel (.xlsx)", "", move || {
                            on_doc_action.run(DocAction::ExportXlsx); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Print\u{2026}", "Ctrl+P", move || {
                            on_doc_action.run(DocAction::Print); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Document History\u{2026}", "Ctrl+Shift+H", move || {
                            on_doc_action.run(DocAction::DocumentHistory); close();
                        })}
                        {menu_action("Document Details\u{2026}", "", move || {
                            on_doc_action.run(DocAction::DocumentDetails); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Rename Document\u{2026}", "", move || {
                            on_doc_action.run(DocAction::RenameDocument); close();
                        })}
                        {menu_action("Delete Document\u{2026}", "", move || {
                            on_doc_action.run(DocAction::DeleteDocument); close();
                        })}
                    </div>
                </Show>
            </div>

            // ─── Edit ───
            <div class="menu-bar-item-wrapper">
                <button class="menu-bar-item"
                    class:open=move || open_menu.get() == Some("edit")
                    on:click=move |_| toggle_menu("edit")
                    on:mouseenter=move |_| hover_menu("edit")
                >{crate::t!("menubar-edit")}</button>
                <Show when=move || open_menu.get() == Some("edit")>
                    <div class="menu-bar-backdrop" on:click=move |_| close()></div>
                    <div class="menu-bar-dropdown">
                        {menu_action("Undo", "Ctrl+Z", move || {
                            on_command.run(ToolbarCommand::Undo); close();
                        })}
                        {menu_action("Redo", "Ctrl+Shift+Z", move || {
                            on_command.run(ToolbarCommand::Redo); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Cut", "Ctrl+X", move || {
                            // Handled natively by the browser.
                            close();
                        })}
                        {menu_action("Copy", "Ctrl+C", move || {
                            close();
                        })}
                        {menu_action("Paste", "Ctrl+V", move || {
                            close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Find and Replace", "Ctrl+F", move || {
                            on_doc_action.run(DocAction::OpenFindReplace); close();
                        })}
                        {menu_action("Copy Anchor Link", "Ctrl+Shift+A", move || {
                            on_doc_action.run(DocAction::CopyLink); close();
                        })}
                    </div>
                </Show>
            </div>

            // ─── View ───
            <div class="menu-bar-item-wrapper">
                <button class="menu-bar-item"
                    class:open=move || open_menu.get() == Some("view")
                    on:click=move |_| toggle_menu("view")
                    on:mouseenter=move |_| hover_menu("view")
                >{crate::t!("menubar-view")}</button>
                <Show when=move || open_menu.get() == Some("view")>
                    <div class="menu-bar-backdrop" on:click=move |_| close()></div>
                    <div class="menu-bar-dropdown">
                        {menu_toggle("Show Comments", comments_visible, move || {
                            on_doc_action.run(DocAction::ToggleComments); close();
                        })}
                        {menu_toggle("Show Conversation", conversation_visible, move || {
                            on_doc_action.run(DocAction::ToggleConversation); close();
                        })}
                        {menu_toggle("Show Cursors", cursors_visible, move || {
                            on_doc_action.run(DocAction::ToggleCursors); close();
                        })}
                        {menu_toggle("Focus Mode", focus_mode, move || {
                            on_doc_action.run(DocAction::ToggleFocusMode); close();
                        })}
                        {menu_toggle("Show Line Numbers", line_numbers_visible, move || {
                            on_doc_action.run(DocAction::ToggleLineNumbers); close();
                        })}
                        {menu_toggle("Show Page Breaks", page_breaks_visible, move || {
                            on_doc_action.run(DocAction::TogglePageBreaks); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_toggle("Show Outline", outline_visible, move || {
                            on_doc_action.run(DocAction::ToggleOutline); close();
                        })}
                        {menu_action("Keep Outline Expanded", "Ctrl+Shift+O", move || {
                            // TODO: separate "keep expanded" preference
                            close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Document History\u{2026}", "Ctrl+Shift+H", move || {
                            on_doc_action.run(DocAction::DocumentHistory); close();
                        })}
                    </div>
                </Show>
            </div>

            // ─── Insert ───
            <div class="menu-bar-item-wrapper">
                <button class="menu-bar-item"
                    class:open=move || open_menu.get() == Some("insert")
                    on:click=move |_| toggle_menu("insert")
                    on:mouseenter=move |_| hover_menu("insert")
                >{crate::t!("menubar-insert")}</button>
                <Show when=move || open_menu.get() == Some("insert")>
                    <div class="menu-bar-backdrop" on:click=move |_| close()></div>
                    <div class="menu-bar-dropdown">
                        // #148 v2 slice 4 — catalog swap. Link
                        // remains hard-coded because it takes a URL
                        // prompt at click time, not a zero-arg
                        // dispatch; every other entry rides the
                        // shared catalog so a new insertable added
                        // to `INSERT_CATALOG` shows up here for
                        // free.
                        {menu_action(crate::t!("menubar-insert-link"), "Ctrl+K", move || {
                            on_command.run(ToolbarCommand::ToggleLink(String::new()));
                            close();
                        })}
                        {crate::inserts::catalog_for(crate::inserts::InsertSurface::Menubar)
                            .into_iter()
                            .map(|item| {
                                let icon = item.icon();
                                let label = format!(
                                    "{icon} {}",
                                    crate::i18n::translate(item.label_key(), None),
                                );
                                let cmd = item.command();
                                view! {
                                    <button class="menu-bar-action"
                                        on:click=move |_| {
                                            on_command.run(cmd.clone());
                                            close();
                                        }
                                    >
                                        <span class="menu-bar-action-label">{label}</span>
                                    </button>
                                }
                            })
                            .collect_view()}
                    </div>
                </Show>
            </div>

            // ─── Format ───
            <div class="menu-bar-item-wrapper">
                <button class="menu-bar-item"
                    class:open=move || open_menu.get() == Some("format")
                    on:click=move |_| toggle_menu("format")
                    on:mouseenter=move |_| hover_menu("format")
                >{crate::t!("menubar-format")}</button>
                <Show when=move || open_menu.get() == Some("format")>
                    <div class="menu-bar-backdrop" on:click=move |_| close()></div>
                    <div class="menu-bar-dropdown">
                        {menu_icon_action("B", "Bold", "Ctrl+B", move || {
                            on_command.run(ToolbarCommand::ToggleBold); close();
                        })}
                        {menu_icon_action("I", "Italic", "Ctrl+I", move || {
                            on_command.run(ToolbarCommand::ToggleItalic); close();
                        })}
                        {menu_icon_action("U", "Underline", "Ctrl+U", move || {
                            on_command.run(ToolbarCommand::ToggleUnderline); close();
                        })}
                        {menu_icon_action("S\u{0336}", "Strikethrough", "Ctrl+Shift+X", move || {
                            on_command.run(ToolbarCommand::ToggleStrike); close();
                        })}
                        {menu_icon_action("x\u{2082}", "Subscript", "Ctrl+,", move || {
                            on_command.run(ToolbarCommand::ToggleSubscript); close();
                        })}
                        {menu_icon_action("x\u{00B2}", "Superscript", "Ctrl+.", move || {
                            on_command.run(ToolbarCommand::ToggleSuperscript); close();
                        })}
                        {menu_icon_action("A", "Text Color", "", move || {
                            // Opens color picker from toolbar — close menu.
                            close();
                        })}
                        {menu_icon_action("\u{270F}", "Highlight", "", move || {
                            close();
                        })}
                        {menu_icon_action("</>", "Code", "Ctrl+Shift+K", move || {
                            on_command.run(ToolbarCommand::ToggleCode); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action_sub("Paragraph Style", move || {})}
                        {menu_action("  Paragraph", "", move || {
                            on_command.run(ToolbarCommand::SetParagraph); close();
                        })}
                        {menu_action("  Heading 1", "", move || {
                            on_command.run(ToolbarCommand::SetHeading(1)); close();
                        })}
                        {menu_action("  Heading 2", "", move || {
                            on_command.run(ToolbarCommand::SetHeading(2)); close();
                        })}
                        {menu_action("  Heading 3", "", move || {
                            on_command.run(ToolbarCommand::SetHeading(3)); close();
                        })}
                        {menu_action("  Code Block", "", move || {
                            on_command.run(ToolbarCommand::SetCodeBlock); close();
                        })}
                        {menu_action("  Quote", "", move || {
                            on_command.run(ToolbarCommand::ToggleBlockquote); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action_sub("Alignment", move || {})}
                        {menu_action("  Left", "", move || {
                            on_command.run(ToolbarCommand::SetAlignment("left".to_string())); close();
                        })}
                        {menu_action("  Center", "", move || {
                            on_command.run(ToolbarCommand::SetAlignment("center".to_string())); close();
                        })}
                        {menu_action("  Right", "", move || {
                            on_command.run(ToolbarCommand::SetAlignment("right".to_string())); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action_sub("List", move || {})}
                        {menu_action("  Bulleted List", "", move || {
                            on_command.run(ToolbarCommand::ToggleBulletList); close();
                        })}
                        {menu_action("  Numbered List", "", move || {
                            on_command.run(ToolbarCommand::ToggleOrderedList); close();
                        })}
                        {menu_action("  Checklist", "", move || {
                            on_command.run(ToolbarCommand::ToggleTaskList); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Clear Formatting", "Ctrl+\\", move || {
                            on_command.run(ToolbarCommand::ClearFormatting); close();
                        })}
                        // #140: owner-only edit lock. Hidden for non-owners so
                        // only someone who can actually toggle it sees it.
                        <Show when=move || can_manage_lock.get()>
                            <div class="menu-bar-sep"></div>
                            {menu_toggle("Lock Edits", locked, move || {
                                on_doc_action.run(DocAction::ToggleLockEdits); close();
                            })}
                        </Show>
                    </div>
                </Show>
            </div>
        </div>
    }
}

fn menu_action(
    label: impl Into<String>,
    shortcut: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    let label: String = label.into();
    view! {
        <button class="menu-bar-action" on:click=move |_| on_click()>
            <span class="menu-bar-action-label">{label}</span>
            <Show when=move || !shortcut.is_empty()>
                <span class="menu-bar-action-shortcut">{shortcut}</span>
            </Show>
        </button>
    }
}

/// Menu item with a checkmark toggle.
fn menu_toggle(
    label: &'static str,
    active: ReadSignal<bool>,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button class="menu-bar-action" on:click=move |_| on_click()>
            <span class="menu-bar-action-check">
                {move || if active.get() { "\u{2713}" } else { "" }}
            </span>
            <span
                class="menu-bar-action-label"
                class:menu-bar-action-label-toggle=move || active.get()
            >{label}</span>
        </button>
    }
}

/// Menu item with an icon on the left (as in a typical Format menu).
fn menu_icon_action(
    icon: &'static str,
    label: &'static str,
    shortcut: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button class="menu-bar-action" on:click=move |_| on_click()>
            <span class="menu-bar-action-icon">{icon}</span>
            <span class="menu-bar-action-label">{label}</span>
            <Show when=move || !shortcut.is_empty()>
                <span class="menu-bar-action-shortcut">{shortcut}</span>
            </Show>
        </button>
    }
}

fn menu_action_sub(
    label: &'static str,
    _on_hover: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <div class="menu-bar-action menu-bar-action-disabled">
            <span class="menu-bar-action-label">{label}</span>
            <span class="menu-bar-action-arrow">"\u{25B8}"</span>
        </div>
    }
}
