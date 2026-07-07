// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

use crate::api::comments;
use crate::collab::ws_client::RemoteCursor;
use crate::editor::state::EditorState;
use super::comment_highlights::InlineThreadInfo;
use crate::i18n::format_relative;

/// How long after the last keystroke we declare typing finished and clear
/// `typing_thread_id`. Common chat clients use ~5s; 2s feels snappier given the
/// "X is typing…" indicator is purely social affordance.
const TYPING_IDLE_TIMEOUT_MS: u32 = 2000;

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
    /// Remote awareness states; used to render "X is typing…" for the
    /// currently-open thread.
    remote_cursors: ReadSignal<Vec<RemoteCursor>>,
    /// Notify the document page when the local user starts/stops typing
    /// into a thread. The page rebroadcasts via the awareness channel.
    on_typing_change: Callback<Option<String>>,
    /// Callback when a thread is clicked — passes (thread_id, block_id) to scroll + open popup.
    on_thread_click: Callback<(String, Option<String>)>,
    /// Bumped by the page whenever a peer's CommentEvent arrives. Tracked
    /// by the thread-list and per-thread message Effects so a peer's
    /// create / status / reply lands in this pane immediately instead of
    /// waiting for the 10-second poll.
    comments_dirty: ReadSignal<u32>,
) -> impl IntoView {
    let (threads, set_threads) = signal::<Vec<ThreadEntry>>(Vec::new());
    let (new_message, set_new_message) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (error_msg, set_error_msg) = signal::<Option<String>>(None);
    let (selected_thread, set_selected_thread) = signal::<Option<String>>(None);
    let (thread_messages, set_thread_messages) = signal::<Vec<MessageEntry>>(Vec::new());
    let (reply_text, set_reply_text) = signal(String::new());
    // Index of the currently focused thread for prev/next navigation (-1 = none).
    let (nav_index, set_nav_index) = signal(-1i32);
    let thread_messages_ref = NodeRef::<leptos::html::Div>::new();

    // Scroll the message list to the bottom whenever it changes (initial
    // load, peer-broadcast reply, local send). Without this, a thread
    // with more messages than fit in the pane opens with the latest
    // message off-screen at the bottom. The 0ms timeout defers past the
    // current microtask drain so Leptos has committed the new children
    // before we read scrollHeight.
    Effect::new(move |_| {
        let _ = thread_messages.get();
        if let Some(el) = thread_messages_ref.get() {
            let el = el.clone();
            gloo_timers::callback::Timeout::new(0, move || {
                el.set_scroll_top(el.scroll_height());
            }).forget();
        }
    });

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
                        let mut entries: Vec<ThreadEntry> = resp
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
                        entries.sort_by_key(|t| t.created_at);
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

    // Load threads when pane becomes visible, doc_id changes, or a peer's
    // CommentEvent bumps `comments_dirty`. The bump triggers an instant
    // refetch instead of waiting for the 10-second poll.
    {
        let refresh = refresh_threads.clone();
        Effect::new(move |_| {
            let _ = comments_dirty.get();
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

    // Load messages when a thread is selected. Also re-runs on
    // `comments_dirty` bumps so a peer's reply broadcast lands in the
    // open thread without waiting for the user to reselect it.
    Effect::new(move |_| {
        let _ = comments_dirty.get();
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

    // Typing-indicator broadcast: while the user is typing into a thread,
    // notify the page (which forwards it through the awareness channel).
    // Idle for `TYPING_IDLE_TIMEOUT_MS` and the indicator clears even if
    // the user never blurs the input. The timer lives in a `StoredValue`
    // with `LocalStorage` because `gloo_timers::Timeout` is !Send and
    // Leptos 0.7 event handlers run on a Send+Sync scheduler.
    let typing_timer: StoredValue<
        Option<gloo_timers::callback::Timeout>,
        leptos::reactive::owner::LocalStorage,
    > = StoredValue::new_local(None);
    let notify_typing = move |thread_id: String| {
        on_typing_change.run(Some(thread_id));
        let timeout = gloo_timers::callback::Timeout::new(TYPING_IDLE_TIMEOUT_MS, move || {
            on_typing_change.run(None);
            typing_timer.update_value(|slot| *slot = None);
        });
        typing_timer.update_value(|slot| *slot = Some(timeout));
    };
    let clear_typing = move || {
        typing_timer.update_value(|slot| *slot = None);
        on_typing_change.run(None);
    };

    // Clear the typing indicator when the pane is hidden by something
    // other than the reply input losing focus — backdrop click, menu-bar
    // toggle, Ctrl+Alt+C — so peers don't see a stuck "X is typing…"
    // for the full 2s idle window.
    Effect::new(move |prev: Option<bool>| {
        let now_visible = visible.get();
        if matches!(prev, Some(true)) && !now_visible {
            clear_typing();
        }
        now_visible
    });

    view! {
        // Always-render the outer wrapper so CSS slide transitions on
        // `.is-open` fire when visibility flips; gate the heavy inner
        // content (threads list, message editor) on `visible` so we don't
        // pay rendering cost when the drawer is hidden.
        <div class="conversation-pane" class:is-open=move || visible.get()>
        <Show when=move || visible.get()>
                <div class="conversation-header">
                    <span class="conversation-title">
                        {move || {
                            if selected_thread.get().is_some() {
                                crate::t!("conversation-thread")
                            } else if pending_block_id.get().is_some() {
                                crate::t!("conversation-comment-on-block")
                            } else {
                                crate::t!("conversation-comments")
                            }
                        }}
                    </span>
                    <div class="conversation-header-actions">
                        <Show when=move || selected_thread.get().is_some()>
                            <button
                                class="conversation-back"
                                on:click=move |_| {
                                    clear_typing();
                                    set_selected_thread.set(None);
                                }
                            >{crate::t!("conversation-back")}</button>
                        </Show>
                        <Show when=move || selected_thread.get().is_none() && !threads.get().is_empty()>
                            <button
                                class="comment-nav-btn"
                                title=crate::t!("conversation-aria-prev")
                                aria-label=crate::t!("conversation-aria-prev")
                                on:click=move |_| {
                                    let items = threads.get_untracked();
                                    if items.is_empty() { return; }
                                    let count = items.len() as i32;
                                    let idx = nav_index.get_untracked();
                                    let new_idx = if idx <= 0 { count - 1 } else { idx - 1 };
                                    set_nav_index.set(new_idx);
                                    let t = &items[new_idx as usize];
                                    on_thread_click.run((t.thread_id.clone(), t.block_id.clone()));
                                }
                            >"\u{25B2}"</button>
                            <button
                                class="comment-nav-btn"
                                title=crate::t!("conversation-aria-next")
                                aria-label=crate::t!("conversation-aria-next")
                                on:click=move |_| {
                                    let items = threads.get_untracked();
                                    if items.is_empty() { return; }
                                    let count = items.len() as i32;
                                    let idx = nav_index.get_untracked();
                                    let new_idx = if idx >= count - 1 { 0 } else { idx + 1 };
                                    set_nav_index.set(new_idx);
                                    let t = &items[new_idx as usize];
                                    on_thread_click.run((t.thread_id.clone(), t.block_id.clone()));
                                }
                            >"\u{25BC}"</button>
                        </Show>
                    </div>
                </div>

                // .conversation-body is the scroll container per the CSS
                // (overflow-y: auto, flex: 1). The NodeRef has to live
                // here — not on the inner .thread-messages — for
                // set_scroll_top to actually move the visible viewport.
                <div class="conversation-body" node_ref=thread_messages_ref>
                    <Show when=move || loading.get()>
                        <div class="conversation-loading">{crate::t!("common-loading")}</div>
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
                                        {crate::t!("conversation-empty")}
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
                                            let status_label = if is_resolved {
                                                crate::t!("conversation-status-resolved")
                                            } else {
                                                crate::t!("conversation-status-open")
                                            };
                                            let preview = t.first_message.clone();
                                            let block_id_for_click = t.block_id.clone();
                                            view! {
                                                <div
                                                    class=if is_resolved { "conversation-thread thread-resolved" } else { "conversation-thread" }
                                                    on:click=move |_| {
                                                        on_thread_click.run((tid_for_click.clone(), block_id_for_click.clone()));
                                                    }
                                                >
                                                    <div class="thread-meta">
                                                        <span class="thread-author">{t.created_by_name.clone()}</span>
                                                        <span class="thread-status">{status_label}</span>
                                                        <span class="thread-time">{format_relative(t.created_at)}</span>
                                                    </div>
                                                    {preview.map(|text| view! {
                                                        <div class="thread-preview">{text}</div>
                                                    })}
                                                    <div class="thread-actions">
                                                        <button
                                                            class="thread-action-btn"
                                                            title=if is_resolved { crate::t!("conversation-reopen") } else { crate::t!("conversation-resolve") }
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
                                                            title=crate::t!("common-delete")
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
                        <div class="thread-messages">
                            {move || {
                                thread_messages.get().into_iter().map(|m| {
                                    view! {
                                        <div class="thread-message">
                                            <div class="message-author">{m.user_name}</div>
                                            <div class="message-content">{m.content}</div>
                                            <div class="message-time">{format_relative(m.created_at)}</div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()
                            }}
                            {move || {
                                let Some(active) = selected_thread.get() else {
                                    return view! { <div></div> }.into_any();
                                };
                                let mut names: Vec<String> = remote_cursors
                                    .get()
                                    .into_iter()
                                    .filter(|c| c.typing_thread_id.as_deref() == Some(active.as_str()))
                                    .map(|c| if c.name.is_empty() { c.user_id } else { c.name })
                                    .collect();
                                names.sort();
                                names.dedup();
                                if names.is_empty() {
                                    view! { <div></div> }.into_any()
                                } else {
                                    let label = match names.len() {
                                        1 => crate::t!("conversation-typing-1", name = names[0].clone()),
                                        2 => crate::t!("conversation-typing-2", a = names[0].clone(), b = names[1].clone()),
                                        _ => crate::t!("conversation-typing-many"),
                                    };
                                    view! {
                                        <div class="thread-typing-indicator">{label}</div>
                                    }.into_any()
                                }
                            }}
                        </div>
                    </Show>
                </div>

                // Input area
                <div class="conversation-input">
                    <Show
                        when=move || selected_thread.get().is_some()
                        fallback=move || {
                            let create = create_thread_on_enter.clone();
                            let placeholder = if pending_block_id.get_untracked().is_some() {
                                crate::t!("conversation-placeholder-block")
                            } else {
                                crate::t!("conversation-placeholder-add")
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
                            let notify_input = notify_typing.clone();
                            let notify_key = notify_typing.clone();
                            let clear_blur = clear_typing.clone();
                            view! {
                                <input
                                    type="text"
                                    class="conversation-message-input"
                                    placeholder=crate::t!("conversation-placeholder-reply")
                                    prop:value=move || reply_text.get()
                                    on:input=move |e| {
                                        set_reply_text.set(event_target_value(&e));
                                        if let Some(tid) = selected_thread.get_untracked() {
                                            notify_input(tid);
                                        }
                                    }
                                    on:keydown=move |e: web_sys::KeyboardEvent| {
                                        if e.key() == "Enter" && !e.shift_key() {
                                            e.prevent_default();
                                            reply();
                                        } else if let Some(tid) = selected_thread.get_untracked() {
                                            notify_key(tid);
                                        }
                                    }
                                    on:blur=move |_| clear_blur()
                                />
                            }
                        }
                    </Show>
                </div>
        </Show>
        </div>
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

