// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Embedded UI catalogs, locale selection, and browser Intl formatting.
//!
//! Loads the active-locale and en-US-fallback bundles at bootstrap
//! and exposes a `t!()` macro that resolves a message id against
//! the active bundle, falling back to en-US, falling back to the
//! key itself. Never panics, never returns empty.
//!
//! Six embedded catalogs provide the UI languages. Regional tags select
//! the matching language catalog while retaining regional Intl formatting.
//!
//! Bootstrap entry point: `i18n::init()`, called once from
//! `main.rs` before mount. After init, any code (component or
//! pure function) can call `t!("key")`.

use std::cell::RefCell;

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource};
use js_sys::{Date, Object, Reflect};
use unic_langid::{langid, LanguageIdentifier};
use wasm_bindgen::prelude::*;

// Locale catalogs compiled into the WASM. Each `.ftl` file is one
// "domain" (currently just `main`); the doc carries the rationale
// for the per-locale-per-domain split that v2 will use to lazy-load.
const EN_US_MAIN_FTL: &str = include_str!("../locales/en-US/main.ftl");
const AR_MAIN_FTL: &str = include_str!("../locales/ar/main.ftl");
const ES_MAIN_FTL: &str = include_str!("../locales/es/main.ftl");
const IT_MAIN_FTL: &str = include_str!("../locales/it/main.ftl");
const FR_MAIN_FTL: &str = include_str!("../locales/fr/main.ftl");
const DE_MAIN_FTL: &str = include_str!("../locales/de/main.ftl");

/// Six shipped UI languages, labeled in their own language.
pub const LOCALE_CHOICES: &[(&str, &str)] = &[
    ("en-US", "English"),
    ("de", "Deutsch"),
    ("es", "Español"),
    ("fr", "Français"),
    ("it", "Italiano"),
    ("ar", "العربية"),
];

fn supported_locale(locale: &str) -> Option<LanguageIdentifier> {
    let id = locale.parse::<LanguageIdentifier>().ok()?;
    LOCALE_CHOICES
        .iter()
        .any(|(tag, _)| same_locale(tag, locale))
        .then_some(id)
}

/// Map a regional tag to the corresponding option in the language selector.
pub fn locale_choice(locale: &str) -> &'static str {
    LOCALE_CHOICES
        .iter()
        .find(|(tag, _)| same_locale(tag, locale))
        .map(|(tag, _)| *tag)
        .unwrap_or("en-US")
}

thread_local! {
    /// The active bundle + the en-US fallback bundle. Initialized
    /// by `init()`; queried by `translate()`. `None` until init
    /// runs — `translate` falls back to returning the key itself
    /// in that uninitialized window, so a sloppy call ordering
    /// won't crash (it'll just show the raw key id in the UI).
    static BUNDLES: RefCell<Option<LoadedBundles>> = const { RefCell::new(None) };

    /// BCP-47 locale tag corresponding to the active bundle. The
    /// locale-aware formatters (`format_date` / `format_number` /
    /// `format_relative`) read from this to construct Intl.*
    /// instances. Mirrors the bundle's locale — `init` and
    /// `set_locale` write both atomically. Empty before init;
    /// formatters fall back to "en-US" in that uninitialized
    /// window so a sloppy call-order won't return garbage.
    static ACTIVE_LOCALE_TAG: RefCell<String> = const { RefCell::new(String::new()) };
}

struct LoadedBundles {
    /// User's active locale.
    active: FluentBundle<FluentResource>,
    /// en-US catalog, always loaded — used when the active bundle
    /// is missing a key. When `active` IS en-US, this field is
    /// still populated (the queries are cheap; not worth special-
    /// casing).
    fallback: FluentBundle<FluentResource>,
}

/// Initialize the embedded catalogs. Call once from `main.rs` before mount.
///
/// `locale` is the active BCP-47 tag. Regional tags use the matching
/// language catalog; unknown or unsupported tags fall back to en-US.
///
/// Most callers should use
/// [`resolve_locale_with_hint`] to apply URL, server preference, local
/// preference, browser language, and en-US fallback in precedence order.
pub fn init(locale: &str) {
    let active_id = parse_or_default(locale);
    let active = build_bundle(&active_id, ftl_for(&active_id));
    let en_us_id: LanguageIdentifier = langid!("en-US");
    let fallback = build_bundle(&en_us_id, EN_US_MAIN_FTL);
    BUNDLES.with(|cell| {
        *cell.borrow_mut() = Some(LoadedBundles { active, fallback });
    });
    ACTIVE_LOCALE_TAG.with(|cell| {
        *cell.borrow_mut() = active_id.to_string();
    });
    sync_html_lang_dir(&active_id);
}

