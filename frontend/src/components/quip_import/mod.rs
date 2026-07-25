// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// Quip import wizard. Step 1: paste a Quip personal access token,
// POST it to `/imports/quip/connect`, and on success show the
// connected profile + a checklist of the caller's root Quip folders
// (Phase 0). Step 2 (this task): "Continue" persists the checked
// scope + the user's Home folder as the destination via
// `POST /imports/quip/{id}/start`, then Step 3 polls
// `GET /imports/quip/{id}` on an interval and shows live inventory
// progress until the walk completes (`phase >= 1`) or the run hits a
// terminal failure status.
//
// Mirrors `template_picker_modal.rs` for the modal skeleton (backdrop
// + `<Show when=visible>` + per-open reset) and `share_dialog.rs` for
// the focus trap + checkbox-row list pattern. The Home-folder lookup
// mirrors the `UserMeResponse` local-struct pattern in
// `folder_picker.rs` / `duplicate_dialog.rs` — Phase 1 deliberately
// skips a destination picker (nesting `FolderPickerDialog` inside
// this modal risks focus-trap conflicts) and always targets Home.
//
// SECURITY: the token field is `type="password"` and its value is
// never passed to `console.*`/`web_sys::console::*` — only
// `ApiClientError`'s opaque `Display` (status + x-request-id, never a
// response body) reaches the error banner. The token signal is
// cleared both when the modal closes and immediately after a
// successful connect, since the token now lives server-side only
// (the backend's `ImportRepo`/`ImportRecord` deliberately has no
// token field — see crates/storage). No token handling happens past
// `connect` — `start`/`get_status` never see or send one.

use std::collections::HashMap;

use leptos::prelude::*;

use wasm_bindgen::JsCast;

use crate::a11y;
use crate::api::client;
use crate::api::imports::{self, ConnectResponse};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserMeResponse {
    home_folder_id: String,
}

/// Why an in-flight import stopped polling with something the user
/// needs to act on. `Cancelled` collapses into `Failed` — Phase 1 has
/// no user-facing "cancel" affordance, so if the run shows up
/// cancelled it was cancelled some other way and reads the same as a
/// failure to this wizard.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportTerminal {
    Failed,
    TokenExpired,
}

