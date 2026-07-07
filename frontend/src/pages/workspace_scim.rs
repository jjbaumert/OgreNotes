// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `/workspaces/:id/scim` — workspace admin "SCIM Tokens" UI
//! (Phase 4 M-E5 piece F).
//!
//! Two responsibilities:
//!   1. List existing tokens (active + revoked) so admins can see
//!      what's been issued and when each was last used.
//!   2. Mint new tokens — the plaintext is shown ONCE in a banner
//!      that auto-disappears on the next mutation; the admin
//!      copies it before leaving the page.
//!
//! Revoking sets `disabled_at` server-side; the row stays in DDB
//! so the historical audit references resolve. The UI shows it
//! greyed-out with an "active" → "revoked" badge.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map};
use wasm_bindgen::JsCast;

use crate::api::client;
use crate::api::scim_tokens::{
    self, CreateScimTokenRequest, CreatedScimToken, ScimTokenSummary,
};

#[component]
pub fn WorkspaceScimPage() -> impl IntoView {
    let params = use_params_map();
    let workspace_id = params
        .read_untracked()
        .get("id")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let navigate = use_navigate();

    if !client::is_authenticated() {
        let nav = navigate.clone();
        nav("/login", Default::default());
        return view! { <div>{crate::t!("common-redirecting-login")}</div> }.into_any();
    }

    let (tokens, set_tokens) = signal::<Vec<ScimTokenSummary>>(Vec::new());
    let (new_name, set_new_name) = signal::<String>(String::new());
    // Plaintext is shown ONCE after a successful create. Cleared on
    // next mutation. None when no fresh-mint banner is showing.
    let (fresh_mint, set_fresh_mint) = signal::<Option<CreatedScimToken>>(None);
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal(false);

    let scim_base_url = {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        format!("{origin}/api/v1/scim/v2/workspaces/{workspace_id}")
    };

    let trigger_load = {
        let workspace_id = workspace_id.clone();
        move || {
            let workspace_id = workspace_id.clone();
            set_busy.set(true);
            set_error.set(None);
            spawn_local(async move {
                match scim_tokens::list_tokens(&workspace_id).await {
                    Ok(list) => set_tokens.set(list),
                    Err(e) => set_error.set(Some(crate::t!("scim-error-load-failed", err = format!("{e:?}")))),
                }
                set_busy.set(false);
            });
        }
    };

    Effect::new({
        let trigger_load = trigger_load.clone();
        move |has_run: Option<bool>| {
            if has_run == Some(true) {
                return true;
            }
            trigger_load();
            true
        }
    });

    let on_create = {
        let workspace_id = workspace_id.clone();
        let trigger_load = trigger_load.clone();
        move |_| {
            let workspace_id = workspace_id.clone();
            let name = new_name.get().trim().to_string();
            if name.is_empty() {
                set_error.set(Some(crate::t!("scim-error-name-required")));
                return;
            }
            set_busy.set(true);
            set_error.set(None);
            set_fresh_mint.set(None);
            let req = CreateScimTokenRequest { name };
            let trigger_load = trigger_load.clone();
            spawn_local(async move {
                let result = scim_tokens::create_token(&workspace_id, &req).await;
                let saved_ok = match result {
                    Ok(minted) => {
                        set_fresh_mint.set(Some(minted));
                        set_new_name.set(String::new());
                        true
                    }
                    Err(e) => {
                        set_error.set(Some(crate::t!("scim-error-create-failed", err = format!("{e:?}"))));
                        false
                    }
                };
                set_busy.set(false);
                if saved_ok {
                    trigger_load();
                }
            });
        }
    };

    let on_revoke = {
        let workspace_id = workspace_id.clone();
        let trigger_load = trigger_load.clone();
        move |token_id: String| {
            let workspace_id = workspace_id.clone();
            set_busy.set(true);
            set_error.set(None);
            // Clear the fresh-mint banner — if it's still on screen
            // and the admin clicked revoke, they should not see
            // both at the same time.
            set_fresh_mint.set(None);
            let trigger_load = trigger_load.clone();
            spawn_local(async move {
                let saved_ok = match scim_tokens::revoke_token(&workspace_id, &token_id).await {
                    Ok(()) => true,
                    Err(e) => {
                        set_error.set(Some(crate::t!("scim-error-revoke-failed", err = format!("{e:?}"))));
                        false
                    }
                };
                set_busy.set(false);
                if saved_ok {
                    trigger_load();
                }
            });
        }
    };

    let on_copy_token = move |plaintext: String| {
        if let Some(window) = web_sys::window() {
            let clipboard = window.navigator().clipboard();
            let _ = clipboard.write_text(&plaintext);
        }
    };

    let on_select_url = move |ev: leptos::ev::MouseEvent| {
        if let Some(target) = ev.target() {
            if let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() {
                let _ = input.select();
            }
        }
    };

    view! {
        <main id="main-content" tabindex="-1" class="workspace-scim-page">
            <div class="workspace-scim-card">
                <h1>{crate::t!("scim-title")}</h1>
                <p class="workspace-scim-subtitle">
                    {crate::t!("scim-subtitle")}
                </p>

                {
                    let trigger_load = trigger_load.clone();
                    move || error.get().map(|e| {
                        let tl = trigger_load.clone();
                        view! {
                            <div class="workspace-scim-error">
                                {e}
                                " "
                                <button on:click=move |_| tl() disabled=move || busy.get()>
                                    {crate::t!("admin-retry")}
                                </button>
                            </div>
                        }
                    })
                }

                // ── SCIM base URL (admin pastes into the IdP) ──
                <section class="workspace-scim-section">
                    <h2>{crate::t!("scim-base-url-heading")}</h2>
                    <p class="workspace-scim-help">
                        {crate::t!("scim-base-url-help")}
                    </p>
                    <input
                        type="text"
                        class="workspace-scim-base-url"
                        readonly
                        prop:value=scim_base_url.clone()
                        on:click=on_select_url
                    />
                </section>

                // ── Fresh-mint banner ───────────────────────────
                {move || fresh_mint.get().map(|m| {
                    let plaintext = m.token.clone();
                    let on_copy = {
                        let plaintext = plaintext.clone();
                        let on_copy = on_copy_token.clone();
                        move |_| on_copy(plaintext.clone())
                    };
                    view! {
                        <div class="workspace-scim-fresh">
                            <h3>{crate::t!("scim-fresh-heading", name = m.name.clone())}</h3>
                            <p class="workspace-scim-fresh-warning">
                                {crate::t!("scim-fresh-warning")}
                            </p>
                            <div class="workspace-scim-fresh-row">
                                <input
                                    type="text"
                                    class="workspace-scim-fresh-input"
                                    readonly
                                    prop:value=m.token.clone()
                                    on:click=on_select_url
                                />
                                <button on:click=on_copy>{crate::t!("scim-fresh-copy")}</button>
                            </div>
                        </div>
                    }
                })}

                // ── Create form ─────────────────────────────────
                <section class="workspace-scim-section">
                    <h2>{crate::t!("scim-create-heading")}</h2>
                    <div class="workspace-scim-create-row">
                        <input
                            type="text"
                            placeholder=crate::t!("scim-create-placeholder")
                            prop:value=move || new_name.get()
                            on:input=move |ev| set_new_name.set(event_target_value(&ev))
                            disabled=move || busy.get()
                        />
                        <button
                            class="workspace-scim-create"
                            on:click=on_create
                            disabled=move || busy.get()
                        >
                            {crate::t!("scim-create-button")}
                        </button>
                    </div>
                </section>

                // ── Existing tokens ─────────────────────────────
                <section class="workspace-scim-section">
                    <h2>{crate::t!("scim-existing-heading")}</h2>
                    {move || {
                        let rows = tokens.get();
                        if rows.is_empty() {
                            view! {
                                <p class="workspace-scim-empty">
                                    {crate::t!("scim-empty")}
                                </p>
                            }.into_any()
                        } else {
                            view! {
                                <table class="workspace-scim-table">
                                    <thead>
                                        <tr>
                                            <th>{crate::t!("scim-th-name")}</th>
                                            <th>{crate::t!("scim-th-token-id")}</th>
                                            <th>{crate::t!("scim-th-created")}</th>
                                            <th>{crate::t!("scim-th-last-used")}</th>
                                            <th>{crate::t!("scim-th-status")}</th>
                                            <th></th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {rows.into_iter().map(|t| {
                                            let active = t.is_active;
                                            let tid = t.token_id.clone();
                                            let on_revoke = on_revoke.clone();
                                            view! {
                                                <tr class:workspace-scim-revoked=move || !active>
                                                    <td>{t.name}</td>
                                                    <td><code>{t.token_id}</code></td>
                                                    <td>{crate::i18n::format_date(t.created_at, crate::i18n::DateStyle::Long)}</td>
                                                    <td>{format_last_used(t.last_used_at)}</td>
                                                    <td>{
                                                        if active { crate::t!("scim-status-active") } else { crate::t!("scim-status-revoked") }
                                                    }</td>
                                                    <td>{
                                                        if active {
                                                            let tid = tid.clone();
                                                            view! {
                                                                <button
                                                                    class="workspace-scim-revoke"
                                                                    on:click=move |_| on_revoke(tid.clone())
                                                                    disabled=move || busy.get()
                                                                >{crate::t!("scim-revoke")}</button>
                                                            }.into_any()
                                                        } else {
                                                            view! { <span></span> }.into_any()
                                                        }
                                                    }</td>
                                                </tr>
                                            }
                                        }).collect_view()}
                                    </tbody>
                                </table>
                            }.into_any()
                        }
                    }}
                </section>
            </div>
        </main>
    }
    .into_any()
}

fn format_last_used(usec: i64) -> String {
    if usec == 0 {
        crate::t!("admin-status-never")
    } else {
        crate::i18n::format_date(usec, crate::i18n::DateStyle::Long)
    }
}