/// Swap the active locale at runtime. Rebuilds the active bundle;
/// the en-US fallback stays put. Also writes the choice to
/// `localStorage["ogrenotes.locale"]` so subsequent page loads can use it
/// (see [`resolve_locale`]).
///
/// This does not trigger a Leptos re-render. The locale selector saves the
/// server preference and reloads the page after calling `set_locale`.
pub fn set_locale(locale: &str) {
    let active_id = parse_or_default(locale);
    let active = build_bundle(&active_id, ftl_for(&active_id));
    BUNDLES.with(|cell| {
        if let Some(loaded) = cell.borrow_mut().as_mut() {
            loaded.active = active;
        }
    });
    ACTIVE_LOCALE_TAG.with(|cell| {
        *cell.borrow_mut() = active_id.to_string();
    });
    sync_html_lang_dir(&active_id);
    write_localstorage_locale(&active_id.to_string());
}

/// Resolve which locale to use at bootstrap, walking the
/// precedence chain documented in `design/i18n.md`:
///
///   1. URL `?locale=<bcp47>` query parameter
///   2. `localStorage["ogrenotes.locale"]` (cache of the user's pick,
///      written by [`set_locale`] when the switcher fires)
///   3. `navigator.language` (browser default)
///   4. `en-US` (last-resort fallback)
///
/// This is the no-server-hint resolver, used for the logged-out /
/// pre-auth case where there is no server pref to fold in. Once
/// the boot refresh resolves, `main.rs` instead calls
/// [`resolve_locale_with_hint`], which folds the server-stored
/// `UiPrefs.locale` (delivered on the auth response) into tier 2 of
/// the precedence chain and is applied before mount — no async
/// `/users/me` fetch and no reload required.
///
/// Unknown / unsupported BCP-47 tags from any layer fall through
/// to the next layer.
pub fn resolve_locale() -> String {
    pick_locale(
        locale_from_url(),
        None,
        locale_from_localstorage(),
        locale_from_navigator(),
    )
}

/// Like [`resolve_locale`] but folds the server-stored pref (delivered
/// on the auth response) into tier 2 of the precedence chain, matching
/// URL > server preference > `localStorage["ogrenotes.locale"]` >
/// `navigator.language` > en-US. Unsupported tags are skipped.
/// Called from `main.rs` once the boot refresh resolves.
pub fn resolve_locale_with_hint(server_hint: Option<&str>) -> String {
    pick_locale(
        locale_from_url(),
        server_hint.map(str::to_string),
        locale_from_localstorage(),
        locale_from_navigator(),
    )
}

/// First valid, supported locale in precedence order wins, else en-US.
/// Split out from the web_sys readers so it is unit-testable natively.
fn pick_locale(
    url: Option<String>,
    hint: Option<String>,
    stored: Option<String>,
    navigator: Option<String>,
) -> String {
    [url, hint, stored, navigator]
        .into_iter()
        .flatten()
        .find_map(|s| supported_locale(&s).map(|id| id.to_string()))
        .unwrap_or_else(|| "en-US".to_string())
}

/// Compare two BCP-47 tags as "the same locale", treating a bare
/// primary subtag and a regioned variant as equal (`en` == `en-US`)
/// because users can land with either from `navigator.language`
/// while the catalog only ships the regioned form. Used by the
/// app-load prefs bootstrap and the locale switcher to decide
/// whether a stored pref actually differs from the active locale.
pub fn same_locale(a: &str, b: &str) -> bool {
    let primary = |s: &str| {
        s.split(|c| c == '-' || c == '_')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    a.eq_ignore_ascii_case(b) || primary(a) == primary(b)
}

const LOCALSTORAGE_KEY: &str = "ogrenotes.locale";

fn locale_from_localstorage() -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let value = storage.get_item(LOCALSTORAGE_KEY).ok()??;
    if value.is_empty() {
        return None;
    }
    Some(value)
}

fn write_localstorage_locale(locale: &str) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(storage)) = window.local_storage() else {
        return;
    };
    // Best-effort — localStorage can be disabled / quota-full in
    // private-browsing modes. A failure means the next bootstrap
    // falls back to navigator.language; the user's choice is still
    // on the server via the switcher's PUT. Not worth surfacing.
    let _ = storage.set_item(LOCALSTORAGE_KEY, locale);
}

