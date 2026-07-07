// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::a11y;
use crate::api::notifications;

/// Notification bell icon with unread count badge.
/// Shows a dropdown panel when clicked.
#[component]
pub fn NotificationBell() -> impl IntoView {
    let navigate = use_navigate();
    let (unread_count, set_unread_count) = signal(0usize);
    let (panel_open, set_panel_open) = signal(false);
    let (notifications_list, set_notifications_list) = signal::<Vec<NotifEntry>>(Vec::new());
    let (loading, set_loading) = signal(false);

    // Poll unread count on mount and every 30 seconds.
    // Use a cancellation flag so the loop stops when the component unmounts.
    let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let active_for_cleanup = active.clone();
    on_cleanup(move || active_for_cleanup.store(false, std::sync::atomic::Ordering::Relaxed));

    Effect::new(move |_| {
        let active = active.clone();
        leptos::task::spawn_local(async move {
            fetch_unread_count(set_unread_count).await;
            loop {
                gloo_timers::future::TimeoutFuture::new(30_000).await;
                if !active.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                fetch_unread_count(set_unread_count).await;
            }
        });
    });

    // Load full notification list when panel opens.
    Effect::new(move |_| {
        if !panel_open.get() {
            return;
        }
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match notifications::get_notifications().await {
                Ok(resp) => {
                    let entries: Vec<NotifEntry> = resp
                        .notifications
                        .into_iter()
                        .map(|n| NotifEntry {
                            id: n.notif_id,
                            created_at: n.created_at,
                            doc_id: n.doc_id,
                            thread_id: n.thread_id,
                            actor_name: n.actor_name,
                            doc_title: n.doc_title,
                            message: n.message,
                            preview: n.preview,
                            read: n.read,
                        })
                        .collect();
                    set_notifications_list.set(entries);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load notifications: {e}").into(),
                    );
                }
            }
            set_loading.set(false);
        });
    });

    view! {
        <div class="notification-bell-wrapper">
            <button
                class="notification-bell"
                on:click=move |_| set_panel_open.set(!panel_open.get_untracked())
            >
                "\u{1F514}"
                <Show when=move || unread_count.get() != 0>
                    <span class="notification-badge">{move || unread_count.get()}</span>
                </Show>
            </button>

            <Show when=move || panel_open.get()>
                <div class="notification-panel">
                    <div class="notification-panel-header">
                        <span>{crate::t!("notifications-title")}</span>
                        <button
                            class="notification-mark-all"
                            on:click=move |_| {
                                let set_count = set_unread_count;
                                let set_list = set_notifications_list.clone();
                                leptos::task::spawn_local(async move {
                                    match notifications::mark_all_read().await {
                                        Ok(resp) => {
                                            // Mark local entries read for an
                                            // instant visual update...
                                            set_list.update(|items| {
                                                for item in items.iter_mut() {
                                                    item.read = true;
                                                }
                                            });
                                            // ...and drop the badge by exactly
                                            // the number the server flipped.
                                            // #120: the old code re-queried the
                                            // unread count here, but that read
                                            // is eventually consistent and
                                            // returned the PRE-write count, so
                                            // the badge never changed — the
                                            // user-visible "mark all read does
                                            // nothing". `marked` is the exact
                                            // best-effort count, mirroring the
                                            // single-item path's optimistic
                                            // decrement; the 30s poll reconciles
                                            // any residual drift.
                                            set_count.update(|c| {
                                                *c = c.saturating_sub(resp.marked)
                                            });
                                        }
                                        Err(e) => {
                                            web_sys::console::warn_1(
                                                &format!("Failed to mark all read: {e}").into(),
                                            );
                                        }
                                    }
                                });
                            }
                        >{crate::t!("notifications-mark-all-read")}</button>
                        <button
                            class="notification-clear-all"
                            on:click=move |_| {
                                // #120: dismiss (delete) every notification and
                                // empty the bell — distinct from "mark all read"
                                // (which only clears the unread badge/highlight
                                // but keeps the rows visible). The local list +
                                // badge clear instantly; the server delete is
                                // best-effort and fire-and-forget.
                                let set_count = set_unread_count;
                                let set_list = set_notifications_list;
                                leptos::task::spawn_local(async move {
                                    match notifications::dismiss_all().await {
                                        Ok(_) => {
                                            set_list.set(Vec::new());
                                            set_count.set(0);
                                        }
                                        Err(e) => {
                                            web_sys::console::warn_1(
                                                &format!("Failed to clear notifications: {e}").into(),
                                            );
                                        }
                                    }
                                });
                            }
                        >{crate::t!("notifications-clear-all")}</button>
                    </div>
                    <div class="notification-panel-body">
                        <Show when=move || loading.get()>
                            <div class="notification-loading">{crate::t!("common-loading")}</div>
                        </Show>
                        {
                            // Clone into the list closure so the enclosing
                            // panel closure keeps ownership of `navigate` and
                            // stays `Fn` (it re-renders).
                            let navigate = navigate.clone();
                            move || {
                            let items = notifications_list.get();
                            if items.is_empty() && !loading.get() {
                                view! {
                                    <div class="notification-empty">{crate::t!("notifications-empty")}</div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="notification-list">
                                        {items.into_iter().map(|n| {
                                            let nav = navigate.clone();
                                            let n_id = n.id.clone();
                                            // Separate clone for the dismiss-X
                                            // handler (n_id is moved into the
                                            // navigate/mark-read closure below).
                                            let n_id_dismiss = n.id.clone();
                                            let created_at = n.created_at;
                                            let doc_id = n.doc_id.clone();
                                            let thread_id = n.thread_id.clone();
                                            let was_unread = !n.read;
                                            let clickable = doc_id.is_some();
                                            let actor = n.actor_name.clone();
                                            let message = n.message.clone();
                                            let doc_title = n.doc_title.clone();
                                            let preview = n.preview.clone();
                                            view! {
                                                <div
                                                    class=format!("notification-item {}", if n.read { "" } else { "unread" })
                                                    class:clickable=clickable
                                                    on:click=move |_| {
                                                        let Some(doc) = doc_id.clone() else { return };
                                                        // Mark just this one read and drop the badge by one.
                                                        if was_unread {
                                                            let sk = notifications::notification_sk(created_at, &n_id);
                                                            leptos::task::spawn_local(async move {
                                                                let _ = notifications::mark_read(vec![sk]).await;
                                                            });
                                                            set_unread_count.update(|c| *c = c.saturating_sub(1));
                                                        }
                                                        // Deep-link to the document, carrying the thread so
                                                        // the page opens (and centers) the comment.
                                                        let url = match thread_id.clone() {
                                                            Some(tid) => format!("/d/{doc}?comment={tid}"),
                                                            None => format!("/d/{doc}"),
                                                        };
                                                        a11y::defer(move || set_panel_open.set(false));
                                                        nav(&url, Default::default());
                                                    }
                                                >
                                                    <div class="notification-message">
                                                        <span class="notification-actor">{actor}</span>
                                                        " "
                                                        {message}
                                                        {doc_title.map(|t| view! {
                                                            " on "
                                                            <span class="notification-doc">{t}</span>
                                                        })}
                                                    </div>
                                                    {preview.map(|p| view! {
                                                        <div class="notification-preview">{p}</div>
                                                    })}
                                                    <button
                                                        class="notification-dismiss"
                                                        title=crate::t!("notifications-dismiss")
                                                        on:click=move |ev: web_sys::MouseEvent| {
                                                            // Don't let the X trigger the item's
                                                            // navigate handler.
                                                            ev.stop_propagation();
                                                            let sk = notifications::notification_sk(
                                                                created_at, &n_id_dismiss,
                                                            );
                                                            leptos::task::spawn_local(async move {
                                                                let _ = notifications::dismiss(vec![sk]).await;
                                                            });
                                                            // Remove from the bell + drop the badge if
                                                            // it was unread (instant; delete is async).
                                                            let id = n_id_dismiss.clone();
                                                            set_notifications_list.update(|items| {
                                                                items.retain(|i| i.id != id);
                                                            });
                                                            if was_unread {
                                                                set_unread_count.update(|c| {
                                                                    *c = c.saturating_sub(1)
                                                                });
                                                            }
                                                        }
                                                    >"\u{00D7}"</button>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </Show>
        </div>
    }
}

#[derive(Clone)]
struct NotifEntry {
    id: String,
    created_at: i64,
    doc_id: Option<String>,
    thread_id: Option<String>,
    actor_name: String,
    doc_title: Option<String>,
    message: String,
    preview: Option<String>,
    read: bool,
}

async fn fetch_unread_count(set_count: WriteSignal<usize>) {
    match notifications::get_unread_count().await {
        Ok(resp) => set_count.set(resp.count),
        Err(e) => {
            web_sys::console::warn_1(
                &format!("Failed to fetch unread count: {e}").into(),
            );
        }
    }
}
