// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Account settings page — `/settings`.
//!
//! Step 1 of `design/account-menu.md`: the route shell. A tabbed
//! page with five sections (Profile / Appearance / Notifications /
//! Accessibility / Help & Support). The panels are placeholders in
//! this step; subsequent steps move the existing Theme/Locale
//! selectors into Appearance (step 2), add the avatar account menu
//! (step 3), then profile editing / status / notification prefs
//! (steps 4-6).
//!
//! The active tab is mirrored to the URL fragment (`/settings#appearance`)
//! so sections are deep-linkable — this is also the redirect target
//! for the legacy `/profile` link (see [`ProfileRedirect`]).

use leptos::prelude::*;
use leptos_router::hooks::use_location;
use leptos_router::hooks::use_navigate;

use crate::api::client;
use crate::components::accessibility_settings::AccessibilitySettings;
use crate::components::notification_settings::NotificationSettings;
use crate::components::profile_settings::ProfileSettings;
use crate::components::search_dialog::SearchDialog;
use crate::components::status_editor::StatusEditor;
use crate::components::locale_selector::LocaleSelector;
use crate::components::theme_selector::ThemeSelector;

/// Stable section ids. These are the URL-fragment values
/// (`/settings#profile`) and the `role="tab"` keys — keep them in
/// sync with the `t!` labels below.
const TABS: &[(&str, fn() -> String)] = &[
    ("profile", || crate::t!("settings-tab-profile")),
    ("appearance", || crate::t!("settings-tab-appearance")),
    ("notifications", || crate::t!("settings-tab-notifications")),
    ("accessibility", || crate::t!("settings-tab-accessibility")),
    ("help", || crate::t!("settings-tab-help")),
];

/// Read the active section from the URL fragment, defaulting to
/// "profile" when absent or unrecognized. Strips the leading `#`.
/// Resolve a URL fragment (with or without the leading `#`) to a known tab
/// id, defaulting to "profile".
fn tab_id_from(raw: &str) -> String {
    let trimmed = raw.trim_start_matches('#');
    if TABS.iter().any(|(id, _)| *id == trimmed) {
        trimmed.to_string()
    } else {
        "profile".to_string()
    }
}

fn tab_from_hash() -> String {
    let raw = web_sys::window()
        .and_then(|w| w.location().hash().ok())
        .unwrap_or_default();
    tab_id_from(&raw)
}

#[component]
pub fn SettingsPage() -> impl IntoView {
    if !client::is_authenticated() {
        let navigate = use_navigate();
        navigate("/login", Default::default());
        return view! { <div>{crate::t!("common-redirecting-login")}</div> }.into_any();
    }

    // #152: the sidebar lives in `AppShell`; pull the shared ctx so the
    // Search/Ask dialogs mounted below open from the (persistent) sidebar
    // entries.
    let ctx = use_context::<crate::components::app_shell::ShellCtx>()
        .expect("ShellCtx provided by AppShell");

    let (active, set_active) = signal(tab_from_hash());

    // #152: drive the active tab off the router's reactive location hash
    // rather than a window "hashchange" listener. Client-side navigation
    // (the palette "Help" command, the account menu, in-page tab clicks)
    // updates the URL via history pushState, which does NOT fire
    // "hashchange" — but the router's `location.hash` memo does update, so
    // an Effect on it covers deep links, Back/Forward, AND client-side nav.
    let location = use_location();
    Effect::new(move |_| {
        set_active.set(tab_id_from(&location.hash.get()));
    });

    // Switching a tab is a client-side navigation to the fragment (no
    // reload), so the section stays deep-linkable and Back walks tab-to-tab;
    // the Effect above reflects the change into `active`. Routed through the
    // shell-installed nav bridge so this closure stays `Copy` (reusable
    // across every tab button) and falls back to a full load if unmounted.
    let select =
        move |id: &'static str| crate::commands::nav_bridge::go(&format!("/settings#{id}"));

    view! {
        // #152: sidebar lives in the shell; settings renders as Outlet content.
        <>
            <main id="main-content" tabindex="-1" class="main-content">
                <div class="settings-page">
                    <h1 class="settings-title">{crate::t!("settings-title")}</h1>
                    <div class="settings-body">
                        <nav
                            class="settings-tabs"
                            role="tablist"
                            aria-label=crate::t!("settings-aria-tabs")
                        >
                            {TABS.iter().map(|(id, label)| {
                                let id = *id;
                                let label = *label;
                                let is_active = move || active.get() == id;
                                view! {
                                    <button
                                        id=format!("settings-tab-{id}")
                                        class="settings-tab"
                                        class:selected=is_active
                                        role="tab"
                                        aria-selected=move || is_active().to_string()
                                        on:click=move |_| select(id)
                                    >
                                        {label}
                                    </button>
                                }
                            }).collect_view()}
                        </nav>
                        <section
                            class="settings-panel"
                            role="tabpanel"
                            tabindex="0"
                            aria-labelledby=move || format!("settings-tab-{}", active.get())
                        >
                            {move || {
                                let current = active.get();
                                let heading = TABS
                                    .iter()
                                    .find(|(id, _)| *id == current)
                                    .map(|(_, label)| label())
                                    .unwrap_or_default();
                                view! {
                                    <h2 class="settings-panel-title">{heading}</h2>
                                    {panel_body(&current)}
                                }
                            }}
                        </section>
                    </div>
                </div>
            </main>
            // #152: the persistent sidebar always offers Search/Ask; mount
            // the (Global-scoped) dialogs here so those entries work on the
            // settings page too.
            <SearchDialog
                visible=ctx.search_open.read_only()
                on_close=Callback::new(move |_| ctx.search_open.set(false))
            />
            <crate::components::ask_dialog::AskDialog
                visible=Signal::from(ctx.ask_open)
                on_close=Callback::new(move |_| ctx.ask_open.set(false))
            />
        </>
    }
    .into_any()
}

