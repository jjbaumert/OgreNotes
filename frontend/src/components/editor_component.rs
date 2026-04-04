use leptos::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::blobs;
use crate::editor::commands;
use crate::editor::model::{Fragment, MarkType, Node, NodeType, Slice};
use crate::editor::plugins::HistoryPlugin;
use crate::editor::selection::Selection;
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
    /// Signal for receiving remote document updates from collaborators.
    /// When set, the editor replaces its document content and re-renders.
    pub remote_state: ReadSignal<Option<EditorState>>,
    /// Document ID (needed for blob upload).
    pub doc_id: String,
}

/// Apply a transaction to the editor view and notify callbacks.
/// If `history` is provided, records the transaction for undo/redo.
fn apply_and_notify(
    view: &EditorView,
    txn: Transaction,
    history: Option<&Rc<RefCell<HistoryPlugin>>>,
    on_change: &Callback<Vec<u8>>,
    on_state_change: &Callback<EditorState>,
) {
    let old_state = view.state();
    if let Some(h) = history {
        h.borrow_mut().record(&txn, &old_state.doc);
    }
    let new_state = old_state.apply(txn);
    view.update_state(new_state.clone());
    on_state_change.run(new_state.clone());
    if new_state.doc != old_state.doc {
        on_change.run(yrs_bridge::doc_to_ydoc_bytes(&new_state.doc));
    }
}

/// Find the model position just after the top-level block containing the cursor.
fn insert_pos_after_cursor_block(state: &EditorState) -> usize {
    let cursor = state.selection.from();
    let mut offset = 0;
    if let Node::Element { content, .. } = &state.doc {
        for child in &content.children {
            let size = child.node_size();
            if cursor >= offset && cursor < offset + size {
                return offset + size;
            }
            offset += size;
        }
    }
    state.doc.content_size()
}

