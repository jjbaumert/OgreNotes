// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Sheet-tab bar fragment of the spreadsheet view.
//!
//! Renders one button per sheet plus an "+" button that creates a new
//! empty sheet. Tabs activate on single-click and rename on double-click
//! via a native browser prompt. Right-click opens a small context menu
//! with Rename / Delete actions; Delete dispatches through the
//! `delete_sheet` callback the parent owns. All state lives in the
//! caller; this function takes the relevant signals + the persist
//! closure by value (Leptos signals are `Copy`, the engine is a
//! `&'static Mutex`, and `persist` is a `Copy` closure built by
//! `SpreadsheetView`).

use std::sync::Mutex;

use leptos::prelude::*;

use crate::spreadsheet::eval::SpreadsheetEngine;

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
    // (x, y) for sheet `idx`; `None` hides it. Right-click on a tab
    // sets it; clicking a menu item or the backdrop clears it.
    let (tab_menu, set_tab_menu) = signal::<Option<(usize, f64, f64)>>(None);
    view! {
        <div class="ss-sheet-tabs">
            {move || {
                let names = sheet_names.get();
                let active = active_sheet.get();
                names.iter().enumerate().map(|(i, name)| {
                    let is_active = i == active;
                    let name_display = name.clone();
                    let name_for_rename = name.clone();
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
                                // Sheet tabs sit at the bottom of the
                                // viewport, so the menu would extend
                                // off-screen without clamping. The
                                // shared `clamp_menu_position` flips
                                // it upward when needed.
                                let (mx, my) = super::clamp_menu_position(
                                    e.client_x() as f64,
                                    e.client_y() as f64,
                                );
                                set_tab_menu.set(Some((i, mx, my)));
                            }
                            on:dblclick=move |_| {
                                if let Some(window) = web_sys::window() {
                                    if let Ok(Some(new_name)) = window.prompt_with_message_and_default(
                                        &crate::t!("ss-rename-sheet-prompt"), &name_for_rename,
                                    ) {
                                        if !new_name.trim().is_empty() {
                                            set_sheet_names.update(|names| {
                                                if i < names.len() { names[i] = new_name.trim().to_string(); }
                                            });
                                            persist();
                                        }
                                    }
                                }
                            }
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
            {move || {
                let Some((idx, x, y)) = tab_menu.get() else {
                    return view! { <span></span> }.into_any();
                };
                let names_len = sheet_names.get().len();
                // Refuse to delete when this is the only sheet — the
                // doc must keep at least one table.
                let can_delete = names_len > 1;
                let menu_style = format!("left:{}px;top:{}px;", x, y);
                view! {
                    <>
                        <div class="ss-ctx-backdrop"
                            on:mousedown=move |_| set_tab_menu.set(None)>
                        </div>
                        <div class="ss-ctx-menu" style=menu_style>
                            <button class="ss-ctx-item"
                                on:click=move |_| {
                                    set_tab_menu.set(None);
                                    if let Some(window) = web_sys::window() {
                                        let current = sheet_names.get_untracked()
                                            .get(idx).cloned().unwrap_or_default();
                                        if let Ok(Some(new_name)) = window.prompt_with_message_and_default(
                                            &crate::t!("ss-rename-sheet-prompt"), &current,
                                        ) {
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
                                }>{crate::t!("ss-ctx-rename")}</button>
                            <button class="ss-ctx-item"
                                prop:disabled=!can_delete
                                on:click=move |_| {
                                    set_tab_menu.set(None);
                                    if !can_delete { return; }
                                    delete_sheet(idx);
                                }>{crate::t!("ss-ctx-delete")}</button>
                        </div>
                    </>
                }.into_any()
            }}
        </div>
    }
}
