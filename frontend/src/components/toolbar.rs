// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::a11y;
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

/// Formatting toolbar.
#[component]
pub fn Toolbar(
    editor_state: ReadSignal<Option<EditorState>>,
    on_command: Callback<ToolbarCommand>,
    #[prop(default = 0.into())]
    comment_count: Signal<usize>,
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
    // Mobile-only overflow menu: holds the secondary toolbar groups so the
    // primary row stays single-line on narrow screens. CSS hides the
    // trigger on desktop and renders `.toolbar-overflow-content` inline.
    let (overflow_open, set_overflow_open) = signal(false);

    // Tracks how many pixels the soft keyboard occupies at the bottom of the
    // visual viewport. On mobile, responsive.css pins the toolbar to
    // `bottom: var(--mobile-toolbar-offset)`, so when the keyboard opens the
    // toolbar slides up to sit just above it. Zero on desktop.
    let (keyboard_offset, set_keyboard_offset) = signal(0i32);

    Effect::new(move |_| {
        let Some(window) = web_sys::window() else { return };
        let Some(viewport) = window.visual_viewport() else { return };

        let viewport_for_handler = viewport.clone();
        // `Fn`, not `FnMut`: visualViewport `resize`/`scroll` events fire
        // continuously during iOS keyboard animations. Body only reads
        // viewport metrics and writes a signal (`set` takes `&self`), so
        // there's no captured mutable state to protect, and `FnMut` would
        // expose us to the same re-entry panic seen elsewhere.
        let handler = Closure::<dyn Fn()>::wrap(Box::new(move || {
            // Read inner_height each fire so an orientation change picks up
            // the new layout viewport instead of the stale value at setup.
            let window_inner = web_sys::window()
                .and_then(|w| w.inner_height().ok())
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let viewport_height = viewport_for_handler.height();
            let viewport_offset_top = viewport_for_handler.offset_top();
            // Visible bottom edge in document coords:
            //   viewport_offset_top + viewport_height
            // Soft keyboard occupies whatever is left below that.
            let occluded = (window_inner - (viewport_offset_top + viewport_height)).max(0.0);
            set_keyboard_offset.set(occluded.round() as i32);
        }));
        let listener = handler.as_ref().unchecked_ref();
        let _ = viewport.add_event_listener_with_callback("resize", listener);
        let _ = viewport.add_event_listener_with_callback("scroll", listener);
        // Listener lives for the page lifetime (toolbar persists across docs).
        handler.forget();
    });

    // True iff the active document is a spreadsheet — i.e. its
    // first content child is a Table. The block-type dropdown swaps
    // its contents (Paragraph/Headings/lists → Number/Currency/
    // Percent/etc.) based on this flag, since paragraph styles have
    // no meaning inside a spreadsheet cell.
    let is_spreadsheet_doc = move || -> bool {
        editor_state.get().as_ref().is_some_and(state_is_spreadsheet_doc)
    };

    // Determine current block type label and icon for the dropdown.
    // Icon stays a static glyph; label resolves through `t!()` so the
    // active block-type chip honors the current locale.
    let block_type_info = move || -> (&'static str, String) {
        if is_spreadsheet_doc() {
            // We don't track the active cell's number_format here —
            // the toolbar only sees the full doc, not the engine's
            // per-cell style. A static "Format ▾" label is good
            // enough; the menu itself shows the available options.
            return ("\u{0023}", crate::t!("toolbar-block-format"));
        }
        let Some(state) = editor_state.get() else {
            return ("\u{00B6}", crate::t!("toolbar-block-paragraph"));
        };
        if let Some(level) = commands::heading_level(&state) {
            return match level {
                1 => ("H1", crate::t!("toolbar-block-heading-1")),
                2 => ("H2", crate::t!("toolbar-block-heading-2")),
                3 => ("H3", crate::t!("toolbar-block-heading-3")),
                _ => ("H4", crate::t!("toolbar-block-heading-4")),
            };
        }
        if commands::is_in_list(&state, NodeType::BulletList) {
            return ("\u{2022}", crate::t!("toolbar-block-bulleted-list"));
        }
        if commands::is_in_list(&state, NodeType::OrderedList) {
            return ("1.", crate::t!("toolbar-block-numbered-list"));
        }
        if commands::is_in_list(&state, NodeType::TaskList) {
            return ("\u{2611}", crate::t!("toolbar-block-checklist"));
        }
        if commands::is_in_blockquote(&state) {
            return ("\u{201C}", crate::t!("toolbar-block-blockquote"));
        }
        if commands::is_in_code_block(&state) {
            return ("</>", crate::t!("toolbar-block-code-block"));
        }
        ("\u{00B6}", crate::t!("toolbar-block-paragraph"))
    };

    let on_mousedown = |ev: web_sys::MouseEvent| {
        ev.prevent_default();
    };

    view! {
        <div
            class="toolbar"
            role="toolbar"
            aria-label=crate::t!("a11y-toolbar-label")
            on:mousedown=on_mousedown
            style:--mobile-toolbar-offset=move || format!("{}px", keyboard_offset.get())
        >
            // ─── Group 1: Undo / Redo ───
            <div class="toolbar-group" role="group" aria-label=crate::t!("a11y-toolbar-group-undo")>
                <button class="toolbar-btn"
                    title=crate::t!("toolbar-undo")
                    aria-label=crate::t!("toolbar-undo")
                    on:click=move |_| on_command.run(ToolbarCommand::Undo)
                >"\u{21B6}"</button>
                <button class="toolbar-btn"
                    title=crate::t!("toolbar-redo")
                    aria-label=crate::t!("toolbar-redo")
                    on:click=move |_| on_command.run(ToolbarCommand::Redo)
                >"\u{21B7}"</button>
            </div>

            <div class="toolbar-separator"></div>

            // ─── Group 2: Block type dropdown ───
            <div class="toolbar-group" role="group" aria-label=crate::t!("a11y-toolbar-group-block-type")>
                <div class="toolbar-block-dropdown-wrapper">
                    <button class="toolbar-block-dropdown-btn"
                        on:click=move |_| {
                            set_block_dropdown_open.set(!block_dropdown_open.get_untracked());
                            a11y::defer(move || set_text_color_open.set(false));
                            a11y::defer(move || set_highlight_open.set(false));
                        }
                    >
                        <span class="toolbar-block-icon">{move || block_type_info().0}</span>
                        <span class="toolbar-block-label">{move || block_type_info().1}</span>
                        <span class="toolbar-block-arrow">"\u{25BE}"</span>
                    </button>
                    <Show when=move || block_dropdown_open.get()>
                        <div class="toolbar-color-backdrop"
                            on:click=move |_| a11y::defer(move || set_block_dropdown_open.set(false))
                        ></div>
                        <div class="toolbar-block-menu">
                            {move || if is_spreadsheet_doc() {
                                // Spreadsheet-specific menu: number formats
                                // for the active cell selection. Each item
                                // dispatches `SetNumberFormat(<key>)`; the
                                // engine's `format_number` recognises the
                                // same keys (`""` clears, "currency",
                                // "percent", "decimal2", etc.).
                                view! {
                                    <>
                                        {block_menu_item("123", crate::t!("toolbar-num-general"), "", move || {
                                            on_command.run(ToolbarCommand::SetNumberFormat(String::new()));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        <div class="toolbar-block-menu-sep"></div>
                                        {block_menu_item("0", crate::t!("toolbar-num-integer"), "", move || {
                                            on_command.run(ToolbarCommand::SetNumberFormat("integer".into()));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("0.0", crate::t!("toolbar-num-decimal-1"), "", move || {
                                            on_command.run(ToolbarCommand::SetNumberFormat("decimal1".into()));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("0.00", crate::t!("toolbar-num-decimal-2"), "", move || {
                                            on_command.run(ToolbarCommand::SetNumberFormat("decimal2".into()));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("1,234", crate::t!("toolbar-num-thousands"), "", move || {
                                            on_command.run(ToolbarCommand::SetNumberFormat("comma".into()));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        <div class="toolbar-block-menu-sep"></div>
                                        {block_menu_item("$", crate::t!("toolbar-num-currency-usd"), "", move || {
                                            on_command.run(ToolbarCommand::SetNumberFormat("currency".into()));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("\u{20AC}", crate::t!("toolbar-num-currency-eur"), "", move || {
                                            on_command.run(ToolbarCommand::SetNumberFormat("eur".into()));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("%", crate::t!("toolbar-num-percent"), "", move || {
                                            on_command.run(ToolbarCommand::SetNumberFormat("percent".into()));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                    </>
                                }.into_any()
                            } else {
                                view! {
                                    <>
                                        {block_menu_item("\u{00B6}", crate::t!("toolbar-block-paragraph"), "Ctrl+Alt+0", move || {
                                            on_command.run(ToolbarCommand::SetParagraph);
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("H1", crate::t!("toolbar-block-heading-1"), "Ctrl+Alt+1", move || {
                                            on_command.run(ToolbarCommand::SetHeading(1));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("H2", crate::t!("toolbar-block-heading-2"), "Ctrl+Alt+2", move || {
                                            on_command.run(ToolbarCommand::SetHeading(2));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("H3", crate::t!("toolbar-block-heading-3"), "Ctrl+Alt+3", move || {
                                            on_command.run(ToolbarCommand::SetHeading(3));
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        <div class="toolbar-block-menu-sep"></div>
                                        {block_menu_item("\u{2022}", crate::t!("toolbar-block-bulleted-list"), "Ctrl+Shift+L", move || {
                                            on_command.run(ToolbarCommand::ToggleBulletList);
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("1.", crate::t!("toolbar-block-numbered-list"), "", move || {
                                            on_command.run(ToolbarCommand::ToggleOrderedList);
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("\u{2611}", crate::t!("toolbar-block-checklist"), "", move || {
                                            on_command.run(ToolbarCommand::ToggleTaskList);
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        <div class="toolbar-block-menu-sep"></div>
                                        {block_menu_item("\u{201C}", crate::t!("toolbar-block-blockquote"), "", move || {
                                            on_command.run(ToolbarCommand::ToggleBlockquote);
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                        {block_menu_item("</>", crate::t!("toolbar-block-code-block"), "", move || {
                                            on_command.run(ToolbarCommand::SetCodeBlock);
                                            a11y::defer(move || set_block_dropdown_open.set(false));
                                        })}
                                    </>
                                }.into_any()
                            }}
                        </div>
                    </Show>
                </div>
            </div>

            <div class="toolbar-separator"></div>

            // ─── Group 3a: Essential inline marks (always visible) ───
            <div class="toolbar-group" role="group" aria-label=crate::t!("a11y-toolbar-group-inline")>
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Bold)
                    title=crate::t!("toolbar-bold") style="font-weight:bold;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleBold)
                >"B"</button>
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Italic)
                    title=crate::t!("toolbar-italic") style="font-style:italic;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleItalic)
                >"I"</button>
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Underline)
                    title=crate::t!("toolbar-underline") style="text-decoration:underline;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleUnderline)
                >"U"</button>
            </div>

            // ─── Overflow groups: render inline on desktop (display: contents),
            //     collapse into a `⋯` drop-up panel on mobile. ───
            <div class="toolbar-overflow-content" class:is-open=move || overflow_open.get()>

            <div class="toolbar-separator"></div>

            // ─── Group 3b: Secondary inline marks ───
            <div class="toolbar-group" role="group" aria-label=crate::t!("a11y-toolbar-group-inline")>
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Strike)
                    title=crate::t!("toolbar-strikethrough") style="text-decoration:line-through;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleStrike)
                >"S"</button>

                // #143: subscript / superscript.
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Subscript)
                    title=crate::t!("toolbar-subscript")
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleSubscript)
                >"x"<sub>"2"</sub></button>
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Superscript)
                    title=crate::t!("toolbar-superscript")
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleSuperscript)
                >"x"<sup>"2"</sup></button>

                // Text color (A with color bar)
                <div class="toolbar-color-wrapper">
                    <button class="toolbar-btn" title=crate::t!("toolbar-text-color")
                        style="text-decoration:underline; text-decoration-color:#E53935; text-underline-offset:3px;"
                        on:click=move |_| {
                            set_text_color_open.set(!text_color_open.get_untracked());
                            a11y::defer(move || set_highlight_open.set(false));
                        }
                    >"A"</button>
                    <Show when=move || text_color_open.get()>
                        <div class="toolbar-color-backdrop"
                            on:click=move |_| a11y::defer(move || set_text_color_open.set(false))
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
                                            a11y::defer(move || set_text_color_open.set(false));
                                        }
                                    ></button>
                                }
                            }).collect::<Vec<_>>()}
                            <button class="toolbar-color-remove" title=crate::t!("toolbar-remove-color")
                                on:click=move |_| {
                                    on_command.run(ToolbarCommand::ToggleTextColor(String::new()));
                                    a11y::defer(move || set_text_color_open.set(false));
                                }
                            >"\u{2715}"</button>
                        </div>
                    </Show>
                </div>

                // Highlight (marker icon)
                <div class="toolbar-color-wrapper">
                    <button class="toolbar-btn" title=crate::t!("toolbar-highlight")
                        class:active=move || mark_active(MarkType::Highlight)
                        on:click=move |_| {
                            set_highlight_open.set(!highlight_open.get_untracked());
                            a11y::defer(move || set_text_color_open.set(false));
                        }
                    >"\u{270F}"</button>
                    <Show when=move || highlight_open.get()>
                        <div class="toolbar-color-backdrop"
                            on:click=move |_| a11y::defer(move || set_highlight_open.set(false))
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
                                            a11y::defer(move || set_highlight_open.set(false));
                                        }
                                    ></button>
                                }
                            }).collect::<Vec<_>>()}
                            <button class="toolbar-color-remove" title=crate::t!("toolbar-remove-highlight")
                                on:click=move |_| {
                                    on_command.run(ToolbarCommand::ToggleHighlight(String::new()));
                                    a11y::defer(move || set_highlight_open.set(false));
                                }
                            >"\u{2715}"</button>
                        </div>
                    </Show>
                </div>

                // Code
                <button class="toolbar-btn" class:active=move || mark_active(MarkType::Code)
                    title=crate::t!("toolbar-code") style="font-family:var(--font-mono);font-size:12px;"
                    on:click=move |_| on_command.run(ToolbarCommand::ToggleCode)
                >"</>"</button>
            </div>

            <div class="toolbar-separator"></div>

            // ─── Group: Alignment (#134) ───
            <div class="toolbar-group" role="group" aria-label=crate::t!("a11y-toolbar-group-align")>
                <button class="toolbar-btn"
                    title=crate::t!("toolbar-align-left") aria-label=crate::t!("toolbar-align-left")
                    on:click=move |_| on_command.run(ToolbarCommand::SetAlignment("left".to_string()))
                >"\u{21E4}"</button>
                <button class="toolbar-btn"
                    title=crate::t!("toolbar-align-center") aria-label=crate::t!("toolbar-align-center")
                    on:click=move |_| on_command.run(ToolbarCommand::SetAlignment("center".to_string()))
                >"\u{2194}"</button>
                <button class="toolbar-btn"
                    title=crate::t!("toolbar-align-right") aria-label=crate::t!("toolbar-align-right")
                    on:click=move |_| on_command.run(ToolbarCommand::SetAlignment("right".to_string()))
                >"\u{21E5}"</button>
            </div>

            <div class="toolbar-separator"></div>

            // ─── Group 4: Insert ───
            //
            // #148 v2 slice 4 — catalog swap. Block-shape / media /
            // live-app entries iterate from `INSERT_CATALOG`; Link
            // and Embed remain hard-coded because they have
            // dispatch flows that don't fit the zero-arg
            // `InsertEntry::command` shape (Link prompts for a
            // URL, Embed does an async resolve round-trip).
            <div class="toolbar-group" role="group" aria-label=crate::t!("a11y-toolbar-group-insert")>
                <span class="toolbar-label">{crate::t!("toolbar-insert-label")}</span>
                {crate::inserts::catalog_for(crate::inserts::InsertSurface::Toolbar)
                    .into_iter()
                    .map(|item| {
                        let icon = item.icon();
                        let label = crate::i18n::translate(item.label_key(), None);
                        let cmd = item.command();
                        view! {
                            <button
                                class="toolbar-btn"
                                title=label
                                on:click=move |_| on_command.run(cmd.clone())
                            >{icon}</button>
                        }
                    })
                    .collect_view()}
                <button class="toolbar-btn" title=crate::t!("toolbar-link")
                    class:active=move || mark_active(MarkType::Link)
                    on:click=move |_| {
                        if mark_active(MarkType::Link) {
                            on_command.run(ToolbarCommand::ToggleLink(String::new()));
                        } else if let Some(href) = prompt_for_url() {
                            on_command.run(ToolbarCommand::ToggleLink(href));
                        }
                    }
                >"\u{1F517}"</button>
                <button class="toolbar-btn" title=crate::t!("toolbar-embed")
                    on:click=move |_| {
                        let Some(url) = prompt_for_url() else { return };
                        if url.is_empty() { return; }
                        leptos::task::spawn_local(async move {
                            match crate::api::documents::resolve_embed(&url).await {
                                Ok(resp) => {
                                    on_command.run(ToolbarCommand::InsertEmbed {
                                        url: resp.src,
                                        provider: resp.provider,
                                        height: resp.height,
                                        title: None,
                                    });
                                }
                                Err(e) => {
                                    web_sys::console::warn_1(
                                        &format!("embed: {e}").into(),
                                    );
                                }
                            }
                        });
                    }
                >"\u{1F4FA}"</button>
            </div>

            </div>  // ─── /.toolbar-overflow-content ───

            // Spacer pushes Comment + overflow trigger to the right edge.
            <div class="toolbar-spacer"></div>

            // ─── Group 6: Comment (always visible) ───
            <div class="toolbar-group" role="group" aria-label=crate::t!("a11y-toolbar-group-block")>
                <button class="toolbar-btn toolbar-btn-wide" title=crate::t!("toolbar-comment")
                    on:click=move |_| on_command.run(ToolbarCommand::InsertComment)
                >
                    {move || {
                        let count = comment_count.get();
                        if count > 0 {
                            format!("\u{1F4AC} {count}")
                        } else {
                            format!("\u{1F4AC} {}", crate::t!("toolbar-comment-label"))
                        }
                    }}
                </button>
            </div>

            // ─── Overflow trigger (mobile only; CSS hides on desktop) ───
            <Show when=move || overflow_open.get()>
                <div
                    class="toolbar-overflow-backdrop"
                    on:click=move |_| a11y::defer(move || set_overflow_open.set(false))
                ></div>
            </Show>
            <button
                class="toolbar-btn toolbar-overflow-trigger"
                title=crate::t!("toolbar-more")
                aria-label=crate::t!("toolbar-aria-more")
                aria-expanded=move || overflow_open.get().to_string()
                on:click=move |_| set_overflow_open.update(|v| *v = !*v)
            >"\u{22EF}"</button>
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
    ToggleSubscript,
    ToggleSuperscript,
    ToggleTextColor(String),
    ToggleHighlight(String),
    SetParagraph,
    SetHeading(u8),
    ToggleBulletList,
    ToggleOrderedList,
    ToggleTaskList,
    ToggleBlockquote,
    SetCodeBlock,
    /// #134: set the current block's alignment ("left" / "center" /
    /// "right"). Reuses `commands::set_alignment`, the same path the
    /// right-click menu's Alignment submenu already drives.
    SetAlignment(String),
    /// #134: strip all inline marks from the selection ("Clear
    /// Formatting").
    ClearFormatting,
    /// #147: select a model range (find/replace navigation).
    SelectRange { from: usize, to: usize },
    /// #147: replace a model range with text (single find/replace).
    ReplaceRange { from: usize, to: usize, text: String },
    /// #147: replace every range with text in one transaction (Replace All).
    ReplaceAll { matches: Vec<(usize, usize)>, text: String },
    InsertHorizontalRule,
    InsertTable,
    ToggleLink(String),
    /// #148: insert an @-mention document link — replace the `@query` trigger
    /// range `[from, to)` with `title`, linked to `href` (e.g. `/d/<id>`), in
    /// one transaction.
    InsertDocLink {
        from: usize,
        to: usize,
        title: String,
        href: String,
    },
    /// #148: insert an @-user mention. Replace `[from, to)` — the
    /// `@query` trigger — with `display`, carrying a `Mention`
    /// mark with the user's id, in one transaction.
    InsertUserMention {
        from: usize,
        to: usize,
        display: String,
        user_id: String,
    },
    /// #148 `@ask` — open the shared `AskDialog` pre-filled with
    /// `prompt`. The `insert_range` records the `@ask <prompt>`
    /// trigger's `[from, to)` in the doc; when the dialog closes
    /// via "Insert into document", the page issues an
    /// `InsertAiText` for that range with the assistant's answer.
    /// If the user cancels the dialog, no insertion happens.
    ///
    /// `mode` selects Agent (with RAG tools — the free-form
    /// `@ask <question>` path) vs Direct (no tools — the
    /// directive-wrapper path where the composed prompt already
    /// carries its own source content).
    ///
    /// `hidden_suffix`, when `Some`, is appended to whatever the
    /// user submits from the dialog input — invisible to the
    /// user. Used to keep the current-doc source text out of the
    /// visible input for directive wrappers (@summarize on a
    /// 5-page doc otherwise floods the input). The visible
    /// `prompt` shows a short instruction ("Summarize this
    /// document concisely"); the hidden suffix carries the
    /// scope-guard + source text.
    OpenAskDialog {
        prompt: String,
        insert_range: (usize, usize),
        mode: crate::api::ask::AskMode,
        hidden_suffix: Option<String>,
    },
    /// #148: land AI-generated text at `[from, to)`. Companion to
    /// `OpenAskDialog`; called from the AskDialog's on-insert
    /// callback after the stream is Done.
    InsertAiText {
        from: usize,
        to: usize,
        text: String,
    },
    UploadImage,
    /// M-P6 piece B: insert a sandboxed third-party embed. The
    /// String fields are populated by the toolbar's insert flow
    /// after the backend `/embeds/resolve` returns the rewritten
    /// iframe-ready URL + provider tag. The frontend never
    /// constructs an Embed cmd from raw user input — the resolve
    /// round-trip is the only path.
    InsertEmbed {
        url: String,
        provider: String,
        height: u32,
        title: Option<String>,
    },
    /// #136 — insert a live-app block by registry id. The id is one
    /// of the kebab-case strings returned by
    /// `editor::blocks::LiveAppBlockInsert::id()`; dispatch happens
    /// in `editor_component::run_command` which looks up the entry
    /// and delegates to `commands::insert_live_app`.
    InsertLiveApp(&'static str),
    InsertComment,
    Undo,
    Redo,
    /// Spreadsheet-only: set the active selection's number format.
    /// The string maps to `format_number` in eval.rs (`""` clears
    /// the format / reverts to general; `"currency"`, `"percent"`,
    /// etc. match the existing well-known keys).
    SetNumberFormat(String),
    /// Mentions spec §5 (Task 5) — silently refresh a `DocMention`
    /// chip's cached `title`/`snippet` attrs after the per-viewer
    /// degradation overlay's batch resolve found fresher values.
    /// Editable sessions only; the caller (`mention_overlay`) has
    /// already confirmed the values differ from what's cached.
    /// Routes through `commands::update_doc_mention_attrs`, which
    /// tags the transaction `history: skip` — this never creates an
    /// undo entry.
    UpdateDocMentionAttrs {
        node_block_id: String,
        title: String,
        snippet: String,
    },
}

/// Render a single item in the block type dropdown menu.
///
/// `label` is owned `String` so callers can pass a `t!()` result
/// (which translates at call time and returns `String`); `icon` and
/// `shortcut` remain `&'static str` since they are glyphs / key
/// chords that don't localize.
fn block_menu_item(
    icon: &'static str,
    label: String,
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
    let result = window.prompt_with_message(&crate::t!("toolbar-prompt-url")).ok()??;
    let trimmed = result.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// True iff `state` represents a spreadsheet document — i.e. its
/// first content child is a Table. Extracted from the in-Toolbar
/// closure so the predicate can be unit-tested without standing up
/// the Leptos signal layer.
pub(crate) fn state_is_spreadsheet_doc(state: &EditorState) -> bool {
    state.doc.child(0).and_then(|n| n.node_type()) == Some(NodeType::Table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::{Fragment, Node};
    use crate::editor::state::EditorState;

    fn doc_with_first_child(first: Node) -> EditorState {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![first]),
        );
        EditorState::create_default(doc)
    }

    #[test]
    fn state_is_spreadsheet_doc_true_when_first_child_is_table() {
        let state = doc_with_first_child(
            Node::element_with_content(NodeType::Table, Fragment::empty()),
        );
        assert!(state_is_spreadsheet_doc(&state));
    }

    #[test]
    fn state_is_spreadsheet_doc_false_for_document_doc() {
        let state = doc_with_first_child(
            Node::element_with_content(NodeType::Paragraph, Fragment::empty()),
        );
        assert!(!state_is_spreadsheet_doc(&state));
    }

    #[test]
    fn state_is_spreadsheet_doc_false_for_empty_doc() {
        let doc = Node::element_with_content(NodeType::Doc, Fragment::empty());
        let state = EditorState::create_default(doc);
        assert!(!state_is_spreadsheet_doc(&state));
    }
}
