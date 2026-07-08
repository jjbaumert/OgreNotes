// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::cell::RefCell;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, HtmlElement, Node as DomNode};

use super::model::{char_len, Mark, MarkType, Node, NodeType};
use super::selection::Selection;
use super::state::{EditorState, Transaction};

/// The editor view: bridges the document model and the browser DOM.
/// Owns the contenteditable element, renders the model, handles input.
pub struct EditorView {
    /// The contenteditable container element.
    container: HtmlElement,
    /// Current editor state.
    state: Rc<RefCell<EditorState>>,
    /// Callback to dispatch transactions to the outside world.
    dispatch: Rc<dyn Fn(Transaction)>,
    /// Whether we're currently composing (IME input).
    composing: Rc<RefCell<bool>>,
    /// Guard to suppress the beforeinput event immediately after compositionend.
    just_composed: Rc<RefCell<bool>>,
    /// #117: holds the deferred compositionend insert so its `Timeout`
    /// isn't dropped (which would cancel it) before it fires, and so a
    /// rapid follow-on composition replaces rather than leaks it.
    pending_composition: Rc<RefCell<Option<gloo_timers::callback::Timeout>>>,
    /// History plugin for undo/redo (shared with editor component).
    history: Rc<RefCell<super::plugins::HistoryPlugin>>,
    /// Stored (event_name, closure) pairs for cleanup in destroy().
    listeners: Vec<(&'static str, Closure<dyn Fn(web_sys::Event)>)>,
    /// selectionchange listener (attached to document, not container).
    selectionchange_closure: Option<Closure<dyn Fn(web_sys::Event)>>,
}

impl EditorView {
    /// Create a new editor view attached to a container element (writable).
    pub fn new(
        container: HtmlElement,
        state: EditorState,
        dispatch: impl Fn(Transaction) + 'static,
        history: Rc<RefCell<super::plugins::HistoryPlugin>>,
    ) -> Self {
        Self::new_with_options(container, state, dispatch, history, false)
    }

    /// Create a new editor view with the option to render it read-only.
    ///
    /// When `readonly` is true, the container is NOT set as `contenteditable`
    /// and no input/keydown/composition/mutation listeners are attached, so
    /// typing and keyboard shortcuts cannot dispatch transactions. The DOM is
    /// still rendered (so the document is visible) and selection tracking is
    /// still attached so copy works.
    pub fn new_with_options(
        container: HtmlElement,
        state: EditorState,
        dispatch: impl Fn(Transaction) + 'static,
        history: Rc<RefCell<super::plugins::HistoryPlugin>>,
        readonly: bool,
    ) -> Self {
        if !readonly {
            container
                .set_attribute("contenteditable", "true")
                .unwrap_or(());
        }
        container
            .set_attribute("spellcheck", "false")
            .unwrap_or(());
        container.class_list().add_1("editor-content").unwrap_or(());
        if readonly {
            container.class_list().add_1("editor-content-readonly").unwrap_or(());
        }

        let state = Rc::new(RefCell::new(state));
        // Wrap the external dispatch to automatically record to history.
        // Uses try_borrow to avoid RefCell conflicts when the keydown handler
        // dispatches undo/redo while already borrowing the state.
        let history_for_dispatch = Rc::clone(&history);
        let state_for_dispatch = Rc::clone(&state);
        let external_dispatch = Rc::new(dispatch) as Rc<dyn Fn(Transaction)>;
        let dispatch: Rc<dyn Fn(Transaction)> = Rc::new(move |txn: Transaction| {
            if let Ok(state_ref) = state_for_dispatch.try_borrow() {
                history_for_dispatch.borrow_mut().record(&txn, &state_ref.doc);
            }
            external_dispatch(txn);
        });
        let composing = Rc::new(RefCell::new(false));
        let just_composed = Rc::new(RefCell::new(false));

        let mut view = Self {
            container,
            state,
            dispatch,
            composing,
            just_composed,
            pending_composition: Rc::new(RefCell::new(None)),
            history,
            listeners: Vec::new(),
            selectionchange_closure: None,
        };

        view.render();
        if !readonly {
            view.attach_listeners();
        }
        // Signal that the editor is fully wired (rendered + keydown/input
        // listeners attached). Keystrokes arriving before this can be dropped,
        // since the listeners are registered from an async Leptos Effect; the
        // frontend-doctor scenarios wait on `.editor-content[data-editor-ready]`
        // before typing to avoid that race. Also useful for a11y/debug tooling.
        view.container
            .set_attribute("data-editor-ready", "true")
            .unwrap_or(());
        view
    }

    /// Get the current editor state.
    pub fn state(&self) -> EditorState {
        self.state.borrow().clone()
    }

    /// Update the state. Re-renders the DOM only if the document changed;
    /// for selection-only changes, just syncs the selection without
    /// destroying and rebuilding DOM nodes.
    pub fn update_state(&self, new_state: EditorState) {
        let doc_changed = self.state.borrow().doc != new_state.doc;
        *self.state.borrow_mut() = new_state;
        if !*self.composing.borrow() {
            if doc_changed {
                self.render();
            } else {
                let sel = self.state.borrow().selection.clone();
                self.sync_selection_to_dom(&sel);
            }
        }
    }

    /// Render the document model to DOM.
    fn render(&self) {
        let doc = web_sys::window()
            .and_then(|w| w.document())
            .expect("no document");

        let state = self.state.borrow();

        self.container.set_inner_html("");

        if let Node::Element { content, .. } = &state.doc {
            for child in &content.children {
                if let Some(dom_node) = render_node(&doc, child) {
                    self.container
                        .append_child(&dom_node)
                        .unwrap_or_else(|_| dom_node.clone());
                }
            }
        }

        self.sync_selection_to_dom(&state.selection);
    }

    /// Sync the model selection to the browser's DOM selection.
    /// Preserves direction (backward selections where anchor > head).
    fn sync_selection_to_dom(&self, selection: &Selection) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Some(dom_sel) = window.get_selection().ok().flatten() else {
            return;
        };

        let anchor = selection.anchor();
        let head = selection.head();

        if let Some((anchor_node, anchor_offset)) = find_dom_position(&self.container, anchor) {
            if selection.empty() {
                // Cursor: collapse to anchor
                dom_sel.remove_all_ranges().unwrap_or(());
                let _ = dom_sel.collapse_with_offset(
                    Some(&anchor_node),
                    anchor_offset as u32,
                );
            } else if let Some((head_node, head_offset)) =
                find_dom_position(&self.container, head)
            {
                // Range: use setBaseAndExtent to preserve direction
                let _ = dom_sel.set_base_and_extent(
                    &anchor_node,
                    anchor_offset as u32,
                    &head_node,
                    head_offset as u32,
                );
            }
        }
    }

    /// Read the browser's DOM selection and map it to model positions.
    pub fn read_dom_selection(&self) -> Option<Selection> {
        read_dom_selection_from(&self.container)
    }

