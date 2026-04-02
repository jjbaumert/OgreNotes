use leptos::prelude::*;

use crate::editor::commands;
use crate::editor::model::{MarkType, NodeType};
use crate::editor::state::EditorState;

/// Color presets for text color and highlight pickers.
const TEXT_COLORS: &[(&str, &str)] = &[
    ("Red", "#E53935"),
    ("Orange", "#FB8C00"),
    ("Green", "#43A047"),
    ("Blue", "#1E88E5"),
    ("Purple", "#8E24AA"),
    ("Gray", "#757575"),
    ("Black", "#212121"),
];

const HIGHLIGHT_COLORS: &[(&str, &str)] = &[
    ("Yellow", "#FFF176"),
    ("Green", "#A5D6A7"),
    ("Blue", "#90CAF9"),
    ("Pink", "#F48FB1"),
    ("Orange", "#FFCC80"),
    ("Purple", "#CE93D8"),
];

/// Formatting toolbar matching Quip layout.
#[component]
pub fn Toolbar(
    editor_state: ReadSignal<Option<EditorState>>,
    on_command: Callback<ToolbarCommand>,
) -> impl IntoView {
    let mark_active = move |mark_type: MarkType| -> bool {
        editor_state
            .get()
            .map(|state| commands::mark_active_at_cursor_public(&state, mark_type))
            .unwrap_or(false)
    };

    let (text_color_open, set_text_color_open) = signal(false);
    let (highlight_open, set_highlight_open) = signal(false);
    let (block_dropdown_open, set_block_dropdown_open) = signal(false);

    // Determine current block type label and icon for the dropdown.
    let block_type_info = move || -> (&'static str, &'static str) {
        let Some(state) = editor_state.get() else {
            return ("\u{00B6}", "Paragraph");
        };
        if let Some(level) = commands::heading_level(&state) {
            return match level {
                1 => ("H1", "Heading 1"),
                2 => ("H2", "Heading 2"),
                3 => ("H3", "Heading 3"),
                _ => ("H4", "Heading 4"),
            };
        }
        if commands::is_in_list(&state, NodeType::BulletList) {
            return ("\u{2022}", "Bulleted List");
        }
        if commands::is_in_list(&state, NodeType::OrderedList) {
            return ("1.", "Numbered List");
        }
        if commands::is_in_list(&state, NodeType::TaskList) {
            return ("\u{2611}", "Checklist");
        }
        if commands::is_in_blockquote(&state) {
            return ("\u{201C}", "Blockquote");
        }
        if commands::is_in_code_block(&state) {
            return ("</>", "Code Block");
        }
        ("\u{00B6}", "Paragraph")
    };

    let on_mousedown = |ev: web_sys::MouseEvent| {
        ev.prevent_default();
    };

    view! {
        <div class="toolbar" on:mousedown=on_mousedown>
            // ─── Group 1: Undo / Redo ───
            <div class="toolbar-group">
                <button class="toolbar-btn" title="Undo (Ctrl+Z)"
                    on:click=move |_| on_command.run(ToolbarCommand::Undo)
                >"\u{21B6}"</button>
                <button class="toolbar-btn" title="Redo (Ctrl+Shift+Z)"
                    on:click=move |_| on_command.run(ToolbarCommand::Redo)
                >"\u{21B7}"</button>
            </div>

            <div class="toolbar-separator"></div>

            // ─── Group 2: Block type dropdown ───
            <div class="toolbar-group">
                <div class="toolbar-block-dropdown-wrapper">
                    <button class="toolbar-block-dropdown-btn"
                        on:click=move |_| {
                            set_block_dropdown_open.set(!block_dropdown_open.get_untracked());
                            set_text_color_open.set(false);
                            set_highlight_open.set(false);
                        }
                    >
                        <span class="toolbar-block-icon">{move || block_type_info().0}</span>
                        <span class="toolbar-block-label">{move || block_type_info().1}</span>
                        <span class="toolbar-block-arrow">"\u{25BE}"</span>
                    </button>
                    <Show when=move || block_dropdown_open.get()>
                        <div class="toolbar-color-backdrop"
                            on:click=move |_| set_block_dropdown_open.set(false)
                        ></div>
                        <div class="toolbar-block-menu">
                            {block_menu_item("\u{00B6}", "Paragraph", "Ctrl+Alt+0", move || {
                                on_command.run(ToolbarCommand::SetParagraph);
                                set_block_dropdown_open.set(false);
                            })}
                            {block_menu_item("H1", "Heading 1", "Ctrl+Alt+1", move || {
                                on_command.run(ToolbarCommand::SetHeading(1));
                                set_block_dropdown_open.set(false);
                            })}
                            {block_menu_item("H2", "Heading 2", "Ctrl+Alt+2", move || {
                                on_command.run(ToolbarCommand::SetHeading(2));
                                set_block_dropdown_open.set(false);
                            })}
                            {block_menu_item("H3", "Heading 3", "Ctrl+Alt+3", move || {
                                on_command.run(ToolbarCommand::SetHeading(3));
                                set_block_dropdown_open.set(false);
                            })}
                            <div class="toolbar-block-menu-sep"></div>
                            {block_menu_item("\u{2022}", "Bulleted List", "Ctrl+Shift+L", move || {
                                on_command.run(ToolbarCommand::ToggleBulletList);
                                set_block_dropdown_open.set(false);
                            })}
                            {block_menu_item("1.", "Numbered List", "", move || {
                                on_command.run(ToolbarCommand::ToggleOrderedList);
                                set_block_dropdown_open.set(false);
                            })}
                            {block_menu_item("\u{2611}", "Checklist", "", move || {
                                on_command.run(ToolbarCommand::ToggleTaskList);
                                set_block_dropdown_open.set(false);
                            })}
                            <div class="toolbar-block-menu-sep"></div>
                            {block_menu_item("\u{201C}", "Blockquote", "", move || {
                                on_command.run(ToolbarCommand::ToggleBlockquote);
                                set_block_dropdown_open.set(false);
                            })}
                            {block_menu_item("</>", "Code Block", "", move || {
                                on_command.run(ToolbarCommand::SetCodeBlock);
                                set_block_dropdown_open.set(false);
                            })}
                        </div>
                    </Show>
                </div>
            </div>

            <div class="toolbar-separator"></div>

            // ─── Group 3: Inline marks ───
            <div class="toolbar-group">
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Bold)
                    title="Bold (Ctrl+B)" style="font-weight:bold;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBold)
                >"B"</button>
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Italic)
                    title="Italic (Ctrl+I)" style="font-style:italic;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleItalic)
                >"I"</button>
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Underline)
                    title="Underline (Ctrl+U)" style="text-decoration:underline;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleUnderline)
                >"U"</button>
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Strike)
                    title="Strikethrough" style="text-decoration:line-through;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleStrike)
                >"S"</button>

                // Text color (A with color bar)
                <div class="toolbar-color-wrapper">
                    <button class="toolbar-btn" title="Text Color"
                        style="text-decoration:underline; text-decoration-color:#E53935; text-underline-offset:3px;"
                        on:click=move |_| {
                            set_text_color_open.set(!text_color_open.get_untracked());
                            set_highlight_open.set(false);
                        }
                    >"A"</button>
                    <Show when=move || text_color_open.get()>
                        <div class="toolbar-color-backdrop"
                            on:click=move |_| set_text_color_open.set(false)
                        ></div>
                        <div class="toolbar-color-picker">
                            {TEXT_COLORS.iter().map(|(name, hex)| {
                                let hex = hex.to_string();
                                let hex2 = hex.clone();
                                view! {
                                    <button
                                        class="toolbar-color-swatch"
                                        title=*name
                                        style:background-color=hex
                                        on:click=move |_| {
                                            on_command.run(ToolbarCommand::ToggleTextColor(hex2.clone()));
                                            set_text_color_open.set(false);
                                        }
                                    ></button>
                                }
                            }).collect::<Vec<_>>()}
                            <button class="toolbar-color-remove" title="Remove color"
                                on:click=move |_| {
                                    on_command.run(ToolbarCommand::ToggleTextColor(String::new()));
                                    set_text_color_open.set(false);
                                }
                            >"\u{2715}"</button>
                        </div>
                    </Show>
                </div>

                // Highlight (marker icon)
                <div class="toolbar-color-wrapper">
                    <button class="toolbar-btn" title="Highlight"
                        class:active=move || mark_active(MarkType::Highlight)
                        on:click=move |_| {
                            set_highlight_open.set(!highlight_open.get_untracked());
                            set_text_color_open.set(false);
                        }
                    >"\u{270F}"</button>
                    <Show when=move || highlight_open.get()>
                        <div class="toolbar-color-backdrop"
                            on:click=move |_| set_highlight_open.set(false)
                        ></div>
                        <div class="toolbar-color-picker">
                            {HIGHLIGHT_COLORS.iter().map(|(name, hex)| {
                                let hex = hex.to_string();
                                let hex2 = hex.clone();
                                view! {
                                    <button
                                        class="toolbar-color-swatch"
                                        title=*name
                                        style:background-color=hex
                                        on:click=move |_| {
                                            on_command.run(ToolbarCommand::ToggleHighlight(hex2.clone()));
                                            set_highlight_open.set(false);
                                        }
                                    ></button>
                                }
                            }).collect::<Vec<_>>()}
                            <button class="toolbar-color-remove" title="Remove highlight"
                                on:click=move |_| {
                                    on_command.run(ToolbarCommand::ToggleHighlight(String::new()));
                                    set_highlight_open.set(false);
                                }
                            >"\u{2715}"</button>
                        </div>
                    </Show>
                </div>

                // Code
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Code)
                    title="Code (Ctrl+E)" style="font-family:var(--font-mono);font-size:12px;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleCode)
                >"</>"</button>
            </div>

            <div class="toolbar-separator"></div>

            // ─── Group 4: Insert ───
            <div class="toolbar-group">
                <span class="toolbar-label">"Insert:"</span>
                <button class="toolbar-btn" title="Image"
                    on:click=move |_| on_command.run(ToolbarCommand::UploadImage)
                >"\u{1F5BC}"</button>
                <button class="toolbar-btn" title="Link (Ctrl+K)"
                    class:active=move || mark_active(MarkType::Link)
                    on:click=move |_| {
                        if mark_active(MarkType::Link) {
                            on_command.run(ToolbarCommand::ToggleLink(String::new()));
                        } else if let Some(href) = prompt_for_url() {
                            on_command.run(ToolbarCommand::ToggleLink(href));
                        }
                    }
                >"\u{1F517}"</button>
                <button class="toolbar-btn" title="Horizontal Rule"
                    on:click=move |_| on_command.run(ToolbarCommand::InsertHorizontalRule)
                >"\u{2014}"</button>
            </div>

            <div class="toolbar-separator"></div>

            // ─── Group 5: Comment ───
            <div class="toolbar-group">
                <button class="toolbar-btn toolbar-btn-wide" title="Comment (Ctrl+Alt+C)"
                    on:click=move |_| on_command.run(ToolbarCommand::InsertComment)
                >"\u{1F4AC} Comment"</button>
            </div>
        </div>
    }
}

