// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Shared menu primitive — one implementation of dropdown/context-menu
//! chrome for every menu surface (menu bar, editor and spreadsheet
//! context menus, sheet tabs, account menu).
//!
//! Surfaces describe their content as data — a `Vec<MenuEntry>`
//! (action / toggle / separator / submenu) built by a
//! `Callback<(), Vec<MenuEntry>>` — and render it through one of two
//! containers:
//!
//! - [`ContextMenu`]: fixed-position at a clicked point, clamped to
//!   the viewport (estimate from `menu_nav::estimate_menu_size`).
//! - [`AnchoredMenu`]: absolutely positioned under (or over, via a
//!   caller CSS class) a trigger the caller renders.
//!
//! Both provide: transparent backdrop close, Escape close, full
//! keyboard navigation (arrows / Home / End / Enter / Space, with
//! submenu enter/exit that respects RTL), ARIA menu roles, and
//! submenus that open on hover *or* click (so touch and keyboard
//! reach them — the old per-surface CSS `:hover` fly-outs could not).
//!
//! Keyboard events are intercepted by a document-level **capture**
//! phase listener installed only while the menu is open and removed
//! symmetrically (no leaked closures). Capture matters: the editor's
//! contenteditable and the spreadsheet wrapper have their own keydown
//! handlers, and DOM focus deliberately never moves into the menu
//! (`preserve_focus` additionally suppresses `mousedown` default so
//! the editor selection survives clicks) — the highlight is virtual,
//! driven by `menu_nav::MenuNavState`.

use leptos::children::ViewFn;
use leptos::prelude::*;
use ogrenotes_frontend::menu_nav as core;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::dom_position::{clamp_to_viewport_with_size, viewport_size};

/// One row of a menu. Build with the associated constructors and
/// chain the `with_*` modifiers.
#[derive(Clone)]
pub enum MenuEntry {
    Action {
        label: String,
        icon: Option<String>,
        shortcut: Option<String>,
        disabled: bool,
        danger: bool,
        on_activate: Callback<()>,
    },
    Toggle {
        label: String,
        checked: Signal<bool>,
        on_activate: Callback<()>,
    },
    Separator,
    Submenu {
        label: String,
        entries: Vec<MenuEntry>,
    },
}

impl MenuEntry {
    pub fn action(label: impl Into<String>, f: impl Fn() + Send + Sync + 'static) -> Self {
        MenuEntry::Action {
            label: label.into(),
            icon: None,
            shortcut: None,
            disabled: false,
            danger: false,
            on_activate: Callback::new(move |()| f()),
        }
    }

    pub fn toggle(
        label: impl Into<String>,
        checked: impl Into<Signal<bool>>,
        f: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        MenuEntry::Toggle {
            label: label.into(),
            checked: checked.into(),
            on_activate: Callback::new(move |()| f()),
        }
    }

    pub fn submenu(label: impl Into<String>, entries: Vec<MenuEntry>) -> Self {
        MenuEntry::Submenu { label: label.into(), entries }
    }

    pub fn with_shortcut(mut self, s: impl Into<String>) -> Self {
        if let MenuEntry::Action { shortcut, .. } = &mut self {
            *shortcut = Some(s.into());
        }
        self
    }

    pub fn with_icon(mut self, i: impl Into<String>) -> Self {
        if let MenuEntry::Action { icon, .. } = &mut self {
            *icon = Some(i.into());
        }
        self
    }

    pub fn disabled_when(mut self, d: bool) -> Self {
        if let MenuEntry::Action { disabled, .. } = &mut self {
            *disabled = d;
        }
        self
    }

    pub fn danger(mut self) -> Self {
        if let MenuEntry::Action { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }
}

/// Structural shadow for the pure navigation core.
fn nav_shape(entries: &[MenuEntry]) -> Vec<core::NavNode> {
    entries
        .iter()
        .map(|e| match e {
            MenuEntry::Action { disabled, .. } => core::NavNode::Leaf { disabled: *disabled },
            MenuEntry::Toggle { .. } => core::NavNode::Leaf { disabled: false },
            MenuEntry::Separator => core::NavNode::Separator,
            MenuEntry::Submenu { entries, .. } => core::NavNode::Parent {
                // An empty submenu can't be entered — treat as disabled.
                disabled: entries.is_empty(),
                children: nav_shape(entries),
            },
        })
        .collect()
}

/// Entry the highlight currently points at (deepest open panel).
fn entry_at<'a>(
    entries: &'a [MenuEntry],
    path: &[usize],
    active: Option<usize>,
) -> Option<&'a MenuEntry> {
    let mut panel = entries;
    for &i in path {
        match panel.get(i) {
            Some(MenuEntry::Submenu { entries, .. }) => panel = entries,
            _ => return None,
        }
    }
    panel.get(active?)
}

