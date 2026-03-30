use leptos::prelude::*;

use super::toolbar::ToolbarCommand;

/// Block menu: floating menu that appears when hovering over a paragraph.
/// Offers quick access to block type changes and insertions.
#[component]
pub fn BlockMenu(
    /// Whether the menu is visible.
    visible: ReadSignal<bool>,
    /// Callback to execute a command.
    on_command: Callback<ToolbarCommand>,
    /// Y position (pixels from top of viewport).
    top: ReadSignal<f64>,
) -> impl IntoView {
    view! {
        <Show when=move || visible.get()>
            <div
                class="block-menu"
                style:top=move || format!("{}px", top.get())
            >
                <button class="block-menu-item" title="Paragraph"
                    on:click=move |_| on_command.run(ToolbarCommand::SetParagraph)
                >"\u{00B6}"</button>
                <button class="block-menu-item" title="Heading 1"
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(1))
                >"H1"</button>
                <button class="block-menu-item" title="Heading 2"
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(2))
                >"H2"</button>
                <button class="block-menu-item" title="Heading 3"
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(3))
                >"H3"</button>
                <div class="block-menu-divider"></div>
                <button class="block-menu-item" title="Bullet List"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBulletList)
                >"\u{2022}"</button>
                <button class="block-menu-item" title="Ordered List"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleOrderedList)
                >"1."</button>
                <button class="block-menu-item" title="Task List"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleTaskList)
                >"\u{2610}"</button>
                <div class="block-menu-divider"></div>
                <button class="block-menu-item" title="Blockquote"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBlockquote)
                >"\u{201C}"</button>
                <button class="block-menu-item" title="Horizontal Rule"
                    on:click=move |_| on_command.run(ToolbarCommand::InsertHorizontalRule)
                >"\u{2015}"</button>
            </div>
        </Show>
    }
}