#[component]
pub fn QuipImportWizard(
    /// Visibility flag — the parent (the shell) owns it and flips it
    /// from the entry point / on close.
    visible: ReadSignal<bool>,
    /// Called when the wizard should close (backdrop click, Escape,
    /// the header's close button).
    on_close: Callback<()>,
) -> impl IntoView {
    let (token, set_token) = signal(String::new());
    let (connecting, set_connecting) = signal(false);
    let (error, set_error) = signal::<Option<String>>(None);
    let (response, set_response) = signal::<Option<ConnectResponse>>(None);
    // Keyed by root-folder id; default-checked once a connect response
    // arrives (see `do_connect`). Read by `do_start` to build the
    // scope for the actual import.
    let selected: RwSignal<HashMap<String, bool>> = RwSignal::new(HashMap::new());

    // Phase 1 state: the destination (always the user's Home folder —
    // see the module doc comment), the start-in-flight flag, whether
    // we've moved into the progress step, the latest poll result, and
    // a terminal outcome if the run failed / the token was rejected.
    let (home_folder_id, set_home_folder_id) = signal::<Option<String>>(None);
    let (starting, set_starting) = signal(false);
    let (started, set_started) = signal(false);
    // Gate for the poll loop below: flipped false to stop it early
    // (modal close, a terminal result) without waiting out the
    // in-flight `TimeoutFuture`.
    let (polling, set_polling) = signal(false);
    let (progress, set_progress) = signal::<Option<imports::StatusResponse>>(None);
    let (terminal, set_terminal) = signal::<Option<ImportTerminal>>(None);
    // Session identity for the poll loop. The wizard component stays
    // mounted across close/reopen (only `visible` toggles via `<Show>`),
    // so `polling`/`progress`/`terminal` are the SAME signals reused by
    // every `do_start` in the component's lifetime. `polling` alone only
    // guards the loop's *continue* points (top of iteration, before the
    // sleep) — it does NOT guard the moment right after `get_status`
    // resolves, so a loop that was live when the modal closed and got
    // reopened+restarted before its in-flight request returned could
    // write a stale import's status into the signals the NEW import's
    // Step 3 is rendering, and re-enter its own loop as a second live
    // poller. `generation` is bumped on every `do_start` (and effectively
    // invalidated on close, since the next `do_start` after a reopen
    // bumps it again); each loop iteration compares its captured value
    // against the live one, both right after the await and before the
    // sleep, and stops writing/looping the instant it's superseded.
    let generation: RwSignal<u64> = RwSignal::new(0);

    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible.into());

    // Per-open reset + close-time token wipe. Fires on every `visible`
    // transition (not just "became true") so the token never lingers
    // in the signal after the dialog is dismissed. `set_polling(false)`
    // runs unconditionally on *every* transition (open or close) so a
    // live progress-poll loop is always stopped the instant the modal
    // is dismissed — the loop below re-checks this signal every tick
    // and right before its `await`, so it can't outlive the close.
    // `generation` is bumped here too (belt-and-suspenders with
    // `polling`/`on_cleanup`) so an in-flight loop's post-await identity
    // check fails immediately on close, even before its next iteration
    // would've re-checked `polling`.
    Effect::new(move |_| {
        let is_open = visible.get();
        set_token.set(String::new());
        set_polling.set(false);
        generation.update(|g| *g = g.wrapping_add(1));
        if !is_open {
            return;
        }
        set_connecting.set(false);
        set_error.set(None);
        set_response.set(None);
        selected.set(HashMap::new());
        set_starting.set(false);
        set_started.set(false);
        set_progress.set(None);
        set_terminal.set(None);
    });

    // Defensive: if the wizard component is ever unmounted outright
    // (not just hidden via `<Show>`), stop any live poll loop too.
    on_cleanup(move || set_polling.set(false));

    // Home folder id for the destination line + the `start` call.
    // Fetched once, eagerly, rather than gated on `visible` — mirrors
    // `folder_picker.rs`'s rationale: gating on `visible` risks the
    // fetch not re-firing on a later open if the effect already ran.
    Effect::new(move |_| {
        if home_folder_id.get_untracked().is_some() {
            return;
        }
        leptos::task::spawn_local(async move {
            // Surface a failed lookup into the wizard's error banner — the
            // Continue button is disabled while `home_folder_id` is None, so a
            // swallowed error would strand it with no message. `ApiClientError`'s
            // `Display` is opaque (status + x-request-id, never a body — see
            // `do_connect`), so it's safe to surface directly.
            match client::api_get::<UserMeResponse>("/users/me").await {
                Ok(me) => set_home_folder_id.set(Some(me.home_folder_id)),
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    });

    let do_connect = move || {
        if connecting.get_untracked() {
            return;
        }
        let tok = token.get_untracked();
        if tok.trim().is_empty() {
            return;
        }
        set_connecting.set(true);
        set_error.set(None);
        leptos::task::spawn_local(async move {
            match imports::connect(&tok).await {
                Ok(resp) => {
                    // The token now lives server-side (or the connect
                    // attempt failed and is worthless either way) —
                    // clear it from the client immediately.
                    set_token.set(String::new());
                    let sel: HashMap<String, bool> = resp
                        .root_folders
                        .iter()
                        .map(|f| (f.id.clone(), true))
                        .collect();
                    selected.set(sel);
                    set_response.set(Some(resp));
                    set_connecting.set(false);
                }
                Err(e) => {
                    set_token.set(String::new());
                    // `ApiClientError::Display` never carries a response
                    // body (see api/client.rs `http_error`) — safe to
                    // surface directly, and never logged.
                    set_error.set(Some(e.to_string()));
                    set_connecting.set(false);
                }
            }
        });
    };

    // Kick off the actual import: persist the checked scope + Home as
    // the destination, then switch into the progress step and start
    // polling. `start`'s failure path reuses the same `error` signal
    // + `quip-import-error` banner the connect step already uses —
    // `ApiClientError::Display` is opaque by construction (see
    // `do_connect` above), so it's safe to surface directly here too.
    let do_start = move || {
        if starting.get_untracked() || started.get_untracked() {
            return;
        }
        let Some(resp) = response.get_untracked() else {
            return;
        };
        let Some(home) = home_folder_id.get_untracked() else {
            return;
        };
        let roots: Vec<String> = selected
            .get_untracked()
            .into_iter()
            .filter_map(|(id, checked)| checked.then_some(id))
            .collect();
        if roots.is_empty() {
            return;
        }
        set_starting.set(true);
        set_error.set(None);
        let import_id = resp.import_id.clone();
        leptos::task::spawn_local(async move {
            match imports::start(&import_id, &roots, &home).await {
                Ok(_) => {
                    set_starting.set(false);
                    set_started.set(true);
                    set_progress.set(None);
                    set_terminal.set(None);
                    set_polling.set(true);

                    // Bump the session generation and capture it — this
                    // loop only ever writes `progress`/`terminal` (or
                    // continues looping) while `generation` still equals
                    // `my_gen`. The wizard component stays mounted across
                    // close/reopen (only `visible` toggles), so those
                    // signals are shared across every `do_start` in its
                    // lifetime; without this check a stale loop from an
                    // abandoned import could, after a close+reopen+
                    // restart race, write its stale status into the NEW
                    // import's Step 3 and keep running as a second live
                    // poller (see task-5 fix report for the failure
                    // sequence this closes).
                    let my_gen = generation.get_untracked().wrapping_add(1);
                    generation.set(my_gen);

                    // Poll loop: ~1500ms cadence, stopped by `polling`
                    // (flipped false on modal close/reopen by the reset
                    // Effect above, or by this loop itself once the run
                    // reaches a terminal state), by `visible` going
                    // false directly, or by `generation` moving past
                    // `my_gen` (this loop has been superseded by a
                    // newer `do_start`). `polling`/`visible` are
                    // checked before the request and before the sleep;
                    // `generation` is additionally checked immediately
                    // after `get_status` resolves and BEFORE any
                    // `set_progress`/`set_terminal` write, so a
                    // superseded loop can observe a stale response but
                    // never act on it.
                    let poll_import_id = import_id.clone();
                    leptos::task::spawn_local(async move {
                        loop {
                            if !polling.get_untracked()
                                || !visible.get_untracked()
                                || generation.get_untracked() != my_gen
                            {
                                break;
                            }
                            let status = imports::get_status(&poll_import_id).await;
                            // Re-check identity right after the await,
                            // before touching any shared signal — a
                            // reopen+restart could have superseded this
                            // loop while the request was in flight.
                            if generation.get_untracked() != my_gen {
                                break;
                            }
                            if let Ok(st) = status {
                                let is_failure = matches!(
                                    st.status.as_str(),
                                    "failed" | "tokenrejected" | "cancelled"
                                );
                                let inventory_done = st.phase >= 1;
                                if is_failure {
                                    set_terminal.set(Some(if st.status == "tokenrejected" {
                                        ImportTerminal::TokenExpired
                                    } else {
                                        ImportTerminal::Failed
                                    }));
                                    set_progress.set(Some(st));
                                    set_polling.set(false);
                                    break;
                                }
                                set_progress.set(Some(st));
                                if inventory_done {
                                    set_polling.set(false);
                                    break;
                                }
                            }
                            // A dropped/errored poll is treated as
                            // transient and retried on the next tick
                            // rather than surfaced — a single flaky
                            // request shouldn't flash an error banner
                            // over an otherwise-healthy "Scanning…"
                            // view.
                            if !polling.get_untracked()
                                || !visible.get_untracked()
                                || generation.get_untracked() != my_gen
                            {
                                break;
                            }
                            gloo_timers::future::TimeoutFuture::new(1500).await;
                        }
                    });
                }
                Err(e) => {
                    set_starting.set(false);
                    set_error.set(Some(e.to_string()));
                }
            }
        });
    };

    view! {
        <Show when=move || visible.get()>
            <div class="confirm-backdrop" on:click=move |_| a11y::defer_close(on_close)>
                <div
                    node_ref=dialog_ref
                    class="folder-picker-dialog template-picker-dialog quip-import-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="quip-import-title"
                    on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                    on:keydown=move |e: web_sys::KeyboardEvent| {
                        if e.key() == "Escape" {
                            a11y::defer_close(on_close);
                            return;
                        }
                        if let Some(node) = dialog_ref.get() {
                            a11y::handle_tab_trap(&e, node.as_ref());
                        }
                    }
                >
                    <div class="confirm-header">
                        <h3 id="quip-import-title">{crate::t!("quip-import-title")}</h3>
                        <button
                            class="toolbar-btn"
                            aria-label=crate::t!("modal-close")
                            on:click=move |_| a11y::defer_close(on_close)
                        >"\u{00D7}"</button>
                    </div>
                    <div class="folder-picker-body template-picker-body quip-import-body">
                        {move || match response.get() {
                            None => view! {
                                // ─── Step 1: token entry ──────────────
                                <div class="quip-import-step-token">
                                    <label class="template-picker-field">
                                        <span class="template-picker-field-key">
                                            {crate::t!("quip-import-token-label")}
                                        </span>
                                        <input
                                            type="password"
                                            class="template-picker-field-input"
                                            data-autofocus="true"
                                            autocomplete="off"
                                            placeholder=crate::t!("quip-import-token-placeholder")
                                            prop:value=move || token.get()
                                            on:input=move |ev| {
                                                set_token.set(event_target_value(&ev));
                                                set_error.set(None);
                                            }
                                            on:keydown=move |ev: web_sys::KeyboardEvent| {
                                                if ev.key() == "Enter" {
                                                    ev.prevent_default();
                                                    do_connect();
                                                }
                                            }
                                        />
                                    </label>
                                    {move || error.get().map(|e| view! {
                                        <div class="template-picker-error" role="alert">
                                            {crate::t!("quip-import-error", err = e)}
                                        </div>
                                    })}
                                    <div class="confirm-actions">
                                        <button
                                            class="btn btn-primary"
                                            disabled=move || connecting.get() || token.get().trim().is_empty()
                                            on:click=move |_| do_connect()
                                        >{move || if connecting.get() {
                                            crate::t!("quip-import-connecting")
                                        } else {
                                            crate::t!("quip-import-connect")
                                        }}</button>
                                    </div>
                                </div>
                            }.into_any(),
                            Some(resp) if !started.get() => {
                                let profile_name = resp.quip_profile.name.clone();
                                let folders = resp.root_folders;
                                view! {
                                    // ─── Step 2: profile + folder scope ───
                                    // `data-import-id` / `data-quip-user-id`
                                    // are Phase 1 hooks (the Continue
                                    // wire-up needs both to kick off the
                                    // import against this connect session)
                                    // and double as a test-automation
                                    // anchor for the Task 9 demo.
                                    <div
                                        class="quip-import-step-scope"
                                        data-import-id=resp.import_id
                                        data-quip-user-id=resp.quip_profile.id
                                    >
                                        <p class="quip-import-profile">
                                            {crate::t!("quip-import-profile", name = profile_name)}
                                        </p>
                                        <h4 class="template-picker-section-title">
                                            {crate::t!("quip-import-folder-scope-heading")}
                                        </h4>
                                        {if folders.is_empty() {
                                            view! {
                                                <div class="template-picker-empty">
                                                    {crate::t!("quip-import-no-folders")}
                                                </div>
                                            }.into_any()
                                        } else {
                                            folders.into_iter().map(|f| {
                                                let fid = f.id.clone();
                                                let fid_checked = f.id.clone();
                                                view! {
                                                    <label class="share-link-opt quip-import-folder-row">
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=move || {
                                                                selected.get().get(&fid_checked).copied().unwrap_or(false)
                                                            }
                                                            on:change=move |ev| {
                                                                let checked = event_target_checked(&ev);
                                                                selected.update(|m| {
                                                                    m.insert(fid.clone(), checked);
                                                                });
                                                            }
                                                        />
                                                        <span>{f.title}</span>
                                                    </label>
                                                }
                                            }).collect::<Vec<_>>().into_any()
                                        }}
                                        <p class="quip-import-target-home">
                                            {crate::t!("quip-import-target-home")}
                                        </p>
                                        {move || error.get().map(|e| view! {
                                            <div class="template-picker-error" role="alert">
                                                {crate::t!("quip-import-error", err = e)}
                                            </div>
                                        })}
                                        <div class="confirm-actions">
                                            <button
                                                class="btn btn-primary"
                                                disabled=move || {
                                                    starting.get()
                                                        || home_folder_id.get().is_none()
                                                        || !selected.get().values().any(|v| *v)
                                                }
                                                on:click=move |_| do_start()
                                            >{move || if starting.get() {
                                                crate::t!("quip-import-starting")
                                            } else {
                                                crate::t!("quip-import-continue")
                                            }}</button>
                                        </div>
                                    </div>
                                }.into_any()
                            }
                            Some(resp) => {
                                // ─── Step 3: live inventory progress ───
                                // `data-quip-import-*` attrs are test-
                                // automation anchors (mirrors step 2's
                                // `data-import-id` / `data-quip-user-id`
                                // convention) for the doctor probe.
                                view! {
                                    <div
                                        class="quip-import-step-progress"
                                        data-import-id=resp.import_id
                                    >
                                        {move || {
                                            if let Some(term) = terminal.get() {
                                                let msg = match term {
                                                    ImportTerminal::TokenExpired => {
                                                        crate::t!("quip-import-token-expired")
                                                    }
                                                    ImportTerminal::Failed => {
                                                        crate::t!("quip-import-import-failed")
                                                    }
                                                };
                                                view! {
                                                    <div
                                                        class="template-picker-error"
                                                        role="alert"
                                                        data-quip-import-terminal="true"
                                                    >
                                                        {msg}
                                                    </div>
                                                }.into_any()
                                            } else {
                                                match progress.get() {
                                                    None => view! {
                                                        <p class="quip-import-progress-line">
                                                            {crate::t!("quip-import-starting")}
                                                        </p>
                                                    }.into_any(),
                                                    Some(st) if st.phase >= 1 => {
                                                        let total = st.progress.total;
                                                        let minutes = total.div_ceil(45);
                                                        view! {
                                                            <p
                                                                class="quip-import-progress-line"
                                                                data-quip-import-total=total
                                                                data-quip-import-done="true"
                                                            >
                                                                {crate::t!(
                                                                    "quip-import-inventory-done",
                                                                    total = total as i64,
                                                                )}
                                                            </p>
                                                            <p class="quip-import-progress-estimate">
                                                                {crate::t!(
                                                                    "quip-import-estimate",
                                                                    minutes = minutes as i64,
                                                                )}
                                                            </p>
                                                        }.into_any()
                                                    }
                                                    Some(st) => {
                                                        let total = st.progress.total;
                                                        view! {
                                                            <p
                                                                class="quip-import-progress-line"
                                                                data-quip-import-total=total
                                                            >
                                                                {crate::t!(
                                                                    "quip-import-scanning",
                                                                    total = total as i64,
                                                                )}
                                                            </p>
                                                        }.into_any()
                                                    }
                                                }
                                            }
                                        }}
                                    </div>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </div>
        </Show>
    }
}

/// `event_target_value` (checkbox flavor) isn't in `leptos::prelude` —
/// same local helper pattern as `calendar_modal.rs` /
/// `spreadsheet_view/sort_dialog.rs`.
fn event_target_checked(e: &web_sys::Event) -> bool {
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.checked())
        .unwrap_or_default()
}
