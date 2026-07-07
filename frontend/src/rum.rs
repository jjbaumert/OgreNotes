// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P9 piece C — frontend RUM (real-user monitoring)
//! sampler.
//!
//! Captures Largest-Contentful-Paint, First-Contentful-Paint, and
//! navigation timing for a sampled subset of sessions, then POSTs
//! one beacon to `/api/v1/metrics/rum`. The backend (route module
//! `crates/api/src/routes/metrics.rs`) folds each provided vital
//! into a histogram dimensioned by `(page, ua_class)`; the EMF
//! emitter ships those into the existing `OgreNotes` CloudWatch
//! namespace.
//!
//! Scope (v1): LCP + FCP + nav timing on a single beacon ~1.5 s
//! after the window `load` event. CLS and INP require ongoing
//! observation throughout the session — left as v2 work, no
//! infrastructure change required to add them later (the route
//! already accepts the fields).
//!
//! Sampling: 10% session-level. The decision is made once on
//! `init()`; either every vital from this session is sent or none
//! is. A coin-flip per-event would lose the ability to scatter-
//! plot the same session's vitals across the dashboard.
//!
//! Page classification: derived from the URL — `/` → home,
//! `/d/...` → editor, everything else → other. Distinguishing
//! editor vs spreadsheet from the URL alone is impossible; v2
//! can grow an explicit `set_page_kind` API the spreadsheet page
//! calls once it's mounted.

use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::client;

/// Fraction of sessions to sample, in [0.0, 1.0]. Hardcoded for
/// v1 — a runtime knob would mean either a server round-trip
/// before observations can begin (defeating the point of RUM) or a
/// build-time `env!` macro that requires rebuilding the WASM to
/// re-tune. Either is overkill until a tuning signal demands it.
const SAMPLE_RATE: f64 = 0.10;

/// Delay between the window `load` event and the beacon POST. Long
/// enough that the largest-contentful-paint observer has settled
/// for the typical page; short enough that the user hasn't moved on
/// before we report.
const BEACON_DELAY_MS: i32 = 1500;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RumPayload {
    page: &'static str,
    vitals: RumVitals,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RumVitals {
    #[serde(skip_serializing_if = "Option::is_none")]
    lcp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fcp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nav_dcl: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nav_load: Option<f64>,
}

/// Install the RUM sampler. Called once from `main.rs` after
/// authentication hydration so the beacon's auth header is valid.
/// No-op for unsampled sessions and in non-browser contexts.
pub fn init() {
    if !sample_in() {
        return;
    }
    let Some(window) = web_sys::window() else { return };

    // Listener fires once on the window `load` event; the
    // schedule_beacon callback then sets a timeout to capture
    // vitals after the page settles.
    let listener = Closure::wrap(Box::new(move |_evt: web_sys::Event| {
        schedule_beacon();
    }) as Box<dyn FnMut(_)>);
    let _ = window.add_event_listener_with_callback("load", listener.as_ref().unchecked_ref());
    listener.forget();
}

/// Coin flip for sample-in. `js_sys::Math::random()` returns a
/// uniform [0, 1) draw — `< SAMPLE_RATE` is the inclusion
/// criterion. Pulled into its own function so unit-style tests can
/// stub it later if we ever need them.
fn sample_in() -> bool {
    js_sys::Math::random() < SAMPLE_RATE
}

/// Schedule the vital-capture + POST. Reads `performance.*` after
/// `BEACON_DELAY_MS` so the largest-contentful-paint observer has
/// reported its final entry for the typical page.
fn schedule_beacon() {
    let Some(window) = web_sys::window() else { return };
    let cb = Closure::wrap(Box::new(move || {
        let payload = snapshot_payload();
        wasm_bindgen_futures::spawn_local(async move {
            post_beacon(payload).await;
        });
    }) as Box<dyn FnMut()>);
    let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
        cb.as_ref().unchecked_ref(),
        BEACON_DELAY_MS,
    );
    cb.forget();
}

