// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::api::client;
use crate::api::documents;
use crate::theme::{self, ExplicitTheme};
use super::account_menu::AccountMenu;
use super::app_shell::ShellCtx;
use super::chat_panel::ChatPanel;
use super::confirm_dialog::ConfirmDialog;
use super::folder_picker::FolderPickerDialog;
use super::menu::{ContextMenu, MenuEntry};

/// Flip between explicit Light and Dark for the collapsed icon-strip
/// toggle. Reads the current rendered theme from `data-theme` on the
/// `<html>` element rather than tracking a Rust-side mirror — the
/// attribute is the single source of truth set by `theme::apply_*`,
/// so reading it sidesteps any drift between a local signal and the
/// real DOM state (e.g. an unrelated tab persisted a change). When
/// the attribute isn't set yet (system-mode bootstrap), default to
/// Dark since the brand defaults to dark and a single click flip
/// matches the user's likely intent.
fn toggle_light_dark() {
    let current_is_dark = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .map(|v| v != "light")
        .unwrap_or(true);
    let target = if current_is_dark {
        ExplicitTheme::Light
    } else {
        ExplicitTheme::Dark
    };
    leptos::task::spawn_local(async move {
        if let Err(e) = theme::change_theme(Some(target)).await {
            web_sys::console::warn_1(
                &format!("theme persistence failed: {e:?}").into(),
            );
        }
    });
}

/// localStorage key holding the sidebar collapse state ("1" collapsed,
/// "0"/absent expanded). Persisted so the choice survives the
/// full-page reloads that the nav icons trigger via `set_href`.
const SIDEBAR_COLLAPSED_KEY: &str = "ogrenotes.sidebar-collapsed";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

fn read_collapsed() -> bool {
    local_storage()
        .and_then(|s| s.get_item(SIDEBAR_COLLAPSED_KEY).ok().flatten())
        .map(|v| v == "1")
        .unwrap_or(false)
}

fn write_collapsed(collapsed: bool) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(SIDEBAR_COLLAPSED_KEY, if collapsed { "1" } else { "0" });
    }
}

/// Tracks the "icon-rail" viewport band (641–1024px). Inside this band the
/// sidebar is forced to its collapsed icon strip regardless of the user's
/// manual collapse choice; above it the manual choice wins, and below it
/// (≤640px) the mobile drawer takes over. This is the middle of the three
/// responsive phases: expanded → icon rail → off-canvas drawer.
///
/// Mirrors the `prefers-color-scheme` listener in `theme.rs` — the change
/// callback re-queries `match_media` rather than reading a
/// `MediaQueryListEvent`, so no extra web-sys feature is required.
fn use_rail_signal() -> ReadSignal<bool> {
    // Keep these bounds in sync with the CSS breakpoints: the lower bound is
    // `--bp-mobile` + 1 (below it, the ≤640px drawer takes over) and the upper
    // bound is `--bp-tablet` (above it, the expanded sidebar). Both tokens live
    // in `variables.css` and are duplicated as literals in `responsive.css`
    // (CSS `@media` can't read `var()`). A change there must change this too.
    const RAIL_QUERY: &str = "(min-width: 641px) and (max-width: 1024px)";
    let (rail, set_rail) = signal(false);
    let Some(window) = web_sys::window() else {
        return rail;
    };
    let Ok(Some(media)) = window.match_media(RAIL_QUERY) else {
        return rail;
    };
    set_rail.set(media.matches());
    let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        if let Some(w) = web_sys::window() {
            if let Ok(Some(m)) = w.match_media(RAIL_QUERY) {
                set_rail.set(m.matches());
            }
        }
    }) as Box<dyn FnMut(web_sys::Event)>);
    let _ = media.add_event_listener_with_callback("change", closure.as_ref().unchecked_ref());
    // Leak the closure: it must outlive this function and stay attached to
    // `media` for the app's lifetime. Safe because `Sidebar` is a mount-once
    // persistent-shell singleton (it lives in `AppShell`), so exactly one
    // listener is ever created.
    //
    // NOTE: unlike `theme.rs`, which guards its `prefers-color-scheme` listener
    // behind a `thread_local` to stay idempotent across repeated calls, this
    // has NO such guard. If `Sidebar` is ever changed to remount, add one here
    // (or a thread_local) — otherwise each remount leaks another live listener
    // firing on an orphaned signal. (`on_cleanup` can't own the closure: Leptos
    // 0.7 requires it to be Send + Sync, which a wasm `Closure` is not.)
    closure.forget();
    rail
}

