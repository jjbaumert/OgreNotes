// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Accessibility settings (account-menu step 2).
//!
//! Two checkboxes — dyslexia-friendly font and reduce motion —
//! backed by `UiPrefs.dyslexic_font` / `reduce_motion`. On mount the
//! component reflects the stored state; on toggle it applies the
//! change to `<html>` immediately via [`theme::apply_a11y_prefs`] and
//! persists it with a single-field partial PUT to `/users/me/prefs`.
//!
//! The same persistence/apply split the theme selector uses: the DOM
//! update is local and instant; the PUT is best-effort. App-wide
//! application on page load lives in `main.rs`'s prefs bootstrap, not
//! here — this component only renders on `/settings`.

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::api::client;
use crate::theme;

/// Slim `/users/me` decode — only the two a11y fields, per the
/// per-consumer-slim-decode pattern the theme/locale selectors use.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeA11y {
    ui_prefs: Option<UiPrefsRead>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiPrefsRead {
    #[serde(default)]
    dyslexic_font: Option<bool>,
    #[serde(default)]
    reduce_motion: Option<bool>,
}

#[component]
pub fn AccessibilitySettings() -> impl IntoView {
    let (dyslexic, set_dyslexic) = signal(false);
    let (reduce_motion, set_reduce_motion) = signal(false);

    // Reflect stored state on mount so the checkboxes match what
    // `main.rs` already applied to the document at load time.
    leptos::task::spawn_local(async move {
        if let Ok(me) = client::api_get::<MeA11y>("/users/me").await {
            if let Some(p) = me.ui_prefs {
                set_dyslexic.set(p.dyslexic_font.unwrap_or(false));
                set_reduce_motion.set(p.reduce_motion.unwrap_or(false));
            }
        }
    });

    let on_dyslexic = move |ev: web_sys::Event| {
        let on = is_checked(&ev);
        set_dyslexic.set(on);
        theme::apply_a11y_prefs(Some(on), None);
        theme::cache_prefs(None, Some(on), None); // #152: paint pre-mount next load
        persist("dyslexicFont", on);
    };
    let on_reduce_motion = move |ev: web_sys::Event| {
        let on = is_checked(&ev);
        set_reduce_motion.set(on);
        theme::apply_a11y_prefs(None, Some(on));
        theme::cache_prefs(None, None, Some(on)); // #152: paint pre-mount next load
        persist("reduceMotion", on);
    };

    view! {
        <div class="settings-toggle-list">
            <label class="settings-toggle">
                <input
                    type="checkbox"
                    prop:checked=move || dyslexic.get()
                    on:change=on_dyslexic
                />
                <span class="settings-toggle-text">
                    <span class="settings-toggle-label">
                        {crate::t!("settings-a11y-dyslexic-label")}
                    </span>
                    <span class="settings-toggle-hint">
                        {crate::t!("settings-a11y-dyslexic-hint")}
                    </span>
                </span>
            </label>
            <label class="settings-toggle">
                <input
                    type="checkbox"
                    prop:checked=move || reduce_motion.get()
                    on:change=on_reduce_motion
                />
                <span class="settings-toggle-text">
                    <span class="settings-toggle-label">
                        {crate::t!("settings-a11y-reduce-motion-label")}
                    </span>
                    <span class="settings-toggle-hint">
                        {crate::t!("settings-a11y-reduce-motion-hint")}
                    </span>
                </span>
            </label>
        </div>
    }
}

/// Read the `checked` state off a change event's target input.
fn is_checked(ev: &web_sys::Event) -> bool {
    ev.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.checked())
        .unwrap_or(false)
}

/// Persist one boolean a11y pref via a single-field partial PUT. The
/// merge contract on `/users/me/prefs` leaves the other prefs
/// untouched. Best-effort: a failure leaves the local DOM change in
/// place and logs to the console (the next load re-reads the stored,
/// unchanged value).
fn persist(field: &'static str, on: bool) {
    leptos::task::spawn_local(async move {
        let body = serde_json::json!({ field: on });
        if let Err(e) = client::api_put("/users/me/prefs", &body).await {
            web_sys::console::warn_1(
                &format!("a11y pref persistence failed: {e:?}").into(),
            );
        }
    });
}
