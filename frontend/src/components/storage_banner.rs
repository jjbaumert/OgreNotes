// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! App-wide banner shown while the server can't reach its storage.
//!
//! Raised by [`crate::storage_status`] (a fail-fast 503 from the API, or
//! the collaboration socket reporting an edit it couldn't save). While
//! it's up, this polls `GET /api/v1/status` and clears it once the
//! server reports storage reachable; open documents then re-send their
//! unsaved edits (see `sync_indicator::poll_sync_state`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use leptos::prelude::*;

/// How often to ask the server whether storage is back.
const POLL_MS: u32 = 10_000;

#[component]
pub fn StorageBanner() -> impl IntoView {
    let offline = crate::storage_status::install();

    let active = Arc::new(AtomicBool::new(true));
    let active_for_cleanup = Arc::clone(&active);
    on_cleanup(move || active_for_cleanup.store(false, Ordering::Relaxed));

    leptos::task::spawn_local(async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(POLL_MS).await;
            if !active.load(Ordering::Relaxed) {
                break;
            }
            if crate::storage_status::is_offline() && storage_is_reachable().await {
                crate::storage_status::report_reachable();
            }
        }
    });

    view! {
        <Show when=move || offline.get()>
            <div class="storage-banner" role="alert">
                <strong>{crate::t!("storage-offline-title")}</strong>
                " "
                {crate::t!("storage-offline-body")}
            </div>
        </Show>
    }
}

/// One `GET /api/v1/status`. Any failure (the server itself unreachable
/// included) counts as "not yet".
async fn storage_is_reachable() -> bool {
    let Ok(resp) = gloo_net::http::Request::get("/api/v1/status").send().await else {
        return false;
    };
    if !resp.ok() {
        return false;
    }
    resp.json::<serde_json::Value>()
        .await
        .is_ok_and(|v| v["storage"] == "ok")
}
