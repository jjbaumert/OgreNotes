use leptos::prelude::*;

/// @ menu: triggered by typing `@` in the editor.
/// Provides typeahead search for people, documents, and insertion options.
#[component]
pub fn AtMenu(
    /// Whether the menu is visible.
    visible: ReadSignal<bool>,
    /// The current search query (text after @).
    query: ReadSignal<String>,
    /// Position: left pixels from viewport edge.
    left: ReadSignal<f64>,
    /// Position: top pixels from viewport edge.
    top: ReadSignal<f64>,
    /// Callback when an item is selected.
    on_select: Callback<AtMenuItem>,
    /// Callback to close the menu.
    on_close: Callback<()>,
) -> impl IntoView {
    #[allow(unused_variables)]
    let (results, set_results) = signal::<Vec<AtMenuItem>>(Vec::new());

    // TODO: Search users and documents based on query
    // Effect::new(move |_| {
    //     let q = query.get();
    //     if q.is_empty() { return; }
    //     // API call to search
    // });

    view! {
        <Show when=move || visible.get()>
            <div
                class="at-menu"
                style:left=move || format!("{}px", left.get())
                style:top=move || format!("{}px", top.get())
            >
                <div class="at-menu-header">
                    <span class="at-menu-query">"@"{move || query.get()}</span>
                </div>
                <div class="at-menu-body">
                    {move || {
                        let items = results.get();
                        if items.is_empty() {
                            view! {
                                <div class="at-menu-empty">
                                    "Type to search people and documents..."
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="at-menu-results">
                                    {items.into_iter().map(|item| {
                                        let item_clone = item.clone();
                                        view! {
                                            <div
                                                class="at-menu-item"
                                                on:click=move |_| on_select.run(item_clone.clone())
                                            >
                                                <span class="at-menu-icon">{item.icon}</span>
                                                <span class="at-menu-label">{item.label}</span>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </div>
        </Show>
    }
}

/// An item in the @ menu.
#[derive(Debug, Clone)]
pub struct AtMenuItem {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub item_type: AtMenuItemType,
}

/// Types of @ menu items.
#[derive(Debug, Clone)]
pub enum AtMenuItemType {
    /// Mention a user.
    User,
    /// Link to a document.
    Document,
    /// Insert a block element.
    Insert(String),
}
