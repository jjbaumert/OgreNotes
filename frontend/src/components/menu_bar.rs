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
) -> impl IntoView {
    let (open_menu, set_open_menu) = signal::<Option<&'static str>>(None);

    let close = move || set_open_menu.set(None);

    let toggle_menu = move |name: &'static str| {
        if open_menu.get_untracked() == Some(name) {
            set_open_menu.set(None);
        } else {
            set_open_menu.set(Some(name));
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
                    on:mouseenter=move |_| {
                        if open_menu.get_untracked().is_some() {
                            set_open_menu.set(Some("document"));
                        }
                    }
                >"Document"</button>
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
                        <div class="menu-bar-sep"></div>
                        {menu_action_sub("Export", move || {
                            // Submenu would go here; for now show export options inline
                        })}
                        {menu_action("  HTML", "", move || {
                            on_doc_action.run(DocAction::ExportHtml); close();
                        })}
                        {menu_action("  Markdown", "", move || {
                            on_doc_action.run(DocAction::ExportMarkdown); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Print\u{2026}", "Ctrl+P", move || {
                            on_doc_action.run(DocAction::Print); close();
                        })}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Document History\u{2026}", "Ctrl+Shift+H", move || {
                            on_doc_action.run(DocAction::DocumentHistory); close();
                        })}
                        <div class="menu-bar-sep"></div>
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
                    on:mouseenter=move |_| {
                        if open_menu.get_untracked().is_some() {
                            set_open_menu.set(Some("edit"));
                        }
                    }
                >"Edit"</button>
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
                        {menu_action("Find", "Ctrl+F", move || {
                            // Browser find — let the shortcut handle it.
                            close();
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
                    on:mouseenter=move |_| {
                        if open_menu.get_untracked().is_some() {
                            set_open_menu.set(Some("view"));
                        }
                    }
                >"View"</button>
                <Show when=move || open_menu.get() == Some("view")>
                    <div class="menu-bar-backdrop" on:click=move |_| close()></div>
                    <div class="menu-bar-dropdown">
                        {menu_toggle("Show Conversation", conversation_visible, move || {
                            on_doc_action.run(DocAction::ToggleConversation); close();
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
                    on:mouseenter=move |_| {
                        if open_menu.get_untracked().is_some() {
                            set_open_menu.set(Some("insert"));
                        }
                    }
                >"Insert"</button>
                <Show when=move || open_menu.get() == Some("insert")>
                    <div class="menu-bar-backdrop" on:click=move |_| close()></div>
                    <div class="menu-bar-dropdown">
                        {menu_action("Image", "", move || {
                            on_command.run(ToolbarCommand::UploadImage); close();
                        })}
                        {menu_action("Link\u{2026}", "Ctrl+K", move || {
                            close();
                        })}
                        {menu_action("Horizontal Rule", "", move || {
                            on_command.run(ToolbarCommand::InsertHorizontalRule); close();
                        })}
                    </div>
                </Show>
            </div>

            // ─── Format ───
            <div class="menu-bar-item-wrapper">
                <button class="menu-bar-item"
                    class:open=move || open_menu.get() == Some("format")
                    on:click=move |_| toggle_menu("format")
                    on:mouseenter=move |_| {
                        if open_menu.get_untracked().is_some() {
                            set_open_menu.set(Some("format"));
                        }
                    }
                >"Format"</button>
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
                        {menu_action_sub("Alignment", move || {})}
                        {menu_action_sub("List", move || {})}
                        <div class="menu-bar-sep"></div>
                        {menu_action("Clear Formatting", "Ctrl+\\", move || {
                            // TODO: implement clear formatting command
                            close();
                        })}
                    </div>
                </Show>
            </div>
        </div>
    }
}

fn menu_action(
    label: &'static str,
    shortcut: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
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

/// Menu item with an icon on the left (like Quip's Format menu).
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