fn locale_from_url() -> Option<String> {
    let window = web_sys::window()?;
    let location = window.location();
    let search = location.search().ok()?;
    // search is like "?locale=ar&foo=bar"; strip the leading "?"
    // and walk pairs. URLSearchParams would be cleaner but its
    // web-sys binding is gated behind an extra feature.
    let search = search.strip_prefix('?').unwrap_or(&search);
    for pair in search.split('&') {
        if let Some(value) = pair.strip_prefix("locale=") {
            let decoded = urlencoding::decode(value).ok()?.into_owned();
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }
    None
}

fn locale_from_navigator() -> Option<String> {
    let nav = web_sys::window()?.navigator();
    let lang = nav.language()?;
    if lang.is_empty() {
        return None;
    }
    Some(lang)
}

#[cfg(test)]
mod locale_precedence_tests {
    use super::pick_locale;

    fn s(v: &str) -> Option<String> { Some(v.to_string()) }

    #[test]
    fn url_beats_everything() {
        assert_eq!(pick_locale(s("fr"), s("ar"), s("es"), s("de")), "fr");
    }
    #[test]
    fn hint_beats_localstorage_and_navigator() {
        assert_eq!(pick_locale(None, s("ar"), s("es"), s("de")), "ar");
    }
    #[test]
    fn localstorage_beats_navigator() {
        assert_eq!(pick_locale(None, None, s("es"), s("de")), "es");
    }
    #[test]
    fn navigator_is_the_last_real_layer() {
        assert_eq!(pick_locale(None, None, None, s("de")), "de");
    }
    #[test]
    fn empty_strings_are_skipped_and_default_applies() {
        assert_eq!(pick_locale(s(""), None, None, None), "en-US");
        assert_eq!(pick_locale(s(""), s(""), s(""), s("de")), "de");
    }

    #[test]
    fn invalid_and_untranslated_locales_fall_through() {
        assert_eq!(pick_locale(s("INVALID!"), s("ja"), s("de"), s("fr")), "de");
        assert_eq!(pick_locale(s("ja"), s("ar-EG"), s("de"), s("fr")), "ar-EG");
        assert_eq!(pick_locale(None, None, s("INVALID!"), s("fr-CA")), "fr-CA");
        assert_eq!(pick_locale(s("ja"), None, None, s("ja-JP")), "en-US");
    }

    #[test]
    fn regional_tags_keep_their_formatting_region_and_select_the_base_language() {
        for (regional, option) in [
            ("de-AT", "de"),
            ("fr-CA", "fr"),
            ("ar-EG", "ar"),
            ("en-GB", "en-US"),
        ] {
            assert_eq!(pick_locale(None, None, None, s(regional)), regional);
            assert_eq!(super::locale_choice(regional), option);
        }
        assert_eq!(super::parse_or_default("he").to_string(), "en-US");
    }
}

#[cfg(test)]
mod wasm_locale_tests {
    use wasm_bindgen_test::*;
    wasm_bindgen_test_configure!(run_in_browser);

    fn set_ls(key: &str, val: &str) {
        web_sys::window().unwrap().local_storage().unwrap().unwrap()
            .set_item(key, val).unwrap();
    }
    fn clear_ls(key: &str) {
        web_sys::window().unwrap().local_storage().unwrap().unwrap()
            .remove_item(key).unwrap();
    }

    #[wasm_bindgen_test]
    fn hint_wins_over_stored_localstorage() {
        set_ls("ogrenotes.locale", "es");
        // No ?locale= in the test URL, so URL layer is empty; the hint
        // (server pref) must beat the cached localStorage value.
        assert_eq!(super::resolve_locale_with_hint(Some("ar")), "ar");
        clear_ls("ogrenotes.locale");
    }

    #[wasm_bindgen_test]
    fn falls_back_to_localstorage_without_hint() {
        set_ls("ogrenotes.locale", "de");
        assert_eq!(super::resolve_locale_with_hint(None), "de");
        clear_ls("ogrenotes.locale");
    }

    #[wasm_bindgen_test]
    fn active_locale_reflects_what_init_applied() {
        // active_locale() must report the locale the harness actually
        // applied, not re-resolve precedence — that's the whole point
        // of the accessor (the locale switcher's initial <select>
        // value depends on this being correct on a fresh device with
        // a server-pref hint and empty localStorage).
        super::init("ar");
        assert_eq!(super::active_locale(), "ar");

        super::init("en-US");
        assert_eq!(super::active_locale(), "en-US");
    }
}

/// Stamp `<html lang="...">` and `<html dir="...">` to match the
/// active locale. The `lang` attribute helps screen readers
/// pronounce text correctly; the `dir` attribute drives the
/// LTR/RTL layout flip that CSS logical properties consume.
///
/// Direction list of known RTL languages: scoped to v1's pilot
/// (Arabic) plus the other common RTL tags so a future locale
/// addition Just Works.
fn sync_html_lang_dir(id: &LanguageIdentifier) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Some(root) = document.document_element() else {
        return;
    };
    let _ = root.set_attribute("lang", &id.to_string());
    let dir = if is_rtl(id) { "rtl" } else { "ltr" };
    let _ = root.set_attribute("dir", dir);
}

