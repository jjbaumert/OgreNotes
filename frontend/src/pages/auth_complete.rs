// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::client;

/// Page that handles the OAuth callback redirect.
///
/// The backend OAuth callback sets an `HttpOnly; Secure; SameSite=Strict`
/// refresh cookie before redirecting here. We hydrate the in-memory
/// access token via `/auth/refresh` (the cookie is auto-attached) and
/// then send the user to `/`. No tokens are read from the URL fragment
/// — that path was the localStorage-era flow and is being phased out.
#[component]
pub fn AuthCompletePage() -> impl IntoView {
    let navigate = use_navigate();

    leptos::task::spawn_local(async move {
        if client::try_hydrate_from_cookie().await {
            navigate("/", Default::default());
        } else {
            navigate("/login", Default::default());
        }
    });

    view! {
        <main
            id="main-content"
            tabindex="-1"
            style="display:flex;align-items:center;justify-content:center;height:100vh;"
        >
            {crate::t!("auth-complete-status")}
        </main>
    }
}
