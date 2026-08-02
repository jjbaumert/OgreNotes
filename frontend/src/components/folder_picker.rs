// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;

use crate::a11y;
use crate::api::client;
use crate::api::folders::{self, FolderResponse};
use crate::components::folder_tree::{self, FolderTreeRow};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserMeResponse {
    home_folder_id: String,
}

/// Modal that lets the user pick a folder they own/can-edit as a restore
/// target. Lazy-loads each folder's children on first expand rather than
/// fetching the whole tree up front.
#[component]
pub fn FolderPickerDialog(
    #[prop(into)] visible: Signal<bool>,
    on_close: Callback<()>,
    on_pick: Callback<String>,
    #[prop(into)] title: String,
    #[prop(into, optional)] confirm_label: Option<String>,
) -> impl IntoView {
    let confirm_label = confirm_label.unwrap_or_else(|| "Select".to_string());

    // M-P8 piece A: focus trap + save/restore previously-focused
    // element on open/close.
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible);

    // Folder id → loaded response. We keep all visited folders so re-expand
    // is instant and the tree is stable across renders.
    let folders: RwSignal<HashMap<String, FolderResponse>> = RwSignal::new(HashMap::new());
    let expanded: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let root_id: RwSignal<Option<String>> = RwSignal::new(None);
    let selected: RwSignal<Option<String>> = RwSignal::new(None);
    let error: RwSignal<Option<String>> = RwSignal::new(None);

    // Reset the selection each time the dialog opens.
    Effect::new(move |_| {
        if visible.get() {
            selected.set(None);
            error.set(None);
        }
    });

    // Load the Home root eagerly (once, on mount) rather than gating it on
    // `visible` — the visible-gated load could leave the tree stuck on
    // "Loading…" if the open-time effect didn't re-fire. By the time the
    // dialog opens the tree is already populated.
    Effect::new(move |_| {
        if root_id.get_untracked().is_some() {
            return;
        }
        leptos::task::spawn_local(async move {
            match client::api_get::<UserMeResponse>("/users/me").await {
                Ok(me) => {
                    let home = me.home_folder_id.clone();
                    match folders::get_folder(&home).await {
                        Ok(f) => {
                            root_id.set(Some(home.clone()));
                            expanded.update(|set| {
                                set.insert(home.clone());
                            });
                            folders.update(|map| {
                                map.insert(home, f);
                            });
                        }
                        Err(e) => error.set(Some(e.to_string())),
                    }
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    let load_folder = move |id: String| {
        if folders.with_untracked(|m| m.contains_key(&id)) {
            return;
        }
        leptos::task::spawn_local(async move {
            match folders::get_folder(&id).await {
                Ok(f) => {
                    folders.update(|map| {
                        map.insert(id.clone(), f);
                    });
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    let toggle_expand = move |id: String| {
        let is_expanded = expanded.with_untracked(|s| s.contains(&id));
        if is_expanded {
            expanded.update(|s| {
                s.remove(&id);
            });
        } else {
            expanded.update(|s| {
                s.insert(id.clone());
            });
            load_folder(id);
        }
    };

    let do_pick = move || {
        let Some(id) = selected.get_untracked() else {
            return;
        };
        on_pick.run(id);
    };

    // The flattening moved to `components::folder_tree` (#236 Unit 3) so the
    // Quip import wizard's destination step can render the same tree without
    // nesting this dialog inside its own modal. Rendering is unchanged: a flat
    // row list with indentation, rather than a component per node, so the
    // signal graph stays simple.

    view! {
        <Show when=move || visible.get()>
            <div class="confirm-backdrop" on:click=move |_| a11y::defer_close(on_close)>
                <div
                    node_ref=dialog_ref
                    class="folder-picker-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="folder-picker-title"
                    on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                    on:keydown=move |e: web_sys::KeyboardEvent| {
                        if e.key() == "Escape" {
                            a11y::defer_close(on_close);
                            return;
                        }
                        if let Some(node) = dialog_ref.get() {
                            a11y::handle_tab_trap(&e, node.as_ref());
                        }
                    }
                >
                    <div class="confirm-header">
                        <h3 id="folder-picker-title">{title.clone()}</h3>
                        <button
                            class="share-close"
                            aria-label=crate::t!("common-close")
                            on:click=move |_| a11y::defer_close(on_close)
                        >
                            "\u{2715}"
                        </button>
                    </div>
                    <div class="folder-picker-body">
                        {move || {
                            let mut rows: Vec<FolderTreeRow> = Vec::new();
                            if let Some(root) = root_id.get() {
                                folders.with(|map| {
                                    expanded.with(|set| {
                                        folder_tree::flatten_tree(&root, map, set, &mut rows, 0);
                                    });
                                });
                            }
                            if rows.is_empty() {
                                return view! {
                                    <p class="folder-picker-empty">{crate::t!("common-loading")}</p>
                                }.into_any();
                            }
                            view! {
                                <ul class="folder-picker-tree">
                                    {rows.into_iter().map(|row| {
                                        let row_id = row.id.clone();
                                        let row_id_for_pick = row_id.clone();
                                        let row_id_for_toggle = row_id.clone();
                                        let is_selected = selected.get() == Some(row_id.clone());
                                        let indent = format!("padding-inline-start: {}px", (row.depth as u16) * 16 + 8);
                                        let disabled = !row.is_selectable();
                                        // The unloaded placeholder's wording is the
                                        // caller's, not the tree's — see `folder_tree`.
                                        let row_title = if row.is_loaded {
                                            row.title.clone()
                                        } else {
                                            crate::t!("common-loading")
                                        };
                                        let chevron = if !row.has_children {
                                            "".to_string()
                                        } else if row.is_expanded {
                                            "\u{25BE}".to_string()
                                        } else {
                                            "\u{25B8}".to_string()
                                        };

                                        view! {
                                            <li
                                                class="folder-picker-row"
                                                class:selected=is_selected
                                                class:disabled=disabled
                                                style=indent
                                                on:click=move |_| {
                                                    if disabled { return; }
                                                    selected.set(Some(row_id_for_pick.clone()));
                                                }
                                            >
                                                <span
                                                    class="folder-picker-chevron"
                                                    on:click=move |e: web_sys::MouseEvent| {
                                                        e.stop_propagation();
                                                        if !row.has_children { return; }
                                                        toggle_expand(row_id_for_toggle.clone());
                                                    }
                                                >
                                                    {chevron}
                                                </span>
                                                <span class="folder-picker-icon">"\u{1F4C1}"</span>
                                                <span class="folder-picker-title">
                                                    {row_title}
                                                    {if row.is_trash {
                                                        crate::t!("folder-picker-not-available")
                                                    } else {
                                                        String::new()
                                                    }}
                                                </span>
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
                                </ul>
                            }.into_any()
                        }}
                        {move || error.get().map(|e| view! {
                            <p class="folder-picker-error">{e}</p>
                        })}
                    </div>
                    <div class="confirm-actions">
                        <button class="btn btn-secondary" on:click=move |_| a11y::defer_close(on_close)>
                            {crate::t!("common-cancel")}
                        </button>
                        <button
                            class="btn btn-primary"
                            disabled=move || selected.get().is_none()
                            // Deferred like the close paths: on_pick flips the parent's
                            // visible signal to false (Move + Restore both do), tearing
                            // down this <Show> on the same click — the "closure invoked
                            // recursively or after being dropped" panic. Same fix as
                            // DuplicateDialog's confirm.
                            on:click=move |_| a11y::defer(do_pick.clone())
                        >
                            {confirm_label.clone()}
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}
