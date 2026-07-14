// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Sheet-tab bar fragment of the spreadsheet view.
//!
//! Renders one button per sheet plus an "+" button that creates a new
//! empty sheet. Tabs activate on single-click and rename on double-click
//! via a native browser prompt. Right-click opens a small context menu
//! (shared `components::menu` chrome — Escape, keyboard nav, viewport
//! clamping) with Rename / Delete actions; Delete dispatches through
//! the `delete_sheet` callback the parent owns. All state lives in the
//! caller; this function takes the relevant signals + the persist
//! closure by value (Leptos signals are `Copy`, the engine is a
//! `&'static Mutex`, and `persist` is a `Copy` closure built by
//! `SpreadsheetView`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;

use leptos::prelude::*;

use crate::components::menu::{ContextMenu, MenuEntry};
use crate::spreadsheet::eval::SpreadsheetEngine;
use crate::touch::{LONG_PRESS_MS, LongPressTracker, TOUCH_MOVE_THRESHOLD_PX, first_touch_xy};

#[allow(clippy::too_many_arguments)]
pub(super) fn render_sheet_tab_bar(
    sheet_names: ReadSignal<Vec<String>>,
    set_sheet_names: WriteSignal<Vec<String>>,
    active_sheet: ReadSignal<usize>,
    set_active_sheet: WriteSignal<usize>,
    grid_version: ReadSignal<u32>,
    set_grid_version: WriteSignal<u32>,
    engine: &'static Mutex<SpreadsheetEngine>,
    persist: impl Fn() + Copy + Send + Sync + 'static,
    delete_sheet: impl Fn(usize) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    // Local context-menu state. `Some((idx, x, y))` shows the menu at
    // (x, y) for sheet `idx`; `None` hides it. Right-click or a
    // long-press on a tab sets it; the shared menu chrome clears it on
    // item click, backdrop click, or Escape.
    let (tab_menu, set_tab_menu) = signal::<Option<(usize, f64, f64)>>(None);

    // Long-press on a tab opens the same menu — Delete has no other
    // touch-reachable path (dblclick covers Rename). One tracker is
    // shared across tabs; each tab's touchstart records which tab and
    // where, and the tracker's timer callback reads it back. Both are
    // `!Send` (Rc), so they live in LocalStorage stores and the view
    // closures capture only the `Copy + Send` handles.
    let pending_tab: StoredValue<Rc<RefCell<Option<(usize, f64, f64)>>>, LocalStorage> =
        StoredValue::new_local(Rc::new(RefCell::new(None)));
    let tab_tracker: StoredValue<Rc<LongPressTracker>, LocalStorage> = StoredValue::new_local(
        LongPressTracker::new(LONG_PRESS_MS, TOUCH_MOVE_THRESHOLD_PX, move || {
            let target = pending_tab.with_value(|p| *p.borrow());
            if let Some((i, x, y)) = target {
                set_tab_menu.set(Some((i, x, y)));
            }
        }),
    );

    let rename_sheet = move |idx: usize| {
        if let Some(window) = web_sys::window() {
            let current = sheet_names
                .get_untracked()
                .get(idx)
                .cloned()
                .unwrap_or_default();
            if let Ok(Some(new_name)) = window
                .prompt_with_message_and_default(&crate::t!("ss-rename-sheet-prompt"), &current)
            {
                if !new_name.trim().is_empty() {
                    set_sheet_names.update(|names| {
                        if idx < names.len() {
                            names[idx] = new_name.trim().to_string();
                        }
                    });
                    persist();
                }
            }
        }
    };

    let menu_entries = Callback::new(move |()| {
        let Some((idx, _, _)) = tab_menu.get() else {
            return Vec::new();
        };
        // Refuse to delete when this is the only sheet — the doc must
        // keep at least one table.
        let can_delete = sheet_names.get().len() > 1;
        vec![
            MenuEntry::action(crate::t!("ss-ctx-rename"), move || rename_sheet(idx)),
            MenuEntry::action(crate::t!("ss-ctx-delete"), move || delete_sheet(idx))
                .disabled_when(!can_delete)
                .danger(),
        ]
    });

    view! {
        <div class="ss-sheet-tabs">
            {move || {
                let names = sheet_names.get();
                let active = active_sheet.get();
                names.iter().enumerate().map(|(i, name)| {
                    let is_active = i == active;
                    let name_display = name.clone();
                    view! {
                        <button
                            class="ss-sheet-tab"
                            class:active=is_active
                            on:click=move |_| {
                                if active_sheet.get_untracked() != i {
                                    set_active_sheet.set(i);
                                    set_grid_version.set(grid_version.get_untracked().wrapping_add(1));
                                }
                            }
                            on:contextmenu=move |e: web_sys::MouseEvent| {
                                e.prevent_default();
                                // Raw click coordinates — the shared menu
                                // clamps itself to the viewport, which
                                // matters here: the tabs sit at the very
                                // bottom, so the menu always flips up.
                                set_tab_menu.set(Some((i, e.client_x() as f64, e.client_y() as f64)));
                            }
                            on:dblclick=move |_| rename_sheet(i)
                            on:touchstart=move |ev: web_sys::TouchEvent| {
                                if let Some((x, y)) = first_touch_xy(&ev) {
                                    pending_tab.with_value(|p| *p.borrow_mut() = Some((i, x, y)));
                                }
                                tab_tracker.with_value(|t| t.on_start(&ev));
                            }
                            on:touchmove=move |ev: web_sys::TouchEvent| {
                                tab_tracker.with_value(|t| t.on_move(&ev));
                            }
                            on:touchend=move |_| tab_tracker.with_value(|t| t.on_end())
                            on:touchcancel=move |_| tab_tracker.with_value(|t| t.on_end())
                        >{name_display}</button>
                    }
                }).collect::<Vec<_>>()
            }}
            <button class="ss-sheet-tab ss-sheet-add" on:click=move |_| {
                let new_name = format!("Sheet{}", sheet_names.get_untracked().len() + 1);
                set_sheet_names.update(|names| names.push(new_name.clone()));
                let new_idx = sheet_names.get_untracked().len() - 1;
                set_active_sheet.set(new_idx);
                engine.lock().unwrap().clear(); // clear engine so new sheet starts empty
                persist();
            }>"+"</button>

            // ─── Tab context menu (right-click → Rename / Delete) ──
            <ContextMenu
                visible=Signal::derive(move || tab_menu.get().is_some())
                x=Signal::derive(move || tab_menu.get().map(|(_, x, _)| x).unwrap_or_default())
                y=Signal::derive(move || tab_menu.get().map(|(_, _, y)| y).unwrap_or_default())
                entries=menu_entries
                on_close=Callback::new(move |()| set_tab_menu.set(None))
            />
        </div>
    }
}