/// Run an item's action and close the menu — deferred by one task so
/// the `<Show>` teardown never happens while the clicked button's own
/// event dispatch is still on the stack (the "closure invoked
/// recursively or after being dropped" panic class; see
/// `a11y::defer`).
fn activate_and_close(on_activate: Callback<()>, on_close: Callback<()>) {
    crate::a11y::defer(move || {
        on_activate.run(());
        on_close.run(());
    });
}

/// The submenu-enter arrow is logical: it points *into* the fly-out,
/// which opens on the inline-end side (left in RTL).
fn is_rtl() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        .map(|e| e.dir() == "rtl")
        .unwrap_or(false)
}

/// Shared keydown logic. `snapshot` holds the entries as last
/// rendered — the handler runs outside any reactive owner, so it must
/// not call the entries builder (which allocates `Callback`s).
fn handle_menu_key(
    ke: &web_sys::KeyboardEvent,
    snapshot: StoredValue<Vec<MenuEntry>>,
    state: RwSignal<core::MenuNavState>,
    on_close: Callback<()>,
    on_switch: Option<Callback<i32>>,
) {
    let key = ke.key();
    let (enter_key, exit_key) = if is_rtl() {
        ("ArrowLeft", "ArrowRight")
    } else {
        ("ArrowRight", "ArrowLeft")
    };
    let entries = snapshot.get_value();
    let shape = nav_shape(&entries);
    let st = state.get_untracked();
    let mut handled = true;
    match key.as_str() {
        "Escape" => crate::a11y::defer_close(on_close),
        "ArrowDown" => state.set(core::nav_move(&shape, &st, 1)),
        "ArrowUp" => state.set(core::nav_move(&shape, &st, -1)),
        "Home" => state.set(core::nav_home(&shape, &st)),
        "End" => state.set(core::nav_end(&shape, &st)),
        "Enter" | " " => match entry_at(&entries, &st.path, st.active) {
            Some(MenuEntry::Action { disabled: false, on_activate, .. })
            | Some(MenuEntry::Toggle { on_activate, .. }) => {
                activate_and_close(*on_activate, on_close);
            }
            Some(MenuEntry::Submenu { .. }) => state.set(core::nav_enter_submenu(&shape, &st)),
            _ => handled = false,
        },
        k if k == enter_key => {
            let next = core::nav_enter_submenu(&shape, &st);
            if next != st {
                state.set(next);
            } else if let Some(switch) = on_switch {
                switch.run(1);
            } else {
                handled = false;
            }
        }
        k if k == exit_key => {
            if !st.path.is_empty() {
                state.set(core::nav_exit_submenu(&st));
            } else if let Some(switch) = on_switch {
                switch.run(-1);
            } else {
                handled = false;
            }
        }
        _ => handled = false,
    }
    if handled {
        // Capture phase: this both cancels the browser default (caret
        // moves, page scroll on Space) and stops the event before the
        // editor / spreadsheet keydown handlers underneath can react.
        ke.prevent_default();
        ke.stop_propagation();
    }
}

/// Install a document-level capture-phase keydown listener while
/// `enabled` is true; remove it when it flips false and on component
/// cleanup. The `web_sys::Closure` is `!Send`, so it lives in a
/// `LocalStorage` store — the Effect itself only captures `Send`
/// handles.
type KeydownClosure = Closure<dyn Fn(web_sys::Event)>;

fn install_menu_keys(
    enabled: Signal<bool>,
    handler: impl Fn(&web_sys::KeyboardEvent) + Clone + Send + Sync + 'static,
) {
    let registered: StoredValue<Option<KeydownClosure>, LocalStorage> =
        StoredValue::new_local(None);
    let remove = move || {
        if let Some(closure) = registered.try_update_value(|r| r.take()).flatten()
            && let Some(doc) = web_sys::window().and_then(|w| w.document())
        {
            let _ = doc.remove_event_listener_with_callback_and_bool(
                "keydown",
                closure.as_ref().unchecked_ref(),
                true,
            );
        }
    };
    Effect::new(move |_| {
        if enabled.get() {
            if registered.with_value(|r| r.is_some()) {
                return;
            }
            let handler = handler.clone();
            let closure = Closure::wrap(Box::new(move |ev: web_sys::Event| {
                if let Some(ke) = ev.dyn_ref::<web_sys::KeyboardEvent>() {
                    handler(ke);
                }
            }) as Box<dyn Fn(web_sys::Event)>);
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                let _ = doc.add_event_listener_with_callback_and_bool(
                    "keydown",
                    closure.as_ref().unchecked_ref(),
                    true,
                );
            }
            registered.set_value(Some(closure));
        } else {
            remove();
        }
    });
    on_cleanup(remove);
}

