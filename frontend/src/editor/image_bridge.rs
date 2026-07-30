// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Resolves `ogre-blob:` image references (see [`super::blob_ref`]) to
//! fresh presigned download URLs at render time.
//!
//! `editor::view` renders inside both the wasm-pack-tested `lib` crate
//! target and the app `bin` target (see the `caret_rect_for_pos` doc
//! comment in `editor/view.rs` for the same split); in the `lib` target
//! `crate::api` isn't visible, so `editor::view` can't call
//! `api::blobs::request_download_url` directly. This mirrors the
//! `nav_bridge` pattern: the document page installs a resolver closure
//! here on mount (closing over its `doc_id` and the bin-only
//! `api::blobs` call and async executor); `editor::view` calls
//! [`resolve`] for every `ogre-blob:` src it renders and never needs to
//! know how the fetch happens.
//!
//! Resolved URLs are cached here, keyed by `(blob_id, key)` — a
//! document with N images issues at most N presigned-URL requests, no
//! matter how many times the editor re-renders (it does a full DOM
//! rebuild on every dispatched transaction) while those N resolve.
//!
//! The cache is time-bounded, not page-lifetime: see
//! [`CACHE_FRESH_MS`].
//!
//! ## Known hazards (deliberately not defended against here)
//!
//! Two failure modes are latent in the `WAITERS` bookkeeping and are
//! documented rather than fixed, since neither is reachable from the
//! resolver the document page actually installs:
//!
//! - A resolver that never invokes its completion callback leaves a
//!   `WAITERS` entry in place forever, and every later [`resolve`] for
//!   that key sees "already in flight" and does nothing. Recovery
//!   today is a remount (`clear_resolver_if` clears the maps).
//! - While a fetch is in flight, each re-render pushes another waiter
//!   onto that key's list without bound. The list drains on
//!   completion, so this is bounded by the fetch's duration, but a
//!   very slow fetch in a document being typed into will accumulate
//!   closures.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// Fetches a fresh presigned URL for `(blob_id, key)` and reports the
/// result (`None` on failure) via the completion callback. Boxed rather
/// than typed as a concrete future so this module has no dependency on
/// any async runtime — the installer (the document page) owns that.
pub type Resolver = Rc<dyn Fn(String, String, Box<dyn FnOnce(Option<String>)>)>;

type CacheKey = (String, String);
/// Callbacks awaiting an in-flight resolution for one key.
type WaiterList = Vec<Box<dyn FnOnce(String)>>;

/// A resolved presigned URL plus the wall-clock instant it was minted.
struct CacheEntry {
    url: String,
    resolved_at_ms: f64,
}

/// How long a cached presigned URL is treated as usable — half the
/// server's 14400s (4-hour) presign TTL.
///
/// The cache must expire, not just fill: `EditorView::render()` rebuilds
/// the whole DOM on every dispatched transaction, so a tab left open
/// past the presign TTL would otherwise re-render every `<img>` with a
/// URL that 403s on the next keystroke — the original
/// image-breaks-after-4-hours symptom, merely re-keyed from document age
/// to session length. Half the TTL leaves a wide margin for clock skew
/// and for the interval between resolving a URL and the browser actually
/// requesting it.
const CACHE_FRESH_MS: f64 = 7_200_000.0;

