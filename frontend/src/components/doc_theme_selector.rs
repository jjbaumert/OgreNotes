// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Document typography theme selector (#59 T-12, branding.md §Typography).
//!
//! A `<select>` of the five document themes (Default / Editorial /
//! Handwritten / Technical / Classic) backed by `UiPrefs.doc_theme`. On
//! mount it reflects the stored id; on change it applies the theme to
//! `<html>` immediately via [`theme::change_doc_theme`] (which also caches
//! for the next pre-mount paint and persists a single-field PUT to
//! `/users/me/prefs`).
//!
//! Same apply/persist split as the color-theme and accessibility
//! controls: the DOM flips locally and instantly; the PUT is best-effort.
//! App-wide application on load lives in `main.rs`'s prefs bootstrap.

use leptos::prelude::*;
use serde::Deserialize;
use wasm_bindgen::JsCast;

use crate::api::client;
use crate::theme;

/// Slim `/users/me` decode — only the doc-theme field, per the
/// per-consumer-slim-decode pattern the other selectors use.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MeDocTheme {
    ui_prefs: Option<UiPrefsRead>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiPrefsRead {
    #[serde(default)]
    doc_theme: Option<String>,
}

#[component]
pub fn DocThemeSelector() -> impl IntoView {
    // Current selection id; starts at "default" and the on-mount fetch may
    // flip it to the stored value.
    let (selected, set_selected) = signal(String::from("default"));

    leptos::task::spawn_local(async move {
        if let Ok(me) = client::api_get::<MeDocTheme>("/users/me").await {
            if let Some(id) = me.ui_prefs.and_then(|p| p.doc_theme) {
                // Canonicalize: an unknown/stale stored value shows as
                // Default rather than a phantom selection.
                let canonical = theme::normalize_doc_theme(&id)
                    .map(str::to_string)
                    .unwrap_or_else(|| "default".to_string());
                set_selected.set(canonical);
            }
        }
    });

    let on_change = move |ev: web_sys::Event| {
        let val = ev
            .target()
            .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
            .map(|el| el.value())
            .unwrap_or_else(|| "default".to_string());
        set_selected.set(val.clone());
        leptos::task::spawn_local(async move {
            if let Err(e) = theme::change_doc_theme(Some(&val)).await {
                web_sys::console::warn_1(
                    &format!("doc theme persistence failed: {e:?}").into(),
                );
            }
        });
    };

    view! {
        <select
            class="doc-theme-select"
            aria-label=crate::t!("settings-doc-theme-aria")
            prop:value=move || selected.get()
            on:change=on_change
        >
            <option value="default">{crate::t!("settings-doc-theme-default")}</option>
            <option value="editorial">{crate::t!("settings-doc-theme-editorial")}</option>
            <option value="handwritten">{crate::t!("settings-doc-theme-handwritten")}</option>
            <option value="technical">{crate::t!("settings-doc-theme-technical")}</option>
            <option value="classic">{crate::t!("settings-doc-theme-classic")}</option>
        </select>
    }
}