/// Recursively render one panel of entries. `depth` indexes into the
/// nav state's submenu path; submenu fly-outs render only while their
/// parent is on the open path (hover, click, or ArrowRight all put it
/// there).
fn render_panel(
    entries: Vec<MenuEntry>,
    depth: usize,
    state: RwSignal<core::MenuNavState>,
    on_close: Callback<()>,
) -> AnyView {
    entries
        .into_iter()
        .enumerate()
        .map(|(i, entry)| match entry {
            MenuEntry::Separator => view! { <div class="ui-menu-sep"></div> }.into_any(),
            MenuEntry::Action { label, icon, shortcut, disabled, danger, on_activate } => {
                view! {
                    <button
                        class="ui-menu-item"
                        class:active=move || core::is_highlighted(&state.get(), depth, i)
                        class:ui-menu-item-danger=danger
                        role="menuitem"
                        disabled=disabled
                        on:mouseenter=move |_| state.update(|s| {
                            s.path.truncate(depth);
                            s.active = Some(i);
                        })
                        on:click=move |_| {
                            if !disabled {
                                activate_and_close(on_activate, on_close);
                            }
                        }
                    >
                        {icon.map(|ic| view! { <span class="ui-menu-icon">{ic}</span> })}
                        <span class="ui-menu-label">{label}</span>
                        {shortcut.map(|s| view! { <span class="ui-menu-shortcut">{s}</span> })}
                    </button>
                }
                .into_any()
            }
            MenuEntry::Toggle { label, checked, on_activate } => {
                view! {
                    <button
                        class="ui-menu-item"
                        class:active=move || core::is_highlighted(&state.get(), depth, i)
                        role="menuitemcheckbox"
                        aria-checked=move || checked.get().to_string()
                        on:mouseenter=move |_| state.update(|s| {
                            s.path.truncate(depth);
                            s.active = Some(i);
                        })
                        on:click=move |_| activate_and_close(on_activate, on_close)
                    >
                        <span class="ui-menu-check">
                            {move || if checked.get() { "\u{2713}" } else { "" }}
                        </span>
                        <span class="ui-menu-label">{label}</span>
                    </button>
                }
                .into_any()
            }
            MenuEntry::Submenu { label, entries } => {
                let open_here = move || state.get().path.get(depth) == Some(&i);
                let empty = entries.is_empty();
                view! {
                    <div class="ui-menu-subwrap">
                        <button
                            class="ui-menu-item"
                            class:active=move || core::is_highlighted(&state.get(), depth, i)
                            role="menuitem"
                            aria-haspopup="menu"
                            aria-expanded=move || open_here().to_string()
                            disabled=empty
                            on:mouseenter=move |_| state.update(|s| {
                                s.path.truncate(depth);
                                s.path.push(i);
                                s.active = None;
                            })
                            // Click OPENS the fly-out (idempotent, never
                            // toggles shut): pointer clicks arrive right
                            // after our own mouseenter already opened it,
                            // and touch taps fire a synthetic mouseenter
                            // first too — a toggle here would snap the
                            // fly-out closed on the very gesture meant to
                            // open it. Closing is Escape / ArrowLeft /
                            // hovering a sibling / the backdrop.
                            on:click=move |e: web_sys::MouseEvent| {
                                e.stop_propagation();
                                state.update(|s| {
                                    s.path.truncate(depth);
                                    s.path.push(i);
                                });
                            }
                        >
                            <span class="ui-menu-label">{label}</span>
                            <span class="ui-menu-arrow">"\u{25B8}"</span>
                        </button>
                        {move || {
                            open_here().then(|| {
                                view! {
                                    <div class="ui-menu ui-menu-sub" role="menu">
                                        {render_panel(entries.clone(), depth + 1, state, on_close)}
                                    </div>
                                }
                            })
                        }}
                    </div>
                }
                .into_any()
            }
        })
        .collect_view()
        .into_any()
}

