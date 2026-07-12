// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use crate::a11y;
use crate::api::comments;
use crate::i18n::format_relative;

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
    /// Bumped by the page whenever a peer's CommentEvent arrives. Tracked
    /// by the load-messages Effect so a peer's reply lands in this dialog
    /// without the user having to close + reopen it.
    comments_dirty: ReadSignal<u32>,
    /// Callback to close the popup.
    on_close: Callback<()>,
    /// Callback when a new thread is created (passes thread_id).
    #[prop(default = Callback::new(|_: String| {}))]
    on_thread_created: Callback<String>,
    /// Navigate to the previous comment thread.
    #[prop(default = Callback::new(|_: ()| {}))]
    on_prev: Callback<()>,
    /// Navigate to the next comment thread.
    #[prop(default = Callback::new(|_: ()| {}))]
    on_next: Callback<()>,
) -> impl IntoView {
    let (messages, set_messages) = signal::<Vec<PopupMessage>>(Vec::new());
    let (new_comment_text, set_new_comment_text) = signal(String::new());
    let (reply_text, set_reply_text) = signal(String::new());
    let (editing_id, set_editing_id) = signal::<Option<String>>(None);
    let (edit_text, set_edit_text) = signal(String::new());
    // Author id for gating the Edit affordance. Stable for the session, so
    // read once; the server also enforces author-only, this is UX only.
    // Held in a `StoredValue` (Copy) rather than a plain `String` because it
    // flows through several nested `move` reactive closures below (the mode
    // match, the message-list render, per-message edit buttons) — each of
    // which must implement `Fn` and gets (re)constructed on every reactive
    // re-run, so a plain owned `String` would be moved out on the first run.
    let current_uid: StoredValue<String> = StoredValue::new(
        crate::api::client::get_auth().map(|a| a.user_id).unwrap_or_default()
    );
    let (loading, set_loading) = signal(false);
    let (mode, set_mode) = signal(PopupMode::New);
    let messages_ref = NodeRef::<leptos::html::Div>::new();
    // M-P8 piece A: focus trap on the popup. The visibility derived
    // from thread_id+is_new flips synchronously when a thread opens,
    // so we wrap it in a Signal for the helper.
    let popup_ref = NodeRef::<leptos::html::Div>::new();
    let visible_sig = Signal::derive(move || {
        thread_id.get().is_some() || is_new.get()
    });
    a11y::install_focus_trap(popup_ref, visible_sig);

    // Scroll the messages container to the bottom whenever messages change.
    // Without this, a thread long enough to overflow the popup loads with
    // the scrollbar at the top — and a peer's just-broadcast reply lands
    // off-screen at the bottom of the list. The 0ms timeout defers past
    // the current microtask drain so Leptos has actually committed the
    // new children to the DOM before we read scrollHeight.
    Effect::new(move |_| {
        let _ = messages.get();
        if let Some(el) = messages_ref.get() {
            let el = el.clone();
            gloo_timers::callback::Timeout::new(0, move || {
                el.set_scroll_top(el.scroll_height());
            }).forget();
        }
    });

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

    // Load messages when in thread mode. Also re-runs on `comments_dirty`
    // bumps so a peer's reply broadcast lands in an open dialog.
    Effect::new(move |_| {
        let _ = comments_dirty.get();
        let PopupMode::Thread(tid) = mode.get() else {
            set_messages.set(Vec::new());
            return;
        };
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match comments::list_messages(&tid).await {
                Ok(resp) => {
                    set_messages.set(resp.messages.into_iter().map(to_popup).collect());
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
                    set_messages.set(resp.messages.into_iter().map(to_popup).collect());
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
                on:click=move |_| a11y::defer_close(on_close)
            ></div>
            <div class="comment-popup"
                node_ref=popup_ref
                role="dialog"
                aria-modal="true"
                aria-labelledby="comment-popup-title"
                style:left=move || format!("{}px", left.get())
                style:top=move || format!("{}px", top.get())
                on:keydown=move |e: web_sys::KeyboardEvent| {
                    if e.key() == "Escape" {
                        a11y::defer_close(on_close);
                        return;
                    }
                    if let Some(node) = popup_ref.get() {
                        a11y::handle_tab_trap(&e, node.as_ref());
                    }
                }
            >
                // Header
                <div class="comment-popup-header">
                    <span id="comment-popup-title" class="comment-popup-title">
                        {move || {
                            match mode.get() {
                                PopupMode::New => crate::t!("comment-new-title"),
                                PopupMode::Thread(_) => crate::t!("comment-thread-title"),
                            }
                        }}
                    </span>
                    <div class="comment-popup-header-actions">
                        <Show when=move || matches!(mode.get(), PopupMode::Thread(_))>
                            <button class="comment-nav-btn"
                                title=crate::t!("comment-aria-prev")
                                aria-label=crate::t!("comment-aria-prev")
                                on:mousedown=move |e: web_sys::MouseEvent| {
                                    e.prevent_default();
                                    on_prev.run(());
                                }
                            >"\u{25B2}"</button>
                            <button class="comment-nav-btn"
                                title=crate::t!("comment-aria-next")
                                aria-label=crate::t!("comment-aria-next")
                                on:mousedown=move |e: web_sys::MouseEvent| {
                                    e.prevent_default();
                                    on_next.run(());
                                }
                            >"\u{25BC}"</button>
                        </Show>
                        <button class="comment-popup-close"
                            on:click=move |_| a11y::defer_close(on_close)
                        >"\u{2715}"</button>
                    </div>
                </div>

                // Body — this is the scroll container per the CSS
                // (.comment-popup-body has overflow-y: auto), so the
                // scroll-to-bottom NodeRef must attach here, not to
                // .comment-popup-messages (which is just a non-scrolling
                // padded inner wrapper).
                <div class="comment-popup-body" node_ref=messages_ref>
                    {move || match mode.get() {
                        PopupMode::New => {
                            let create = create_on_enter.clone();
                            view! {
                                <textarea
                                    class="comment-popup-textarea"
                                    data-autofocus="true"
                                    placeholder=crate::t!("comment-placeholder-new")
                                    prop:value=move || new_comment_text.get()
                                    on:input=move |e| set_new_comment_text.set(event_target_value(&e))
                                    on:keydown=move |e: web_sys::KeyboardEvent| {
                                        if e.key() == "Enter" && !e.shift_key() {
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
                                        <div class="comment-popup-loading">{crate::t!("common-loading")}</div>
                                    </Show>
                                    {move || {
                                        let current_uid = current_uid.get_value();
                                        let editing = editing_id.get();
                                        messages.get().into_iter().map(|m| {
                                            let is_author = m.user_id == current_uid;
                                            let is_editing = editing.as_deref() == Some(m.message_id.as_str());
                                            let mid = m.message_id.clone();

                                            if is_editing {
                                                let save = {
                                                    let mid = mid.clone();
                                                    move || {
                                                        let text = edit_text.get_untracked();
                                                        if text.trim().is_empty() { return; }
                                                        let PopupMode::Thread(tid) = mode.get_untracked() else { return };
                                                        let mid = mid.clone();
                                                        set_editing_id.set(None);
                                                        leptos::task::spawn_local(async move {
                                                            if comments::edit_message(&tid, &mid, &text).await.is_ok() {
                                                                if let Ok(resp) = comments::list_messages(&tid).await {
                                                                    set_messages.set(resp.messages.into_iter().map(to_popup).collect());
                                                                }
                                                            }
                                                        });
                                                    }
                                                };
                                                let save_click = save.clone();
                                                view! {
                                                    <div class="comment-popup-msg">
                                                        <div class="comment-popup-msg-header">
                                                            <span class="comment-popup-author">{m.user_name.clone()}</span>
                                                        </div>
                                                        <textarea
                                                            class="comment-popup-textarea"
                                                            prop:value=move || edit_text.get()
                                                            on:input=move |e| set_edit_text.set(event_target_value(&e))
                                                            on:keydown={
                                                                let save = save.clone();
                                                                move |e: web_sys::KeyboardEvent| {
                                                                    if e.key() == "Enter" && !e.shift_key() {
                                                                        e.prevent_default();
                                                                        save();
                                                                    }
                                                                }
                                                            }
                                                        ></textarea>
                                                        <div class="comment-edit-actions">
                                                            <button
                                                                class="comment-popup-send"
                                                                on:click=move |_| save_click()
                                                            >{crate::t!("comment-save")}</button>
                                                            <button
                                                                class="comment-edit-cancel"
                                                                on:click=move |_| set_editing_id.set(None)
                                                            >{crate::t!("common-cancel")}</button>
                                                        </div>
                                                    </div>
                                                }.into_any()
                                            } else {
                                                let edit_btn = is_author.then(|| {
                                                    let mid = mid.clone();
                                                    let content = m.content.clone();
                                                    view! {
                                                        <button
                                                            class="comment-edit-btn"
                                                            on:click=move |_| {
                                                                set_edit_text.set(content.clone());
                                                                set_editing_id.set(Some(mid.clone()));
                                                            }
                                                        >{crate::t!("comment-edit")}</button>
                                                    }
                                                });
                                                let edited_marker = m.updated_at.map(|ts| view! {
                                                    <span
                                                        class="comment-popup-edited"
                                                        title=crate::i18n::format_date(ts, crate::i18n::DateStyle::Long)
                                                    >{crate::t!("comment-edited")}</span>
                                                });
                                                view! {
                                                    <div class="comment-popup-msg">
                                                        <div class="comment-popup-msg-header">
                                                            <span class="comment-popup-author">{m.user_name.clone()}</span>
                                                            <span class="comment-popup-time">{format_relative(m.created_at)}</span>
                                                            {edited_marker}
                                                        </div>
                                                        <div class="comment-popup-text">{m.content.clone()}</div>
                                                        {edit_btn}
                                                    </div>
                                                }.into_any()
                                            }
                                        }).collect::<Vec<_>>()
                                    }}
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
                                >{crate::t!("common-send")}</button>
                            }.into_any()
                        }
                        PopupMode::Thread(_) => {
                            let reply = reply_on_enter.clone();
                            view! {
                                <div class="comment-popup-reply-row">
                                    <input
                                        type="text"
                                        class="comment-popup-input"
                                        data-autofocus="true"
                                        placeholder=crate::t!("comment-placeholder-reply")
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
                                    >{crate::t!("common-send")}</button>
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
    message_id: String,
    user_id: String,
    user_name: String,
    content: String,
    created_at: i64,
    updated_at: Option<i64>,
}

/// Map a wire `MessageItem` into the popup's view model. Centralizes the
/// user_name fallback + field copy shared by the initial load, reply, and
/// edit reload paths.
fn to_popup(m: comments::MessageItem) -> PopupMessage {
    PopupMessage {
        message_id: m.message_id,
        user_id: m.user_id.clone(),
        user_name: if m.user_name.is_empty() { m.user_id } else { m.user_name },
        content: m.content,
        created_at: m.created_at,
        updated_at: m.updated_at,
    }
}

