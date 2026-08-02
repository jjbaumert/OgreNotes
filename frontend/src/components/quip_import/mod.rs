// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// Quip import wizard. Step 1: paste a Quip personal access token,
// POST it to `/imports/quip/connect`, and on success show the
// connected profile + a checklist of the caller's root Quip folders
// (Phase 0). Step 2: "Continue" persists the checked scope + the
// chosen destination *parent* via `POST /imports/quip/{id}/start`,
// then Step 3 polls `GET /imports/quip/{id}` on an interval and shows
// live inventory progress until the walk completes (`phase >= 1`) or
// the run hits a terminal failure status.
//
// DESTINATION: the parent this wizard sends is not where the
// documents end up. The server creates one dedicated
// `Quip Import — <date>` folder under it per import and lands every
// document in that folder (#172), so undoing a bad import is deleting
// one folder rather than hand-picking documents out of the parent.
// That subfolder is what the status poll names
// (`destinationFolderId`) and what the completion step's "Open
// folder" button opens (#174), and it is what
// `quip-import-target-home` / `quip-import-target-folder` promise the
// user — naming the parent alone would be a false promise.
//
// The parent defaults to the caller's Home and stays Home for a user
// who never opens the destination step (#236 Unit 3). Choosing one is
// a *step of this wizard*, not a nested dialog: `FolderPickerDialog`
// is itself a modal with its own focus trap, and mounting it inside
// this modal's trap is the documented reason Phase 1 shipped without a
// picker at all. Step 2 swaps its own body for a folder tree built
// from `components::folder_tree` — the picker's data, none of its
// shell — so there is only ever one trap, one `role="dialog"`, and one
// Escape handler on screen. Every transition into and out of that
// step goes through `a11y::defer`, because flipping the signal that
// owns the subtree from inside its own `on:click` is this codebase's
// modal-close panic ("closure invoked recursively or after being
// dropped").
//
// Mirrors `template_picker_modal.rs` for the modal skeleton (backdrop
// + `<Show when=visible>` + per-open reset) and `share_dialog.rs` for
// the focus trap + checkbox-row list pattern. The Home-folder lookup
// mirrors the `UserMeResponse` local-struct pattern in
// `folder_picker.rs` / `duplicate_dialog.rs`.
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

use std::collections::{HashMap, HashSet};

use leptos::prelude::*;

use wasm_bindgen::JsCast;

use crate::a11y;
use crate::api::client;
use crate::api::folders::{self, FolderResponse};
use crate::api::imports::{self, ConnectResponse};
use crate::components::folder_tree::{self, FolderTreeRow};

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
    /// Attachments that did not come across, in documents that did. Counts
    /// **images**, not documents.
    images_dropped: Option<Section>,
    /// Documents whose deep nesting was flattened. Counts documents.
    content_truncated: Option<Section>,
    /// Documents whose @mentions became plain text. Counts documents.
    mentions_degraded: Option<Section>,
    /// Embedded live-app blocks that did not come across. Counts **blocks**.
    live_apps_dropped: Option<Section>,
    /// Spreadsheet formulas that did not come across. Counts **formulas**.
    spreadsheet_formulas_dropped: Option<Section>,
}

/// Which outcome a [`Section`] is describing.
///
/// An enum rather than a set of key strings threaded through
/// `section_view`: the sections differ in wording, in their
/// test-automation anchor, and in which tier they belong to, and all three
/// differences belong in one place where adding an outcome is a compile
/// error at every site that matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeKind {
    /// Quip refused to serve these. The reader may be able to get access.
    Skipped,
    /// The importer tried, retried, and gave up. Retrying already happened.
    Failed,
    /// Attachments that could not be copied out of Quip.
    ImagesDropped,
    /// Nesting flattened below the walker's depth cap.
    ContentTruncated,
    /// @mentions that lost their link and became plain text.
    MentionsDegraded,
    /// Embedded Quip live apps whose contents were not converted.
    LiveAppsDropped,
    /// Spreadsheet formulas that were not imported.
    SpreadsheetFormulasDropped,
}

/// Which tier of the completion screen an [`OutcomeKind`] belongs to.
///
/// The screen has to carry seven outcomes and they are not equally
/// important: a document that never arrived is a different order of loss
/// from a picture that did not come with one that did. Flattening all seven
/// into one list would make the reader rank them, and the ranking is
/// exactly what we know and they do not.
///
/// Both tiers show their counts unconditionally. The tier is a *heading*,
/// not a disclosure — nothing here is collapsed behind an extra click, and
/// in particular no whole-document loss is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeTier {
    /// A document is missing from the destination folder. Nothing the user
    /// does inside OgreNotes will find it; the remedy, if any, is in Quip.
    Documents,
    /// The document arrived and something inside it did not. The user has
    /// the document and can see what is there — this tier tells them what
    /// to look for.
    WithinDocuments,
}

