// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::a11y;
use crate::api::{sharing, users};

/// Share dialog component for managing folder/document access.
#[component]
pub fn ShareDialog(
    /// Whether the dialog is visible.
    visible: ReadSignal<bool>,
    /// Callback to close the dialog.
    on_close: Callback<()>,
    /// Folder ID to share (from the document's parent folder).
    folder_id: ReadSignal<Option<String>>,
    /// The document being viewed (empty = none). Drives the doc-scoped
    /// link-sharing section, distinct from the folder-member list above.
    doc_id: ReadSignal<String>,
) -> impl IntoView {
    let (email_input, set_email_input) = signal(String::new());
    let (access_level, set_access_level) = signal("EDIT".to_string());
    let (status_msg, set_status_msg) = signal(String::new());
    let (status_error, set_status_error) = signal(false);
    let (members, set_members) = signal::<Vec<MemberEntry>>(Vec::new());
    let (loading, set_loading) = signal(false);

    // Link-sharing (doc-scoped) state. `link_mode` is "none"/"view"/"edit".
    let (link_mode, set_link_mode) = signal(String::from("none"));
    let (link_can_manage, set_link_can_manage) = signal(false);
    let (view_opts, set_view_opts) = signal(sharing::ViewOptions::default());
    let (link_loaded, set_link_loaded) = signal(false);
    let (link_status, set_link_status) = signal(String::new());

    // M-P8 piece A: focus trap + save/restore previously-focused
    // element on open/close.
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible.into());

    // Load existing members when dialog opens.
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        let Some(fid) = folder_id.get() else {
            return;
        };
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match sharing::list_members(&fid).await {
                Ok(resp) => {
                    let entries: Vec<MemberEntry> = resp
                        .members
                        .into_iter()
                        .map(|m| MemberEntry {
                            user_id: m.user_id,
                            name: m.name,
                            email: m.email,
                            access_level: m.access_level,
                        })
                        .collect();
                    set_members.set(entries);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load members: {e}").into(),
                    );
                }
            }
            set_loading.set(false);
        });
    });

    let do_share = move || {
        let email = email_input.get_untracked();
        let level = access_level.get_untracked();
        let Some(fid) = folder_id.get_untracked() else {
            set_status_msg.set(crate::t!("share-error-no-folder"));
            set_status_error.set(true);
            return;
        };

        if email.trim().is_empty() {
            set_status_msg.set(crate::t!("share-error-enter-email"));
            set_status_error.set(true);
            return;
        }

        set_status_msg.set(crate::t!("share-status-searching"));
        set_status_error.set(false);

        leptos::task::spawn_local(async move {
            // Look up user by email.
            let search_result = match users::search_by_email(&email).await {
                Ok(resp) => resp,
                Err(e) => {
                    set_status_msg.set(crate::t!("share-error-search-failed", err = e.to_string()));
                    set_status_error.set(true);
                    return;
                }
            };

            let Some(user) = search_result.users.into_iter().next() else {
                set_status_msg.set(crate::t!("share-error-no-user", email = email.clone()));
                set_status_error.set(true);
                return;
            };

            // Add as folder member.
            match sharing::add_member(&fid, &user.user_id, &level).await {
                Ok(()) => {
                    set_status_msg.set(crate::t!("share-status-shared-with", name = user.name.clone()));
                    set_status_error.set(false);
                    set_email_input.set(String::new());
                    // Refresh member list.
                    if let Ok(resp) = sharing::list_members(&fid).await {
                        let entries: Vec<MemberEntry> = resp
                            .members
                            .into_iter()
                            .map(|m| MemberEntry {
                                user_id: m.user_id,
                                name: m.name,
                                email: m.email,
                                access_level: m.access_level,
                            })
                            .collect();
                        set_members.set(entries);
                    }
                }
                Err(e) => {
                    set_status_msg.set(crate::t!("share-error-failed", err = e.to_string()));
                    set_status_error.set(true);
                }
            }
        });
    };

    let do_share_click = do_share.clone();

    // Load link-sharing settings when the dialog opens for a document.
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        let did = doc_id.get();
        if did.is_empty() {
            set_link_loaded.set(false);
            return;
        }
        leptos::task::spawn_local(async move {
            match sharing::get_link_settings(&did).await {
                Ok(s) => {
                    set_link_mode.set(s.link_sharing_mode.unwrap_or_else(|| "none".to_string()));
                    set_view_opts.set(s.view_options);
                    set_link_can_manage.set(s.can_manage);
                    set_link_loaded.set(true);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load link settings: {e}").into(),
                    );
                    set_link_loaded.set(false);
                }
            }
        });
    });

    // Change the link mode ("none"/"view"/"edit"); leaves view-options as-is.
    let apply_link_mode = move |m: String| {
        let did = doc_id.get_untracked();
        if did.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            match sharing::set_link_mode(&did, &m).await {
                Ok(()) => {
                    set_link_mode.set(m);
                    set_link_status.set(crate::t!("share-link-saved"));
                }
                Err(e) => set_link_status.set(crate::t!("share-link-error", err = e.to_string())),
            }
        });
    };

    // Toggle one view-mode sub-option (optimistic; reverts on error so the
    // checkbox never diverges from the server — a stale local value would
    // also corrupt the base state of the next toggle's full-options PATCH).
    let toggle_view_opt = move |field: &'static str| {
        let did = doc_id.get_untracked();
        if did.is_empty() {
            return;
        }
        let prev_opts = view_opts.get_untracked();
        let mut opts = prev_opts.clone();
        match field {
            "allowComments" => opts.allow_comments = !opts.allow_comments,
            "showHistory" => opts.show_history = !opts.show_history,
            "showConversation" => opts.show_conversation = !opts.show_conversation,
            "allowRequestAccess" => opts.allow_request_access = !opts.allow_request_access,
            _ => return,
        }
        set_view_opts.set(opts.clone());
        leptos::task::spawn_local(async move {
            match sharing::set_link_view_options(&did, &opts).await {
                Ok(()) => set_link_status.set(crate::t!("share-link-saved")),
                Err(e) => {
                    set_view_opts.set(prev_opts); // revert the optimistic advance
                    set_link_status.set(crate::t!("share-link-error", err = e.to_string()));
                }
            }
        });
    };

    // Copy the current document URL (the page the dialog is open on).
    // Reflect-guard the clipboard API: `navigator.clipboard` is undefined
    // in non-secure contexts, where the typed binding would panic — and we
    // only claim "Link copied" once the write actually dispatches (matches
    // the spreadsheet_view clipboard helper).
    let copy_link = move || {
        let Some(win) = web_sys::window() else { return };
        // #101: copy the canonical `/d/:id` URL (stable opaque doc id), NOT
        // `location.href`. The page URL can carry a slug/title or a
        // `?comment=` deep-link query that shouldn't ride along in a shared
        // link, and the link must be independent of how the user reached
        // the doc. Falls back to the page URL only if no doc id is set.
        let did = doc_id.get_untracked();
        let href = if did.is_empty() {
            win.location().href().unwrap_or_default()
        } else {
            let origin = win.location().origin().unwrap_or_default();
            format!("{origin}/d/{did}")
        };
        let nav = win.navigator();
        if let Ok(clip) = js_sys::Reflect::get(&nav, &"clipboard".into()) {
            if let Ok(write) = js_sys::Reflect::get(&clip, &"writeText".into()) {
                if let Ok(write_fn) = write.dyn_into::<js_sys::Function>() {
                    let _ = write_fn.call1(&clip, &href.into());
                    set_link_status.set(crate::t!("share-link-copied"));
                }
            }
        }
        // Clipboard API unavailable: status stays empty; user copies manually.
    };

    view! {
        <Show when=move || visible.get()>
            <div class="share-backdrop" on:click=move |_| a11y::defer_close(on_close)>
                <div
                    node_ref=dialog_ref
                    class="share-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="share-dialog-title"
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
                    <div class="share-header">
                        <h3 id="share-dialog-title">{crate::t!("share-title")}</h3>
                        <button
                            class="share-close"
                            aria-label=crate::t!("common-close")
                            on:click=move |_| a11y::defer_close(on_close)
                        >
                            "\u{2715}"
                        </button>
                    </div>

                    <div class="share-body">
                        // ─── Link sharing (this document) ───
                        <Show when=move || link_loaded.get()>
                            <div class="share-link-section">
                                <h4>{crate::t!("share-link-heading")}</h4>

                                // Manager: editable mode segmented control.
                                <Show when=move || link_can_manage.get()>
                                    <div
                                        class="share-link-modes"
                                        role="group"
                                        aria-label=crate::t!("share-link-heading")
                                    >
                                        <button
                                            class="share-link-mode-btn"
                                            class:selected=move || link_mode.get() == "none"
                                            on:click=move |_| apply_link_mode("none".to_string())
                                        >{crate::t!("share-link-mode-off")}</button>
                                        <button
                                            class="share-link-mode-btn"
                                            class:selected=move || link_mode.get() == "view"
                                            on:click=move |_| apply_link_mode("view".to_string())
                                        >{crate::t!("share-link-mode-view")}</button>
                                        <button
                                            class="share-link-mode-btn"
                                            class:selected=move || link_mode.get() == "edit"
                                            on:click=move |_| apply_link_mode("edit".to_string())
                                        >{crate::t!("share-link-mode-edit")}</button>
                                    </div>
                                </Show>

                                // View-mode sub-options (manager, view mode only).
                                <Show when=move || link_can_manage.get() && link_mode.get() == "view">
                                    <div class="share-link-options">
                                        <label class="share-link-opt">
                                            <input
                                                type="checkbox"
                                                prop:checked=move || view_opts.get().allow_comments
                                                on:change=move |_| toggle_view_opt("allowComments")
                                            />
                                            <span>{crate::t!("share-link-opt-comments")}</span>
                                        </label>
                                        <label class="share-link-opt">
                                            <input
                                                type="checkbox"
                                                prop:checked=move || view_opts.get().show_history
                                                on:change=move |_| toggle_view_opt("showHistory")
                                            />
                                            <span>{crate::t!("share-link-opt-history")}</span>
                                        </label>
                                        <label class="share-link-opt">
                                            <input
                                                type="checkbox"
                                                prop:checked=move || view_opts.get().show_conversation
                                                on:change=move |_| toggle_view_opt("showConversation")
                                            />
                                            <span>{crate::t!("share-link-opt-conversation")}</span>
                                        </label>
                                        <label class="share-link-opt">
                                            <input
                                                type="checkbox"
                                                prop:checked=move || view_opts.get().allow_request_access
                                                on:change=move |_| toggle_view_opt("allowRequestAccess")
                                            />
                                            <span>{crate::t!("share-link-opt-request")}</span>
                                        </label>
                                    </div>
                                </Show>

                                // Viewer + off: state line (managers see the buttons instead).
                                <Show when=move || !link_can_manage.get() && link_mode.get() == "none">
                                    <p class="share-link-note">{crate::t!("share-link-off")}</p>
                                </Show>

                                // Link is on: explanatory note + copy (shown to everyone).
                                <Show when=move || link_mode.get() != "none">
                                    <p class="share-link-note">{crate::t!("share-link-note")}</p>
                                    <button
                                        class="share-link-copy"
                                        on:click=move |_| copy_link()
                                    >{crate::t!("share-link-copy")}</button>
                                </Show>

                                {move || {
                                    let s = link_status.get();
                                    if s.is_empty() {
                                        view! { <div></div> }.into_any()
                                    } else {
                                        view! {
                                            <div class="share-link-status" role="status" aria-live="polite">{s}</div>
                                        }.into_any()
                                    }
                                }}
                            </div>
                        </Show>

                        <div class="share-input-row">
                            <input
                                type="email"
                                class="share-email-input"
                                placeholder=crate::t!("share-placeholder-email")
                                prop:value=move || email_input.get()
                                on:input=move |e| {
                                    set_email_input.set(event_target_value(&e));
                                    set_status_msg.set(String::new());
                                }
                                on:keydown=move |e: web_sys::KeyboardEvent| {
                                    if e.key() == "Enter" {
                                        e.prevent_default();
                                        do_share();
                                    }
                                }
                            />
                            <select
                                class="share-level-select"
                                prop:value=move || access_level.get()
                                on:change=move |e| {
                                    set_access_level.set(event_target_value(&e));
                                }
                            >
                                <option value="EDIT">{crate::t!("share-role-edit")}</option>
                                <option value="COMMENT">{crate::t!("share-role-comment")}</option>
                                <option value="VIEW">{crate::t!("share-role-view")}</option>
                            </select>
                            <button
                                class="share-btn"
                                on:click=move |_| do_share_click()
                            >{crate::t!("share-button")}</button>
                        </div>

                        {move || {
                            let msg = status_msg.get();
                            if msg.is_empty() {
                                view! { <div></div> }.into_any()
                            } else {
                                let is_err = status_error.get();
                                let class = if is_err { "share-status share-error" } else { "share-status share-success" };
                                // M-P8 piece B: errors are "alert" so
                                // they pre-empt the SR queue; success
                                // confirmations are "status" + polite
                                // so they wait for a pause.
                                let role = if is_err { "alert" } else { "status" };
                                view! {
                                    <div class=class role=role aria-live="polite">{msg}</div>
                                }.into_any()
                            }
                        }}

                        // Member list
                        <Show when=move || !members.get().is_empty()>
                            <div class="share-members">
                                <h4>{crate::t!("share-members-heading")}</h4>
                                {move || {
                                    members.get().into_iter().map(|m| {
                                        let uid = m.user_id.clone();
                                        let level = m.access_level.clone();
                                        let is_owner = level == "OWN";
                                        view! {
                                            <div class="share-member-row">
                                                <span class="share-member-id">{format!("{} ({})", m.name, m.email)}</span>
                                                <span class="share-member-level">{format_access_level(&level)}</span>
                                                <Show when=move || !is_owner>
                                                    <button
                                                        class="share-member-remove"
                                                        on:click={
                                                            let uid = uid.clone();
                                                            move |_| {
                                                                let uid = uid.clone();
                                                                let fid = folder_id.get_untracked();
                                                                leptos::task::spawn_local(async move {
                                                                    if let Some(fid) = fid {
                                                                        let _ = sharing::remove_member(&fid, &uid).await;
                                                                        if let Ok(resp) = sharing::list_members(&fid).await {
                                                                            let entries: Vec<MemberEntry> = resp
                                                                                .members
                                                                                .into_iter()
                                                                                .map(|m| MemberEntry {
                                                                                    user_id: m.user_id,
                                                                                    name: m.name,
                                                                                    email: m.email,
                                                                                    access_level: m.access_level,
                                                                                })
                                                                                .collect();
                                                                            set_members.set(entries);
                                                                        }
                                                                    }
                                                                });
                                                            }
                                                        }
                                                    >"\u{2715}"</button>
                                                </Show>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()
                                }}
                            </div>
                        </Show>
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[derive(Clone)]
struct MemberEntry {
    user_id: String,
    name: String,
    email: String,
    access_level: String,
}

fn format_access_level(level: &str) -> String {
    match level {
        "OWN" => crate::t!("share-role-owner"),
        "EDIT" => crate::t!("share-role-edit"),
        "COMMENT" => crate::t!("share-role-comment"),
        "VIEW" => crate::t!("share-role-view"),
        other => other.to_string(),
    }
}
