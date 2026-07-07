// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

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
                <button class="block-menu-item" title=crate::t!("toolbar-block-paragraph")
                    on:click=move |_| on_command.run(ToolbarCommand::SetParagraph)
                >"\u{00B6}"</button>
                <button class="block-menu-item" title=crate::t!("toolbar-block-heading-1")
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(1))
                >"H1"</button>
                <button class="block-menu-item" title=crate::t!("toolbar-block-heading-2")
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(2))
                >"H2"</button>
                <button class="block-menu-item" title=crate::t!("toolbar-block-heading-3")
                    on:click=move |_| on_command.run(ToolbarCommand::SetHeading(3))
                >"H3"</button>
                <div class="block-menu-divider"></div>
                <button class="block-menu-item" title=crate::t!("toolbar-block-bulleted-list")
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBulletList)
                >"\u{2022}"</button>
                <button class="block-menu-item" title=crate::t!("toolbar-block-numbered-list")
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleOrderedList)
                >"1."</button>
                <button class="block-menu-item" title=crate::t!("toolbar-block-checklist")
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleTaskList)
                >"\u{2610}"</button>
                <div class="block-menu-divider"></div>
                <button class="block-menu-item" title=crate::t!("toolbar-block-blockquote")
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBlockquote)
                >"\u{201C}"</button>
                {catalog_block_menu_items(on_command)}
            </div>
        </Show>
    }
}

/// #148 v2 slice 4 — catalog swap. Renders one block-menu entry
/// per catalog item marked visible in the BlockMenu (HR + every
/// live-app block, per the rule on `CatalogItem::visible_in`).
/// The block-menu's other buttons (headings, lists, blockquote)
/// aren't inserts — they're mark/type toggles on the current
/// block, so they stay hard-coded.
fn catalog_block_menu_items(on_command: Callback<ToolbarCommand>) -> impl IntoView {
    crate::inserts::catalog_for(crate::inserts::InsertSurface::BlockMenu)
        .into_iter()
        .map(|item| {
            let icon = item.icon();
            let label = crate::i18n::translate(item.label_key(), None);
            let cmd = item.command();
            view! {
                <button
                    class="block-menu-item"
                    title=label
                    on:click=move |_| on_command.run(cmd.clone())
                >{icon}</button>
            }
        })
        .collect_view()
}
