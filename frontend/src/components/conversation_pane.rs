use leptos::prelude::*;

use crate::api::comments;
use crate::editor::state::EditorState;
use super::comment_highlights::InlineThreadInfo;

/// Conversation pane (right side panel) for document comments.
/// Shows document-level and block-anchored inline comment threads.
#[component]
pub fn ConversationPane(
    /// Whether the pane is visible.
    visible: ReadSignal<bool>,
    /// Document ID for loading threads.
    doc_id: ReadSignal<String>,
    /// Current editor state (for extracting block text).
    editor_state: ReadSignal<Option<EditorState>>,
    /// Block ID to attach the next comment to (set when user clicks Comment with cursor in a block).
    pending_block_id: ReadSignal<Option<String>>,
    /// Callback to clear the pending block ID after thread creation.
    on_block_used: Callback<()>,
    /// Callback to report inline threads for highlight rendering.
    on_threads_loaded: Callback<Vec<InlineThreadInfo>>,
    /// Auto-select this thread (set when user clicks a comment highlight).
    filter_thread_id: ReadSignal<Option<String>>,
) -> impl IntoView {
    let (threads, set_threads) = signal::<Vec<ThreadEntry>>(Vec::new());
    let (new_message, set_new_message) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (selected_thread, set_selected_thread) = signal::<Option<String>>(None);
    let (thread_messages, set_thread_messages) = signal::<Vec<MessageEntry>>(Vec::new());
    let (reply_text, set_reply_text) = signal(String::new());

    // Load threads when pane becomes visible or doc_id changes.
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        let id = doc_id.get();
        if id.is_empty() {
            return;
        }

        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match comments::list_threads(&id).await {
                Ok(resp) => {
                    let entries: Vec<ThreadEntry> = resp
                        .threads
                        .into_iter()
                        .map(|t| ThreadEntry {
                            thread_id: t.thread_id,
                            created_by: t.created_by,
                            status: t.status,
                            thread_type: t.thread_type,
                            block_id: t.block_id,
                            created_at: t.created_at,
                        })
                        .collect();
                    // Emit inline threads for highlight overlay.
                    let inline: Vec<InlineThreadInfo> = entries
                        .iter()
                        .filter(|t| t.thread_type == "inline" && t.block_id.is_some())
                        .map(|t| InlineThreadInfo {
                            thread_id: t.thread_id.clone(),
                            block_id: t.block_id.clone().unwrap(),
                        })
                        .collect();
                    on_threads_loaded.run(inline);
                    set_threads.set(entries);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load threads: {e}").into(),
                    );
                }
            }
            set_loading.set(false);
        });
    });

    // Auto-select thread when filter_thread_id changes.
    Effect::new(move |_| {
        if let Some(tid) = filter_thread_id.get() {
            set_selected_thread.set(Some(tid));
        }
    });

    // Load messages when a thread is selected.
    Effect::new(move |_| {
        let Some(tid) = selected_thread.get() else {
            set_thread_messages.set(Vec::new());
            return;
        };

        leptos::task::spawn_local(async move {
            match comments::list_messages(&tid).await {
                Ok(resp) => {
                    let msgs: Vec<MessageEntry> = resp
                        .messages
                        .into_iter()
                        .map(|m| MessageEntry {
                            user_id: m.user_id,
                            content: m.content,
                            created_at: m.created_at,
                        })
                        .collect();
                    set_thread_messages.set(msgs);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load messages: {e}").into(),
                    );
                }
            }
        });
    });

    let doc_id_for_create = doc_id.clone();
    let create_thread = move || {
        let msg = new_message.get_untracked();
        if msg.trim().is_empty() {
            return;
        }
        set_new_message.set(String::new());
        let id = doc_id_for_create.get_untracked();
        let block_id = pending_block_id.get_untracked();
        leptos::task::spawn_local(async move {
            match comments::create_thread(&id, &msg, block_id.as_deref()).await {
                Ok(resp) => {
                    let now = js_sys::Date::now() as i64 * 1000;
                    let user_id = crate::api::client::get_auth()
                        .map(|a| a.user_id)
                        .unwrap_or_default();
                    let thread_type = if block_id.is_some() { "inline" } else { "document" };
                    set_threads.update(|list| {
                        list.insert(0, ThreadEntry {
                            thread_id: resp.thread_id.clone(),
                            created_by: user_id,
                            status: "open".to_string(),
                            thread_type: thread_type.to_string(),
                            block_id: block_id.clone(),
                            created_at: now,
                        });
                    });
                    // Update highlights — threads signal already contains the new entry.
                    if block_id.is_some() {
                        let inline: Vec<InlineThreadInfo> = threads
                            .get_untracked()
                            .iter()
                            .filter(|t| t.thread_type == "inline" && t.block_id.is_some())
                            .map(|t| InlineThreadInfo {
                                thread_id: t.thread_id.clone(),
                                block_id: t.block_id.clone().unwrap(),
                            })
                            .collect();
                        on_threads_loaded.run(inline);
                    }
                    on_block_used.run(());
                    set_selected_thread.set(Some(resp.thread_id));
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to create thread: {e}").into(),
                    );
                }
            }
        });
    };

    let send_reply = move || {
        let msg = reply_text.get_untracked();
        if msg.trim().is_empty() {
            return;
        }
        let Some(tid) = selected_thread.get_untracked() else { return };
        set_reply_text.set(String::new());
        leptos::task::spawn_local(async move {
            match comments::add_message(&tid, &msg).await {
                Ok(()) => {
                    if let Ok(resp) = comments::list_messages(&tid).await {
                        let msgs: Vec<MessageEntry> = resp
                            .messages
                            .into_iter()
                            .map(|m| MessageEntry {
                                user_id: m.user_id,
                                content: m.content,
                                created_at: m.created_at,
                            })
                            .collect();
                        set_thread_messages.set(msgs);
                    }
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to send reply: {e}").into(),
                    );
                }
            }
        });
    };

    let create_thread_on_enter = create_thread.clone();
    let send_reply_on_enter = send_reply.clone();

    view! {
        <Show when=move || visible.get()>
            <div class="conversation-pane">
                <div class="conversation-header">
                    <span class="conversation-title">
                        {move || {
                            if selected_thread.get().is_some() {
                                "Thread"
                            } else if pending_block_id.get().is_some() {
                                "Comment on block"
                            } else {
                                "Comments"
                            }
                        }}
                    </span>
                    <Show when=move || selected_thread.get().is_some()>
                        <button
                            class="conversation-back"
                            on:click=move |_| set_selected_thread.set(None)
                        >"\u{2190} Back"</button>
                    </Show>
                </div>

                <div class="conversation-body">
                    <Show when=move || loading.get()>
                        <div class="conversation-loading">"Loading..."</div>
                    </Show>

                    // Thread list view.
                    <Show when=move || selected_thread.get().is_none()>
                        {move || {
                            let items = threads.get();
                            if items.is_empty() && !loading.get() {
                                view! {
                                    <div class="conversation-empty">
                                        "No comments yet. Start a conversation!"
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="conversation-threads">
                                        {items.into_iter().map(|t| {
                                            let tid = t.thread_id.clone();
                                            let is_inline = t.thread_type == "inline";
                                            let block_label = if is_inline {
                                                t.block_id.as_deref().unwrap_or("").to_string()
                                            } else {
                                                String::new()
                                            };
                                            let has_block = is_inline && !block_label.is_empty();
                                            view! {
                                                <div
                                                    class="conversation-thread"
                                                    on:click=move |_| set_selected_thread.set(Some(tid.clone()))
                                                >
                                                    <Show when=move || has_block>
                                                        <div class="thread-anchor-text">
                                                            "\u{1F4CE} Block comment"
                                                        </div>
                                                    </Show>
                                                    <div class="thread-author">{t.created_by}</div>
                                                    <div class="thread-status">{t.status}</div>
                                                    <div class="thread-time">{format_time(t.created_at)}</div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                }.into_any()
                            }
                        }}
                    </Show>

                    // Thread messages view.
                    <Show when=move || selected_thread.get().is_some()>
                        {move || {
                            let msgs = thread_messages.get();
                            view! {
                                <div class="thread-messages">
                                    {msgs.into_iter().map(|m| {
                                        view! {
                                            <div class="thread-message">
                                                <div class="message-author">{m.user_id}</div>
                                                <div class="message-content">{m.content}</div>
                                                <div class="message-time">{format_time(m.created_at)}</div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }
                        }}
                    </Show>
                </div>

                // Input area
                <div class="conversation-input">
                    <Show
                        when=move || selected_thread.get().is_some()
                        fallback=move || {
                            let create = create_thread_on_enter.clone();
                            let placeholder = if pending_block_id.get_untracked().is_some() {
                                "Comment on this block..."
                            } else {
                                "Add a comment..."
                            };
                            view! {
                                <input
                                    type="text"
                                    class="conversation-message-input"
                                    placeholder=placeholder
                                    prop:value=move || new_message.get()
                                    on:input=move |e| set_new_message.set(event_target_value(&e))
                                    on:keydown=move |e: web_sys::KeyboardEvent| {
                                        if e.key() == "Enter" && !e.shift_key() {
                                            e.prevent_default();
                                            create();
                                        }
                                    }
                                />
                            }
                        }
                    >
                        {
                            let reply = send_reply_on_enter.clone();
                            view! {
                                <input
                                    type="text"
                                    class="conversation-message-input"
                                    placeholder="Reply..."
                                    prop:value=move || reply_text.get()
                                    on:input=move |e| set_reply_text.set(event_target_value(&e))
                                    on:keydown=move |e: web_sys::KeyboardEvent| {
                                        if e.key() == "Enter" && !e.shift_key() {
                                            e.prevent_default();
                                            reply();
                                        }
                                    }
                                />
                            }
                        }
                    </Show>
                </div>
            </div>
        </Show>
    }
}

#[derive(Clone)]
struct ThreadEntry {
    thread_id: String,
    created_by: String,
    status: String,
    thread_type: String,
    block_id: Option<String>,
    created_at: i64,
}

#[derive(Clone)]
struct MessageEntry {
    user_id: String,
    content: String,
    created_at: i64,
}

fn format_time(timestamp_usec: i64) -> String {
    let now_ms = js_sys::Date::now() as i64;
    let ts_ms = timestamp_usec / 1000;
    let diff_secs = (now_ms - ts_ms) / 1000;

    if diff_secs < 60 {
        "just now".to_string()
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{}h ago", diff_secs / 3600)
    } else {
        format!("{}d ago", diff_secs / 86400)
    }
}
