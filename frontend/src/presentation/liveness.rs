// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Pure predicates for present mode's connection-liveness poll (#210).
//!
//! `frontend/src/pages/present.rs` never called `CollabClient::record_activity()`,
//! so the heartbeat (`ws_client.rs`) would let the socket sit idle past
//! `IDLE_DISCONNECT_MS` (30 min) and deliberately close it — and present
//! mode had no reconnect path either, so every follower's "Follow …" pill
//! silently vanished. The approved fix is "visible tab = active": a
//! displayed slide IS the live session, so the page keeps the connection
//! warm unconditionally while `document.visibilityState` is `"visible"`,
//! and reconnects once the tab is visible again if the socket dropped
//! while backgrounded (or from a transient network blip).
//!
//! These two decisions — "should this tick record activity" and "should
//! this tick ask for a reconnect" — are pure functions of (visibility,
//! connectedness), so they're pulled out of the DOM/WASM plumbing in
//! `present.rs` (which needs a live browser environment to exercise) and
//! placed here where `cargo test --lib` can reach them directly.

/// Whether the present-mode liveness poll should call
/// `client.record_activity()` this tick. Unconditional while the tab is
/// visible — "visible tab = active" per the approved design, regardless
/// of whether the viewer has pressed a key. A hidden/backgrounded tab is
/// left alone so it can idle out normally, same as any other page.
pub fn should_keep_warm(document_visible: bool) -> bool {
    document_visible
}

/// Whether the present-mode liveness poll should bump the reconnect
/// trigger this tick. Mirrors `document.rs`'s `on_activity` gate
/// (`!client.is_connected()`): only a *visible* tab whose client is
/// *not* currently connected needs a reconnect — a visible tab that's
/// already connected has nothing to do, and a hidden tab shouldn't force
/// a reconnect it doesn't need yet (it'll get one via this same check
/// the moment it becomes visible again).
pub fn should_trigger_reconnect(document_visible: bool, client_connected: bool) -> bool {
    document_visible && !client_connected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_warm_only_while_visible() {
        assert!(should_keep_warm(true));
        assert!(!should_keep_warm(false));
    }

    #[test]
    fn reconnect_only_when_visible_and_disconnected() {
        assert!(should_trigger_reconnect(true, false), "visible + dropped -> reconnect");
        assert!(!should_trigger_reconnect(true, true), "visible + already connected -> no-op");
        assert!(!should_trigger_reconnect(false, false), "hidden + dropped -> wait for visibility");
        assert!(!should_trigger_reconnect(false, true), "hidden + connected -> no-op");
    }
}
