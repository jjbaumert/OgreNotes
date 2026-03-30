use leptos::prelude::*;
use super::chat_panel::ChatPanel;

#[component]
pub fn Sidebar() -> impl IntoView {
    let (collapsed, set_collapsed) = signal(false);

    let toggle = move |_| set_collapsed.update(|c| *c = !*c);

    view! {
        <nav
            class:sidebar=true
            class:collapsed=collapsed
            aria-label="Main navigation"
        >
            <div class="sidebar-header">
                <span class="sidebar-logo">
                    {move || if collapsed.get() { "O" } else { "OgreNotes" }}
                </span>
                <button
                    class="toolbar-btn"
                    on:click=toggle
                    style="color: white;"
                    aria-label=move || if collapsed.get() { "Expand sidebar" } else { "Collapse sidebar" }
                    aria-expanded=move || (!collapsed.get()).to_string()
                >
                    {move || if collapsed.get() { "\u{2192}" } else { "\u{2190}" }}
                </button>
            </div>

            <div class="sidebar-section" style:display=move || if collapsed.get() { "none" } else { "block" }>
                <div class="sidebar-section-title">"Navigation"</div>
                <a href="/" class="sidebar-item">
                    "\u{1F3E0} Home"
                </a>
            </div>

            <div class="sidebar-section" style:display=move || if collapsed.get() { "none" } else { "block" }>
                <div class="sidebar-section-title">"Recent"</div>
                <div class="sidebar-item" style="color: rgba(255,255,255,0.4); font-style: italic;">
                    "No recent items"
                </div>
            </div>

            <div class="sidebar-section" style:display=move || if collapsed.get() { "none" } else { "block" }>
                <div class="sidebar-section-title">"Pinned"</div>
                <div class="sidebar-item" style="color: rgba(255,255,255,0.4); font-style: italic;">
                    "No pinned items"
                </div>
            </div>

            <div style:display=move || if collapsed.get() { "none" } else { "block" }>
                <ChatPanel />
            </div>
        </nav>
    }
}
