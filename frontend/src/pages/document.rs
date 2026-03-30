use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};

use crate::api::client;
use crate::api::documents;
use crate::collab::ws_client::CollabClient;
use crate::components::conversation_pane::ConversationPane;
use crate::components::document_outline::DocumentOutline;
use crate::components::editor_component::{EditorComponent, EditorProps};
use crate::components::notification_bell::NotificationBell;
use crate::components::share_dialog::ShareDialog;
use crate::components::sidebar::Sidebar;
use crate::components::toolbar::{Toolbar, ToolbarCommand};
use crate::editor::state::EditorState;

#[component]
pub fn DocumentPage() -> impl IntoView {
    if !client::is_authenticated() {
        let navigate = use_navigate();
        navigate("/login", Default::default());
        return view! { <div>"Redirecting to login..."</div> }.into_any();
    }

    let params = use_params_map();
    let doc_id = move || params.read().get("id").unwrap_or_default();

    let (title, set_title) = signal("Loading...".to_string());
    let (error, set_error) = signal::<Option<String>>(None);
    let (current_id, set_current_id) = signal(String::new());
    let (initial_content, set_initial_content) = signal::<Option<Vec<u8>>>(None);
    let (content_loaded, set_content_loaded) = signal(false);
    let (editor_state, set_editor_state) = signal::<Option<EditorState>>(None);
    let (toolbar_command, set_toolbar_command) = signal::<Option<ToolbarCommand>>(None);
    let (outline_visible, set_outline_visible) = signal(false);
    let (share_visible, set_share_visible) = signal(false);
    let (conversation_visible, set_conversation_visible) = signal(false);
    // Track whether WS is connected (Arc for Send+Sync in Callback).
    // The on_change Callback just checks this flag; the actual WS send
    // is done in the editor_component dispatch, not in the debounced save.
    let ws_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // CollabClient lives in Rc (not Send) — only used from Effects and editor dispatch
    let collab_client: std::rc::Rc<std::cell::RefCell<Option<CollabClient>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    // Reactively load document when the ID changes
    Effect::new(move |_| {
        let id = doc_id();
        if id.is_empty() || id == current_id.get_untracked() {
            return;
        }
        set_current_id.set(id.clone());
        set_title.set("Loading...".to_string());
        set_error.set(None);
        set_content_loaded.set(false);

        leptos::task::spawn_local(async move {
            match documents::get_document(&id).await {
                Ok(doc) => set_title.set(doc.title),
                Err(e) => set_error.set(Some(e.to_string())),
            }

            match documents::get_content(&id).await {
                Ok(bytes) => {
                    set_initial_content.set(Some(bytes));
                    set_content_loaded.set(true);
                }
                Err(e) => {
                    set_initial_content.set(None);
                    set_content_loaded.set(true);
                    web_sys::console::warn_1(
                        &format!("Failed to load content: {e}").into(),
                    );
                }
            }
        });
    });

    // Connect WebSocket for real-time collaboration after content loads.
    let collab_for_ws = std::rc::Rc::clone(&collab_client);
    let ws_doc_id = current_id.clone();
    let ws_connected_for_ws = std::sync::Arc::clone(&ws_connected);
    Effect::new(move |_| {
        if !content_loaded.get() {
            return;
        }
        let id = ws_doc_id.get_untracked();
        if id.is_empty() {
            return;
        }

        // Disconnect existing client if document ID changed
        if let Some(old_client) = collab_for_ws.borrow_mut().take() {
            old_client.disconnect();
        }

        let initial_bytes = initial_content.get_untracked();
        let client = CollabClient::new(
            id.clone(),
            initial_bytes.as_deref(),
        );

        // Set up remote update callback.
        // Preserves the local cursor/selection position when remote changes arrive.
        let editor_state_for_ws = editor_state.clone();
        let set_editor_state_ws = set_editor_state.clone();
        client.set_on_remote_update(Box::new(move |doc| {
            // Preserve current selection from existing state
            let selection = editor_state_for_ws.get_untracked()
                .map(|s| s.selection.clone());
            let mut state = crate::editor::state::EditorState::create_default(doc);
            if let Some(sel) = selection {
                // Clamp selection to document bounds
                let max = state.doc.content_size();
                let from = sel.from().min(max);
                let to = sel.to().min(max);
                if from == to {
                    state.selection = crate::editor::selection::Selection::cursor(from);
                } else {
                    state.selection = crate::editor::selection::Selection::text(from, to);
                }
            }
            set_editor_state_ws.set(Some(state));
        }));

        let collab_ref = std::rc::Rc::clone(&collab_for_ws);
        *collab_ref.borrow_mut() = Some(client);

        // Request a ws-token and connect
        let collab_for_connect = std::rc::Rc::clone(&collab_for_ws);
        let ws_connected_for_connect = std::sync::Arc::clone(&ws_connected_for_ws);
        leptos::task::spawn_local(async move {
            match documents::request_ws_token(&id).await {
                Ok(resp) => {
                    // Build WebSocket URL from current page origin
                    let origin = web_sys::window()
                        .and_then(|w| w.location().origin().ok())
                        .unwrap_or_default();
                    let ws_origin = if origin.starts_with("https") {
                        origin.replacen("https", "wss", 1)
                    } else {
                        origin.replacen("http", "ws", 1)
                    };
                    let ws_url = format!(
                        "{ws_origin}/api/v1/documents/{id}/ws?token={}",
                        resp.token
                    );

                    if let Some(ref client) = *collab_for_connect.borrow() {
                        client.connect(&ws_url, &resp.token, std::sync::Arc::clone(&ws_connected_for_connect));
                        crate::editor::debug::log("collab", "WebSocket connecting", &[
                            ("doc_id", &id),
                        ]);
                    }
                }
                Err(e) => {
                    crate::editor::debug::warn("collab", &format!("ws-token request failed: {e}"));
                    // Silently fall back to REST — the on_change callback handles this
                }
            }
        });
    });

    // Send incremental yrs updates over WebSocket when connected.
    // This Effect watches the editor_state signal and sends updates via the CollabClient.
    let collab_for_send = std::rc::Rc::clone(&collab_client);
    let (prev_doc_hash, set_prev_doc_hash) = signal(0u64);
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else { return };
        // Simple change detection via text content hash
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            state.doc.text_content().hash(&mut hasher);
            hasher.finish()
        };
        if hash == prev_doc_hash.get_untracked() {
            return; // No change
        }
        set_prev_doc_hash.set(hash);

        if let Some(ref client) = *collab_for_send.borrow() {
            if client.is_synced() {
                client.send_update(&state.doc);
            }
        }
    });

    // Auto-save with REST fallback.
    // When WebSocket is connected, skip REST save (the editor_component
    // dispatch sends incremental updates via WS directly).
    // When disconnected, use debounced REST PUT as before.
    let save_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let save_doc_id = current_id.clone();
    let ws_connected_for_save = std::sync::Arc::clone(&ws_connected);
    let on_change = Callback::new(move |bytes: Vec<u8>| {
        // Skip REST save if WebSocket is handling persistence
        if ws_connected_for_save.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }

        let id = save_doc_id.get_untracked();
        if id.is_empty() {
            return;
        }
        let current_title = title.get_untracked();
        let generation = save_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let gen_ref = std::sync::Arc::clone(&save_generation);

        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;
            if gen_ref.load(std::sync::atomic::Ordering::Relaxed) != generation {
                return;
            }

            let mut attempts = 0;
            loop {
                attempts += 1;
                match documents::put_content(&id, &bytes).await {
                    Ok(()) => break,
                    Err(crate::api::client::ApiClientError::Http(409, _)) if attempts < 3 => {
                        gloo_timers::future::TimeoutFuture::new(100).await;
                        continue;
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Auto-save failed: {e}").into(),
                        );
                        break;
                    }
                }
            }

            if let Err(e) = documents::update_document_title(&id, &current_title).await {
                web_sys::console::error_1(
                    &format!("Title save failed: {e}").into(),
                );
            }
        });
    });

    let on_state_change = Callback::new(move |state: EditorState| {
        set_editor_state.set(Some(state.clone()));

        // Update title and URL slug from the first block's text content
        let first_text = state.doc.child(0).map(|n| n.text_content()).unwrap_or_default();
        let display_title = if first_text.trim().is_empty() {
            "Untitled".to_string()
        } else {
            first_text.clone()
        };
        set_title.set(display_title);

        let slug = slugify(&first_text);
        let id = current_id.get_untracked();
        if !id.is_empty() {
            if let Some(window) = web_sys::window() {
                let new_url = format!("/d/{id}/{slug}");
                let _ = window.history().and_then(|h| {
                    h.replace_state_with_url(
                        &wasm_bindgen::JsValue::NULL,
                        "",
                        Some(&new_url),
                    )
                });
            }
        }
    });

    // Outline navigation: scroll to heading position
    let on_outline_navigate = Callback::new(move |_pos: usize| {
        // TODO: scroll editor to position (requires EditorView access)
        // For now the outline shows headings; scrolling will be wired in Phase 2
    });

    // Toolbar dispatches commands via signal
    let on_command = Callback::new(move |cmd: ToolbarCommand| {
        set_toolbar_command.set(Some(cmd));
    });

    // Global keydown handler for outline toggle
    let on_page_keydown = move |ev: web_sys::KeyboardEvent| {
        let ctrl_or_meta = ev.ctrl_key() || ev.meta_key();
        if ctrl_or_meta && ev.shift_key() && ev.key().to_lowercase() == "o" {
            ev.prevent_default();
            set_outline_visible.set(!outline_visible.get_untracked());
        }
        if ctrl_or_meta && ev.alt_key() && ev.key().to_lowercase() == "c" {
            ev.prevent_default();
            set_conversation_visible.set(!conversation_visible.get_untracked());
        }
    };

    view! {
        <div class="app-layout" on:keydown=on_page_keydown>
            <Sidebar />
            <div class="main-content">
                {move || error.get().map(|e| view! {
                    <div style="color: var(--color-error); padding: var(--space-md);">
                        {e}
                    </div>
                })}

                <div class="doc-header">
                    <div class="doc-title">{title}</div>
                    <div class="doc-header-actions">
                        <NotificationBell />
                        <button
                            class="share-button"
                            on:click=move |_| set_share_visible.set(true)
                        >"Share"</button>
                    </div>
                </div>

                <Toolbar
                    editor_state=editor_state
                    on_command=on_command
                />

                {move || {
                    if content_loaded.get() {
                        let content = initial_content.get();
                        view! {
                            <EditorComponent props=EditorProps {
                                initial_content: content,
                                on_change: on_change.clone(),
                                on_state_change: on_state_change.clone(),
                                command_signal: toolbar_command,
                                doc_id: current_id.get_untracked(),
                            } />
                        }.into_any()
                    } else {
                        view! {
                            <div class="editor-container">
                                <div class="editor-content" style="color: var(--color-text-secondary);">
                                    "Loading document..."
                                </div>
                            </div>
                        }.into_any()
                    }
                }}

                <DocumentOutline
                    editor_state=editor_state
                    visible=outline_visible
                    on_navigate=on_outline_navigate
                />

                <ConversationPane
                    visible=conversation_visible
                    doc_id=current_id
                />
            </div>

            <ShareDialog
                visible=share_visible
                on_close=Callback::new(move |_| set_share_visible.set(false))
                resource_id=current_id.get_untracked()
            />
        </div>
    }
    .into_any()
}

/// Convert text to a URL-safe slug.
fn slugify(text: &str) -> String {
    let slug: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive hyphens and trim
    let mut result = String::new();
    for c in slug.chars() {
        if c == '-' && result.ends_with('-') {
            continue;
        }
        result.push(c);
    }
    let result = result.trim_matches('-').to_string();
    if result.is_empty() {
        "untitled".to_string()
    } else {
        result
    }
}
