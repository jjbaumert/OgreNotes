// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use std::collections::HashSet;

use leptos::prelude::*;

use crate::api::folders::{ChildResponse, FolderResponse};
use crate::i18n::{format_date, DateStyle};

/// Sort field for the file browser.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SortField {
    Title,
    Date,
}

/// Sort order.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    fn toggle(self) -> Self {
        match self {
            SortOrder::Asc => SortOrder::Desc,
            SortOrder::Desc => SortOrder::Asc,
        }
    }

    fn indicator(self) -> &'static str {
        match self {
            SortOrder::Asc => " \u{25B2}",
            SortOrder::Desc => " \u{25BC}",
        }
    }
}

fn sort_children(children: &[ChildResponse], field: SortField, order: SortOrder) -> Vec<ChildResponse> {
    let mut sorted = children.to_vec();
    // Always put folders before documents, then sort within each group
    sorted.sort_by(|a, b| {
        let a_is_folder = a.child_type == "folder";
        let b_is_folder = b.child_type == "folder";
        if a_is_folder != b_is_folder {
            return b_is_folder.cmp(&a_is_folder); // folders first
        }
        let cmp = match field {
            SortField::Title => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            SortField::Date => a.added_at.cmp(&b.added_at),
        };
        match order {
            SortOrder::Asc => cmp,
            SortOrder::Desc => cmp.reverse(),
        }
    });
    sorted
}

