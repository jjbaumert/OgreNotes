// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// Quip import wizard. Step 1: paste a Quip personal access token,
// POST it to `/imports/quip/connect`, and on success show the
// connected profile + a checklist of the caller's root Quip folders
// (Phase 0). Step 2 (this task): "Continue" persists the checked
// scope + the user's Home folder as the destination *parent* via
// `POST /imports/quip/{id}/start`, then Step 3 polls
// `GET /imports/quip/{id}` on an interval and shows live inventory
// progress until the walk completes (`phase >= 1`) or the run hits a
// terminal failure status.
//
// DESTINATION: Home is the *parent* this wizard sends, not where the
// documents end up. The server creates one dedicated
// `Quip Import — <date>` folder under it per import and lands every
// document in that folder (#172), so undoing a bad import is deleting
// one folder rather than hand-picking documents out of Home. That
// subfolder is what the status poll names (`destinationFolderId`) and
// what the completion step's "Open folder" button opens (#174), and
// it is what `quip-import-target-home` promises the user — Home alone
// would be a false promise.
//
// Mirrors `template_picker_modal.rs` for the modal skeleton (backdrop
// + `<Show when=visible>` + per-open reset) and `share_dialog.rs` for
// the focus trap + checkbox-row list pattern. The Home-folder lookup
// mirrors the `UserMeResponse` local-struct pattern in
// `folder_picker.rs` / `duplicate_dialog.rs` — Phase 1 deliberately
// skips a destination picker (nesting `FolderPickerDialog` inside
// this modal risks focus-trap conflicts), so the parent is always
// Home.
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

/// Everything the completion state renders, derived from one poll's
/// report. Extracted as a plain value (rather than computed inline in
/// the `view!`) so the "a run that lost documents does not look like a
/// clean run" property is testable natively — same shape as
/// `sync_indicator`'s `compute_state`.
///
/// Built only from a report the server actually sent; see
/// [`Completion::from_report`] and the `None` fallback at its call site.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Completion {
    /// Documents that landed. From the report's counter, **not** from
    /// `progress.total` — `progress.total` is every thread the inventory
    /// *discovered*, which includes the ones that were skipped or failed.
    /// Labelling that "Imported N" is precisely the claim this feature
    /// exists to stop making.
    imported: u64,
    /// Documents Quip refused to serve. `None` when there were none —
    /// an absent section renders nothing at all, which is what makes a
    /// lossy run visibly different from a clean one.
    skipped: Option<Section>,
    /// Documents the importer tried and gave up on.
    failed: Option<Section>,
    /// Chat threads, which are counted but never named.
    chat_skipped: u64,
}

/// Which outcome a [`Section`] is describing.
///
/// An enum rather than a pair of key strings threaded through
/// `section_view`: the two sections differ in wording and in their
/// test-automation anchor, and both differences belong in one place where
/// adding a third outcome is a compile error at every site that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeKind {
    /// Quip refused to serve these. The reader may be able to get access.
    Skipped,
    /// The importer tried, retried, and gave up. Retrying already happened.
    Failed,
}

impl OutcomeKind {
    /// Stable `data-quip-import-outcome` value — a test/automation anchor,
    /// never shown to the user (the visible label is [`Self::label`]).
    fn anchor(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }

    /// The section's visible heading, localized.
    fn label(self, count: u64) -> String {
        match self {
            Self::Skipped => crate::t!("quip-import-report-skipped", count = count as i64),
            Self::Failed => crate::t!("quip-import-report-failed", count = count as i64),
        }
    }
}

/// One expandable outcome section: how many, which ones we can name, and
/// how many we cannot.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    /// The true total, from the server's uncapped counter.
    total: u64,
    /// `(quip_thread_id, detail)` — a bounded prefix of `total`. Both are
    /// plain text and are rendered through text nodes.
    named: Vec<(String, String)>,
    /// `total - named.len()`: how many are in the total but unnamed. Any
    /// non-zero value **must** be rendered ("…and N more"); a list that
    /// silently stopped at the note budget would put the original bug
    /// back, one layer up.
    hidden: u64,
}