/// Wall clock in Unix milliseconds.
///
/// This module can't call `api::client::now_ms` — `crate::api` isn't
/// part of the lib target (see the module doc) — so it mirrors that
/// function's cfg split instead: a real clock under `wasm32`, and 0.0
/// natively, where there's no `Date` and the cache never runs for real
/// anyway.
///
/// Unit tests (on either architecture — the lib's `#[cfg(test)]` module
/// compiles under `wasm-pack test` too) read a settable `TEST_NOW_MS`
/// instead, so the freshness window is testable without waiting two
/// hours. Integration tests in `frontend/tests/` link the lib built
/// *without* `cfg(test)` and so get the real clock.
fn now_ms() -> f64 {
    #[cfg(test)]
    {
        TEST_NOW_MS.with(|c| c.get())
    }
    #[cfg(all(not(test), target_arch = "wasm32"))]
    {
        js_sys::Date::now()
    }
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        0.0
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only clock, in Unix milliseconds. See `now_ms`.
    static TEST_NOW_MS: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// The cached URL for `cache_key` if one is present *and* still within
/// [`CACHE_FRESH_MS`]. A stale entry is evicted on the way out, so the
/// caller's next step (a fetch, for [`resolve`]) starts from a clean
/// slate.
fn take_fresh_cached(cache_key: &CacheKey) -> Option<String> {
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        let entry = cache.get(cache_key)?;
        if now_ms() - entry.resolved_at_ms < CACHE_FRESH_MS {
            return Some(entry.url.clone());
        }
        cache.remove(cache_key);
        None
    })
}

/// How many further `resolve()` calls for a failed key are answered from
/// the negative cache (no fetch) before the next call is allowed to retry.
/// `EditorView::render()` fully rebuilds the DOM on every dispatched
/// transaction, so a `resolve()` call for a broken image happens once per
/// keystroke anywhere in the document — this bounds a permanently-broken
/// blob (deleted, revoked, offline) to one real request per ~20
/// keystrokes instead of one per keystroke, without needing a wall-clock
/// (`Date.now()`, unavailable/stubbed-to-0 in native tests — see
/// `api::client::now_ms` for the same split) to express "a short while."
const FAILURE_BACKOFF_CALLS: u32 = 20;

thread_local! {
    static RESOLVER: RefCell<Option<Resolver>> = const { RefCell::new(None) };
    static CACHE: RefCell<HashMap<CacheKey, CacheEntry>> = RefCell::new(HashMap::new());
    // A non-empty entry means a fetch is already in flight for that key —
    // a second `resolve()` call for the same key while one is pending
    // queues its callback here instead of firing a second request.
    static WAITERS: RefCell<HashMap<CacheKey, WaiterList>> = RefCell::new(HashMap::new());
    // Keys whose most recent fetch failed, and how many more `resolve()`
    // calls to answer with `None` (no fetch) before retrying. See
    // `FAILURE_BACKOFF_CALLS`.
    static FAILED: RefCell<HashMap<CacheKey, u32>> = RefCell::new(HashMap::new());
}

/// Install (or clear) the resolver. The document page calls this on mount
/// (closing over the current `doc_id`) and clears it (`None`) on
/// cleanup, mirroring `nav_bridge::set_navigate`.
pub fn set_resolver(resolver: Option<Resolver>) {
    RESOLVER.with(|cell| *cell.borrow_mut() = resolver);
}

/// Clear the resolver — but only if the currently-installed one is
/// (pointer-)identical to `expected`. Also clears `CACHE`/`WAITERS`/
/// `FAILED` when it does.
///
/// Guards a remount race: `EditorComponent` remounts on document switch
/// (`pages/document.rs`), and if the new mount's `set_resolver` call runs
/// *before* the old mount's `on_cleanup` fires, an unconditional
/// `set_resolver(None)` in that stale cleanup would wipe the new mount's
/// resolver — every image in the new document would stay `src`-less
/// forever (no reload-free recovery, since nothing ever calls
/// `set_resolver(Some(_))` again until the next remount). Comparing
/// identity before clearing means a superseded cleanup is a no-op instead
/// of a wipe. When identity DOES match (the normal single-document-open
/// case), also clearing the cache/waiter/failure state prevents a
/// dropped in-flight fetch task from leaving a `WAITERS` entry that can
/// never be removed — which would wedge every future `resolve()` for
/// that `(blob_id, key)`, forever, since `resolve()` treats any non-empty
/// `WAITERS` entry as "already in flight."
pub fn clear_resolver_if(expected: &Resolver) {
    let matched = RESOLVER.with(|r| {
        let mut r = r.borrow_mut();
        let is_match = r.as_ref().is_some_and(|cur| Rc::ptr_eq(cur, expected));
        if is_match {
            *r = None;
        }
        is_match
    });
    if matched {
        CACHE.with(|c| c.borrow_mut().clear());
        WAITERS.with(|w| w.borrow_mut().clear());
        FAILED.with(|f| f.borrow_mut().clear());
    }
}

