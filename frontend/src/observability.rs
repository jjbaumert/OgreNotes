// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Frontend (WASM) metric surface — Phase 1 of
//! `design/observability.md`.
//!
//! The six baseline counters defined here pair with server-side
//! counters in `crates/api/src/routes/ws.rs` and
//! `crates/collab/src/{room,protocol,document}.rs`. The
//! discrepancy_map agent (Phase 3) plots each pair on the same axis;
//! the gap is the answer to most live-collab bugs. For the current
//! edits-not-persisting investigation, the meaningful ratio is
//! `client.ws.frames_sent_total` vs the server's
//! `ws.messages_total{type=update}`.
//!
//! Counter values accumulate in an in-process `HashMap` keyed by the
//! counter's static name. Phase 1 supports dimensionless counters
//! only — the Phase-2 envelope work adds the vetted dimension
//! allowlist on both sides. The map is bounded by the size of the
//! allowlist (6 entries today), so no eviction is needed.
//!
//! The periodic flush + URL-flag debug surface are wired up in the
//! task-11 follow-up; this module is the data substrate.

use std::cell::RefCell;
use std::collections::HashMap;

use serde::Serialize;

// ─── Counter name constants ─────────────────────────────────────
//
// Static strings deliberately — the server's allowlist matches by
// `&'static str` equality, and emitting at call sites with these
// constants gives the compiler one place to typo-check from. Adding
// a counter is a one-line change here + one-line update to the
// server's `CLIENT_METRIC_ALLOWLIST` in
// `crates/api/src/routes/client_telemetry.rs`.

/// Fired once per editor transaction that produced a doc change.
/// Pairs with the server's `ws.messages_total{type=update}`. Pre-WS
/// transactions count too: this is the "user typed something"
/// signal regardless of whether sync had completed.
pub const EDITOR_TRANSACTIONS: &str = "client.editor.transactions_total";

/// Fired each time the yrs `observe_update_v1` callback runs.
/// Diverges from `EDITOR_TRANSACTIONS` when a transaction produced
/// no yrs-level change (e.g. selection-only). The server doesn't
/// see these so there's no symmetric counter; the value comes from
/// the ratio with EDITOR_TRANSACTIONS — if it's far below 1, most
/// model changes aren't yielding CRDT updates.
pub const COLLAB_OBSERVE_FIRED: &str = "client.collab.observe_fired_total";

/// Fired each time the WS-send drain loop empties the pending-updates
/// buffer. Pairs with `client.ws.frames_sent_total` below — a flush
/// that runs but doesn't increment frames_sent means there was
/// nothing to send (the user's edits never reached the buffer).
pub const COLLAB_PENDING_DRAINED: &str = "client.collab.pending_updates_drained_total";

/// Fired once per outbound WS frame the client attempted to send.
/// Pairs with the server's `ws.messages_total` (summed across types).
/// The first-order discrepancy detector for the current bug class.
pub const WS_FRAMES_SENT: &str = "client.ws.frames_sent_total";

/// Fired when `WebSocket::send_with_u8_array` returns Err — the
/// client tried to send but the socket rejected it (closing /
/// CLOSED state). Pairs with the server's
/// `ws.send_errors_total{side=primary}`; a non-zero client-side
/// value means edits were generated and rejected by the socket,
/// invisible to the server.
pub const WS_SEND_ERRORS: &str = "client.ws.send_errors_total";

/// Fired once per inbound WS frame the client received. Pairs with
/// the server's broadcast metric. Lets us spot the case where the
/// server thinks it's broadcasting but the client never sees the
/// frame (e.g. browser dropped the WS).
pub const WS_REMOTE_FRAMES_RECEIVED: &str = "client.ws.remote_frames_received_total";

/// Fired once per `sync_children` call in yrs_bridge. Pairs with
/// SYNC_MODEL_BLOCKS and SYNC_MATCHED_BLOCKS to surface the
/// "every keystroke produces a 60 KB rewrite" pathology: if
/// matched / (calls × model_blocks_per_call) is far below 1, block-
/// id matching is failing and every block is being treated as a
/// fresh insert.
pub const SYNC_CALLS: &str = "client.collab.sync_children_calls_total";

/// Sum of `new_children.len()` across all `sync_children` calls.
/// Divided by SYNC_CALLS gives "blocks per edit"; divided by
/// SYNC_MATCHED_BLOCKS gives the miss ratio.
pub const SYNC_MODEL_BLOCKS: &str = "client.collab.sync_model_blocks_total";

/// Sum of `matched` count across all `sync_children` calls — i.e.
/// how many of the model's blocks found a corresponding yrs block
/// by blockId. A healthy run has SYNC_MATCHED_BLOCKS ≈
/// SYNC_MODEL_BLOCKS; the current bug shows up as
/// SYNC_MATCHED_BLOCKS ≪ SYNC_MODEL_BLOCKS.
pub const SYNC_MATCHED_BLOCKS: &str = "client.collab.sync_matched_blocks_total";