/// True while the window is actively resizing. Used to suppress the sidebar's
/// CSS transitions during a resize so responsive phase changes snap cleanly
/// instead of animating — in particular the ≤640px drawer, whose `transform`
/// and width would otherwise slide-and-widen ("flash bigger") as the viewport
/// crosses the breakpoint (a transition firing on a breakpoint change). The
/// flag clears ~180ms after resize settles, so the hamburger open/close slide
/// (not a resize) is unaffected. Same mount-once leak caveat as
/// [`use_rail_signal`].
fn use_resize_suppression() -> ReadSignal<bool> {
    use std::cell::Cell;
    use std::rc::Rc;
    let (resizing, set_resizing) = signal(false);
    let Some(window) = web_sys::window() else {
        return resizing;
    };
    // Debounce the re-enable: each resize bumps a generation and schedules a
    // timeout that only clears the flag if no newer resize has arrived since.
    let generation: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    // Only suppress on WIDTH changes. Our breakpoints are width-based, and
    // mobile browsers fire real `resize` events on height-only changes (URL-bar
    // / soft-keyboard show-hide during scroll or a swipe-to-close). Ignoring
    // those keeps an in-progress hamburger/swipe drawer slide from snapping.
    let read_width = || {
        web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i32
    };
    let last_width: Rc<Cell<i32>> = Rc::new(Cell::new(read_width()));
    let window_for_timeout = window.clone();
    let resize_cb = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        let width = read_width();
        if width == last_width.get() {
            return; // height-only resize — leave transitions intact
        }
        last_width.set(width);
        if !resizing.get_untracked() {
            set_resizing.set(true);
        }
        let g = generation.get().wrapping_add(1);
        generation.set(g);
        let generation_check = generation.clone();
        let clear = Closure::once_into_js(move || {
            if generation_check.get() == g {
                set_resizing.set(false);
            }
        });
        let _ = window_for_timeout.set_timeout_with_callback_and_timeout_and_arguments_0(
            clear.unchecked_ref(),
            180,
        );
    }) as Box<dyn FnMut(web_sys::Event)>);
    let _ = window.add_event_listener_with_callback("resize", resize_cb.as_ref().unchecked_ref());
    resize_cb.forget();
    resizing
}

/// The document a row context menu (right-click or its `⋯` button) is
/// currently open for, plus where to place the menu.
#[derive(Clone)]
struct DocMenuTarget {
    id: String,
    x: f64,
    y: f64,
}

/// Copy the canonical `/d/:id` URL (stable opaque doc id — same rule
/// as the document page's Copy Link, #101) to the clipboard via
/// `navigator.clipboard.writeText`, without eval/Function.
fn copy_doc_link(id: &str) {
    let Some(window) = web_sys::window() else { return };
    let Ok(origin) = window.location().origin() else { return };
    let href = format!("{origin}/d/{id}");
    let write_text = js_sys::Reflect::get(&window.navigator(), &"clipboard".into())
        .and_then(|clip| js_sys::Reflect::get(&clip, &"writeText".into()))
        .and_then(|func| func.dyn_into::<js_sys::Function>());
    if let Ok(write_text) = write_text {
        let clip = js_sys::Reflect::get(&window.navigator(), &"clipboard".into())
            .unwrap_or(wasm_bindgen::JsValue::NULL);
        let _ = write_text.call1(&clip, &href.into());
    }
}

/// Anchor point for a menu opened from a row's `⋯` button: just under
/// the button's bottom-start corner (falls back to the click point).
fn button_anchor(ev: &web_sys::MouseEvent) -> (f64, f64) {
    ev.current_target()
        .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        .map(|el| {
            let r = el.get_bounding_client_rect();
            (r.left(), r.bottom() + 2.0)
        })
        .unwrap_or((ev.client_x() as f64, ev.client_y() as f64))
}