    /// Attach keyboard, input, and composition event listeners.
    fn attach_listeners(&mut self) {
        // Keydown -- dispatch to keymap for keyboard shortcuts
        let composing = Rc::clone(&self.composing);
        let state_kd = Rc::clone(&self.state);
        let dispatch_kd = Rc::clone(&self.dispatch);
        let history_kd = Rc::clone(&self.history);
        let keymap = super::keymap::default_keymap();
        let container_kd = self.container.clone();

        let on_keydown = Closure::wrap(Box::new(move |event: web_sys::Event| {
            if *composing.borrow() {
                return;
            }
            let Some(ke) = event.dyn_ref::<web_sys::KeyboardEvent>() else {
                return;
            };

            let key = ke.key();
            let ctrl = ke.ctrl_key();
            let meta = ke.meta_key();
            let shift = ke.shift_key();
            let alt = ke.alt_key();
            let mod_key = ctrl || meta;

            // Escape blurs the editor — keyboard escape hatch since Tab
            // inserts a tab character (issue #18) instead of moving focus
            // out, which would otherwise trap keyboard-only users in the
            // editor (WCAG 2.1.2 No Keyboard Trap).
            if key == "Escape" && !ctrl && !meta && !alt && !shift {
                let _ = container_kd.blur();
                return;
            }

            // Handle undo/redo directly (needs HistoryPlugin access).
            // Important: drop the history borrow BEFORE calling dispatch,
            // because dispatch records to history (which borrows it again).
            if mod_key && !alt {
                let current_state = state_kd.borrow().clone();
                if key.to_lowercase() == "z" && !shift {
                    event.prevent_default();
                    let txn = history_kd.borrow_mut().undo(&current_state);
                    if let Some(txn) = txn {
                        super::debug::log("keydown", "undo", &[]);
                        dispatch_kd(txn);
                    }
                    return;
                }
                if (key.to_lowercase() == "z" && shift)
                    || (key.to_lowercase() == "y" && !shift)
                {
                    event.prevent_default();
                    let txn = history_kd.borrow_mut().redo(&current_state);
                    if let Some(txn) = txn {
                        super::debug::log("keydown", "redo", &[]);
                        dispatch_kd(txn);
                    }
                    return;
                }
            }

            // Handle Ctrl+K for link insertion (needs window.prompt).
            if mod_key && !shift && !alt && key.to_lowercase() == "k" {
                event.prevent_default();
                let current_state = state_kd.borrow().clone();
                // Check if link is already active — if so, remove it
                let has_link = super::commands::mark_active_at_cursor_public(
                    &current_state,
                    super::model::MarkType::Link,
                );
                if has_link || current_state.selection.empty() {
                    // Remove link or do nothing for cursor with no link
                    if has_link {
                        super::commands::toggle_link("", &current_state, Some(&|txn| {
                            dispatch_kd(txn);
                        }));
                    }
                } else {
                    // Prompt for URL
                    if let Some(window) = web_sys::window() {
                        if let Ok(Some(href)) = window.prompt_with_message("Enter URL:") {
                            let href = href.trim().to_string();
                            if !href.is_empty() {
                                super::commands::toggle_link(&href, &current_state, Some(&|txn| {
                                    dispatch_kd(txn);
                                }));
                            }
                        }
                    }
                }
                return;
            }

            let current_state = state_kd.borrow().clone();

            // #78: a horizontal rule (or any atom block) is a void DOM
            // element the browser can't move a caret across, so native
            // ArrowUp/ArrowDown stalls at the rule. When the caret is at the
            // boundary of its block with an atom adjacent in the travel
            // direction, move it explicitly to the text block beyond.
            if !mod_key && !alt && !shift && (key == "ArrowUp" || key == "ArrowDown") {
                let dir = if key == "ArrowUp" { -1 } else { 1 };
                // Two gates, cheapest first:
                //  1. Pure-model boundary: caret exactly at the block's
                //     start (up) / end (down) — always on the edge line,
                //     no DOM needed.
                //  2. First/last visual line: caret anywhere on that line
                //     (e.g. mid-text in a single-line paragraph above/below
                //     the rule) — confirmed by matching the caret's DOM rect
                //     top to the block edge's. This is what was missing for
                //     #78: a single-line paragraph's whole text is the first
                //     AND last line, so arrowing up/down from the middle must
                //     still cross the rule.
                let cross_to = super::selection::arrow_over_atom(
                    &current_state.doc, &current_state.selection, dir,
                )
                .or_else(|| {
                    let cross = super::selection::atom_cross(
                        &current_state.doc, &current_state.selection, dir,
                    )?;
                    caret_on_block_edge_line(
                        current_state.selection.head(),
                        cross.block_edge,
                    )
                    .then_some(cross.target)
                });
                if let Some(new_sel) = cross_to {
                    event.prevent_default();
                    super::debug::log("keydown", "arrow_over_atom", &[("key", &key)]);
                    dispatch_kd(current_state.transaction().set_selection(new_sel));
                    return;
                }
            }

            let handled = keymap.handle(
                &key, ctrl, meta, shift, alt,
                &current_state,
                &|txn| { dispatch_kd(txn); },
            );

            if handled {
                super::debug::log("keydown", "handled", &[
                    ("key", &key),
                    ("ctrl", &ctrl.to_string()),
                    ("shift", &shift.to_string()),
                    ("alt", &alt.to_string()),
                ]);
                event.prevent_default();
            }
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("keydown", on_keydown);

        // beforeinput -- handle text insertion, deletion
        let state = Rc::clone(&self.state);
        let dispatch = Rc::clone(&self.dispatch);
        let composing2 = Rc::clone(&self.composing);
        let just_composed2 = Rc::clone(&self.just_composed);
        let container2 = self.container.clone();

        let on_before_input = Closure::wrap(Box::new(move |event: web_sys::Event| {
            if *composing2.borrow() {
                return;
            }

            // Suppress the insertText that immediately follows compositionend
            // Only clear the flag when we actually suppress an insertText.
            if *just_composed2.borrow() {
                if let Some(ie) = event.dyn_ref::<web_sys::InputEvent>() {
                    if ie.input_type() == "insertText" {
                        *just_composed2.borrow_mut() = false;
                        event.prevent_default();
                        return;
                    }
                }
                // Non-insertText event: don't clear the flag yet
            }

            let Some(ie) = event.dyn_ref::<web_sys::InputEvent>() else {
                return;
            };

            let input_type = ie.input_type();
            let data = ie.data();
            let current_state = state.borrow().clone();

            match input_type.as_str() {
                "insertText" => {
                    event.prevent_default(); // always prevent DOM mutation
                    if let Some(text) = data {
                        if !text.is_empty() {
                            if let Some(sel) = read_dom_selection_from(&container2) {
                                let model_sel = state.borrow().selection.clone();
                                super::debug::warn("input", &format!(
                                    "insertText data='{}' dom_pos={} model_pos={}",
                                    text, sel.from(), model_sel.from(),
                                ));
                                let state_with_sel = EditorState {
                                    selection: sel,
                                    ..current_state
                                };
                                if let Ok(insert_txn) = state_with_sel.transaction().insert_text(&text) {
                                    // Check input rules after insertion
                                    let post_insert = state_with_sel.apply(insert_txn.clone());
                                    let cursor = post_insert.selection.from();
                                    if let Some((text_before, block_start)) =
                                        super::input_rules::get_block_text_before(&post_insert.doc, cursor)
                                    {
                                        let rules = super::input_rules::default_input_rules();
                                        if let Some(rule_txn) = super::input_rules::check_input_rules(
                                            &rules, &post_insert, &text_before, block_start,
                                        ) {
                                            super::debug::log("input", "input rule matched", &[]);
                                            // Combine insert + rule steps into one transaction
                                            let rule_sel = rule_txn.selection.clone();
                                            let all_steps: Vec<_> = insert_txn.steps.iter().cloned()
                                                .chain(rule_txn.steps)
                                                .collect();
                                            let result = all_steps.into_iter().try_fold(
                                                state_with_sel.transaction(),
                                                |txn, step| txn.step(step),
                                            );
                                            if let Ok(mut combined) = result {
                                                combined.selection = rule_sel;
                                                dispatch(combined);
                                                return;
                                            } else {
                                                super::debug::error("input", "failed to combine input rule steps");
                                            }
                                        }
                                    }
                                    dispatch(insert_txn);
                                } else {
                                    super::debug::error("input", "insert_text failed");
                                }
                            }
                        }
                    }
                }
                "deleteContentBackward" => {
                    event.prevent_default();
                    if let Some(sel) = read_dom_selection_from(&container2) {
                        let state_with_sel = EditorState {
                            selection: sel,
                            ..current_state.clone()
                        };
                        if state_with_sel.selection.empty() {
                            let pos = state_with_sel.selection.from();
                            super::debug::log("backspace", "at cursor", &[
                                ("pos", &pos.to_string()),
                            ]);
                            if pos > 0 {
                                // #78: Backspace at a block's start over an atom
                                // (horizontal rule, image, embed) deletes just the
                                // atom. join_backward would skip it and merge into
                                // the text block beyond, leaving the rule floating.
                                let atom_before = super::selection::atom_before_cursor_block(
                                    &state_with_sel.doc, &state_with_sel.selection,
                                );
                                if let Some((from, to)) = atom_before {
                                    if let Ok(mut txn) = state_with_sel.transaction().delete(from, to) {
                                        txn.selection = Selection::cursor(pos - (to - from));
                                        super::debug::log("backspace", "delete atom before block", &[
                                            ("from", &from.to_string()),
                                            ("to", &to.to_string()),
                                        ]);
                                        dispatch(txn);
                                    }
                                } else if let Ok(txn) = state_with_sel.transaction().join_backward() {
                                    super::debug::log("backspace", "join_backward succeeded", &[]);
                                    dispatch(txn);
                                } else {
                                    // join_backward failed. Check if we're at block start
                                    // for list-specific handling, otherwise just delete a char.
                                    let block = super::state::find_block_at(&state_with_sel.doc, pos);
                                    let at_block_start = block.map_or(false, |b| pos == b.content_start);

                                    let mut handled = false;

                                    if at_block_start {
                                        // At block start with no previous textblock to join.
                                        // If in a nested list item, dedent it.
                                        // If in a top-level list item, unwrap to paragraph.
                                        let item = super::state::find_item_at(&state_with_sel.doc, pos);
                                        let is_nested = item.is_some() && {
                                            let container = super::state::find_container_at(&state_with_sel.doc, pos);
                                            container.map_or(false, |c| {
                                                super::state::find_item_at(&state_with_sel.doc, c.offset).is_some()
                                            })
                                        };

                                        if is_nested {
                                            super::commands::lift_list_item(
                                                &state_with_sel, Some(&|txn: super::state::Transaction| {
                                                    dispatch(txn);
                                                }),
                                            );
                                            super::debug::log("backspace", "dedent nested list item", &[]);
                                            handled = true;
                                        } else if item.is_some() {
                                            if let Ok(txn) = state_with_sel.transaction().lift_from_list() {
                                                let cursor_after = txn.selection.from();
                                                super::debug::log("backspace", "lift_from_list succeeded", &[
                                                    ("cursor_after", &cursor_after.to_string()),
                                                ]);
                                                dispatch(txn);
                                                handled = true;
                                            }
                                        }

                                        // At block start, not in a list, join_backward failed:
                                        // This shouldn't happen now that join_backward searches
                                        // deeper, but log it if it does.
                                        if !handled {
                                            super::debug::warn("backspace", "at block start with no action available");
                                        }
                                    }

                                    // Fallback: delete a single character (works for mid-text)
                                    if !handled && !at_block_start {
                                        if let Ok(txn) = state_with_sel.transaction().delete(pos - 1, pos) {
                                            super::debug::log("backspace", "delete char", &[
                                                ("from", &(pos - 1).to_string()),
                                                ("to", &pos.to_string()),
                                            ]);
                                            dispatch(txn);
                                        } else {
                                            super::debug::warn("backspace", "delete char failed");
                                        }
                                    }
                                }
                            }
                        } else if super::commands::delete_table_selection(&state_with_sel, Some(&|txn| dispatch(txn))) {
                            // Table-spanning selection: cleared cell contents
                        } else if let Ok(txn) = state_with_sel.transaction().delete_selection() {
                            dispatch(txn);
                        }
                    }
                }
                "deleteContentForward" => {
                    event.prevent_default();
                    if let Some(sel) = read_dom_selection_from(&container2) {
                        let state_with_sel = EditorState {
                            selection: sel,
                            ..current_state.clone()
                        };
                        if state_with_sel.selection.empty() {
                            let pos = state_with_sel.selection.from();
                            let max = state_with_sel.doc.content_size();
                            if pos < max {
                                // Try joining with next block first (delete at block end)
                                if let Ok(txn) = state_with_sel.transaction().join_forward() {
                                    dispatch(txn);
                                } else if let Ok(txn) =
                                    state_with_sel.transaction().delete(pos, pos + 1)
                                {
                                    dispatch(txn);
                                }
                            }
                        } else if super::commands::delete_table_selection(&state_with_sel, Some(&|txn| dispatch(txn))) {
                            // Table-spanning selection: cleared cell contents
                        } else if let Ok(txn) = state_with_sel.transaction().delete_selection() {
                            dispatch(txn);
                        }
                    }
                }
                "insertParagraph" => {
                    event.prevent_default();
                    if let Some(sel) = read_dom_selection_from(&container2) {
                        super::debug::log("enter", "split_block", &[
                            ("pos", &sel.from().to_string()),
                        ]);
                        let state_with_sel = EditorState {
                            selection: sel,
                            ..current_state
                        };
                        if let Ok(txn) = state_with_sel.transaction().split_block() {
                            dispatch(txn);
                        } else {
                            let doc = &state_with_sel.doc;
                            let content_size = doc.content_size();
                            let child_count = doc.child_count();
                            let pos = state_with_sel.selection.from();
                            let block = super::state::find_block_at(doc, pos);
                            let child0_info = doc.child(0).map(|c| format!(
                                "type={:?} node_size={} text_len={}",
                                c.node_type(), c.node_size(), c.text_content().len(),
                            )).unwrap_or_else(|| "none".to_string());
                            super::debug::error("enter", &format!(
                                "split_block failed pos={pos} content_size={content_size} \
                                 child_count={child_count} block_found={} child0=[{child0_info}] \
                                 doc_text='{}'",
                                block.is_some(),
                                doc.text_content().chars().take(100).collect::<String>(),
                            ));
                        }
                    }
                }
                "insertLineBreak" | "insertSoftLineBreak" => {
                    // Shift+Enter: insert a hard break (<br>) without splitting the block
                    event.prevent_default();
                    if let Some(sel) = read_dom_selection_from(&container2) {
                        let state_with_sel = EditorState {
                            selection: sel,
                            ..current_state
                        };
                        let from = state_with_sel.selection.from();
                        let to = state_with_sel.selection.to();
                        let br_node = super::model::Node::element(
                            super::model::NodeType::HardBreak,
                        );
                        let content = super::model::Fragment::from(vec![br_node]);
                        let slice = super::model::Slice::new(content, 0, 0);
                        if let Ok(txn) = state_with_sel.transaction().replace(from, to, slice) {
                            dispatch(txn);
                        }
                    }
                }
                "deleteWordBackward" => {
                    event.prevent_default();
                    if let Some(sel) = read_dom_selection_from(&container2) {
                        let state_with_sel = EditorState {
                            selection: sel,
                            ..current_state.clone()
                        };
                        if let Ok(txn) = state_with_sel.transaction().delete_word_backward() {
                            dispatch(txn);
                        }
                    }
                }
                "deleteWordForward" => {
                    event.prevent_default();
                    if let Some(sel) = read_dom_selection_from(&container2) {
                        let state_with_sel = EditorState {
                            selection: sel,
                            ..current_state.clone()
                        };
                        if let Ok(txn) = state_with_sel.transaction().delete_word_forward() {
                            dispatch(txn);
                        }
                    }
                }
                // Prevent unhandled input types from mutating DOM without model update
                "deleteSoftLineBackward" | "deleteSoftLineForward"
                | "deleteHardLineBackward" | "deleteHardLineForward"
                | "insertFromPaste" | "insertFromDrop"
                | "historyUndo" | "historyRedo" => {
                    event.prevent_default();
                    // These are not yet implemented; prevent DOM corruption.
                    // Paste/clipboard handling will be added in 8i.
                }
                _ => {
                    // Unknown input types: prevent default to avoid DOM/model desync
                    event.prevent_default();
                }
            }
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("beforeinput", on_before_input);

        // compositionstart
        let composing_start = Rc::clone(&self.composing);
        let on_comp_start = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            *composing_start.borrow_mut() = true;
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("compositionstart", on_comp_start);

        // compositionend
        let composing_end = Rc::clone(&self.composing);
        let just_composed_end = Rc::clone(&self.just_composed);
        let state_comp = Rc::clone(&self.state);
        let dispatch_comp = Rc::clone(&self.dispatch);
        let container_comp = self.container.clone();
        let pending_comp = Rc::clone(&self.pending_composition);

        let on_comp_end = Closure::wrap(Box::new(move |event: web_sys::Event| {
            *composing_end.borrow_mut() = false;

            let Some(ce) = event.dyn_ref::<web_sys::CompositionEvent>() else {
                return;
            };
            let Some(data) = ce.data() else { return };
            if data.is_empty() {
                return;
            }
            // Only set the guard when we actually insert composed text.
            *just_composed_end.borrow_mut() = true;

            // #117: DEFER the selection read + insert to the next macrotask.
            //
            // The composed text (`data`) is correct here, but
            // `read_dom_selection_from` is not on every browser: Chrome
            // Android fires `compositionend` BEFORE the composed string is
            // written into the DOM, so a synchronous read returns a caret
            // offset short by the composed length and the text lands in the
            // wrong place. A 0 ms `Timeout` runs after the current task —
            // and after the browser has settled the composed text into the
            // DOM — so the read is correct. On desktop the DOM is already
            // settled at `compositionend`, so reading one tick later yields
            // the same position: this is behaviour-preserving there.
            //
            // The `Timeout` is stored on `self` so it isn't dropped (which
            // would cancel it) before it fires; the next composition
            // replaces it.
            let state = Rc::clone(&state_comp);
            let dispatch = Rc::clone(&dispatch_comp);
            let container = container_comp.clone();
            let timeout = gloo_timers::callback::Timeout::new(0, move || {
                let current_state = state.borrow().clone();
                if let Some(sel) = read_dom_selection_from(&container) {
                    let state_with_sel = EditorState {
                        selection: sel,
                        ..current_state
                    };
                    if let Ok(txn) = state_with_sel.transaction().insert_text(&data) {
                        dispatch(txn);
                    }
                }
            });
            *pending_comp.borrow_mut() = Some(timeout);
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("compositionend", on_comp_end);

        // copy — serialize selection to clipboard
        let state_copy = Rc::clone(&self.state);
        let container_copy = self.container.clone();
        let on_copy = Closure::wrap(Box::new(move |event: web_sys::Event| {
            event.prevent_default();
            let Some(ce) = event.dyn_ref::<web_sys::ClipboardEvent>() else { return };
            let Some(clipboard_data) = ce.clipboard_data() else { return };
            let Some(sel) = read_dom_selection_from(&container_copy) else { return };

            let state = state_copy.borrow().clone();
            let state_with_sel = EditorState {
                selection: sel,
                ..state
            };
            let slice = state_with_sel.selected_slice();
            if slice.content.children.is_empty() {
                return;
            }

            let html = super::clipboard::serialize_to_html(&slice);
            let text = super::clipboard::serialize_to_text(&slice);
            clipboard_data.set_data("text/html", &html).ok();
            clipboard_data.set_data("text/plain", &text).ok();
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("copy", on_copy);

        // cut — copy to clipboard + delete selection
        let state_cut = Rc::clone(&self.state);
        let dispatch_cut = Rc::clone(&self.dispatch);
        let container_cut = self.container.clone();
        let on_cut = Closure::wrap(Box::new(move |event: web_sys::Event| {
            event.prevent_default();
            let Some(ce) = event.dyn_ref::<web_sys::ClipboardEvent>() else { return };
            let Some(clipboard_data) = ce.clipboard_data() else { return };
            let Some(sel) = read_dom_selection_from(&container_cut) else { return };

            let state = state_cut.borrow().clone();
            let state_with_sel = EditorState {
                selection: sel,
                ..state
            };
            let slice = state_with_sel.selected_slice();
            if slice.content.children.is_empty() {
                return;
            }

            let html = super::clipboard::serialize_to_html(&slice);
            let text = super::clipboard::serialize_to_text(&slice);
            clipboard_data.set_data("text/html", &html).ok();
            clipboard_data.set_data("text/plain", &text).ok();

            // Delete the selection
            if let Ok(txn) = state_with_sel.transaction().delete_selection() {
                dispatch_cut(txn);
            }
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("cut", on_cut);

        // paste — read clipboard and insert content
        let state_paste = Rc::clone(&self.state);
        let dispatch_paste = Rc::clone(&self.dispatch);
        let container_paste = self.container.clone();
        let on_paste = Closure::wrap(Box::new(move |event: web_sys::Event| {
            event.prevent_default();
            let Some(ce) = event.dyn_ref::<web_sys::ClipboardEvent>() else { return };
            let Some(clipboard_data) = ce.clipboard_data() else { return };
            let Some(sel) = read_dom_selection_from(&container_paste) else { return };

            let state = state_paste.borrow().clone();
            let state_with_sel = EditorState {
                selection: sel,
                ..state
            };

            // Prefer HTML when it carries actual structure; fall through to
            // markdown parsing on plain text if the HTML is "trivial" — i.e.
            // only Paragraphs and text with no marks, which happens when a
            // source copies raw markdown into an HTML wrapper (`<p>**bold**</p>`
            // etc.). Without this fallback, pasted markdown source would land
            // verbatim with the syntax characters still visible.
            let html = clipboard_data.get_data("text/html").unwrap_or_default();
            let text = clipboard_data.get_data("text/plain").unwrap_or_default();
            super::debug::log("paste", "clipboard", &[
                ("html_len", &html.len().to_string()),
                ("html_preview", &html.chars().take(500).collect::<String>()),
                ("text_len", &text.len().to_string()),
                ("text_preview", &text.chars().take(500).collect::<String>()),
            ]);
            let slice = if !html.is_empty() {
                let html_slice = super::clipboard::parse_from_html(&html);
                // Sources like terminals, file viewers, and "view source" panels
                // wrap copied content in a bare `<pre>` block — sometimes with
                // per-line `<div>` / `<br>` elements that our `convert_code_block`
                // strips when extracting text via `text_content()`. The result is
                // a multi-line markdown file collapsed into one CodeBlock. We
                // detect this shape (single root CodeBlock + the source HTML had
                // no `<code>` child, which means it was a bare `<pre>` rather
                // than an explicit code block) and parse the original markdown
                // source instead. When text/plain is empty (some sources omit
                // it), fall back to the CodeBlock's own text.
                //
                // The `<code>` check matters: `<pre><code>...</code></pre>`
                // with no language class is still an unlabeled code block whose
                // content must stay literal — re-parsing it as markdown would
                // mangle constructs like `*ptr` or `> redirect`.
                let html_lower = html.to_ascii_lowercase();
                let has_code_element = html_lower.contains("<code");
                let plain_pre_wrapper = html_slice.content.children.len() == 1
                    && html_slice.content.children[0].node_type()
                        == Some(super::model::NodeType::CodeBlock)
                    && !has_code_element;
                let trivial = super::markdown::is_trivial_slice(&html_slice);
                if !text.is_empty() && (trivial || plain_pre_wrapper) {
                    super::debug::log("paste", "branch", &[
                        ("chosen", if plain_pre_wrapper {
                            "markdown (HTML was a plain <pre> wrapper)"
                        } else {
                            "markdown (HTML was trivial)"
                        }),
                    ]);
                    super::markdown::parse_from_markdown(&text)
                } else if plain_pre_wrapper {
                    super::debug::log("paste", "branch", &[
                        ("chosen", "markdown (plain <pre> wrapper, text/plain empty)"),
                    ]);
                    let cb_text = html_slice.content.children[0].text_content();
                    super::markdown::parse_from_markdown(&cb_text)
                } else {
                    super::debug::log("paste", "branch", &[
                        ("chosen", "html"),
                        ("root_children", &html_slice.content.children.len().to_string()),
                    ]);
                    html_slice
                }
            } else if !text.is_empty() {
                super::debug::log("paste", "branch", &[("chosen", "markdown (no HTML)")]);
                super::markdown::parse_from_markdown(&text)
            } else {
                super::debug::warn("paste", "clipboard empty — nothing to paste");
                return;
            };

            if slice.content.children.is_empty() {
                super::debug::warn("paste", "parsed slice is empty");
                return;
            }

            // Determine paste context and strategy
            let pos = state_with_sel.selection.from();
            let in_list = super::state::find_item_at(&state_with_sel.doc, pos).is_some();
            super::debug::log("paste", "context", &[
                ("pos", &pos.to_string()),
                ("in_list", &in_list.to_string()),
                ("slice_children", &slice.content.children.len().to_string()),
            ]);
            let pasting_list = slice.content.children.iter().any(|n| matches!(
                n.node_type(),
                Some(super::model::NodeType::BulletList)
                    | Some(super::model::NodeType::OrderedList)
                    | Some(super::model::NodeType::TaskList)
            ));

            if in_list && pasting_list {
                // Pasting list items into a list: extract items from the pasted list
                // and insert them as siblings in the current list.
                // Non-list content (e.g., a stray Heading captured by the selection)
                // is converted to list items to avoid inserting invalid children.
                let mut items = Vec::new();
                for node in &slice.content.children {
                    if let Some(nt) = node.node_type() {
                        if matches!(nt,
                            super::model::NodeType::BulletList
                            | super::model::NodeType::OrderedList
                            | super::model::NodeType::TaskList
                        ) {
                            // Extract list items from the pasted list
                            for j in 0..node.child_count() {
                                if let Some(item) = node.child(j) {
                                    items.push(item.clone());
                                }
                            }
                        } else if matches!(nt,
                            super::model::NodeType::ListItem
                            | super::model::NodeType::TaskItem
                        ) {
                            // Already a list item — use as-is
                            items.push(node.clone());
                        } else {
                            // Non-list content (Heading, Paragraph, etc.):
                            // wrap in a ListItem if it has text, otherwise skip
                            let text = node.text_content();
                            if !text.trim().is_empty() {
                                let para = super::model::Node::element_with_content(
                                    super::model::NodeType::Paragraph,
                                    super::model::Fragment::from(vec![
                                        super::model::Node::text(&text),
                                    ]),
                                );
                                items.push(super::model::Node::element_with_content(
                                    super::model::NodeType::ListItem,
                                    super::model::Fragment::from(vec![para]),
                                ));
                            }
                            // Empty non-list nodes (like the stray empty Heading) are dropped
                        }
                    }
                }
                let item_info = super::state::find_item_at(&state_with_sel.doc, pos);
                if let Some(item) = item_info {
                    let item_text = item.content.children.iter()
                        .map(|c| c.text_content())
                        .collect::<String>();
                    let item_is_empty = item_text.trim().is_empty();

                    let item_slice = super::model::Slice::new(
                        super::model::Fragment::from(items), 0, 0,
                    );

                    if item_is_empty {
                        // Empty bullet: replace it with the pasted items
                        let from = item.offset;
                        let to = item.offset + item.node_size;
                        if let Ok(txn) = state_with_sel.transaction().replace(from, to, item_slice) {
                            dispatch_paste(txn);
                        }
                    } else {
                        // Non-empty bullet: insert pasted items before the current item
                        let insert_pos = item.offset;
                        if let Ok(txn) = state_with_sel.transaction().replace(insert_pos, insert_pos, item_slice) {
                            dispatch_paste(txn);
                        }
                    }
                }
            } else {
                // Determine paste context.
                // If the pasted content contains block-level nodes (headings, lists, etc.),
                // fit to Doc context so they're preserved. Otherwise fit to the current block.
                let has_blocks = slice.content.children.iter().any(|n| {
                    matches!(
                        n.node_type(),
                        Some(super::model::NodeType::Heading)
                            | Some(super::model::NodeType::BulletList)
                            | Some(super::model::NodeType::OrderedList)
                            | Some(super::model::NodeType::TaskList)
                            | Some(super::model::NodeType::Blockquote)
                            | Some(super::model::NodeType::CodeBlock)
                            | Some(super::model::NodeType::HorizontalRule)
                            | Some(super::model::NodeType::Table)
                            | Some(super::model::NodeType::Image)
                    )
                });

                let parent_type = if has_blocks {
                    super::model::NodeType::Doc
                } else {
                    super::state::find_block_at(&state_with_sel.doc, pos)
                        .map(|b| b.node_type)
                        .unwrap_or(super::model::NodeType::Doc)
                };

                let fitted = super::clipboard::fit_slice_to_context(slice, parent_type);

                if has_blocks {
                    // Block-level paste: replace at the block level, not inside the paragraph.
                    // Split the current block at the cursor, sandwich pasted blocks between halves.
                    if let Some(block) = super::state::find_block_at(&state_with_sel.doc, pos) {
                        let offset = pos.saturating_sub(block.content_start).min(block.content.size());
                        let before_content = block.content.cut(0, offset);
                        let after_content = block.content.cut(offset, block.content.size());

                        let mut nodes = Vec::new();

                        // Add the "before" part of the current block if it has content
                        if !before_content.children.is_empty()
                            && before_content.children.iter().any(|n| !n.text_content().is_empty())
                        {
                            nodes.push(super::model::Node::Element {
                                node_type: block.node_type,
                                attrs: block.attrs.clone(),
                                content: before_content,
                                marks: vec![],
                            });
                        }

                        // Add all pasted blocks
                        nodes.extend(fitted.content.children);

                        // Add the "after" part of the current block if it has content
                        if !after_content.children.is_empty()
                            && after_content.children.iter().any(|n| !n.text_content().is_empty())
                        {
                            nodes.push(super::model::Node::element_with_content(
                                super::model::NodeType::Paragraph,
                                after_content,
                            ));
                        }

                        let block_slice = super::model::Slice::new(
                            super::model::Fragment::from(nodes),
                            0,
                            0,
                        );
                        if let Ok(txn) = state_with_sel
                            .transaction()
                            .replace(block.offset, block.offset + block.node_size, block_slice)
                        {
                            dispatch_paste(txn);
                        }
                    }
                } else {
                    // Inline paste: insert into the current block
                    if let Ok(txn) = state_with_sel.transaction().replace_selection(fitted) {
                        dispatch_paste(txn);
                    }
                }
            }
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("paste", on_paste);

        // click — open links with Ctrl+click (Cmd+click on Mac) in a new tab.
        // Regular clicks in contenteditable just position the cursor.
        let on_click = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(me) = event.dyn_ref::<web_sys::MouseEvent>() else { return };
            // Ctrl+click (Windows/Linux) or Meta+click (Mac)
            if !me.ctrl_key() && !me.meta_key() {
                return;
            }
            // Walk up from the click target to find an <a> element
            let mut node = event.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok());
            while let Some(el) = node {
                if el.tag_name().to_lowercase() == "a" {
                    if let Some(href) = el.get_attribute("href") {
                        event.prevent_default();
                        // #2: re-validate the live DOM href before opening it.
                        // The render-time sanitizer only guards what we wrote;
                        // an extension/devtools (or any future DOM mutation)
                        // could have swapped in a `javascript:`/other scheme.
                        if is_safe_url(&href) {
                            let _ = web_sys::window()
                                .and_then(|w| w.open_with_url_and_target(&href, "_blank").ok());
                        }
                    }
                    return;
                }
                node = el.parent_element();
            }
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("click", on_click);

        // selectionchange — sync DOM selection to model so toolbar updates
        // when the user moves the cursor (click, arrow keys).
        // This event fires on the document, not the container.
        let state_sel = Rc::clone(&self.state);
        let dispatch_sel = Rc::clone(&self.dispatch);
        let container_sel = self.container.clone();
        let composing_sel = Rc::clone(&self.composing);

        let on_selectionchange = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            if *composing_sel.borrow() {
                return;
            }
            // While the focus_trap is mid-restore (a modal just
            // closed and focus is on its way back to the editor),
            // ignore selectionchange events — headless Chromium
            // snaps the contenteditable's range to a fresh cursor
            // at position 0 in this window, and accepting that
            // would silently destroy the user's pre-modal
            // selection just as a palette-dispatched command is
            // about to apply against it. Cleared one microtask
            // after focus() lands by install_focus_trap.
            if crate::a11y::is_focus_restore_in_progress() {
                return;
            }
            if let Some(sel) = read_dom_selection_from(&container_sel) {
                let current = state_sel.borrow();
                if current.selection != sel {
                    drop(current);
                    let state = state_sel.borrow().clone();
                    let txn = state.transaction().set_selection(sel);
                    dispatch_sel(txn);
                }
            }
        }) as Box<dyn Fn(web_sys::Event)>);

        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            doc.add_event_listener_with_callback(
                "selectionchange",
                on_selectionchange.as_ref().unchecked_ref(),
            )
            .unwrap_or(());
        }
        self.selectionchange_closure = Some(on_selectionchange);
    }

    /// Add an event listener and store it for cleanup.
    fn add_listener(&mut self, event: &'static str, closure: Closure<dyn Fn(web_sys::Event)>) {
        self.container
            .add_event_listener_with_callback(event, closure.as_ref().unchecked_ref())
            .unwrap_or(());
        self.listeners.push((event, closure));
    }

    /// Get the container element (for Leptos integration).
    pub fn container(&self) -> &HtmlElement {
        &self.container
    }

    /// Destroy the view, removing event listeners and clearing content.
    pub fn destroy(mut self) {
        self.remove_listeners();
        self.container.set_inner_html("");
    }

    /// Remove all event listeners from the container and document.
    fn remove_listeners(&mut self) {
        for (event, closure) in &self.listeners {
            self.container
                .remove_event_listener_with_callback(
                    event,
                    closure.as_ref().unchecked_ref(),
                )
                .unwrap_or(());
        }
        self.listeners.clear();

        if let Some(closure) = self.selectionchange_closure.take() {
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                doc.remove_event_listener_with_callback(
                    "selectionchange",
                    closure.as_ref().unchecked_ref(),
                )
                .unwrap_or(());
            }
        }
    }
}

/// Ensure event listeners are cleaned up if EditorView is dropped without calling destroy().
impl Drop for EditorView {
    fn drop(&mut self) {
        self.remove_listeners();
    }
}

// ─── DOM Rendering ──────────────────────────────────────────────

/// Render a model Node to a DOM Node.
fn render_node(doc: &Document, node: &Node) -> Option<DomNode> {
    match node {
        Node::Text { text, marks } => {
            let text_node = doc.create_text_node(text);
            if marks.is_empty() {
                Some(text_node.into())
            } else {
                // Wrap in mark elements (innermost first, outermost last)
                let mut current: DomNode = text_node.into();
                for mark in marks.iter().rev() {
                    let wrapper = create_mark_element(doc, mark)?;
                    wrapper.append_child(&current).ok()?;
                    current = wrapper.into();
                }
                Some(current)
            }
        }
        Node::Element {
            node_type,
            attrs,
            content,
            ..
        } => {
            // Handle special node types with custom rendering
            match node_type {
                NodeType::Heading => {
                    let level = attrs.get("level").map(|s| s.as_str()).unwrap_or("1");
                    let tag = match level {
                        "2" => "h2",
                        "3" => "h3",
                        _ => "h1",
                    };
                    let el = doc.create_element(tag).ok()?;
                    if let Some(bid) = attrs.get("blockId") {
                        el.set_attribute("data-block-id", bid).ok()?;
                    }
                    apply_block_align(&el, attrs);
                    render_children(doc, &el, content);
                    return Some(el.into());
                }
                NodeType::CodeBlock => {
                    let pre = doc.create_element("pre").ok()?;
                    if let Some(bid) = attrs.get("blockId") {
                        pre.set_attribute("data-block-id", bid).ok()?;
                    }
                    apply_block_align(&pre, attrs);
                    let code = doc.create_element("code").ok()?;
                    if let Some(lang) = attrs.get("language") {
                        if !lang.is_empty() {
                            code.set_attribute("class", &format!("language-{lang}"))
                                .ok()?;
                        }
                    }
                    render_children(doc, &code, content);
                    pre.append_child(&code).ok()?;
                    return Some(pre.into());
                }
                NodeType::Image => {
                    let el = doc.create_element("img").ok()?;
                    if let Some(src) = attrs.get("src") {
                        if is_safe_url(src) {
                            el.set_attribute("src", src).ok()?;
                        }
                    }
                    if let Some(alt) = attrs.get("alt") {
                        el.set_attribute("alt", alt).ok()?;
                    }
                    return Some(el.into());
                }
                NodeType::Mention => {
                    // #148 slice 6 — mention chip. `class="mention"`
                    // matches the pre-existing text+MarkType::Mention
                    // DOM (see clipboard.rs::tag_to_mark) so
                    // stylesheets and paste round-trip keep working.
                    // `data-user-id` carries the opaque server-side
                    // id; the inner text is the display name
                    // snapshot from insert-time.
                    //
                    // `contenteditable="false"` makes the whole
                    // chip a single cursor stop — the caret can
                    // land before or after but never inside, and
                    // Backspace deletes the atom in one keystroke
                    // (matching `is_atom = true` in the model).
                    let el = doc.create_element("span").ok()?;
                    el.set_attribute("class", "mention").ok()?;
                    el.set_attribute("contenteditable", "false").ok()?;
                    if let Some(uid) = attrs.get("user_id") {
                        el.set_attribute("data-user-id", uid).ok()?;
                    }
                    let display = attrs
                        .get("display")
                        .map(String::as_str)
                        .unwrap_or("");
                    el.set_text_content(Some(display));
                    return Some(el.into());
                }
                NodeType::Embed => {
                    // M-P6 piece B: wrap the iframe in a non-editable
                    // div so the editor treats the embed as a single
                    // atom — clicks inside the iframe stay with the
                    // third-party content, not the editor's selection.
                    // The wrapper carries `contenteditable="false"`;
                    // the iframe itself rides with sandbox + lazy
                    // loading + `strict-origin-when-cross-origin`
                    // (sends only our origin, not the document path —
                    // `no-referrer` broke YouTube, which validates the
                    // embedding origin: Error 153). Height is clamped at
                    // render-time (200..1200 px); missing attributes
                    // fall through to provider-agnostic defaults.
                    let wrapper = doc.create_element("div").ok()?;
                    wrapper.set_attribute("class", "embed-block").ok()?;
                    wrapper.set_attribute("contenteditable", "false").ok()?;
                    let iframe = doc.create_element("iframe").ok()?;
                    if let Some(url) = attrs.get("url") {
                        if is_safe_url(url) {
                            iframe.set_attribute("src", url).ok()?;
                        }
                    }
                    if let Some(title) = attrs.get("title") {
                        iframe.set_attribute("title", title).ok()?;
                    }
                    iframe
                        .set_attribute("sandbox", "allow-scripts allow-same-origin")
                        .ok()?;
                    iframe
                        .set_attribute("referrerpolicy", "strict-origin-when-cross-origin")
                        .ok()?;
                    iframe.set_attribute("loading", "lazy").ok()?;
                    iframe.set_attribute("frameborder", "0").ok()?;
                    iframe.set_attribute("width", "100%").ok()?;
                    let raw_height = attrs
                        .get("height")
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(400);
                    let clamped = raw_height.clamp(200, 1200);
                    iframe
                        .set_attribute("height", &clamped.to_string())
                        .ok()?;
                    wrapper.append_child(&iframe).ok()?;
                    return Some(wrapper.into());
                }
                _ => {}
            }

            // #136 — live-app block plugin fallback. Blocks that
            // own this NodeType own the whole render (they walk
            // their own `content`); return whatever they produce.
            // See `design/live-app-blocks.md`.
            if let Some(block) = super::blocks::view_for(*node_type) {
                return block.render(doc, *node_type, attrs, content);
            }

            // Generic element rendering
            let tag = node_type_to_tag(*node_type);
            let el = doc.create_element(tag).ok()?;

            // Set block ID for commentable blocks
            if let Some(bid) = attrs.get("blockId") {
                el.set_attribute("data-block-id", bid).ok()?;
            }

            // Set data attributes for special types
            match node_type {
                NodeType::TaskItem => {
                    let checked = attrs
                        .get("checked")
                        .map(|v| v == "true")
                        .unwrap_or(false);
                    el.set_attribute("data-type", "taskItem").ok()?;
                    el.set_attribute("data-checked", &checked.to_string())
                        .ok()?;
                }
                NodeType::TaskList => {
                    el.set_attribute("data-type", "taskList").ok()?;
                }
                NodeType::TableCell | NodeType::TableHeader => {
                    if let Some(align) = attrs.get("align") {
                        if matches!(align.as_str(), "left" | "center" | "right") {
                            el.set_attribute("style", &format!("text-align: {align}")).ok()?;
                        }
                    }
                }
                NodeType::Paragraph | NodeType::Blockquote => {
                    apply_block_align(&el, attrs);
                }
                _ => {}
            }

            if !node_type.is_leaf() {
                render_children(doc, &el, content);
            }

            Some(el.into())
        }
    }
}

/// Apply block-level `text-align` styling from a node's `align`
/// attribute. Reads `attrs["align"]` and sets the element's inline
/// `style` to `text-align: <value>` when the value is "center" or
/// "right". "left" is the natural default and is intentionally NOT
/// written so the cleared state matches the no-attr state. Mirrors
/// the same allowlist that `commands::set_alignment` enforces.
fn apply_block_align(
    el: &Element,
    attrs: &std::collections::HashMap<String, String>,
) {
    if let Some(align) = attrs.get("align") {
        if matches!(align.as_str(), "center" | "right") {
            let _ = el.set_attribute("style", &format!("text-align: {align}"));
        }
    }
}

/// Render children of a Fragment into a DOM Element.
/// Appends a `<br>` to empty blocks so the browser gives them height and
/// allows the cursor to be placed inside them.
fn render_children(doc: &Document, parent: &Element, content: &super::model::Fragment) {
    if content.children.is_empty() {
        if let Ok(br) = doc.create_element("br") {
            let _ = br.set_attribute("data-sentinel", "");
            let _ = parent.append_child(&br);
        }
        return;
    }
    for child in &content.children {
        if let Some(child_dom) = render_node(doc, child) {
            parent.append_child(&child_dom).unwrap_or_else(|_| child_dom);
        }
    }
}

/// Create a DOM element for a mark.
fn create_mark_element(doc: &Document, mark: &Mark) -> Option<Element> {
    let tag = match mark.mark_type {
        MarkType::Bold => "strong",
        MarkType::Italic => "em",
        MarkType::Underline => "u",
        MarkType::Strike => "s",
        MarkType::Code => "code",
        MarkType::Link => "a",
        MarkType::TextColor => "span",
        MarkType::Highlight => "mark",
        MarkType::Subscript => "sub",
        MarkType::Superscript => "sup",
        // #148: user @-mention renders as a span the styling
        // then hooks via the `.mention` class.
        MarkType::Mention => "span",
    };

    let el = doc.create_element(tag).ok()?;

    if mark.mark_type == MarkType::Link {
        if let Some(href) = mark.attrs.get("href") {
            if is_safe_url(href) {
                el.set_attribute("href", href).ok()?;
            }
        }
        if let Some(title) = mark.attrs.get("title") {
            if !title.is_empty() {
                el.set_attribute("title", title).ok()?;
            }
        }
        el.set_attribute("rel", "noopener noreferrer nofollow")
            .ok()?;
        el.set_attribute("target", "_blank").ok()?;
    }

    if mark.mark_type == MarkType::TextColor {
        if let Some(color) = mark.attrs.get("color") {
            if is_safe_color(color) {
                el.set_attribute("style", &format!("color: {color}")).ok()?;
            }
        }
    }

    if mark.mark_type == MarkType::Highlight {
        if let Some(color) = mark.attrs.get("color") {
            if is_safe_color(color) {
                el.set_attribute("style", &format!("background: {color}")).ok()?;
            }
        }
    }

    if mark.mark_type == MarkType::Mention {
        // Style hook + machine-readable id. `data-user-id` is
        // treated as opaque by the renderer — future UX (hover
        // card, avatar) reads it back from the DOM.
        el.set_attribute("class", "mention").ok()?;
        if let Some(uid) = mark.attrs.get("user_id") {
            if !uid.is_empty() {
                el.set_attribute("data-user-id", uid).ok()?;
            }
        }
    }

    Some(el)
}

/// Check that a URL uses a safe protocol.
pub(crate) fn is_safe_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        // Same-origin relative path, but NOT a protocol-relative `//host`
        // — the browser resolves `//evil.com` against the page protocol
        // (→ `https://evil.com`), so it must be rejected (#2).
        || (lower.starts_with('/') && !lower.starts_with("//"))
}

/// Check that a color value is safe (hex color or named color, no script injection).
pub(crate) fn is_safe_color(color: &str) -> bool {
    let c = color.trim();
    // Allow hex colors: #RGB, #RRGGBB, #RRGGBBAA
    if c.starts_with('#') && c.len() <= 9 && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()) {
        return true;
    }
    // Allow simple named colors
    matches!(
        c.to_lowercase().as_str(),
        "red" | "blue" | "green" | "orange" | "purple" | "yellow" | "pink"
            | "brown" | "gray" | "grey" | "black" | "white" | "cyan" | "magenta"
            | "inherit" | "transparent"
    )
}

/// Map a NodeType to an HTML tag name.
fn node_type_to_tag(nt: NodeType) -> &'static str {
    match nt {
        NodeType::Doc => "div",
        NodeType::Paragraph => "p",
        NodeType::Heading => "h1", // overridden in render_node
        NodeType::BulletList => "ul",
        NodeType::OrderedList => "ol",
        NodeType::ListItem => "li",
        NodeType::TaskList => "ul",
        NodeType::TaskItem => "li",
        NodeType::Blockquote => "blockquote",
        NodeType::CodeBlock => "pre", // overridden in render_node
        NodeType::HorizontalRule => "hr",
        NodeType::HardBreak => "br",
        NodeType::Image => "img", // overridden in render_node
        NodeType::Table => "table",
        NodeType::TableRow => "tr",
        NodeType::TableCell => "td",
        NodeType::TableHeader => "th",
        NodeType::Embed => "iframe",
        NodeType::Calendar => "div", // overridden in render_node
        NodeType::CalendarEvent => "span", // overridden in render_node
        NodeType::Kanban => "div", // overridden in render_node
        NodeType::KanbanColumn => "div", // overridden in render_node
        NodeType::KanbanCard => "div", // overridden in render_node
        // #148 slice 6 — mention chip. Attribute + inner text
        // emission handled by the renderer's per-node override
        // (added in PR-B); this base tag is `span` to match the
        // pre-existing text+MarkType::Mention DOM output for
        // paste round-trip stability.
        NodeType::Mention => "span",
    }
}

/// Viewport top + height of a collapsed caret at model position `pos`,
/// or None if it can't be resolved. Mirrors the bin-side
/// `dom_position_for_model_pos` but stays inside the editor module (which
/// is compiled into the library, where `crate::components` isn't
/// visible), reusing the local `find_dom_position`.
fn caret_rect_for_pos(pos: usize) -> Option<(f64, f64)> {
    let document = web_sys::window()?.document()?;
    let editor: HtmlElement = document
        .query_selector(".editor-content")
        .ok()??
        .dyn_into()
        .ok()?;
    let (node, offset) = find_dom_position(&editor, pos)?;
    let range = document.create_range().ok()?;
    range.set_start(&node, offset as u32).ok()?;
    range.set_end(&node, offset as u32).ok()?;
    let rect = range.get_bounding_client_rect();
    if rect.height() < 1.0 {
        return None;
    }
    Some((rect.top(), rect.height()))
}

/// #78: is the caret on the same visual line as its block's near edge
/// (`block_edge` — start for ArrowUp, end for ArrowDown)? Compares the
/// viewport top of each position's DOM rect: equal tops (within half a
/// caret height) ⇒ same visual line, so an adjacent atom block should be
/// crossed. Returns false when either rect is unavailable, degrading to
/// the pure-model boundary path (which already ran).
fn caret_on_block_edge_line(caret_pos: usize, block_edge: usize) -> bool {
    if caret_pos == block_edge {
        return true;
    }
    match (caret_rect_for_pos(caret_pos), caret_rect_for_pos(block_edge)) {
        (Some((caret_top, h)), Some((edge_top, _))) => {
            (caret_top - edge_top).abs() < (h * 0.5).max(2.0)
        }
        _ => false,
    }
}

// ─── DOM Position Mapping ───────────────────────────────────────

/// Find the DOM node and offset for a model position.
pub(crate) fn find_dom_position(container: &HtmlElement, target_pos: usize) -> Option<(DomNode, usize)> {
    let mut pos = 0;
    find_in_element(container.as_ref(), &mut pos, target_pos)
}

fn find_in_element(
    element: &Element,
    pos: &mut usize,
    target: usize,
) -> Option<(DomNode, usize)> {
    let children = element.child_nodes();
    let len = children.length();

    for i in 0..len {
        let child = children.item(i)?;

        if child.node_type() == DomNode::TEXT_NODE {
            let text = child.text_content().unwrap_or_default();
            let text_len = char_len(&text);
            if target >= *pos && target <= *pos + text_len {
                return Some((child, target - *pos));
            }
            *pos += text_len;
        } else if child.node_type() == DomNode::ELEMENT_NODE {
            let el = child.dyn_ref::<Element>()?;
            let tag = el.tag_name().to_lowercase();

            if is_mark_tag(&tag) {
                // Mark wrappers are transparent
                if let Some(result) = find_in_element(el, pos, target) {
                    return Some(result);
                }
            } else if let Some(atom_size) = read_atom_size(el) {
                // Wrapper for a model-side atom (Calendar, Kanban, …).
                // Mirror of dom_to_model_walk's atom handling: the
                // subtree is opaque, so target == atom's start goes
                // to index i inside the parent (before the wrapper),
                // target == atom's end goes to index i+1 (after the
                // wrapper), and any interior target — which the
                // schema shouldn't produce for isolating atoms —
                // falls back to "before". Without this, model position
                // N past a calendar descended into the calendar's
                // toolbar/day-cell divs and placed the DOM caret
                // inside the calendar; visible symptom was "cursor
                // jumps above the calendar" on the first keystroke
                // past it, which then round-tripped through
                // dom_to_model_walk and drove a caret-oscillation
                // loop that spun the browser CPU.
                if target == *pos {
                    return Some((element.clone().into(), i as usize));
                }
                if target > *pos && target < *pos + atom_size {
                    return Some((element.clone().into(), i as usize));
                }
                if target == *pos + atom_size {
                    return Some((element.clone().into(), (i + 1) as usize));
                }
                *pos += atom_size;
            } else if is_leaf_tag(&tag) {
                if is_sentinel(el) {
                    continue; // skip rendering-only <br>
                }
                if target == *pos {
                    return Some((element.clone().into(), i as usize));
                }
                *pos += 1;
            } else {
                // Block element
                if target == *pos {
                    // Target is at the open boundary (before the block in the parent)
                    return Some((element.clone().into(), i as usize));
                }
                *pos += 1; // open boundary

                if let Some(result) = find_in_element(el, pos, target) {
                    return Some(result);
                }

                *pos += 1; // close boundary
            }
        }
    }

    if target == *pos {
        Some((element.clone().into(), len as usize))
    } else {
        None
    }
}

/// Map a DOM position (node + offset) to a model position.
fn dom_position_to_model(
    container: &HtmlElement,
    node: &DomNode,
    offset: usize,
) -> Option<usize> {
    let mut pos = 0;
    dom_to_model_walk(container.as_ref(), node, offset, &mut pos)
}

fn dom_to_model_walk(
    element: &Element,
    target_node: &DomNode,
    target_offset: usize,
    pos: &mut usize,
) -> Option<usize> {
    let children = element.child_nodes();
    let len = children.length();

    // Check if target is this element with a child-index offset
    if element
        .dyn_ref::<DomNode>()
        .map(|n| n.is_same_node(Some(target_node)))
        .unwrap_or(false)
    {
        let mut child_pos = *pos;
        for i in 0..target_offset.min(len as usize) {
            if let Some(child) = children.item(i as u32) {
                child_pos += dom_node_model_size(&child);
            }
        }
        return Some(child_pos);
    }

    for i in 0..len {
        let child = children.item(i)?;

        if child.node_type() == DomNode::TEXT_NODE {
            if child.is_same_node(Some(target_node)) {
                return Some(*pos + target_offset);
            }
            let text = child.text_content().unwrap_or_default();
            *pos += char_len(&text);
        } else if child.node_type() == DomNode::ELEMENT_NODE {
            let el = child.dyn_ref::<Element>()?;
            let tag = el.tag_name().to_lowercase();

            if is_mark_tag(&tag) {
                if let Some(result) = dom_to_model_walk(el, target_node, target_offset, pos) {
                    return Some(result);
                }
            } else if let Some(atom_size) = read_atom_size(el) {
                // Wrapper for a model-side atom (Calendar, Kanban,
                // etc.). The DOM subtree can be arbitrarily large,
                // but the model treats it as a single opaque unit
                // of size `atom_size`. A click that lands on the
                // wrapper or anywhere inside it maps to the atom's
                // start position; a walk past the wrapper advances
                // pos by exactly `atom_size`.
                if child.is_same_node(Some(target_node)) {
                    return Some(*pos);
                }
                if el.contains(Some(target_node)) {
                    return Some(*pos);
                }
                *pos += atom_size;
            } else if is_leaf_tag(&tag) {
                if is_sentinel(el) {
                    continue; // skip rendering-only <br>
                }
                if child.is_same_node(Some(target_node)) {
                    return Some(*pos);
                }
                *pos += 1;
            } else {
                *pos += 1; // open boundary
                if let Some(result) = dom_to_model_walk(el, target_node, target_offset, pos) {
                    return Some(result);
                }
                *pos += 1; // close boundary
            }
        }
    }

    None
}

/// Compute the model size of a DOM node.
fn dom_node_model_size(node: &DomNode) -> usize {
    if node.node_type() == DomNode::TEXT_NODE {
        char_len(&node.text_content().unwrap_or_default())
    } else if node.node_type() == DomNode::ELEMENT_NODE {
        if let Some(el) = node.dyn_ref::<Element>() {
            let tag = el.tag_name().to_lowercase();
            if is_mark_tag(&tag) {
                let children = el.child_nodes();
                let mut size = 0;
                for i in 0..children.length() {
                    if let Some(child) = children.item(i) {
                        size += dom_node_model_size(&child);
                    }
                }
                size
            } else if let Some(atom_size) = read_atom_size(el) {
                atom_size
            } else if is_leaf_tag(&tag) {
                if is_sentinel(el) { 0 } else { 1 }
            } else {
                let children = el.child_nodes();
                let mut size = 2; // open + close
                for i in 0..children.length() {
                    if let Some(child) = children.item(i) {
                        size += dom_node_model_size(&child);
                    }
                }
                size
            }
        } else {
            0
        }
    } else {
        0
    }
}

/// Read the `data-atom-size` attribute from an element that wraps
/// a model-side atom (Calendar, Kanban, etc). Returns `None` for
/// elements without the attribute (normal containers). The atom
/// itself decides its model size at render time based on the
/// child count — see `frontend/src/editor/blocks/*.rs`.
fn read_atom_size(el: &Element) -> Option<usize> {
    el.get_attribute("data-atom-size")?.parse::<usize>().ok()
}

fn is_mark_tag(tag: &str) -> bool {
    matches!(
        tag,
        "strong" | "b" | "em" | "i" | "u" | "s" | "del" | "code" | "a" | "span" | "mark"
            | "sub" | "sup"
    )
}

fn is_leaf_tag(tag: &str) -> bool {
    matches!(tag, "hr" | "br" | "img")
}

/// Check if a DOM element is a sentinel `<br>` (rendering artifact, not a model node).
fn is_sentinel(el: &Element) -> bool {
    el.has_attribute("data-sentinel")
}

/// Read DOM selection from a container element.
fn read_dom_selection_from(container: &HtmlElement) -> Option<Selection> {
    let window = web_sys::window()?;
    let dom_sel = window.get_selection().ok()??;

    if dom_sel.range_count() == 0 {
        return None;
    }

    let anchor_node = dom_sel.anchor_node()?;
    let anchor_offset = dom_sel.anchor_offset() as usize;
    let focus_node = dom_sel.focus_node()?;
    let focus_offset = dom_sel.focus_offset() as usize;

    let anchor_pos = dom_position_to_model(container, &anchor_node, anchor_offset)?;
    let focus_pos = dom_position_to_model(container, &focus_node, focus_offset)?;

    if anchor_pos == focus_pos {
        Some(Selection::cursor(anchor_pos))
    } else {
        Some(Selection::text(anchor_pos, focus_pos))
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_safe_url_rejects_protocol_relative() {
        // #2: `//host` resolves against the page protocol (→ https://host),
        // so it must be rejected even though it starts with `/`.
        assert!(!is_safe_url("//evil.com"));
        assert!(!is_safe_url("  //evil.com/path"));
        assert!(!is_safe_url("//evil.com"));
        // Genuine same-origin relative paths still pass.
        assert!(is_safe_url("/documents/123"));
        assert!(is_safe_url("/"));
        // Normal absolute + mailto still pass; dangerous schemes rejected.
        assert!(is_safe_url("https://example.com"));
        assert!(is_safe_url("mailto:a@b.com"));
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("data:text/html,x"));
    }

    #[test]
    fn node_type_to_tag_mapping() {
        assert_eq!(node_type_to_tag(NodeType::Paragraph), "p");
        assert_eq!(node_type_to_tag(NodeType::BulletList), "ul");
        assert_eq!(node_type_to_tag(NodeType::OrderedList), "ol");
        assert_eq!(node_type_to_tag(NodeType::ListItem), "li");
        assert_eq!(node_type_to_tag(NodeType::Blockquote), "blockquote");
        assert_eq!(node_type_to_tag(NodeType::CodeBlock), "pre");
        assert_eq!(node_type_to_tag(NodeType::HorizontalRule), "hr");
        assert_eq!(node_type_to_tag(NodeType::HardBreak), "br");
        assert_eq!(node_type_to_tag(NodeType::Image), "img");
    }

    #[test]
    fn mark_tag_detection() {
        assert!(is_mark_tag("strong"));
        assert!(is_mark_tag("em"));
        assert!(is_mark_tag("u"));
        assert!(is_mark_tag("s"));
        assert!(is_mark_tag("code"));
        assert!(is_mark_tag("a"));
        assert!(is_mark_tag("b"));
        assert!(is_mark_tag("i"));
        assert!(!is_mark_tag("p"));
        assert!(!is_mark_tag("div"));
    }

    #[test]
    fn leaf_tag_detection() {
        assert!(is_leaf_tag("hr"));
        assert!(is_leaf_tag("br"));
        assert!(is_leaf_tag("img"));
        assert!(!is_leaf_tag("p"));
        assert!(!is_leaf_tag("strong"));
    }

    // ─── Position-mapping invariant tests ────────────────────
    //
    // The DOM walker (find_in_element) must consume exactly node_size()
    // positions for every model node. If these diverge, remote cursor
    // positions will be wrong. We test this by computing "DOM size"
    // from the model using the same rules the walker uses:
    //   - Text node: char_len(text)
    //   - Mark wrapper: transparent (0 overhead)
    //   - Leaf element (hr, br, img): 1
    //   - Block element: 2 + children_dom_size
    // and checking it equals model node_size().

    use super::super::model::{Fragment, Mark, MarkType, Node, NodeType};

    /// Compute the position-space size of a model node as the DOM walker
    /// would count it. This mirrors find_in_element's counting rules.
    fn dom_walker_size(node: &Node) -> usize {
        match node {
            Node::Text { text, marks } => {
                // Mark wrappers are transparent — they don't add boundaries.
                // The walker just counts the text chars.
                let _ = marks; // marks don't affect position size
                super::super::model::char_len(text)
            }
            Node::Element { node_type, content, .. } => {
                let tag = node_type_to_tag(*node_type);
                if is_leaf_tag(tag) {
                    // Leaf: hr, br, img → size 1
                    1
                } else {
                    // Block element → open(1) + children + close(1)
                    let children_size: usize = content.children.iter()
                        .map(|c| dom_walker_size(c))
                        .sum();
                    2 + children_size
                }
            }
        }
    }

    /// Assert that dom_walker_size matches node_size for a document node
    /// and all its descendants.
    fn assert_sizes_match(node: &Node) {
        let dom_size = dom_walker_size(node);
        let model_size = node.node_size();
        assert_eq!(
            dom_size, model_size,
            "DOM walker size ({dom_size}) != model node_size ({model_size}) for {node:?}"
        );
        // Recurse into children
        if let Node::Element { content, .. } = node {
            for child in &content.children {
                assert_sizes_match(child);
            }
        }
    }

    #[test]
    fn position_sizes_match_plain_paragraph() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn position_sizes_match_heading() {
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("level".to_string(), "2".to_string());
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_attrs(
                NodeType::Heading,
                attrs,
                Fragment::from(vec![Node::text("Title")]),
            )]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn position_sizes_match_bold_text() {
        // Bold text in a paragraph: <p><strong>hello</strong></p>
        // Marks are transparent, so size = just the text chars
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text_with_marks(
                    "hello",
                    vec![Mark::new(MarkType::Bold)],
                )]),
            )]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn position_sizes_match_mixed_marks() {
        // <p>plain <strong>bold</strong> end</p>
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![
                    Node::text("plain "),
                    Node::text_with_marks("bold", vec![Mark::new(MarkType::Bold)]),
                    Node::text(" end"),
                ]),
            )]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn position_sizes_match_nested_list() {
        // <ul><li><p>item 1</p></li><li><p>item 2</p></li></ul>
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("item 1")]),
                        )]),
                    ),
                    Node::element_with_content(
                        NodeType::ListItem,
                        Fragment::from(vec![Node::element_with_content(
                            NodeType::Paragraph,
                            Fragment::from(vec![Node::text("item 2")]),
                        )]),
                    ),
                ]),
            )]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn position_sizes_match_heading_then_list() {
        // The exact structure from the bug: heading followed by bullet list
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Heading,
                    Fragment::from(vec![Node::text("Conversation Pane")]),
                ),
                Node::element_with_content(
                    NodeType::BulletList,
                    Fragment::from(vec![
                        Node::element_with_content(
                            NodeType::ListItem,
                            Fragment::from(vec![Node::element_with_content(
                                NodeType::Paragraph,
                                Fragment::from(vec![Node::text("Log of every action")]),
                            )]),
                        ),
                        Node::element_with_content(
                            NodeType::ListItem,
                            Fragment::from(vec![Node::element_with_content(
                                NodeType::Paragraph,
                                Fragment::from(vec![Node::text("Document-level messages")]),
                            )]),
                        ),
                    ]),
                ),
            ]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn position_sizes_match_code_marks_in_list() {
        // List items with inline code marks: <li><p>Use <code>Ctrl+C</code> to copy</p></li>
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::BulletList,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::ListItem,
                    Fragment::from(vec![Node::element_with_content(
                        NodeType::Paragraph,
                        Fragment::from(vec![
                            Node::text("Use "),
                            Node::text_with_marks("Ctrl+C", vec![Mark::new(MarkType::Code)]),
                            Node::text(" to copy"),
                        ]),
                    )]),
                )]),
            )]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn position_sizes_match_hr_and_image() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("before")]),
                ),
                Node::element(NodeType::HorizontalRule),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("after")]),
                ),
            ]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn position_sizes_match_blockquote() {
        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Blockquote,
                Fragment::from(vec![Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("quoted text")]),
                )]),
            )]),
        );
        assert_sizes_match(&doc);
    }

    #[test]
    fn safe_url_validation() {
        assert!(is_safe_url("https://example.com"));
        assert!(is_safe_url("http://example.com"));
        assert!(is_safe_url("mailto:user@example.com"));
        assert!(is_safe_url("/relative/path"));
        assert!(!is_safe_url("javascript:alert(1)"));
        assert!(!is_safe_url("JAVASCRIPT:alert(1)"));
        assert!(!is_safe_url("data:text/html,<script>alert(1)</script>"));
        assert!(!is_safe_url("vbscript:alert(1)"));
    }
}