/// Phase 1 of #93. Fired once per `apply_actions` call that takes
/// the "blocks reordered — clear and rewrite everything" slow path
/// (`yrs_bridge.rs::apply_actions` lines ~344). With d92dac4
/// landing reliable block-id matching, this counter should sit at
/// 0 in steady state and only tick on legitimate reorder edits
/// (drag-and-drop a list item, sort table rows). A non-zero
/// steady-state value means a regression of `find_match` ordering
/// invariants and the ~60 KB-per-edit pathology has returned.
pub const SYNC_SLOW_PATH: &str = "client.collab.sync_slow_path_total";

/// #96. Incremented each time `find_match` strategy 3 (the #92 Option B
/// "container-tag fallback") heals a pre-d92dac4 legacy block — i.e. a
/// container yrs Element that has no blockId is matched by tag. This is the
/// drain gauge for retiring the fallback: once it sits at 0 across the doc
/// population (every legacy doc has been opened+edited and migrated), the
/// fallback path can be removed and the "every Element has a real blockId"
/// invariant restored. A non-zero value means legacy docs are still in
/// circulation.
pub const BLOCKID_CONTAINER_FALLBACK: &str = "client.collab.blockid_container_fallback_total";

/// The six Phase-1 counter names, used for sanity-checking that the
/// client allowlist mirrors the server's. The server's allowlist at
/// `crates/api/src/routes/client_telemetry.rs::CLIENT_METRIC_ALLOWLIST`
/// must contain every entry here.
pub const ALL_COUNTERS: &[&str] = &[
    EDITOR_TRANSACTIONS,
    COLLAB_OBSERVE_FIRED,
    COLLAB_PENDING_DRAINED,
    WS_FRAMES_SENT,
    WS_SEND_ERRORS,
    WS_REMOTE_FRAMES_RECEIVED,
    SYNC_CALLS,
    SYNC_MODEL_BLOCKS,
    SYNC_MATCHED_BLOCKS,
    SYNC_SLOW_PATH,
    BLOCKID_CONTAINER_FALLBACK,
];

// ─── Buffer + emit API ──────────────────────────────────────────

thread_local! {
    /// WASM is single-threaded so a `RefCell` keyed by `&'static str`
    /// is the right shape — no Mutex needed. The map is bounded by
    /// `ALL_COUNTERS.len()` so no eviction strategy is needed.
    static BUFFER: RefCell<HashMap<&'static str, u64>> = RefCell::new(HashMap::new());
}

/// Increment a counter by 1. Idempotent and side-effect-free aside
/// from the buffer update — safe to call from any code path,
/// including hot loops.
pub fn inc(name: &'static str) {
    add(name, 1);
}

/// Add `delta` to a counter. Used where the call site already
/// knows a batch size (e.g. flushing N pending updates emits
/// `WS_FRAMES_SENT += N` in one call).
pub fn add(name: &'static str, delta: u64) {
    BUFFER.with(|b| {
        *b.borrow_mut().entry(name).or_insert(0) += delta;
    });
}

