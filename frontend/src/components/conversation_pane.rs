use leptos::prelude::*;

/// Conversation pane (right side panel) for document comments.
/// Shows document-level and inline comment threads.
#[component]
pub fn ConversationPane(
    /// Whether the pane is visible.
    visible: ReadSignal<bool>,
    /// Document ID for loading threads.
    doc_id: ReadSignal<String>,
) -> impl IntoView {
    let (threads, set_threads) = signal::<Vec<ThreadEntry>>(Vec::new());
    let (new_message, set_new_message) = signal(String::new());

    // Load threads when pane becomes visible
    let doc_id_for_load = doc_id.clone();
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        let id = doc_id_for_load.get();
        if id.is_empty() {
            return;
        }

        leptos::task::spawn_local(async move {
            // TODO: Call GET /documents/:id/threads API
            // For now, show empty state
            set_threads.set(Vec::new());
        });
    });

    view! {
        <Show when=move || visible.get()>
            <div class="conversation-pane">
                <div class="conversation-header">
                    <span class="conversation-title">"Comments"</span>
                </div>

                <div class="conversation-body">
                    {move || {
                        let items = threads.get();
                        if items.is_empty() {
                            view! {
                                <div class="conversation-empty">
                                    "No comments yet. Start a conversation!"
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="conversation-threads">
                                    {items.into_iter().map(|t| {
                                        view! {
                                            <div class="conversation-thread">
                                                <div class="thread-author">{t.created_by}</div>
                                                <div class="thread-content">{t.first_message}</div>
                                                <div class="thread-time">{format_time(t.created_at)}</div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }}
                </div>

                <div class="conversation-input">
                    <input
                        type="text"
                        class="conversation-message-input"
                        placeholder="Add a comment..."
                        prop:value=move || new_message.get()
                        on:input=move |e| set_new_message.set(event_target_value(&e))
                        on:keydown=move |e: web_sys::KeyboardEvent| {
                            if e.key() == "Enter" && !e.shift_key() {
                                e.prevent_default();
                                let msg = new_message.get_untracked();
                                if !msg.trim().is_empty() {
                                    set_new_message.set(String::new());
                                    // TODO: POST to /documents/:id/threads with the message
                                }
                            }
                        }
                    />
                </div>
            </div>
        </Show>
    }
}

/// A thread entry for display.
#[derive(Clone)]
struct ThreadEntry {
    thread_id: String,
    created_by: String,
    first_message: String,
    created_at: i64,
}

/// Simple time formatting (relative).
fn format_time(timestamp_usec: i64) -> String {
    let now_ms = js_sys::Date::now() as i64;
    let ts_ms = timestamp_usec / 1000; // usec to ms
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
