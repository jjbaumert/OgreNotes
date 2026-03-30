use leptos::prelude::*;

/// Notification bell icon with unread count badge.
/// Shows a dropdown panel when clicked.
#[component]
pub fn NotificationBell() -> impl IntoView {
    let (unread_count, set_unread_count) = signal(0usize);
    let (panel_open, set_panel_open) = signal(false);
    let (notifications, _set_notifications) = signal::<Vec<NotifEntry>>(Vec::new());

    // Poll unread count periodically
    // TODO: Replace with WebSocket push in production
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            // TODO: Call GET /api/v1/notifications/unread-count
            set_unread_count.set(0);
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
                                // TODO: Call POST /api/v1/notifications/read-all
                                set_unread_count.set(0);
                            }
                        >"Mark all read"</button>
                    </div>
                    <div class="notification-panel-body">
                        {move || {
                            let items = notifications.get();
                            if items.is_empty() {
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