fn is_rtl(id: &LanguageIdentifier) -> bool {
    matches!(
        id.language.as_str(),
        "ar" | "fa" | "he" | "ur" | "yi" | "ps" | "sd"
    )
}

/// Look up a message id and format it with the given arguments.
/// Falls back to en-US if the active bundle doesn't carry the key,
/// then to the key itself as a visible debug-string.
///
/// Called by the `t!()` macro — most code shouldn't call this
/// directly. Exposed `pub` so the macro lives outside the module.
pub fn translate(key: &str, args: Option<&FluentArgs>) -> String {
    BUNDLES.with(|cell| {
        let borrowed = cell.borrow();
        let Some(bundles) = borrowed.as_ref() else {
            // Pre-init call — return the key id so the UI surfaces
            // the misuse instead of crashing.
            return key.to_string();
        };
        if let Some(s) = format_from(&bundles.active, key, args) {
            return s;
        }
        if let Some(s) = format_from(&bundles.fallback, key, args) {
            return s;
        }
        // Last-resort: the key itself. Debug-visible — a string
        // like "sidebar.title" rendered in the UI signals the
        // missing key without crashing the page.
        key.to_string()
    })
}

/// Format a single message from a bundle. `None` if the message
/// doesn't exist or formatting errors. We treat any format error
/// the same as "missing" — the fallback chain takes over.
fn format_from(
    bundle: &FluentBundle<FluentResource>,
    key: &str,
    args: Option<&FluentArgs>,
) -> Option<String> {
    let msg = bundle.get_message(key)?;
    let pattern = msg.value()?;
    let mut errors = Vec::new();
    let formatted = bundle.format_pattern(pattern, args, &mut errors);
    if !errors.is_empty() {
        // Format errors are usually missing args or wrong variant
        // selectors — debug-log them, don't fail the call.
        #[cfg(target_arch = "wasm32")]
        web_sys::console::warn_1(&format!("fluent format errors for {key:?}: {errors:?}").into());
        return None;
    }
    Some(formatted.into_owned())
}

fn parse_or_default(locale: &str) -> LanguageIdentifier {
    supported_locale(locale).unwrap_or_else(|| langid!("en-US"))
}

fn ftl_for(id: &LanguageIdentifier) -> &'static str {
    // Match on the language subtag rather than the full BCP-47 tag so
    // that regional variants (es-MX, fr-CA, de-AT, it-CH, ar-EG, …)
    // pick up the base-language catalog instead of falling through to
    // en-US. The granular tag still survives in `ACTIVE_LOCALE_TAG`,
    // so Intl.* formatters get the region-specific locale even when
    // the Fluent bundle is the base catalog.
    match id.language.as_str() {
        "ar" => AR_MAIN_FTL,
        "es" => ES_MAIN_FTL,
        "it" => IT_MAIN_FTL,
        "fr" => FR_MAIN_FTL,
        "de" => DE_MAIN_FTL,
        // en and any unsupported language — use the source-of-truth
        // en-US catalog (it's also the fallback bundle).
        _ => EN_US_MAIN_FTL,
    }
}

