// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! #152: persistent application shell.
//!
//! The sidebar used to be rendered inside every page (home, settings,
//! document), so any navigation — a full reload or even a client-side
//! route swap — tore it down and rebuilt it, flashing the theme-colored
//! page background for a frame. `AppShell` hoists the sidebar (and the
//! `.app-layout` flex wrapper) above the router `<Outlet/>`, so it stays
//! mounted across navigations and only the outlet content swaps. No
//! sidebar remount → no flash.
//!
//! Page ↔ sidebar state that used to travel as `<Sidebar>` props now
//! travels through [`ShellCtx`], provided here and consumed by the pages
//! via `use_context`. This is the first use of Leptos context in the
//! frontend; the struct is deliberately small and `Copy`.

use leptos::prelude::*;
use leptos_router::components::Outlet;
use leptos_router::hooks::use_navigate;

use crate::a11y;
use crate::components::sidebar::Sidebar;
use crate::components::template_picker_modal::TemplatePickerModal;
use crate::pages::document::{load_bool_pref, PREF_LINE_NUMBERS, PREF_PAGE_BREAKS};

/// Shared state between the persistent shell (sidebar + `.app-layout`
/// wrapper) and whichever page is mounted in the `<Outlet/>`. `Copy` so it
/// can be pulled from context once and freely captured into closures —
/// including leaked ones (the document page's `fullscreenchange` handler).
#[derive(Clone, Copy)]
pub struct ShellCtx {
    /// Open the search / command palette. The mounted page owns the actual
    /// `SearchDialog` (it carries a page-specific `CommandScope`) and reads
    /// this; the sidebar Search entry sets it.
    pub search_open: RwSignal<bool>,
    /// Open the Ask dialog (page-owned, same split as search).
    pub ask_open: RwSignal<bool>,
    /// #142: open the template picker modal. Unlike search/ask the modal
    /// itself is shell-owned (mounted in `AppShell`) since the picker fires
    /// from multiple surfaces (sidebar, Document menu, home page) — having
    /// one mount avoids state duplication.
    pub template_picker_open: RwSignal<bool>,
    /// Mobile drawer open state. The shell renders the backdrop; a page's
    /// header hamburger toggles it.
    pub drawer_open: RwSignal<bool>,
    /// #144 sidebar refresh ticks — a page bumps these when the user
    /// stars/unstars or changes collection membership.
    pub favorites_dirty: RwSignal<u32>,
    pub collections_dirty: RwSignal<u32>,
    /// Layout flags applied as `class:*` on the shell's `.app-layout` (so
    /// the existing `.app-layout.*` / `.show-line-numbers` CSS keeps
    /// matching — those selectors target descendants, which all live in the
    /// Outlet). Only the document page sets these; other pages leave them
    /// at their defaults.
    pub focus_mode: RwSignal<bool>,
    pub expanded: RwSignal<bool>,
    pub show_line_numbers: RwSignal<bool>,
    pub show_page_breaks: RwSignal<bool>,
    /// The home page registers an in-memory "reset to the home root"
    /// callback while it's the active outlet (clearing it on cleanup). The
    /// Home nav runs it when set — avoiding a route change on the page that
    /// owns the state — and otherwise client-side-navigates to `/`.
    pub home_reset: RwSignal<Option<Callback<()>>>,
}

impl ShellCtx {
    fn new() -> Self {
        Self {
            search_open: RwSignal::new(false),
            ask_open: RwSignal::new(false),
            template_picker_open: RwSignal::new(false),
            drawer_open: RwSignal::new(false),
            favorites_dirty: RwSignal::new(0),
            collections_dirty: RwSignal::new(0),
            focus_mode: RwSignal::new(false),
            expanded: RwSignal::new(false),
            // Seed the persisted editor view-options once, here, so the
            // toggle classes paint correctly even before a document mounts
            // (and survive navigation between documents).
            show_line_numbers: RwSignal::new(load_bool_pref(PREF_LINE_NUMBERS)),
            show_page_breaks: RwSignal::new(load_bool_pref(PREF_PAGE_BREAKS)),
            home_reset: RwSignal::new(None),
        }
    }
}

#[component]
pub fn AppShell() -> impl IntoView {
    let ctx = ShellCtx::new();
    provide_context(ctx);

    // #152: install the client-side navigation bridge for the command palette.
    // Palette commands are registered before mount (no Router context), so they
    // route through this bridge; the shell — always mounted in-app, with Router
    // context — provides the navigate closure. Cleared on unmount.
    let nav_for_bridge = use_navigate();
    crate::commands::nav_bridge::set_navigate(Some(Callback::new(
        move |path: String| nav_for_bridge(&path, Default::default()),
    )));
    on_cleanup(|| crate::commands::nav_bridge::set_navigate(None));

    let on_search = Callback::new(move |()| ctx.search_open.set(true));
    let on_ask = Callback::new(move |()| ctx.ask_open.set(true));
    let on_templates = Callback::new(move |()| ctx.template_picker_open.set(true));

    // Home nav: run the page-registered in-memory reset if present (the
    // home page is the active outlet), else a client-side navigate to "/".
    // Because the sidebar lives here in the shell, navigate("/") swaps only
    // the outlet content — the sidebar never remounts, so no flash.
    let navigate = use_navigate();
    let on_home = Callback::new(move |()| {
        if let Some(cb) = ctx.home_reset.get_untracked() {
            cb.run(());
        } else {
            navigate("/", Default::default());
        }
    });

    view! {
        <div
            class="app-layout"
            class:focus-mode=move || ctx.focus_mode.get()
            class:expanded=move || ctx.expanded.get()
            class:show-line-numbers=move || ctx.show_line_numbers.get()
            class:show-page-breaks=move || ctx.show_page_breaks.get()
        >
            <Sidebar
                on_search=on_search
                on_ask=on_ask
                on_templates=on_templates
                on_home=on_home
                is_open=ctx.drawer_open.read_only()
                favorites_refresh=ctx.favorites_dirty
                collections_refresh=ctx.collections_dirty
            />
            <Show when=move || ctx.drawer_open.get()>
                <div
                    class="drawer-backdrop sidebar-backdrop"
                    on:click=move |_| a11y::defer(move || ctx.drawer_open.set(false))
                ></div>
            </Show>
            <Outlet/>
            // #142: shell-mounted template picker. One modal serves every
            // entry point (sidebar Templates row, Document menu "New from
            // Template", home "New from Template").
            <TemplatePickerModal
                visible=ctx.template_picker_open.read_only()
                on_close=Callback::new(move |_| ctx.template_picker_open.set(false))
            />
        </div>
    }
}
