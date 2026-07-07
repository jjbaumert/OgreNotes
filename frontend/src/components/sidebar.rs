// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use crate::api::client;
use crate::theme::{self, ExplicitTheme};
use super::account_menu::AccountMenu;
use super::chat_panel::ChatPanel;

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
    let (collapsed, set_collapsed) = signal(read_collapsed());

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

    let toggle = move |_| set_collapsed.update(|c| *c = !*c);

    // Persist every collapse/expand (the header toggle, and the Chats
    // icon which expands to reveal the ChatPanel) so the choice
    // survives reloads.
    Effect::new(move |_| write_collapsed(collapsed.get()));

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
            aria-label=crate::t!("sidebar-aria-main-nav")
        >
            <div class="sidebar-header">
                <span class="sidebar-logo">
                    {move || if collapsed.get() { "O" } else { "OgreNotes" }}
                </span>
                <button
                    class="toolbar-btn"
                    on:click=toggle
                    style="color: white;"
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
                    // Click expands the sidebar so the ChatPanel
                    // becomes visible. Re-collapsing is via the ←
                    // button in the header.
                    on:click=move |_| set_collapsed.set(false)
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
                        on:click=move |_| cb.run(())
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
                        on:click=move |_| cb.run(())
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
                        on:click=move |_| cb.run(())
                        style="cursor: pointer;"
                    >
                        <span>{format!("\u{1F4CB} {}", crate::t!("sidebar-templates"))}</span>
                    </div>
                })}
            </div>

            <div class="sidebar-section" style:display=move || if collapsed.get() { "none" } else { "block" }>
                <div class="sidebar-section-title">{crate::t!("sidebar-section-recent")}</div>
                <div class="sidebar-item sidebar-item-muted">
                    {crate::t!("sidebar-empty-recent")}
                </div>
            </div>

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
                            view! {
                                <a class="sidebar-item sidebar-favorite-item" href=href title=f.title
                                    on:click=move |ev: web_sys::MouseEvent| {
                                        if ev.button() == 0 && !ev.ctrl_key() && !ev.meta_key()
                                            && !ev.shift_key() && !ev.alt_key() {
                                            ev.prevent_default();
                                            crate::commands::nav_bridge::go(&nav_href);
                                        }
                                    }
                                >
                                    <span class="sidebar-favorite-star">"\u{2605}"</span>
                                    <span class="sidebar-favorite-title">{label}</span>
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
                                    view! {
                                        <a class="sidebar-item sidebar-favorite-item" href=href title=d.title
                                            on:click=move |ev: web_sys::MouseEvent| {
                                                if ev.button() == 0 && !ev.ctrl_key() && !ev.meta_key()
                                                    && !ev.shift_key() && !ev.alt_key() {
                                                    ev.prevent_default();
                                                    crate::commands::nav_bridge::go(&nav_href);
                                                }
                                            }
                                        >
                                            <span class="sidebar-favorite-title">{label}</span>
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
        </nav>
    }
}
