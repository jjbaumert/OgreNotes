// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Can the server reach its storage?
//!
//! A self-hosted server keeps documents in cloud storage (DynamoDB/S3)
//! and can lose its internet connection while the browser can still
//! reach the server itself. The WebSocket "Offline" badge doesn't cover
//! that case — the socket stays up — so this module holds the app-wide
//! answer, set from two places:
//!
//! - the API client, when a response carries the server's
//!   `x-ogrenotes-storage: unavailable` header (its fail-fast 503);
//! - the collaboration client, when the server reports that an edit
//!   could not be saved (`persist-failed`).
//!
//! The app shell's banner reads it and polls `GET /api/v1/status` until
//! storage is back, then clears it.

use std::cell::{Cell, RefCell};

use leptos::prelude::*;

/// Header the server sets on its fail-fast "storage unavailable" 503.
pub const STORAGE_HEADER: &str = "x-ogrenotes-storage";

thread_local! {
    /// The answer itself. A plain flag so it works before the app mounts
    /// (the boot-time session restore reports into it).
    static FLAG: Cell<bool> = const { Cell::new(false) };
    /// A reactive mirror for the banner, once the app shell installs it.
    static SIGNAL: RefCell<Option<RwSignal<bool>>> = const { RefCell::new(None) };
}

/// Create the app-wide signal, seeded with the current answer. Called
/// once by the app shell; returns it for the banner to read.
pub fn install() -> RwSignal<bool> {
    let signal = RwSignal::new(FLAG.with(Cell::get));
    SIGNAL.with(|s| *s.borrow_mut() = Some(signal));
    signal
}

fn set(offline: bool) {
    if FLAG.with(|f| f.replace(offline)) == offline {
        return;
    }
    SIGNAL.with(|s| {
        if let Some(signal) = *s.borrow() {
            signal.set(offline);
        }
    });
}

/// Record that the server can't reach its storage.
pub fn report_unreachable() {
    set(true);
}

/// Record that storage is reachable again.
pub fn report_reachable() {
    set(false);
}

/// Whether storage is currently believed unreachable (untracked read).
pub fn is_offline() -> bool {
    FLAG.with(Cell::get)
}

/// Whether a response is the server's storage-unavailable answer.
pub fn is_storage_unavailable(status: u16, header: Option<&str>) -> bool {
    status == 503 && header == Some("unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_tagged_503_means_storage_is_down() {
        assert!(is_storage_unavailable(503, Some("unavailable")));
        // Other 503s (Ask not configured, job queue down) are not it.
        assert!(!is_storage_unavailable(503, None));
        assert!(!is_storage_unavailable(500, Some("unavailable")));
    }
}