impl OutcomeKind {
    /// Stable `data-quip-import-outcome` value — a test/automation anchor,
    /// never shown to the user (the visible label is [`Self::label`]).
    fn anchor(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::Failed => "failed",
            Self::ImagesDropped => "images-dropped",
            Self::ContentTruncated => "content-truncated",
            Self::MentionsDegraded => "mentions-degraded",
            Self::LiveAppsDropped => "live-apps-dropped",
            Self::SpreadsheetFormulasDropped => "spreadsheet-formulas-dropped",
        }
    }

    /// The section's visible heading, localized.
    ///
    /// Each kind gets its own string rather than a shared "N problems"
    /// template: the *unit* differs (images vs. documents) and so does the
    /// remedy, and a reader who cannot tell "8 pictures" from "8 documents"
    /// has been told a number and nothing else.
    fn label(self, count: u64) -> String {
        match self {
            Self::Skipped => crate::t!("quip-import-report-skipped", count = count as i64),
            Self::Failed => crate::t!("quip-import-report-failed", count = count as i64),
            Self::ImagesDropped => {
                crate::t!("quip-import-report-images", count = count as i64)
            }
            Self::ContentTruncated => {
                crate::t!("quip-import-report-truncated", count = count as i64)
            }
            Self::MentionsDegraded => {
                crate::t!("quip-import-report-mentions", count = count as i64)
            }
            Self::LiveAppsDropped => {
                crate::t!("quip-import-report-live-apps", count = count as i64)
            }
            Self::SpreadsheetFormulasDropped => {
                crate::t!("quip-import-report-formulas", count = count as i64)
            }
        }
    }

    /// Which tier this kind is grouped under. See [`OutcomeTier`].
    fn tier(self) -> OutcomeTier {
        match self {
            Self::Skipped | Self::Failed => OutcomeTier::Documents,
            Self::ImagesDropped
            | Self::ContentTruncated
            | Self::MentionsDegraded
            | Self::LiveAppsDropped
            | Self::SpreadsheetFormulasDropped => OutcomeTier::WithinDocuments,
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

    /// The two numbers this section renders: the count on its heading, and
    /// the "…and N more" count (`None` when nothing is unnamed).
    ///
    /// Pulled out of the `view!` so **the one property this whole feature
    /// rests on is assertable natively**: both numbers come from the
    /// server's uncapped counter, and neither is `named.len()`. There is no
    /// DOM harness in this crate, so a number computed inline in the markup
    /// would be checkable only by eye — and "the list silently stopped at
    /// 25" is precisely the bug that survives an eyeball.
    fn display_counts(&self) -> (u64, Option<u64>) {
        (self.total, (self.hidden > 0).then_some(self.hidden))
    }

    /// `None` for an absent outcome, collapsing "the server sent nothing"
    /// and "the server sent an empty outcome" to the same rendering.
    ///
    /// The server since #208 sends `null` for a kind that did not occur, so
    /// the empty-but-present case should not arise from our own backend —
    /// but a client does not get to assume its server's version, and a zero
    /// section drawn on a clean run would be a new noise source rather than
    /// a new signal.
    fn from_optional(outcome: Option<&imports::Outcome>) -> Option<Self> {
        outcome.and_then(Section::new)
    }
}

impl Completion {
    fn from_report(report: &imports::ImportReport) -> Self {
        Self {
            imported: report.imported,
            skipped: Section::new(&report.skipped),
            failed: Section::new(&report.failed),
            chat_skipped: report.chat_threads_skipped,
            images_dropped: Section::from_optional(report.images_dropped.as_ref()),
            content_truncated: Section::from_optional(report.content_truncated.as_ref()),
            mentions_degraded: Section::from_optional(report.mentions_degraded.as_ref()),
            live_apps_dropped: Section::from_optional(report.live_apps_dropped.as_ref()),
            spreadsheet_formulas_dropped: Section::from_optional(
                report.spreadsheet_formulas_dropped.as_ref(),
            ),
        }
    }

    /// The sections of one tier, in a fixed order, skipping the ones that
    /// did not happen.
    ///
    /// Order is severity-descending within each tier and is deliberate: a
    /// document Quip refused is more actionable than one that failed; a
    /// missing Kanban board or a dead formula is a bigger hole in a document
    /// than a flattened list or an unlinked name.
    fn sections(&self, tier: OutcomeTier) -> Vec<(OutcomeKind, Section)> {
        [
            (OutcomeKind::Skipped, &self.skipped),
            (OutcomeKind::Failed, &self.failed),
            (OutcomeKind::LiveAppsDropped, &self.live_apps_dropped),
            (
                OutcomeKind::SpreadsheetFormulasDropped,
                &self.spreadsheet_formulas_dropped,
            ),
            (OutcomeKind::ImagesDropped, &self.images_dropped),
            (OutcomeKind::ContentTruncated, &self.content_truncated),
            (OutcomeKind::MentionsDegraded, &self.mentions_degraded),
        ]
        .into_iter()
        .filter(|(kind, _)| kind.tier() == tier)
        .filter_map(|(kind, section)| section.clone().map(|s| (kind, s)))
        .collect()
    }

    /// True when the run finished with nothing to report beyond the
    /// documents it imported. Drives nothing on its own — it exists so
    /// the difference between a clean run and a lossy one is a single
    /// assertable predicate rather than an eyeball over the `view!`.
    ///
    /// Every kind counts here, not only the whole-document ones. A run that
    /// dropped 400 images is not a clean run, and this predicate is the
    /// codebase's one-line answer to "did this import lose anything".
    fn is_clean(&self) -> bool {
        self.sections(OutcomeTier::Documents).is_empty()
            && self.sections(OutcomeTier::WithinDocuments).is_empty()
            && self.chat_skipped == 0
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
    // #236 Unit 3: the destination parent the user picked, as
    // `(folder id, folder title)`. `None` — the value a wizard that is
    // opened and driven straight through never leaves — means Home, exactly
    // as before this step existed.
    let destination: RwSignal<Option<(String, String)>> = RwSignal::new(None);
    // Whether step 2 is currently showing the folder tree instead of the
    // scope checklist. A sub-step of this modal, never a modal of its own.
    let (picking_destination, set_picking_destination) = signal(false);
    // The destination step's slice of the picker's data layer: every folder
    // visited so far, which of them are expanded, and the row the user has
    // highlighted but not yet confirmed. `dest_selected` is deliberately
    // separate from `destination` — highlighting a row must not change where
    // the import goes until "Use this folder" is pressed, so Cancel is a real
    // cancel.
    let dest_folders: RwSignal<HashMap<String, FolderResponse>> = RwSignal::new(HashMap::new());
    let dest_expanded: RwSignal<HashSet<String>> = RwSignal::new(HashSet::new());
    let dest_selected: RwSignal<Option<String>> = RwSignal::new(None);
    // Where the import is going, right now — the single derivation both the
    // `start` call and the DOM anchor read. See [`effective_target`].
    let target_folder_id = effective_target(destination, home_folder_id);
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
        // A fresh open starts on Home again, and re-fetches the tree rather
        // than showing a cache that predates any folder the user has created
        // since. Resetting `picking_destination` matters most: a wizard
        // reopened onto a half-finished destination step would show a tree
        // with no scope behind it.
        destination.set(None);
        set_picking_destination.set(false);
        dest_folders.set(HashMap::new());
        dest_expanded.set(HashSet::new());
        dest_selected.set(None);
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

    // ─── Destination step (#236 Unit 3) ──────────────────────────
    //
    // Lazy per-folder loading, same as `FolderPickerDialog`: a folder's
    // children are fetched the first time it is expanded rather than walking
    // the whole tree up front. A failed fetch goes to the wizard's existing
    // error banner (`ApiClientError`'s `Display` is opaque — see
    // `do_connect`), never to a silent empty branch.
    let load_dest_folder = move |id: String| {
        if dest_folders.with_untracked(|m| m.contains_key(&id)) {
            return;
        }
        leptos::task::spawn_local(async move {
            match folders::get_folder(&id).await {
                Ok(f) => dest_folders.update(|m| {
                    m.insert(id.clone(), f);
                }),
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    };

    let toggle_dest_expand = move |id: String| {
        if dest_expanded.with_untracked(|s| s.contains(&id)) {
            dest_expanded.update(|s| {
                s.remove(&id);
            });
            return;
        }
        dest_expanded.update(|s| {
            s.insert(id.clone());
        });
        load_dest_folder(id);
    };

    // Enter the destination step. The tree is rooted at Home — the same root
    // `FolderPickerDialog` uses — so the step cannot be entered before the
    // `/users/me` lookup lands; the "Change" button is disabled until then,
    // and this guard is the second half of that.
    //
    // Opens highlighted on the destination currently in effect, so the
    // primary button is live immediately and pressing it is a no-op rather
    // than a trap the user has to work out.
    let open_destination_step = move || {
        let Some(home) = home_folder_id.get_untracked() else {
            return;
        };
        dest_expanded.update(|s| {
            s.insert(home.clone());
        });
        load_dest_folder(home.clone());
        dest_selected.set(Some(
            destination
                .get_untracked()
                .map(|(id, _)| id)
                .unwrap_or(home),
        ));
        set_picking_destination.set(true);
    };

    // Leave the step, keeping whatever destination was already in effect.
    let cancel_destination_step = move || set_picking_destination.set(false);

    // Leave the step, adopting the highlighted row. The title comes from the
    // fetched folder rather than the row that was clicked so the label the
    // user then reads is the server's name for the folder, not a stale one.
    let confirm_destination_step = move || {
        let Some(id) = dest_selected.get_untracked() else {
            return;
        };
        let Some(title) = dest_folders.with_untracked(|m| m.get(&id).map(|f| f.title.clone()))
        else {
            return;
        };
        destination.set(Some((id, title)));
        set_picking_destination.set(false);
    };

    // Kick off the actual import: persist the checked scope + the chosen
    // parent (Home when the user never chose one), then switch into the
    // progress step and start polling. `start`'s failure path reuses the same `error` signal
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
        // The destination becomes a wire value. Home when the user never
        // opened the destination step; `None` only when the `/users/me`
        // lookup has not landed, which also keeps Continue disabled —
        // starting an import with no parent at all would 400.
        let Some(target) = target_folder_id.get_untracked() else {
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
            match imports::start(&import_id, &roots, &target).await {
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
                                let root_folders = resp.root_folders;
                                let import_id_attr = resp.import_id;
                                let quip_user_attr = resp.quip_profile.id;
                                // Step 2 has two faces and exactly one is
                                // mounted at a time: the scope checklist, or
                                // the destination tree. Swapping them inside
                                // the wizard's own body — rather than mounting
                                // `FolderPickerDialog` over it — is what keeps
                                // this to one focus trap, one `role="dialog"`,
                                // and one Escape handler (#236 Unit 3).
                                view! {
                                {move || {
                                if picking_destination.get() {
                                return view! {
                                    // ─── Step 2b: destination tree ────────
                                    <div
                                        class="quip-import-step-destination"
                                        data-quip-import-step="destination"
                                    >
                                        <h4 class="template-picker-section-title">
                                            {crate::t!("quip-import-destination-heading")}
                                        </h4>
                                        <p class="quip-import-destination-hint">
                                            {crate::t!("quip-import-destination-hint")}
                                        </p>
                                        <div class="folder-picker-body">
                                            {move || {
                                                let mut rows: Vec<FolderTreeRow> = Vec::new();
                                                if let Some(root) = home_folder_id.get() {
                                                    dest_folders.with(|map| {
                                                        dest_expanded.with(|set| {
                                                            folder_tree::flatten_tree(
                                                                &root, map, set, &mut rows, 0,
                                                            );
                                                        });
                                                    });
                                                }
                                                // A single not-yet-loaded root is
                                                // "still fetching", not a tree.
                                                if rows.first().map(|r| !r.is_loaded).unwrap_or(true) {
                                                    return view! {
                                                        <p class="folder-picker-empty">
                                                            {crate::t!("common-loading")}
                                                        </p>
                                                    }.into_any();
                                                }
                                                view! {
                                                    <ul class="folder-picker-tree">
                                                        {rows.into_iter().map(|row| {
                                                            let row_id = row.id.clone();
                                                            let row_id_for_toggle = row_id.clone();
                                                            let is_selected =
                                                                dest_selected.get() == Some(row_id.clone());
                                                            let indent = format!(
                                                                "padding-inline-start: {}px",
                                                                (row.depth as u16) * 16 + 8,
                                                            );
                                                            let disabled = !row.is_selectable();
                                                            let has_children = row.has_children;
                                                            let chevron = if !has_children {
                                                                ""
                                                            } else if row.is_expanded {
                                                                "\u{25BE}"
                                                            } else {
                                                                "\u{25B8}"
                                                            };
                                                            let row_title = if row.is_loaded {
                                                                row.title.clone()
                                                            } else {
                                                                crate::t!("common-loading")
                                                            };
                                                            view! {
                                                                <li
                                                                    class="folder-picker-row"
                                                                    class:selected=is_selected
                                                                    class:disabled=disabled
                                                                    style=indent
                                                                    on:click=move |_| {
                                                                        if disabled { return; }
                                                                        dest_selected
                                                                            .set(Some(row_id.clone()));
                                                                    }
                                                                >
                                                                    <span
                                                                        class="folder-picker-chevron"
                                                                        // Deferred: expanding
                                                                        // re-renders this whole
                                                                        // list, dropping the very
                                                                        // closure that is running.
                                                                        on:click=move |e: web_sys::MouseEvent| {
                                                                            e.stop_propagation();
                                                                            if !has_children { return; }
                                                                            let id = row_id_for_toggle.clone();
                                                                            a11y::defer(move || toggle_dest_expand(id));
                                                                        }
                                                                    >
                                                                        {chevron}
                                                                    </span>
                                                                    <span class="folder-picker-icon">
                                                                        "\u{1F4C1}"
                                                                    </span>
                                                                    <span class="folder-picker-title">
                                                                        {row_title}
                                                                    </span>
                                                                </li>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </ul>
                                                }.into_any()
                                            }}
                                        </div>
                                        {move || error.get().map(|e| view! {
                                            <div class="template-picker-error" role="alert">
                                                {crate::t!("quip-import-error", err = e)}
                                            </div>
                                        })}
                                        <div class="confirm-actions">
                                            <button
                                                class="btn btn-secondary"
                                                // Both actions unmount this step
                                                // from inside their own handler —
                                                // the same reason every close path
                                                // in this modal defers.
                                                on:click=move |_| a11y::defer(cancel_destination_step)
                                            >
                                                {crate::t!("common-cancel")}
                                            </button>
                                            <button
                                                class="btn btn-primary"
                                                disabled=move || dest_selected.get().is_none()
                                                on:click=move |_| a11y::defer(confirm_destination_step)
                                            >
                                                {crate::t!("quip-import-destination-select")}
                                            </button>
                                        </div>
                                    </div>
                                }.into_any();
                                }
                                let profile_name = profile_name.clone();
                                let folders = root_folders.clone();
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
                                        data-import-id=import_id_attr.clone()
                                        data-quip-user-id=quip_user_attr.clone()
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
                                        // Where the import will land, and the
                                        // way into changing it. The default —
                                        // the wording a user who never presses
                                        // "Change" sees — is the Home promise
                                        // this line has always made.
                                        //
                                        // `data-quip-import-target` carries the
                                        // id that `do_start` will actually send,
                                        // derived from the same
                                        // `start_target_folder_id` call, so an
                                        // automation anchor and the wire value
                                        // cannot drift apart.
                                        <div class="quip-import-destination-row">
                                            <p
                                                class="quip-import-target-home"
                                                data-quip-import-target=move || {
                                                    target_folder_id.get().unwrap_or_default()
                                                }
                                            >
                                                {move || match destination.get() {
                                                    None => crate::t!("quip-import-target-home"),
                                                    Some((_, title)) => crate::t!(
                                                        "quip-import-target-folder",
                                                        folder = title,
                                                    ),
                                                }}
                                            </p>
                                            <button
                                                class="btn btn-secondary quip-import-destination-change"
                                                // The tree is rooted at Home, so
                                                // there is nothing to show until
                                                // `/users/me` lands.
                                                disabled=move || home_folder_id.get().is_none()
                                                // Deferred: this flips the signal
                                                // that owns the subtree the button
                                                // itself lives in. Tearing that
                                                // down inside its own `on:click`
                                                // is the "closure invoked
                                                // recursively or after being
                                                // dropped" panic.
                                                on:click=move |_| a11y::defer(open_destination_step)
                                            >
                                                {crate::t!("quip-import-destination-change")}
                                            </button>
                                        </div>
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
                                }}
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
/// Each kind gets its own section rather than one "problems" list because
/// they ask different things of the reader: a skip means Quip refused
/// access, which the reader may be able to fix and re-run; a failure means
/// the importer already retried and lost, which they cannot; a dropped
/// image means go and look at a document that is otherwise fine. Chat
/// threads get a count-only line — they are not documents, and folding them
/// into "skipped" would inflate a number the reader is meant to act on.
///
/// **Two tiers, seven sections (#208).** The whole-document losses come
/// first, ungrouped and unheaded — they are the headline, and the reader
/// must not have to get past a heading to reach them. The within-document
/// losses follow under their own heading, which appears only when at least
/// one of them happened. That heading is the only concession to the fact
/// that seven sections is a lot of screen: it groups, it does not hide.
/// Every section's count is on its `<summary>`, visible without expanding
/// anything; only the *named examples* are behind the disclosure. No
/// whole-document loss is ever collapsed out of view.
fn completion_view(c: Completion) -> impl IntoView {
    let imported = c.imported;
    let chat = c.chat_skipped;
    let documents = c.sections(OutcomeTier::Documents);
    let within = c.sections(OutcomeTier::WithinDocuments);
    view! {
        <p
            class="quip-import-progress-line"
            data-quip-import-content-done="true"
            data-quip-import-imported=imported
        >
            {crate::t!("quip-import-report-imported", imported = imported as i64)}
        </p>
        {documents.into_iter().map(|(kind, s)| section_view(s, kind)).collect::<Vec<_>>()}
        {(chat > 0).then(|| view! {
            <p class="quip-import-report-chat" data-quip-import-chat-skipped=chat>
                {crate::t!("quip-import-report-chat", count = chat as i64)}
            </p>
        })}
        // The heading is drawn only alongside the sections it heads: a
        // standing "some content didn't come across" over an empty region
        // would announce a loss on every clean run.
        {(!within.is_empty()).then(|| view! {
            <p
                class="quip-import-report-group-heading"
                data-quip-import-report-group="within-documents"
            >
                {crate::t!("quip-import-report-within-heading")}
            </p>
            {within.into_iter().map(|(kind, s)| section_view(s, kind)).collect::<Vec<_>>()}
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
    // Both numbers from `display_counts`, which is counter-derived and
    // pinned by test — never from `s.named.len()`, which stops at the
    // storage row's per-kind budget.
    let (total, hidden) = s.display_counts();
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
                {hidden.map(|hidden| view! {
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
/// The `target_folder_id` a `POST /imports/quip/{id}/start` must carry: the
/// **parent** the import's `Quip Import — <date>` folder is created beneath
/// (#172), not where documents land.
///
/// `chosen` is the folder the user picked in the destination step, `home` the
/// caller's Home folder from `/users/me`. The rules, in the order they matter:
///
/// - A picked folder wins. This is the whole of #236 Unit 3 — the server
///   already authorizes whatever id arrives here (`check_folder_access(...,
///   Edit)` in `routes/imports.rs`), so sending the user's choice is both the
///   feature and the thing the access check is run against.
/// - Otherwise Home, which is what this wizard sent unconditionally before the
///   destination step existed. A user who never opens that step must be
///   indistinguishable from that user.
/// - Blank is not an id. An empty or whitespace-only choice falls through to
///   Home rather than being sent, for the same reason
///   [`open_folder_destination`] refuses to navigate on one.
/// - `None` — no choice and no Home — means there is nothing to send. The
///   caller must not start: an import with no parent is a 400, and the
///   Continue button is disabled in exactly this state.
fn start_target_folder_id(chosen: Option<&str>, home: Option<&str>) -> Option<String> {
    fn usable(id: Option<&str>) -> Option<String> {
        id.map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
    }
    usable(chosen).or_else(|| usable(home))
}

/// The wizard's live destination, as one derived signal over the two signals
/// that determine it: the folder the user picked, and the Home folder from
/// `/users/me`.
///
/// A function over signals rather than a closure inside the component so that
/// **the read is testable, not just the rule**. `start_target_folder_id` alone
/// leaves a gap no test of it can see: a call site that applies the rule
/// correctly but feeds it the wrong signal — `None` where the user's choice
/// belongs — sends every import to Home and passes every test of the pure
/// function. Pulling the reads in here closes that; the construction
/// `QuipImportWizard` uses is the one
/// `the_wizards_live_destination_follows_the_users_choice` drives.
///
/// One derivation, two consumers, deliberately: the `data-quip-import-target`
/// attribute reads it tracked so the anchor re-renders, `do_start` reads it
/// untracked at the moment of the click. They cannot disagree about where the
/// import is going.
fn effective_target(
    destination: RwSignal<Option<(String, String)>>,
    home_folder_id: ReadSignal<Option<String>>,
) -> Signal<Option<String>> {
    Signal::derive(move || {
        start_target_folder_id(
            destination.get().as_ref().map(|(id, _)| id.as_str()),
            home_folder_id.get().as_deref(),
        )
    })
}

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

    // ─── #208: every recorded loss is a visible loss ───────────

    /// **The deliverable.** A run that lost images, flattened nesting,
    /// degraded mentions, dropped a Kanban board or dropped formulas must
    /// show each of them, with its own count and its own wording. Before
    /// #208 all five reached the user only as an increment to `notesDropped`
    /// — a number with no explanation attached.
    #[test]
    fn every_within_document_loss_gets_its_own_section() {
        let c = Completion::from_report(&ImportReport {
            imported: 47,
            images_dropped: Some(outcome(8, vec![note("qi1", "image blob-9: not stored")])),
            content_truncated: Some(outcome(3, vec![note("qc1", "nesting flattened")])),
            mentions_degraded: Some(outcome(5, vec![note("qm1", "lookup rejected")])),
            live_apps_dropped: Some(outcome(2, vec![note("ql1", "2 live app(s) dropped")])),
            spreadsheet_formulas_dropped: Some(outcome(
                300,
                vec![note("qs1", "300 formula(s) not imported")],
            )),
            ..ImportReport::default()
        });

        let within = c.sections(OutcomeTier::WithinDocuments);
        assert_eq!(within.len(), 5, "all five kinds must be drawn");
        assert_eq!(
            within.iter().map(|(k, s)| (*k, s.total)).collect::<Vec<_>>(),
            vec![
                (OutcomeKind::LiveAppsDropped, 2),
                (OutcomeKind::SpreadsheetFormulasDropped, 300),
                (OutcomeKind::ImagesDropped, 8),
                (OutcomeKind::ContentTruncated, 3),
                (OutcomeKind::MentionsDegraded, 5),
            ],
            "each kind keeps its own count — a shared total would be a new lie",
        );
        // Distinct anchors and distinct wording: two sections that read the
        // same are one section with the wrong number on it twice.
        let anchors: Vec<&str> = within.iter().map(|(k, _)| k.anchor()).collect();
        let mut deduped = anchors.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(deduped.len(), anchors.len(), "anchors must be distinct");
        assert!(!c.is_clean(), "a run that lost 8 images is not a clean run");
    }

    /// The tiering. Whole-document losses and within-document losses are
    /// different orders of loss, and the screen says so — but a lost
    /// *document* is never demoted into the second tier, where a reader
    /// scanning the headline could miss it.
    #[test]
    fn a_lost_document_is_never_grouped_with_a_lost_picture() {
        let c = Completion::from_report(&ImportReport {
            imported: 47,
            skipped: outcome(2, vec![note("qt1", "403")]),
            failed: outcome(1, vec![note("qt9", "gave up")]),
            images_dropped: Some(outcome(8, vec![note("qi1", "not stored")])),
            ..ImportReport::default()
        });

        let documents = c.sections(OutcomeTier::Documents);
        assert_eq!(
            documents.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            vec![OutcomeKind::Skipped, OutcomeKind::Failed],
            "the whole-document losses are the first tier, in severity order",
        );
        assert_eq!(
            c.sections(OutcomeTier::WithinDocuments)
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>(),
            vec![OutcomeKind::ImagesDropped],
        );
        // Every kind lands in exactly one tier, so nothing can be dropped
        // from the screen by a tier that forgot it.
        for kind in [
            OutcomeKind::Skipped,
            OutcomeKind::Failed,
            OutcomeKind::ImagesDropped,
            OutcomeKind::ContentTruncated,
            OutcomeKind::MentionsDegraded,
            OutcomeKind::LiveAppsDropped,
            OutcomeKind::SpreadsheetFormulasDropped,
        ] {
            assert!(
                matches!(
                    kind.tier(),
                    OutcomeTier::Documents | OutcomeTier::WithinDocuments
                ),
                "{kind:?} must belong to a tier",
            );
        }
    }

    /// **The truncation property, on the new kinds.** 4 000 dropped images
    /// yield 25 notes and a "…and 3 975 more" line. The total comes from the
    /// server's counter; a section that reported `named.len()` would tell a
    /// user who lost 4 000 images that 25 were lost — the original silence
    /// with a smaller number on it.
    #[test]
    fn a_truncated_within_document_list_never_reads_as_complete() {
        let named: Vec<ReportNote> = (0..25)
            .map(|i| note(&format!("qi{i:04}"), "image blob-9: Quip denied access"))
            .collect();
        let c = Completion::from_report(&ImportReport {
            imported: 47,
            images_dropped: Some(outcome(4_000, named)),
            ..ImportReport::default()
        });

        let s = c.images_dropped.expect("images section");
        assert_eq!(s.total, 4_000, "the total comes from the uncapped counter");
        assert_eq!(s.named.len(), 25, "the note list stops at the storage budget");
        assert_eq!(
            s.hidden, 3_975,
            "the unnamed remainder must be stated, not swallowed",
        );
        // And the two numbers the section actually renders — the heading's
        // count and the "…and N more" count — are both the counter's, not
        // the list's. `section_view` reads exactly this pair.
        assert_eq!(
            s.display_counts(),
            (4_000, Some(3_975)),
            "both rendered numbers must be counter-derived",
        );
        let (heading, more) = s.display_counts();
        assert_ne!(
            heading,
            s.named.len() as u64,
            "the heading must be the true total, not the sample size",
        );
        assert!(more.is_some(), "a truncated list must disclose its remainder");
    }

    /// The complement, on a new kind: when every loss is named, no "…and N
    /// more" line is drawn. Claiming a hidden remainder that does not exist
    /// would be its own small lie.
    #[test]
    fn a_complete_within_document_list_renders_no_remainder_line() {
        let c = Completion::from_report(&ImportReport {
            content_truncated: Some(outcome(2, vec![note("qc1", "flattened"), note("qc2", "flattened")])),
            ..ImportReport::default()
        });
        let s = c.content_truncated.expect("truncation section");
        assert_eq!(s.display_counts(), (2, None));
    }

    /// `notesDropped` is row-global — it cannot say which section lost
    /// notes. It was deliberately ignored when the skip/fail view was built
    /// and must stay ignored now that there are five sections, where the
    /// temptation to reach for it is five times larger. A report whose only
    /// non-zero field is `notesDropped` draws nothing.
    #[test]
    fn the_row_global_drop_count_is_not_a_per_section_signal() {
        let c = Completion::from_report(&ImportReport {
            imported: 47,
            notes_dropped: 900,
            ..ImportReport::default()
        });
        assert!(c.sections(OutcomeTier::Documents).is_empty());
        assert!(c.sections(OutcomeTier::WithinDocuments).is_empty());
        assert!(
            c.is_clean(),
            "notesDropped alone cannot be attributed to any section, so it \
             cannot make a section appear",
        );
    }

    /// A kind that reports a count and names **nothing** still draws.
    ///
    /// This is not a corner case — it is the ordinary shape of
    /// `mentionsDegraded`. The server writes a note only for the systemic
    /// cause (the lookup endpoint refusing the run); the far more common
    /// per-document degradation bumps the counter alone. A `Section::new`
    /// that treated an empty note list as "nothing happened" would silence
    /// the most common within-document loss on exactly the runs where
    /// nothing else went wrong, and every other test here supplies a note.
    #[test]
    fn a_counter_with_no_named_examples_still_draws_its_section() {
        let c = Completion::from_report(&ImportReport {
            imported: 47,
            mentions_degraded: Some(outcome(5, vec![])),
            ..ImportReport::default()
        });

        let s = c.mentions_degraded.clone().expect(
            "a counted loss with no named example is still a loss the user must be told about",
        );
        assert_eq!(s.total, 5, "the heading count is the counter's");
        assert!(s.named.is_empty(), "precondition: the server named nothing");
        assert_eq!(
            s.display_counts(),
            (5, Some(5)),
            "all five are unnamed, so all five are disclosed as the remainder",
        );
        assert_eq!(
            c.sections(OutcomeTier::WithinDocuments)
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>(),
            vec![OutcomeKind::MentionsDegraded],
            "the section must reach the screen, not just the struct",
        );
        assert!(!c.is_clean(), "a run that degraded 5 documents' mentions is not clean");
    }

    /// A server that predates #208 sends no such fields at all. That must
    /// decode — a hard error here would break the whole progress poll mid
    /// rolling deploy, freezing the wizard on a live import — and must read
    /// as "this server does not report that", i.e. draw nothing.
    ///
    /// Built from wire JSON rather than a struct literal on purpose: the
    /// behaviour when a field is *missing* is the contract, and a struct
    /// literal cannot express a missing field.
    #[test]
    fn a_report_from_a_server_without_the_new_fields_decodes_and_draws_nothing() {
        let report: ImportReport = serde_json::from_value(serde_json::json!({
            "imported": 47,
            "skipped": { "total": 2, "notes": [{ "quipThreadId": "qt1", "detail": "403" }] },
            "failed": { "total": 0, "notes": [] },
            "chatThreadsSkipped": 12,
            "notesDropped": 0,
        }))
        .expect("a pre-#208 report must decode");

        assert!(report.images_dropped.is_none());
        assert!(report.content_truncated.is_none());
        assert!(report.mentions_degraded.is_none());
        assert!(report.live_apps_dropped.is_none());
        assert!(report.spreadsheet_formulas_dropped.is_none());

        let c = Completion::from_report(&report);
        assert_eq!(c.imported, 47, "the rest of the report must still decode");
        assert_eq!(c.chat_skipped, 12);
        assert_eq!(
            c.sections(OutcomeTier::Documents).len(),
            1,
            "the sections that server does send must still draw",
        );
        assert!(c.sections(OutcomeTier::WithinDocuments).is_empty());
    }

    /// An explicit `null` — what the current server sends for a kind that
    /// did not occur — reads identically to a missing field. Both mean
    /// "draw nothing"; neither is evidence of a loss.
    #[test]
    fn an_explicitly_null_kind_draws_nothing() {
        let report: ImportReport = serde_json::from_value(serde_json::json!({
            "imported": 47,
            "imagesDropped": null,
            "contentTruncated": null,
            "mentionsDegraded": null,
            "liveAppsDropped": null,
            "spreadsheetFormulasDropped": null,
        }))
        .expect("a null kind must decode");
        assert!(Completion::from_report(&report).is_clean());
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

    // ─── #236 Unit 3: where the import lands ───────────────────
    //
    // What these can and cannot reach: `start_target_folder_id` is the sole
    // expression that produces `start`'s `target_folder_id` argument, so its
    // behaviour is the destination feature. The *click* that populates its
    // `chosen` argument is not covered — this crate has no DOM harness, and
    // no amount of native testing changes that. Nor is the one-line wiring in
    // `do_start` that hands this function's result to `imports::start`: a
    // mutation that ignored the picked folder there would survive every test
    // in this module. A `frontend-doctor` scenario driving the step is the
    // honest coverage for both, and is the outstanding gap on #174.

    /// **The deliverable.** A folder the user picked is what `start`
    /// receives. The server authorizes exactly this id
    /// (`check_folder_access(..., Edit)` in `routes/imports.rs`, pinned by
    /// `start_rejects_unauthorized_target_folder`), so sending the choice is
    /// simultaneously the feature and the thing the access check runs
    /// against — a wizard that quietly kept sending Home would pass an access
    /// check on a folder the user never chose.
    #[test]
    fn the_picked_folder_is_what_start_receives() {
        assert_eq!(
            start_target_folder_id(Some("folder-projects"), Some("folder-home")).as_deref(),
            Some("folder-projects"),
        );
    }

    /// **Negative control.** A user who never opens the destination step must
    /// be indistinguishable from every user before this step existed: the
    /// import goes to Home. This asserts the pre-change constant behaviour
    /// and would hold verbatim against the wizard that had no picker at all.
    #[test]
    fn an_untouched_destination_still_imports_to_home() {
        assert_eq!(
            start_target_folder_id(None, Some("folder-home")).as_deref(),
            Some("folder-home"),
        );
    }

    /// Home is a row in the tree like any other, and the destination step
    /// opens highlighted on it. Confirming that highlight must land in the
    /// same place as never opening the step — otherwise "Change → Use this
    /// folder" on the default would silently mean something else.
    #[test]
    fn choosing_home_explicitly_matches_the_untouched_default() {
        assert_eq!(
            start_target_folder_id(Some("folder-home"), Some("folder-home")),
            start_target_folder_id(None, Some("folder-home")),
        );
    }

    /// A blank id is not a folder — the same rule
    /// [`open_folder_destination`] applies on the way back. Sending one would
    /// fail the server's access check on an empty id and surface as an error
    /// banner over a wizard the user filled in correctly.
    #[test]
    fn a_blank_choice_falls_back_to_home_rather_than_being_sent() {
        for blank in ["", "   "] {
            assert_eq!(
                start_target_folder_id(Some(blank), Some("folder-home")).as_deref(),
                Some("folder-home"),
                "a blank choice ({blank:?}) must fall back to Home",
            );
        }
    }

    /// Before `/users/me` lands there is no Home and nothing to send. The
    /// Continue button is disabled in exactly this state; this is `do_start`'s
    /// half of the same guard, because a start with no parent is a 400.
    #[test]
    fn without_a_home_folder_and_without_a_choice_there_is_nothing_to_start() {
        assert_eq!(start_target_folder_id(None, None), None);
        assert_eq!(start_target_folder_id(Some("  "), None), None);
    }

    /// A choice still works if the Home lookup failed — the tree cannot be
    /// opened in that state today, but the rule is "the choice wins", not
    /// "the choice wins when Home is also known".
    #[test]
    fn a_choice_does_not_depend_on_home_being_known() {
        assert_eq!(
            start_target_folder_id(Some("folder-projects"), None).as_deref(),
            Some("folder-projects"),
        );
    }

    /// **The read, not just the rule.** The tests above pin
    /// `start_target_folder_id`; this one pins that the wizard feeds it the
    /// signal the user's choice actually lives in. Without it, a `do_start`
    /// that applied the rule perfectly to `None` — sending every import to
    /// Home no matter what was picked — would pass every other test here.
    ///
    /// Drives the same construction the component does: `effective_target`
    /// over the wizard's two destination signals. Leptos signals need no DOM,
    /// so this much of the wiring is reachable natively. The click that
    /// writes `destination` is not.
    #[test]
    fn the_wizards_live_destination_follows_the_users_choice() {
        let destination: RwSignal<Option<(String, String)>> = RwSignal::new(None);
        let (home_folder_id, set_home_folder_id) = signal::<Option<String>>(None);
        let target = effective_target(destination, home_folder_id);

        assert_eq!(
            target.get_untracked(),
            None,
            "before /users/me lands there is nothing to start",
        );

        set_home_folder_id.set(Some("folder-home".to_string()));
        assert_eq!(
            target.get_untracked().as_deref(),
            Some("folder-home"),
            "an untouched wizard imports to Home",
        );

        destination.set(Some((
            "folder-projects".to_string(),
            "Projects".to_string(),
        )));
        assert_eq!(
            target.get_untracked().as_deref(),
            Some("folder-projects"),
            "the folder the user picked is the folder the import goes to",
        );

        destination.set(None);
        assert_eq!(
            target.get_untracked().as_deref(),
            Some("folder-home"),
            "and the default returns if the choice is cleared",
        );
    }

    /// The default destination line must keep promising Home. Paired with
    /// `an_untouched_destination_still_imports_to_home`: one pins where an
    /// untouched wizard sends the import, this pins what it tells the user it
    /// will do. Both held before this unit and must still hold after.
    #[test]
    fn the_untouched_destination_line_still_promises_home() {
        let line = EN_US
            .lines()
            .find(|l| l.starts_with("quip-import-target-home ="))
            .expect("en-US catalog is missing quip-import-target-home");
        assert!(
            line.contains("Home"),
            "the default line must still name Home; got {line:?}",
        );
        assert!(
            !line.contains('{'),
            "the default line takes no arguments; got {line:?}",
        );
    }

    /// The chosen-folder line interpolates the folder's name. A mismatched
    /// placeable here renders the raw key over the one line that tells the
    /// user their import is no longer going to Home.
    #[test]
    fn the_chosen_destination_line_names_the_folder() {
        let line = EN_US
            .lines()
            .find(|l| l.starts_with("quip-import-target-folder ="))
            .expect("en-US catalog is missing quip-import-target-folder");
        assert!(
            line.contains("$folder"),
            "quip-import-target-folder must interpolate $folder; got {line:?}",
        );
    }

    /// Every string the destination step renders must exist. Unlike the
    /// report strings these are all argument-free, so existence is the whole
    /// contract — but a missing one renders its raw key as a heading or a
    /// button label.
    #[test]
    fn every_destination_string_exists() {
        for key in [
            "quip-import-destination-change",
            "quip-import-destination-heading",
            "quip-import-destination-hint",
            "quip-import-destination-select",
            // Borrowed from the shared catalog rather than duplicated.
            "common-cancel",
            "common-loading",
        ] {
            assert!(
                EN_US.lines().any(|l| l.starts_with(&format!("{key} ="))),
                "en-US catalog is missing {key}",
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
            ("quip-import-report-images", "$count"),
            ("quip-import-report-truncated", "$count"),
            ("quip-import-report-mentions", "$count"),
            ("quip-import-report-live-apps", "$count"),
            ("quip-import-report-formulas", "$count"),
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
        // The tier heading takes no arguments, so it is checked for
        // existence only — but it must exist, or the group renders its raw
        // key as a heading over the sections it is meant to introduce.
        assert!(
            EN_US
                .lines()
                .any(|l| l.starts_with("quip-import-report-within-heading =")),
            "en-US catalog is missing quip-import-report-within-heading",
        );
    }

    /// Every locale carries every `quip-import-report-*` key the component
    /// can render, so no locale falls back to a raw key in the middle of the
    /// one screen whose job is to be readable.
    ///
    /// The catalogs are compared to each other rather than to a hand-kept
    /// list: a list would have to be updated alongside a new key, and the
    /// failure of forgetting is silent in exactly the locales nobody on the
    /// team reads.
    #[test]
    fn every_locale_carries_every_report_string() {
        const CATALOGS: [(&str, &str); 6] = [
            ("en-US", EN_US),
            ("de", include_str!("../../../locales/de/main.ftl")),
            ("es", include_str!("../../../locales/es/main.ftl")),
            ("fr", include_str!("../../../locales/fr/main.ftl")),
            ("it", include_str!("../../../locales/it/main.ftl")),
            ("ar", include_str!("../../../locales/ar/main.ftl")),
        ];

        fn report_keys(catalog: &str) -> Vec<String> {
            let mut keys: Vec<String> = catalog
                .lines()
                .filter_map(|l| l.split_once(" ="))
                .map(|(k, _)| k.to_string())
                .filter(|k| k.starts_with("quip-import-report-"))
                .collect();
            keys.sort();
            keys
        }

        let expected = report_keys(EN_US);
        assert!(
            expected.len() >= 13,
            "precondition: en-US should hold every report key; got {expected:?}",
        );
        for (name, catalog) in CATALOGS {
            assert_eq!(
                report_keys(catalog),
                expected,
                "{name} does not carry the same quip-import-report-* keys as en-US",
            );
        }
    }

    /// The same guarantee, widened to the whole wizard (#236 Unit 3): the
    /// destination step's strings are not `quip-import-report-*`, so the test
    /// above would not have noticed a locale that was missing them, and a
    /// German user would have met a raw key where the button that changes
    /// their import's destination should be.
    ///
    /// Widened rather than duplicated for the new prefix, because the next
    /// wizard string will not be a `destination-` one either. All six
    /// catalogs already agreed on the pre-existing keys, so this asserts a
    /// property that was true before it was written.
    #[test]
    fn every_locale_carries_every_wizard_string() {
        const CATALOGS: [(&str, &str); 6] = [
            ("en-US", EN_US),
            ("de", include_str!("../../../locales/de/main.ftl")),
            ("es", include_str!("../../../locales/es/main.ftl")),
            ("fr", include_str!("../../../locales/fr/main.ftl")),
            ("it", include_str!("../../../locales/it/main.ftl")),
            ("ar", include_str!("../../../locales/ar/main.ftl")),
        ];

        fn wizard_keys(catalog: &str) -> Vec<String> {
            let mut keys: Vec<String> = catalog
                .lines()
                .filter_map(|l| l.split_once(" ="))
                .map(|(k, _)| k.to_string())
                .filter(|k| k.starts_with("quip-import-"))
                .collect();
            keys.sort();
            keys
        }

        let expected = wizard_keys(EN_US);
        for key in [
            "quip-import-target-folder",
            "quip-import-destination-change",
            "quip-import-destination-heading",
            "quip-import-destination-hint",
            "quip-import-destination-select",
        ] {
            assert!(
                expected.iter().any(|k| k == key),
                "precondition: en-US should hold {key}",
            );
        }
        for (name, catalog) in CATALOGS {
            assert_eq!(
                wizard_keys(catalog),
                expected,
                "{name} does not carry the same quip-import-* keys as en-US",
            );
        }
    }

    /// A translated string is a translation, not English in a foreign file.
    /// The check that survives having no translator on the team: the
    /// destination step's wording must differ from en-US in every locale that
    /// is not en-US. It cannot judge quality — it does catch the failure mode
    /// this task is most likely to produce, which is pasting English into
    /// `de` and `ar` to make the parity test above go green.
    ///
    /// `quip-import-destination-change` is exempt: "Change" is genuinely
    /// "Cambia" in it and "Cambiar" in es, but a one-word button label is
    /// exactly where a real translation can coincide with English, so a
    /// future locale that legitimately matches would fail this for no reason.
    #[test]
    fn the_destination_step_is_translated_not_copied() {
        const TRANSLATED: [(&str, &str); 5] = [
            ("de", include_str!("../../../locales/de/main.ftl")),
            ("es", include_str!("../../../locales/es/main.ftl")),
            ("fr", include_str!("../../../locales/fr/main.ftl")),
            ("it", include_str!("../../../locales/it/main.ftl")),
            ("ar", include_str!("../../../locales/ar/main.ftl")),
        ];

        fn value_of(catalog: &str, key: &str) -> String {
            catalog
                .lines()
                .find(|l| l.starts_with(&format!("{key} =")))
                .unwrap_or_else(|| panic!("catalog is missing {key}"))
                .split_once(" = ")
                .expect("a catalog line has a value")
                .1
                .to_string()
        }

        for key in [
            "quip-import-target-folder",
            "quip-import-destination-heading",
            "quip-import-destination-hint",
            "quip-import-destination-select",
        ] {
            let english = value_of(EN_US, key);
            for (name, catalog) in TRANSLATED {
                assert_ne!(
                    value_of(catalog, key),
                    english,
                    "{name}'s {key} is the en-US string verbatim",
                );
            }
        }
    }
}
