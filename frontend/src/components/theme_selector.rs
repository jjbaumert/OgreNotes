// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P1 piece D — three-state theme selector.
//!
//! Renders a row of three buttons (System / Light / Dark) and
//! reflects the current selection. Clicking a button does two
//! things atomically from the user's perspective:
//!
//!   1. Calls `theme::apply_explicit_theme` so the page flips
//!      immediately (no waiting on the PUT round-trip).
//!   2. PUTs `/users/me/prefs` to persist the choice across
//!      sessions / devices.
//!
//! If the PUT fails, the local apply still happened — the user
//! sees the change and the next page load will pick up the stored
//! (stale) pref. That mirrors the contract documented on
//! `theme::change_theme`: don't revert the user's UI choice mid-
//! click because of a transient server hiccup.
//!
//! Initial state on mount: fetch /users/me, read `uiPrefs.theme`,
//! set the selected button. If the user hasn't customized
//! anything (uiPrefs absent or theme absent), default to "System"
//! and let the OS pref drive — matching the bootstrap path in
//! `main.rs`.

use leptos::prelude::*;
use serde::Deserialize;

use crate::api::client;
use crate::theme::{self, ExplicitTheme};

/// Slim decode of /users/me — only the field we need. Follows the
/// same per-consumer pattern as `home.rs` and `folder_picker.rs`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserMePrefs {
    ui_prefs: Option<UiPrefsRead>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiPrefsRead {
    #[serde(default)]
    theme: Option<String>,
}

/// Compact theme selector for the sidebar footer. The three
/// buttons render side by side; the currently-selected one gets a
/// pressed visual state via the `--selected` class. Width is
/// constrained by the sidebar's padded footer container.
#[component]
pub fn ThemeSelector() -> impl IntoView {
    // `None` ⇒ System (no explicit pref); `Some(Light/Dark)` ⇒
    // explicit. Starts as System; the on-mount fetch may flip it.
    let (selected, set_selected) = signal::<Option<ExplicitTheme>>(None);

    // Bootstrap: pull the user's stored pref on first render. If
    // they have one, mirror it into the selected signal AND apply
    // it to the DOM (overriding the system-pref bootstrap that
    // already ran in main.rs).
    leptos::task::spawn_local(async move {
        if let Ok(me) = client::api_get::<UserMePrefs>("/users/me").await {
            if let Some(stored) = me
                .ui_prefs
                .as_ref()
                .and_then(|p| p.theme.as_deref())
                .and_then(theme::pref_from_str)
            {
                set_selected.set(Some(stored));
                theme::apply_explicit_theme(Some(stored));
            }
            // No `else` arm — absent / "system" stays at the
            // default (None ⇒ System), and main.rs already
            // applied the OS pref to the DOM.
        }
    });

    // Click handler factory. Each button calls this with its
    // target value; the handler updates the signal + dispatches
    // the change_theme call which handles apply + PUT in one go.
    let on_pick = move |target: Option<ExplicitTheme>| {
        set_selected.set(target);
        leptos::task::spawn_local(async move {
            if let Err(e) = theme::change_theme(target).await {
                // The local apply already happened in
                // change_theme. Log the persistence failure so it
                // shows in the console; the user sees the visual
                // change either way.
                web_sys::console::warn_1(
                    &format!("theme persistence failed: {e:?}").into(),
                );
            }
        });
    };

    view! {
        <div class="theme-selector" role="group" aria-label=crate::t!("theme-aria-label")>
            <button
                class="theme-selector-btn"
                class:selected=move || selected.get().is_none()
                on:click=move |_| on_pick(None)
                title=crate::t!("theme-system")
            >
                <span class="theme-selector-emoji">{"\u{1F5A5}\u{FE0F}"}</span>
                <span class="theme-selector-label">{crate::t!("theme-label-system")}</span>
            </button>
            <button
                class="theme-selector-btn"
                class:selected=move || selected.get() == Some(ExplicitTheme::Light)
                on:click=move |_| on_pick(Some(ExplicitTheme::Light))
                title=crate::t!("theme-light")
            >
                <span class="theme-selector-emoji">{"\u{2600}\u{FE0F}"}</span>
                <span class="theme-selector-label">{crate::t!("theme-label-light")}</span>
            </button>
            <button
                class="theme-selector-btn"
                class:selected=move || selected.get() == Some(ExplicitTheme::Dark)
                on:click=move |_| on_pick(Some(ExplicitTheme::Dark))
                title=crate::t!("theme-dark")
            >
                <span class="theme-selector-emoji">{"\u{1F319}"}</span>
                <span class="theme-selector-label">{crate::t!("theme-label-dark")}</span>
            </button>
        </div>
    }
}