impl Section {
    /// `None` for an empty outcome, so a clean run has no section to draw.
    fn new(outcome: &imports::Outcome) -> Option<Self> {
        if outcome.total == 0 && outcome.notes.is_empty() {
            return None;
        }
        Some(Self {
            total: outcome.total,
            named: outcome
                .notes
                .iter()
                .map(|n| (n.quip_thread_id.clone(), n.detail.clone()))
                .collect(),
            hidden: outcome.hidden(),
        })
    }
}

impl Completion {
    fn from_report(report: &imports::ImportReport) -> Self {
        Self {
            imported: report.imported,
            skipped: Section::new(&report.skipped),
            failed: Section::new(&report.failed),
            chat_skipped: report.chat_threads_skipped,
        }
    }

    /// True when the run finished with nothing to report beyond the
    /// documents it imported. Drives nothing on its own — it exists so
    /// the difference between a clean run and a lossy one is a single
    /// assertable predicate rather than an eyeball over the `view!`.
    fn is_clean(&self) -> bool {
        self.skipped.is_none() && self.failed.is_none() && self.chat_skipped == 0
    }
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
    // #174: the channel "Open folder" uses to land the user in the import's
    // destination folder. Read here, at setup, because `use_context` must not
    // run from a DOM callback. `None` only when the wizard is mounted outside
    // the shell (no such mount today), which degrades to Home.
    let shell = use_context::<crate::components::app_shell::ShellCtx>();

    let (token, set_token) = signal(String::new());
    let (connecting, set_connecting) = signal(false);
    let (error, set_error) = signal::<Option<String>>(None);
    let (response, set_response) = signal::<Option<ConnectResponse>>(None);
    // Keyed by root-folder id; default-checked once a connect response
    // arrives (see `do_connect`). Read by `do_start` to build the
    // scope for the actual import.
    let selected: RwSignal<HashMap<String, bool>> = RwSignal::new(HashMap::new());

