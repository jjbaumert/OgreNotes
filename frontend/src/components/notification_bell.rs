use leptos::prelude::*;

use crate::api::notifications;

/// Notification bell icon with unread count badge.
/// Shows a dropdown panel when clicked.
#[component]
pub fn NotificationBell() -> impl IntoView {
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
                            message: n.message,
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
                        <span>"Notifications"</span>
                        <button
                            class="notification-mark-all"
                            on:click=move |_| {
                                let set_count = set_unread_count;
                                let set_list = set_notifications_list.clone();
                                leptos::task::spawn_local(async move {
                                    match notifications::mark_all_read().await {
                                        Ok(_) => {
                                            set_count.set(0);
                                            // Mark all local entries as read
                                            set_list.update(|items| {
                                                for item in items.iter_mut() {
                                                    item.read = true;
                                                }
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
                        >"Mark all read"</button>
                    </div>
                    <div class="notification-panel-body">
                        <Show when=move || loading.get()>
                            <div class="notification-loading">"Loading..."</div>
                        </Show>
                        {move || {
                            let items = notifications_list.get();
                            if items.is_empty() && !loading.get() {
                                view! {
                                    <div class="notification-empty">"No notifications"</div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="notification-list">
                                        {items.into_iter().map(|n| {
                                            view! {
                                                <div class=format!("notification-item {}", if n.read { "" } else { "unread" })>
                                                    <div class="notification-message">{n.message}</div>
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
    message: String,
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
