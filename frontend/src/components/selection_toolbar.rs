// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use crate::editor::state::EditorState;
use super::dom_position;

const TOOLBAR_WIDTH: f64 = 40.0;
const TOOLBAR_HEIGHT: f64 = 32.0;
const TOOLBAR_GAP: f64 = 4.0;

/// Command dispatched from the floating selection toolbar.
#[derive(Debug, Clone)]
pub enum SelectionCommand {
    Comment,
}

/// A floating toolbar that appears above the text selection.
/// Shows a comment button (and future formatting shortcuts).
#[component]
pub fn SelectionToolbar(
    /// Current editor state (to detect non-empty selections).
    editor_state: ReadSignal<Option<EditorState>>,
    /// Scroll tick — forces re-position when the editor container scrolls.
    scroll_tick: ReadSignal<u32>,
    /// Callback when a command is triggered.
    on_command: Callback<SelectionCommand>,
) -> impl IntoView {
    let (visible, set_visible) = signal(false);
    let (left, set_left) = signal(0.0f64);
    let (top, set_top) = signal(0.0f64);

    // Show/hide based on whether there's a non-empty text selection.
    // Position above the selection using the shared positioning library.
    Effect::new(move |_| {
        let _tick = scroll_tick.get();
        let Some(state) = editor_state.get() else {
            set_visible.set(false);
            return;
        };
        if state.selection.empty() {
            set_visible.set(false);
            return;
        }

        let Some(sel_rect) = dom_position::selection_viewport_rect() else {
            set_visible.set(false);
            return;
        };

        let (x, y) = dom_position::place_above(&sel_rect, TOOLBAR_WIDTH, TOOLBAR_HEIGHT, TOOLBAR_GAP);
        set_left.set(x);
        set_top.set(y);
        set_visible.set(true);
    });

    view! {
        <Show when=move || visible.get()>
            <div
                class="selection-toolbar"
                style:left=move || format!("{}px", left.get())
                style:top=move || format!("{}px", top.get())
            >
                <button
                    class="selection-toolbar-btn"
                    title=crate::t!("selection-toolbar-comment")
                    on:mousedown=move |e: web_sys::MouseEvent| {
                        e.prevent_default(); // don't steal focus / deselect
                        on_command.run(SelectionCommand::Comment);
                    }
                >"\u{1F4AC}"</button>
            </div>
        </Show>
    }
}