fn build_bundle(id: &LanguageIdentifier, ftl: &str) -> FluentBundle<FluentResource> {
    let resource = FluentResource::try_new(ftl.to_string()).unwrap_or_else(|(r, errs)| {
        log_catalog_error(&format!(
            "fluent: {id} ftl has {} parse errors; loading partial",
            errs.len()
        ));
        r
    });
    let mut bundle = FluentBundle::new(vec![id.clone()]);
    // Disable Unicode bidi isolation marks (U+2068 / U+2069) around
    // interpolated values. They render as visible boxes in browsers
    // that don't fully support the chars (which is most). Re-enable
    // in v2 when we add proper bidi handling for mixed-direction
    // content per `design/i18n.md`'s bidi note.
    bundle.set_use_isolating(false);
    #[cfg(target_arch = "wasm32")]
    bundle.set_formatter(Some(|value, _| match value {
        fluent_bundle::FluentValue::Number(number) => Some(format_fluent_number(number)),
        _ => None,
    }));
    if let Err(errs) = bundle.add_resource(resource) {
        log_catalog_error(&format!("fluent: bundle errors for {id}: {errs:?}"));
    }
    bundle
}

fn log_catalog_error(message: &str) {
    #[cfg(target_arch = "wasm32")]
    web_sys::console::error_1(&message.into());
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("{message}");
}

/// Translate a message id. Wraps `i18n::translate` with macro
/// sugar for the common arg-passing case:
///
///   t!("sidebar-section-navigation")
///   t!("greeting", name = "Alice")
///   t!("unread-count", count = 5)
///
/// Args are typed via `fluent_bundle::FluentValue`'s `From` impls
/// — strings, numbers, and booleans all convert automatically.
#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::i18n::translate($key, None)
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        let mut __args = ::fluent_bundle::FluentArgs::new();
        $(__args.set(stringify!($name), $value);)+
        $crate::i18n::translate($key, Some(&__args))
    }};
}

// ─── Locale-aware formatting (M-P2 piece 5) ──────────────────────
//
// Dates, numbers, and relative times use the browser's native Intl
// API rather than a bundled-in ICU Rust crate. The decision +
// rationale lives in `design/i18n.md` §Locale-aware formatting; the
// short version is: every modern browser ships a full CLDR-backed
// ICU stack as `Intl`, so we pay zero WASM bundle cost vs. ~MB+ for
// the equivalent icu_* crate set, and every locale Just Works
// without us shipping data files for each.
//
// js_sys exposes Intl.NumberFormat and Intl.DateTimeFormat, but its
// bindings expose `format` as a getter returning a bound JS Function
// — accurate to the spec but awkward at call sites. The extern
// blocks below flatten `format` to a direct method call, and add a
// binding for Intl.RelativeTimeFormat (which js_sys doesn't bind at
// all). RelativeTimeFormat has been baseline-supported in every
// shipping browser since 2020; no polyfill needed.

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = Intl, js_name = NumberFormat)]
    type IntlNumberFormat;
    #[wasm_bindgen(constructor, js_namespace = Intl, js_class = "NumberFormat", catch)]
    fn new(locale: &str, options: &Object) -> Result<IntlNumberFormat, JsValue>;
    #[wasm_bindgen(method, structural)]
    fn format(this: &IntlNumberFormat, value: f64) -> String;
}

// Shared binding for `Intl.DateTimeFormat`. Also exposes
// `formatToParts` used by `editor/blocks/calendar.rs` for
// tz-aware date extraction. Kept as `pub(crate)` so the
// calendar block reuses THIS binding rather than declaring
// a second extern type against the same JS class — LTO=fat
// merges duplicate JS-target bindings and rejects the shared
// describe symbol as multiply defined (deploy would fail
// with `__wbindgen_describe___wbg_formatToParts_… symbol
// multiply defined`).
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = Intl, js_name = DateTimeFormat)]
    pub type IntlDateTimeFormat;
    #[wasm_bindgen(constructor, js_namespace = Intl, js_class = "DateTimeFormat", catch)]
    pub fn new(locale: &str, options: &Object) -> Result<IntlDateTimeFormat, wasm_bindgen::JsValue>;
    #[wasm_bindgen(method, structural)]
    pub fn format(this: &IntlDateTimeFormat, date: &Date) -> String;
    #[wasm_bindgen(method, structural, js_name = formatToParts)]
    pub fn format_to_parts(this: &IntlDateTimeFormat, date: &Date) -> js_sys::Array;
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = Intl, js_name = RelativeTimeFormat)]
    type IntlRelativeTimeFormat;
    #[wasm_bindgen(constructor, js_namespace = Intl, js_class = "RelativeTimeFormat")]
    fn new(locale: &str, options: &Object) -> IntlRelativeTimeFormat;
    #[wasm_bindgen(method, structural)]
    fn format(this: &IntlRelativeTimeFormat, value: f64, unit: &str) -> String;
}

