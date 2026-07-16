// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P1 — runtime theme application.
//!
//! Piece C bootstraps the document with `data-theme="light"` or
//! `"dark"` based on the OS `prefers-color-scheme` pref (called
//! once from `main.rs` pre-mount, no flash for dark-OS users).
//! Piece D adds the user-pref layer on top: an explicit
//! Light/Dark override that wins over the OS pref, plus a
//! `change_theme` entry point that the sidebar's theme selector
//! calls (applies locally + persists via PUT /users/me/prefs).
//!
//! Listener lifecycle: the OS-pref change listener is tracked in
//! a thread_local OnceCell so calling `apply_system_theme` more
//! than once doesn't accumulate duplicate listeners. Switching to
//! explicit Light/Dark removes the listener (otherwise an OS
//! toggle would clobber the user's choice). Switching back to
//! System re-installs it.

use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::client::{self, ApiClientError};

thread_local! {
    /// Stores the currently-installed `prefers-color-scheme` change
    /// listener and the MediaQueryList it's attached to. The pair
    /// is needed for `removeEventListener` (which requires the same
    /// callback reference that was registered). `None` ⇒ no
    /// listener installed; explicit-Light/Dark mode lives here.
    static SYSTEM_LISTENER: RefCell<Option<InstalledListener>> = const { RefCell::new(None) };
}

struct InstalledListener {
    media: web_sys::MediaQueryList,
    closure: Closure<dyn FnMut(web_sys::Event)>,
}

/// Explicit theme choice the user can pick. `None` (System) means
/// "track the OS pref"; `Some(Light)`/`Some(Dark)` override it.
/// The discriminator the storage layer uses
/// (`ogrenotes_storage::models::user::ThemePref`) has three
/// variants — System, Light, Dark — so this enum maps directly
/// minus the System variant (which is the `None` case at the
/// frontend boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplicitTheme {
    Light,
    Dark,
}

// ─── Pre-mount pref cache (#152) ────────────────────────────────
//
// The stored UI prefs live server-side and only arrive after
// `GET /users/me` resolves — well after mount. Until then `<html>`
// reflected only the OS `prefers-color-scheme`, so a user whose stored
// theme diverged from their OS flashed the wrong background on every
// load/navigation. Mirroring the boot-relevant prefs into localStorage
// lets `main.rs` paint the user's real choice *synchronously, pre-mount*;
// `/users/me` remains the source of truth and refreshes the cache.

const THEME_KEY: &str = "ogrenotes.theme";
const DYSLEXIC_KEY: &str = "ogrenotes.dyslexic_font";
const REDUCE_MOTION_KEY: &str = "ogrenotes.reduce_motion";
const DOC_THEME_KEY: &str = "ogrenotes.doc_theme";

/// The document typography themes (#59 T-12, branding.md §Typography).
/// `"default"` is Inter (the `:root` values in variables.css) and carries
/// no `data-doc-theme` attribute; the others each map to a
/// `[data-doc-theme="<id>"]` block that re-points `--font-doc-*`. Order is
/// the selector's display order.
pub const DOC_THEMES: &[&str] = &["default", "editorial", "handwritten", "technical", "classic"];

/// Canonicalize a stored/selected doc-theme id: a known non-default id
/// stays as-is; `"default"`, unknown, or empty collapses to `None` (=
/// Inter, no attribute). Keeps a stale/garbage stored value from painting
/// a phantom attribute.
pub fn normalize_doc_theme(s: &str) -> Option<&'static str> {
    DOC_THEMES
        .iter()
        .copied()
        .find(|&t| t == s && t != "default")
}

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

/// Cache the boot-relevant prefs in localStorage so the *next* load can
/// apply them pre-mount with no flash. Each `None` field is left
/// untouched. Best-effort — private browsing / quota-full silently no-op.
pub fn cache_prefs(theme: Option<&str>, dyslexic_font: Option<bool>, reduce_motion: Option<bool>) {
    let Some(ls) = local_storage() else {
        return;
    };
    if let Some(t) = theme {
        let _ = ls.set_item(THEME_KEY, t);
    }
    if let Some(on) = dyslexic_font {
        let _ = ls.set_item(DYSLEXIC_KEY, if on { "true" } else { "false" });
    }
    if let Some(on) = reduce_motion {
        let _ = ls.set_item(REDUCE_MOTION_KEY, if on { "true" } else { "false" });
    }
}