/// Synchronous cache peek: the resolved URL for `(blob_id, key)` if one
/// is cached and still fresh ([`CACHE_FRESH_MS`]), without triggering a
/// fetch or touching `WAITERS`/`FAILED`. Used by the clipboard HTML
/// serializer (`clipboard.rs::element_tags`), which runs synchronously
/// inside a `copy`/`cut` DOM event handler and can't await a fetch —
/// since the image was just rendered, this is expected to be a hit.
///
/// Applies the same freshness window as [`resolve`] rather than a
/// looser one: handing a stale URL to the clipboard would put a link
/// that's about to 403 into an external paste. On a miss the serializer
/// falls back to emitting the raw reference, and the durable
/// `data-ogre-blob` attribute it emits either way keeps an in-app paste
/// lossless.
pub fn peek(blob_id: &str, key: &str) -> Option<String> {
    take_fresh_cached(&(blob_id.to_string(), key.to_string()))
}

/// Resolve `(blob_id, key)` to a presigned download URL.
///
/// - Fresh cache hit: returns `Some(url)` synchronously; `on_ready` is
///   not called. An entry older than [`CACHE_FRESH_MS`] is evicted and
///   treated as a miss, so a long-lived tab re-resolves rather than
///   re-rendering an expired URL.
/// - Recently failed (within `FAILURE_BACKOFF_CALLS` calls of its last
///   failure): returns `None` immediately without fetching — see
///   `FAILURE_BACKOFF_CALLS`.
/// - Cache miss: returns `None` immediately. Unless a resolution for this
///   key is already in flight, kicks off the installed resolver's async
///   fetch. `on_ready` runs exactly once, later, when a fetch this call
///   triggered-or-joined succeeds. On failure (or no resolver installed)
///   `on_ready` never runs — the caller's `<img>` is left without a
///   `src`, which reads as a broken image rather than a wrong one — and
///   the key enters the failure backoff above.
pub fn resolve(blob_id: &str, key: &str, on_ready: impl FnOnce(String) + 'static) -> Option<String> {
    let cache_key: CacheKey = (blob_id.to_string(), key.to_string());

    if let Some(url) = take_fresh_cached(&cache_key) {
        return Some(url);
    }

    let still_backing_off = FAILED.with(|f| {
        let mut f = f.borrow_mut();
        match f.get_mut(&cache_key) {
            Some(remaining) if *remaining > 0 => {
                *remaining -= 1;
                true
            }
            Some(_) => {
                f.remove(&cache_key);
                false
            }
            None => false,
        }
    });
    if still_backing_off {
        return None;
    }

    let already_in_flight = WAITERS.with(|w| {
        let mut w = w.borrow_mut();
        let entry = w.entry(cache_key.clone()).or_default();
        let was_idle = entry.is_empty();
        entry.push(Box::new(on_ready));
        !was_idle
    });
    if already_in_flight {
        return None;
    }

    let Some(resolver) = RESOLVER.with(|r| r.borrow().clone()) else {
        // No resolver installed — shouldn't happen once the document page
        // has mounted, but don't leave a wedged waiter list.
        WAITERS.with(|w| {
            w.borrow_mut().remove(&cache_key);
        });
        return None;
    };

    let done_key = cache_key.clone();
    resolver(
        blob_id.to_string(),
        key.to_string(),
        Box::new(move |result| {
            let waiters = WAITERS
                .with(|w| w.borrow_mut().remove(&done_key))
                .unwrap_or_default();
            if let Some(url) = result {
                CACHE.with(|c| {
                    c.borrow_mut().insert(
                        done_key.clone(),
                        CacheEntry {
                            url: url.clone(),
                            resolved_at_ms: now_ms(),
                        },
                    );
                });
                for waiter in waiters {
                    waiter(url.clone());
                }
            } else {
                // Waiters are dropped: their elements stay without a
                // `src`, matching a broken image rather than a wrong
                // one. Not cached (so a later, backed-off retry can
                // succeed once the underlying problem clears), but
                // negative-cached via `FAILED` so a permanently-broken
                // blob doesn't cost one request per keystroke — see
                // `FAILURE_BACKOFF_CALLS`.
                FAILED.with(|f| {
                    f.borrow_mut().insert(done_key.clone(), FAILURE_BACKOFF_CALLS);
                });
            }
        }),
    );

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn reset() {
        set_resolver(None);
        CACHE.with(|c| c.borrow_mut().clear());
        WAITERS.with(|w| w.borrow_mut().clear());
        FAILED.with(|f| f.borrow_mut().clear());
        set_test_now_ms(0.0);
    }

    /// Move the test clock `now_ms` reads. See `now_ms`'s cfg split.
    fn set_test_now_ms(ms: f64) {
        TEST_NOW_MS.with(|c| c.set(ms));
    }

    #[test]
    fn cache_miss_without_resolver_returns_none_and_does_not_wedge() {
        reset();
        let ready_calls = Rc::new(Cell::new(0));
        let ready_calls_cb = Rc::clone(&ready_calls);
        let result = resolve("b1", "k1", move |_| {
            ready_calls_cb.set(ready_calls_cb.get() + 1);
        });
        assert_eq!(result, None);
        assert_eq!(ready_calls.get(), 0);
        // A second call for the same key doesn't hang waiting on a fetch
        // that will never start.
        let result2 = resolve("b1", "k1", |_| {});
        assert_eq!(result2, None);
        reset();
    }

    /// Two `<img>` elements referencing the identical `(blob_id, key)` —
    /// e.g. the same image pasted twice — must trigger exactly one fetch,
    /// and both must be notified once it resolves. The resolver here
    /// defers its callback (stores it instead of invoking it immediately)
    /// so both `resolve()` calls land while the fetch is genuinely still
    /// in flight, exercising the `WAITERS` queue rather than a cache hit.
    #[test]
    fn resolver_fires_once_for_duplicate_keys_in_flight_and_notifies_all_waiters() {
        reset();
        let fetch_count = Rc::new(Cell::new(0));
        let pending_cb: Rc<RefCell<Option<Box<dyn FnOnce(Option<String>)>>>> =
            Rc::new(RefCell::new(None));
        let fetch_count_r = Rc::clone(&fetch_count);
        let pending_cb_r = Rc::clone(&pending_cb);
        set_resolver(Some(Rc::new(move |_blob_id: String, _key: String, cb| {
            fetch_count_r.set(fetch_count_r.get() + 1);
            *pending_cb_r.borrow_mut() = Some(cb);
        })));

        let notified = Rc::new(RefCell::new(Vec::new()));
        let n1 = Rc::clone(&notified);
        let n2 = Rc::clone(&notified);

        let r1 = resolve("b1", "k1", move |url| n1.borrow_mut().push(url));
        let r2 = resolve("b1", "k1", move |url| n2.borrow_mut().push(url));
        assert_eq!(r1, None);
        assert_eq!(r2, None, "second caller must queue, not double-fetch");
        assert_eq!(fetch_count.get(), 1, "duplicate keys must not double-fetch");
        assert!(notified.borrow().is_empty(), "fetch hasn't resolved yet");

        // Now let the (only) in-flight fetch complete.
        let cb = pending_cb.borrow_mut().take().expect("fetch was started");
        cb(Some("https://example.com/b1/k1?sig=1".to_string()));

        assert_eq!(notified.borrow().len(), 2, "both waiters must be notified");
        assert!(notified.borrow().iter().all(|u| u == "https://example.com/b1/k1?sig=1"));
        assert_eq!(
            resolve("b1", "k1", |_| panic!("on_ready must not run on a cache hit")),
            Some("https://example.com/b1/k1?sig=1".to_string()),
            "resolved URL must now be cached"
        );
        reset();
    }

    #[test]
    fn resolved_url_is_cached_for_subsequent_calls() {
        reset();
        set_resolver(Some(Rc::new(|blob_id: String, key: String, cb| {
            cb(Some(format!("https://example.com/{blob_id}/{key}")));
        })));

        let first = resolve("b2", "k2", |_| {});
        assert_eq!(first, None, "first call must go through the resolver");

        let second = resolve("b2", "k2", |_| {
            panic!("on_ready must not run on a cache hit");
        });
        assert_eq!(second, Some("https://example.com/b2/k2".to_string()));
        reset();
    }

    /// A permanently-broken blob (resolver always fails) must not cost one
    /// fetch per `resolve()` call — `EditorView::render()` rebuilds the
    /// whole DOM on every dispatched transaction, so an unbounded retry
    /// here would mean one request per keystroke, forever. Simulates
    /// `FAILURE_BACKOFF_CALLS + few more` re-renders and asserts the
    /// fetch count stops climbing once backoff kicks in, then resumes
    /// after the backoff window elapses.
    #[test]
    fn failed_resolution_is_negative_cached_and_does_not_storm_requests() {
        reset();
        let fetch_count = Rc::new(Cell::new(0));
        let fetch_count_r = Rc::clone(&fetch_count);
        set_resolver(Some(Rc::new(move |_blob_id: String, _key: String, cb| {
            fetch_count_r.set(fetch_count_r.get() + 1);
            cb(None); // simulates a 403/deleted blob/offline — every time
        })));

        // First call: cache miss, no backoff yet -> fetches (and fails).
        assert_eq!(resolve("broken", "k", |_| {}), None);
        assert_eq!(fetch_count.get(), 1);

        // Next FAILURE_BACKOFF_CALLS re-renders (e.g. keystrokes) must not
        // issue a single further request.
        for _ in 0..FAILURE_BACKOFF_CALLS {
            assert_eq!(resolve("broken", "k", |_| {}), None);
        }
        assert_eq!(
            fetch_count.get(),
            1,
            "backoff window must fully suppress re-fetching"
        );

        // Backoff window has now elapsed (this resolve call both observes
        // the expired backoff and starts the next attempt).
        assert_eq!(resolve("broken", "k", |_| {}), None);
        assert_eq!(fetch_count.get(), 2, "must retry after backoff elapses");
        reset();
    }

    /// Regression: a tab left open past the presigned URL's TTL must
    /// re-resolve, not keep serving the stale URL forever.
    ///
    /// `EditorView::render()` rebuilds the DOM on every dispatched
    /// transaction, so an unconditional cache hit meant the next
    /// keystroke after ~4 hours re-rendered every `<img>` with a URL
    /// that 403s — the original "images break after 4 hours" symptom,
    /// re-keyed from document age to session length.
    #[test]
    fn stale_cache_entry_is_refetched_after_the_freshness_window() {
        reset();
        let fetch_count = Rc::new(Cell::new(0));
        let fetch_count_r = Rc::clone(&fetch_count);
        set_resolver(Some(Rc::new(move |blob_id: String, key: String, cb| {
            let n = fetch_count_r.get() + 1;
            fetch_count_r.set(n);
            cb(Some(format!("https://example.com/{blob_id}/{key}?sig={n}")));
        })));

        assert_eq!(resolve("b5", "k5", |_| {}), None, "first call fetches");
        assert_eq!(fetch_count.get(), 1);
        let first_url = "https://example.com/b5/k5?sig=1".to_string();
        assert_eq!(resolve("b5", "k5", |_| {}), Some(first_url.clone()));
        assert_eq!(fetch_count.get(), 1, "fresh entry must be served from cache");

        // Just inside the window: still a hit.
        set_test_now_ms(CACHE_FRESH_MS - 1.0);
        assert_eq!(resolve("b5", "k5", |_| {}), Some(first_url.clone()));
        assert_eq!(peek("b5", "k5"), Some(first_url));
        assert_eq!(fetch_count.get(), 1, "must not refetch inside the window");

        // Past the window: the entry is stale, so this call refetches.
        // The refetching call itself returns `None` (the fetch is only
        // synchronous because this test's resolver is), and the *next*
        // call sees the freshly-minted URL.
        set_test_now_ms(CACHE_FRESH_MS + 1.0);
        assert_eq!(resolve("b5", "k5", |_| {}), None, "stale entry is a miss");
        assert_eq!(fetch_count.get(), 2, "stale entry must trigger a refetch");
        assert_eq!(
            resolve("b5", "k5", |_| {}),
            Some("https://example.com/b5/k5?sig=2".to_string()),
            "the refreshed URL must now be cached"
        );
        assert_eq!(fetch_count.get(), 2);
        reset();
    }

    /// `peek` applies the same freshness window as `resolve` — the
    /// clipboard serializer must never put an about-to-403 URL into an
    /// external paste.
    #[test]
    fn peek_treats_a_stale_entry_as_a_miss() {
        reset();
        set_resolver(Some(Rc::new(|blob_id: String, key: String, cb| {
            cb(Some(format!("https://example.com/{blob_id}/{key}")));
        })));
        assert_eq!(resolve("b6", "k6", |_| {}), None);
        assert_eq!(peek("b6", "k6"), Some("https://example.com/b6/k6".to_string()));

        set_test_now_ms(CACHE_FRESH_MS + 1.0);
        assert_eq!(peek("b6", "k6"), None, "stale entry must not be peeked");
        reset();
    }

    #[test]
    fn peek_returns_none_before_resolution_and_the_url_after() {
        reset();
        set_resolver(Some(Rc::new(|blob_id: String, key: String, cb| {
            cb(Some(format!("https://example.com/{blob_id}/{key}")));
        })));

        assert_eq!(peek("b3", "k3"), None, "not fetched yet");
        assert_eq!(resolve("b3", "k3", |_| {}), None);
        assert_eq!(
            peek("b3", "k3"),
            Some("https://example.com/b3/k3".to_string())
        );
        reset();
    }

    /// A stale mount's `on_cleanup` must not wipe a newer mount's
    /// resolver — the remount-race the doc comment on `clear_resolver_if`
    /// describes.
    #[test]
    fn clear_resolver_if_is_a_noop_when_superseded() {
        reset();
        let old: Resolver = Rc::new(|_: String, _: String, cb| cb(None));
        let new: Resolver = Rc::new(|_: String, _: String, cb| cb(None));

        set_resolver(Some(Rc::clone(&old)));
        // A newer mount installs its own resolver before the old mount's
        // cleanup runs.
        set_resolver(Some(Rc::clone(&new)));

        clear_resolver_if(&old);

        // The new resolver must still be installed and usable.
        let called = Rc::new(Cell::new(false));
        let called_cb = Rc::clone(&called);
        RESOLVER.with(|r| {
            r.borrow().as_ref().unwrap()(
                "b".to_string(),
                "k".to_string(),
                Box::new(move |_| called_cb.set(true)),
            );
        });
        assert!(called.get(), "new resolver must not have been cleared");
        reset();
    }

    #[test]
    fn clear_resolver_if_clears_state_when_identity_matches() {
        reset();
        set_resolver(Some(Rc::new(|blob_id: String, key: String, cb| {
            cb(Some(format!("https://example.com/{blob_id}/{key}")));
        })));
        // Warm the cache.
        assert_eq!(resolve("b4", "k4", |_| {}), None);
        assert_eq!(peek("b4", "k4"), Some("https://example.com/b4/k4".to_string()));

        let installed = RESOLVER.with(|r| r.borrow().clone().unwrap());
        clear_resolver_if(&installed);

        assert_eq!(peek("b4", "k4"), None, "cache must be cleared");
        assert!(
            RESOLVER.with(|r| r.borrow().is_none()),
            "resolver must be cleared"
        );
        reset();
    }
}