/// Handle the UploadImage command: open a file picker, upload to S3, insert image node.
fn handle_image_upload(
    doc_id: &str,
    view_ref: &Rc<RefCell<Option<EditorView>>>,
    history: &Rc<RefCell<HistoryPlugin>>,
    on_change: &Callback<Vec<u8>>,
    on_state_change: &Callback<EditorState>,
) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
    let Ok(input) = document.create_element("input") else { return };

    let _ = input.set_attribute("type", "file");
    let _ = input.set_attribute("accept", "image/*");
    let _ = input.set_attribute("style", "display:none");
    let _ = document.body().unwrap().append_child(&input);

    let input_el = input.clone();
    let doc_id = doc_id.to_string();
    let view_ref = Rc::clone(view_ref);
    let history = Rc::clone(history);
    let on_change = on_change.clone();
    let on_state_change = on_state_change.clone();

    let on_change_closure = Closure::wrap(Box::new(move |_: web_sys::Event| {
        let input_el = input_el.clone();
        let doc_id = doc_id.clone();
        let view_ref = Rc::clone(&view_ref);
        let history = Rc::clone(&history);
        let on_change = on_change.clone();
        let on_state_change = on_state_change.clone();

        leptos::task::spawn_local(async move {
            let html_input: web_sys::HtmlInputElement = input_el.clone().dyn_into().unwrap();
            let Some(files) = html_input.files() else { return };
            let Some(file) = files.get(0) else { return };

            let filename = file.name();
            let content_type = file.type_();
            let content_type = if content_type.is_empty() {
                "application/octet-stream".to_string()
            } else {
                content_type
            };

            // Read file bytes
            let Ok(array_buffer) = wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await else {
                return;
            };
            let bytes = js_sys::Uint8Array::new(&array_buffer).to_vec();

            // Upload: get presigned URL → PUT to S3 → get download URL
            let upload = match blobs::request_upload_url(&doc_id, &filename, &content_type).await {
                Ok(u) => u,
                Err(e) => {
                    web_sys::console::error_1(&format!("Upload URL failed: {e}").into());
                    return;
                }
            };

            if let Err(e) = blobs::upload_to_s3(&upload.upload_url, &bytes, &content_type).await {
                web_sys::console::error_1(&format!("S3 upload failed: {e}").into());
                return;
            }

            let download_url = match blobs::request_download_url(&doc_id, &upload.blob_id, &upload.key).await {
                Ok(u) => u,
                Err(e) => {
                    web_sys::console::error_1(&format!("Download URL failed: {e}").into());
                    return;
                }
            };

            // Insert image node after the current block
            let v = view_ref.borrow();
            let Some(v) = v.as_ref() else { return };
            let state = v.state();

            let mut attrs = HashMap::new();
            attrs.insert("src".to_string(), download_url);
            attrs.insert("alt".to_string(), filename);
            let img = Node::element_with_attrs(NodeType::Image, attrs, Fragment::empty());

            let insert_pos = insert_pos_after_cursor_block(&state);
            let slice = Slice::new(Fragment::from(vec![img]), 0, 0);
            let mut txn_result = state.transaction().replace(insert_pos, insert_pos, slice);
            if let Ok(ref mut txn) = txn_result {
                txn.selection = Selection::cursor(insert_pos + 1);
            }
            if let Ok(txn) = txn_result {
                apply_and_notify(v, txn, Some(&history), &on_change, &on_state_change);
            }

            input_el.remove();
        });
    }) as Box<dyn Fn(web_sys::Event)>);

    input
        .add_event_listener_with_callback("change", on_change_closure.as_ref().unchecked_ref())
        .unwrap_or(());
    on_change_closure.forget();

    if let Ok(html_input) = input.dyn_into::<web_sys::HtmlElement>() {
        html_input.click();
    }
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
        let Some(container) = container_ref.get() else { return };
        if view_ref_init.borrow().is_some() { return } // already initialized

        let html_element: web_sys::HtmlElement = container.into();

        let doc = if let Some(ref bytes) = props_clone.initial_content {
            yrs_bridge::ydoc_bytes_to_doc(bytes).unwrap_or_else(|_| Node::empty_doc())
        } else {
            Node::empty_doc()
        };

        let state = EditorState::create_default(doc);
        props_clone.on_state_change.run(state.clone());

        // Use Weak to break the Rc cycle: dispatch -> view_ref -> EditorView -> dispatch
        let view_ref_weak: Weak<RefCell<Option<EditorView>>> = Rc::downgrade(&view_ref_init);
        let on_change = props_clone.on_change.clone();
        let on_state_change = props_clone.on_state_change.clone();

        let dispatch = move |txn: Transaction| {
            let Some(view_rc) = view_ref_weak.upgrade() else { return };
            let view = view_rc.borrow();
            let Some(view) = view.as_ref() else { return };
            // History recording is handled by the view's dispatch wrapper.
            apply_and_notify(view, txn, None, &on_change, &on_state_change);
        };

        let editor_view = EditorView::new(html_element, state, dispatch, Rc::clone(&history_ref_init));
        *view_ref_init.borrow_mut() = Some(editor_view);
    });

    // Apply remote document updates from collaborators.
    let view_ref_remote = Rc::clone(&view_ref);
    let on_state_change_remote = props.on_state_change.clone();
    let remote_state_signal = props.remote_state;
    Effect::new(move |_| {
        let Some(new_state) = remote_state_signal.get() else { return };
        let view = view_ref_remote.borrow();
        let Some(view) = view.as_ref() else { return };
        view.update_state(new_state.clone());
        on_state_change_remote.run(new_state);
    });

    // Process toolbar commands reactively
    let view_ref_cmd = Rc::clone(&view_ref);
    let history_ref_cmd = Rc::clone(&history_ref);
    let on_change_cmd = props.on_change.clone();
    let on_state_change_cmd = props.on_state_change.clone();

    Effect::new(move |_| {
        let Some(cmd) = props.command_signal.get() else { return };

        let view = view_ref_cmd.borrow();
        let Some(view) = view.as_ref() else { return };

        // Sync DOM selection to model before executing the command,
        // so toolbar actions see the user's actual selection, not a stale cursor.
        let state = {
            let mut s = view.state();
            if let Some(dom_sel) = view.read_dom_selection() {
                s.selection = dom_sel;
            }
            s
        };
        let history = Rc::clone(&history_ref_cmd);
        let on_change = on_change_cmd.clone();
        let on_state_change = on_state_change_cmd.clone();

        let dispatch_fn = |txn: Transaction| {
            let v = view_ref_cmd.borrow();
            let Some(v) = v.as_ref() else { return };
            apply_and_notify(v, txn, Some(&history), &on_change, &on_state_change);
        };

        match cmd {
            ToolbarCommand::ToggleBold => { commands::toggle_mark(MarkType::Bold, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleItalic => { commands::toggle_mark(MarkType::Italic, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleUnderline => { commands::toggle_mark(MarkType::Underline, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleStrike => { commands::toggle_mark(MarkType::Strike, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleCode => { commands::toggle_mark(MarkType::Code, &state, Some(&dispatch_fn)); }
            ToolbarCommand::SetParagraph => { commands::set_paragraph(&state, Some(&dispatch_fn)); }
            ToolbarCommand::SetHeading(level) => { commands::set_heading(level, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleBulletList => { commands::toggle_list(NodeType::BulletList, NodeType::ListItem, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleOrderedList => { commands::toggle_list(NodeType::OrderedList, NodeType::ListItem, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleTaskList => { commands::toggle_list(NodeType::TaskList, NodeType::TaskItem, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleBlockquote => { commands::toggle_blockquote(&state, Some(&dispatch_fn)); }
            ToolbarCommand::SetCodeBlock => { commands::set_code_block(&state, Some(&dispatch_fn)); }
            ToolbarCommand::InsertHorizontalRule => { commands::insert_horizontal_rule(&state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleLink(ref href) => { commands::toggle_link(href, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleTextColor(ref color) => { commands::toggle_color_mark(MarkType::TextColor, color, &state, Some(&dispatch_fn)); }
            ToolbarCommand::ToggleHighlight(ref color) => { commands::toggle_color_mark(MarkType::Highlight, color, &state, Some(&dispatch_fn)); }
            ToolbarCommand::InsertComment => {}
            ToolbarCommand::UploadImage => {
                handle_image_upload(&props.doc_id, &view_ref_cmd, &history_ref_cmd, &on_change_cmd, &on_state_change_cmd);
            }
            ToolbarCommand::Undo => {
                if let Some(txn) = history_ref_cmd.borrow_mut().undo(&state) {
                    dispatch_fn(txn);
                }
            }
            ToolbarCommand::Redo => {
                if let Some(txn) = history_ref_cmd.borrow_mut().redo(&state) {
                    dispatch_fn(txn);
                }
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
