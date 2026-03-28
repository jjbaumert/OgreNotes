use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};

use crate::api::client;
use crate::api::documents;
use crate::components::editor_component::{EditorComponent, EditorProps};
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

    // Auto-save with a 500ms delay to batch rapid changes
    let save_doc_id = current_id.clone();
    let on_change = Callback::new(move |bytes: Vec<u8>| {
        let id = save_doc_id.get_untracked();
        if id.is_empty() {
            return;
        }
        let current_title = title.get_untracked();

        leptos::task::spawn_local(async move {
            // Simple delay to batch rapid keystrokes
            gloo_timers::future::TimeoutFuture::new(500).await;

            if let Err(e) = documents::put_content(&id, &bytes).await {
                web_sys::console::error_1(
                    &format!("Auto-save failed: {e}").into(),
                );
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

    // Toolbar dispatches commands via signal
    let on_command = Callback::new(move |cmd: ToolbarCommand| {
        set_toolbar_command.set(Some(cmd));
    });

    view! {
        <div class="app-layout">
            <Sidebar />
            <div class="main-content">
                {move || error.get().map(|e| view! {
                    <div style="color: var(--color-error); padding: var(--space-md);">
                        {e}
                    </div>
                })}

                <div class="doc-header">
                    <div class="doc-title">{title}</div>
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
            </div>
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
