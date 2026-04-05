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
    /// Text selection anchor within the block (offset from block content start).
    pending_anchor_start: ReadSignal<Option<u32>>,
    /// Text selection end within the block.
    pending_anchor_end: ReadSignal<Option<u32>>,
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
    let (error_msg, set_error_msg) = signal::<Option<String>>(None);
    let (selected_thread, set_selected_thread) = signal::<Option<String>>(None);
    let (thread_messages, set_thread_messages) = signal::<Vec<MessageEntry>>(Vec::new());
    let (reply_text, set_reply_text) = signal(String::new());

    // Shared thread-loading function (used by initial load + polling).
    let refresh_threads = {
        let set_threads = set_threads.clone();
        let set_loading = set_loading.clone();
        let on_threads_loaded = on_threads_loaded.clone();
        std::rc::Rc::new(move |id: String, show_loading: bool| {
            if id.is_empty() { return; }
            let set_threads = set_threads.clone();
            let set_loading = set_loading.clone();
            let on_threads_loaded = on_threads_loaded.clone();
            if show_loading { set_loading.set(true); }
            leptos::task::spawn_local(async move {
                match comments::list_threads(&id).await {
                    Ok(resp) => {
                        let entries: Vec<ThreadEntry> = resp
                            .threads
                            .into_iter()
                            .map(|t| ThreadEntry {
                                thread_id: t.thread_id,
                                created_by: t.created_by,
                                created_by_name: t.created_by_name,
                                status: t.status,
                                thread_type: t.thread_type,
                                block_id: t.block_id,
                                anchor_start: t.anchor_start,
                                anchor_end: t.anchor_end,
                                first_message: t.first_message,
                                created_at: t.created_at,
                            })
                            .collect();
                        let inline: Vec<InlineThreadInfo> = entries
                            .iter()
                            .filter(|t| t.thread_type == "inline" && t.block_id.is_some())
                            .map(|t| InlineThreadInfo {
                                thread_id: t.thread_id.clone(),
                                block_id: t.block_id.clone().unwrap(),
                                anchor_start: t.anchor_start,
                                anchor_end: t.anchor_end,
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
        })
    };

    // Load threads when pane becomes visible or doc_id changes.
    {
        let refresh = refresh_threads.clone();
        Effect::new(move |_| {
            if !visible.get() { return; }
            refresh(doc_id.get(), true);
        });
    }

    // Poll for new threads every 10 seconds while pane is visible.
    {
        let refresh = refresh_threads.clone();
        let poll_handle: std::rc::Rc<std::cell::RefCell<Option<gloo_timers::callback::Interval>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let poll_ref = poll_handle.clone();
        Effect::new(move |_| {
            if visible.get() {
                let refresh = refresh.clone();
                let doc = doc_id.get_untracked();
                *poll_ref.borrow_mut() = Some(gloo_timers::callback::Interval::new(10_000, move || {
                    refresh(doc.clone(), false);
                }));
            } else {
                *poll_ref.borrow_mut() = None; // stop polling
            }
        });
    }

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
                            user_name: if m.user_name.is_empty() { m.user_id } else { m.user_name },
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
        set_error_msg.set(None);
        let id = doc_id_for_create.get_untracked();
        let block_id = pending_block_id.get_untracked();
        let anchor_start = pending_anchor_start.get_untracked();
        let anchor_end = pending_anchor_end.get_untracked();
        leptos::task::spawn_local(async move {
            match comments::create_thread(&id, &msg, block_id.as_deref(), anchor_start, anchor_end).await {
                Ok(resp) => {
                    let now = js_sys::Date::now() as i64 * 1000;
                    let auth = crate::api::client::get_auth();
                    let user_id = auth.as_ref().map(|a| a.user_id.clone()).unwrap_or_default();
                    let user_name = auth.as_ref().map(|a| a.name.clone()).unwrap_or_else(|| user_id.clone());
                    let thread_type = if block_id.is_some() { "inline" } else { "document" };
                    let preview = if msg.len() > 120 {
                        let mut p = msg[..120].to_string();
                        p.push_str("...");
                        Some(p)
                    } else {
                        Some(msg.clone())
                    };
                    set_threads.update(|list| {
                        list.insert(0, ThreadEntry {
                            thread_id: resp.thread_id.clone(),
                            created_by: user_id,
                            created_by_name: user_name,
                            status: "open".to_string(),
                            thread_type: thread_type.to_string(),
                            block_id: block_id.clone(),
                            anchor_start,
                            anchor_end,
                            first_message: preview,
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
                                anchor_start: t.anchor_start,
                                anchor_end: t.anchor_end,
                            })
                            .collect();
                        on_threads_loaded.run(inline);
                    }
                    on_block_used.run(());
                    set_selected_thread.set(Some(resp.thread_id));
                }
                Err(crate::api::client::ApiClientError::Http(409, _)) => {
                    set_error_msg.set(Some("This block already has a comment. Select the existing thread to reply.".to_string()));
                }
                Err(e) => {
                    set_error_msg.set(Some(format!("Failed to create thread: {e}")));
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
                                user_name: if m.user_name.is_empty() { m.user_id } else { m.user_name },
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
                    {move || error_msg.get().map(|msg| view! {
                        <div class="conversation-error"
                            on:click=move |_| set_error_msg.set(None)
                        >{msg}" \u{2715}"</div>
                    })}

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
                                            let tid_for_click = tid.clone();
                                            let tid_for_resolve = tid.clone();
                                            let tid_for_delete = tid.clone();
                                            let is_resolved = t.status == "resolved";
                                            let status_label = if is_resolved { "Resolved" } else { "Open" };
                                            let preview = t.first_message.clone();
                                            view! {
                                                <div
                                                    class=if is_resolved { "conversation-thread thread-resolved" } else { "conversation-thread" }
                                                    on:click=move |_| set_selected_thread.set(Some(tid_for_click.clone()))
                                                >
                                                    <div class="thread-meta">
                                                        <span class="thread-author">{t.created_by_name.clone()}</span>
                                                        <span class="thread-status">{status_label}</span>
                                                        <span class="thread-time">{format_time(t.created_at)}</span>
                                                    </div>
                                                    {preview.map(|text| view! {
                                                        <div class="thread-preview">{text}</div>
                                                    })}
                                                    <div class="thread-actions">
                                                        <button
                                                            class="thread-action-btn"
                                                            title=if is_resolved { "Reopen" } else { "Resolve" }
                                                            on:click=move |e: web_sys::MouseEvent| {
                                                                e.stop_propagation();
                                                                let tid = tid_for_resolve.clone();
                                                                let new_status = if is_resolved { "open" } else { "resolved" };
                                                                let set_threads = set_threads.clone();
                                                                leptos::task::spawn_local(async move {
                                                                    if comments::update_thread_status(&tid, new_status).await.is_ok() {
                                                                        set_threads.update(|list| {
                                                                            if let Some(t) = list.iter_mut().find(|t| t.thread_id == tid) {
                                                                                t.status = new_status.to_string();
                                                                            }
                                                                        });
                                                                    }
                                                                });
                                                            }
                                                        >{if is_resolved { "\u{21BB}" } else { "\u{2713}" }}</button>
                                                        <button
                                                            class="thread-action-btn thread-delete-btn"
                                                            title="Delete"
                                                            on:click=move |e: web_sys::MouseEvent| {
                                                                e.stop_propagation();
                                                                let tid = tid_for_delete.clone();
                                                                let set_threads = set_threads.clone();
                                                                let on_threads_loaded = on_threads_loaded.clone();
                                                                leptos::task::spawn_local(async move {
                                                                    if comments::delete_thread(&tid).await.is_ok() {
                                                                        set_threads.update(|list| {
                                                                            list.retain(|t| t.thread_id != tid);
                                                                        });
                                                                        // Update highlights
                                                                        let inline: Vec<InlineThreadInfo> = threads
                                                                            .get_untracked()
                                                                            .iter()
                                                                            .filter(|t| t.thread_type == "inline" && t.block_id.is_some())
                                                                            .map(|t| InlineThreadInfo {
                                                                                thread_id: t.thread_id.clone(),
                                                                                block_id: t.block_id.clone().unwrap(),
                                                                                anchor_start: t.anchor_start,
                                                                                anchor_end: t.anchor_end,
                                                                            })
                                                                            .collect();
                                                                        on_threads_loaded.run(inline);
                                                                    }
                                                                });
                                                            }
                                                        >"\u{1F5D1}"</button>
                                                    </div>
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
                                                <div class="message-author">{m.user_name}</div>
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
    created_by_name: String,
    status: String,
    thread_type: String,
    block_id: Option<String>,
    anchor_start: Option<u32>,
    anchor_end: Option<u32>,
    first_message: Option<String>,
    created_at: i64,
}

#[derive(Clone)]
struct MessageEntry {
    user_name: String,
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
    } else if diff_secs < 604800 {
        // Within the last week — show day name
        let date = js_sys::Date::new_0();
        date.set_time(ts_ms as f64);
        let day = date.get_day(); // 0=Sun, 1=Mon, ...
        let day_name = match day {
            0 => "Sun", 1 => "Mon", 2 => "Tue", 3 => "Wed",
            4 => "Thu", 5 => "Fri", 6 => "Sat", _ => "?",
        };
        day_name.to_string()
    } else {
        // Older than a week — show Month/Day
        let date = js_sys::Date::new_0();
        date.set_time(ts_ms as f64);
        let month = date.get_month() + 1; // 0-indexed
        let day = date.get_date();
        let month_name = match month {
            1 => "Jan", 2 => "Feb", 3 => "Mar", 4 => "Apr",
            5 => "May", 6 => "Jun", 7 => "Jul", 8 => "Aug",
            9 => "Sep", 10 => "Oct", 11 => "Nov", 12 => "Dec",
            _ => "?",
        };
        format!("{month_name} {day}")
    }
}
