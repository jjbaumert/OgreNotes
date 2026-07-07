// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P3 piece B — offline / sync indicator.
//!
//! Renders the document-header pill that tells the user whether
//! their edits are landing on the server:
//!
//!   • Saved          — connected + queue empty
//!   • Saving…        — connected + queue draining, or mid-handshake
//!   • Offline        — disconnected + nothing queued
//!   • Offline — N pending — disconnected + N un-sent updates
//!
//! v1 deliberately stops short of queued-write **replay** — that
//! needs client-side CRDT-on-storage work (carried to v2 in
//! `design/phase5-plan.md`). Today an offline-while-typing user
//! sees their text accumulate locally; on reconnect the CollabClient
//! sends the queued updates via its normal sync path. The indicator
//! is the user-visible truth of "is your work landing right now".
//!
//! The component is **presentational** — it consumes a
//! `Signal<SyncState>` and renders. The page is responsible for
//! driving that signal; the canonical wiring is
//! [`poll_sync_state`], which polls the CollabClient every 500 ms
//! and writes the derived state to a `WriteSignal`. Half-second
//! latency is below the threshold a user notices and avoids
//! per-keystroke recomputes.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use leptos::prelude::*;

use crate::collab::ws_client::{CollabClient, ConnectionState};

/// User-visible sync-state classification. Derived from the
/// CollabClient's `connection_state()` + `pending_count()` —
/// kept as its own enum so the UI can render four distinct
/// affordances without spreading WebSocket-state semantics into
/// view code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// Up to date with the server. No queue, connection synced.
    Saved,
    /// Either mid-handshake or the queue is draining. Transient.
    Saving,
    /// WebSocket is down. `pending` carries the un-sent count so
    /// the badge can warn the user how much is at risk.
    Offline { pending: usize },
}

impl SyncState {
    /// Convenience predicate for callers gating destructive UI on
    /// connection state — e.g. the share / delete buttons can use
    /// `state.get().is_offline()` to disable themselves while the
    /// pending queue would be at risk.
    pub fn is_offline(&self) -> bool {
        matches!(self, SyncState::Offline { .. })
    }
}

/// Pill-shaped sync indicator. Mount once per editor page;
/// driven by a `Signal<SyncState>` the page is responsible for
/// keeping fresh.
#[component]
pub fn SyncIndicator(state: Signal<SyncState>) -> impl IntoView {
    // CSS class flips on the state variant so the colour swap is
    // a single class change (no re-mount), and aria-live="polite"
    // means screen readers announce a transition without
    // interrupting the user mid-keystroke.
    view! {
        <div
            class=move || format!("sync-indicator {}", class_for(state.get()))
            role="status"
            aria-live="polite"
            aria-atomic="true"
            title=move || tooltip_for(state.get())
        >
            <span class="sync-indicator-dot" aria-hidden="true"></span>
            <span class="sync-indicator-label">{move || label_for(state.get())}</span>
        </div>
    }
}

fn class_for(state: SyncState) -> &'static str {
    match state {
        SyncState::Saved => "is-saved",
        SyncState::Saving => "is-saving",
        SyncState::Offline { .. } => "is-offline",
    }
}

fn label_for(state: SyncState) -> String {
    match state {
        SyncState::Saved => crate::t!("sync-saved"),
        SyncState::Saving => crate::t!("sync-saving"),
        SyncState::Offline { pending: 0 } => crate::t!("sync-offline"),
        SyncState::Offline { pending } => {
            crate::t!("sync-offline-pending", count = pending as i64)
        }
    }
}

fn tooltip_for(state: SyncState) -> String {
    // Tooltip carries the long-form explanation; the badge text
    // stays terse. Screen readers also read the label, not this.
    match state {
        SyncState::Saved => crate::t!("sync-saved-tooltip"),
        SyncState::Saving => crate::t!("sync-saving-tooltip"),
        SyncState::Offline { pending: 0 } => crate::t!("sync-offline-tooltip"),
        SyncState::Offline { pending } => {
            crate::t!("sync-offline-pending-tooltip", count = pending as i64)
        }
    }
}

/// Start a polling loop that derives a `SyncState` from a
/// (possibly absent) `CollabClient` and writes it to `set_state`
/// every 500 ms. Cancels itself on Leptos component unmount via
/// `on_cleanup`, so safe to call once from inside a Leptos page
/// Effect at mount time.
///
/// The polling cadence is a deliberate trade — a faster interval
/// would feel snappier on offline-detection but burns CPU on every
/// editor session; 500 ms is the upper bound of "imperceptible
/// latency" for status UI and matches comparable products.
///
/// Why polling rather than a state-change callback on the
/// CollabClient: the client's state transitions live inside
/// `Rc<RefCell<_>>` closures fired from raw `WebSocket` event
/// handlers; threading a Leptos signal write through every one of
/// those mutation sites is more invasive than the half-second
/// polling delay is worth. A future piece can swap this for a
/// proper subscription if the wiring cost ever pays off.
pub fn poll_sync_state(
    client: Rc<RefCell<Option<CollabClient>>>,
    set_state: WriteSignal<SyncState>,
) {
    // The CollabClient itself is `!Send` (Rc<RefCell<…>>), but the
    // cancellation flag has to be Send + Sync because Leptos's
    // `on_cleanup` accepts only Send + Sync closures. Arc<AtomicBool>
    // matches the pattern used by `notification_bell.rs`.
    let active = Arc::new(AtomicBool::new(true));
    let active_for_cleanup = Arc::clone(&active);
    on_cleanup(move || active_for_cleanup.store(false, Ordering::Relaxed));

    leptos::task::spawn_local(async move {
        // Seed immediately so the badge doesn't briefly render
        // a stale `Saved` placeholder before the first tick.
        set_state.set(compute_state(&client));
        loop {
            gloo_timers::future::TimeoutFuture::new(500).await;
            if !active.load(Ordering::Relaxed) {
                break;
            }
            let next = compute_state(&client);
            set_state.set(next);
        }
    });
}

fn compute_state(client: &Rc<RefCell<Option<CollabClient>>>) -> SyncState {
    let borrow = client.borrow();
    let Some(c) = borrow.as_ref() else {
        // No client wired up yet — the page is in the gap between
        // mount and the first connect. Pretend "Saved" so the badge
        // doesn't shout false alarms on a fresh page load.
        return SyncState::Saved;
    };
    let conn = c.connection_state();
    let pending = c.pending_count();
    match conn {
        ConnectionState::Synced if pending == 0 => SyncState::Saved,
        ConnectionState::Synced
        | ConnectionState::Connected
        | ConnectionState::Connecting => SyncState::Saving,
        ConnectionState::Disconnected => SyncState::Offline { pending },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_offline_only_for_offline_variant() {
        assert!(!SyncState::Saved.is_offline());
        assert!(!SyncState::Saving.is_offline());
        assert!(SyncState::Offline { pending: 0 }.is_offline());
        assert!(SyncState::Offline { pending: 5 }.is_offline());
    }

    #[test]
    fn class_for_each_variant() {
        assert_eq!(class_for(SyncState::Saved), "is-saved");
        assert_eq!(class_for(SyncState::Saving), "is-saving");
        assert_eq!(class_for(SyncState::Offline { pending: 0 }), "is-offline");
        assert_eq!(class_for(SyncState::Offline { pending: 3 }), "is-offline");
    }
}
