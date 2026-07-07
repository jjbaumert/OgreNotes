// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod a11y;
mod api;
mod app;
mod collab;
mod commands;
mod components;
pub mod editor;
mod inserts;
// `i18n` now lives in `lib.rs` so editor/*.rs and other lib
// modules can call translate() without going through a shim.
// The binary re-imports both the module and the `t!` macro
// so bin-side `crate::t!` continues to resolve.
pub use ogrenotes_frontend::i18n;
pub use ogrenotes_frontend::t;
pub mod observability;
mod pages;
mod rum;
mod spreadsheet;
mod theme;
mod touch;

fn main() {
    install_panic_hook();

    // Phase 1 observability — apply any `?debug=...&level=...`
    // URL flag before mount so the very first emits (locale,
    // theme, auth hydration) can be captured if requested. The
    // call is a no-op when no flag is present.
    editor::debug::init_from_url();

    // Phase 5 M-P2: initialize the i18n harness before mount so
    // any component that renders translated strings has a valid
    // bundle at render time. Piece 2 walks the URL → navigator
    // .language → en-US precedence chain in `resolve_locale`;
    // the user's stored UiPrefs.locale is consulted later by the
    // locale-switcher component (piece 3) once /users/me resolves.
    // Brief locale flash on first load is the same trade-off the
    // theme bootstrap makes.
    let locale = i18n::resolve_locale();
    i18n::init(&locale);

    // Phase 5 M-P4 piece A: install the command palette's baseline
    // action set. Has to run after `i18n::init` because each command's
    // label resolves through the active fluent bundle at render time;
    // the registration call itself doesn't touch translations, but
    // any subsequent palette query would see raw keys if init hadn't
    // run yet. Idempotent on `id`, so a future re-registration path
    // (e.g. per-page scope extensions in M-P4 piece B) can layer on
    // top safely.
    commands::register_defaults();
    // M-P4 piece C: rehydrate the most-recently-used command list
    // from localStorage so the next palette open ranks familiar
    // commands first. Best-effort — failure (private browsing,
    // quota-full) silently degrades to no-MRU.
    commands::load_recent_from_storage();

    // Stamp `data-theme` (+ a11y attributes) on `<html>` BEFORE mount so
    // there's no flash during WASM hydration. #152: apply the user's
    // locally-cached explicit theme synchronously first — otherwise a
    // stored Dark pref on a light-OS machine flashed a light background
    // until `/users/me` resolved. Only when there's no cached explicit
    // choice do we fall back to the OS `prefers-color-scheme` pref (which
    // also installs the live OS-change listener). The authoritative
    // `/users/me` prefs are applied — and the cache refreshed — post-auth
    // in `apply_stored_prefs`.
    if !theme::apply_cached_prefs() {
        theme::apply_system_theme();
    }

    // Hydrate the in-memory access token from the HttpOnly refresh
    // cookie BEFORE mount so route guards (`is_authenticated()` checks
    // in home / document pages) see the correct logged-in state on
    // page load. Without this pre-mount step every reload would
    // bounce a logged-in user to /login. (#33 frontend half.)
    //
    // We use `wasm_bindgen_futures::spawn_local` rather than
    // `leptos::task::spawn_local` because the Leptos executor isn't
    // installed until `mount_to_body` runs — calling Leptos's
    // spawn_local before mount panics with "tried to spawn a Future
    // ... before the Executor had been set". The wasm-bindgen variant
    // just queues a microtask on the JS event loop and is safe pre-mount.
    wasm_bindgen_futures::spawn_local(async {
        let _ = api::client::try_hydrate_from_cookie().await;
        leptos::mount::mount_to_body(app::App);
        // #152: the real sidebar is now in the DOM — drop the static boot
        // skeleton (index.html) that covered the sidebar column during the
        // WASM/hydration gap. Leaving it would intercept clicks over the
        // sidebar (it's fixed at z-index 90), so removal is required.
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("boot-skeleton"))
        {
            el.remove();
        }
        // Phase 5 M-P9 piece C: install the RUM sampler after the
        // auth cookie has been hydrated, so the beacon's Bearer
        // token is valid for the 10% of sessions that get sampled.
        // No-op in the other 90%. Adds a `load` listener (waits for
        // the page's load event), then captures vitals 1.5 s later
        // and POSTs once to /metrics/rum.
        rum::init();
        // Phase 1 observability — install the periodic client-
        // telemetry flush. Auth must be hydrated first because the
        // POST is rejected without a Bearer token; the flush is a
        // no-op for the brief window before hydration finishes.
        //
        // The token-getter is injected here rather than reached
        // directly from observability::flush because the `api`
        // module lives only in this bin target, and the
        // observability module is shared with the lib target so
        // it must not import `crate::api`.
        observability::set_token_getter(api::client::get_token);
        observability::init_flush_loop();

        // Apply the user's persisted UI prefs (theme + accessibility +
        // locale) once auth is hydrated, so stored choices take effect
        // on every page — independent of whether the settings UI is
        // mounted. This lifts the responsibility that used to live on
        // the sidebar's ThemeSelector/LocaleSelector mount (moved to
        // /settings in the account-menu work). Best-effort: an
        // unauthenticated load (login page) just no-ops on the 401.
        apply_stored_prefs().await;
    });
}

