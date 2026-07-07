// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Notification settings (account-menu step 6).
//!
//! Surfaces the user's email-notification preference — the existing,
//! worker-honored `NotifEmailPref` (All / Mentions only / Off). Loads
//! from `/users/me` and persists each change immediately via
//! `PUT /users/me/notification-prefs`. The notify worker already reads
//! this field, so a change takes effect for the next email with no
//! further wiring.
//!
//! v1 covers the email channel only (the field the backend honors).
//! Per-event toggles / in-app channels are a follow-up once the worker
//! grows finer-grained preferences.

use leptos::prelude::*;

use crate::api::client;

/// Slim `/users/me` decode — the email-notification tag
/// ("all" / "mentionsonly" / "disabled").
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeNotif {
    email_notifications: String,
}

/// (wire value, i18n label key) for the three choices. The wire value
/// is the lowercase `NotifEmailPref` serde tag.
const CHOICES: &[(&str, &str)] = &[
    ("all", "settings-notif-all"),
    ("mentionsonly", "settings-notif-mentions"),
    ("disabled", "settings-notif-off"),
];

#[component]
pub fn NotificationSettings() -> impl IntoView {
    // Default to the backend default ("mentionsonly") until the load
    // resolves, so the control isn't momentarily unselected.
    let (selected, set_selected) = signal("mentionsonly".to_string());

    leptos::task::spawn_local(async move {
        if let Ok(me) = client::api_get::<MeNotif>("/users/me").await {
            set_selected.set(me.email_notifications);
        }
    });

    view! {
        <div class="settings-field">
            <span class="settings-field-label">
                {crate::t!("settings-notif-email-heading")}
            </span>
            <div class="settings-radio-group" role="radiogroup">
                {CHOICES.iter().map(|(value, label_key)| {
                    let value = *value;
                    // Dynamic key ⇒ call the resolver directly (the
                    // `t!` macro only accepts string literals).
                    let label = crate::i18n::translate(label_key, None);
                    view! {
                        <label class="settings-radio">
                            <input
                                type="radio"
                                name="email-notifications"
                                prop:checked=move || selected.get() == value
                                on:change=move |_| {
                                    set_selected.set(value.to_string());
                                    persist(value);
                                }
                            />
                            <span>{label}</span>
                        </label>
                    }
                }).collect_view()}
            </div>
            <span class="settings-toggle-hint">{crate::t!("settings-notif-hint")}</span>
        </div>
    }
}

/// Persist the chosen email-notification preference. Best-effort: a
/// failure logs and leaves the local selection in place (the next load
/// re-reads the stored value).
fn persist(value: &'static str) {
    leptos::task::spawn_local(async move {
        let body = serde_json::json!({ "emailNotifications": value });
        if let Err(e) = client::api_put("/users/me/notification-prefs", &body).await {
            web_sys::console::warn_1(
                &format!("notification pref persistence failed: {e:?}").into(),
            );
        }
    });
}
