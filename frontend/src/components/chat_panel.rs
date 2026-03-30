use leptos::prelude::*;

/// Chat panel component for the sidebar.
/// Lists user's chats (DMs + group rooms) with a "New Chat" button.
#[component]
pub fn ChatPanel() -> impl IntoView {
    let (chats, _set_chats) = signal::<Vec<ChatEntry>>(Vec::new());
    let (expanded, set_expanded) = signal(false);

    view! {
        <div class="sidebar-section">
            <div
                class="sidebar-section-header"
                on:click=move |_| set_expanded.set(!expanded.get_untracked())
            >
                <span class="sidebar-section-title">"Chats"</span>
                <span class="sidebar-section-toggle">
                    {move || if expanded.get() { "\u{25BC}" } else { "\u{25B6}" }}
                </span>
            </div>

            <Show when=move || expanded.get()>
                <div class="sidebar-section-body">
                    {move || {
                        let items = chats.get();
                        if items.is_empty() {
                            view! {
                                <div class="sidebar-empty">"No chats yet"</div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="chat-list">
                                    {items.into_iter().map(|c| {
                                        view! {
                                            <div class="chat-item">
                                                <span class="chat-title">{c.title}</span>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }}

                    <button class="sidebar-action-btn" on:click=move |_| {
                        // TODO: Open new chat dialog
                    }>"+ New Chat"</button>
                </div>
            </Show>
        </div>
    }
}

/// A chat entry for display in the sidebar.
#[derive(Clone)]
struct ChatEntry {
    id: String,
    title: String,
}