/// Render the body of the active section. Appearance and
/// Accessibility carry their controls (moved out of the sidebar in
/// step 2); the remaining sections are placeholders until steps 4-6
/// (profile editing, notification prefs, help) fill them in.
fn panel_body(tab: &str) -> AnyView {
    match tab {
        "profile" => view! {
            <StatusEditor />
            <ProfileSettings />
        }
        .into_any(),
        "appearance" => view! {
            <div class="settings-field">
                <span class="settings-field-label">
                    {crate::t!("settings-appearance-theme")}
                </span>
                <ThemeSelector />
            </div>
            // Language lives here (not in Profile): its i18n key has
            // always been `settings-appearance-language`, and the
            // account menu's "Settings" entry lands on this tab.
            <div class="settings-field">
                <span class="settings-field-label">
                    {crate::t!("settings-appearance-language")}
                </span>
                <LocaleSelector />
            </div>
        }
        .into_any(),
        "accessibility" => view! { <AccessibilitySettings /> }.into_any(),
        "notifications" => view! { <NotificationSettings /> }.into_any(),
        "help" => help_panel(),
        _ => view! {
            <p class="settings-panel-muted">{crate::t!("settings-coming-soon")}</p>
        }
        .into_any(),
    }
}

/// Static Help & Support content: the platform-correct keyboard
/// shortcuts and the build stamp. No external docs link yet — a
/// real URL doesn't exist, and a placeholder would just be a dead
/// link (the kind step 1 set out to remove).
fn help_panel() -> AnyView {
    let is_mac = web_sys::window()
        .and_then(|w| w.navigator().platform().ok())
        .map(|p| p.to_lowercase().contains("mac"))
        .unwrap_or(false);
    let palette_key = if is_mac { "\u{2318}K" } else { "Ctrl+K" };
    let actions_key = if is_mac { "\u{2318}\u{21E7}P" } else { "Ctrl+Shift+P" };
    let version = format!(
        "v{} {}",
        env!("CARGO_PKG_VERSION"),
        option_env!("GIT_HASH").unwrap_or("unknown"),
    );

    view! {
        <h3 class="settings-subheading">{crate::t!("settings-help-shortcuts")}</h3>
        <dl class="settings-shortcuts">
            <div class="settings-shortcut-row">
                <dt><kbd>{palette_key}</kbd></dt>
                <dd>{crate::t!("settings-help-shortcut-palette")}</dd>
            </div>
            <div class="settings-shortcut-row">
                <dt><kbd>{actions_key}</kbd></dt>
                <dd>{crate::t!("settings-help-shortcut-actions")}</dd>
            </div>
        </dl>
        <h3 class="settings-subheading">{crate::t!("settings-help-version")}</h3>
        <p class="settings-panel-muted">{version}</p>
    }
    .into_any()
}

/// Redirect target for the legacy `/profile` link still emitted by
/// the sidebar. Sends the user to the unified settings page, which
/// opens on the Profile section by default (`tab_from_hash` falls
/// back to "profile"). Once the account menu (step 3) replaces the
/// sidebar profile button this route can be retired.
#[component]
pub fn ProfileRedirect() -> impl IntoView {
    let navigate = use_navigate();
    navigate("/settings", Default::default());
    view! { <div>{crate::t!("common-redirecting")}</div> }
}