/// Apply the localStorage-cached theme + accessibility prefs to `<html>`
/// synchronously, before mount. Returns `true` iff an explicit Light/Dark
/// theme was applied from cache — the caller then SKIPS the OS fallback.
/// `false` (no cache, or cached "system"/unknown) means the caller should
/// apply the OS pref. This is the #152 fix.
pub fn apply_cached_prefs() -> bool {
    let Some(ls) = local_storage() else {
        return false;
    };

    // Accessibility attributes — independent of the theme branch below.
    let dyslexic = ls.get_item(DYSLEXIC_KEY).ok().flatten().map(|v| v == "true");
    let reduce_motion = ls
        .get_item(REDUCE_MOTION_KEY)
        .ok()
        .flatten()
        .map(|v| v == "true");
    apply_a11y_prefs(dyslexic, reduce_motion);
    // Document typography theme — independent of the color-scheme branch.
    apply_doc_theme(ls.get_item(DOC_THEME_KEY).ok().flatten().as_deref());

    let cached_theme = ls.get_item(THEME_KEY).ok().flatten();
    match cached_theme.as_deref().and_then(pref_from_str) {
        Some(explicit) => {
            apply_explicit_theme(Some(explicit));
            true
        }
        None => false,
    }
}

/// Apply the OS-level color-scheme pref to `<html>` and install
/// (or re-install) the listener so subsequent OS changes are
/// reflected live. Idempotent — multiple calls don't accumulate
/// listeners thanks to the thread_local guard.
pub fn apply_system_theme() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(media)) = window.match_media("(prefers-color-scheme: dark)") else {
        return;
    };
    apply_theme_for_media(&media);

    SYSTEM_LISTENER.with(|cell| {
        if cell.borrow().is_some() {
            return; // listener already installed
        }
        let closure = Closure::wrap(Box::new(|_event: web_sys::Event| {
            if let Some(w) = web_sys::window() {
                if let Ok(Some(m)) = w.match_media("(prefers-color-scheme: dark)") {
                    apply_theme_for_media(&m);
                }
            }
        }) as Box<dyn FnMut(web_sys::Event)>);
        let _ = media
            .add_event_listener_with_callback("change", closure.as_ref().unchecked_ref());
        *cell.borrow_mut() = Some(InstalledListener {
            media: media.clone(),
            closure,
        });
    });
}

/// Apply an explicit user preference (Light / Dark) or re-engage
/// system tracking (None). When switching to explicit mode this
/// removes the system listener so subsequent OS toggles don't
/// override the user's choice; switching back to None re-installs.
pub fn apply_explicit_theme(theme: Option<ExplicitTheme>) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(root) = document.document_element() else {
        return;
    };
    match theme {
        Some(ExplicitTheme::Light) => {
            remove_system_listener();
            let _ = root.set_attribute("data-theme", "light");
        }
        Some(ExplicitTheme::Dark) => {
            remove_system_listener();
            let _ = root.set_attribute("data-theme", "dark");
        }
        None => {
            // Re-engage OS tracking. apply_system_theme is
            // idempotent — safe even if a listener is already
            // installed (the guard short-circuits).
            apply_system_theme();
        }
    }
}

fn remove_system_listener() {
    SYSTEM_LISTENER.with(|cell| {
        if let Some(installed) = cell.borrow_mut().take() {
            let _ = installed.media.remove_event_listener_with_callback(
                "change",
                installed.closure.as_ref().unchecked_ref(),
            );
            // Dropping `installed` drops the closure, releasing the
            // JS-side wrapper.
        }
    });
}

/// Apply a theme change locally AND persist it via
/// `PUT /users/me/prefs`. Called from the theme selector UI when
/// the user clicks a different option.
///
/// On a server error the local apply still happened — the user
/// sees the change immediately, and the next page load will pull
/// the stale stored pref. This is the right trade-off: a 500 from
/// the server shouldn't revert the user's UI choice mid-click.
pub async fn change_theme(theme: Option<ExplicitTheme>) -> Result<(), ApiClientError> {
    apply_explicit_theme(theme);
    let wire_str = match theme {
        None => "system",
        Some(ExplicitTheme::Light) => "light",
        Some(ExplicitTheme::Dark) => "dark",
    };
    // Cache immediately so the next load paints this choice pre-mount (#152),
    // without waiting for the PUT round-trip or the next /users/me.
    cache_prefs(Some(wire_str), None, None);
    client::api_put("/users/me/prefs", &serde_json::json!({ "theme": wire_str })).await
}

/// Map a server-side `ThemePref` string (the on-wire form of
/// `ogrenotes_storage::models::user::ThemePref`) into the
/// frontend's `Option<ExplicitTheme>` shape. "system" / unknown /
/// missing all resolve to None.
pub fn pref_from_str(s: &str) -> Option<ExplicitTheme> {
    match s {
        "light" => Some(ExplicitTheme::Light),
        "dark" => Some(ExplicitTheme::Dark),
        _ => None,
    }
}

