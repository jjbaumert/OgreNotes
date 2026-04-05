use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;

use crate::api::client;
use crate::api::documents;
use crate::collab::ws_client::CollabClient;
use crate::collab::ws_client::RemoteCursor;
use crate::components::at_menu::{AtMenu, AtMenuItem, AtMenuItemType};
use crate::components::block_menu::BlockMenu;
use crate::components::comment_highlights::{CommentHighlights, InlineThreadInfo};
use crate::components::comment_popup::CommentPopup;
use crate::components::conversation_pane::ConversationPane;
use crate::components::selection_toolbar::{SelectionToolbar, SelectionCommand};
use crate::components::cursor_overlay::CursorOverlay;
use crate::components::history_viewer::HistoryViewer;
use crate::components::menu_bar::{DocAction, MenuBar};
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
    // Remote document state — set by the collab callback, consumed by EditorComponent
    // to update the contenteditable DOM when a collaborator makes changes.
    let (remote_state, set_remote_state) = signal::<Option<EditorState>>(None);
    let (outline_visible, set_outline_visible) = signal(false);
    let (share_visible, set_share_visible) = signal(false);
    let (folder_id, set_folder_id) = signal::<Option<String>>(None);
    let (conversation_visible, set_conversation_visible) = signal(false);
    // Inline comment state
    let (pending_block_id, set_pending_block_id) = signal::<Option<String>>(None);
    let (pending_anchor_start, set_pending_anchor_start) = signal::<Option<u32>>(None);
    let (pending_anchor_end, set_pending_anchor_end) = signal::<Option<u32>>(None);
    let (filter_thread_id, set_filter_thread_id) = signal::<Option<String>>(None);
    let (inline_threads, set_inline_threads) = signal::<Vec<InlineThreadInfo>>(Vec::new());
    let (comment_count, set_comment_count) = signal(0usize);
    // Comment popup state (for inline comment threads shown near highlighted text)
    let (popup_thread_id, set_popup_thread_id) = signal::<Option<String>>(None);
    let (popup_left, set_popup_left) = signal(0.0f64);
    let (popup_top, set_popup_top) = signal(0.0f64);
    let (popup_is_new, set_popup_is_new) = signal(false);
    let (popup_block_id, set_popup_block_id) = signal::<Option<String>>(None);
    let (popup_anchor_start, set_popup_anchor_start) = signal::<Option<u32>>(None);
    let (popup_anchor_end, set_popup_anchor_end) = signal::<Option<u32>>(None);
    // Block menu state
    let (block_menu_visible, set_block_menu_visible) = signal(false);
    let (block_menu_top, set_block_menu_top) = signal(0.0f64);
    // Remote cursor presence
    let (remote_cursors, set_remote_cursors) = signal::<Vec<RemoteCursor>>(Vec::new());
    // Scroll tick — incremented on editor-container scroll to force overlay re-render
    let (scroll_tick, set_scroll_tick) = signal(0u32);
    // History viewer
    let (history_visible, set_history_visible) = signal(false);
    let (current_doc_text, set_current_doc_text) = signal(String::new());
    // At menu state
    let (at_menu_visible, set_at_menu_visible) = signal(false);
    let (at_menu_query, set_at_menu_query) = signal(String::new());
    let (at_menu_left, set_at_menu_left) = signal(0.0f64);
    let (at_menu_top, set_at_menu_top) = signal(0.0f64);
    // Track whether WS is connected (Arc for Send+Sync in Callback).
    // The on_change Callback just checks this flag; the actual WS send
    // is done in the editor_component dispatch, not in the debounced save.
    let ws_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // CollabClient lives in Rc (not Send) — only used from Effects and editor dispatch
    let collab_client: std::rc::Rc<std::cell::RefCell<Option<CollabClient>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    // Track which doc_id the current CollabClient is for, so we can reuse it on reconnect.
    let collab_doc_id: std::rc::Rc<std::cell::RefCell<String>> =
        std::rc::Rc::new(std::cell::RefCell::new(String::new()));

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
                Ok(doc) => {
                    set_title.set(doc.title);
                    set_folder_id.set(doc.folder_id);
                }
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

    // Flag to suppress the send_update Effect when state changes come from remote updates.
    // Prevents feedback loops (remote → set_editor_state → send_update → echo back).
    let remote_update_flag = std::rc::Rc::new(std::cell::Cell::new(false));

    // Connect WebSocket for real-time collaboration after content loads.
    let collab_for_ws = std::rc::Rc::clone(&collab_client);
    let collab_doc_id_for_ws = std::rc::Rc::clone(&collab_doc_id);
    let ws_doc_id = current_id.clone();
    let ws_connected_for_ws = std::sync::Arc::clone(&ws_connected);
    let remote_flag_for_ws = std::rc::Rc::clone(&remote_update_flag);
    Effect::new(move |_| {
        if !content_loaded.get() {
            return;
        }
        let id = ws_doc_id.get_untracked();
        if id.is_empty() {
            return;
        }

        let is_same_doc = *collab_doc_id_for_ws.borrow() == id;

        if is_same_doc {
            // Same document — reuse the existing CollabClient and its persistent
            // yrs::Doc (preserves client_id and CRDT clock). Just disconnect the
            // old WebSocket; the reconnect code below will open a fresh one.
            if let Some(ref client) = *collab_for_ws.borrow() {
                client.disconnect();
            }
        } else {
            // Different document — drop the old client entirely and create a new one.
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
            // Sets `remote_state` which the EditorComponent watches to update the DOM,
            // and also sets `remote_flag` so the send Effect skips this change.
            let editor_state_for_ws = editor_state.clone();
            let set_remote_state_ws = set_remote_state.clone();
            let remote_flag_for_ws = remote_flag_for_ws.clone();
            client.set_on_remote_update(Box::new(move |doc| {
                let selection = editor_state_for_ws.get_untracked()
                    .map(|s| s.selection.clone());
                let mut state = crate::editor::state::EditorState::create_default(doc);
                if let Some(sel) = selection {
                    let max = state.doc.content_size();
                    let from = sel.from().min(max);
                    let to = sel.to().min(max);
                    if from == to {
                        state.selection = crate::editor::selection::Selection::cursor(from);
                    } else {
                        state.selection = crate::editor::selection::Selection::text(from, to);
                    }
                }
                remote_flag_for_ws.set(true);
                set_remote_state_ws.set(Some(state));
            }));

            // Set up awareness callback for remote cursor presence.
            client.set_on_awareness_update(Box::new(move |cursors| {
                set_remote_cursors.set(cursors);
            }));

            *collab_for_ws.borrow_mut() = Some(client);
            *collab_doc_id_for_ws.borrow_mut() = id.clone();
        }

        // Request a ws-token and connect (shared by both same-doc reconnect and new-doc).
        let collab_for_connect = std::rc::Rc::clone(&collab_for_ws);
        let ws_connected_for_connect = std::sync::Arc::clone(&ws_connected_for_ws);
        leptos::task::spawn_local(async move {
            match documents::request_ws_token(&id).await {
                Ok(resp) => {
                    let origin = web_sys::window()
                        .and_then(|w| w.location().origin().ok())
                        .unwrap_or_default();
                    let ws_origin = if origin.starts_with("https") {
                        origin.replacen("https", "wss", 1)
                    } else {
                        let api_origin = origin.replacen("http", "ws", 1);
                        if api_origin.contains(":8080") {
                            api_origin.replace(":8080", ":3000")
                        } else {
                            api_origin
                        }
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
                }
            }
        });
    });

    // Send incremental yrs updates over WebSocket when connected.
    // Debounced: rapid keystrokes are batched into fewer WS sends.
    // Uses gloo_timers::callback::Timeout instead of spawn_local + TimeoutFuture
    // to avoid re-entrant polling of the wasm-bindgen task runner's RefCell
    // (Leptos Effects use queueMicrotask which shares the same microtask queue).
    let collab_for_send = std::rc::Rc::clone(&collab_client);
    let (prev_doc_hash, set_prev_doc_hash) = signal(0u64);
    let pending_send_timer: std::rc::Rc<std::cell::RefCell<Option<gloo_timers::callback::Timeout>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let remote_flag_for_send = std::rc::Rc::clone(&remote_update_flag);
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else { return };

        // Skip remote-originated state changes to prevent feedback loops.
        if remote_flag_for_send.get() {
            remote_flag_for_send.set(false);
            // Still update the hash so the next local change is detected correctly.
            set_prev_doc_hash.set(state.doc.structural_hash());
            return;
        }

        let hash = state.doc.structural_hash();
        if hash == prev_doc_hash.get_untracked() {
            return;
        }
        set_prev_doc_hash.set(hash);

        // Debounce: set a timer. If another change arrives before the timeout,
        // the previous timer is dropped (cancelled) and a new one is set.
        let collab = collab_for_send.clone();
        let doc = state.doc.clone();
        let timer_ref = pending_send_timer.clone();
        *timer_ref.borrow_mut() = Some(gloo_timers::callback::Timeout::new(
            crate::collab::ws_client::WS_SEND_DEBOUNCE_MS,
            move || {
                if let Some(ref client) = *collab.borrow() {
                    if client.is_synced() {
                        client.send_update(&doc);
                    }
                }
            },
        ));
    });

    // Send local cursor/selection position as awareness updates.
    let collab_for_awareness = std::rc::Rc::clone(&collab_client);
    let (prev_sel_hash, set_prev_sel_hash) = signal(0u64);
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else { return };
        // Quick change detection on selection
        let sel_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            state.selection.from().hash(&mut h);
            state.selection.to().hash(&mut h);
            h.finish()
        };
        if sel_hash == prev_sel_hash.get_untracked() {
            return;
        }
        set_prev_sel_hash.set(sel_hash);

        if let Some(ref client) = *collab_for_awareness.borrow() {
            if client.is_synced() {
                let auth = crate::api::client::get_auth();
                let user_id = auth.as_ref().map(|a| a.user_id.as_str()).unwrap_or("unknown");
                let name = auth.as_ref().map(|a| a.name.as_str()).unwrap_or("Anonymous");
                let color_idx = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    user_id.hash(&mut h);
                    (h.finish() % 12) as u8
                };
                let from = state.selection.from() as u32;
                let to = state.selection.to() as u32;
                if from == to {
                    client.send_awareness(user_id, name, color_idx, Some(from), None, None);
                } else {
                    client.send_awareness(user_id, name, color_idx, Some(to), Some(from), Some(to));
                }
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
    // Track title changes separately so we can save the title even when WS handles content.
    let title_save_doc_id = save_doc_id.clone();
    let (prev_title, set_prev_title) = signal(String::new());
    let pending_title_timer: std::rc::Rc<std::cell::RefCell<Option<gloo_timers::callback::Timeout>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    Effect::new(move |_| {
        let current_title = title.get();
        if current_title == prev_title.get_untracked() || current_title == "Loading..." {
            return;
        }
        set_prev_title.set(current_title.clone());
        let id = title_save_doc_id.get_untracked();
        if id.is_empty() {
            return;
        }
        let timer_ref = pending_title_timer.clone();
        *timer_ref.borrow_mut() = Some(gloo_timers::callback::Timeout::new(1000, move || {
            leptos::task::spawn_local(async move {
                if let Err(e) = documents::update_document_title(&id, &current_title).await {
                    web_sys::console::error_1(&format!("Title save failed: {e}").into());
                }
            });
        }));
    });

    let on_change = Callback::new(move |bytes: Vec<u8>| {
        // Skip REST content save if WebSocket is handling persistence.
        // Title is saved separately via the Effect above.
        if ws_connected_for_save.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }

        let id = save_doc_id.get_untracked();
        if id.is_empty() {
            return;
        }
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
        });
    });

    let on_state_change = Callback::new(move |state: EditorState| {
        set_editor_state.set(Some(state.clone()));
        set_current_doc_text.set(state.doc.text_content());

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

    // Outline navigation: scroll to heading position.
    // Finds the nth heading element in the editor DOM and scrolls it into view.
    let on_outline_navigate = Callback::new(move |pos: usize| {
        if let Some(state) = editor_state.get_untracked() {
            let entries = crate::components::document_outline::extract_outline(&state.doc);
            if let Some(idx) = entries.iter().position(|e| e.position == pos) {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    let selector = ".editor-content h1, .editor-content h2, .editor-content h3";
                    if let Ok(nodes) = doc.query_selector_all(selector) {
                        if let Some(node) = nodes.item(idx as u32) {
                            if let Ok(el) = node.dyn_into::<web_sys::Element>() {
                                el.scroll_into_view_with_bool(true);
                            }
                        }
                    }
                }
            }
        }
    });

    // Toolbar dispatches commands via signal.
    // InsertComment is handled here (opens comment pane), everything else goes to editor.
    let on_command = Callback::new(move |cmd: ToolbarCommand| {
        if matches!(cmd, ToolbarCommand::InsertComment) {
            open_comment_pane(
                &editor_state,
                &set_pending_block_id,
                &set_pending_anchor_start,
                &set_pending_anchor_end,
                &set_conversation_visible,
                &conversation_visible,
            );
            return;
        }
        set_toolbar_command.set(Some(cmd));
    });

    // Detect @ mention trigger: watch editor state for `@` followed by query text at cursor.
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else {
            return;
        };
        let pos = state.selection.from();
        if !state.selection.empty() {
            if at_menu_visible.get_untracked() {
                set_at_menu_visible.set(false);
            }
            return;
        }

        // Extract text before cursor in the current text node.
        let text_before = state.doc.text_before(pos).unwrap_or_default();

        // Find the last `@` in text_before.
        if let Some(at_idx) = text_before.rfind('@') {
            let query = &text_before[at_idx + 1..];
            // Only trigger if @ is at word boundary (start of text or preceded by whitespace).
            let before_at = if at_idx > 0 {
                text_before.as_bytes().get(at_idx - 1).copied()
            } else {
                Some(b' ') // start of text counts as boundary
            };
            if before_at.map_or(true, |c| c == b' ' || c == b'\n') && !query.contains(' ') {
                set_at_menu_query.set(query.to_string());
                if !at_menu_visible.get_untracked() {
                    // Position the menu at the cursor using the browser selection.
                    if let Some(window) = web_sys::window() {
                        if let Some(selection) = window.get_selection().ok().flatten() {
                            if selection.range_count() > 0 {
                                if let Ok(range) = selection.get_range_at(0) {
                                    let rect = range.get_bounding_client_rect();
                                    set_at_menu_left.set(rect.left());
                                    set_at_menu_top.set(rect.bottom() + 4.0);
                                }
                            }
                        }
                    }
                    set_at_menu_visible.set(true);
                }
                return;
            }
        }

        if at_menu_visible.get_untracked() {
            set_at_menu_visible.set(false);
        }
    });

    // Block menu command handler (reuses the same command dispatch).
    let on_block_command = Callback::new(move |cmd: ToolbarCommand| {
        set_toolbar_command.set(Some(cmd));
        set_block_menu_visible.set(false);
    });

    // AtMenu: select callback inserts a mention.
    let on_at_select = Callback::new(move |item: AtMenuItem| {
        set_at_menu_visible.set(false);
        // TODO: Insert mention node or link at cursor position
        web_sys::console::log_1(
            &format!("Selected @mention: {} ({})", item.label, item.id).into(),
        );
    });

    let on_at_close = Callback::new(move |_: ()| {
        set_at_menu_visible.set(false);
    });

    // Attach scroll listener to .editor-container so fixed-position overlays
    // (comment highlights, selection toolbar, cursor overlay) re-render on scroll.
    // Uses requestAnimationFrame to throttle to ~60fps.
    {
        let scroll_raf_pending = std::rc::Rc::new(std::cell::Cell::new(false));
        Effect::new(move |_| {
            if !content_loaded.get() {
                return;
            }
            let Some(window) = web_sys::window() else { return };
            let Some(document) = window.document() else { return };
            let Some(container) = document.query_selector(".editor-container").ok().flatten() else {
                return;
            };

            let raf_pending = scroll_raf_pending.clone();
            let on_scroll = Closure::wrap(Box::new(move |_: web_sys::Event| {
                if raf_pending.get() {
                    return;
                }
                raf_pending.set(true);
                let raf_pending_inner = raf_pending.clone();
                let cb = Closure::once_into_js(move || {
                    raf_pending_inner.set(false);
                    set_scroll_tick.set(scroll_tick.get_untracked().wrapping_add(1));
                });
                let _ = web_sys::window().map(|w| w.request_animation_frame(cb.as_ref().unchecked_ref()));
            }) as Box<dyn Fn(web_sys::Event)>);

            let _ = container.add_event_listener_with_callback("scroll", on_scroll.as_ref().unchecked_ref());
            on_scroll.forget(); // leak — lives as long as the DOM element
        });
    }

    // Mousemove on editor area: detect block hover for BlockMenu.
    let on_editor_mousemove = move |ev: web_sys::MouseEvent| {
        let x = ev.client_x() as f64;
        let target = ev.target();

        // Only show block menu when hovering in the left gutter area (< 40px from editor left)
        if let Some(target) = target.and_then(|t| t.dyn_ref::<web_sys::Element>().cloned()) {
            // Walk up to find a block-level element inside .editor-content
            let mut el = Some(target);
            while let Some(ref current) = el {
                let tag = current.tag_name().to_lowercase();
                if matches!(tag.as_str(), "p" | "h1" | "h2" | "h3" | "blockquote" | "hr") {
                    // Check if this element is inside .editor-content
                    if let Some(parent) = current.closest(".editor-content").ok().flatten() {
                        let rect = current.get_bounding_client_rect();
                        let editor_rect = parent.get_bounding_client_rect();
                        // Show menu when hovering within 40px of the left edge
                        if x < editor_rect.left() + 40.0 {
                            set_block_menu_top.set(rect.top());
                            set_block_menu_visible.set(true);
                            return;
                        }
                    }
                    break;
                }
                el = current.parent_element();
            }
        }
        set_block_menu_visible.set(false);
    };

    // Global keydown handler for outline toggle
    let on_page_keydown = move |ev: web_sys::KeyboardEvent| {
        let ctrl_or_meta = ev.ctrl_key() || ev.meta_key();
        if ctrl_or_meta && ev.shift_key() && ev.key().to_lowercase() == "o" {
            ev.prevent_default();
            set_outline_visible.set(!outline_visible.get_untracked());
        }
        if ctrl_or_meta && ev.alt_key() && ev.key().to_lowercase() == "c" {
            ev.prevent_default();
            open_comment_pane(
                &editor_state,
                &set_pending_block_id,
                &set_pending_anchor_start,
                &set_pending_anchor_end,
                &set_conversation_visible,
                &conversation_visible,
            );
        }
        if ctrl_or_meta && ev.shift_key() && ev.key().to_lowercase() == "h" {
            ev.prevent_default();
            set_history_visible.set(!history_visible.get_untracked());
        }
    };

    // Handle document-level actions from the menu bar.
    // Capture navigate at definition time (inside Router context).
    let navigate_for_action = leptos_router::hooks::use_navigate();
    let on_doc_action = Callback::new(move |action: DocAction| {
        match action {
            DocAction::NewDocument => {
                let navigate = navigate_for_action.clone();
                leptos::task::spawn_local(async move {
                    match documents::create_document("Untitled", None).await {
                        Ok(doc) => { navigate(&format!("/d/{}", doc.id), Default::default()); }
                        Err(e) => { web_sys::console::error_1(&format!("New doc failed: {e}").into()); }
                    }
                });
            }
            DocAction::Share => {
                set_share_visible.set(true);
            }
            DocAction::CopyLink => {
                if let Some(window) = web_sys::window() {
                    if let Ok(href) = window.location().href() {
                        // Use wasm_bindgen to call clipboard.writeText safely (no eval/Function).
                        let promise = js_sys::Reflect::get(
                            &window.navigator(),
                            &"clipboard".into(),
                        )
                        .and_then(|clip| {
                            js_sys::Reflect::get(&clip, &"writeText".into())
                        })
                        .and_then(|func| {
                            func.dyn_into::<js_sys::Function>()
                        });
                        if let Ok(write_text) = promise {
                            let clip = js_sys::Reflect::get(
                                &window.navigator(),
                                &"clipboard".into(),
                            ).unwrap_or(wasm_bindgen::JsValue::NULL);
                            let _ = write_text.call1(&clip, &href.into());
                        }
                    }
                }
            }
            DocAction::ExportHtml => {
                let id = current_id.get_untracked();
                if !id.is_empty() {
                    if let Some(window) = web_sys::window() {
                        let _ = window.open_with_url_and_target(
                            &format!("/api/v1/documents/{id}/export/html"),
                            "_blank",
                        );
                    }
                }
            }
            DocAction::ExportMarkdown => {
                let id = current_id.get_untracked();
                if !id.is_empty() {
                    if let Some(window) = web_sys::window() {
                        let _ = window.open_with_url_and_target(
                            &format!("/api/v1/documents/{id}/export/markdown"),
                            "_blank",
                        );
                    }
                }
            }
            DocAction::Print => {
                if let Some(window) = web_sys::window() {
                    let _ = window.print();
                }
            }
            DocAction::DocumentHistory => {
                set_history_visible.set(!history_visible.get_untracked());
            }
            DocAction::DeleteDocument => {
                if let Some(window) = web_sys::window() {
                    if window.confirm_with_message("Delete this document?").unwrap_or(false) {
                        let id = current_id.get_untracked();
                        let navigate = leptos_router::hooks::use_navigate();
                        leptos::task::spawn_local(async move {
                            if let Err(e) = documents::delete_document(&id).await {
                                web_sys::console::error_1(&format!("Delete failed: {e}").into());
                            } else {
                                navigate("/", Default::default());
                            }
                        });
                    }
                }
            }
            DocAction::ToggleConversation => {
                set_conversation_visible.set(!conversation_visible.get_untracked());
            }
            DocAction::ToggleOutline => {
                set_outline_visible.set(!outline_visible.get_untracked());
            }
        }
    });

    view! {
        <div class="app-layout" on:keydown=on_page_keydown on:mousemove=on_editor_mousemove>
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

                <MenuBar
                    on_command=on_command
                    on_doc_action=on_doc_action
                    conversation_visible=conversation_visible
                    outline_visible=outline_visible
                />

                <Toolbar
                    editor_state=editor_state
                    on_command=on_command
                    comment_count=Signal::derive(move || comment_count.get())
                />

                // Editor + side panels in a row
                <div class="editor-with-panels">
                {move || {
                    if content_loaded.get() {
                        let content = initial_content.get();
                        view! {
                            <EditorComponent props=EditorProps {
                                initial_content: content,
                                on_change: on_change.clone(),
                                on_state_change: on_state_change.clone(),
                                command_signal: toolbar_command,
                                remote_state: remote_state,
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
                    editor_state=editor_state
                    pending_block_id=pending_block_id
                    pending_anchor_start=pending_anchor_start
                    pending_anchor_end=pending_anchor_end
                    on_block_used=Callback::new(move |_| {
                        set_pending_block_id.set(None);
                        set_pending_anchor_start.set(None);
                        set_pending_anchor_end.set(None);
                    })
                    on_threads_loaded=Callback::new(move |threads: Vec<InlineThreadInfo>| {
                        set_comment_count.set(threads.len());
                        set_inline_threads.set(threads);
                    })
                    filter_thread_id=filter_thread_id
                />
                </div> // editor-with-panels
            </div>

            <ShareDialog
                visible=share_visible
                on_close=Callback::new(move |_| set_share_visible.set(false))
                folder_id=folder_id
            />

            <CursorOverlay cursors=remote_cursors scroll_tick=scroll_tick />

            <CommentHighlights
                threads=inline_threads
                editor_state=editor_state
                scroll_tick=scroll_tick
                on_click=Callback::new(move |(thread_id, left, top): (String, f64, f64)| {
                    set_popup_thread_id.set(Some(thread_id));
                    set_popup_left.set(left);
                    set_popup_top.set(top);
                })
            />

            <CommentPopup
                thread_id=popup_thread_id
                left=popup_left
                top=popup_top
                doc_id=current_id
                block_id=popup_block_id
                anchor_start=popup_anchor_start
                anchor_end=popup_anchor_end
                is_new=popup_is_new
                on_close=Callback::new(move |_| {
                    set_popup_thread_id.set(None);
                    set_popup_is_new.set(false);
                    set_popup_block_id.set(None);
                })
                on_thread_created=Callback::new(move |_tid: String| {
                    // Refresh thread list in conversation pane
                })
            />

            <SelectionToolbar
                editor_state=editor_state
                scroll_tick=scroll_tick
                on_command=Callback::new(move |cmd: SelectionCommand| {
                    match cmd {
                        SelectionCommand::Comment => {
                            // Open the popup for a new comment at the selection
                            if let Some(state) = editor_state.get_untracked() {
                                let from = state.selection.from();
                                let to = state.selection.to();
                                if let Some(bid) = state.doc.block_id_at(from) {
                                    let block = crate::editor::state::find_block_at(&state.doc, from);
                                    let (a_start, a_end) = if from != to {
                                        if let Some(b) = &block {
                                            let rel_from = (from.saturating_sub(b.content_start)) as u32;
                                            let rel_to = (to.saturating_sub(b.content_start)).min(b.content.size()) as u32;
                                            if rel_from < rel_to { (Some(rel_from), Some(rel_to)) }
                                            else { (None, None) }
                                        } else { (None, None) }
                                    } else { (None, None) };

                                    // Get position from the browser selection
                                    if let Some(window) = web_sys::window() {
                                        if let Some(sel) = window.get_selection().ok().flatten() {
                                            if let Ok(range) = sel.get_range_at(0) {
                                                let rect = range.get_bounding_client_rect();
                                                let popup_width = 420.0;
                                                set_popup_left.set((rect.left() - popup_width - 12.0).max(4.0));
                                                set_popup_top.set(rect.top());
                                            }
                                        }
                                    }

                                    set_popup_block_id.set(Some(bid));
                                    set_popup_anchor_start.set(a_start);
                                    set_popup_anchor_end.set(a_end);
                                    set_popup_is_new.set(true);
                                    set_popup_thread_id.set(None);
                                }
                            }
                        }
                    }
                })
            />

            <HistoryViewer
                visible=history_visible
                doc_id=current_id
                current_text=current_doc_text
            />

            <BlockMenu
                visible=block_menu_visible
                on_command=on_block_command
                top=block_menu_top
            />

            <AtMenu
                visible=at_menu_visible
                query=at_menu_query
                left=at_menu_left
                top=at_menu_top
                on_select=on_at_select
                on_close=on_at_close
            />
        </div>
    }
    .into_any()
}

/// Open the comment pane. Finds the block ID at the cursor position
/// and sets it as pending for inline comment creation.
fn open_comment_pane(
    editor_state: &ReadSignal<Option<EditorState>>,
    set_pending_block_id: &WriteSignal<Option<String>>,
    set_pending_anchor_start: &WriteSignal<Option<u32>>,
    set_pending_anchor_end: &WriteSignal<Option<u32>>,
    set_conversation_visible: &WriteSignal<bool>,
    conversation_visible: &ReadSignal<bool>,
) {
    if let Some(state) = editor_state.get_untracked() {
        let from = state.selection.from();
        let to = state.selection.to();
        if let Some(block_id) = state.doc.block_id_at(from) {
            // Compute selection offsets relative to the block's content start
            let block = crate::editor::state::find_block_at(&state.doc, from);
            let (anchor_start, anchor_end) = if from != to {
                if let Some(b) = &block {
                    let rel_from = (from.saturating_sub(b.content_start)) as u32;
                    let rel_to = (to.saturating_sub(b.content_start)).min(b.content.size()) as u32;
                    if rel_from < rel_to {
                        (Some(rel_from), Some(rel_to))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                }
            } else {
                (None, None) // cursor, no text selection
            };
            set_pending_block_id.set(Some(block_id));
            set_pending_anchor_start.set(anchor_start);
            set_pending_anchor_end.set(anchor_end);
            set_conversation_visible.set(true);
            return;
        }
    }
    set_pending_block_id.set(None);
    set_pending_anchor_start.set(None);
    set_pending_anchor_end.set(None);
    set_conversation_visible.set(!conversation_visible.get_untracked());
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
