use leptos::prelude::*;
use crate::api::comments;

/// Mode for the comment popup.
#[derive(Clone, PartialEq)]
enum PopupMode {
    /// Creating a new comment thread.
    New,
    /// Viewing/replying to an existing thread.
    Thread(String),
}

/// A comment popup that appears to the left of the document content,
/// aligned with the commented block. Supports both new comment creation
/// and viewing/replying to existing threads.
#[component]
pub fn CommentPopup(
    /// Thread ID to display (None = hidden, Some = existing thread).
    thread_id: ReadSignal<Option<String>>,
    /// Position top (viewport pixels).
    left: ReadSignal<f64>,
    top: ReadSignal<f64>,
    /// Document ID for creating new threads.
    doc_id: ReadSignal<String>,
    /// Block ID for new inline comments.
    block_id: ReadSignal<Option<String>>,
    /// Anchor start for new inline comments.
    anchor_start: ReadSignal<Option<u32>>,
    /// Anchor end for new inline comments.
    anchor_end: ReadSignal<Option<u32>>,
    /// Whether this is a new comment (no existing thread).
    is_new: ReadSignal<bool>,
    /// Callback to close the popup.
    on_close: Callback<()>,
    /// Callback when a new thread is created (passes thread_id).
    #[prop(default = Callback::new(|_: String| {}))]
    on_thread_created: Callback<String>,
) -> impl IntoView {
    let (messages, set_messages) = signal::<Vec<PopupMessage>>(Vec::new());
    let (new_comment_text, set_new_comment_text) = signal(String::new());
    let (reply_text, set_reply_text) = signal(String::new());
    let (loading, set_loading) = signal(false);
    let (mode, set_mode) = signal(PopupMode::New);

    // Determine mode from props.
    Effect::new(move |_| {
        if let Some(tid) = thread_id.get() {
            set_mode.set(PopupMode::Thread(tid));
        } else if is_new.get() {
            set_mode.set(PopupMode::New);
            set_messages.set(Vec::new());
            set_new_comment_text.set(String::new());
        }
    });

    // Load messages when in thread mode.
    Effect::new(move |_| {
        let PopupMode::Thread(tid) = mode.get() else {
            set_messages.set(Vec::new());
            return;
        };
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match comments::list_messages(&tid).await {
                Ok(resp) => {
                    set_messages.set(
                        resp.messages.into_iter().map(|m| PopupMessage {
                            user_name: if m.user_name.is_empty() { m.user_id } else { m.user_name },
                            content: m.content,
                            created_at: m.created_at,
                        }).collect()
                    );
                }
                Err(_) => {}
            }
            set_loading.set(false);
        });
    });

    // Create new thread.
    let create_thread = move || {
        let text = new_comment_text.get_untracked();
        if text.trim().is_empty() { return; }
        let doc = doc_id.get_untracked();
        let bid = block_id.get_untracked();
        let a_start = anchor_start.get_untracked();
        let a_end = anchor_end.get_untracked();
        set_new_comment_text.set(String::new());
        leptos::task::spawn_local(async move {
            match comments::create_thread(&doc, &text, bid.as_deref(), a_start, a_end).await {
                Ok(resp) => {
                    on_thread_created.run(resp.thread_id.clone());
                    set_mode.set(PopupMode::Thread(resp.thread_id));
                }
                Err(e) => {
                    web_sys::console::error_1(&format!("Comment failed: {e}").into());
                }
            }
        });
    };

    // Send reply in thread mode.
    let send_reply = move || {
        let text = reply_text.get_untracked();
        if text.trim().is_empty() { return; }
        let PopupMode::Thread(tid) = mode.get_untracked() else { return };
        set_reply_text.set(String::new());
        leptos::task::spawn_local(async move {
            if comments::add_message(&tid, &text).await.is_ok() {
                if let Ok(resp) = comments::list_messages(&tid).await {
                    set_messages.set(
                        resp.messages.into_iter().map(|m| PopupMessage {
                            user_name: if m.user_name.is_empty() { m.user_id } else { m.user_name },
                            content: m.content,
                            created_at: m.created_at,
                        }).collect()
                    );
                }
            }
        });
    };

    let create_on_enter = create_thread.clone();
    let reply_on_enter = send_reply.clone();

    let is_visible = move || thread_id.get().is_some() || is_new.get();

    view! {
        <Show when=is_visible>
            // Backdrop
            <div class="comment-popup-backdrop"
                on:click=move |_| on_close.run(())
            ></div>
            <div class="comment-popup"
                style:left=move || format!("{}px", left.get())
                style:top=move || format!("{}px", top.get())
            >
                // Header
                <div class="comment-popup-header">
                    <span class="comment-popup-title">
                        {move || {
                            match mode.get() {
                                PopupMode::New => "New Comment".to_string(),
                                PopupMode::Thread(_) => "Comment Thread".to_string(),
                            }
                        }}
                    </span>
                    <button class="comment-popup-close"
                        on:click=move |_| on_close.run(())
                    >"\u{2715}"</button>
                </div>

                // Body
                <div class="comment-popup-body">
                    {move || match mode.get() {
                        PopupMode::New => {
                            let create = create_on_enter.clone();
                            view! {
                                <textarea
                                    class="comment-popup-textarea"
                                    placeholder="Add a comment about this section"
                                    prop:value=move || new_comment_text.get()
                                    on:input=move |e| set_new_comment_text.set(event_target_value(&e))
                                    on:keydown=move |e: web_sys::KeyboardEvent| {
                                        if e.key() == "Enter" && (e.ctrl_key() || e.meta_key()) {
                                            e.prevent_default();
                                            create();
                                        }
                                    }
                                ></textarea>
                            }.into_any()
                        }
                        PopupMode::Thread(_) => {
                            view! {
                                <div class="comment-popup-messages">
                                    <Show when=move || loading.get()>
                                        <div class="comment-popup-loading">"Loading..."</div>
                                    </Show>
                                    {move || messages.get().into_iter().map(|m| {
                                        view! {
                                            <div class="comment-popup-msg">
                                                <div class="comment-popup-msg-header">
                                                    <span class="comment-popup-author">{m.user_name}</span>
                                                    <span class="comment-popup-time">{format_time(m.created_at)}</span>
                                                </div>
                                                <div class="comment-popup-text">{m.content}</div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }}
                </div>

                // Footer
                <div class="comment-popup-footer">
                    {move || match mode.get() {
                        PopupMode::New => {
                            let create = create_thread.clone();
                            view! {
                                <button
                                    class="comment-popup-send"
                                    on:click=move |_| create()
                                >"Send"</button>
                            }.into_any()
                        }
                        PopupMode::Thread(_) => {
                            let reply = reply_on_enter.clone();
                            view! {
                                <div class="comment-popup-reply-row">
                                    <input
                                        type="text"
                                        class="comment-popup-input"
                                        placeholder="Type a message..."
                                        prop:value=move || reply_text.get()
                                        on:input=move |e| set_reply_text.set(event_target_value(&e))
                                        on:keydown=move |e: web_sys::KeyboardEvent| {
                                            if e.key() == "Enter" && !e.shift_key() {
                                                e.prevent_default();
                                                reply();
                                            }
                                        }
                                    />
                                    <button
                                        class="comment-popup-send"
                                        on:click=move |_| send_reply()
                                    >"Send"</button>
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </Show>
    }
}

#[derive(Clone)]
struct PopupMessage {
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
        let date = js_sys::Date::new_0();
        date.set_time(ts_ms as f64);
        let day = date.get_day();
        match day {
            0 => "Sun", 1 => "Mon", 2 => "Tue", 3 => "Wed",
            4 => "Thu", 5 => "Fri", 6 => "Sat", _ => "?",
        }.to_string()
    } else {
        let date = js_sys::Date::new_0();
        date.set_time(ts_ms as f64);
        let month = date.get_month() + 1;
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