/// Date format style. Curated subset of Intl.DateTimeFormat option
/// presets — the three shapes the codebase actually needs. Adding
/// a fourth presets here is cheap; spreading bespoke option Objects
/// across call sites is not.
#[derive(Clone, Copy)]
pub enum DateStyle {
    /// Compact numeric date — locale-default short form.
    /// en-US: "5/19/26".  ar: "١٩‏/٥‏/٢٠٢٦".
    Short,
    /// Month-name + day + year.
    /// en-US: "May 19, 2026".  ar: "١٩ مايو ٢٠٢٦".
    Medium,
    /// Medium + hour:minute (no seconds, no timezone — admin /
    /// audit surfaces want at-a-glance recency, not forensic
    /// precision).
    /// en-US: "May 19, 2026, 2:35 PM".
    Long,
}

/// Format a microsecond timestamp as an absolute date using
/// Intl.DateTimeFormat with the active locale. Returns "—" for
/// zero/negative timestamps (UI placeholder; never panics).
pub fn format_date(timestamp_usec: i64, style: DateStyle) -> String {
    if timestamp_usec <= 0 {
        return "—".to_string();
    }
    let ms = (timestamp_usec / 1000) as f64;
    let date = Date::new(&JsValue::from_f64(ms));
    let opts = date_style_options(style);
    let locale = current_locale_or_default();
    let fmt = match IntlDateTimeFormat::new(&locale, &opts) {
        Ok(f) => f,
        Err(_) => return "—".to_string(),
    };
    fmt.format(&date)
}

fn date_style_options(style: DateStyle) -> Object {
    let opts = Object::new();
    let set = |key: &str, value: &str| {
        let _ = Reflect::set(&opts, &JsValue::from_str(key), &JsValue::from_str(value));
    };
    match style {
        DateStyle::Short => {
            set("year", "2-digit");
            set("month", "numeric");
            set("day", "numeric");
        }
        DateStyle::Medium => {
            set("year", "numeric");
            set("month", "short");
            set("day", "numeric");
        }
        DateStyle::Long => {
            set("year", "numeric");
            set("month", "short");
            set("day", "numeric");
            set("hour", "numeric");
            set("minute", "2-digit");
        }
    }
    opts
}

/// Format a number with locale-aware grouping separators and
/// digit shapes.
/// en-US: "1,234,567".  de: "1.234.567".  ar: "١٬٢٣٤٬٥٦٧".
pub fn format_number(value: f64) -> String {
    format_number_with_precision(value, 3)
}

/// Locale-aware numbers with a surface-specific fractional precision limit.
/// Unsupported precision falls back to the numeric string without panicking.
pub fn format_number_with_precision(value: f64, max_fractional_digits: u8) -> String {
    let locale = current_locale_or_default();
    let opts = Object::new();
    let _ = Reflect::set(
        &opts,
        &"maximumFractionDigits".into(),
        &JsValue::from(max_fractional_digits),
    );
    IntlNumberFormat::new(&locale, &opts)
        .map_or_else(|_| value.to_string(), |fmt| fmt.format(value))
}

/// Preserve Fluent's number options and let Intl validate their combinations.
/// Invalid options use the locale's default number format instead.
#[cfg(target_arch = "wasm32")]
fn format_fluent_number(number: &fluent_bundle::types::FluentNumber) -> String {
    use fluent_bundle::types::{FluentNumberCurrencyDisplayStyle, FluentNumberStyle};

    let opts = Object::new();
    let set = |key: &str, value: JsValue| {
        let _ = Reflect::set(&opts, &JsValue::from_str(key), &value);
    };
    let options = &number.options;
    set(
        "style",
        JsValue::from_str(match options.style {
            FluentNumberStyle::Decimal => "decimal",
            FluentNumberStyle::Currency => "currency",
            FluentNumberStyle::Percent => "percent",
        }),
    );
    if let Some(currency) = options.currency.as_deref() {
        set("currency", JsValue::from_str(currency));
    }
    set(
        "currencyDisplay",
        JsValue::from_str(match options.currency_display {
            FluentNumberCurrencyDisplayStyle::Symbol => "symbol",
            FluentNumberCurrencyDisplayStyle::Code => "code",
            FluentNumberCurrencyDisplayStyle::Name => "name",
        }),
    );
    set("useGrouping", JsValue::from_bool(options.use_grouping));
    for (key, digits) in [
        ("minimumIntegerDigits", options.minimum_integer_digits),
        ("minimumFractionDigits", options.minimum_fraction_digits),
        ("maximumFractionDigits", options.maximum_fraction_digits),
        ("minimumSignificantDigits", options.minimum_significant_digits),
        ("maximumSignificantDigits", options.maximum_significant_digits),
    ] {
        if let Some(digits) = digits {
            set(key, JsValue::from_f64(digits as f64));
        }
    }
    let locale = current_locale_or_default();
    IntlNumberFormat::new(&locale, &opts)
        .or_else(|_| IntlNumberFormat::new(&locale, &Object::new()))
        .map_or_else(|_| number.value.to_string(), |fmt| fmt.format(number.value))
}

