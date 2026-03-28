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
    /// Stored (event_name, closure) pairs for cleanup in destroy().
    listeners: Vec<(&'static str, Closure<dyn Fn(web_sys::Event)>)>,
    /// selectionchange listener (attached to document, not container).
    selectionchange_closure: Option<Closure<dyn Fn(web_sys::Event)>>,
}

impl EditorView {
    /// Create a new editor view attached to a container element.
    pub fn new(
        container: HtmlElement,
        state: EditorState,
        dispatch: impl Fn(Transaction) + 'static,
    ) -> Self {
        container
            .set_attribute("contenteditable", "true")
            .unwrap_or(());
        container
            .set_attribute("spellcheck", "false")
            .unwrap_or(());
        container.class_list().add_1("editor-content").unwrap_or(());

        let state = Rc::new(RefCell::new(state));
        let dispatch = Rc::new(dispatch) as Rc<dyn Fn(Transaction)>;
        let composing = Rc::new(RefCell::new(false));
        let just_composed = Rc::new(RefCell::new(false));

        let mut view = Self {
            container,
            state,
            dispatch,
            composing,
            just_composed,
            listeners: Vec::new(),
            selectionchange_closure: None,
        };

        view.render();
        view.attach_listeners();
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
        let keymap = super::keymap::default_keymap();

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

            let current_state = state_kd.borrow().clone();

            let handled = keymap.handle(
                &key, ctrl, meta, shift, alt,
                &current_state,
                &|txn| { dispatch_kd(txn); },
            );

            if handled {
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
                                            }
                                        }
                                    }
                                    dispatch(insert_txn);
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
                            if pos > 0 {
                                // Try joining with previous block first (backspace at block start)
                                if let Ok(txn) = state_with_sel.transaction().join_backward() {
                                    dispatch(txn);
                                } else if let Ok(txn) = state_with_sel.transaction().delete(pos - 1, pos) {
                                    dispatch(txn);
                                }
                            }
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
                                if let Ok(txn) =
                                    state_with_sel.transaction().delete(pos, pos + 1)
                                {
                                    dispatch(txn);
                                }
                            }
                        } else if let Ok(txn) = state_with_sel.transaction().delete_selection() {
                            dispatch(txn);
                        }
                    }
                }
                "insertParagraph" => {
                    event.prevent_default();
                    if let Some(sel) = read_dom_selection_from(&container2) {
                        let state_with_sel = EditorState {
                            selection: sel,
                            ..current_state
                        };
                        if let Ok(txn) = state_with_sel.transaction().split_block() {
                            dispatch(txn);
                        }
                    }
                }
                // Prevent unhandled input types from mutating DOM without model update
                "deleteWordBackward" | "deleteWordForward"
                | "deleteSoftLineBackward" | "deleteSoftLineForward"
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

        let on_comp_end = Closure::wrap(Box::new(move |event: web_sys::Event| {
            *composing_end.borrow_mut() = false;

            if let Some(ce) = event.dyn_ref::<web_sys::CompositionEvent>() {
                if let Some(data) = ce.data() {
                    if !data.is_empty() {
                        // Only set the guard when we actually insert composed text
                        *just_composed_end.borrow_mut() = true;

                        let current_state = state_comp.borrow().clone();
                        if let Some(sel) = read_dom_selection_from(&container_comp) {
                            let state_with_sel = EditorState {
                                selection: sel,
                                ..current_state
                            };
                            if let Ok(txn) = state_with_sel.transaction().insert_text(&data) {
                                dispatch_comp(txn);
                            }
                        }
                    }
                }
            }
        }) as Box<dyn Fn(web_sys::Event)>);
        self.add_listener("compositionend", on_comp_end);

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
                    render_children(doc, &el, content);
                    return Some(el.into());
                }
                NodeType::CodeBlock => {
                    let pre = doc.create_element("pre").ok()?;
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
                _ => {}
            }

            // Generic element rendering
            let tag = node_type_to_tag(*node_type);
            let el = doc.create_element(tag).ok()?;

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
                _ => {}
            }

            if !node_type.is_leaf() {
                render_children(doc, &el, content);
            }

            Some(el.into())
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
    };

    let el = doc.create_element(tag).ok()?;

    if mark.mark_type == MarkType::Link {
        if let Some(href) = mark.attrs.get("href") {
            // Validate URL protocol to prevent XSS
            if is_safe_url(href) {
                el.set_attribute("href", href).ok()?;
            }
        }
        el.set_attribute("rel", "noopener noreferrer nofollow")
            .ok()?;
        el.set_attribute("target", "_blank").ok()?;
    }

    Some(el)
}

/// Check that a URL uses a safe protocol.
fn is_safe_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with('/')
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
    }
}

// ─── DOM Position Mapping ───────────────────────────────────────

/// Find the DOM node and offset for a model position.
fn find_dom_position(container: &HtmlElement, target_pos: usize) -> Option<(DomNode, usize)> {
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

fn is_mark_tag(tag: &str) -> bool {
    matches!(
        tag,
        "strong" | "b" | "em" | "i" | "u" | "s" | "del" | "code" | "a"
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
