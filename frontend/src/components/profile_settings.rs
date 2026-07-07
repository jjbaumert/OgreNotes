// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Profile settings (account-menu step 4).
//!
//! Edit the caller's display name and avatar URL, view the
//! (read-only) email, and pick the UI language. Name + avatar persist
//! via `PUT /users/me`; the language control (`LocaleSelector`)
//! self-persists on change with its own reload, independent of the
//! Save button.
//!
//! On a successful save the in-memory auth state's name is updated so
//! the account menu and editor header reflect the new name on the
//! next render without a full reload. Avatar refreshes naturally —
//! the account menu re-fetches `/users/me` on its next mount.
//!
//! Validation is mirrored client-side (non-empty name, http(s)
//! avatar) for instant feedback; the server re-validates and caps
//! regardless — the client checks are UX, never the trust boundary.

use leptos::prelude::*;

use crate::api::client;
use crate::components::locale_selector::LocaleSelector;

/// Slim `/users/me` decode — the fields this form edits or shows.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeProfile {
    name: String,
    email: String,
    avatar_url: Option<String>,
}

/// Save outcome surfaced next to the button.
#[derive(Clone)]
enum SaveState {
    Saved,
    Error(String),
}

#[component]
pub fn ProfileSettings() -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (avatar, set_avatar) = signal(String::new());
    let (email, set_email) = signal(String::new());
    let (saving, set_saving) = signal(false);
    let (status, set_status) = signal::<Option<SaveState>>(None);

    // #29 BYOK: the user's personal Anthropic key lives only in this
    // browser's localStorage (never sent to the OgreNotes server except the
    // transient `x-anthropic-key` pass-through on /ask). The input is never
    // pre-filled with the raw key — only a masked fingerprint is shown.
    let (byok_input, set_byok_input) = signal(String::new());
    let (byok_fp, set_byok_fp) = signal::<Option<String>>(crate::api::ask::byok_fingerprint());
    let (byok_saved, set_byok_saved) = signal(false);

    let save_byok = move |_| {
        let key = byok_input.get_untracked();
        if key.trim().is_empty() {
            return;
        }
        crate::api::ask::set_byok_key(&key);
        set_byok_fp.set(crate::api::ask::byok_fingerprint());
        // Clear the field so the raw key doesn't linger in the DOM.
        set_byok_input.set(String::new());
        set_byok_saved.set(true);
    };
    let clear_byok = move |_| {
        crate::api::ask::clear_byok_key();
        set_byok_fp.set(None);
        set_byok_input.set(String::new());
        set_byok_saved.set(false);
    };

    // Load current profile into the form.
    leptos::task::spawn_local(async move {
        match client::api_get::<MeProfile>("/users/me").await {
            Ok(me) => {
                set_name.set(me.name);
                set_email.set(me.email);
                set_avatar.set(me.avatar_url.unwrap_or_default());
            }
            Err(e) => {
                // A blank form is actively misleading here (the user
                // can't tell "server returned blank" from "load
                // failed"), so surface it rather than silently drop.
                web_sys::console::warn_1(&format!("profile load failed: {e:?}").into());
                set_status.set(Some(SaveState::Error(crate::t!(
                    "settings-profile-load-error"
                ))));
            }
        }
    });

    let on_save = move |_| {
        // Guard against a double-click landing two events before the
        // reactive `prop:disabled` flush — otherwise two concurrent
        // PUTs race and the later resolve wins the auth-state write.
        if saving.get_untracked() {
            return;
        }
        let new_name = name.get_untracked().trim().to_string();
        let new_avatar = avatar.get_untracked().trim().to_string();

        // Client-side mirror of the server validation — fast feedback,
        // not the authority.
        if new_name.is_empty() {
            set_status.set(Some(SaveState::Error(crate::t!(
                "settings-profile-name-required"
            ))));
            return;
        }
        if !new_avatar.is_empty()
            && !(new_avatar.starts_with("https://") || new_avatar.starts_with("http://"))
        {
            set_status.set(Some(SaveState::Error(crate::t!(
                "settings-profile-avatar-invalid"
            ))));
            return;
        }

        set_saving.set(true);
        set_status.set(None);
        leptos::task::spawn_local(async move {
            // `avatarUrl: ""` is meaningful to the server (clear the
            // avatar), so it's always sent.
            let body = serde_json::json!({
                "name": new_name,
                "avatarUrl": new_avatar,
            });
            match client::api_put("/users/me", &body).await {
                Ok(()) => {
                    // Reflect the new name in the in-memory auth state
                    // so the account menu / editor header pick it up.
                    if let Some(mut auth) = client::get_auth() {
                        auth.name = new_name.clone();
                        client::set_auth(auth);
                    }
                    set_name.set(new_name);
                    set_avatar.set(new_avatar);
                    set_status.set(Some(SaveState::Saved));
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("profile save failed: {e:?}").into(),
                    );
                    set_status.set(Some(SaveState::Error(crate::t!(
                        "settings-profile-error"
                    ))));
                }
            }
            set_saving.set(false);
        });
    };

    view! {
        <div class="settings-form">
            <div class="settings-field">
                <label class="settings-field-label" for="profile-name">
                    {crate::t!("settings-profile-name")}
                </label>
                <input
                    id="profile-name"
                    class="settings-input"
                    type="text"
                    prop:value=move || name.get()
                    on:input=move |ev| set_name.set(event_target_value(&ev))
                />
            </div>

            <div class="settings-field">
                <label class="settings-field-label" for="profile-avatar">
                    {crate::t!("settings-profile-avatar")}
                </label>
                <input
                    id="profile-avatar"
                    class="settings-input"
                    type="url"
                    placeholder="https://…"
                    prop:value=move || avatar.get()
                    on:input=move |ev| set_avatar.set(event_target_value(&ev))
                />
            </div>

            <div class="settings-field">
                <span class="settings-field-label">{crate::t!("settings-profile-email")}</span>
                <div class="settings-readonly">{move || email.get()}</div>
                <span class="settings-toggle-hint">
                    {crate::t!("settings-profile-email-hint")}
                </span>
            </div>

            <div class="settings-field">
                <span class="settings-field-label">
                    {crate::t!("settings-appearance-language")}
                </span>
                <LocaleSelector />
            </div>

            <div class="settings-field">
                <label class="settings-field-label" for="byok-key">
                    {crate::t!("settings-byok-label")}
                </label>
                <input
                    id="byok-key"
                    class="settings-input"
                    type="password"
                    autocomplete="off"
                    placeholder="sk-ant-…"
                    prop:value=move || byok_input.get()
                    on:input=move |ev| {
                        set_byok_input.set(event_target_value(&ev));
                        set_byok_saved.set(false);
                    }
                />
                <span class="settings-toggle-hint">
                    {crate::t!("settings-byok-hint")}
                </span>
                <div class="settings-byok-status">
                    {move || match byok_fp.get() {
                        Some(fp) => {
                            let label = crate::t!("settings-byok-active");
                            view! {
                                <span class="settings-byok-active">
                                    {format!("{label} ({fp})")}
                                </span>
                            }
                            .into_any()
                        }
                        None => view! {
                            <span class="settings-toggle-hint">
                                {crate::t!("settings-byok-none")}
                            </span>
                        }
                        .into_any(),
                    }}
                    {move || byok_saved.get().then(|| view! {
                        <span class="settings-saved" role="status">
                            {crate::t!("settings-saved")}
                        </span>
                    })}
                </div>
                <div class="settings-form-actions">
                    <button
                        class="btn btn-primary"
                        on:click=save_byok
                        prop:disabled=move || byok_input.get().trim().is_empty()
                    >
                        {crate::t!("settings-byok-save")}
                    </button>
                    <button
                        class="btn btn-danger"
                        on:click=clear_byok
                        prop:disabled=move || byok_fp.get().is_none()
                    >
                        {crate::t!("settings-byok-clear")}
                    </button>
                </div>
            </div>

            <div class="settings-form-actions">
                <button
                    class="btn btn-primary"
                    on:click=on_save
                    prop:disabled=move || saving.get()
                >
                    {move || {
                        if saving.get() {
                            crate::t!("settings-saving")
                        } else {
                            crate::t!("settings-save")
                        }
                    }}
                </button>
                {move || status.get().map(|s| match s {
                    SaveState::Saved => view! {
                        <span class="settings-saved" role="status">
                            {crate::t!("settings-saved")}
                        </span>
                    }
                    .into_any(),
                    SaveState::Error(msg) => view! {
                        <span class="settings-error" role="alert">{msg}</span>
                    }
                    .into_any(),
                })}
            </div>
        </div>
    }
}
