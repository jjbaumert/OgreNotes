// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `/auth/mfa-challenge?handle=...` — login step 2 (Phase 4 M-E3
//! piece E).
//!
//! Reached via:
//!   - the dev-login frontend dispatcher when the server responded
//!     202 (already-enrolled user),
//!   - the OAuth callback redirect when the same condition holds.
//!
//! The page reads the opaque handle from the URL query string,
//! shows a TOTP input + a "use recovery code" toggle, and posts to
//! `/auth/mfa/challenge` or `/auth/mfa/recovery` accordingly. On
//! success it stores the returned TokenResponse via
//! `client::set_auth_from_token` and navigates home.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_query_map};

use crate::api::{client, mfa};

#[component]
pub fn MfaChallengePage() -> impl IntoView {
    let query = use_query_map();
    let handle = query
        .read_untracked()
        .get("handle")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let navigate = use_navigate();

    let (code_input, set_code_input) = signal::<String>(String::new());
    let (use_recovery, set_use_recovery) = signal(false);
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal(false);

    if handle.is_empty() {
        // Missing handle = the link is malformed or expired. Push
        // back to login; the user can retry from there.
        let nav = navigate.clone();
        nav("/login", Default::default());
        return view! { <div>{crate::t!("mfa-challenge-missing-handle")}</div> }.into_any();
    }

    let on_submit = {
        let navigate = navigate.clone();
        let handle = handle.clone();
        move |_| {
            let code = code_input.get();
            let recovery = use_recovery.get();
            if code.trim().is_empty() {
                set_error.set(Some(if recovery {
                    crate::t!("mfa-enter-recovery")
                } else {
                    crate::t!("mfa-enter-totp")
                }));
                return;
            }
            set_busy.set(true);
            set_error.set(None);
            let navigate = navigate.clone();
            let handle = handle.clone();
            spawn_local(async move {
                let result = if recovery {
                    mfa::recovery(&handle, code.trim()).await
                } else {
                    mfa::challenge(&handle, code.trim()).await
                };
                match result {
                    Ok(token) => {
                        client::set_auth_from_token(&token);
                        navigate("/", Default::default());
                    }
                    Err(_e) => {
                        // Both paths show a clean user-facing string.
                        // The earlier `{e:?}` debug-format on the
                        // TOTP path leaked the ApiClientError variant
                        // name + raw server body — minor info-
                        // disclosure and ugly UX.
                        set_error.set(Some(if recovery {
                            crate::t!("mfa-challenge-error-invalid-recovery")
                        } else {
                            crate::t!("mfa-challenge-error-invalid-totp")
                        }));
                        set_busy.set(false);
                    }
                }
            });
        }
    };

    let on_toggle_recovery = move |_| {
        set_use_recovery.update(|v| *v = !*v);
        set_code_input.set(String::new());
        set_error.set(None);
    };

    view! {
        <main id="main-content" tabindex="-1" class="mfa-page">
            <div class="mfa-card">
                <h1 class="mfa-title">{crate::t!("mfa-challenge-title")}</h1>

                {move || if use_recovery.get() {
                    view! {
                        <p class="mfa-subtitle">
                            {crate::t!("mfa-challenge-subtitle-recovery")}
                        </p>
                    }.into_any()
                } else {
                    view! {
                        <p class="mfa-subtitle">
                            {crate::t!("mfa-challenge-subtitle-totp")}
                        </p>
                    }.into_any()
                }}

                {move || error.get().map(|e| view! {
                    <div class="mfa-error" role="alert">{e}</div>
                })}

                <div class="mfa-verify">
                    <input
                        id="mfa-challenge-input"
                        type="text"
                        inputmode=move || if use_recovery.get() { "text" } else { "numeric" }
                        autocomplete="off"
                        autocapitalize="off"
                        placeholder=move || {
                            if use_recovery.get() { "XXXXX-XXXXX" } else { "123456" }
                        }
                        maxlength=move || if use_recovery.get() { 11 } else { 6 }
                        prop:value=move || code_input.get()
                        on:input=move |ev| set_code_input.set(event_target_value(&ev))
                    />
                    <button
                        class="mfa-verify-btn"
                        disabled=move || busy.get()
                        on:click=on_submit.clone()
                    >
                        {move || if busy.get() { crate::t!("mfa-verifying") } else { crate::t!("mfa-challenge-verify") }}
                    </button>
                </div>

                <button
                    class="mfa-fallback-link"
                    on:click=on_toggle_recovery
                >
                    {move || if use_recovery.get() {
                        crate::t!("mfa-challenge-use-totp")
                    } else {
                        crate::t!("mfa-challenge-use-recovery")
                    }}
                </button>
            </div>
        </main>
    }
    .into_any()
}
