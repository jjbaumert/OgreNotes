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
//! Resolved URLs are cached here, keyed by `(blob_id, key)`, for the
//! page's lifetime — a document with N images issues at most N
//! presigned-URL requests total, no matter how many times the editor
//! re-renders (it does a full DOM rebuild on every dispatched
//! transaction) while those N resolve.

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

thread_local! {
    static RESOLVER: RefCell<Option<Resolver>> = const { RefCell::new(None) };
    static CACHE: RefCell<HashMap<CacheKey, String>> = RefCell::new(HashMap::new());
    // A non-empty entry means a fetch is already in flight for that key —
    // a second `resolve()` call for the same key while one is pending
    // queues its callback here instead of firing a second request.
    static WAITERS: RefCell<HashMap<CacheKey, WaiterList>> = RefCell::new(HashMap::new());
}

/// Install (or clear) the resolver. The document page calls this on mount
/// (closing over the current `doc_id`) and clears it (`None`) on
/// cleanup, mirroring `nav_bridge::set_navigate`.
pub fn set_resolver(resolver: Option<Resolver>) {
    RESOLVER.with(|cell| *cell.borrow_mut() = resolver);
}

/// Resolve `(blob_id, key)` to a presigned download URL.
///
/// - Cache hit: returns `Some(url)` synchronously; `on_ready` is not
///   called.
/// - Cache miss: returns `None` immediately. Unless a resolution for this
///   key is already in flight, kicks off the installed resolver's async
///   fetch. `on_ready` runs exactly once, later, when a fetch this call
///   triggered-or-joined succeeds. On failure (or no resolver installed)
///   `on_ready` never runs — the caller's `<img>` is left without a
///   `src`, which reads as a broken image rather than a wrong one.
pub fn resolve(blob_id: &str, key: &str, on_ready: impl FnOnce(String) + 'static) -> Option<String> {
    let cache_key: CacheKey = (blob_id.to_string(), key.to_string());

    if let Some(url) = CACHE.with(|c| c.borrow().get(&cache_key).cloned()) {
        return Some(url);
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
                    c.borrow_mut().insert(done_key.clone(), url.clone());
                });
                for waiter in waiters {
                    waiter(url.clone());
                }
            }
            // On failure the waiters are simply dropped: their elements
            // stay without a `src`, matching a broken image rather than
            // a wrong one. Deliberately not cached, so a later render
            // (e.g. after the user's session token refreshes) retries.
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
}
