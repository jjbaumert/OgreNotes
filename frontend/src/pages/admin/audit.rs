// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Admin audit log viewer — combined AdminAudit + SecurityAudit
//! viewer with target, actor, kind, and date-range filters. Both
//! audit tables key rows on the user PK, so `target` is required to
//! load anything (matches the M-E2 backend contract).

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::admin::{self, AuditEntry, AuditFilter};

use super::AdminGate;

#[component]
pub fn AdminAuditPage() -> impl IntoView {
    view! {
        <AdminGate>
            <AuditView />
        </AdminGate>
    }
}

#[component]
fn AuditView() -> impl IntoView {
    let (target, set_target) = signal::<String>(String::new());
    let (actor, set_actor) = signal::<String>(String::new());
    let (kind, set_kind) = signal::<String>(String::new());
    let (from_input, set_from_input) = signal::<String>(String::new());
    let (to_input, set_to_input) = signal::<String>(String::new());

    let (entries, set_entries) = signal::<Vec<AuditEntry>>(Vec::new());
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal::<bool>(false);

    let load = move || {
        let tgt = target.get();
        if tgt.trim().is_empty() {
            set_error.set(Some(crate::t!("admin-audit-error-target-required")));
            set_entries.set(Vec::new());
            return;
        }
        set_busy.set(true);
        set_error.set(None);
        let actor_v = actor.get();
        let kind_v = kind.get();
        let from_v = parse_iso_to_usec(&from_input.get());
        let to_v = parse_iso_to_usec(&to_input.get());
        spawn_local(async move {
            let filter = AuditFilter {
                target: tgt.trim(),
                actor: Some(actor_v.trim()).filter(|s| !s.is_empty()),
                kind: Some(kind_v.trim()).filter(|s| !s.is_empty()),
                from: from_v,
                to: to_v,
                limit: Some(100),
            };
            match admin::list_audit(filter).await {
                Ok(list) => set_entries.set(list.entries),
                Err(e) => set_error.set(Some(crate::t!("admin-audit-error-load-failed", err = format!("{e:?}")))),
            }
            set_busy.set(false);
        });
    };

    view! {
        <main id="main-content" tabindex="-1" class="admin-audit">
            <h1>{crate::t!("admin-audit-title")}</h1>

            <div class="admin-audit-filters">
                <label>
                    {crate::t!("admin-audit-label-target")}" "
                    <input
                        type="text"
                        prop:value=move || target.get()
                        on:input=move |ev| set_target.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    {crate::t!("admin-audit-label-actor")}" "
                    <input
                        type="text"
                        prop:value=move || actor.get()
                        on:input=move |ev| set_actor.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    {crate::t!("admin-audit-label-kind")}" "
                    <input
                        type="text"
                        placeholder=crate::t!("admin-audit-placeholder-kind")
                        prop:value=move || kind.get()
                        on:input=move |ev| set_kind.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    {crate::t!("admin-audit-label-from")}" "
                    <input
                        type="datetime-local"
                        prop:value=move || from_input.get()
                        on:input=move |ev| set_from_input.set(event_target_value(&ev))
                    />
                </label>
                <label>
                    {crate::t!("admin-audit-label-to")}" "
                    <input
                        type="datetime-local"
                        prop:value=move || to_input.get()
                        on:input=move |ev| set_to_input.set(event_target_value(&ev))
                    />
                </label>
                <button on:click=move |_| load() disabled=move || busy.get()>
                    {crate::t!("admin-audit-search")}
                </button>
            </div>

            {move || error.get().map(|msg| view! {
                <div class="admin-error">{msg}</div>
            })}

            <table class="admin-audit-table">
                <thead>
                    <tr>
                        <th>{crate::t!("admin-audit-th-when")}</th>
                        <th>{crate::t!("admin-audit-th-source")}</th>
                        <th>{crate::t!("admin-audit-th-kind")}</th>
                        <th>{crate::t!("admin-audit-th-actor")}</th>
                        <th>{crate::t!("admin-audit-th-target")}</th>
                        <th>{crate::t!("admin-audit-th-detail")}</th>
                    </tr>
                </thead>
                <tbody>
                    <For
                        each=move || entries.get()
                        key=|e| e.audit_id.clone()
                        children=move |e: AuditEntry| {
                            view! {
                                <tr class={format!("admin-audit-row admin-audit-{}", e.source)}>
                                    <td>{format_usec(e.created_at)}</td>
                                    <td>{e.source.clone()}</td>
                                    <td>{e.kind.clone()}</td>
                                    <td>{e.actor_id.clone()}</td>
                                    <td>{e.target_user_id.clone()}</td>
                                    <td><code>{e.detail.to_string()}</code></td>
                                </tr>
                            }
                        }
                    />
                </tbody>
            </table>
        </main>
    }
}

fn format_usec(usec: i64) -> String {
    let ms = (usec / 1000) as f64;
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    date.to_iso_string().as_string().unwrap_or_default()
}

/// Parse the `<input type="datetime-local">` string back to
/// microseconds-since-epoch. The control returns `YYYY-MM-DDTHH:MM`
/// in local time; we feed it to `Date.parse` which interprets it as
/// local and returns ms-since-epoch. Empty / invalid inputs map to
/// `None` so the filter is left unset on the server side.
fn parse_iso_to_usec(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let ms = js_sys::Date::parse(trimmed);
    if ms.is_nan() {
        return None;
    }
    Some((ms as i64) * 1000)
}