/// Fire-and-forget POST of the beacon. Inlined here rather than
/// going through `client::api_post_empty` because that helper
/// calls `redirect_to_login()` on a 401 — appropriate for an
/// interactive request, deadly for a delayed metrics beacon that
/// would otherwise yank a user mid-session if their token
/// expired between page load and our 1.5 s capture. RUM beacons
/// are inherently best-effort: an unauthenticated, 4xx, or
/// network-error response is just one data point lost in a 10%
/// sample. Nothing observes the result.
async fn post_beacon(payload: RumPayload) {
    let Some(token) = client::get_token() else {
        return;
    };
    let Ok(body) = serde_json::to_string(&payload) else {
        return;
    };
    let req = gloo_net::http::Request::post("/api/v1/metrics/rum")
        .header("Content-Type", "application/json")
        .header("Authorization", &format!("Bearer {token}"));
    let Ok(req) = req.body(body) else { return };
    let _ = req.send().await;
}

fn snapshot_payload() -> RumPayload {
    let page = classify_page();
    let mut vitals = RumVitals::default();
    if let Some(window) = web_sys::window() {
        if let Some(perf) = window.performance() {
            vitals.lcp = latest_entry_start(&perf, "largest-contentful-paint");
            vitals.fcp = first_paint_entry(&perf, "first-contentful-paint");
            if let Some((dcl, load)) = navigation_timings(&perf) {
                vitals.nav_dcl = Some(dcl);
                vitals.nav_load = Some(load);
            }
        }
    }
    RumPayload { page, vitals }
}

/// Map the current URL path to one of the four `page` dimensions
/// the backend accepts. Anything ambiguous falls through to
/// `"other"` — keeps the dimension cardinality bounded.
fn classify_page() -> &'static str {
    let Some(window) = web_sys::window() else { return "other" };
    let Ok(path) = window.location().pathname() else {
        return "other";
    };
    if path == "/" || path.is_empty() {
        "home"
    } else if path.starts_with("/d/") {
        "editor"
    } else {
        "other"
    }
}

/// Read the most recent `entryType` performance entry's
/// `startTime` (ms). Returns `None` if no entry exists. Used for
/// LCP, which can fire multiple times as larger elements paint;
/// we want the final, largest one.
fn latest_entry_start(perf: &web_sys::Performance, entry_type: &str) -> Option<f64> {
    let entries = perf.get_entries_by_type(entry_type);
    let len = entries.length();
    if len == 0 {
        return None;
    }
    // `startTime` is on PerformanceEntry; the JS-side getter is
    // available on any subtype. Reach for the last entry and
    // pull its startTime via Reflect to avoid pulling in a
    // dedicated web-sys type for every performance subtype.
    let last = entries.get(len - 1);
    js_sys::Reflect::get(&last, &JsValue::from_str("startTime"))
        .ok()
        .and_then(|v| v.as_f64())
}

/// Look for a `paint` entry whose `name` field equals the given
/// label (e.g. `"first-contentful-paint"`) and return its
/// `startTime`. The `paint` entry-type bundles `first-paint`
/// and `first-contentful-paint`; we filter by name.
fn first_paint_entry(perf: &web_sys::Performance, name: &str) -> Option<f64> {
    let entries = perf.get_entries_by_type("paint");
    let len = entries.length();
    for i in 0..len {
        let entry = entries.get(i);
        let entry_name = js_sys::Reflect::get(&entry, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if entry_name == name {
            return js_sys::Reflect::get(&entry, &JsValue::from_str("startTime"))
                .ok()
                .and_then(|v| v.as_f64());
        }
    }
    None
}

/// Extract domContentLoadedEventEnd + loadEventEnd from the
/// navigation timing entry (relative to navigationStart, in ms).
/// Returns None if no navigation entry is available.
fn navigation_timings(perf: &web_sys::Performance) -> Option<(f64, f64)> {
    let entries = perf.get_entries_by_type("navigation");
    if entries.length() == 0 {
        return None;
    }
    let nav = entries.get(0);
    let dcl = js_sys::Reflect::get(&nav, &JsValue::from_str("domContentLoadedEventEnd"))
        .ok()
        .and_then(|v| v.as_f64())?;
    let load = js_sys::Reflect::get(&nav, &JsValue::from_str("loadEventEnd"))
        .ok()
        .and_then(|v| v.as_f64())?;
    Some((dcl, load))
}
