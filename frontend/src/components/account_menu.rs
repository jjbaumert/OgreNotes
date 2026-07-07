// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Account menu (account-menu step 3).
//!
//! Avatar-anchored dropdown that replaces the standalone sign-out
//! row in the sidebar footer. It's the single personal-actions hub:
//! identity (name + email), a link to the Profile section, a link to
//! Settings, and Sign out.
//!
//! Open/close uses the same simple toggle + `<Show>` pattern as
//! `notification_bell.rs` (no outside-click backdrop) — the menu's
//! items all navigate away, and re-clicking the trigger closes it.
//!
//! The avatar prefers the stored `avatar_url` (OAuth users get a real
//! photo) and falls back to initials derived from the display name,
//! which are available synchronously from the in-memory auth state —
//! so the trigger renders immediately without waiting on a fetch.
//!
//! The "Keyboard shortcuts" item (added in step 6) links to
//! `/settings#help`, which renders the platform-aware shortcut keys
//! and the build version via `help_panel()` in `pages/settings.rs`.

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::client;

/// Slim `/users/me` decode — the avatar URL plus the (already
/// expiry-filtered) status. Name/email come from the synchronous auth
/// state; this fetch upgrades the initials placeholder to a real photo
/// and surfaces the status pill.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeAvatar {
    avatar_url: Option<String>,
    #[serde(default)]
    status: Option<StatusView>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatusView {
    text: String,
    #[serde(default)]
    emoji: Option<String>,
}

#[component]
pub fn AccountMenu() -> impl IntoView {
    let (open, set_open) = signal(false);
    let (avatar_url, set_avatar_url) = signal::<Option<String>>(None);
    let (status, set_status) = signal::<Option<StatusView>>(None);

    // Identity from the in-memory auth state — present on every
    // authenticated page, no fetch required.
    let auth = client::get_auth();
    let name = auth.as_ref().map(|a| a.name.clone()).unwrap_or_default();
    let email = auth.map(|a| a.email).unwrap_or_default();
    let initials = initials_of(&name);

    // Upgrade to the stored avatar image + surface the status.
    leptos::task::spawn_local(async move {
        if let Ok(me) = client::api_get::<MeAvatar>("/users/me").await {
            if let Some(url) = me.avatar_url.filter(|u| !u.is_empty()) {
                set_avatar_url.set(Some(url));
            }
            set_status.set(me.status);
        }
    });

    // #152: navigate to a settings anchor client-side (no full reload) via the
    // shell-installed nav bridge. The settings page reads the location hash
    // reactively, so a same-page tab change still lands; a full load is the
    // fallback if the bridge isn't installed.
    let go = move |anchor: &'static str| {
        set_open.set(false);
        crate::commands::nav_bridge::go(anchor);
    };

    let sign_out = move |_| {
        set_open.set(false);
        leptos::task::spawn_local(async move {
            client::logout().await;
            if let Some(window) = web_sys::window() {
                let _ = window.location().set_href("/login");
            }
        });
    };

    let trigger_name = name.clone();
    let initials_for_avatar = initials.clone();

    view! {
        <div class="account-menu">
            <button
                class="account-menu-trigger"
                aria-haspopup="menu"
                aria-label=crate::t!("account-menu-aria")
                aria-expanded=move || open.get().to_string()
                on:click=move |_| set_open.update(|o| *o = !*o)
            >
                <span class="account-avatar">
                    {move || match avatar_url.get() {
                        Some(url) => view! {
                            <img class="account-avatar-img" src=url alt="" />
                        }
                        .into_any(),
                        None => view! {
                            <span class="account-avatar-initials">
                                {initials_for_avatar.clone()}
                            </span>
                        }
                        .into_any(),
                    }}
                </span>
                <span class="account-menu-id">
                    <span class="account-menu-name">{trigger_name}</span>
                    {move || status.get().map(|s| view! {
                        <span class="account-menu-status">
                            {s.emoji.map(|e| view! {
                                <span class="account-menu-status-emoji">{e}</span>
                            })}
                            <span class="account-menu-status-text">{s.text}</span>
                        </span>
                    })}
                </span>
                <span class="account-menu-chevron" aria-hidden="true">"\u{25BE}"</span>
            </button>

            <Show when=move || open.get()>
                <div class="account-menu-dropdown" role="menu">
                    <div class="account-menu-identity">
                        <span class="account-menu-identity-name">{name.clone()}</span>
                        <span class="account-menu-identity-email">{email.clone()}</span>
                    </div>
                    <button
                        class="account-menu-item"
                        role="menuitem"
                        on:click=move |_| go("/settings#profile")
                    >
                        {crate::t!("account-menu-profile")}
                    </button>
                    <button
                        class="account-menu-item"
                        role="menuitem"
                        on:click=move |_| go("/settings#appearance")
                    >
                        {crate::t!("account-menu-settings")}
                    </button>
                    <button
                        class="account-menu-item"
                        role="menuitem"
                        on:click=move |_| go("/settings#help")
                    >
                        {crate::t!("account-menu-shortcuts")}
                    </button>
                    <button
                        class="account-menu-item account-menu-item-danger"
                        role="menuitem"
                        on:click=sign_out
                    >
                        {crate::t!("sidebar-sign-out")}
                    </button>
                </div>
            </Show>
        </div>
    }
}

/// Up to two uppercase initials from a display name; `"?"` when the
/// name is empty (e.g. auth state somehow missing). Used as the
/// avatar placeholder before the stored image (if any) loads.
fn initials_of(name: &str) -> String {
    let initials: String = name
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    if initials.is_empty() {
        "?".to_string()
    } else {
        initials
    }
}
