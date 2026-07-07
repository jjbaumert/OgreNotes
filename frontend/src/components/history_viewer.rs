// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

use crate::api::history;
use super::confirm_dialog::ConfirmDialog;
use super::diff_block_view::DiffBlockView;
use super::dom_position;
use crate::i18n::format_relative;

/// Edit history viewer. A right-hand pane lists all document versions;
/// clicking a version opens a modal popup with the attributed diff and
/// a "Restore" action.
#[component]
pub fn HistoryViewer(
    /// Whether the pane is visible.
    visible: ReadSignal<bool>,
    /// Document ID.
    doc_id: ReadSignal<String>,
    /// Fires after a "Jump to block" click. Parent decides whether to
    /// close the pane (mobile) or leave it open (desktop). The child
    /// has already closed its modal and scrolled the editor.
    on_jump: Callback<()>,
) -> impl IntoView {
    let (versions, set_versions) = signal::<Vec<history::VersionEntry>>(Vec::new());
    let (loading, set_loading) = signal(false);
    let (diff_entries, set_diff_entries) = signal::<Vec<history::DiffEntry>>(Vec::new());
    let (selected_version, set_selected_version) = signal::<Option<u64>>(None);
    let (restoring, set_restoring) = signal(false);
    let (restore_error, set_restore_error) = signal::<Option<String>>(None);
    // Two-step restore: clicking the version's "Restore" button stages a
    // pending version + opens the confirm dialog. Restore only fires
    // when the user explicitly confirms — restoring overwrites the live
    // doc with the old snapshot and triggers a hard reload, which
    // discards any in-flight unsaved local edits.
    let (pending_restore, set_pending_restore) = signal::<Option<u64>>(None);

    let reload_versions = move || {
        let id = doc_id.get_untracked();
        if id.is_empty() {
            return;
        }
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match history::list_versions(&id).await {
                Ok(resp) => set_versions.set(resp.versions),
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load versions: {e}").into(),
                    );
                }
            }
            set_loading.set(false);
        });
    };

    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        // Entering the pane: start at the version list, never a stale diff
        // modal left open from a prior session. Resetting here — while the
        // pane is visible and stable — keeps every modal teardown on the
        // deliberate selected→None path, never during a pane close (#90).
        set_selected_version.set(None);
        reload_versions();
    });

    // Load the attributed diff for a selected version. The backend
    // computes `(prev_version → selected_version)` so the diff describes
    // "what this version changed", and stamps every entry with the
    // selected version's author + timestamp.
    Effect::new(move |_| {
        let Some(version) = selected_version.get() else {
            set_diff_entries.set(Vec::new());
            return;
        };
        let id = doc_id.get();
        if id.is_empty() {
            return;
        }
        let prev = versions
            .get_untracked()
            .iter()
            .map(|v| v.version)
            .filter(|v| *v < version)
            .max();

        leptos::task::spawn_local(async move {
            let Some(prev) = prev else {
                set_diff_entries.set(Vec::new());
                return;
            };
            match history::diff_versions(&id, prev, version).await {
                Ok(resp) => set_diff_entries.set(resp.diffs),
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load diff: {e}").into(),
                    );
                    set_diff_entries.set(Vec::new());
                }
            }
        });
    });

    let close_modal = move || {
        // #90: defer the teardown by one microtask. close_modal runs
        // synchronously inside the modal's on:click handlers (the X
        // button, the backdrop, the footer Close). Flipping
        // selected_version here makes the modal's `<Show>` rebuild on the
        // SAME reactive turn the click is still being dispatched in —
        // Leptos drops the modal subtree's event closures mid-turn and
        // then re-invokes one as it re-attaches listeners, producing
        // "closure invoked recursively or after being dropped" (observed
        // in Firefox; reproduced via the history-pane doctor scenario).
        // One microtask lets the click + reactive turn settle before the
        // modal unmounts — the same remedy `a11y::defer_close` applies to
        // every other modal's close path.
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            set_selected_version.set(None);
            set_restore_error.set(None);
        });
    };

    let confirm_restore = move || {
        let Some(version) = pending_restore.get_untracked() else { return };
        set_pending_restore.set(None);
        if restoring.get_untracked() {
            return;
        }
        let id = doc_id.get_untracked();
        if id.is_empty() {
            return;
        }
        set_restore_error.set(None);
        set_restoring.set(true);
        leptos::task::spawn_local(async move {
            match history::restore_version(&id, version).await {
                Ok(()) => {
                    set_selected_version.set(None);
                    reload_versions();
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().reload();
                    }
                }
                Err(e) => {
                    set_restore_error.set(Some(format!("Restore failed: {e}")));
                }
            }
            set_restoring.set(false);
        });
    };

    // #90: The diff modal's lifecycle is tied ONLY to `selected_version`,
    // NOT to pane visibility. Two reasons, both producing the cascading
    // "closure invoked recursively or after being dropped" panic:
    //
    //   1. The Memo keeps version-to-version switching (Some(1) → Some(2))
    //      from re-firing the predicate — a bare `move || …is_some()` would
    //      re-evaluate on every click, tearing down the modal frame and
    //      dropping its on:click closures.
    //   2. Dropping the `visible &&` term: coupling the modal to pane
    //      visibility meant closing the pane (visible→false) tore the modal
    //      subtree down while a diff was open. Any in-flight diff-load
    //      (the spawn_local in the Effect above) resolving *after* that
    //      teardown then set a signal whose modal-body closure had been
    //      dropped — the panic, fired from a queued microtask on close.
    //
    // The pane hides via `.history-pane { display: none }` (CSS), which
    // also hides the still-mounted modal, so decoupling is safe: the modal
    // now unmounts only on a deliberate selected→None (close button, jump,
    // or the reopen-reset above) while the pane is visible and stable.
    let modal_open = Memo::new(move |_| selected_version.get().is_some());

    view! {
        <div class="history-pane" class:is-open=move || visible.get()>
            <div class="history-header">{crate::t!("history-title")}</div>

            <div class="history-versions">
                <Show when=move || loading.get()>
                    <div class="history-loading">{crate::t!("common-loading")}</div>
                </Show>
                {move || {
                    let items = versions.get();
                    if items.is_empty() && !loading.get() {
                        view! {
                            <div class="history-empty">{crate::t!("history-empty")}</div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="history-list">
                                {items.into_iter().map(|v| {
                                    let ver = v.version;
                                    let is_selected = move || selected_version.get() == Some(ver);
                                    view! {
                                        <div
                                            class="history-version-item"
                                            class:selected=is_selected
                                            on:click=move |_| set_selected_version.set(Some(ver))
                                        >
                                            <span class="history-version-num">{format!("v{}", v.version)}</span>
                                            <span class="history-version-time">{format_relative(v.created_at)}</span>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        }.into_any()
                    }
                }}
            </div>

            <Show when=move || modal_open.get()>
                // #90: The structural elements (backdrop, modal frame,
                // close buttons, restore button) are deliberately rendered
                // OUTSIDE any reactive closure so they survive version
                // switching without being torn down. Only the version-
                // dependent inner parts (header label, diff body) live in
                // their own `move ||` closures. This keeps the on:click
                // closures stable across version-switches.
                <div
                    class="history-diff-backdrop"
                    on:click=move |_| close_modal()
                >
                    <div
                        class="history-diff-modal"
                        on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                    >
                        <div class="history-diff-modal-header">
                            {move || {
                                // Reactive header — re-renders on version
                                // switch, but the surrounding modal frame
                                // and its handlers stay intact.
                                let Some(ver) = selected_version.get() else {
                                    return view! { <div></div> }.into_any();
                                };
                                let entries = diff_entries.get();
                                let attribution = entries.iter().find_map(|e| {
                                    match (e.user_id.as_deref(), e.timestamp) {
                                        (Some(uid), Some(ts)) => Some(format!("{} \u{2022} {}", uid, format_relative(ts))),
                                        (None, Some(ts)) => Some(format_relative(ts)),
                                        _ => None,
                                    }
                                });
                                view! {
                                    <div class="history-diff-version">
                                        <span class="history-diff-label">{crate::t!("history-changes-in-v", version = ver.to_string())}</span>
                                        {attribution.map(|a| view! {
                                            <span class="history-diff-attribution">{a}</span>
                                        })}
                                    </div>
                                }.into_any()
                            }}
                            <button
                                class="history-modal-close"
                                aria-label=crate::t!("history-aria-close")
                                on:click=move |_| close_modal()
                            >
                                "\u{00d7}"
                            </button>
                        </div>

                        <div class="history-diff-modal-body">
                            {move || {
                                let items = diff_entries.get();
                                if items.is_empty() {
                                    view! {
                                        <div class="history-diff-empty">
                                            {crate::t!("history-no-prior")}
                                        </div>
                                    }.into_any()
                                } else {
                                    view! {
                                        <div class="history-diff">
                                            {items.into_iter().map(|entry| {
                                                render_diff_entry_card(entry, set_selected_version, on_jump)
                                            }).collect::<Vec<_>>()}
                                        </div>
                                    }.into_any()
                                }
                            }}
                            {move || restore_error.get().map(|msg| view! {
                                <div class="history-restore-error">{msg}</div>
                            })}
                        </div>

                        <div class="history-diff-modal-footer">
                            <button
                                class="btn btn-secondary"
                                on:click=move |_| close_modal()
                            >
                                {crate::t!("common-close")}
                            </button>
                            <button
                                class="btn btn-primary"
                                disabled=move || restoring.get()
                                on:click=move |_| {
                                    // #90: Read selected_version at click
                                    // time (untracked) so this handler is
                                    // stable across version switches. The
                                    // captured `ver` of the old structure
                                    // is what made these click closures
                                    // get re-created (and dropped) on
                                    // every version change.
                                    if let Some(ver) = selected_version.get_untracked() {
                                        set_pending_restore.set(Some(ver));
                                    }
                                }
                            >
                                {move || if restoring.get() { crate::t!("history-restoring") } else { crate::t!("history-restore-to-this-version") }}
                            </button>
                        </div>
                    </div>
                </div>
            </Show>

            <ConfirmDialog
                visible=Signal::derive(move || pending_restore.get().is_some())
                title=crate::t!("history-restore-version")
                message=crate::t!("history-restore-confirm-message")
                confirm_label=crate::t!("history-restore-confirm-label")
                destructive=true
                on_confirm=Callback::new(move |()| confirm_restore())
                on_cancel=Callback::new(move |()| set_pending_restore.set(None))
            />
        </div>
    }
}

// ─── Diff entry card rendering ─────────────────────────────────

/// Render one `DiffEntry` as a card in the diff modal body. The card has:
///
/// - A left gutter color via `.diff-card--{added,removed,modified,deleted}`
/// - A header with the node-type label and a "Jump to block ↗" button.
///   The button is hidden for `Removed` entries whose block is no longer
///   in the live doc; instead the card carries a "(deleted)" badge.
/// - For `Added`/`Removed`: each block rendered via `<DiffBlockView>` in
///   sequence. For `Modified`: blocks[0] (old, struck-through) above
///   blocks[1] (new), no separator label.
///
/// `set_selected_version` lets the jump handler close the modal so the
/// editor is unobscured; `on_jump` lets the parent close the pane on
/// small viewports.
fn render_diff_entry_card(
    entry: history::DiffEntry,
    set_selected_version: WriteSignal<Option<u64>>,
    on_jump: Callback<()>,
) -> AnyView {
    let kind = entry.kind;
    let node_type_label = node_type_label(&entry.node_type);
    let block_id = entry.block_id.clone();
    let block_index = entry.block_index;

    // We can't know from the modal whether a Removed block is still
    // present in the live doc without querying the DOM, so the
    // "(deleted)" treatment fires only when the entry has no block_id
    // AND it's a Removed kind. With a stable id we let scroll_to_block
    // try and silently no-op if the element isn't there — preferable
    // to a false negative.
    let is_deleted = matches!(kind, history::DiffKind::Removed) && block_id.is_none();
    let card_class = match kind {
        history::DiffKind::Added => "diff-card diff-card--added",
        history::DiffKind::Removed if is_deleted => "diff-card diff-card--deleted",
        history::DiffKind::Removed => "diff-card diff-card--removed",
        history::DiffKind::Modified => "diff-card diff-card--modified",
    };

    let body: AnyView = match kind {
        history::DiffKind::Modified => {
            // Backend invariant: a Modified entry's `blocks` is exactly
            // [old, new]. Catch a regression early in dev builds; in
            // release builds extras are silently dropped (the
            // alternative — panicking — is worse than a degraded card).
            debug_assert_eq!(
                entry.blocks.len(),
                2,
                "Modified diff entry must carry exactly two blocks (old, new), got {}",
                entry.blocks.len(),
            );
            let mut iter = entry.blocks.into_iter();
            let old = iter.next();
            let new = iter.next();
            view! {
                <div class="diff-card-body">
                    {old.map(|b| view! {
                        <div class="diff-block--old">
                            <DiffBlockView block=b />
                        </div>
                    })}
                    {new.map(|b| view! {
                        <div class="diff-block--new">
                            <DiffBlockView block=b />
                        </div>
                    })}
                </div>
            }
            .into_any()
        }
        _ => view! {
            <div class="diff-card-body">
                {entry.blocks.into_iter().map(|b| view! {
                    <DiffBlockView block=b />
                }).collect::<Vec<_>>()}
            </div>
        }
        .into_any(),
    };

    let jump_id = block_id.clone();
    let on_jump_click = move |_| {
        // Close the modal so the editor is unobscured before scrolling.
        // Deferred by a microtask for the same reason as close_modal
        // (#90): tearing the modal's `<Show>` down synchronously inside
        // this on:click drops its closures mid-turn and re-invokes one.
        // The parent's on_jump callback decides whether to also close
        // the pane (mobile) or leave it open (desktop).
        let jump_id = jump_id.clone();
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            set_selected_version.set(None);
            let _ = dom_position::scroll_to_block(jump_id.as_deref(), block_index);
            on_jump.run(());
        });
    };

    let header_action: AnyView = if is_deleted {
        view! { <span class="diff-card-deleted">{crate::t!("history-deleted-badge")}</span> }.into_any()
    } else {
        view! {
            <button
                class="diff-card-jump"
                title=crate::t!("history-jump-to-block-title")
                on:click=on_jump_click
            >
                {crate::t!("history-jump-to-block-label")}
            </button>
        }
        .into_any()
    };

    view! {
        <div class=card_class>
            <div class="diff-card-header">
                <span class="diff-card-label">{node_type_label}</span>
                {header_action}
            </div>
            {body}
        </div>
    }
    .into_any()
}

/// Friendly label for the entry header — derived from the yrs tag name.
///
/// Returns an owned `String` because the label routes through `t!()`,
/// which formats against the active Fluent bundle at call time.
fn node_type_label(node_type: &str) -> String {
    match node_type {
        "paragraph" => crate::t!("node-paragraph"),
        "heading" => crate::t!("node-heading"),
        "bullet_list" => crate::t!("node-bullet-list"),
        "ordered_list" => crate::t!("node-ordered-list"),
        "list_item" => crate::t!("node-list-item"),
        "task_list" => crate::t!("node-task-list"),
        "task_item" => crate::t!("node-task-item"),
        "blockquote" => crate::t!("node-blockquote"),
        "code_block" => crate::t!("node-code-block"),
        "horizontal_rule" => crate::t!("node-horizontal-rule"),
        "image" => crate::t!("node-image"),
        "table" => crate::t!("node-table"),
        "table_row" => crate::t!("node-table-row"),
        "table_cell" => crate::t!("node-table-cell"),
        "table_header" => crate::t!("node-table-header"),
        _ => crate::t!("node-block"),
    }
}
