// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Cross-document consent prompt.
//!
//! When a spreadsheet contains REFERENCERANGE / REFERENCESHEET
//! formulas pointing at foreign documents, the engine queues those
//! ids for fetch. Before the view layer dispatches the HTTP fetches
//! (which would use the *current* user's auth, not the doc author's),
//! the user must approve — otherwise a malicious doc could weaponize
//! the recipient's permissions to exfiltrate any doc the recipient
//! has access to.
//!
//! Approval is session-scoped (a `HashSet<String>` in a signal),
//! NOT persisted into the document. Re-opening the same doc in a
//! new session re-prompts.

use std::collections::HashSet;

use leptos::prelude::*;

pub(super) fn render_foreign_consent(
    consent_pending: ReadSignal<Vec<String>>,
    set_consent_pending: WriteSignal<Vec<String>>,
    set_consent_approved: WriteSignal<HashSet<String>>,
    on_approve: impl Fn(Vec<String>) + Clone + Send + Sync + 'static,
    on_deny: impl Fn(Vec<String>) + Clone + Send + Sync + 'static,
) -> impl IntoView {
    // `Clone` (not `Copy`) on the handler bounds so callers can pass
    // closures that capture non-Copy state — the SpreadsheetView's
    // `Arc<AtomicBool>` liveness flag in particular. The outer
    // render closure clones the handlers once per re-render and
    // hands a fresh clone to each button's on:click.
    move || {
        let pending = consent_pending.get();
        if pending.is_empty() {
            return view! { <span></span> }.into_any();
        }
        let on_approve_for_btn = on_approve.clone();
        let on_deny_for_btn = on_deny.clone();
        view! {
            <div class="ss-foreign-consent-backdrop"></div>
            <div class="ss-foreign-consent">
                <h3 class="ss-foreign-consent-title">
                    {crate::t!("ss-foreign-title")}
                </h3>
                <ul class="ss-foreign-consent-list">
                    {pending.iter().map(|id| view! {
                        <li class="ss-foreign-consent-item">
                            <code>{id.clone()}</code>
                        </li>
                    }).collect::<Vec<_>>()}
                </ul>
                <p class="ss-foreign-consent-hint">
                    {crate::t!("ss-foreign-hint")}
                </p>
                <div class="ss-foreign-consent-actions">
                    <button class="ss-tool-btn" on:click=move |_| {
                        let ids = consent_pending.get_untracked();
                        on_deny_for_btn(ids);
                        set_consent_pending.set(Vec::new());
                    }>{crate::t!("ss-foreign-deny")}</button>
                    <button class="ss-tool-btn active" on:click=move |_| {
                        let ids = consent_pending.get_untracked();
                        set_consent_approved.update(|s| {
                            for id in &ids { s.insert(id.clone()); }
                        });
                        on_approve_for_btn(ids);
                        set_consent_pending.set(Vec::new());
                    }>{crate::t!("ss-foreign-allow")}</button>
                </div>
            </div>
        }.into_any()
    }
}
