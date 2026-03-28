use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::api::client;
use crate::api::documents;
use crate::api::folders::{self, FolderResponse};
use crate::components::file_browser::FileBrowser;
use crate::components::sidebar::Sidebar;

#[component]
pub fn HomePage() -> impl IntoView {
    let (folder, set_folder) = signal::<Option<FolderResponse>>(None);
    let (error, set_error) = signal::<Option<String>>(None);
    let (home_folder_id, set_home_folder_id) = signal::<Option<String>>(None);

    // Redirect to login if not authenticated
    if !client::is_authenticated() {
        let navigate = use_navigate();
        navigate("/login", Default::default());
        return view! { <div>"Redirecting to login..."</div> }.into_any();
    }

    // Load user info and home folder on mount
    {
        let set_folder = set_folder.clone();
        let set_error = set_error.clone();
        let set_home_folder_id = set_home_folder_id.clone();
        leptos::task::spawn_local(async move {
            // Get the user's home folder ID
            match client::api_get::<UserMeResponse>("/users/me").await {
                Ok(user) => {
                    set_home_folder_id.set(Some(user.home_folder_id.clone()));
                    // Load home folder contents
                    match folders::get_folder(&user.home_folder_id).await {
                        Ok(f) => set_folder.set(Some(f)),
                        Err(e) => set_error.set(Some(e.to_string())),
                    }
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    }

    let refresh_folder = move || {
        let set_folder = set_folder.clone();
        let set_error = set_error.clone();
        if let Some(id) = home_folder_id.get() {
            leptos::task::spawn_local(async move {
                match folders::get_folder(&id).await {
                    Ok(f) => set_folder.set(Some(f)),
                    Err(e) => set_error.set(Some(e.to_string())),
                }
            });
        }
    };

    let on_create_document = {
        move |_| {
            leptos::task::spawn_local(async move {
                match documents::create_document("Untitled", None).await {
                    Ok(doc) => {
                        if let Some(window) = web_sys::window() {
                            let _ = window
                                .location()
                                .set_href(&format!("/d/{}/untitled", doc.id));
                        }
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to create document: {e}").into(),
                        );
                    }
                }
            });
        }
    };

    let on_create_folder = {
        let refresh = refresh_folder.clone();
        move |_| {
            let refresh = refresh.clone();
            let set_error = set_error.clone();
            leptos::task::spawn_local(async move {
                match folders::create_folder("New Folder", None).await {
                    Ok(_) => refresh(),
                    Err(e) => set_error.set(Some(e.to_string())),
                }
            });
        }
    };

    let on_navigate_folder = Callback::new(move |folder_id: String| {
        let set_folder = set_folder.clone();
        let set_error = set_error.clone();
        leptos::task::spawn_local(async move {
            match folders::get_folder(&folder_id).await {
                Ok(f) => set_folder.set(Some(f)),
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    });

    let on_open_document = Callback::new(move |doc_id: String| {
        if let Some(window) = web_sys::window() {
            let _ = window.location().set_href(&format!("/d/{doc_id}/doc"));
        }
    });

    view! {
        <div class="app-layout">
            <Sidebar />
            <div class="main-content">
                <div class="file-browser">
                    <div class="action-bar">
                        <button class="btn btn-primary" on:click=on_create_document>
                            "+ New Document"
                        </button>
                        <button class="btn btn-secondary" on:click=on_create_folder>
                            "+ New Folder"
                        </button>
                    </div>

                    {move || error.get().map(|e| view! {
                        <div style="color: var(--color-error); margin-bottom: var(--space-md);">
                            {e}
                        </div>
                    })}

                    <FileBrowser
                        folder=folder
                        on_navigate_folder=on_navigate_folder
                        on_open_document=on_open_document
                    />
                </div>
            </div>
        </div>
    }
    .into_any()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserMeResponse {
    home_folder_id: String,
}
