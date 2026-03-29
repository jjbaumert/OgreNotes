use leptos::prelude::*;

use crate::api::folders::{ChildResponse, FolderResponse};

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
) -> impl IntoView {
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
                        let sorted = sort_children(&children, sort_field.get(), sort_order.get());
                        let current_field = sort_field.get();
                        let current_order = sort_order.get();

                        view! {
                            <div class="breadcrumbs">
                                <span class="breadcrumb-link">{title}</span>
                            </div>
                            <table class="file-list">
                                <thead>
                                    <tr>
                                        <th
                                            class="sortable-header"
                                            on:click=move |_| toggle_sort(SortField::Title)
                                        >
                                            "Title"
                                            {if current_field == SortField::Title {
                                                current_order.indicator()
                                            } else { "" }}
                                        </th>
                                        <th>"Type"</th>
                                        <th
                                            class="sortable-header"
                                            on:click=move |_| toggle_sort(SortField::Date)
                                        >
                                            "Added"
                                            {if current_field == SortField::Date {
                                                current_order.indicator()
                                            } else { "" }}
                                        </th>
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
                                                <td class="file-type">{type_label}</td>
                                                <td class="file-date">{format_timestamp(child.added_at)}</td>
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

/// Format a microsecond timestamp as a short date string.
fn format_timestamp(usec: i64) -> String {
    if usec <= 0 {
        return "-".to_string();
    }
    let secs = usec / 1_000_000;
    let days_since_epoch = secs / 86400;
    // Simple date calculation (approximate, good enough for display)
    let mut y = 1970i64;
    let mut remaining = days_since_epoch;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let months = [31, if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 29 } else { 28 },
                  31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    for &days in &months {
        if remaining < days { break; }
        remaining -= days;
        m += 1;
    }
    format!("{}-{:02}-{:02}", y, m + 1, remaining + 1)
}
