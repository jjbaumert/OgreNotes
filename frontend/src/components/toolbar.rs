use leptos::prelude::*;

use crate::editor::commands;
use crate::editor::model::{MarkType, NodeType};
use crate::editor::state::EditorState;

/// Formatting toolbar with active state tracking.
#[component]
pub fn Toolbar(
    /// Current editor state (for active button highlighting).
    editor_state: ReadSignal<Option<EditorState>>,
    /// Callback to execute a command.
    on_command: Callback<ToolbarCommand>,
) -> impl IntoView {
    // Helper to check if a mark is active at the current cursor
    let mark_active = move |mark_type: MarkType| -> bool {
        editor_state.get().map(|state| {
            commands::mark_active_at_cursor_public(&state, mark_type)
        }).unwrap_or(false)
    };

    let is_heading = move |level: u8| -> bool {
        editor_state.get().map(|state| {
            commands::heading_level(&state) == Some(level)
        }).unwrap_or(false)
    };

    let is_para = move || -> bool {
        editor_state.get().map(|state| {
            commands::is_paragraph(&state)
        }).unwrap_or(false)
    };

    let in_bullet = move || -> bool {
        editor_state.get().map(|state| {
            commands::is_in_list(&state, NodeType::BulletList)
        }).unwrap_or(false)
    };

    let in_ordered = move || -> bool {
        editor_state.get().map(|state| {
            commands::is_in_list(&state, NodeType::OrderedList)
        }).unwrap_or(false)
    };

    let in_task = move || -> bool {
        editor_state.get().map(|state| {
            commands::is_in_list(&state, NodeType::TaskList)
        }).unwrap_or(false)
    };

    let in_bq = move || -> bool {
        editor_state.get().map(|state| {
            commands::is_in_blockquote(&state)
        }).unwrap_or(false)
    };

    // Prevent toolbar mousedown from stealing focus from the editor.
    // This keeps the browser selection intact when clicking toolbar buttons.
    let on_mousedown = |ev: web_sys::MouseEvent| {
        ev.prevent_default();
    };

    view! {
        <div class="toolbar" on:mousedown=on_mousedown>
            // Undo / Redo
            <div class="toolbar-group">
                <button
                    class="toolbar-btn"
                    title="Undo (Ctrl+Z)"
                    on:click=move |_| on_command.run(ToolbarCommand::Undo)
                >"\u{21B6}"</button>
                <button
                    class="toolbar-btn"
                    title="Redo (Ctrl+Shift+Z)"
                    on:click=move |_| on_command.run(ToolbarCommand::Redo)
                >"\u{21B7}"</button>
            </div>

            <div class="toolbar-separator"></div>

            // Inline marks
            <div class="toolbar-group">
                <button
                    class="toolbar-btn"
                    class:active=move || mark_active(MarkType::Bold)
                    title="Bold (Ctrl+B)"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBold)
                >"B"</button>
                <button
                    class="toolbar-btn"
                    class:active=move || mark_active(MarkType::Italic)
                    title="Italic (Ctrl+I)"
                    style="font-style: italic;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleItalic)
                >"I"</button>
                <button
                    class="toolbar-btn"
                    class:active=move || mark_active(MarkType::Underline)
                    title="Underline (Ctrl+U)"
                    style="text-decoration: underline;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleUnderline)
                >"U"</button>
                <button
                    class="toolbar-btn"
                    class:active=move || mark_active(MarkType::Strike)
                    title="Strikethrough"
                    style="text-decoration: line-through;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleStrike)
                >"S"</button>
                <button
                    class="toolbar-btn"
                    class:active=move || mark_active(MarkType::Code)
                    title="Code (Ctrl+E)"
                    style="font-family: var(--font-mono); font-size: 12px;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleCode)
                >"<>"</button>
            </div>

            <div class="toolbar-separator"></div>

            // Block types
            <div class="toolbar-group">
                <button
                    class="toolbar-btn"
                    class:active=is_para
                    title="Paragraph (Ctrl+Alt+0)"
                    on:click=move |_| on_command.run(ToolbarCommand::SetParagraph)
                >"\u{00B6}"</button>
                <button
                    class="toolbar-btn"
                    class:active=move || is_heading(1)
                    title="Heading 1 (Ctrl+Alt+1)"
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(1))
                >"H1"</button>
                <button
                    class="toolbar-btn"
                    class:active=move || is_heading(2)
                    title="Heading 2 (Ctrl+Alt+2)"
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(2))
                >"H2"</button>
                <button
                    class="toolbar-btn"
                    class:active=move || is_heading(3)
                    title="Heading 3 (Ctrl+Alt+3)"
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(3))
                >"H3"</button>
            </div>

            <div class="toolbar-separator"></div>

            // Lists and blocks
            <div class="toolbar-group">
                <button
                    class="toolbar-btn"
                    class:active=in_bullet
                    title="Bullet List"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBulletList)
                >"\u{2022}"</button>
                <button
                    class="toolbar-btn"
                    class:active=in_ordered
                    title="Ordered List"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleOrderedList)
                >"1."</button>
                <button
                    class="toolbar-btn"
                    class:active=in_task
                    title="Task List"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleTaskList)
                >"\u{2610}"</button>
                <button
                    class="toolbar-btn"
                    class:active=in_bq
                    title="Blockquote"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBlockquote)
                >"\u{201C}"</button>
                <button
                    class="toolbar-btn"
                    title="Horizontal Rule"
                    on:click=move |_| on_command.run(ToolbarCommand::InsertHorizontalRule)
                >"--"</button>
            </div>

            <div class="toolbar-separator"></div>

            <div class="toolbar-group">
                <button
                    class="toolbar-btn"
                    title="Insert Image"
                    on:click=move |_| on_command.run(ToolbarCommand::UploadImage)
                >"\u{1F4F7}"</button>
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
    SetParagraph,
    SetHeading(u8),
    ToggleBulletList,
    ToggleOrderedList,
    ToggleTaskList,
    ToggleBlockquote,
    InsertHorizontalRule,
    UploadImage,
    Undo,
    Redo,
}
