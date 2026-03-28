use leptos::prelude::*;

use crate::api::folders::FolderResponse;

#[component]
pub fn FileBrowser(
    folder: ReadSignal<Option<FolderResponse>>,
    on_navigate_folder: Callback<String>,
    on_open_document: Callback<String>,
) -> impl IntoView {
    view! {
        <div class="file-browser">
            {move || match folder.get() {
                None => view! {
                    <div class="empty-state">
                        <p class="empty-state-text">"Loading..."</p>
                    </div>
                }.into_any(),
                Some(f) => {
                    let children = f.children.clone();
                    let title = f.title.clone();
                    if children.is_empty() {
                        view! {
                            <div class="breadcrumbs">
                                <span class="breadcrumb-link">{title}</span>
                            </div>
                            <div class="empty-state">
                                <p class="empty-state-text">
                                    "Nothing here yet. Create a document or folder."
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        view! {
                            <div class="breadcrumbs">
                                <span class="breadcrumb-link">{title}</span>
                            </div>
                            <table class="file-list">
                                <thead>
                                    <tr>
                                        <th>"Title"</th>
                                        <th>"Type"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {children.into_iter().map(|child| {
                                        let child_id = child.child_id.clone();
                                        let child_title = child.title.clone();
                                        let child_type = child.child_type.clone();
                                        let on_nav = on_navigate_folder.clone();
                                        let on_open = on_open_document.clone();
                                        let id_for_click = child_id.clone();
                                        let is_folder = child_type == "folder";
                                        let icon = if is_folder { "\u{1F4C1}" } else { "\u{1F4C4}" };
                                        let type_label = if is_folder { "Folder" } else { "Document" };

                                        view! {
                                            <tr>
                                                <td>
                                                    <span
                                                        class="file-name"
                                                        on:click=move |_| {
                                                            if is_folder {
                                                                on_nav.run(id_for_click.clone());
                                                            } else {
                                                                on_open.run(id_for_click.clone());
                                                            }
                                                        }
                                                    >
                                                        <span class="file-icon">{icon}</span>
                                                        {child_title}
                                                    </span>
                                                </td>
                                                <td class="file-date">{type_label}</td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        }.into_any()
                    }
                }
            }}
        </div>
    }
}