/// Format a microsecond timestamp as relative time when recent
/// (< 1 day), as a short day-name within a week, and as an
/// absolute date otherwise. Uses Intl.RelativeTimeFormat +
/// Intl.DateTimeFormat with the active locale.
///
/// Tiered presentation matches activity-feed UX in every editor
/// the codebase targets — recent edits feel immediate, week-old
/// edits read as a day of the week, older edits commit to a
/// concrete date. The thresholds are deliberately wide (60s / 1h /
/// 1d / 7d) — drift across the boundary by a few seconds isn't
/// visible to users and we don't ship sub-minute precision UI.
///
/// Returns "—" for zero/negative timestamps; never panics.
pub fn format_relative(timestamp_usec: i64) -> String {
    if timestamp_usec <= 0 {
        return "—".to_string();
    }
    let now_ms = Date::now() as i64;
    let ts_ms = timestamp_usec / 1000;
    let diff_secs = (now_ms - ts_ms) / 1000;
    let locale = current_locale_or_default();

    if diff_secs < 0 {
        // Clock skew, future-dated timestamp. Show the absolute
        // date rather than a misleading "in N minutes" — server
        // clock disagreements should not surface as predictions.
        return format_date(timestamp_usec, DateStyle::Medium);
    }

    if diff_secs < 60 {
        // "now" in the active locale. `numeric: "auto"` is what
        // tells RelativeTimeFormat to use the locale's word for
        // "now" instead of literally "0 seconds ago".
        return relative_format(&locale, 0.0, "second");
    }
    if diff_secs < 3600 {
        return relative_format(&locale, -((diff_secs / 60) as f64), "minute");
    }
    if diff_secs < 86_400 {
        return relative_format(&locale, -((diff_secs / 3600) as f64), "hour");
    }
    // Past 1 day: switch from relative to absolute. Within a week,
    // a short day-name reads better than "3 days ago"; older than
    // a week, commit to a date.
    if diff_secs < 604_800 {
        let ms = ts_ms as f64;
        let date = Date::new(&JsValue::from_f64(ms));
        let opts = Object::new();
        let _ = Reflect::set(&opts, &JsValue::from_str("weekday"), &JsValue::from_str("short"));
        return match IntlDateTimeFormat::new(&locale, &opts) {
            Ok(fmt) => fmt.format(&date),
            Err(_) => "—".to_string(),
        };
    }
    format_date(timestamp_usec, DateStyle::Medium)
}

fn relative_format(locale: &str, value: f64, unit: &str) -> String {
    let opts = Object::new();
    let _ = Reflect::set(&opts, &JsValue::from_str("numeric"), &JsValue::from_str("auto"));
    let fmt = IntlRelativeTimeFormat::new(locale, &opts);
    fmt.format(value, unit)
}

fn current_locale_or_default() -> String {
    ACTIVE_LOCALE_TAG.with(|cell| {
        let borrowed = cell.borrow();
        if borrowed.is_empty() {
            "en-US".to_string()
        } else {
            borrowed.clone()
        }
    })
}

/// The BCP-47 tag of the currently-active locale — what `init` /
/// `set_locale` last applied (the locale the user is actually seeing).
/// UI that needs to reflect the active locale (e.g. the locale
/// switcher's initial `<select>` value) should read THIS rather than
/// re-running `resolve_locale`, which recomputes precedence and can
/// disagree with the applied server-pref hint on a fresh device.
pub fn active_locale() -> String {
    current_locale_or_default()
}

#[cfg(test)]
mod catalog_tests;
