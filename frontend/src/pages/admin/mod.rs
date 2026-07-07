// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E2 admin console. Three pages share this module:
//!
//! - `users.rs` — list / search / per-row actions (disable, enable,
//!   promote, demote, set ask-enabled).
//! - `metrics.rs` — in-process counters / gauges / histograms.
//! - `audit.rs` — combined AdminAudit + SecurityAudit viewer with
//!   target / actor / kind / date filters.
//!
//! Every page mounts behind a route-level gate (`AdminGate`) that
//! reads `/users/me` once, redirects non-admins to `/`, and otherwise
//! renders the child. The server still enforces `require_admin` on
//! every `/admin/*` request — the frontend gate is UX, never
//! authoritative.

pub mod audit;
pub mod metrics;
pub mod users;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use serde::Deserialize;
use wasm_bindgen_futures::spawn_local;

use crate::api::client;

/// Slim decode of `/users/me` used by the route gate. `is_admin` is
/// `#[serde(default)]` so a backend that hasn't deployed the M-E2
/// patch (which added the field) reads as `false` — the gate then
/// redirects, matching the server-side behavior (`require_admin`
/// rejects with 403). The full `UserMeResponse` lives in
/// `pages/home.rs` and we don't pull it in here so a future schema
/// drift on home doesn't ripple into the admin pages.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct UserMeAdminGate {
    #[serde(default)]
    is_admin: bool,
}

/// Wrap an admin page in a one-shot async check against `/users/me`.
/// While the check is in flight the wrapper renders a loading
/// placeholder; if the user is non-admin we navigate to `/` and the
/// child never mounts.
#[component]
pub fn AdminGate(children: ChildrenFn) -> impl IntoView {
    let (status, set_status) = signal::<GateStatus>(GateStatus::Pending);

    if !client::is_authenticated() {
        let navigate = use_navigate();
        navigate("/login", Default::default());
        return view! { <div>{crate::t!("common-redirecting-login")}</div> }.into_any();
    }

    spawn_local(async move {
        match client::api_get::<UserMeAdminGate>("/users/me").await {
            Ok(me) if me.is_admin => set_status.set(GateStatus::Allowed),
            Ok(_) => set_status.set(GateStatus::Denied),
            Err(_) => set_status.set(GateStatus::Denied),
        }
    });

    view! {
        {move || match status.get() {
            GateStatus::Pending => view! {
                <div class="admin-loading">{crate::t!("admin-loading")}</div>
            }.into_any(),
            GateStatus::Denied => {
                let navigate = use_navigate();
                navigate("/", Default::default());
                view! { <div>{crate::t!("admin-redirecting")}</div> }.into_any()
            }
            GateStatus::Allowed => view! {
                <div class="admin-shell">
                    <AdminNav />
                    {children()}
                </div>
            }.into_any(),
        }}
    }
    .into_any()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GateStatus {
    Pending,
    Allowed,
    Denied,
}

/// Sub-nav rendered at the top of every admin page. Three links plus
/// a "back to app" shortcut so the operator isn't trapped on
/// `/admin/*` after they're done.
#[component]
fn AdminNav() -> impl IntoView {
    view! {
        <nav class="admin-nav">
            <a href="/admin/users">{crate::t!("admin-nav-users")}</a>
            <a href="/admin/metrics">{crate::t!("admin-nav-metrics")}</a>
            <a href="/admin/audit">{crate::t!("admin-nav-audit")}</a>
            <span class="admin-nav-sep">"·"</span>
            <a href="/">{crate::t!("admin-nav-back")}</a>
        </nav>
    }
}