/// Common open/close machinery: nav-state reset on open + keyboard
/// installation. Returns the nav state and the entries snapshot the
/// key handler reads.
fn menu_machinery(
    open: Signal<bool>,
    on_close: Callback<()>,
    on_switch: Option<Callback<i32>>,
) -> (RwSignal<core::MenuNavState>, StoredValue<Vec<MenuEntry>>) {
    let state = RwSignal::new(core::MenuNavState::default());
    let snapshot: StoredValue<Vec<MenuEntry>> = StoredValue::new(Vec::new());
    Effect::new(move |_| {
        if open.get() {
            state.set(core::MenuNavState::default());
        }
    });
    install_menu_keys(open, move |ke| {
        handle_menu_key(ke, snapshot, state, on_close, on_switch);
    });
    (state, snapshot)
}

/// Right-click style menu: fixed position at (`x`, `y`), clamped so it
/// stays inside the viewport, transparent backdrop, Escape + keyboard
/// navigation. Set `preserve_focus` when the surface underneath (the
/// editor's contenteditable) must keep DOM focus and its selection.
#[component]
pub fn ContextMenu(
    #[prop(into)] visible: Signal<bool>,
    #[prop(into)] x: Signal<f64>,
    #[prop(into)] y: Signal<f64>,
    entries: Callback<(), Vec<MenuEntry>>,
    on_close: Callback<()>,
    #[prop(optional)] preserve_focus: bool,
) -> impl IntoView {
    let (state, snapshot) = menu_machinery(visible, on_close, None);
    view! {
        <Show when=move || visible.get()>
            <div
                class="ui-menu-backdrop"
                on:mousedown=move |e: web_sys::MouseEvent| {
                    if preserve_focus {
                        e.prevent_default();
                    }
                    crate::a11y::defer_close(on_close);
                }
                on:contextmenu=move |e: web_sys::MouseEvent| {
                    // Right-clicking the backdrop dismisses; re-opening
                    // at the new position is the owning surface's job.
                    // Synchronous (not deferred) so a surface handler
                    // that re-opens the menu on the same bubbled event
                    // isn't undone by a later deferred close.
                    e.prevent_default();
                    on_close.run(());
                }
            ></div>
            {move || {
                let entries = entries.run(());
                snapshot.set_value(entries.clone());
                let shape = nav_shape(&entries);
                let (w, h) = core::estimate_menu_size(&shape);
                let (vw, vh) = viewport_size();
                let (cx, cy) = clamp_to_viewport_with_size(x.get(), y.get(), w, h, vw, vh);
                // Near the right edge a menu + fly-out column won't
                // fit — flip submenus to open leftward (cascades to
                // every nesting level via CSS).
                let flip = x.get() + 2.0 * core::MENU_EST_WIDTH > vw;
                view! {
                    <div
                        class="ui-menu"
                        class:ui-menu-flip-x=flip
                        role="menu"
                        style:left=format!("{cx}px")
                        style:top=format!("{cy}px")
                        on:mousedown=move |e: web_sys::MouseEvent| {
                            if preserve_focus {
                                e.prevent_default();
                            }
                        }
                        on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                    >
                        {render_panel(entries, 0, state, on_close)}
                    </div>
                }
            }}
        </Show>
    }
}

/// Dropdown anchored to a trigger the caller renders: the caller wraps
/// both in a `position: relative` container and this panel positions
/// itself with CSS (`.ui-menu-anchored`, adjustable via `class`).
/// `header` renders non-interactive content (e.g. the account menu's
/// identity block) above the items. `on_switch` is the menu-bar hook:
/// ArrowRight past a leaf item / ArrowLeft at the root panel call it
/// with +1 / −1 so the bar can rotate between its menus.
#[component]
pub fn AnchoredMenu(
    #[prop(into)] open: Signal<bool>,
    entries: Callback<(), Vec<MenuEntry>>,
    on_close: Callback<()>,
    #[prop(optional)] class: &'static str,
    #[prop(optional)] header: Option<ViewFn>,
    #[prop(optional)] preserve_focus: bool,
    #[prop(optional)] on_switch: Option<Callback<i32>>,
) -> impl IntoView {
    let (state, snapshot) = menu_machinery(open, on_close, on_switch);
    view! {
        <Show when=move || open.get()>
            <div
                class="ui-menu-backdrop"
                on:mousedown=move |e: web_sys::MouseEvent| {
                    if preserve_focus {
                        e.prevent_default();
                    }
                    crate::a11y::defer_close(on_close);
                }
            ></div>
            <div
                class=format!("ui-menu ui-menu-anchored {class}")
                role="menu"
                on:mousedown=move |e: web_sys::MouseEvent| {
                    if preserve_focus {
                        e.prevent_default();
                    }
                }
                on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
            >
                {header.clone().map(|h| h.run())}
                {move || {
                    let entries = entries.run(());
                    snapshot.set_value(entries.clone());
                    render_panel(entries, 0, state, on_close)
                }}
            </div>
        </Show>
    }
}