#[component]
pub fn FileBrowser(
    folder: ReadSignal<Option<FolderResponse>>,
    on_navigate_folder: Callback<String>,
    on_open_document: Callback<String>,
    /// When true, per-row Restore / Delete-forever actions are rendered.
    /// Only meaningful when the displayed folder is the user's Trash.
    #[prop(optional)] trash_mode: bool,
    /// Called with the doc id when the user clicks the row's Restore button.
    #[prop(optional)] on_restore: Option<Callback<String>>,
    /// Called with the doc id when the user clicks Delete forever.
    #[prop(optional)] on_purge: Option<Callback<String>>,
    /// Phase 5 M-P7 piece C — multi-select column. When supplied,
    /// the table grows a leading checkbox column; clicking a row's
    /// checkbox fires `on_toggle_select(child_id)`. The hosting
    /// page owns the actual `selected_ids` set and renders the
    /// selection action bar.
    #[prop(into, default = Signal::derive(|| HashSet::new()))]
    selected_ids: Signal<HashSet<String>>,
    #[prop(optional)] on_toggle_select: Option<Callback<String>>,
    /// #150 — folder management. When supplied, folder rows (outside Trash
    /// mode) expose Rename / Delete actions. `on_rename_folder` gets
    /// `(folder_id, current_title)`; `on_delete_folder` gets the folder id.
    /// The hosting page owns the rename prompt + delete confirmation.
    #[prop(optional)] on_rename_folder: Option<Callback<(String, String)>>,
    #[prop(optional)] on_delete_folder: Option<Callback<String>>,
    /// #150 — folder ids that must NOT show Rename/Delete (the synthetic
    /// Trash and Private rows spliced into Home). System folders are also
    /// rejected server-side; this hides the affordance for the ones shown
    /// in browse. Was a single-value `protected_folder_id` before #142
    /// added Private to the list.
    #[prop(optional)] protected_folder_ids: Vec<String>,
) -> impl IntoView {
    let selectable = on_toggle_select.is_some();
    let folder_actions_enabled = on_rename_folder.is_some() || on_delete_folder.is_some();
    let (sort_field, set_sort_field) = signal(SortField::Title);
    let (sort_order, set_sort_order) = signal(SortOrder::Asc);

    let toggle_sort = move |field: SortField| {
        if sort_field.get() == field {
            set_sort_order.set(sort_order.get().toggle());
        } else {
            set_sort_field.set(field);
            set_sort_order.set(SortOrder::Asc);
        }
    };

    view! {
        <div class="file-browser">
            {move || match folder.get() {
                None => view! {
                    <div class="empty-state">
                        <p class="empty-state-text">{crate::t!("common-loading")}</p>
                    </div>
                }.into_any(),
                Some(f) => {
                    let children = f.children.clone();
                    if children.is_empty() {
                        view! {
                            <div class="empty-state">
                                <p class="empty-state-text">
                                    {crate::t!("file-browser-empty")}
                                </p>
                            </div>
                        }.into_any()
                    } else {
                        let sorted = sort_children(&children, sort_field.get(), sort_order.get());
                        let current_field = sort_field.get();
                        let current_order = sort_order.get();

                        view! {
                            <table class="file-list" aria-label=crate::t!("a11y-file-table-label")>
                                <thead>
                                    <tr>
                                        {selectable.then(|| view! {
                                            <th class="file-row-select" aria-label=crate::t!("file-browser-th-select")></th>
                                        })}
                                        <th
                                            class="sortable-header"
                                            on:click=move |_| toggle_sort(SortField::Title)
                                        >
                                            {crate::t!("file-browser-th-title")}
                                            {if current_field == SortField::Title {
                                                current_order.indicator()
                                            } else { "" }}
                                        </th>
                                        <th
                                            class="sortable-header"
                                            on:click=move |_| toggle_sort(SortField::Date)
                                        >
                                            {crate::t!("file-browser-th-added")}
                                            {if current_field == SortField::Date {
                                                current_order.indicator()
                                            } else { "" }}
                                        </th>
                                        {trash_mode.then(|| view! { <th></th> })}
                                    </tr>
                                </thead>
                                <tbody>
                                    {sorted.into_iter().map(|child| {
                                        let child_id = child.child_id.clone();
                                        let child_title = child.title.clone();
                                        let child_type = child.child_type.clone();
                                        let on_nav = on_navigate_folder.clone();
                                        let on_open = on_open_document.clone();
                                        let id_for_click = child_id.clone();
                                        let is_folder = child_type == "folder";
                                        // #150: folder Rename/Delete actions —
                                        // folder rows only, outside Trash mode,
                                        // never on the protected (Trash) row.
                                        let is_protected = protected_folder_ids
                                            .iter()
                                            .any(|id| id == &child_id);
                                        let show_folder_actions =
                                            folder_actions_enabled && is_folder && !trash_mode
                                                && !is_protected;
                                        let rename_cb = on_rename_folder;
                                        let delete_cb = on_delete_folder;
                                        let folder_rename_id = child_id.clone();
                                        let folder_rename_title = child.title.clone();
                                        let folder_delete_id = child_id.clone();
                                        let folder_actions = show_folder_actions.then(|| view! {
                                            <span class="folder-row-actions">
                                                <button
                                                    class="btn btn-secondary btn-sm"
                                                    on:click=move |e: web_sys::MouseEvent| {
                                                        e.stop_propagation();
                                                        if let Some(cb) = rename_cb {
                                                            cb.run((
                                                                folder_rename_id.clone(),
                                                                folder_rename_title.clone(),
                                                            ));
                                                        }
                                                    }
                                                    title=crate::t!("folder-action-rename")
                                                >{crate::t!("folder-action-rename")}</button>
                                                <button
                                                    class="btn btn-danger btn-sm"
                                                    on:click=move |e: web_sys::MouseEvent| {
                                                        e.stop_propagation();
                                                        if let Some(cb) = delete_cb {
                                                            cb.run(folder_delete_id.clone());
                                                        }
                                                    }
                                                    title=crate::t!("folder-action-delete")
                                                >{crate::t!("folder-action-delete")}</button>
                                            </span>
                                        });
                                        // Icon now carries the only signal of doc type since
                                        // the Type column was removed. Title attribute provides
                                        // the textual label for screen readers / hover.
                                        let (icon, type_label) = match child_type.as_str() {
                                            "folder" => ("\u{1F4C1}", crate::t!("file-type-folder")),
                                            "spreadsheet" => ("\u{1F4CA}", crate::t!("file-type-spreadsheet")),
                                            "chat" => ("\u{1F4AC}", crate::t!("file-type-chat")),
                                            _ => ("\u{1F4C4}", crate::t!("file-type-document")),
                                        };
                                        let restore_id = child_id.clone();
                                        let purge_id = child_id.clone();
                                        let on_restore = on_restore.clone();
                                        let on_purge = on_purge.clone();
                                        let show_actions = trash_mode && !is_folder;

                                        // Only documents can ride the bulk selection
                                        // — folders aren't bulk-deletable in v1, so
                                        // hide their checkbox to keep the UX clear.
                                        let select_id = child_id.clone();
                                        let id_for_class = child_id.clone();
                                        let id_for_check = child_id.clone();
                                        let toggle = on_toggle_select;
                                        view! {
                                            <tr
                                                class:is-selected=move || {
                                                    selected_ids.with(|s| s.contains(&id_for_class))
                                                }
                                            >
                                                {(selectable && !is_folder).then(|| view! {
                                                    <td class="file-row-select">
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=move || {
                                                                selected_ids.with(|s| s.contains(&id_for_check))
                                                            }
                                                            on:click=move |e: web_sys::MouseEvent| {
                                                                e.stop_propagation();
                                                                if let Some(cb) = toggle {
                                                                    cb.run(select_id.clone());
                                                                }
                                                            }
                                                            aria-label=crate::t!("file-browser-th-select")
                                                        />
                                                    </td>
                                                })}
                                                {(selectable && is_folder).then(|| view! {
                                                    <td class="file-row-select"></td>
                                                })}
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
                                                        <span
                                                            class="file-icon"
                                                            title=type_label.clone()
                                                            aria-label=type_label
                                                        >{icon}</span>
                                                        {child_title}
                                                    </span>
                                                    {folder_actions}
                                                </td>
                                                <td class="file-date">{format_date(child.added_at, DateStyle::Short)}</td>
                                                {trash_mode.then(|| view! {
                                                    <td class="file-row-actions">
                                                        {show_actions.then(|| view! {
                                                            <button
                                                                class="btn btn-secondary"
                                                                on:click=move |e: web_sys::MouseEvent| {
                                                                    e.stop_propagation();
                                                                    if let Some(cb) = on_restore {
                                                                        cb.run(restore_id.clone());
                                                                    }
                                                                }
                                                            >{crate::t!("document-trash-restore")}</button>
                                                            <button
                                                                class="btn btn-danger"
                                                                on:click=move |e: web_sys::MouseEvent| {
                                                                    e.stop_propagation();
                                                                    if let Some(cb) = on_purge {
                                                                        cb.run(purge_id.clone());
                                                                    }
                                                                }
                                                            >{crate::t!("document-trash-delete-forever")}</button>
                                                        })}
                                                    </td>
                                                })}
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


#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::folders::ChildResponse;

    fn child(id: &str, ctype: &str, title: &str, added_at: i64) -> ChildResponse {
        ChildResponse {
            child_id: id.to_string(),
            child_type: ctype.to_string(),
            title: title.to_string(),
            added_at,
            is_deleted: false,
        }
    }

    #[test]
    fn sort_folders_before_docs() {
        let items = vec![
            child("1", "doc", "Zebra", 100),
            child("2", "folder", "Alpha", 200),
        ];
        let sorted = sort_children(&items, SortField::Title, SortOrder::Asc);
        assert_eq!(sorted[0].child_type, "folder");
        assert_eq!(sorted[1].child_type, "doc");
    }

    #[test]
    fn sort_by_title_asc() {
        let items = vec![
            child("1", "doc", "Banana", 100),
            child("2", "doc", "Apple", 200),
        ];
        let sorted = sort_children(&items, SortField::Title, SortOrder::Asc);
        assert_eq!(sorted[0].title, "Apple");
        assert_eq!(sorted[1].title, "Banana");
    }

    #[test]
    fn sort_by_title_desc() {
        let items = vec![
            child("1", "doc", "Apple", 100),
            child("2", "doc", "Banana", 200),
        ];
        let sorted = sort_children(&items, SortField::Title, SortOrder::Desc);
        assert_eq!(sorted[0].title, "Banana");
        assert_eq!(sorted[1].title, "Apple");
    }

    #[test]
    fn sort_by_date_asc() {
        let items = vec![
            child("1", "doc", "A", 300),
            child("2", "doc", "B", 100),
        ];
        let sorted = sort_children(&items, SortField::Date, SortOrder::Asc);
        assert_eq!(sorted[0].added_at, 100);
        assert_eq!(sorted[1].added_at, 300);
    }

    #[test]
    fn sort_empty() {
        let sorted = sort_children(&[], SortField::Title, SortOrder::Asc);
        assert!(sorted.is_empty());
    }

    #[test]
    fn sort_case_insensitive() {
        let items = vec![
            child("1", "doc", "banana", 100),
            child("2", "doc", "Apple", 200),
        ];
        let sorted = sort_children(&items, SortField::Title, SortOrder::Asc);
        assert_eq!(sorted[0].title, "Apple");
    }

    #[test]
    fn sort_order_toggle() {
        assert_eq!(SortOrder::Asc.toggle(), SortOrder::Desc);
        assert_eq!(SortOrder::Desc.toggle(), SortOrder::Asc);
    }
}
