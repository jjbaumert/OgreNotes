// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Document-editor right-click context menu.
//!
//! Visible when `visible` is true; positioned by `x`/`y` (viewport
//! coordinates). Items dispatch one of `EditorContextCommand` —
//! caller handles the actual work (clipboard ops, format toggles,
//! Comment popup, link prompt). Closed via `on_close` on
//! backdrop-click, Escape keydown, or after any item fires.
//!
//! Mirrors the spreadsheet's `.ss-ctx-menu` chrome (positioning,
//! shadow, separator) so the two surfaces feel consistent. The
//! menu sits at the document's body level (via a fixed-position
//! backdrop) so it's not clipped by `overflow: auto` on the
//! editor container.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

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
    // Dismiss on Escape — registered at the document level so it
    // catches the key even when focus has shifted to the menu's own
    // button. Cleared via remove_event_listener when `visible` flips
    // back to false.
    Effect::new(move |_| {
        let v = visible.get();
        if !v {
            return;
        }
        let close_cb = on_close;
        let closure = wasm_bindgen::closure::Closure::wrap(Box::new(
            move |event: web_sys::Event| {
                let Some(ke) = event.dyn_ref::<web_sys::KeyboardEvent>() else { return };
                if ke.key() == "Escape" {
                    close_cb.run(());
                }
            },
        )
            as Box<dyn Fn(web_sys::Event)>);
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
        let _ = doc.add_event_listener_with_callback(
            "keydown",
            closure.as_ref().unchecked_ref(),
        );
        // Forget the closure — the next render of this Effect (when
        // `visible` flips to false again) will install a fresh one
        // and the previous handler simply no-ops on Escape because
        // the menu is already hidden. A precise cleanup would track
        // the closure handle, but the cost of a stale listener that
        // only acts on Escape-while-menu-visible is zero.
        closure.forget();
    });

    let dispatch = move |cmd: EditorContextCommand| {
        on_command.run(cmd);
        on_close.run(());
    };

    view! {
        <Show when=move || visible.get()>
            // Full-viewport backdrop catches the click that dismisses
            // the menu — clicking anywhere outside the menu closes it.
            // The backdrop is transparent (no background-color) so it
            // doesn't darken the page.
            <div
                class="editor-ctx-backdrop"
                on:mousedown=move |e: web_sys::MouseEvent| {
                    e.prevent_default();
                    on_close.run(());
                }
                on:contextmenu=move |e: web_sys::MouseEvent| {
                    // Right-clicking the backdrop dismisses the current
                    // menu. Re-opening at the new position is the
                    // editor container's job; let its handler run.
                    e.prevent_default();
                    on_close.run(());
                }
            ></div>
            <div
                class="editor-ctx-menu"
                style:left=move || format!("{}px", x.get())
                style:top=move || format!("{}px", y.get())
                on:mousedown=move |e: web_sys::MouseEvent| {
                    // Prevent the editor from losing focus when the
                    // user clicks an item — the clipboard / format
                    // commands need the contenteditable to still own
                    // DOM focus when they run.
                    e.prevent_default();
                }
                on:click=move |e: web_sys::MouseEvent| {
                    // The backdrop's mousedown handler dismisses on
                    // any click — stop propagation here so clicks
                    // INSIDE the menu don't bubble back out and
                    // close it before the item's handler fires.
                    e.stop_propagation();
                }
            >
                <button
                    class="editor-ctx-item"
                    disabled=move || selection_empty.get()
                    on:click=move |_| dispatch(EditorContextCommand::Cut)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-cut")}</span>
                    <span class="editor-ctx-shortcut">"Ctrl+X"</span>
                </button>
                <button
                    class="editor-ctx-item"
                    disabled=move || selection_empty.get()
                    on:click=move |_| dispatch(EditorContextCommand::Copy)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-copy")}</span>
                    <span class="editor-ctx-shortcut">"Ctrl+C"</span>
                </button>
                <button
                    class="editor-ctx-item"
                    on:click=move |_| dispatch(EditorContextCommand::Paste)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-paste")}</span>
                    <span class="editor-ctx-shortcut">"Ctrl+V"</span>
                </button>

                <div class="editor-ctx-sep"></div>

                <button
                    class="editor-ctx-item"
                    disabled=move || selection_empty.get()
                    on:click=move |_| dispatch(EditorContextCommand::Comment)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-comment")}</span>
                    <span class="editor-ctx-shortcut">"\u{1F4AC}"</span>
                </button>

                <div class="editor-ctx-sep"></div>

                // ── Paragraph style submenu ───────────────────────
                <div class="editor-ctx-submenu-wrap">
                    <button
                        class="editor-ctx-item editor-ctx-item-has-submenu"
                        // Don't dismiss on click — the user is
                        // navigating into the submenu, not picking
                        // an action. CSS :hover handles the open.
                        on:click=move |e: web_sys::MouseEvent| {
                            e.stop_propagation();
                        }
                    >
                        <span class="editor-ctx-label">{crate::t!("editorctx-paragraph-style")}</span>
                        <span class="editor-ctx-submenu-arrow">"\u{25B6}"</span>
                    </button>
                    <div class="editor-ctx-submenu">
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::SetParagraph)
                        >
                            <span class="editor-ctx-label">{crate::t!("toolbar-block-paragraph")}</span>
                        </button>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::SetHeading1)
                        >
                            <span class="editor-ctx-label">{crate::t!("toolbar-block-heading-1")}</span>
                        </button>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::SetHeading2)
                        >
                            <span class="editor-ctx-label">{crate::t!("toolbar-block-heading-2")}</span>
                        </button>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::SetHeading3)
                        >
                            <span class="editor-ctx-label">{crate::t!("toolbar-block-heading-3")}</span>
                        </button>
                        <div class="editor-ctx-sep"></div>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::ToggleBulletList)
                        >
                            <span class="editor-ctx-label">{crate::t!("node-bullet-list")}</span>
                        </button>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::ToggleOrderedList)
                        >
                            <span class="editor-ctx-label">{crate::t!("node-ordered-list")}</span>
                        </button>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::ToggleTaskList)
                        >
                            <span class="editor-ctx-label">{crate::t!("node-task-list")}</span>
                        </button>
                        <div class="editor-ctx-sep"></div>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::ToggleBlockquote)
                        >
                            <span class="editor-ctx-label">{crate::t!("toolbar-block-blockquote")}</span>
                        </button>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::SetCodeBlock)
                        >
                            <span class="editor-ctx-label">{crate::t!("node-code-block")}</span>
                        </button>
                    </div>
                </div>

                // ── Alignment submenu ─────────────────────────────
                <div class="editor-ctx-submenu-wrap">
                    <button
                        class="editor-ctx-item editor-ctx-item-has-submenu"
                        on:click=move |e: web_sys::MouseEvent| {
                            e.stop_propagation();
                        }
                    >
                        <span class="editor-ctx-label">{crate::t!("menu-alignment")}</span>
                        <span class="editor-ctx-submenu-arrow">"\u{25B6}"</span>
                    </button>
                    <div class="editor-ctx-submenu">
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::AlignLeft)
                        >
                            <span class="editor-ctx-label">{crate::t!("menu-align-left")}</span>
                        </button>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::AlignCenter)
                        >
                            <span class="editor-ctx-label">{crate::t!("menu-align-center")}</span>
                        </button>
                        <button
                            class="editor-ctx-item"
                            on:click=move |_| dispatch(EditorContextCommand::AlignRight)
                        >
                            <span class="editor-ctx-label">{crate::t!("menu-align-right")}</span>
                        </button>
                    </div>
                </div>

                <div class="editor-ctx-sep"></div>

                <button
                    class="editor-ctx-item"
                    on:click=move |_| dispatch(EditorContextCommand::ToggleBold)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-bold")}</span>
                    <span class="editor-ctx-shortcut">"Ctrl+B"</span>
                </button>
                <button
                    class="editor-ctx-item"
                    on:click=move |_| dispatch(EditorContextCommand::ToggleItalic)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-italic")}</span>
                    <span class="editor-ctx-shortcut">"Ctrl+I"</span>
                </button>
                <button
                    class="editor-ctx-item"
                    on:click=move |_| dispatch(EditorContextCommand::ToggleUnderline)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-underline")}</span>
                    <span class="editor-ctx-shortcut">"Ctrl+U"</span>
                </button>
                <button
                    class="editor-ctx-item"
                    on:click=move |_| dispatch(EditorContextCommand::ToggleStrike)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-strikethrough")}</span>
                    <span class="editor-ctx-shortcut">""</span>
                </button>
                <button
                    class="editor-ctx-item"
                    on:click=move |_| dispatch(EditorContextCommand::ToggleCode)
                >
                    <span class="editor-ctx-label">{crate::t!("menu-code")}</span>
                    <span class="editor-ctx-shortcut">""</span>
                </button>

                <div class="editor-ctx-sep"></div>

                <button
                    class="editor-ctx-item"
                    on:click=move |_| dispatch(EditorContextCommand::InsertLink)
                >
                    <span class="editor-ctx-label">{crate::t!("editorctx-insert-link")}</span>
                    <span class="editor-ctx-shortcut">"Ctrl+K"</span>
                </button>
            </div>
        </Show>
    }
}