#[component]
pub fn Sidebar(
    #[prop(optional)] on_search: Option<Callback<()>>,
    /// Phase 6 M-6.2 piece B: open the Ask dialog. Page owns the
    /// AskDialog mount + visibility signal; this callback flips it
    /// from the sidebar entry. Absent on pages that don't host an
    /// Ask surface yet (admin, MFA flows).
    #[prop(optional)] on_ask: Option<Callback<()>>,
    /// #142: open the template picker modal. Shell-owned (mounted in
    /// `AppShell`); the sidebar Templates entry calls this.
    #[prop(optional)] on_templates: Option<Callback<()>>,
    /// #152: when set (the home page provides it), the Home item resets to
    /// the home root IN MEMORY instead of a full-page reload — no teardown
    /// flash. Absent elsewhere, where the Home item full-reloads to `/`.
    #[prop(optional)] on_home: Option<Callback<()>>,
    /// Whether the sidebar is open as a mobile drawer. Has no effect on desktop
    /// (where the sidebar is always visible). When this signal is true on mobile,
    /// CSS slides the sidebar in via the .is-open class.
    #[prop(optional)] is_open: Option<ReadSignal<bool>>,
    /// #144: bump to force the Favorites section to re-fetch (the document
    /// page bumps it when the user stars/unstars). Absent → fetch once on
    /// mount only.
    #[prop(optional, into)] favorites_refresh: Option<Signal<u32>>,
    /// #144: bump to force the Collections sections to re-fetch (the document
    /// page bumps it on collection create / membership change).
    #[prop(optional, into)] collections_refresh: Option<Signal<u32>>,
) -> impl IntoView {
    // Initialize from persisted state so navigating from the icon
    // strip (the nav icons full-reload via `set_href`) keeps the
    // collapsed view instead of springing back to expanded.
    //
    // `manual_collapsed` is the user's explicit choice (persisted). `rail`
    // is the responsive 641–1024px band that forces the icon strip. The
    // effective `collapsed` used throughout the view is the OR of the two:
    // above the rail band the manual choice wins; inside it, icons are
    // forced. Only `manual_collapsed` is persisted, so leaving the band
    // restores the user's real choice.
    let (manual_collapsed, set_manual_collapsed) = signal(read_collapsed());
    let rail = use_rail_signal();
    // Suppress transitions during resize so phase changes snap cleanly (no
    // drawer "flash bigger" as the ≤640px breakpoint is crossed).
    let resizing = use_resize_suppression();
    // Lets the Chats icon open the ChatPanel from inside the forced rail band
    // (ChatPanel only renders in the expanded sidebar). Transient — never
    // persisted — so peeking at Chat never pollutes the real collapse choice.
    let (chat_expand, set_chat_expand) = signal(false);
    let collapsed =
        Memo::new(move |_| !chat_expand.get() && (manual_collapsed.get() || rail.get()));
    // Scope the transient chat override to the rail band: the moment the
    // viewport leaves the band (either direction out of 641–1024px), drop it
    // so the manual choice governs again above the band and the drawer below.
    // Without this, resizing rail→desktop with Chat open would keep the sidebar
    // expanded against a persisted "collapsed" preference until the next click.
    Effect::new(move |_| {
        if !rail.get() {
            set_chat_expand.set(false);
        }
    });

    // #144: the current user's starred docs. Fetched on mount and whenever
    // `favorites_refresh` ticks.
    let favorites: RwSignal<Vec<crate::api::documents::FavoriteItem>> = RwSignal::new(Vec::new());
    Effect::new(move |_| {
        if let Some(r) = favorites_refresh {
            r.track();
        }
        leptos::task::spawn_local(async move {
            if let Ok(list) = crate::api::documents::list_favorites().await {
                favorites.set(list);
            }
        });
    });

    // #144: the user's collections (named groups), each with its docs inlined.
    // Fetched on mount and whenever `collections_refresh` ticks.
    let collections: RwSignal<Vec<crate::api::documents::CollectionWithItems>> =
        RwSignal::new(Vec::new());
    Effect::new(move |_| {
        if let Some(r) = collections_refresh {
            r.track();
        }
        leptos::task::spawn_local(async move {
            if let Ok(list) = crate::api::documents::list_collections().await {
                collections.set(list);
            }
        });
    });

    // ── Per-document row menu (right-click / `⋯`) ─────────────
    // One shared ContextMenu + move/delete dialogs for every favorite
    // and collection row. Refreshes ride the shell's dirty ticks — the
    // same signals this component receives as its refresh props — so
    // a mutation here updates every listening surface.
    let doc_menu: RwSignal<Option<DocMenuTarget>> = RwSignal::new(None);
    let move_target: RwSignal<Option<DocMenuTarget>> = RwSignal::new(None);
    let delete_target: RwSignal<Option<DocMenuTarget>> = RwSignal::new(None);
    let shell = use_context::<ShellCtx>();
    let bump_favorites = move || {
        if let Some(ctx) = shell {
            ctx.favorites_dirty.update(|n| *n = n.wrapping_add(1));
        }
    };
    let bump_all = move || {
        if let Some(ctx) = shell {
            ctx.favorites_dirty.update(|n| *n = n.wrapping_add(1));
            ctx.collections_dirty.update(|n| *n = n.wrapping_add(1));
        }
    };

    // Mobile drawer hygiene: close the drawer when a sidebar entry
    // navigates or opens a dialog — otherwise it stays parked over the
    // new page. No-op on desktop (drawer_open only matters ≤640px).
    let close_drawer = move || {
        if let Some(ctx) = shell {
            ctx.drawer_open.set(false);
        }
    };

    // ── "+ New" (sidebar header) ──────────────────────────────
    // Creation used to live only on the home page's action bar; this
    // makes it reachable from any page. Same create-then-navigate
    // flow as home.rs.
    let new_menu: RwSignal<Option<(f64, f64)>> = RwSignal::new(None);
    let new_menu_entries = Callback::new(move |()| {
        let mut items = vec![
            MenuEntry::action(crate::t!("sidebar-new-document"), move || {
                close_drawer();
                leptos::task::spawn_local(async move {
                    match documents::create_document("Untitled", None).await {
                        Ok(doc) => crate::commands::nav_bridge::go(&format!("/d/{}", doc.id)),
                        Err(e) => web_sys::console::error_1(
                            &format!("Failed to create document: {e}").into(),
                        ),
                    }
                });
            }),
            MenuEntry::action(crate::t!("sidebar-new-spreadsheet"), move || {
                close_drawer();
                leptos::task::spawn_local(async move {
                    match documents::create_spreadsheet("Untitled Spreadsheet", None).await {
                        Ok(doc) => crate::commands::nav_bridge::go(&format!("/d/{}", doc.id)),
                        Err(e) => web_sys::console::error_1(
                            &format!("Failed to create spreadsheet: {e}").into(),
                        ),
                    }
                });
            }),
        ];
        if let Some(cb) = on_templates {
            items.push(MenuEntry::Separator);
            items.push(MenuEntry::action(crate::t!("menubar-doc-new-template"), move || {
                close_drawer();
                cb.run(());
            }));
        }
        items
    });

    // ── Swipe-to-close for the mobile drawer ──────────────────
    // A horizontal swipe toward the drawer's hidden edge (inline-start:
    // left in LTR, right in RTL) closes it — matching the slide-in
    // animation's direction.
    let swipe_start: StoredValue<Option<(f64, f64)>> = StoredValue::new(None);
    let on_nav_touchstart = move |ev: web_sys::TouchEvent| {
        swipe_start.set_value(crate::touch::first_touch_xy(&ev));
    };
    let on_nav_touchend = move |ev: web_sys::TouchEvent| {
        let Some((sx, sy)) = swipe_start.get_value() else { return };
        swipe_start.set_value(None);
        let Some(touch) = ev.changed_touches().get(0) else { return };
        let (ex, ey) = (touch.client_x() as f64, touch.client_y() as f64);
        let Some(dir) = crate::touch::swipe_direction(sx, sy, ex, ey, 50.0) else { return };
        let drawer_open = is_open.map(|s| s.get_untracked()).unwrap_or(false);
        if !drawer_open {
            return;
        }
        let rtl = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .map(|el| el.get_attribute("dir").as_deref() == Some("rtl"))
            .unwrap_or(false);
        let closes = matches!(
            (dir, rtl),
            (crate::touch::SwipeDir::Left, false) | (crate::touch::SwipeDir::Right, true)
        );
        if closes {
            close_drawer();
        }
    };

    let doc_menu_entries = Callback::new(move |()| {
        let Some(target) = doc_menu.get() else {
            return Vec::new();
        };
        let is_favorite = favorites.get().iter().any(|f| f.id == target.id);
        let id_open = target.id.clone();
        let id_favorite = target.id.clone();
        let id_link = target.id.clone();
        let target_move = target.clone();
        let target_delete = target.clone();
        vec![
            MenuEntry::action(crate::t!("sidebar-doc-open-new-tab"), move || {
                if let Some(window) = web_sys::window() {
                    let _ = window
                        .open_with_url_and_target(&format!("/d/{id_open}"), "_blank");
                }
            }),
            MenuEntry::action(
                if is_favorite {
                    crate::t!("favorite-menu-remove")
                } else {
                    crate::t!("favorite-menu-add")
                },
                move || {
                    let id = id_favorite.clone();
                    leptos::task::spawn_local(async move {
                        let res = if is_favorite {
                            documents::remove_favorite(&id).await
                        } else {
                            documents::add_favorite(&id).await
                        };
                        if res.is_ok() {
                            bump_favorites();
                        }
                    });
                },
            ),
            MenuEntry::action(crate::t!("menubar-doc-copy-link"), move || {
                copy_doc_link(&id_link);
            }),
            MenuEntry::Separator,
            MenuEntry::action(crate::t!("menubar-doc-move-folder"), move || {
                move_target.set(Some(target_move.clone()));
            }),
            MenuEntry::Separator,
            MenuEntry::action(crate::t!("document-trash-dialog-confirm"), move || {
                delete_target.set(Some(target_delete.clone()));
            })
            .danger(),
        ]
    });

    // Collapse/expand. Read the effective state BEFORE mutating so clearing
    // `chat_expand` in the same handler can't flip the intent mid-computation.
    // Above the rail band we flip and persist the manual choice; inside the
    // rail band we only drop the transient chat expand — `rail` re-forces the
    // strip and the persisted desktop choice stays untouched.
    let toggle = move |_| {
        let was_collapsed = collapsed.get();
        set_chat_expand.set(false);
        if !rail.get() {
            set_manual_collapsed.set(!was_collapsed);
        }
    };

    // Persist every manual collapse/expand (the header toggle, and the Chats
    // icon which expands to reveal the ChatPanel) so the choice survives
    // reloads. The responsive `rail` forcing is deliberately NOT persisted.
    Effect::new(move |_| write_collapsed(manual_collapsed.get()));

    let is_open_class = move || is_open.map(|s| s.get()).unwrap_or(false);

    let shortcut_hint = web_sys::window()
        .and_then(|w| w.navigator().platform().ok())
        .map(|p| p.to_lowercase().contains("mac"))
        .unwrap_or(false)
        .then_some("\u{2318}K")
        .unwrap_or("Ctrl+K");

    view! {
        <nav
            class:sidebar=true
            class:collapsed=collapsed
            class:is-open=is_open_class
            class:no-anim=resizing
            class:chat-expanded=chat_expand
            aria-label=crate::t!("sidebar-aria-main-nav")
            on:touchstart=on_nav_touchstart
            on:touchend=on_nav_touchend
        >
            <div class="sidebar-header">
                <span class="sidebar-logo">
                    {move || if collapsed.get() { "O" } else { "OgreNotes" }}
                </span>
                <button
                    class="toolbar-btn"
                    style="color: white;"
                    style:display=move || if collapsed.get() { "none" } else { "inline-flex" }
                    aria-haspopup="menu"
                    aria-label=crate::t!("sidebar-new-aria")
                    title=crate::t!("sidebar-new-aria")
                    on:click=move |ev: web_sys::MouseEvent| {
                        let (x, y) = button_anchor(&ev);
                        new_menu.set(Some((x, y)));
                    }
                >"+"</button>
                <button
                    class="toolbar-btn"
                    on:click=toggle
                    style="color: white;"
                    // Hidden in the 641–1024px rail band (collapse is forced by
                    // viewport size) — but shown when the Chats icon has forced
                    // an expand there, so the user can close it again.
                    style:display=move || if rail.get() && !chat_expand.get() { "none" } else { "inline-flex" }
                    aria-label=move || if collapsed.get() { crate::t!("sidebar-aria-expand") } else { crate::t!("sidebar-aria-collapse") }
                    aria-expanded=move || (!collapsed.get()).to_string()
                >
                    {move || if collapsed.get() { "\u{2192}" } else { "\u{2190}" }}
                </button>
            </div>

            // Collapsed-state icon strip — replaces the empty bar
            // that the original collapsed view showed. Items mirror
            // the expanded sidebar's navigation + footer; Chats
            // re-expands the sidebar (the ChatPanel only renders
            // inside the expanded view). Theme is a 2-state toggle
            // here (Light ↔ Dark); the full 3-way theme + locale
            // controls live in the /settings Appearance section.
            <div
                class="sidebar-icons"
                style:display=move || if collapsed.get() { "flex" } else { "none" }
                role="group"
                aria-label=crate::t!("sidebar-aria-main-nav")
            >
                <button
                    class="sidebar-icon-btn"
                    title=crate::t!("sidebar-home")
                    aria-label=crate::t!("sidebar-home")
                    // #152: same flash-free path as the expanded Home item —
                    // use `on_home` (in-memory reset / client-side nav inside
                    // the persistent shell) when provided, else full-reload.
                    on:click=move |_| {
                        if let Some(cb) = on_home {
                            cb.run(());
                        } else if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href("/");
                        }
                    }
                >"\u{1F3E0}"</button>

                {move || on_search.map(|cb| view! {
                    <button
                        class="sidebar-icon-btn"
                        title=crate::t!("sidebar-search")
                        aria-label=crate::t!("sidebar-search")
                        on:click=move |_| cb.run(())
                    >"\u{1F50D}"</button>
                })}

                {move || on_ask.map(|cb| view! {
                    <button
                        class="sidebar-icon-btn"
                        title=crate::t!("sidebar-ask")
                        aria-label=crate::t!("sidebar-ask")
                        on:click=move |_| cb.run(())
                    >"\u{2728}"</button>
                })}

                // Tasks nav intentionally omitted (#103): there is no
                // tasks feature or /tasks route yet, and the icon used to
                // dead-end on the not-found page. Restore this button
                // (routed + i18n'd) when the tasks surface ships.

                <button
                    class="sidebar-icon-btn"
                    title="Chats"
                    aria-label="Chats"
                    // Click expands the sidebar so the ChatPanel becomes
                    // visible; re-collapse via the ← button in the header.
                    // Above the rail band this persists the expand (the manual
                    // choice); inside the rail band it uses the transient
                    // `chat_expand` override so the strip returns on close.
                    on:click=move |_| {
                        if rail.get() {
                            set_chat_expand.set(true);
                        } else {
                            set_manual_collapsed.set(false);
                        }
                    }
                >"\u{1F4AC}"</button>

                <button
                    class="sidebar-icon-btn"
                    title=crate::t!("theme-aria-label")
                    aria-label=crate::t!("theme-aria-label")
                    on:click=move |_| toggle_light_dark()
                >"\u{1F319}"</button>

                <button
                    class="sidebar-icon-btn"
                    title="Profile"
                    aria-label="Profile"
                    // #152: client-side nav straight to the (in-shell) settings
                    // route — no full reload, and skips the /profile redirect
                    // bounce (which is a flat, shell-less route).
                    on:click=move |_| crate::commands::nav_bridge::go("/settings")
                >"\u{1F464}"</button>

                <button
                    class="sidebar-icon-btn"
                    title=crate::t!("sidebar-sign-out")
                    aria-label=crate::t!("sidebar-sign-out")
                    on:click=move |_| {
                        leptos::task::spawn_local(async move {
                            client::logout().await;
                            if let Some(window) = web_sys::window() {
                                let _ = window.location().set_href("/login");
                            }
                        });
                    }
                >"\u{1F6AA}"</button>
            </div>

            <div class="sidebar-section" style:display=move || if collapsed.get() { "none" } else { "block" }>
                // M-P2 piece 1 pilot — the first user-visible string
                // routed through the i18n harness. See
                // frontend/locales/en-US/main.ftl for the catalog;
                // subsequent pieces convert the rest of the strings.
                <div class="sidebar-section-title">{crate::t!("sidebar-section-navigation")}</div>
                // Home. On the home page (`on_home` provided) this resets to
                // the home root IN MEMORY — no `set_href` full reload, so no
                // teardown flash (#152), and it still works when the URL is
                // already `/` but the user has navigated into a subfolder.
                // On other pages there's no `on_home`, so it full-reloads to
                // `/` (the clean teardown that matters when leaving a
                // document/editor page).
                <div
                    class="sidebar-item"
                    on:click=move |_| {
                        close_drawer();
                        if let Some(cb) = on_home {
                            cb.run(());
                        } else if let Some(window) = web_sys::window() {
                            let _ = window.location().set_href("/");
                        }
                    }
                    style="cursor: pointer;"
                >
                    {format!("\u{1F3E0} {}", crate::t!("sidebar-home"))}
                </div>
                {move || on_search.map(|cb| view! {
                    <div
                        class="sidebar-item"
                        on:click=move |_| { close_drawer(); cb.run(()); }
                        style="cursor: pointer;"
                    >
                        <span>{format!("\u{1F50D} {}", crate::t!("sidebar-search"))}</span>
                        <span class="sidebar-item-shortcut">
                            {shortcut_hint}
                        </span>
                    </div>
                })}
                {move || on_ask.map(|cb| view! {
                    <div
                        class="sidebar-item"
                        on:click=move |_| { close_drawer(); cb.run(()); }
                        style="cursor: pointer;"
                    >
                        <span>{format!("\u{2728} {}", crate::t!("sidebar-ask"))}</span>
                    </div>
                })}
                // #142: Templates entry — opens the shell-mounted template
                // picker. Shown whenever on_templates is wired (always when
                // mounted under AppShell).
                {move || on_templates.map(|cb| view! {
                    <div
                        class="sidebar-item"
                        on:click=move |_| { close_drawer(); cb.run(()); }
                        style="cursor: pointer;"
                    >
                        <span>{format!("\u{1F4CB} {}", crate::t!("sidebar-templates"))}</span>
                    </div>
                })}
            </div>

            // The "Recent" section that used to sit here was a
            // hardcoded, permanently-empty placeholder — removed until
            // a real recents source exists.

            <div class="sidebar-section" style:display=move || if collapsed.get() { "none" } else { "block" }>
                <div class="sidebar-section-title">{crate::t!("sidebar-section-favorites")}</div>
                {move || {
                    let favs = favorites.get();
                    if favs.is_empty() {
                        view! {
                            <div class="sidebar-item sidebar-item-muted">
                                {crate::t!("sidebar-empty-favorites")}
                            </div>
                        }.into_any()
                    } else {
                        favs.into_iter().map(|f| {
                            let href = format!("/d/{}", f.id);
                            // #152: keep the real href (new-tab / middle-click /
                            // hover preview keep working) but intercept a plain
                            // left-click for client-side nav — no full reload.
                            let nav_href = href.clone();
                            let label = f.title.clone();
                            let ctx_id = f.id.clone();
                            let btn_id = f.id.clone();
                            view! {
                                <a class="sidebar-item sidebar-favorite-item" href=href title=f.title
                                    on:click=move |ev: web_sys::MouseEvent| {
                                        if ev.button() == 0 && !ev.ctrl_key() && !ev.meta_key()
                                            && !ev.shift_key() && !ev.alt_key() {
                                            ev.prevent_default();
                                            close_drawer();
                                            crate::commands::nav_bridge::go(&nav_href);
                                        }
                                    }
                                    on:contextmenu=move |ev: web_sys::MouseEvent| {
                                        ev.prevent_default();
                                        doc_menu.set(Some(DocMenuTarget {
                                            id: ctx_id.clone(),
                                            x: ev.client_x() as f64,
                                            y: ev.client_y() as f64,
                                        }));
                                    }
                                >
                                    <span class="sidebar-favorite-star">"\u{2605}"</span>
                                    <span class="sidebar-favorite-title">{label}</span>
                                    <button class="sidebar-doc-actions"
                                        aria-haspopup="menu"
                                        aria-label=crate::t!("sidebar-doc-actions-aria")
                                        on:click=move |ev: web_sys::MouseEvent| {
                                            ev.prevent_default();
                                            ev.stop_propagation();
                                            let (x, y) = button_anchor(&ev);
                                            doc_menu.set(Some(DocMenuTarget {
                                                id: btn_id.clone(), x, y,
                                            }));
                                        }
                                    >"\u{22EF}"</button>
                                </a>
                            }
                        }).collect::<Vec<_>>().into_any()
                    }
                }}
            </div>

            // #144: Collections — one section per named group, each listing
            // its docs. Hidden entirely when the user has no collections.
            {move || {
                let colls = collections.get();
                colls.into_iter().map(|c| {
                    view! {
                        <div class="sidebar-section" style:display=move || if collapsed.get() { "none" } else { "block" }>
                            <div class="sidebar-section-title">{c.name}</div>
                            {if c.items.is_empty() {
                                view! {
                                    <div class="sidebar-item sidebar-item-muted">
                                        {crate::t!("sidebar-empty-collection")}
                                    </div>
                                }.into_any()
                            } else {
                                c.items.into_iter().map(|d| {
                                    let href = format!("/d/{}", d.id);
                                    let nav_href = href.clone(); // #152: client-side left-click
                                    let label = d.title.clone();
                                    let ctx_id = d.id.clone();
                                    let btn_id = d.id.clone();
                                    view! {
                                        <a class="sidebar-item sidebar-favorite-item" href=href title=d.title
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                if ev.button() == 0 && !ev.ctrl_key() && !ev.meta_key()
                                                    && !ev.shift_key() && !ev.alt_key() {
                                                    ev.prevent_default();
                                                    close_drawer();
                                                    crate::commands::nav_bridge::go(&nav_href);
                                                }
                                            }
                                            on:contextmenu=move |ev: web_sys::MouseEvent| {
                                                ev.prevent_default();
                                                doc_menu.set(Some(DocMenuTarget {
                                                    id: ctx_id.clone(),
                                                    x: ev.client_x() as f64,
                                                    y: ev.client_y() as f64,
                                                }));
                                            }
                                        >
                                            <span class="sidebar-favorite-title">{label}</span>
                                            <button class="sidebar-doc-actions"
                                                aria-haspopup="menu"
                                                aria-label=crate::t!("sidebar-doc-actions-aria")
                                                on:click=move |ev: web_sys::MouseEvent| {
                                                    ev.prevent_default();
                                                    ev.stop_propagation();
                                                    let (x, y) = button_anchor(&ev);
                                                    doc_menu.set(Some(DocMenuTarget {
                                                        id: btn_id.clone(), x, y,
                                                    }));
                                                }
                                            >"\u{22EF}"</button>
                                        </a>
                                    }
                                }).collect::<Vec<_>>().into_any()
                            }}
                        </div>
                    }
                }).collect::<Vec<_>>()
            }}

            <div style:display=move || if collapsed.get() { "none" } else { "block" }>
                <ChatPanel />
            </div>

            <div
                style:display=move || if collapsed.get() { "none" } else { "block" }
                style="margin-top: auto; padding: 8px 16px 16px;"
            >
                // Theme + locale selectors moved out of the footer into
                // the /settings Appearance section (account-menu step 2).
                // The collapsed icon-strip keeps a quick 2-state theme
                // toggle; the full controls live in settings now.
                // Build stamp: Cargo version + git short SHA captured by
                // build.rs at compile time. Sits above the account menu so
                // ops can confirm at a glance whether a deploy actually
                // shipped — without round-tripping a server endpoint.
                <div class="sidebar-version">
                    <span class="sidebar-version-num">
                        {concat!("v", env!("CARGO_PKG_VERSION"))}
                    </span>
                    <span class="sidebar-version-hash">
                        {option_env!("GIT_HASH").unwrap_or("unknown")}
                    </span>
                </div>
                // Account menu (step 3): avatar-anchored hub for
                // identity, Profile/Settings links, and Sign out — it
                // replaces the standalone sign-out row that lived here.
                <AccountMenu />
            </div>

            // ── "+ New" menu (header) ──
            <ContextMenu
                visible=Signal::derive(move || new_menu.get().is_some())
                x=Signal::derive(move || new_menu.get().map(|(x, _)| x).unwrap_or_default())
                y=Signal::derive(move || new_menu.get().map(|(_, y)| y).unwrap_or_default())
                entries=new_menu_entries
                on_close=Callback::new(move |()| new_menu.set(None))
            />

            // ── Row-menu chrome: shared context menu + dialogs ──
            <ContextMenu
                visible=Signal::derive(move || doc_menu.get().is_some())
                x=Signal::derive(move || doc_menu.get().map(|t| t.x).unwrap_or_default())
                y=Signal::derive(move || doc_menu.get().map(|t| t.y).unwrap_or_default())
                entries=doc_menu_entries
                on_close=Callback::new(move |()| doc_menu.set(None))
            />
            <FolderPickerDialog
                visible=Signal::derive(move || move_target.get().is_some())
                title=crate::t!("document-move-folder-title")
                confirm_label=crate::t!("document-move-here")
                on_close=Callback::new(move |()| move_target.set(None))
                on_pick=Callback::new(move |folder_id: String| {
                    let Some(target) = move_target.get_untracked() else { return };
                    move_target.set(None);
                    leptos::task::spawn_local(async move {
                        match documents::bulk_move(vec![target.id], &folder_id).await {
                            Ok(_) => bump_all(),
                            Err(e) => {
                                web_sys::console::error_1(&format!("Move failed: {e}").into());
                            }
                        }
                    });
                })
            />
            <ConfirmDialog
                visible=Signal::derive(move || delete_target.get().is_some())
                title=crate::t!("document-trash-dialog-title")
                message=crate::t!("document-trash-dialog-message")
                confirm_label=crate::t!("document-trash-dialog-confirm")
                destructive=true
                on_cancel=Callback::new(move |_| delete_target.set(None))
                on_confirm=Callback::new(move |_| {
                    let Some(target) = delete_target.get_untracked() else { return };
                    delete_target.set(None);
                    leptos::task::spawn_local(async move {
                        if documents::delete_document(&target.id).await.is_ok() {
                            // Trashing the doc that's open right now: leave the
                            // page the same way the document page's own Delete
                            // does (full reload to a fresh home listing).
                            let viewing = web_sys::window()
                                .and_then(|w| w.location().pathname().ok())
                                .map(|p| p.contains(&target.id))
                                .unwrap_or(false);
                            if viewing {
                                if let Some(window) = web_sys::window() {
                                    let _ = window.location().set_href("/");
                                }
                            } else {
                                bump_all();
                            }
                        }
                    });
                })
            />
        </nav>
    }
}
