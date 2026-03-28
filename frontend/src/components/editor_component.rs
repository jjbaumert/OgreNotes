use leptos::prelude::*;
use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::editor::commands;
use crate::editor::model::{MarkType, Node};
use crate::editor::plugins::HistoryPlugin;
use crate::editor::state::{EditorState, Transaction};
use crate::editor::view::EditorView;
use crate::editor::yrs_bridge;

use super::toolbar::ToolbarCommand;

/// Props for the editor component.
#[derive(Clone)]
pub struct EditorProps {
    /// Initial document content as yrs bytes. If None, creates an empty doc.
    pub initial_content: Option<Vec<u8>>,
    /// Callback when the document changes (for auto-save).
    pub on_change: Callback<Vec<u8>>,
    /// Callback to report the current editor state (for toolbar).
    pub on_state_change: Callback<EditorState>,
    /// Signal for receiving toolbar commands.
    pub command_signal: ReadSignal<Option<ToolbarCommand>>,
}

/// The main editor component. Wraps EditorView in a Leptos component.
#[component]
pub fn EditorComponent(props: EditorProps) -> impl IntoView {
    let container_ref = NodeRef::<leptos::html::Div>::new();
    let view_ref: Rc<RefCell<Option<EditorView>>> = Rc::new(RefCell::new(None));
    let history_ref: Rc<RefCell<HistoryPlugin>> = Rc::new(RefCell::new(HistoryPlugin::new()));

    // Initialize the editor after the DOM element is mounted
    let view_ref_init = Rc::clone(&view_ref);
    let history_ref_init = Rc::clone(&history_ref);
    let props_clone = props.clone();

    Effect::new(move |_| {
        let Some(container) = container_ref.get() else {
            return;
        };

        // Already initialized
        if view_ref_init.borrow().is_some() {
            return;
        }

        let html_element: web_sys::HtmlElement = container.into();

        // Build initial document from yrs bytes or empty
        let doc = if let Some(ref bytes) = props_clone.initial_content {
            yrs_bridge::ydoc_bytes_to_doc(bytes).unwrap_or_else(|_| Node::empty_doc())
        } else {
            Node::empty_doc()
        };

        let state = EditorState::create_default(doc);
        props_clone.on_state_change.run(state.clone());

        // Use Weak to break the Rc cycle: dispatch -> view_ref -> EditorView -> dispatch
        let view_ref_weak: Weak<RefCell<Option<EditorView>>> = Rc::downgrade(&view_ref_init);
        let history_dispatch = Rc::clone(&history_ref_init);
        let on_change = props_clone.on_change.clone();
        let on_state_change = props_clone.on_state_change.clone();

        let dispatch = move |txn: Transaction| {
            let Some(view_rc) = view_ref_weak.upgrade() else {
                return; // view was dropped
            };
            let view = view_rc.borrow();
            let Some(view) = view.as_ref() else {
                return;
            };

            let old_state = view.state();
            history_dispatch.borrow_mut().record(&txn, &old_state.doc);

            let new_state = old_state.apply(txn);
            view.update_state(new_state.clone());
            on_state_change.run(new_state.clone());

            if new_state.doc != old_state.doc {
                let bytes = yrs_bridge::doc_to_ydoc_bytes(&new_state.doc);
                on_change.run(bytes);
            }
        };

        let editor_view = EditorView::new(html_element, state, dispatch);
        *view_ref_init.borrow_mut() = Some(editor_view);
    });

    // Process toolbar commands reactively
    let view_ref_cmd = Rc::clone(&view_ref);
    let history_ref_cmd = Rc::clone(&history_ref);
    let on_change_cmd = props.on_change.clone();
    let on_state_change_cmd = props.on_state_change.clone();

    Effect::new(move |_| {
        let Some(cmd) = props.command_signal.get() else {
            return;
        };

        let view = view_ref_cmd.borrow();
        let Some(view) = view.as_ref() else {
            return;
        };

        let state = view.state();
        let history = Rc::clone(&history_ref_cmd);
        let on_change = on_change_cmd.clone();
        let on_state_change = on_state_change_cmd.clone();

        let dispatch_fn = |txn: Transaction| {
            let v = view_ref_cmd.borrow();
            let Some(v) = v.as_ref() else { return; };
            let old_state = v.state();
            history.borrow_mut().record(&txn, &old_state.doc);
            let new_state = old_state.apply(txn);
            v.update_state(new_state.clone());
            on_state_change.run(new_state.clone());
            if new_state.doc != old_state.doc {
                let bytes = yrs_bridge::doc_to_ydoc_bytes(&new_state.doc);
                on_change.run(bytes);
            }
        };

        match cmd {
            ToolbarCommand::ToggleBold => {
                commands::toggle_mark(MarkType::Bold, &state, Some(&dispatch_fn));
            }
            ToolbarCommand::ToggleItalic => {
                commands::toggle_mark(MarkType::Italic, &state, Some(&dispatch_fn));
            }
            ToolbarCommand::ToggleUnderline => {
                commands::toggle_mark(MarkType::Underline, &state, Some(&dispatch_fn));
            }
            ToolbarCommand::ToggleStrike => {
                commands::toggle_mark(MarkType::Strike, &state, Some(&dispatch_fn));
            }
            ToolbarCommand::ToggleCode => {
                commands::toggle_mark(MarkType::Code, &state, Some(&dispatch_fn));
            }
            ToolbarCommand::SetParagraph => {
                // MVP: converting heading -> paragraph requires ReplaceAroundStep.
                // For now, this is a no-op.
            }
            ToolbarCommand::SetHeading(level) => {
                commands::set_heading(level, &state, Some(&dispatch_fn));
            }
        }
    });

    view! {
        <div class="editor-container">
            <div
                node_ref=container_ref
                class="editor-content"
            ></div>
        </div>
    }
}