/// Slim `/users/me` decode for the load-time prefs bootstrap — only
/// the fields that drive global application. Mirrors the per-consumer
/// slim-decode pattern the settings components use.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapMe {
    ui_prefs: Option<BootstrapPrefs>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapPrefs {
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    locale: Option<String>,
    #[serde(default)]
    dyslexic_font: Option<bool>,
    #[serde(default)]
    reduce_motion: Option<bool>,
}

/// Fetch the stored UI prefs and apply them to the document at load
/// time. Theme: an explicit stored pref overrides the system default
/// already applied pre-mount; absent / "system" leaves OS tracking in
/// place. Accessibility: writes the `<html>` attributes the
/// stylesheet keys off. Locale: first-login-on-a-new-device sync —
/// localStorage is empty so `resolve_locale` fell back to
/// navigator.language; if the stored pref differs, apply it + reload.
/// The `same_locale` equality guard makes this a no-op once the
/// reload settles (localStorage then carries the stored value).
async fn apply_stored_prefs() {
    let Ok(me) = api::client::api_get::<BootstrapMe>("/users/me").await else {
        return;
    };
    let Some(prefs) = me.ui_prefs else {
        return;
    };

    // Refresh the local cache from the server so the next load paints
    // correctly pre-mount (#152) — also covers a pref changed on another
    // device since this device last loaded.
    theme::cache_prefs(prefs.theme.as_deref(), prefs.dyslexic_font, prefs.reduce_motion);

    // Apply the server's theme authoritatively: explicit Light/Dark, or
    // re-engage OS tracking for "system". This corrects a stale cached
    // value applied pre-mount (e.g. the pref was changed elsewhere). An
    // absent theme field leaves the pre-mount state untouched.
    if let Some(theme_str) = prefs.theme.as_deref() {
        theme::apply_explicit_theme(theme::pref_from_str(theme_str));
    }

    theme::apply_a11y_prefs(prefs.dyslexic_font, prefs.reduce_motion);

    if let Some(stored) = prefs.locale.filter(|s| !s.is_empty()) {
        if !i18n::same_locale(&stored, &i18n::resolve_locale()) {
            i18n::set_locale(&stored);
            if let Some(window) = web_sys::window() {
                let _ = window.location().reload();
            }
        }
    }
}

/// Install the WASM panic hook. In debug builds (`cfg(debug_assertions)`)
/// install `console_error_panic_hook` so developers see the full
/// Rust panic message + a JS-formatted stack trace in DevTools. In
/// release builds (`#[cfg(not(debug_assertions))]`) install a
/// minimal hook that logs a generic "internal error" line — panic
/// messages can include formatted argument values (`assert_eq!`'s
/// left/right) which may carry user data, and function names alone
/// help an attacker map the codebase. (#41)
fn install_panic_hook() {
    #[cfg(debug_assertions)]
    {
        console_error_panic_hook::set_once();
    }
    #[cfg(not(debug_assertions))]
    {
        std::panic::set_hook(Box::new(|_info| {
            // Deliberately discard `_info`. No payload, no location,
            // no stack frames — anything from the panic site could
            // include argument values or symbol names that are not
            // safe for production console output. A future event-id
            // → server reporter belongs here, sending only an opaque
            // identifier the user can quote to support.
            web_sys::console::error_1(
                &"OgreNotes internal error — please refresh the page.".into(),
            );
        }));
    }
}
