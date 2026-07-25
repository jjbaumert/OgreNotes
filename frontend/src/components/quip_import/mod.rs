// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// Quip import wizard — Phase 0. Step 1 (the only step this task
// builds): paste a Quip personal access token, POST it to
// `/imports/quip/connect`, and on success show the connected
// profile + a checklist of the caller's root Quip folders. The
// "Continue" button that would carry the checked scope into Phase 1
// (actually importing) is intentionally disabled here — that's the
// next task's wire-up, not this one's.
//
// Mirrors `template_picker_modal.rs` for the modal skeleton (backdrop
// + `<Show when=visible>` + per-open reset) and `share_dialog.rs` for
// the focus trap + checkbox-row list pattern.
//
// SECURITY: the token field is `type="password"` and its value is
// never passed to `console.*`/`web_sys::console::*` — only
// `ApiClientError`'s opaque `Display` (status + x-request-id, never a
// response body) reaches the error banner. The token signal is
// cleared both when the modal closes and immediately after a
// successful connect, since the token now lives server-side only
// (the backend's `ImportRepo`/`ImportRecord` deliberately has no
// token field — see crates/storage).

use std::collections::HashMap;

use leptos::prelude::*;

use wasm_bindgen::JsCast;

use crate::a11y;
use crate::api::imports::{self, ConnectResponse};

