use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::client;

#[component]
pub fn LoginPage() -> impl IntoView {
    let (error, set_error) = signal::<Option<String>>(None);
    let (loading, set_loading) = signal(false);
    let (dev_name, set_dev_name) = signal("Dev User".to_string());
    let (dev_email, set_dev_email) = signal("dev@ogrenotes.local".to_string());
    let navigate = use_navigate();

    // Dev login for local development (bypasses OAuth)
    let on_dev_login = move |_| {
        let name = dev_name.get_untracked();
        let email = dev_email.get_untracked();
        if name.trim().is_empty() || email.trim().is_empty() {
            set_error.set(Some("Name and email are required".to_string()));
            return;
        }
        set_loading.set(true);
        set_error.set(None);
        let navigate = navigate.clone();
        leptos::task::spawn_local(async move {
            match client::dev_login(&email, &name).await {
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

                <input
                    type="text"
                    class="login-input"
                    placeholder="Display name"
                    prop:value=move || dev_name.get()
                    on:input=move |e| set_dev_name.set(event_target_value(&e))
                    style="margin-bottom: 8px; width: 100%; padding: 8px; border: 1px solid var(--color-border); border-radius: 4px; font-size: 14px;"
                />
                <input
                    type="email"
                    class="login-input"
                    placeholder="Email"
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
                    {move || if loading.get() { "Signing in..." } else { "Dev Login (local)" }}
                </button>

                <button
                    class="login-btn"
                    on:click=on_oauth_login
                    style="background: #333;"
                >
                    "Sign in with GitHub"
                </button>
            </div>
        </div>
    }
}
