// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Status editor (account-menu step 5).
//!
//! Sets or clears the user's self-set status — an optional emoji, a
//! short text, and an optional auto-expiry. Lives in the Profile tab
//! (the account menu's "Profile & Status" entry points here). Persists
//! via `PUT /users/me/status`; the account menu reflects the new
//! status on its next mount.
//!
//! Expiry is sent as epoch microseconds (the storage convention):
//! `now_ms + minutes` converted to µs. "Don't clear" sends `null`.

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::client;

/// Slim `/users/me` decode — just the (already expiry-filtered) status.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeStatus {
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
pub fn StatusEditor() -> impl IntoView {
    let (emoji, set_emoji) = signal(String::new());
    let (text, set_text) = signal(String::new());
    // Auto-clear delay in minutes; 0 ⇒ never.
    let (expiry_mins, set_expiry_mins) = signal(0i64);
    let (saving, set_saving) = signal(false);

    // Load current status into the fields.
    leptos::task::spawn_local(async move {
        if let Ok(me) = client::api_get::<MeStatus>("/users/me").await {
            if let Some(s) = me.status {
                set_text.set(s.text);
                set_emoji.set(s.emoji.unwrap_or_default());
            }
        }
    });

    let on_set = move |_| {
        if saving.get_untracked() {
            return;
        }
        let text_val = text.get_untracked().trim().to_string();
        // Empty text would clear server-side; route that through the
        // explicit Clear button instead so "Set" never silently wipes.
        if text_val.is_empty() {
            return;
        }
        let emoji_val = emoji.get_untracked().trim().to_string();
        let mins = expiry_mins.get_untracked();
        set_saving.set(true);
        leptos::task::spawn_local(async move {
            // µs since epoch = (ms now + delay ms) * 1000.
            let expires_at: Option<i64> = if mins > 0 {
                Some(((js_sys::Date::now() + (mins as f64) * 60_000.0) * 1000.0) as i64)
            } else {
                None
            };
            let body = serde_json::json!({
                "text": text_val,
                "emoji": emoji_val,
                "expiresAt": expires_at,
            });
            if let Err(e) = client::api_put("/users/me/status", &body).await {
                web_sys::console::warn_1(&format!("status save failed: {e:?}").into());
            }
            set_saving.set(false);
        });
    };

    let on_clear = move |_| {
        if saving.get_untracked() {
            return;
        }
        set_saving.set(true);
        set_text.set(String::new());
        set_emoji.set(String::new());
        set_expiry_mins.set(0);
        leptos::task::spawn_local(async move {
            // Empty text is the clear signal on the server.
            let body = serde_json::json!({ "text": "" });
            if let Err(e) = client::api_put("/users/me/status", &body).await {
                web_sys::console::warn_1(&format!("status clear failed: {e:?}").into());
            }
            set_saving.set(false);
        });
    };

    view! {
        <div class="settings-field">
            <span class="settings-field-label">{crate::t!("settings-status-heading")}</span>
            <div class="status-editor-row">
                <input
                    class="settings-input status-emoji-input"
                    type="text"
                    maxlength="8"
                    aria-label=crate::t!("settings-status-emoji")
                    placeholder="🙂"
                    prop:value=move || emoji.get()
                    on:input=move |ev| set_emoji.set(event_target_value(&ev))
                />
                <input
                    class="settings-input status-text-input"
                    type="text"
                    placeholder=crate::t!("settings-status-text")
                    prop:value=move || text.get()
                    on:input=move |ev| set_text.set(event_target_value(&ev))
                />
            </div>
            <select
                class="settings-input"
                aria-label=crate::t!("settings-status-expiry")
                prop:value=move || expiry_mins.get().to_string()
                on:change=move |ev| {
                    set_expiry_mins.set(event_target_value(&ev).parse().unwrap_or(0));
                }
            >
                <option value="0">{crate::t!("settings-status-expiry-never")}</option>
                <option value="30">{crate::t!("settings-status-expiry-30m")}</option>
                <option value="60">{crate::t!("settings-status-expiry-1h")}</option>
                <option value="240">{crate::t!("settings-status-expiry-4h")}</option>
            </select>
            <div class="settings-form-actions">
                <button class="btn btn-primary" on:click=on_set prop:disabled=move || saving.get()>
                    {crate::t!("settings-status-set")}
                </button>
                <button
                    class="btn btn-secondary"
                    on:click=on_clear
                    prop:disabled=move || saving.get()
                >
                    {crate::t!("settings-status-clear")}
                </button>
            </div>
        </div>
    }
}