#[component]
pub fn QuipImportWizard(
    /// Visibility flag — the parent (the shell) owns it and flips it
    /// from the entry point / on close.
    visible: ReadSignal<bool>,
    /// Called when the wizard should close (backdrop click, Escape,
    /// the header's close button).
    on_close: Callback<()>,
) -> impl IntoView {
    let (token, set_token) = signal(String::new());
    let (connecting, set_connecting) = signal(false);
    let (error, set_error) = signal::<Option<String>>(None);
    let (response, set_response) = signal::<Option<ConnectResponse>>(None);
    // Keyed by root-folder id; default-checked once a connect response
    // arrives (see `do_connect`). Phase 1 reads this to build the scope
    // for the actual import — unused by this task's disabled Continue.
    let selected: RwSignal<HashMap<String, bool>> = RwSignal::new(HashMap::new());

    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible.into());

    // Per-open reset + close-time token wipe. Fires on every `visible`
    // transition (not just "became true") so the token never lingers
    // in the signal after the dialog is dismissed.
    Effect::new(move |_| {
        let is_open = visible.get();
        set_token.set(String::new());
        if !is_open {
            return;
        }
        set_connecting.set(false);
        set_error.set(None);
        set_response.set(None);
        selected.set(HashMap::new());
    });

    let do_connect = move || {
        if connecting.get_untracked() {
            return;
        }
        let tok = token.get_untracked();
        if tok.trim().is_empty() {
            return;
        }
        set_connecting.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match imports::connect(&tok).await {
                Ok(resp) => {
                    // The token now lives server-side (or the connect
                    // attempt failed and is worthless either way) —
                    // clear it from the client immediately.
                    set_token.set(String::new());
                    let sel: HashMap<String, bool> = resp
                        .root_folders
                        .iter()
                        .map(|f| (f.id.clone(), true))
                        .collect();
                    selected.set(sel);
                    set_response.set(Some(resp));
                    set_connecting.set(false);
                }
                Err(e) => {
                    set_token.set(String::new());
                    // `ApiClientError::Display` never carries a response
                    // body (see api/client.rs `http_error`) — safe to
                    // surface directly, and never logged.
                    set_error.set(Some(e.to_string()));
                    set_connecting.set(false);
                }
            }
        });
    };

    view! {
        <Show when=move || visible.get()>
            <div class="confirm-backdrop" on:click=move |_| a11y::defer_close(on_close)>
                <div
                    node_ref=dialog_ref
                    class="folder-picker-dialog template-picker-dialog quip-import-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="quip-import-title"
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
                        <h3 id="quip-import-title">{crate::t!("quip-import-title")}</h3>
                        <button
                            class="toolbar-btn"
                            aria-label=crate::t!("modal-close")
                            on:click=move |_| a11y::defer_close(on_close)
                        >"\u{00D7}"</button>
                    </div>
                    <div class="folder-picker-body template-picker-body quip-import-body">
                        {move || match response.get() {
                            None => view! {
                                // ─── Step 1: token entry ──────────────
                                <div class="quip-import-step-token">
                                    <label class="template-picker-field">
                                        <span class="template-picker-field-key">
                                            {crate::t!("quip-import-token-label")}
                                        </span>
                                        <input
                                            type="password"
                                            class="template-picker-field-input"
                                            data-autofocus="true"
                                            autocomplete="off"
                                            placeholder=crate::t!("quip-import-token-placeholder")
                                            prop:value=move || token.get()
                                            on:input=move |ev| {
                                                set_token.set(event_target_value(&ev));
                                                set_error.set(None);
                                            }
                                            on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                if ev.key() == "Enter" {
                                                    ev.prevent_default();
                                                    do_connect();
                                                }
                                            }
                                        />
                                    </label>
                                    {move || error.get().map(|e| view! {
                                        <div class="template-picker-error" role="alert">
                                            {crate::t!("quip-import-error", err = e)}
                                        </div>
                                    })}
                                    <div class="confirm-actions">
                                        <button
                                            class="btn btn-primary"
                                            disabled=move || connecting.get() || token.get().trim().is_empty()
                                            on:click=move |_| do_connect()
                                        >{move || if connecting.get() {
                                            crate::t!("quip-import-connecting")
                                        } else {
                                            crate::t!("quip-import-connect")
                                        }}</button>
                                    </div>
                                </div>
                            }.into_any(),
                            Some(resp) => {
                                let profile_name = resp.quip_profile.name.clone();
                                let folders = resp.root_folders;
                                view! {
                                    // ─── Step 2: profile + folder scope ───
                                    // `data-import-id` / `data-quip-user-id`
                                    // are Phase 1 hooks (the scope-continue
                                    // wire-up needs both to kick off the
                                    // import against this connect session)
                                    // and double as a test-automation
                                    // anchor for the Task 9 demo.
                                    <div
                                        class="quip-import-step-scope"
                                        data-import-id=resp.import_id
                                        data-quip-user-id=resp.quip_profile.id
                                    >
                                        <p class="quip-import-profile">
                                            {crate::t!("quip-import-profile", name = profile_name)}
                                        </p>
                                        <h4 class="template-picker-section-title">
                                            {crate::t!("quip-import-folder-scope-heading")}
                                        </h4>
                                        {if folders.is_empty() {
                                            view! {
                                                <div class="template-picker-empty">
                                                    {crate::t!("quip-import-no-folders")}
                                                </div>
                                            }.into_any()
                                        } else {
                                            folders.into_iter().map(|f| {
                                                let fid = f.id.clone();
                                                let fid_checked = f.id.clone();
                                                view! {
                                                    <label class="share-link-opt quip-import-folder-row">
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=move || {
                                                                selected.get().get(&fid_checked).copied().unwrap_or(false)
                                                            }
                                                            on:change=move |ev| {
                                                                let checked = event_target_checked(&ev);
                                                                selected.update(|m| {
                                                                    m.insert(fid.clone(), checked);
                                                                });
                                                            }
                                                        />
                                                        <span>{f.title}</span>
                                                    </label>
                                                }
                                            }).collect::<Vec<_>>().into_any()
                                        }}
                                        <div class="confirm-actions">
                                            <button
                                                class="btn btn-primary"
                                                disabled=true
                                                title=crate::t!("quip-import-continue-hint")
                                            >{crate::t!("quip-import-continue")}</button>
                                        </div>
                                    </div>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </div>
        </Show>
    }
}

/// `event_target_value` (checkbox flavor) isn't in `leptos::prelude` —
/// same local helper pattern as `calendar_modal.rs` /
/// `spreadsheet_view/sort_dialog.rs`.
fn event_target_checked(e: &web_sys::Event) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.checked())
        .unwrap_or_default()
}
