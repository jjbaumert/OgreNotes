// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P2 piece 3 — locale switcher in the sidebar footer.
//!
//! Lets a user pick between the shipped locales (en-US, ar in v1)
//! and persists the choice both client-side (localStorage, via
//! `i18n::set_locale`) and server-side (`PUT /users/me/prefs`).
//!
//! Reload-on-change UX: switching locale triggers a full
//! `window.location.reload()`. The alternative — reactive
//! re-render of every component that called `t!()` — needs a
//! signal-counter that components subscribe to and a macro
//! rewrite, which is its own feature. Reload is honest about the
//! trade-off and avoids the complexity gradient; a future piece
//! can land the reactive path when the UX cost actually bites.
//!
//! First-login-on-new-device path: on mount the component fetches
//! `/users/me`. If the stored `ui_prefs.locale` differs from the
//! currently-active locale (because localStorage is empty on the
//! new device, so bootstrap fell through to navigator.language),
//! apply the stored pref + reload. The next bootstrap picks up
//! the new localStorage value and the chain settles. Net: a user
//! sees one extra reload on first login per device, never again.

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::api::client;
use crate::i18n;

/// Slim /users/me decode — only the locale field, per the
/// per-consumer-slim-decode pattern home.rs uses.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserMeLocale {
    ui_prefs: Option<UiPrefsRead>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiPrefsRead {
    #[serde(default)]
    locale: Option<String>,
}

/// Locale catalog: the shipping locales. Adding a new locale here
/// + dropping a `frontend/locales/<bcp47>/main.ftl` file + wiring
/// the language subtag into `i18n::ftl_for` is the full extension
/// surface. Labels are rendered in the native language so users
/// can self-identify without already understanding the current UI
/// locale.
const LOCALE_CHOICES: &[(&str, &str)] = &[
    ("en-US", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("ar", "العربية"),
];

/// Compact locale switcher for the sidebar footer. Renders a
/// native `<select>` (zero CSS overhead vs a custom dropdown,
/// good keyboard a11y for free). Reflects the current active
/// locale; on change fires the save+reload flow.
#[component]
pub fn LocaleSelector() -> impl IntoView {
    // Initialize from the harness's resolved locale — same value
    // it used to build the active bundle, so the `<select>`
    // accurately reflects what the user is seeing.
    let initial = i18n::resolve_locale();
    let (current, set_current) = signal(initial);

    // First-login-on-new-device sync: if the server has a stored
    // pref that doesn't match the active locale, apply it +
    // reload. The reload-loop guard is the equality check —
    // after reload, localStorage carries the stored value, so
    // `resolve_locale` returns it and this branch becomes a no-op.
    leptos::task::spawn_local(async move {
        let Ok(me) = client::api_get::<UserMeLocale>("/users/me").await else {
            return;
        };
        let Some(stored) = me.ui_prefs.and_then(|p| p.locale) else {
            return;
        };
        if stored.is_empty() {
            return;
        }
        if !i18n::same_locale(&stored, &i18n::resolve_locale()) {
            i18n::set_locale(&stored);
            reload_page();
        }
    });

    let on_change = move |ev: web_sys::Event| {
        let Some(target) = ev.target() else { return };
        let Ok(select) = target.dyn_into::<web_sys::HtmlSelectElement>() else {
            return;
        };
        let picked = select.value();
        if picked.is_empty() || i18n::same_locale(&picked, &current.get_untracked()) {
            return;
        }
        set_current.set(picked.clone());
        leptos::task::spawn_local(async move {
            // Server pref persists across devices; localStorage
            // (written inside set_locale) persists across reloads
            // on this device. Both writes are best-effort —
            // continuing to reload even on PUT failure means the
            // user sees their picked locale immediately; the
            // stored pref just won't propagate to their other
            // devices until they pick again.
            let body = serde_json::json!({ "locale": picked });
            if let Err(e) = client::api_put("/users/me/prefs", &body).await {
                web_sys::console::warn_1(
                    &format!("locale persistence failed: {e:?}").into(),
                );
            }
            i18n::set_locale(&picked);
            reload_page();
        });
    };

    view! {
        <div class="locale-selector">
            <label for="locale-select" class="locale-selector-label">"\u{1F310}"</label>  // globe emoji
            <select
                id="locale-select"
                class="locale-selector-select"
                on:change=on_change
                aria-label=crate::t!("locale-aria-label")
                prop:value=move || current.get()
            >
                {LOCALE_CHOICES.iter().map(|(tag, label)| {
                    view! {
                        <option value=*tag>{*label}</option>
                    }
                }).collect_view()}
            </select>
        </div>
    }
}

fn reload_page() {
    if let Some(window) = web_sys::window() {
        let _ = window.location().reload();
    }
}
