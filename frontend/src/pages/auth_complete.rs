use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::client::{self, AuthState};

/// Page that handles the OAuth callback redirect.
/// Parses tokens from the URL fragment and stores them, then redirects to home.
#[component]
pub fn AuthCompletePage() -> impl IntoView {
    let navigate = use_navigate();

    // Parse tokens from the URL hash fragment on mount
    leptos::task::spawn_local(async move {
        if let Some(window) = web_sys::window() {
            let hash = window.location().hash().unwrap_or_default();
            if let Some(params) = parse_fragment(&hash) {
                client::set_auth(params);
                navigate("/", Default::default());
                return;
            }
        }
        // If parsing failed, redirect to login
        navigate("/login", Default::default());
    });

    view! {
        <div style="display:flex;align-items:center;justify-content:center;height:100vh;">
            "Completing sign in..."
        </div>
    }
}

/// Parse auth tokens from a URL fragment like:
/// `#access_token=...&refresh_token=...&session_id=...&user_id=...&email=...&name=...`
fn parse_fragment(hash: &str) -> Option<AuthState> {
    let hash = hash.strip_prefix('#')?;
    let mut params = std::collections::HashMap::new();
    for pair in hash.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        let value = parts.next().unwrap_or("");
        let decoded = urlencoding::decode(value).ok()?;
        params.insert(key.to_string(), decoded.into_owned());
    }

    Some(AuthState {
        access_token: params.remove("access_token")?,
        refresh_token: params.remove("refresh_token")?,
        session_id: params.remove("session_id")?,
        user_id: params.remove("user_id")?,
        email: params.remove("email")?,
        name: params.remove("name").unwrap_or_default(),
        expires_at: client::now_ms() + 900_000.0, // 15 min TTL
    })
}
