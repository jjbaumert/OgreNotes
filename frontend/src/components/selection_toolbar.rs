use leptos::prelude::*;
use crate::editor::state::EditorState;

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
    // Position above the selection using the browser's Selection API.
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

        // Get the browser selection range to position the toolbar
        let Some(window) = web_sys::window() else {
            set_visible.set(false);
            return;
        };
        let Some(sel) = window.get_selection().ok().flatten() else {
            set_visible.set(false);
            return;
        };
        if sel.range_count() == 0 {
            set_visible.set(false);
            return;
        }
        let Ok(range) = sel.get_range_at(0) else {
            set_visible.set(false);
            return;
        };
        let rect = range.get_bounding_client_rect();
        if rect.width() < 1.0 {
            set_visible.set(false);
            return;
        }

        // Position above the selection, centered horizontally
        let toolbar_width = 40.0; // approximate width of the toolbar
        let x = rect.left() + (rect.width() / 2.0) - (toolbar_width / 2.0);
        let y = rect.top() - 36.0; // 36px above the selection

        set_left.set(x.max(4.0));
        set_top.set(y.max(4.0));
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
                    title="Comment on selection"
                    on:mousedown=move |e: web_sys::MouseEvent| {
                        e.prevent_default(); // don't steal focus / deselect
                        on_command.run(SelectionCommand::Comment);
                    }
                >"\u{1F4AC}"</button>
            </div>
        </Show>
    }
}
