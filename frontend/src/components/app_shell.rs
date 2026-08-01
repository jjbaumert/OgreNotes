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
use crate::components::quip_import::QuipImportWizard;
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
    /// Quip-import wizard (Phase 0). Shell-owned for the same reason
    /// as `template_picker_open` — the entry point lives in the
    /// sidebar's "+ New" menu, but the modal is mounted once here so
    /// state doesn't duplicate if more entry points are added later.
    pub quip_import_open: RwSignal<bool>,
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
    /// #174: "show me this folder in the file browser", by folder id.
    ///
    /// The folder view has no route of its own — folders are in-memory state
    /// on the home page (`on_navigate_folder` + the breadcrumb trail), so a
    /// shell-owned surface (the Quip import wizard's "Open folder") cannot
    /// reach one with a URL. Registered by the home page while it is the
    /// active outlet, exactly like [`Self::home_reset`], and cleared on its
    /// unmount; a caller that finds it `None` arms [`Self::requested_folder`]
    /// instead.
    pub open_folder: RwSignal<Option<Callback<String>>>,
    /// #174: the folder the home page should open on its NEXT mount.
    ///
    /// The hand-off for the case [`Self::open_folder`] cannot cover: the home
    /// page is not mounted (the wizard opens from any page, so "Open folder"
    /// can fire from a document), or it is mounted under `/trash` and needs a
    /// real route change first. The caller arms this and then navigates to
    /// `/`; the mounting page takes the value and opens that folder instead
    /// of Home. See [`FolderRequest`] for why it is a type and not a signal.
    pub requested_folder: FolderRequest,
}

/// #174: the one-shot "open this folder when the home page next mounts"
/// channel.
///
/// A type rather than a bare `RwSignal<Option<String>>` so that reading and
/// clearing cannot come apart: [`Self::take`] is the only way to see the
/// value, and it always empties the slot. A consumer able to *peek* could
/// read the id, then fail its own work and return without clearing — leaving
/// the request armed for the next, entirely unrelated Home mount, which would
/// silently drop the user into a folder no action of theirs asked for, with
/// nothing connecting the two. Fusing the read to the clear retires that whole
/// class of leak at the type level instead of by discipline at each call site.
#[derive(Clone, Copy)]
pub struct FolderRequest(RwSignal<Option<String>>);

impl FolderRequest {
    fn new() -> Self {
        Self(RwSignal::new(None))
    }

    /// Arm the request: ask the home page to open `folder_id` when it next
    /// mounts. The caller navigates to `/` right after. A second arm before
    /// the first is consumed replaces it — the newest ask is the live one.
    pub fn set(&self, folder_id: String) {
        self.0.set(Some(folder_id));
    }

    /// Take the armed request, clearing it in the same step. `None` when
    /// nothing is armed.
    ///
    /// Clearing is not the caller's job and cannot be skipped by an early
    /// return, a `?`, or an error branch. Consumers should take the request in
    /// straight-line setup code, *before* any fallible work, so that a mount
    /// which then fails simply drops it rather than stranding it.
    pub fn take(&self) -> Option<String> {
        let mut taken = None;
        self.0.update(|slot| taken = slot.take());
        taken
    }
}

impl ShellCtx {
    fn new() -> Self {
        Self {
            search_open: RwSignal::new(false),
            ask_open: RwSignal::new(false),
            template_picker_open: RwSignal::new(false),
            quip_import_open: RwSignal::new(false),
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
            open_folder: RwSignal::new(None),
            requested_folder: FolderRequest::new(),
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
    let on_quip_import = Callback::new(move |()| ctx.quip_import_open.set(true));

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
                on_quip_import=on_quip_import
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
            // Quip import wizard (Phase 0): token-entry + connect step.
            // Same shell-mount rationale as the template picker above.
            <QuipImportWizard
                visible=ctx.quip_import_open.read_only()
                on_close=Callback::new(move |_| ctx.quip_import_open.set(false))
            />
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The #174 follow-up regression.** A Home mount consumes the pending
    /// folder request up front and then does fallible work (`/users/me`, then
    /// the folder fetch). When that work fails the request is simply dropped —
    /// it must NOT survive to be picked up by the next, unrelated Home mount,
    /// which would silently drop the user into a folder with no action of
    /// theirs connecting the two.
    ///
    /// The failed mount is modelled by taking the request and never using it,
    /// which is exactly what the mount does on its error paths: it takes the
    /// request in straight-line setup, then its fetches fail and the value is
    /// dropped.
    ///
    /// (`ShellCtx::new` can't be built in a native test — it reads a
    /// localStorage pref — which is the other reason this channel is its own
    /// type: the invariant is testable without a browser.)
    #[test]
    fn a_mount_that_never_uses_the_request_leaves_nothing_armed() {
        let request = FolderRequest::new();
        request.set("folder-quip-import-1".to_string());

        // Mount #1: takes the request, then fails before it can act on it.
        let taken = request.take();
        assert_eq!(taken.as_deref(), Some("folder-quip-import-1"));
        drop(taken);

        // Mount #2 — an ordinary, unrelated visit to Home — must find nothing.
        assert_eq!(
            request.take(),
            None,
            "a failed mount must not leave a folder request armed for the next one",
        );
    }

    /// One request, one mount: consuming it twice cannot open the folder twice
    /// (a re-mount within a session is routine).
    #[test]
    fn a_request_is_consumed_exactly_once() {
        let request = FolderRequest::new();
        request.set("f1".to_string());

        assert_eq!(request.take().as_deref(), Some("f1"));
        assert_eq!(request.take(), None);
    }

    /// Nothing armed reads as nothing — the ordinary Home visit, which must
    /// not be perturbed by this channel existing at all.
    #[test]
    fn an_unarmed_request_hands_back_nothing() {
        assert_eq!(FolderRequest::new().take(), None);
    }

    /// Two "Open folder" clicks before the page mounts: the newest ask wins,
    /// and the older one does not linger behind it.
    #[test]
    fn the_newest_request_replaces_an_unconsumed_older_one() {
        let request = FolderRequest::new();
        request.set("older".to_string());
        request.set("newer".to_string());

        assert_eq!(request.take().as_deref(), Some("newer"));
        assert_eq!(request.take(), None);
    }
}