/// Drain the buffer and return the accumulated deltas. After this
/// returns, counters reset to zero — the next `inc` starts a new
/// batch. The periodic flush in `task 11`'s telemetry module calls
/// this and ships the result to `/api/v1/client-telemetry`. Callers
/// that don't ship — e.g. tests — can use the values directly.
pub fn drain() -> Vec<(&'static str, u64)> {
    BUFFER.with(|b| {
        let mut taken = std::mem::take(&mut *b.borrow_mut());
        let mut out: Vec<(&'static str, u64)> = taken.drain().collect();
        // Sort so flushed batches are deterministic; the server
        // doesn't care, but it makes test fixtures and operator
        // log-grepping nicer.
        out.sort_by_key(|(name, _)| *name);
        out
    })
}

// ─── Periodic flush (binary-target / WASM only) ─────────────────
//
// The buffer above is target-agnostic so the unit tests build
// natively. The flush + POST below pull in WASM-only deps
// (wasm_bindgen_futures, gloo_net, the `api` module from main.rs)
// and so are gated to the wasm32 target.

// ─── Periodic flush (binary-target / WASM only) ─────────────────
//
// The buffer above is target-agnostic so the unit tests build
// natively. The flush + POST below pull in WASM-only deps
// (wasm_bindgen_futures, gloo_net) and so are gated to the
// wasm32 target.
//
// The auth token is supplied via `set_token_getter` rather than
// reached directly because the `api` module is declared in
// `main.rs` only, not in `lib.rs` — and this file is visible
// from both targets so it must not refer to `crate::api`. The
// callback also keeps the module decoupled from where the token
// actually lives, which is appropriate for a Phase-2 future
// where the operator-pushed-config handler may rotate it.

type TokenGetter = Box<dyn Fn() -> Option<String>>;

thread_local! {
    /// Installed once by `main.rs` before `init_flush_loop` so the
    /// flush loop can look up the current Bearer token without
    /// reaching across the lib/bin boundary into `crate::api`.
    static TOKEN_GETTER: RefCell<Option<TokenGetter>> = const { RefCell::new(None) };
}

/// Register the function the flush loop will call to fetch the
/// current auth token. Idempotent — last registration wins.
pub fn set_token_getter(f: impl Fn() -> Option<String> + 'static) {
    TOKEN_GETTER.with(|t| *t.borrow_mut() = Some(Box::new(f)));
}

fn current_token() -> Option<String> {
    TOKEN_GETTER.with(|t| t.borrow().as_ref().and_then(|f| f()))
}

#[cfg(target_arch = "wasm32")]
mod flush {
    use super::*;

    /// Cadence at which the buffer drains to
    /// `/api/v1/client-telemetry`. Matches the server's
    /// `rate_limit_client_telemetry_per_min` default of 12/min
    /// (steady state is one req per 5 s, well under the cap and
    /// tight enough to surface the current bug within a few
    /// minutes of activity).
    const FLUSH_INTERVAL_MS: u32 = 10_000;

    #[derive(Debug, Serialize)]
    struct TelemetryBatch {
        counters: Vec<CounterDelta>,
    }

    #[derive(Debug, Serialize)]
    struct CounterDelta {
        name: &'static str,
        delta: u64,
    }

    thread_local! {
        /// Holds the periodic-flush handle so the interval doesn't
        /// drop (gloo timers stop when their handle drops).
        static FLUSH_HANDLE: RefCell<Option<gloo_timers::callback::Interval>> =
            const { RefCell::new(None) };
    }

    /// Install the periodic flush loop. Safe to call multiple
    /// times — re-installs replace the existing interval. Called
    /// from `main.rs` after `mount_to_body` and after
    /// `set_token_getter` so the auth token has been hydrated by
    /// the time the first flush fires.
    pub fn init() {
        let interval = gloo_timers::callback::Interval::new(FLUSH_INTERVAL_MS, || {
            flush_now();
        });
        FLUSH_HANDLE.with(|h| *h.borrow_mut() = Some(interval));
    }

    /// Drain the buffer and POST to /api/v1/client-telemetry.
    /// Exposed so the URL flag's "force flush" handler can trigger
    /// out-of-band sends. Fire-and-forget — failure is one data
    /// point lost; the next interval covers it.
    pub fn flush_now() {
        let counters: Vec<CounterDelta> = drain()
            .into_iter()
            .map(|(name, delta)| CounterDelta { name, delta })
            .collect();
        if counters.is_empty() {
            return;
        }
        let batch = TelemetryBatch { counters };
        wasm_bindgen_futures::spawn_local(async move {
            post_telemetry(batch).await;
        });
    }

    /// Fire-and-forget POST. Inlined rather than going through
    /// `client::api_post_empty` because that helper would yank a
    /// user mid-session on a 401 — appropriate for an interactive
    /// request, deadly for a periodic metrics flush. Same posture
    /// as the RUM beacon (`crate::rum::post_beacon`).
    async fn post_telemetry(batch: TelemetryBatch) {
        let Some(token) = current_token() else {
            return;
        };
        let Ok(body) = serde_json::to_string(&batch) else {
            return;
        };
        let req = gloo_net::http::Request::post("/api/v1/client-telemetry")
            .header("Content-Type", "application/json")
            .header("Authorization", &format!("Bearer {token}"));
        let Ok(req) = req.body(body) else { return };
        let _ = req.send().await;
    }
}

#[cfg(target_arch = "wasm32")]
pub use flush::{flush_now, init as init_flush_loop};

/// Stubs for the native (non-WASM) build so the unit-test target
/// still compiles when callers reference these names. The native
/// build never installs the flush loop, so the stubs are no-ops.
#[cfg(not(target_arch = "wasm32"))]
pub fn init_flush_loop() {}

#[cfg(not(target_arch = "wasm32"))]
pub fn flush_now() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inc_accumulates_and_drain_resets() {
        // Use a unique counter name per test so parallel test
        // execution doesn't observe sibling tests' state. The
        // production code only uses the ALL_COUNTERS list, but
        // tests are free to invent.
        let key = "client.test.inc_accumulates";
        for _ in 0..3 {
            add(key, 1);
        }
        let drained = drain();
        let v = drained.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        assert_eq!(v, Some(3));

        // Second drain finds nothing (reset).
        let drained2 = drain();
        assert!(drained2.iter().all(|(k, _)| *k != key));
    }

    #[test]
    fn add_zero_is_a_noop() {
        let key = "client.test.add_zero";
        add(key, 0);
        let drained = drain();
        // The entry may or may not be present with value 0; either
        // way the server's projection adds 0 which is a no-op.
        if let Some((_, v)) = drained.iter().find(|(k, _)| *k == key) {
            assert_eq!(*v, 0);
        }
    }

    #[test]
    fn all_counters_have_client_prefix() {
        // The discrepancy_map agent matches client metrics by the
        // `client.` prefix. If we ever ship one that doesn't, the
        // server's allowlist would still accept it but the agent
        // pairing would silently miss it.
        for c in ALL_COUNTERS {
            assert!(
                c.starts_with("client."),
                "counter {c} must start with `client.` prefix",
            );
        }
    }
}