    // Phase 1 state: the destination *parent* (always the user's Home
    // folder; the server files the documents into a dedicated subfolder
    // of it — see the module doc comment), the start-in-flight flag, whether
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
    // the destination parent, then switch into the progress step and start
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
                                // Task 6 lands the content pass, which
                                // advances threads Pending -> ContentDone
                                // (and `record.phase` to `2`) after
                                // inventory (`phase` `1`) completes. The
                                // worker writes a terminal `"succeeded"`
                                // status right AFTER that phase bump, so a
                                // poll can land between the two writes —
                                // completion is keyed off `phase >= 2`,
                                // never off `status`. `succeeded` is
                                // deliberately absent from `is_failure`.
                                let content_done = st.phase >= 2;
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
                                if content_done {
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
                                    <p class="quip-import-token-hint">
                                        {crate::t!("quip-import-token-hint")}
                                        " "
                                        <a
                                            href="https://quip.com/dev/token"
                                            target="_blank"
                                            rel="noopener noreferrer"
                                        >{crate::t!("quip-import-token-hint-link")}</a>
                                    </p>
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
                                                    // phase >= 2: the content pass finished.
                                                    // Keyed off `phase`, not `status`: the
                                                    // terminal `"succeeded"` write lands just
                                                    // after the phase bump, so `status` may
                                                    // still read `"running"`. "Open folder"
                                                    // opens the folder the documents actually
                                                    // landed in — since #172 that is a dedicated
                                                    // `Quip Import — <date>` folder under the
                                                    // caller's Home, which the status poll names
                                                    // (#174). A poll that names no destination
                                                    // (an older server, or an import that never
                                                    // started) falls back to Home, which is where
                                                    // this button always used to go.
                                                    Some(st) if st.phase >= 2 => {
                                                        let total = st.progress.total;
                                                        let destination =
                                                            open_folder_destination(&st);
                                                        // The report is what turns "Imported N
                                                        // items" (N = every thread *discovered*,
                                                        // skipped ones included) into an honest
                                                        // account. When the server has no REPORT
                                                        // row the breakdown genuinely isn't known,
                                                        // so the pre-report line is kept rather
                                                        // than inventing a zero.
                                                        let done = st.report
                                                            .as_ref()
                                                            .map(Completion::from_report);
                                                        view! {
                                                            {match done {
                                                                None => view! {
                                                                    <p
                                                                        class="quip-import-progress-line"
                                                                        data-quip-import-total=total
                                                                        data-quip-import-content-done="true"
                                                                    >
                                                                        {crate::t!(
                                                                            "quip-import-content-done",
                                                                            total = total as i64,
                                                                        )}
                                                                    </p>
                                                                }.into_any(),
                                                                Some(c) => completion_view(c).into_any(),
                                                            }}
                                                            <div class="confirm-actions">
                                                                <button
                                                                    class="btn btn-primary"
                                                                    on:click=move |_| {
                                                                        // Close first, and DEFERRED —
                                                                        // a synchronous close-then-
                                                                        // navigate is this codebase's
                                                                        // modal-close panic ("closure
                                                                        // invoked recursively or after
                                                                        // being dropped"). Ordering
                                                                        // preserved from before #174.
                                                                        a11y::defer_close(on_close);
                                                                        open_import_folder(
                                                                            shell,
                                                                            destination.clone(),
                                                                        );
                                                                    }
                                                                >{crate::t!("quip-import-open-folder")}</button>
                                                            </div>
                                                        }.into_any()
                                                    }
                                                    // phase == 1: inventory is done and the
                                                    // content pass is running — `done` climbs
                                                    // toward `total` for free as threads move
                                                    // Pending -> ContentDone (Task 6).
                                                    Some(st) if st.phase >= 1 => {
                                                        let total = st.progress.total;
                                                        let done = st.progress.done;
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
                                                                    "quip-import-importing",
                                                                    done = done as i64,
                                                                    total = total as i64,
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

/// The completion state's body: what landed, then — only when there is
/// something to say — what did not.
///
/// Skips and failures get their own sections rather than one "problems"
/// list because the two ask different things of the reader: a skip means
/// Quip refused access, which the reader may be able to fix and re-run;
/// a failure means the importer already retried and lost, which they
/// cannot. Chat threads get a third, count-only line — they are not
/// documents, and folding them into "skipped" would inflate a number the
/// reader is meant to act on.
fn completion_view(c: Completion) -> impl IntoView {
    let imported = c.imported;
    let chat = c.chat_skipped;
    view! {
        <p
            class="quip-import-progress-line"
            data-quip-import-content-done="true"
            data-quip-import-imported=imported
        >
            {crate::t!("quip-import-report-imported", imported = imported as i64)}
        </p>
        {c.skipped.map(|s| section_view(s, OutcomeKind::Skipped))}
        {c.failed.map(|s| section_view(s, OutcomeKind::Failed))}
        {(chat > 0).then(|| view! {
            <p class="quip-import-report-chat" data-quip-import-chat-skipped=chat>
                {crate::t!("quip-import-report-chat", count = chat as i64)}
            </p>
        })}
    }
}

/// One collapsed-by-default outcome section.
///
/// Native `<details>`/`<summary>`: keyboard- and screen-reader-accessible
/// with no script and no bundle cost, and collapsed by default so a
/// 25-line list doesn't bury the headline.
///
/// The two strings a note carries — the Quip thread id and the server's
/// `detail` — are interpolated as Leptos child expressions, which become
/// **text nodes**. They are never fed to `inner_html`: `detail` is
/// server-authored prose about a failure and must not be able to inject
/// markup, however sanitized it already is upstream.
fn section_view(s: Section, kind: OutcomeKind) -> impl IntoView {
    let total = s.total;
    let hidden = s.hidden;
    view! {
        <details
            class="quip-import-report-section"
            data-quip-import-outcome=kind.anchor()
            data-quip-import-outcome-total=total
        >
            <summary class="quip-import-report-summary">{kind.label(total)}</summary>
            <ul class="quip-import-report-list">
                {s.named.into_iter().map(|(id, detail)| view! {
                    <li class="quip-import-report-note">
                        {if id.is_empty() {
                            crate::t!("quip-import-report-note-general", detail = detail.as_str())
                        } else {
                            crate::t!(
                                "quip-import-report-note",
                                id = id.as_str(),
                                detail = detail.as_str(),
                            )
                        }}
                    </li>
                }).collect::<Vec<_>>()}
                // The truncation line. Driven by the server's uncapped
                // counter minus what it could name — never by the note
                // list's own length, which stops at a storage budget and
                // would otherwise let the list read as complete.
                {(hidden > 0).then(|| view! {
                    <li
                        class="quip-import-report-more"
                        data-quip-import-outcome-hidden=hidden
                    >
                        {crate::t!("quip-import-report-and-more", count = hidden as i64)}
                    </li>
                })}
            </ul>
        </details>
    }
}

/// Which folder "Open folder" should open for this poll: the destination the
/// server named, or `None` meaning "we were not told — use Home".
///
/// `None` is the honest answer in three cases and they all read the same to
/// the user: a server older than #174 (the field decodes to `None`), an
/// import that never started (no destination was ever chosen), and a server
/// that sent an empty id. Guessing a folder id from anything else — the
/// import id, the picked parent the wizard sent to `start` — would navigate
/// somewhere wrong, which is worse than the Home fallback this button had
/// before.
fn open_folder_destination(status: &imports::StatusResponse) -> Option<String> {
    status
        .destination_folder_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// Take the user to `folder_id`, or to Home when there is none.
///
/// The folder view has no route of its own (folders are in-memory state on
/// the home page), so this goes through the shell's `open_folder` /
/// `requested_folder` channel: run the home page's registered opener when that
/// page is the active outlet, else hand the id over and navigate to `/` so
/// the mounting page opens it. Home — a plain navigate to `/` — is the
/// fallback for every case where we have no folder to open, which is exactly
/// what this button did before #174.
fn open_import_folder(
    shell: Option<crate::components::app_shell::ShellCtx>,
    folder_id: Option<String>,
) {
    match (shell, folder_id) {
        (Some(ctx), Some(folder_id)) => match ctx.open_folder.get_untracked() {
            Some(open) => open.run(folder_id),
            None => {
                ctx.requested_folder.set(folder_id);
                crate::commands::nav_bridge::go("/");
            }
        },
        _ => crate::commands::nav_bridge::go("/"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::imports::{ImportReport, Outcome, ReportNote};

    /// The catalog this component reads its wording from. Asserted against
    /// directly (rather than through `t!`) because `i18n::init` touches
    /// `document` and cannot run in a native test.
    const EN_US: &str = include_str!("../../../locales/en-US/main.ftl");

    fn note(id: &str, detail: &str) -> ReportNote {
        ReportNote {
            quip_thread_id: id.to_string(),
            detail: detail.to_string(),
        }
    }

    fn outcome(total: u64, notes: Vec<ReportNote>) -> Outcome {
        Outcome { total, notes }
    }

    fn clean_report() -> ImportReport {
        ImportReport {
            imported: 47,
            ..ImportReport::default()
        }
    }

    /// **The deliverable.** A run that dropped documents must not summarize
    /// the same as one that dropped none — if the two are indistinguishable,
    /// the whole unit is invisible and the user is told "Imported 47" while
    /// three documents silently vanished.
    #[test]
    fn a_run_with_skips_does_not_summarize_like_a_clean_run() {
        let clean = Completion::from_report(&clean_report());
        let lossy = Completion::from_report(&ImportReport {
            imported: 47,
            skipped: outcome(3, vec![note("qt1", "Quip denied access (HTTP 403)")]),
            ..ImportReport::default()
        });

        assert!(clean.is_clean(), "a report with only imports is clean");
        assert!(!lossy.is_clean(), "a report naming a skipped thread is not");
        assert_ne!(clean, lossy);
        // Same headline number, different body: the difference is entirely
        // the section a clean run does not draw.
        assert_eq!(clean.imported, lossy.imported);
        assert!(clean.skipped.is_none());
        assert!(lossy.skipped.is_some());
    }

    /// A failure is not a skip. They render as separate sections with
    /// separate wording and separate anchors, because the reader's options
    /// differ: a skip may be fixable by getting access in Quip, a failure
    /// has already been retried to exhaustion.
    #[test]
    fn skips_and_failures_do_not_collapse_into_one_bucket() {
        let c = Completion::from_report(&ImportReport {
            imported: 10,
            skipped: outcome(2, vec![note("qt1", "Quip denied access (HTTP 403)")]),
            failed: outcome(1, vec![note("qt9", "Quip returned HTTP 500; gave up")]),
            ..ImportReport::default()
        });

        let skipped = c.skipped.expect("skipped section");
        let failed = c.failed.expect("failed section");
        assert_eq!(skipped.total, 2);
        assert_eq!(failed.total, 1);
        assert_eq!(skipped.named[0].0, "qt1");
        assert_eq!(failed.named[0].0, "qt9");
        assert_ne!(OutcomeKind::Skipped.anchor(), OutcomeKind::Failed.anchor());
    }

    /// The single most important property: the counter is the total, the
    /// note list is a sample. 10 000 inaccessible threads yield 25 notes and
    /// a "…and 9 975 more" line — never a 25-item list that reads complete.
    #[test]
    fn a_truncated_note_list_never_reads_as_complete() {
        let named: Vec<ReportNote> = (0..25)
            .map(|i| note(&format!("qt{i:04}"), "Quip denied access (HTTP 403)"))
            .collect();
        let c = Completion::from_report(&ImportReport {
            imported: 0,
            skipped: outcome(10_000, named),
            ..ImportReport::default()
        });

        let s = c.skipped.expect("skipped section");
        assert_eq!(s.total, 10_000, "the total comes from the uncapped counter");
        assert_eq!(s.named.len(), 25, "the note list stops at the storage budget");
        assert_eq!(
            s.hidden, 9_975,
            "the unnamed remainder must be stated, not swallowed",
        );
        assert!(
            s.hidden > 0,
            "with more skips than notes there is always something to disclose",
        );
    }

    /// The complement: when every skip is named, nothing extra is claimed.
    #[test]
    fn a_complete_note_list_claims_no_hidden_remainder() {
        let c = Completion::from_report(&ImportReport {
            skipped: outcome(2, vec![note("qt1", "403"), note("qt2", "403")]),
            ..ImportReport::default()
        });
        assert_eq!(c.skipped.expect("skipped section").hidden, 0);
    }

    /// A server that ever sent more notes than its counter must not wrap the
    /// remainder into a nonsense total. (`Outcome::hidden` saturates; the
    /// API's `max(counter, notes.len())` makes it unreachable from our own
    /// server, but the client does not get to assume that.)
    #[test]
    fn more_notes_than_the_counter_yields_no_negative_remainder() {
        let c = Completion::from_report(&ImportReport {
            skipped: outcome(0, vec![note("", "a selected folder could not be read")]),
            ..ImportReport::default()
        });
        let s = c.skipped.expect("a note alone is enough to draw the section");
        assert_eq!(s.hidden, 0);
        assert_eq!(s.total, 0);
        assert_eq!(s.named[0].0, "", "a folder-level loss names no thread");
    }

    /// Chat threads are counted, never named, and never folded into
    /// `skipped` — inflating an actionable number with content that was
    /// never in scope would make the skip count useless.
    #[test]
    fn chat_threads_are_counted_separately_from_skips() {
        let c = Completion::from_report(&ImportReport {
            imported: 5,
            chat_threads_skipped: 12,
            ..ImportReport::default()
        });
        assert_eq!(c.chat_skipped, 12);
        assert!(c.skipped.is_none(), "chats are not access-denied skips");
        assert!(!c.is_clean(), "12 unimported chat threads is worth saying");
    }

    // ─── #174: where "Open folder" goes ────────────────────────

    /// A finished-import status as the server sends it. Built from wire JSON
    /// rather than a struct literal on purpose: the field *name*, and the
    /// behaviour when it is missing entirely, are the contract this button
    /// rests on — a struct literal would test neither.
    fn finished_status(destination: Option<serde_json::Value>) -> imports::StatusResponse {
        let mut body = serde_json::json!({
            "status": "succeeded",
            "phase": 2,
            "progress": { "done": 3, "total": 3, "stage": "content" },
            "report": null,
        });
        if let Some(d) = destination {
            body["destinationFolderId"] = d;
        }
        serde_json::from_value(body).expect("a status response must decode")
    }

    /// **The deliverable.** "Open folder" must open the folder the documents
    /// landed in — since #172 a dedicated per-import folder — not Home, where
    /// the user then has to hunt for it.
    #[test]
    fn open_folder_targets_the_folder_the_documents_landed_in() {
        let status = finished_status(Some(serde_json::json!("folder-quip-import-1")));
        assert_eq!(
            open_folder_destination(&status).as_deref(),
            Some("folder-quip-import-1"),
        );
    }

    /// A server that predates #174 sends no such field at all. That must
    /// decode — a hard error here would break the whole progress poll mid
    /// rolling deploy — and read as "no destination", i.e. Home.
    #[test]
    fn a_status_from_a_server_without_the_field_decodes_and_means_home() {
        let status = finished_status(None);
        assert_eq!(status.phase, 2, "the rest of the poll must still decode");
        assert_eq!(open_folder_destination(&status), None);
    }

    /// An import that was never started has an explicit `null` destination.
    /// Same handling as a missing field: Home, never a guess.
    #[test]
    fn an_explicitly_null_destination_means_home() {
        let status = finished_status(Some(serde_json::Value::Null));
        assert_eq!(open_folder_destination(&status), None);
    }

    /// A blank id is not a folder. Navigating on one would produce a lookup
    /// for the empty id — an error banner instead of a destination.
    #[test]
    fn a_blank_destination_is_not_a_folder() {
        for blank in ["", "   "] {
            let status = finished_status(Some(serde_json::json!(blank)));
            assert_eq!(
                open_folder_destination(&status),
                None,
                "a blank destination ({blank:?}) must fall back to Home",
            );
        }
    }

    /// Every string this component renders must exist in the catalog with
    /// the argument name the component actually passes — a mismatched
    /// placeable renders as the raw key or drops the number.
    #[test]
    fn every_report_string_exists_with_the_argument_it_is_given() {
        for (key, arg) in [
            ("quip-import-report-imported", "$imported"),
            ("quip-import-report-skipped", "$count"),
            ("quip-import-report-failed", "$count"),
            ("quip-import-report-and-more", "$count"),
            ("quip-import-report-chat", "$count"),
            ("quip-import-report-note", "$detail"),
            ("quip-import-report-note-general", "$detail"),
        ] {
            let line = EN_US
                .lines()
                .find(|l| l.starts_with(&format!("{key} =")))
                .unwrap_or_else(|| panic!("en-US catalog is missing {key}"));
            assert!(
                line.contains(arg),
                "{key} must interpolate {arg}; got {line:?}",
            );
        }
        // The id-bearing note form needs both placeables.
        let note_line = EN_US
            .lines()
            .find(|l| l.starts_with("quip-import-report-note ="))
            .expect("quip-import-report-note");
        assert!(note_line.contains("$id"), "got {note_line:?}");
    }
}
