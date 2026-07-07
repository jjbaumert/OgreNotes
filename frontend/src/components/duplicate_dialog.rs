// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #146 follow-up: the "Duplicate document" dialog. Prompts for the copy's
//! name (pre-filled from the source) and its destination folder, defaulting
//! to the source's own folder with Home as the obvious top-level option. When
//! the chosen folder is shared with other users, it warns — with the head
//! count — *before* the duplicate happens.

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;

use crate::a11y;
use crate::api::client;
use crate::api::folders::{self, ChildResponse, FolderResponse};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserMeResponse {
    user_id: String,
    home_folder_id: String,
}

#[component]
pub fn DuplicateDialog(
    #[prop(into)] visible: Signal<bool>,
    /// Pre-filled name (the source document's current title).
    #[prop(into)] initial_name: Signal<String>,
    /// The source document's folder — the default destination.
    #[prop(into)] source_folder_id: Signal<Option<String>>,
    on_close: Callback<()>,
    /// `(chosen_name, destination_folder_id)`.
    on_confirm: Callback<(String, String)>,
) -> impl IntoView {
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible);

    let name: RwSignal<String> = RwSignal::new(String::new());
    let folders: RwSignal<HashMap<String, FolderResponse>> = RwSignal::new(HashMap::new());
    let expanded: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let root_id: RwSignal<Option<String>> = RwSignal::new(None);
    let selected: RwSignal<Option<String>> = RwSignal::new(None);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    // The current user's canonical id (from /users/me) so the share count can
    // reliably exclude self — `get_auth()` was unreliable here.
    let me_id: RwSignal<String> = RwSignal::new(String::new());
    // Number of *other* users with access to the selected folder (None when
    // the folder is private to the current user).
    let share_others: RwSignal<Option<usize>> = RwSignal::new(None);

    // Load the Home root + current user id eagerly (once, on mount) rather
    // than gating on `visible` — the visible-gated load left the tree stuck
    // on "Loading…".
    Effect::new(move |_| {
        if root_id.get_untracked().is_some() {
            return;
        }
        leptos::task::spawn_local(async move {
            let me = match client::api_get::<UserMeResponse>("/users/me").await {
                Ok(me) => me,
                Err(e) => {
                    error.set(Some(e.to_string()));
                    return;
                }
            };
            me_id.set(me.user_id);
            let home = me.home_folder_id;
            match folders::get_folder(&home).await {
                Ok(f) => {
                    root_id.set(Some(home.clone()));
                    expanded.update(|s| {
                        s.insert(home.clone());
                    });
                    folders.update(|m| {
                        m.insert(home.clone(), f);
                    });
                    // Default to Home when nothing's picked yet.
                    if selected.get_untracked().is_none() {
                        selected.set(Some(home));
                    }
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    // On open: reset the name, and default the destination to the source's
    // folder (falling back to Home once it has loaded).
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        name.set(initial_name.get_untracked());
        error.set(None);
        let src = source_folder_id.get_untracked();
        selected.set(src.clone().or_else(|| root_id.get_untracked()));
        // Fetch the source folder's name for the destination label if needed.
        if let Some(src) = src {
            if !folders.with_untracked(|m| m.contains_key(&src)) {
                leptos::task::spawn_local(async move {
                    if let Ok(f) = folders::get_folder(&src).await {
                        folders.update(|m| {
                            m.insert(src, f);
                        });
                    }
                });
            }
        }
    });

    // When the destination changes, count how many OTHER users can see that
    // folder (folder members exclude the owner, so this is the shared-with
    // set minus the current user). Re-runs when the user id loads, and guards
    // against a stale count if the selection moves on mid-fetch.
    Effect::new(move |_| {
        let Some(folder) = selected.get() else {
            share_others.set(None);
            return;
        };
        let me = me_id.get();
        leptos::task::spawn_local(async move {
            let result = crate::api::sharing::list_members(&folder).await;
            if selected.get_untracked().as_deref() != Some(folder.as_str()) {
                return;
            }
            match result {
                Ok(resp) => {
                    let others = resp.members.iter().filter(|m| m.user_id != me).count();
                    share_others.set((others > 0).then_some(others));
                }
                Err(_) => share_others.set(None),
            }
        });
    });

    let load_folder = move |id: String| {
        if folders.with_untracked(|m| m.contains_key(&id)) {
            return;
        }
        leptos::task::spawn_local(async move {
            match folders::get_folder(&id).await {
                Ok(f) => folders.update(|m| {
                    m.insert(id, f);
                }),
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

    let dest_name = move || {
        selected
            .get()
            .and_then(|id| folders.with(|m| m.get(&id).map(|f| f.title.clone())))
            .unwrap_or_else(|| crate::t!("common-loading"))
    };

    let do_confirm = move || {
        let n = name.get_untracked().trim().to_string();
        let Some(folder) = selected.get_untracked() else {
            return;
        };
        if n.is_empty() {
            return;
        }
        on_confirm.run((n, folder));
    };

    view! {
        <Show when=move || visible.get()>
            <div class="confirm-backdrop" on:click=move |_| a11y::defer_close(on_close)>
                <div
                    node_ref=dialog_ref
                    class="folder-picker-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="duplicate-dialog-title"
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
                        <h3 id="duplicate-dialog-title">{crate::t!("duplicate-dialog-title")}</h3>
                        <button
                            class="share-close"
                            aria-label=crate::t!("common-close")
                            on:click=move |_| a11y::defer_close(on_close)
                        >"\u{2715}"</button>
                    </div>

                    <div class="duplicate-name-field">
                        <label class="settings-field-label" for="duplicate-name">
                            {crate::t!("duplicate-name-label")}
                        </label>
                        <input
                            id="duplicate-name"
                            class="settings-input"
                            type="text"
                            prop:value=move || name.get()
                            on:input=move |e| name.set(event_target_value(&e))
                        />
                    </div>

                    <div class="duplicate-dest-label">
                        {move || crate::t!("duplicate-destination-label")}
                        ": "
                        <strong>{dest_name}</strong>
                    </div>

                    <div class="folder-picker-body">
                        {move || {
                            let mut rows: Vec<FolderRow> = Vec::new();
                            if let Some(root) = root_id.get() {
                                folders.with(|map| {
                                    expanded.with(|set| {
                                        render_tree(&root, map, set, &mut rows, 0);
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
                                        let disabled = row.is_trash || !row.is_loaded;
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
                                                >{chevron}</span>
                                                <span class="folder-picker-icon">"\u{1F4C1}"</span>
                                                <span class="folder-picker-title">{row.title}</span>
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

                    // Share warning — shown BEFORE the duplicate so the user
                    // knows the copy will land in a folder others can see.
                    {move || share_others.get().map(|n| view! {
                        <div class="duplicate-share-warning" role="alert">
                            "\u{26A0} "
                            {crate::t!("duplicate-share-warning", count = n as i64)}
                        </div>
                    })}

                    <div class="confirm-actions">
                        <button class="btn btn-secondary" on:click=move |_| a11y::defer_close(on_close)>
                            {crate::t!("common-cancel")}
                        </button>
                        <button
                            class="btn btn-primary"
                            disabled=move || selected.get().is_none() || name.get().trim().is_empty()
                            // Deferred like the close paths above: on_confirm flips the
                            // parent's `visible` to false, tearing down this <Show> on the
                            // same click. Running it synchronously drops the dialog's
                            // closures mid-event — the "closure invoked recursively or
                            // after being dropped" panic (seen in the doc-actions doctor
                            // run). One microtask lets the click settle first.
                            on:click=move |_| a11y::defer(do_confirm.clone())
                        >
                            {crate::t!("duplicate-confirm")}
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}

/// Flatten the loaded folder subtree into indented rows (folders only).
fn render_tree(
    root_id: &str,
    folders: &HashMap<String, FolderResponse>,
    expanded: &HashSet<String>,
    out: &mut Vec<FolderRow>,
    depth: u8,
) {
    let Some(folder) = folders.get(root_id) else {
        out.push(FolderRow {
            id: root_id.to_string(),
            title: crate::t!("common-loading"),
            depth,
            has_children: false,
            is_expanded: false,
            is_loaded: false,
            is_trash: false,
        });
        return;
    };
    let child_folders: Vec<&ChildResponse> = folder
        .children
        .iter()
        .filter(|c| c.child_type == "folder")
        .collect();
    out.push(FolderRow {
        id: folder.id.clone(),
        title: folder.title.clone(),
        depth,
        has_children: !child_folders.is_empty(),
        is_expanded: expanded.contains(&folder.id),
        is_loaded: true,
        is_trash: folder.is_trash,
    });
    if !expanded.contains(&folder.id) {
        return;
    }
    for child in child_folders {
        render_tree(&child.child_id, folders, expanded, out, depth + 1);
    }
}

struct FolderRow {
    id: String,
    title: String,
    depth: u8,
    has_children: bool,
    is_expanded: bool,
    is_loaded: bool,
    is_trash: bool,
}