/// Commands that the toolbar can dispatch.
#[derive(Debug, Clone)]
pub enum ToolbarCommand {
    ToggleBold,
    ToggleItalic,
    ToggleUnderline,
    ToggleStrike,
    ToggleCode,
    ToggleTextColor(String),
    ToggleHighlight(String),
    SetParagraph,
    SetHeading(u8),
    ToggleBulletList,
    ToggleOrderedList,
    ToggleTaskList,
    ToggleBlockquote,
    SetCodeBlock,
    InsertHorizontalRule,
    ToggleLink(String),
    UploadImage,
    InsertComment,
    Undo,
    Redo,
}

/// Render a single item in the block type dropdown menu.
fn block_menu_item(
    icon: &'static str,
    label: &'static str,
    shortcut: &'static str,
    on_click: impl Fn() + 'static,
) -> impl IntoView {
    view! {
        <button class="toolbar-block-menu-item" on:click=move |_| on_click()>
            <span class="toolbar-block-menu-icon">{icon}</span>
            <span class="toolbar-block-menu-label">{label}</span>
            <Show when=move || !shortcut.is_empty()>
                <span class="toolbar-block-menu-shortcut">{shortcut}</span>
            </Show>
        </button>
    }
}

fn prompt_for_url() -> Option<String> {
    let window = web_sys::window()?;
    let result = window.prompt_with_message("Enter URL:").ok()??;
    let trimmed = result.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}
