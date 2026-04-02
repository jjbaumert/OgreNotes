use leptos::prelude::*;

use crate::api::chats;

/// Chat panel component for the sidebar.
/// Lists user's chats (DMs + group rooms) with a "New Chat" button.
#[component]
pub fn ChatPanel() -> impl IntoView {
    let (chat_list, set_chat_list) = signal::<Vec<ChatEntry>>(Vec::new());
    let (expanded, set_expanded) = signal(false);
    let (loading, set_loading) = signal(false);
    // Selected chat for viewing messages.
    let (selected_chat, set_selected_chat) = signal::<Option<String>>(None);
    let (messages, set_messages) = signal::<Vec<MsgEntry>>(Vec::new());
    let (reply_text, set_reply_text) = signal(String::new());

    // Load chat list when expanded.
    Effect::new(move |_| {
        if !expanded.get() {
            return;
        }
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match chats::list_chats().await {
                Ok(resp) => {
                    let entries: Vec<ChatEntry> = resp
                        .chats
                        .into_iter()
                        .map(|c| ChatEntry {
                            id: c.id,
                            title: c.title.unwrap_or_else(|| {
                                if c.chat_type == "directMessage" {
                                    format!("DM ({})", c.member_ids.len())
                                } else {
                                    format!("Chat ({})", c.member_ids.len())
                                }
                            }),
                        })
                        .collect();
                    set_chat_list.set(entries);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load chats: {e}").into(),
                    );
                }
            }
            set_loading.set(false);
        });
    });

    // Load messages when a chat is selected.
    Effect::new(move |_| {
        let Some(cid) = selected_chat.get() else {
            set_messages.set(Vec::new());
            return;
        };

        leptos::task::spawn_local(async move {
            match chats::list_messages(&cid).await {
                Ok(resp) => {
                    let msgs: Vec<MsgEntry> = resp
                        .messages
                        .into_iter()
                        .map(|m| MsgEntry {
                            user_id: m.user_id,
                            content: m.content,
                            created_at: m.created_at,
                        })
                        .collect();
                    set_messages.set(msgs);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load messages: {e}").into(),
                    );
                }
            }
        });
    });

    let send_reply = move || {
        let msg = reply_text.get_untracked();
        if msg.trim().is_empty() {
            return;
        }
        let Some(cid) = selected_chat.get_untracked() else { return };
        set_reply_text.set(String::new());
        leptos::task::spawn_local(async move {
            match chats::send_message(&cid, &msg).await {
                Ok(()) => {
                    // Reload messages.
                    if let Ok(resp) = chats::list_messages(&cid).await {
                        let msgs: Vec<MsgEntry> = resp
                            .messages
                            .into_iter()
                            .map(|m| MsgEntry {
                                user_id: m.user_id,
                                content: m.content,
                                created_at: m.created_at,
                            })
                            .collect();
                        set_messages.set(msgs);
                    }
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to send message: {e}").into(),
                    );
                }
            }
        });
    };

    let send_reply_on_enter = send_reply.clone();

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
                    // Back button when viewing a chat.
                    <Show when=move || selected_chat.get().is_some()>
                        <button
                            class="sidebar-back-btn"
                            on:click=move |_| set_selected_chat.set(None)
                        >"\u{2190} Back to chats"</button>
                    </Show>

                    // Chat list view.
                    <Show when=move || selected_chat.get().is_none()>
                        <Show when=move || loading.get()>
                            <div class="sidebar-loading">"Loading..."</div>
                        </Show>
                        {move || {
                            let items = chat_list.get();
                            if items.is_empty() && !loading.get() {
                                view! {
                                    <div class="sidebar-empty">"No chats yet"</div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="chat-list">
                                        {items.into_iter().map(|c| {
                                            let cid = c.id.clone();
                                            view! {
                                                <div
                                                    class="chat-item"
                                                    on:click=move |_| set_selected_chat.set(Some(cid.clone()))
                                                >
                                                    <span class="chat-title">{c.title}</span>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                }.into_any()
                            }
                        }}
                    </Show>

                    // Chat messages view.
                    <Show when=move || selected_chat.get().is_some()>
                        <div class="chat-messages">
                            {move || {
                                let msgs = messages.get();
                                msgs.into_iter().map(|m| {
                                    view! {
                                        <div class="chat-message">
                                            <span class="chat-msg-author">{m.user_id}</span>
                                            <span class="chat-msg-content">{m.content}</span>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()
                            }}
                        </div>
                        <div class="chat-input">
                            <input
                                type="text"
                                class="chat-message-input"
                                placeholder="Type a message..."
                                prop:value=move || reply_text.get()
                                on:input=move |e| set_reply_text.set(event_target_value(&e))
                                on:keydown=move |e: web_sys::KeyboardEvent| {
                                    if e.key() == "Enter" && !e.shift_key() {
                                        e.prevent_default();
                                        send_reply_on_enter();
                                    }
                                }
                            />
                        </div>
                    </Show>

                    <Show when=move || selected_chat.get().is_none()>
                        <button class="sidebar-action-btn" on:click=move |_| {
                            // TODO: Open new chat dialog
                        }>"+ New Chat"</button>
                    </Show>
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

/// A message entry for display.
#[derive(Clone)]
struct MsgEntry {
    user_id: String,
    content: String,
    created_at: i64,
}
