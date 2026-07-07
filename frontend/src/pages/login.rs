// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::client::{self, LoginOutcome};

/// Map a `LoginOutcome` to the right post-login route. Centralized
/// so the three dev-login buttons (custom, Alice, Bob) don't drift.
fn route_after_login(outcome: LoginOutcome) -> String {
    match outcome {
        LoginOutcome::MfaRequired { handle } => {
            format!("/auth/mfa-challenge?handle={}", urlencoding::encode(&handle))
        }
        LoginOutcome::Authenticated {
            mfa_enrollment_required,
            ..
        } => {
            if mfa_enrollment_required {
                "/auth/mfa-enroll".to_string()
            } else {
                "/".to_string()
            }
        }
    }
}

#[component]
pub fn LoginPage() -> impl IntoView {
    let (error, set_error) = signal::<Option<String>>(None);
    let (loading, set_loading) = signal(false);
    let (dev_name, set_dev_name) = signal("Dev User".to_string());
    let (dev_email, set_dev_email) = signal("dev@ogrenotes.local".to_string());
    let navigate = use_navigate();

    // Show dev login only on localhost (dev mode with Trunk proxy)
    let is_dev = web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .map(|h| h == "localhost" || h == "127.0.0.1")
        .unwrap_or(false);

    // Dev login for local development (bypasses OAuth)
    let on_dev_login = {
        let navigate = navigate.clone();
        move |_| {
            let name = dev_name.get_untracked();
            let email = dev_email.get_untracked();
            if name.trim().is_empty() || email.trim().is_empty() {
                set_error.set(Some(crate::t!("login-error-name-email-required")));
                return;
            }
            set_loading.set(true);
            set_error.set(None);
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                match client::dev_login(&email, &name).await {
                    Ok(outcome) => {
                        let path = route_after_login(outcome);
                        navigate(&path, Default::default());
                    }
                    Err(e) => {
                        set_error.set(Some(e.to_string()));
                        set_loading.set(false);
                    }
                }
            });
        }
    };

    // Quick dev login helpers
    let on_alice = {
        let navigate = navigate.clone();
        move |_| {
            set_loading.set(true);
            set_error.set(None);
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                match client::dev_login("alice@ogrenotes.local", "Alice").await {
                    Ok(outcome) => {
                        let path = route_after_login(outcome);
                        navigate(&path, Default::default());
                    }
                    Err(e) => { set_error.set(Some(e.to_string())); set_loading.set(false); }
                }
            });
        }
    };

    let on_bob = {
        let navigate = navigate.clone();
        move |_| {
            set_loading.set(true);
            set_error.set(None);
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                match client::dev_login("bob@ogrenotes.local", "Bob").await {
                    Ok(outcome) => {
                        let path = route_after_login(outcome);
                        navigate(&path, Default::default());
                    }
                    Err(e) => { set_error.set(Some(e.to_string())); set_loading.set(false); }
                }
            });
        }
    };

    // OAuth login — navigates directly to the API server.
    // In dev mode (Trunk proxy on :8080), we must bypass the proxy because
    // it follows the GitHub OAuth redirect server-side and fails with
    // "too many redirects". In production the frontend is served from the
    // same origin as the API, so a relative path works.
    let make_oauth_handler = |provider: &'static str| {
        move |_| {
            if let Some(window) = web_sys::window() {
                let origin = window.location().origin().unwrap_or_default();
                let api_url = if origin.contains(":8080") {
                    format!("http://localhost:3000/api/v1/auth/login/{provider}")
                } else {
                    format!("{origin}/api/v1/auth/login/{provider}")
                };
                let _ = window.location().set_href(&api_url);
            }
        }
    };
    let on_github_login = make_oauth_handler("github");
    let on_google_login = make_oauth_handler("google");

    view! {
        <main id="main-content" tabindex="-1" class="login-page">
            <div class="login-card">
                <h1 class="login-title">"OgreNotes"</h1>
                <p class="login-subtitle">{crate::t!("login-tagline")}</p>

                {move || error.get().map(|e| view! {
                    <div
                        role="alert"
                        style="color: var(--color-error); margin-bottom: 16px; font-size: 13px;"
                    >
                        {e}
                    </div>
                })}

                {if is_dev { Some(view! {
                    <div>
                        <input
                            type="text"
                            class="login-input"
                            placeholder=crate::t!("login-placeholder-name")
                            prop:value=move || dev_name.get()
                            on:input=move |e| set_dev_name.set(event_target_value(&e))
                            style="margin-bottom: 8px; width: 100%; padding: 8px; border: 1px solid var(--color-border); border-radius: 4px; font-size: 14px;"
                        />
                        <input
                            type="email"
                            class="login-input"
                            placeholder=crate::t!("login-placeholder-email")
                            prop:value=move || dev_email.get()
                            on:input=move |e| set_dev_email.set(event_target_value(&e))
                            style="margin-bottom: 12px; width: 100%; padding: 8px; border: 1px solid var(--color-border); border-radius: 4px; font-size: 14px;"
                        />

                        <button
                            class="login-btn"
                            on:click=on_dev_login
                            disabled=loading
                            style="margin-bottom: 8px;"
                        >
                            {move || if loading.get() { crate::t!("login-signing-in") } else { crate::t!("login-dev-button") }}
                        </button>

                        <div style="display: flex; gap: 8px; margin-bottom: 8px;">
                            <button
                                class="login-btn"
                                disabled=loading
                                style="flex: 1; background: #2E7D32;"
                                on:click=on_alice
                            >"\u{1F469} Alice"</button>
                            <button
                                class="login-btn"
                                disabled=loading
                                style="flex: 1; background: #1565C0;"
                                on:click=on_bob
                            >"\u{1F468} Bob"</button>
                        </div>
                    </div>
                }) } else { None }}

                <button
                    class="login-btn login-btn-github"
                    on:click=on_github_login
                >
                    {crate::t!("login-github")}
                </button>

                <button
                    class="login-btn login-btn-google"
                    on:click=on_google_login
                    style="margin-top: 8px;"
                >
                    {crate::t!("login-google")}
                </button>
            </div>
        </main>
    }
}
