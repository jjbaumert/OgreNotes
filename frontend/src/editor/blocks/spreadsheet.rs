// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Embedded grid UI. Only local viewer state lives here; every write is a
//! normal editor transaction. EditorView owns these islands and their listeners.
use super::{LiveAppBlockInsert, LiveAppBlockView};
use crate::editor::{
    commands::embedded_spreadsheet as commands,
    model::{Fragment, Node, NodeType},
    state::{EditorState, Transaction},
};
use ogrenotes_spreadsheet::embedded::{self, SheetData};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{Document, Element, HtmlElement, HtmlInputElement, Node as DomNode};

pub struct SpreadsheetView;
pub struct SpreadsheetInsert;
impl LiveAppBlockView for SpreadsheetView {
    fn node_types(&self) -> &'static [NodeType] {
        &[NodeType::Spreadsheet, NodeType::SpreadsheetSheet]
    }
    fn render(
        &self,
        doc: &Document,
        node_type: NodeType,
        attrs: &HashMap<String, String>,
        content: &Fragment,
    ) -> Option<DomNode> {
        if node_type != NodeType::Spreadsheet {
            return None;
        }
        let root = doc.create_element("div").ok()?;
        root.set_attribute("class", "embedded-spreadsheet").ok()?;
        root.set_attribute("contenteditable", "false").ok()?;
        root.set_attribute("data-atom-size", &(content.size() + 2).to_string())
            .ok()?;
        root.set_attribute("data-block-id", attrs.get("blockId")?)
            .ok()?;
        root.set_attribute("role", "group").ok()?;
        root.set_attribute("aria-label", &label("embedded-sheet-label"))
            .ok()?;
        root.set_text_content(Some(&label("embedded-sheet-label")));
        Some(root.into())
    }
}
impl LiveAppBlockInsert for SpreadsheetInsert {
    fn id(&self) -> &'static str {
        "spreadsheet"
    }
    fn label_key(&self) -> &'static str {
        "insert-spreadsheet-label"
    }
    fn description_key(&self) -> &'static str {
        "insert-spreadsheet-description"
    }
    fn icon(&self) -> &'static str {
        "▦"
    }
    fn build_default_node(&self) -> Node {
        Node::element_with_attrs(
            NodeType::Spreadsheet,
            HashMap::from([
                ("version".into(), "1".into()),
                ("title".into(), label("embedded-sheet-label")),
            ]),
            Fragment::from(vec![commands::new_sheet("Sheet1")]),
        )
    }
}
fn label(key: &str) -> String {
    crate::i18n::translate(key, None)
}
pub fn event_inside(event: &web_sys::Event) -> bool {
    event
        .target()
        .and_then(|target| target.dyn_into::<Element>().ok())
        .and_then(|el| el.closest(".embedded-spreadsheet").ok().flatten())
        .is_some()
}
pub fn focused_inside() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
        .and_then(|el| el.closest(".embedded-spreadsheet").ok().flatten())
        .is_some()
}
#[derive(Default)]
struct Local {
    sheets: Vec<SheetData>,
    active: String,
    selected: (usize, usize),
    row_start: usize,
    col_start: usize,
    dirty: bool,
}
type SheetListener = (&'static str, Closure<dyn Fn(web_sys::Event)>);

/// Owned by one EditorView. No document/global listeners or leaked closures.
pub struct SpreadsheetIsland {
    pub root: HtmlElement,
    local: Rc<RefCell<Local>>,
    listeners: Vec<SheetListener>,
}
impl SpreadsheetIsland {
    pub fn new(
        root: HtmlElement,
        state: Rc<RefCell<EditorState>>,
        dispatch: Rc<dyn Fn(Transaction)>,
        history: Rc<RefCell<crate::editor::plugins::HistoryPlugin>>,
        readonly: bool,
        rendering: Rc<Cell<bool>>,
    ) -> Self {
        let local = Rc::new(RefCell::new(Local::default()));
        let mut island = Self {
            root,
            local,
            listeners: Vec::new(),
        };
        if readonly {
            island.root.set_attribute("data-readonly", "true").ok();
        }
        for name in [
            "click", "input", "change", "keydown", "dblclick", "focusout",
        ] {
            let root = island.root.clone();
            let local = island.local.clone();
            let state = state.clone();
            let dispatch = dispatch.clone();
            let history = history.clone();
            let rendering = rendering.clone();
            let listener = Closure::wrap(Box::new(move |event: web_sys::Event| {
                let Some(target) = event.target().and_then(|t| t.dyn_into::<Element>().ok()) else {
                    return;
                };
                let Some(book) = root.get_attribute("data-block-id") else {
                    return;
                };
                let action = target
                    .get_attribute("data-sheet-action")
                    .unwrap_or_default();
                let mut txn = None;
                if name == "input" && action == "raw" {
                    local.borrow_mut().dirty = true;
                    clear_draft_error(&root);
                    return;
                }
                if name == "focusout" {
                    if readonly
                        || rendering.get()
                        || root.has_attribute("data-sheet-refreshing")
                        || !local.borrow().dirty
                    {
                        return;
                    }
                    let next = js_sys::Reflect::get(event.as_ref(), &"relatedTarget".into())
                        .ok()
                        .and_then(|target| target.dyn_into::<DomNode>().ok());
                    if root.contains(next.as_ref()) {
                        return;
                    }
                    let result = commit_raw(&root, &local, &state.borrow(), &book);
                    if let Ok(txn) = result {
                        local.borrow_mut().dirty = false;
                        if let Some(txn) = txn {
                            dispatch(txn);
                        }
                        refresh(&root, &local, false, readonly);
                    }
                    return;
                }
                if name == "keydown" {
                    let Some(key) = event.dyn_ref::<web_sys::KeyboardEvent>() else {
                        return;
                    };
                    if key.is_composing() {
                        return;
                    }
                    if (key.ctrl_key() || key.meta_key())
                        && matches!(key.key().to_lowercase().as_str(), "z" | "y")
                        && !readonly
                    {
                        event.prevent_default();
                        event.stop_propagation();
                        let snapshot = state.borrow().clone();
                        let txn = if key.key().to_lowercase() == "y" || key.shift_key() {
                            history.borrow_mut().redo(&snapshot)
                        } else {
                            history.borrow_mut().undo(&snapshot)
                        };
                        if let Some(txn) = txn {
                            dispatch(txn);
                        }
                        return;
                    }
                    if key.key() == "Escape" {
                        local.borrow_mut().dirty = false;
                        clear_draft_error(&root);
                        refresh(&root, &local, true, readonly);
                        if let Some(input) = target.dyn_ref::<HtmlElement>() {
                            input.blur().ok();
                        }
                        return;
                    }
                    if action == "raw" && (key.key() == "Enter" || key.key() == "Tab") && !readonly
                    {
                        event.prevent_default();
                        let Ok(committed) = commit_raw(&root, &local, &state.borrow(), &book)
                        else {
                            return;
                        };
                        txn = committed;
                        local.borrow_mut().dirty = false;
                        let horizontal = key.key() == "Tab";
                        navigate(
                            &local,
                            if horizontal {
                                if key.shift_key() { -1 } else { 1 }
                            } else {
                                0
                            },
                            if horizontal {
                                0
                            } else if key.shift_key() {
                                -1
                            } else {
                                1
                            },
                        );
                    } else if action == "cell" {
                        let delta = match key.key().as_str() {
                            "ArrowLeft" => Some((-1, 0)),
                            "ArrowRight" => Some((1, 0)),
                            "ArrowUp" => Some((0, -1)),
                            "ArrowDown" | "Enter" => Some((0, 1)),
                            "Tab" => Some((if key.shift_key() { -1 } else { 1 }, 0)),
                            _ => None,
                        };
                        if delta.is_some() && !readonly && local.borrow().dirty {
                            let Ok(committed) = commit_raw(&root, &local, &state.borrow(), &book)
                            else {
                                focus_raw(&root);
                                return;
                            };
                            local.borrow_mut().dirty = false;
                            if let Some(txn) = committed {
                                dispatch(txn);
                            }
                        }
                        if key.key() == "Tab" {
                            let l = local.borrow();
                            if let Some(sheet) = l.sheets.iter().find(|s| s.id == l.active)
                                && (key.shift_key() && l.selected == (0, 0)
                                    || !key.shift_key()
                                        && l.selected == (sheet.cols - 1, sheet.rows - 1))
                            {
                                return; // Native Tab leaves the grid at its boundaries.
                            }
                        }

                        if let Some((x, y)) = delta {
                            event.prevent_default();
                            navigate(&local, x, y);
                            refresh(&root, &local, true, readonly);
                            focus_cell(&root, &local);
                            return;
                        }
                        if !readonly
                            && (key.key() == "F2"
                                || key.key().chars().count() == 1
                                    && !key.ctrl_key()
                                    && !key.meta_key()
                                    && !key.alt_key())
                        {
                            event.prevent_default();
                            if let Some(input) = input(&root, "raw") {
                                if key.key() != "F2" {
                                    input.set_value(&key.key());
                                    local.borrow_mut().dirty = true;
                                }
                                input.focus().ok();
                            }
                        }
                        return;
                    } else {
                        return;
                    }
                } else if name == "dblclick" && action == "cell" && !readonly {
                    if let Some(input) = input(&root, "raw") {
                        input.focus().ok();
                        input.select();
                    }
                    return;
                } else if name == "click" {
                    if let Some(tab) = target.get_attribute("data-sheet-id") {
                        if !readonly {
                            let Ok(committed) = commit_raw(&root, &local, &state.borrow(), &book)
                            else {
                                return;
                            };
                            txn = committed;
                        }
                        let mut l = local.borrow_mut();
                        l.active = tab;
                        l.selected = (0, 0);
                        l.row_start = 0;
                        l.col_start = 0;
                        l.dirty = false;
                    } else if action == "cell" {
                        if !readonly {
                            let Ok(committed) = commit_raw(&root, &local, &state.borrow(), &book)
                            else {
                                focus_raw(&root);
                                return;
                            };
                            txn = committed;
                        }
                        let mut l = local.borrow_mut();
                        l.selected = (
                            target
                                .get_attribute("data-col")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                            target
                                .get_attribute("data-row")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0),
                        );
                        l.dirty = false;
                    } else if action == "add" && !readonly {
                        txn = commands::add_sheet(&state.borrow(), &book);
                    } else if action == "rename" && !readonly {
                        if let Some(value) = input(&root, "name") {
                            txn = commands::set_attribute(
                                &state.borrow(),
                                &book,
                                &local.borrow().active,
                                "sheetName".into(),
                                value.value(),
                            );
                        }
                    } else if action.starts_with("page-") {
                        let mut l = local.borrow_mut();
                        match action.as_str() {
                            "page-down" => l.row_start += 30,
                            "page-up" => l.row_start = l.row_start.saturating_sub(30),
                            "page-right" => l.col_start += 12,
                            "page-left" => l.col_start = l.col_start.saturating_sub(12),
                            _ => {}
                        }
                    } else {
                        return;
                    }
                } else if name == "change" && action == "format" && !readonly {
                    if let Some(value) = target.dyn_ref::<HtmlInputElement>() {
                        let l = local.borrow();
                        txn = commands::set_attribute(
                            &state.borrow(),
                            &book,
                            &l.active,
                            embedded::format_key(l.selected.1, l.selected.0),
                            value.value(),
                        );
                    }
                } else {
                    return;
                }
                if let Some(txn) = txn {
                    dispatch(txn);
                }
                refresh(&root, &local, true, readonly);
                if action == "cell" || action == "switch" || name == "keydown" {
                    focus_cell(&root, &local);
                }
            }) as Box<dyn Fn(web_sys::Event)>);
            island
                .root
                .add_event_listener_with_callback(name, listener.as_ref().unchecked_ref())
                .ok();
            island.listeners.push((name, listener));
        }
        island
    }
    pub fn update(&self, node: &Node) {
        let Node::Element { content, .. } = node else {
            return;
        };
        let sheets = content
            .children
            .iter()
            .map(|node| match node {
                Node::Element {
                    node_type: NodeType::SpreadsheetSheet,
                    attrs,
                    ..
                } => embedded::decode_sheet(attrs).ok(),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
            .filter(|sheets| embedded::validate_workbook(sheets).is_ok())
            .unwrap_or_default();
        self.root
            .set_attribute("data-atom-size", &node.node_size().to_string())
            .ok();
        {
            let mut l = self.local.borrow_mut();
            if l.sheets == sheets
                && self
                    .root
                    .query_selector(".embedded-sheet-toolbar")
                    .ok()
                    .flatten()
                    .is_some()
            {
                return;
            }
            l.sheets = sheets;
            if !l.sheets.iter().any(|s| s.id == l.active) {
                l.active = l.sheets.first().map(|s| s.id.clone()).unwrap_or_default();
                l.dirty = false;
            }
        }
        refresh(
            &self.root,
            &self.local,
            false,
            self.root.has_attribute("data-readonly"),
        );
    }
}
impl Drop for SpreadsheetIsland {
    fn drop(&mut self) {
        for (name, listener) in &self.listeners {
            self.root
                .remove_event_listener_with_callback(name, listener.as_ref().unchecked_ref())
                .ok();
        }
    }
}
fn input(root: &HtmlElement, action: &str) -> Option<HtmlInputElement> {
    root.query_selector(&format!("input[data-sheet-action='{action}']"))
        .ok()??
        .dyn_into()
        .ok()
}
fn focus_raw(root: &HtmlElement) {
    if let Some(raw) = input(root, "raw") {
        raw.focus().ok();
    }
}
fn clear_draft_error(root: &HtmlElement) {
    if let Some(input) = input(root, "raw") {
        input.remove_attribute("aria-invalid").ok();
    }
    if let Ok(Some(error)) = root.query_selector(".embedded-sheet-error") {
        error.remove();
    }
}
fn commit_raw(
    root: &HtmlElement,
    local: &RefCell<Local>,
    state: &EditorState,
    book: &str,
) -> Result<Option<Transaction>, ()> {
    let l = local.borrow();
    if !l.dirty {
        return Ok(None);
    }
    let Some(input) = input(root, "raw") else {
        return Err(());
    };
    let value = input.value();
    let txn = commands::set_attribute(
        state,
        book,
        &l.active,
        embedded::cell_key(l.selected.1, l.selected.0),
        value.clone(),
    );
    let unchanged = l
        .sheets
        .iter()
        .find(|sheet| sheet.id == l.active)
        .is_some_and(|sheet| {
            sheet
                .cells
                .get(&l.selected)
                .map(String::as_str)
                .unwrap_or("")
                == value
        });
    if txn.is_some() || unchanged {
        clear_draft_error(root);
        return Ok(txn);
    }
    input.set_attribute("aria-invalid", "true").ok();
    if root
        .query_selector(".embedded-sheet-error")
        .ok()
        .flatten()
        .is_none()
        && let Some(doc) = root.owner_document()
        && let Some(error) = element(&doc, "div", "embedded-sheet-error")
    {
        error.set_attribute("role", "alert").ok();
        error.set_text_content(Some(&label("embedded-sheet-draft-invalid")));
        root.append_child(&error).ok();
    }
    Err(())
}

fn navigate(local: &RefCell<Local>, x: isize, y: isize) {
    let mut l = local.borrow_mut();
    let Some(s) = l.sheets.iter().find(|s| s.id == l.active) else {
        return;
    };
    let (cols, rows) = (s.cols, s.rows);
    l.selected = if x > 0 && l.selected.0 + 1 == cols && l.selected.1 + 1 < rows {
        (0, l.selected.1 + 1)
    } else if x < 0 && l.selected.0 == 0 && l.selected.1 > 0 {
        (cols - 1, l.selected.1 - 1)
    } else {
        (
            l.selected.0.saturating_add_signed(x).min(cols - 1),
            l.selected.1.saturating_add_signed(y).min(rows - 1),
        )
    };
    l.col_start = l.selected.0 / 12 * 12;
    l.row_start = l.selected.1 / 30 * 30;
}
fn focus_cell(root: &HtmlElement, local: &RefCell<Local>) {
    let l = local.borrow();
    if let Ok(Some(el)) = root.query_selector(&format!(
        "[data-sheet-action=cell][data-col='{}'][data-row='{}']",
        l.selected.0, l.selected.1
    )) && let Some(el) = el.dyn_ref::<HtmlElement>()
    {
        el.focus().ok();
    }
}
fn element(doc: &Document, tag: &str, class: &str) -> Option<Element> {
    let e = doc.create_element(tag).ok()?;
    e.set_class_name(class);
    Some(e)
}
fn button(doc: &Document, action: &str, text: &str) -> Option<Element> {
    let e = element(doc, "button", "")?;
    e.set_attribute("type", "button").ok()?;
    e.set_attribute("data-sheet-action", action).ok()?;
    e.set_text_content(Some(text));
    Some(e)
}
fn column_name(mut n: usize) -> String {
    let mut out = String::new();
    loop {
        out.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out
}
/// Control reconciliation may blur a removed tab/page button synchronously.
/// Suppress persistence and reentrant Local borrows throughout that DOM update.
struct SheetRefreshGuard<'a> {
    root: &'a HtmlElement,
    local: &'a RefCell<Local>,
    focus_was_inside: bool,
}
impl Drop for SheetRefreshGuard<'_> {
    fn drop(&mut self) {
        // Internal control replacement must not leave a dirty editor with focus
        // on the body, since a later prose click would then emit no focusout.
        let restore_draft_focus = self.focus_was_inside && self.local.borrow().dirty;
        if restore_draft_focus
            && self.root.is_connected()
            && let Some(doc) = self.root.owner_document()
            && !doc
                .active_element()
                .is_some_and(|el| self.root.contains(Some(el.as_ref())))
            && let Some(input) = input(self.root, "raw")
        {
            input.focus().ok();
        }
        self.root.remove_attribute("data-sheet-refreshing").ok();
    }
}
fn refresh(root: &HtmlElement, local: &RefCell<Local>, force: bool, readonly: bool) {
    let Some(doc) = root.owner_document() else {
        return;
    };
    let focus_was_inside = doc
        .active_element()
        .is_some_and(|el| root.contains(Some(el.as_ref())));
    root.set_attribute("data-sheet-refreshing", "true").ok();
    // Declared before the Local borrow so Drop runs after that borrow releases.
    let _refresh_guard = SheetRefreshGuard {
        root,
        local,
        focus_was_inside,
    };
    let mut l = local.borrow_mut();
    let Some(sheet) = l.sheets.iter().find(|s| s.id == l.active).cloned() else {
        root.set_text_content(Some(&label("embedded-sheet-invalid")));
        return;
    };
    l.row_start = l.row_start.min((sheet.rows - 1) / 30 * 30);
    l.col_start = l.col_start.min((sheet.cols - 1) / 12 * 12);
    l.selected = (
        l.selected.0.min(sheet.cols - 1),
        l.selected.1.min(sheet.rows - 1),
    );
    let engine = embedded::engine_for_sheet(&sheet, &l.sheets);
    if root
        .query_selector(".embedded-sheet-toolbar")
        .ok()
        .flatten()
        .is_none()
    {
        root.set_text_content(None);
        let Some(toolbar) = element(&doc, "div", "embedded-sheet-toolbar") else {
            return;
        };
        for (action, key) in [
            ("name", "embedded-sheet-name"),
            ("raw", "embedded-sheet-formula"),
            ("format", "embedded-sheet-format"),
        ] {
            let Ok(control) = doc.create_element("input") else {
                return;
            };
            control.set_attribute("data-sheet-action", action).ok();
            control.set_attribute("aria-label", &label(key)).ok();
            control.set_attribute("placeholder", &label(key)).ok();
            control
                .set_attribute("maxlength", if action == "raw" { "4096" } else { "128" })
                .ok();
            if readonly {
                control.set_attribute("readonly", "").ok();
            }
            toolbar.append_child(&control).ok();
            if action == "name"
                && let Some(b) = button(&doc, "rename", &label("embedded-sheet-rename"))
            {
                if readonly {
                    b.set_attribute("disabled", "").ok();
                }
                toolbar.append_child(&b).ok();
            }
        }
        root.append_child(&toolbar).ok();
        for class in [
            "embedded-sheet-tabs",
            "embedded-sheet-grid",
            "embedded-sheet-pages",
        ] {
            if let Some(el) = element(&doc, "div", class) {
                root.append_child(&el).ok();
            }
        }
    }
    let raw = input(root, "raw");
    if !l.dirty
        && let Some(raw) = raw
    {
        raw.set_value(engine.get_raw(l.selected));
    }
    for (action, value) in [
        ("name", sheet.name.clone()),
        (
            "format",
            sheet.formats.get(&l.selected).cloned().unwrap_or_default(),
        ),
    ] {
        if let Some(control) = input(root, action)
            && doc.active_element().as_ref() != Some(control.as_ref())
        {
            control.set_value(&value);
        }
    }
    if let Ok(Some(tabs)) = root.query_selector(".embedded-sheet-tabs") {
        tabs.set_text_content(None);
        for s in &l.sheets {
            if let Some(tab) = button(&doc, "switch", &s.name) {
                tab.set_attribute("data-sheet-id", &s.id).ok();
                tab.set_attribute(
                    "aria-pressed",
                    if s.id == l.active { "true" } else { "false" },
                )
                .ok();
                tabs.append_child(&tab).ok();
            }
        }
        if let Some(add) = button(&doc, "add", &label("embedded-sheet-add")) {
            if readonly || l.sheets.len() >= embedded::MAX_SHEETS {
                add.set_attribute("disabled", "").ok();
            }
            tabs.append_child(&add).ok();
        }
    }
    let signature = format!(
        "{}:{}:{}:{}:{}",
        l.active, l.row_start, l.col_start, sheet.rows, sheet.cols
    );
    let Ok(Some(grid)) = root.query_selector(".embedded-sheet-grid") else {
        return;
    };
    if force || grid.get_attribute("data-window").as_deref() != Some(&signature) {
        grid.set_text_content(None);
        grid.set_attribute("data-window", &signature).ok();
        let Some(table) = element(&doc, "table", "") else {
            return;
        };
        table.set_attribute("role", "grid").ok();
        table
            .set_attribute("aria-rowcount", &sheet.rows.to_string())
            .ok();
        table
            .set_attribute("aria-colcount", &sheet.cols.to_string())
            .ok();
        let Some(header) = element(&doc, "tr", "") else {
            return;
        };
        if let Some(corner) = element(&doc, "th", "") {
            header.append_child(&corner).ok();
        }
        for col in l.col_start..sheet.cols.min(l.col_start + 12) {
            if let Some(th) = element(&doc, "th", "") {
                th.set_text_content(Some(&column_name(col)));
                header.append_child(&th).ok();
            }
        }
        table.append_child(&header).ok();
        for row in l.row_start..sheet.rows.min(l.row_start + 30) {
            let Some(tr) = element(&doc, "tr", "") else {
                return;
            };
            if let Some(th) = element(&doc, "th", "") {
                th.set_text_content(Some(&(row + 1).to_string()));
                tr.append_child(&th).ok();
            }
            for col in l.col_start..sheet.cols.min(l.col_start + 12) {
                let Some(td) = element(&doc, "td", "") else {
                    return;
                };
                if let Some(cell) = button(&doc, "cell", &engine.get_display((col, row))) {
                    cell.set_attribute("data-col", &col.to_string()).ok();
                    cell.set_attribute("data-row", &row.to_string()).ok();
                    cell.set_attribute("aria-label", &format!("{}{}", column_name(col), row + 1))
                        .ok();
                    td.append_child(&cell).ok();
                }
                tr.append_child(&td).ok();
            }
            table.append_child(&tr).ok();
        }
        grid.append_child(&table).ok();
    }
    if let Ok(cells) = grid.query_selector_all("[data-sheet-action=cell]") {
        for i in 0..cells.length() {
            if let Some(cell) = cells.item(i).and_then(|n| n.dyn_into::<Element>().ok()) {
                let col = cell
                    .get_attribute("data-col")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let row = cell
                    .get_attribute("data-row")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                cell.set_text_content(Some(&engine.get_display((col, row))));
                cell.set_attribute(
                    "aria-selected",
                    if l.selected == (col, row) {
                        "true"
                    } else {
                        "false"
                    },
                )
                .ok();
                cell.set_attribute(
                    "tabindex",
                    if l.selected == (col, row) { "0" } else { "-1" },
                )
                .ok();
            }
        }
    }
    if let Ok(Some(pages)) = root.query_selector(".embedded-sheet-pages") {
        pages.set_text_content(None);
        for (action, key, disabled) in [
            ("page-up", "embedded-sheet-previous-rows", l.row_start == 0),
            (
                "page-down",
                "embedded-sheet-next-rows",
                l.row_start + 30 >= sheet.rows,
            ),
            (
                "page-left",
                "embedded-sheet-previous-columns",
                l.col_start == 0,
            ),
            (
                "page-right",
                "embedded-sheet-next-columns",
                l.col_start + 12 >= sheet.cols,
            ),
        ] {
            if let Some(b) = button(&doc, action, &label(key)) {
                if disabled {
                    b.set_attribute("disabled", "").ok();
                }
                pages.append_child(&b).ok();
            }
        }
    }
}