/// Apply the accessibility UI prefs to `<html>` as data attributes
/// the stylesheet keys off (`[data-dyslexic]`,
/// `[data-reduce-motion]`). A `None` field leaves the existing
/// attribute untouched — so a single-field toggle from the settings
/// UI doesn't disturb the other pref. Pure DOM; persistence is the
/// caller's job (mirrors `apply_explicit_theme` vs `change_theme`).
pub fn apply_a11y_prefs(dyslexic_font: Option<bool>, reduce_motion: Option<bool>) {
    let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        return;
    };
    if let Some(on) = dyslexic_font {
        set_bool_attr(&root, "data-dyslexic", on);
    }
    if let Some(on) = reduce_motion {
        set_bool_attr(&root, "data-reduce-motion", on);
    }
}

/// Apply the document typography theme (#59 T-12) to `<html>` as
/// `data-doc-theme="<id>"`, the attribute the `[data-doc-theme=…]` blocks
/// in variables.css key off. A known non-default id sets the attribute; a
/// `None`/`"default"`/unknown value removes it, falling back to the
/// `:root` Inter defaults. Pure DOM; persistence is `change_doc_theme`'s
/// job (mirrors `apply_a11y_prefs` vs the settings writers).
pub fn apply_doc_theme(theme: Option<&str>) {
    let Some(root) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
    else {
        return;
    };
    match theme.and_then(normalize_doc_theme) {
        Some(id) => {
            let _ = root.set_attribute("data-doc-theme", id);
        }
        None => {
            let _ = root.remove_attribute("data-doc-theme");
        }
    }
}

/// Cache the document theme in localStorage so the next load can paint it
/// pre-mount (mirrors `cache_prefs`). Best-effort; a `None` leaves the
/// stored value untouched, matching the other cache writers.
pub fn cache_doc_theme(theme: Option<&str>) {
    if let (Some(ls), Some(t)) = (local_storage(), theme) {
        let _ = ls.set_item(DOC_THEME_KEY, t);
    }
}

/// Apply the localStorage-cached document theme to `<html>` pre-mount.
/// Called from `apply_cached_prefs`; separate so the boot path can invoke
/// it independently.
pub fn apply_cached_doc_theme() {
    if let Some(ls) = local_storage() {
        let cached = ls.get_item(DOC_THEME_KEY).ok().flatten();
        apply_doc_theme(cached.as_deref());
    }
}

/// Apply + cache + persist a document typography theme selection. Like
/// `change_theme`: the DOM flips immediately, the cache is written for the
/// next pre-mount paint, and the PUT persists across sessions; a server
/// error leaves the local change in place. Pass `"default"` (or `None`) to
/// clear back to Inter.
pub async fn change_doc_theme(theme: Option<&str>) -> Result<(), ApiClientError> {
    let wire = theme.and_then(normalize_doc_theme).unwrap_or("default");
    apply_doc_theme(Some(wire));
    cache_doc_theme(Some(wire));
    client::api_put("/users/me/prefs", &serde_json::json!({ "docTheme": wire })).await
}

/// Set `name="true"` when `on`, otherwise remove the attribute
/// entirely (so the CSS `[name="true"]` selectors match only the
/// enabled state and there's no lingering `="false"` to reason about).
fn set_bool_attr(root: &web_sys::Element, name: &str, on: bool) {
    if on {
        let _ = root.set_attribute(name, "true");
    } else {
        let _ = root.remove_attribute(name);
    }
}

/// Reflect the given media-query state onto `<html>`. Extracted
/// so the change listener can call the same code path as the
/// initial application.
fn apply_theme_for_media(media: &web_sys::MediaQueryList) {
    let theme = if media.matches() { "dark" } else { "light" };
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        if let Some(root) = document.document_element() {
            let _ = root.set_attribute("data-theme", theme);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_doc_theme_maps_known_and_rejects_the_rest() {
        // Known non-default ids pass through unchanged.
        assert_eq!(normalize_doc_theme("editorial"), Some("editorial"));
        assert_eq!(normalize_doc_theme("handwritten"), Some("handwritten"));
        assert_eq!(normalize_doc_theme("technical"), Some("technical"));
        assert_eq!(normalize_doc_theme("classic"), Some("classic"));
        // "default" is Inter (no attribute) — collapses to None.
        assert_eq!(normalize_doc_theme("default"), None);
        // Unknown / stale / empty values collapse to None, never a
        // phantom attribute.
        assert_eq!(normalize_doc_theme("Editorial"), None); // case-sensitive
        assert_eq!(normalize_doc_theme("bogus"), None);
        assert_eq!(normalize_doc_theme(""), None);
    }

    #[test]
    fn doc_themes_catalog_shape() {
        // "default" must be first (the selector's default option) and the
        // catalog must contain each themed id exactly once.
        assert_eq!(DOC_THEMES.first(), Some(&"default"));
        for id in ["editorial", "handwritten", "technical", "classic"] {
            assert_eq!(DOC_THEMES.iter().filter(|&&t| t == id).count(), 1, "{id}");
        }
    }
}
