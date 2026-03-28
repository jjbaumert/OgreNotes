use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::client;

#[component]
pub fn LoginPage() -> impl IntoView {
    let (error, set_error) = signal::<Option<String>>(None);
    let (loading, set_loading) = signal(false);
    let navigate = use_navigate();

    // Dev login for local development (bypasses OAuth)
    let on_dev_login = move |_| {
        set_loading.set(true);
        set_error.set(None);
        let navigate = navigate.clone();
        leptos::task::spawn_local(async move {
            match client::dev_login("dev@ogrenotes.local", "Dev User").await {
                Ok(_auth) => {
                    navigate("/", Default::default());
                }
                Err(e) => {
                    set_error.set(Some(e.to_string()));
                    set_loading.set(false);
                }
            }
        });
    };

    // Real OAuth login (not yet implemented)
    let on_oauth_login = move |_| {
        if let Some(window) = web_sys::window() {
            let _ = window.location().set_href("/api/v1/auth/login");
        }
    };

    view! {
        <div class="login-page">
            <div class="login-card">
                <h1 class="login-title">"OgreNotes"</h1>
                <p class="login-subtitle">"Documents with teeth."</p>

                {move || error.get().map(|e| view! {
                    <div style="color: var(--color-error); margin-bottom: 16px; font-size: 13px;">
                        {e}
                    </div>
                })}

                <button
                    class="login-btn"
                    on:click=on_dev_login
                    disabled=loading
                    style="margin-bottom: 8px;"
                >
                    {move || if loading.get() { "Signing in..." } else { "Dev Login (local)" }}
                </button>

                <button
                    class="login-btn"
                    on:click=on_oauth_login
                    style="background: #333; opacity: 0.5;"
                    disabled=true
                >
                    "Sign in with GitHub (coming soon)"
                </button>
            </div>
        </div>
    }
}
