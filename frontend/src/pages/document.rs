// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;

use crate::api::client;
use crate::api::documents;
use crate::collab::ws_client::CollabClient;
use crate::collab::ws_client::RemoteCursor;
use crate::components::at_menu::{AtMenu, AtMenuItem, AtMenuItemKind};
use crate::components::block_menu::BlockMenu;
use crate::a11y;
use crate::components::comment_highlights::{AddCommentBubble, CommentHighlights, InlineThreadInfo};
use crate::components::comment_popup::CommentPopup;
use crate::components::confirm_dialog::ConfirmDialog;
use crate::components::conversation_pane::ConversationPane;
use crate::components::duplicate_dialog::DuplicateDialog;
use crate::components::find_replace_bar::FindReplaceBar;
use crate::components::folder_picker::FolderPickerDialog;
use crate::components::selection_toolbar::{SelectionToolbar, SelectionCommand};
use crate::components::cursor_overlay::CursorOverlay;
use crate::components::document_details::DocumentDetailsDialog;
use crate::components::editor_gutter::EditorGutterOverlay;
use crate::components::history_viewer::HistoryViewer;
use crate::components::menu::AnchoredMenu;
use crate::components::menu_bar::{DocAction, MenuBar, document_menu_entries};
use crate::components::document_outline::DocumentOutline;
use crate::components::editor_component::{EditorComponent, EditorProps};
use crate::components::notification_bell::NotificationBell;
use crate::components::search_dialog::SearchDialog;
use crate::components::share_dialog::ShareDialog;
use crate::components::spreadsheet_view::SpreadsheetView;
use crate::components::sync_indicator::{poll_sync_state, SyncIndicator, SyncState};
use crate::components::toolbar::{Toolbar, ToolbarCommand};
use crate::editor::state::EditorState;

/// The closed set of formats reachable from the Document → Export
/// menu. Used to forge a compile-time-exhaustive mapping from menu
/// arm to (wire-format token, file extension) — passing both as raw
/// `&str` invites the bug where someone adds a new format and the
/// extension drifts (e.g. `"markdown"` vs `"md"` is already not the
/// same string today).
enum ExportFormat {
    Html,
    Markdown,
    Csv,
    Xlsx,
}

impl ExportFormat {
    /// Wire-format token used in the `/export/{format}` URL.
    fn wire(&self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "markdown",
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }

    /// File extension suggested to the browser via `<a download>`.
    fn ext(&self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Markdown => "md",
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }
}

/// Spawn an authenticated export of the current doc and trigger a
/// download of the result. Lives outside the component so the four
/// `DocAction::Export*` arms collapse to one line each — and so the
/// route stays bearer-only (a fresh-tab `window.open` can't attach
/// the access token, which is why this used to 401).
fn spawn_export(doc_id: String, title: String, fmt: ExportFormat) {
    if doc_id.is_empty() { return; }
    let wire = fmt.wire();
    let ext = fmt.ext();
    leptos::task::spawn_local(async move {
        match documents::export_document(&doc_id, wire).await {
            Ok(bytes) => {
                // #119: Markdown is text people paste into notes/PRs/issues,
                // so it goes to the clipboard instead of a downloaded file.
                // The other formats (html/csv/xlsx) are binary-ish artifacts
                // and still download.
                if matches!(fmt, ExportFormat::Markdown) {
                    copy_text_to_clipboard(String::from_utf8_lossy(&bytes).into_owned());
                } else {
                    let filename = format!("{}.{ext}", sanitize_filename(&title));
                    trigger_download(&filename, &bytes);
                }
            }
            Err(e) => {
                web_sys::console::error_1(
                    &format!("export {wire} failed: {e}").into(),
                );
            }
        }
    });
}

/// Write `text` to the OS clipboard via `navigator.clipboard.writeText`.
/// Reflect-based to avoid a hard dependency on the web-sys `Clipboard`
/// binding (not universally available across targeted browsers); all
/// error paths are swallowed. (#119)
fn copy_text_to_clipboard(text: String) {
    use wasm_bindgen::JsCast;
    let Some(window) = web_sys::window() else { return };
    let nav = window.navigator();
    let Ok(clip) = js_sys::Reflect::get(&nav, &"clipboard".into()) else { return };
    let Ok(write) = js_sys::Reflect::get(&clip, &"writeText".into()) else { return };
    let Ok(write_fn) = write.dyn_into::<js_sys::Function>() else { return };
    let _ = write_fn.call1(&clip, &text.into());
}

/// Replace anything outside `[A-Za-z0-9 _-]` with `_`, trim, and fall
/// back to `"document"` so the saved file always has a usable name.
fn sanitize_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let trimmed = cleaned.trim().trim_matches('_').trim();
    if trimmed.is_empty() { "document".to_string() } else { trimmed.to_string() }
}

/// Build a Blob from `bytes`, point a hidden `<a download>` at the
/// resulting object URL, click it, then revoke the URL. This is the
/// browser-side equivalent of `Content-Disposition: attachment` —
/// the route can't send that header here because we're fetching
/// (not navigating), so the client triggers the save itself.
fn trigger_download(filename: &str, bytes: &[u8]) {
    let array = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let blob = match web_sys::Blob::new_with_u8_array_sequence(&parts) {
        Ok(b) => b,
        Err(_) => return,
    };
    let url = match web_sys::Url::create_object_url_with_blob(&blob) {
        Ok(u) => u,
        Err(_) => return,
    };
    if let Some(document) = web_sys::window().and_then(|w| w.document()) {
        if let Ok(anchor) = document
            .create_element("a")
            .and_then(|el| el.dyn_into::<web_sys::HtmlAnchorElement>().map_err(|_| wasm_bindgen::JsValue::NULL))
        {
            anchor.set_href(&url);
            anchor.set_download(filename);
            // Disambiguate from the Leptos trait method also named
            // `style` that shadows web-sys's HtmlElement::style here.
            web_sys::HtmlElement::style(&anchor)
                .set_property("display", "none")
                .ok();
            if let Some(body) = document.body() {
                body.append_child(&anchor).ok();
                anchor.click();
                body.remove_child(&anchor).ok();
            }
        }
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

/// Build a `[data-block-id="…"]` CSS selector with the value escaped, so a
/// block id containing `"` or `\` can't break out of the selector string
/// and throw a `DOMException` — which `query_selector`'s `Ok`-guarded
/// callers swallow, silently no-op'ing the scroll/popup-positioning path.
/// Block ids are server-generated today; this keeps the lookup robust
/// regardless (#5).
fn block_id_selector(block_id: &str) -> String {
    let escaped = block_id.replace('\\', "\\\\").replace('"', "\\\"");
    format!("[data-block-id=\"{escaped}\"]")
}

// #139: localStorage keys for the editor view-option preferences.
// `pub(crate)` so `AppShell` can seed the shared `ShellCtx` line-number /
// page-break signals once on mount (those flags now live in the shell, so
// the persistent sidebar's `.app-layout` carries the toggle classes).
pub(crate) const PREF_LINE_NUMBERS: &str = "pref:line_numbers";
pub(crate) const PREF_PAGE_BREAKS: &str = "pref:page_breaks";

/// Read a boolean view-option preference from localStorage ("1" = on).
pub(crate) fn load_bool_pref(key: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(key).ok().flatten())
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Persist a boolean view-option preference to localStorage.
pub(crate) fn save_bool_pref(key: &str, val: bool) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(key, if val { "1" } else { "0" });
    }
}

/// #145: request the browser Fullscreen API on the app root. Best-effort — a
/// denial/absence is swallowed so Expand still falls back to chrome-hiding.
fn request_app_fullscreen() {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.query_selector(".app-layout").ok().flatten())
    {
        let _ = el.request_fullscreen();
    }
}

/// #145: leave browser full-screen if we're currently in it.
fn exit_app_fullscreen() {
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        if doc.fullscreen_element().is_some() {
            let _ = doc.exit_fullscreen();
        }
    }
}

/// #110: lifecycle of the viewer's "request edit access" click — drives the
/// button label/disabled state so a viewer gets clear feedback without a
/// page reload.
#[derive(Clone, Copy, PartialEq)]
enum RequestAccessState {
    Idle,
    Sending,
    Sent,
    Failed,
}

#[component]
pub fn DocumentPage() -> impl IntoView {
    if !client::is_authenticated() {
        let navigate = use_navigate();
        navigate("/login", Default::default());
        return view! { <div>{crate::t!("common-redirecting-login")}</div> }.into_any();
    }

    // #152: shared shell state. Several signals that used to be local now
    // live here so the persistent sidebar (in `AppShell`) and its
    // `.app-layout` wrapper survive navigation. Below, the moved signals are
    // aliased to `ctx` fields under their original read/write names, so the
    // existing call sites are unchanged (an `RwSignal` answers both `.get()`
    // and `.set()`/`.update()`).
    let ctx = use_context::<crate::components::app_shell::ShellCtx>()
        .expect("ShellCtx provided by AppShell");

    let params = use_params_map();
    let doc_id = move || params.read().get("id").unwrap_or_default();
    // `?comment=<thread_id>` deep-link target from a notification (#50).
    let query = use_query_map();

    let (title, set_title) = signal("Loading...".to_string());
    let (error, set_error) = signal::<Option<String>>(None);
    let (current_id, set_current_id) = signal(String::new());
    let (is_trashed, set_is_trashed) = signal(false);
    // #110: viewer-facing "request edit access" affordance. The backend
    // decides eligibility (view-only user on a request-enabled View link);
    // the frontend just reflects it and tracks the click's outcome.
    let (can_request_access, set_can_request_access) = signal(false);
    let (request_access_state, set_request_access_state) = signal(RequestAccessState::Idle);
    // #111: the caller's write authority. A View-only user gets a read-only
    // editor even though the WS now delivers them live updates. Defaults to
    // editable until the doc metadata loads.
    let (can_edit, set_can_edit) = signal(true);
    // #140: per-document edit lock. `is_locked` freezes the editor read-only
    // for everyone (server-enforced); `can_manage_lock` is true only for the
    // owner, gating the Format-menu "Lock Edits" toggle.
    let (is_locked, set_is_locked) = signal(false);
    let (can_manage_lock, set_can_manage_lock) = signal(false);
    // #142: whether this doc is currently marked as a template. Drives the
    // Document-menu item label (Mark vs Unmark Template).
    let (is_template, set_is_template) = signal(false);
    // #144: whether this doc is starred by the current user, and a tick the
    // sidebar Favorites section watches so a star/unstar updates it live.
    let (is_favorite, set_is_favorite) = signal(false);
    let set_favorites_dirty = ctx.favorites_dirty; // #152: now in shell ctx
    // #144: the star dropdown (Added to Favorites / Collections / Remove). Open
    // state, the current doc's collection memberships (loaded when it opens),
    // and a tick the sidebar Collections area watches for live refresh.
    let (fav_menu_open, set_fav_menu_open) = signal(false);
    let doc_collections: RwSignal<Vec<documents::CollectionMembership>> = RwSignal::new(Vec::new());
    let set_collections_dirty = ctx.collections_dirty; // #152: now in shell ctx
    // #147: in-app Find & Replace bar visibility.
    let (find_visible, set_find_visible) = signal(false);
    let (confirm_delete_visible, set_confirm_delete_visible) = signal(false);
    let (confirm_purge_visible, set_confirm_purge_visible) = signal(false);
    let (restore_picker_visible, set_restore_picker_visible) = signal(false);
    // #146: folder picker for "Move to Folder" (distinct from the trash
    // restore picker above).
    let (move_picker_visible, set_move_picker_visible) = signal(false);
    // Phone-only `⋯` document-actions menu in the header (the menu bar
    // is hidden at the ≤640px breakpoint).
    let mobile_doc_menu_open = RwSignal::new(false);
    // #149: multi-folder membership — the folders this doc is in (loaded from
    // GET /documents/:id/folders) and the picker for adding another.
    let doc_folders: RwSignal<Vec<documents::DocFolder>> = RwSignal::new(Vec::new());
    let (add_folder_picker_visible, set_add_folder_picker_visible) = signal(false);
    // #146 follow-up: the Duplicate dialog (name + destination + share warning).
    let (duplicate_dialog_visible, set_duplicate_dialog_visible) = signal(false);
    let (initial_content, set_initial_content) = signal::<Option<Vec<u8>>>(None);
    let (content_loaded, set_content_loaded) = signal(false);
    // Bumped by the activity tracker to re-trigger the connect Effect
    // after a 30-min idle disconnect. The Effect's same-doc branch then
    // reuses the existing CollabClient (preserving its yrs::Doc and
    // CRDT clock) and just opens a fresh WebSocket.
    let (reconnect_trigger, set_reconnect_trigger) = signal(0u32);
    let (doc_type, set_doc_type) = signal("document".to_string());
    // #141: document metadata for the Details panel (microsecond timestamps,
    // matching the rest of the app's `format_date`/`format_relative`).
    let (created_at, set_created_at) = signal(0i64);
    let (updated_at, set_updated_at) = signal(0i64);
    let (details_visible, set_details_visible) = signal(false);
    let (editor_state, set_editor_state) = signal::<Option<EditorState>>(None);
    let (toolbar_command, set_toolbar_command) = signal::<Option<ToolbarCommand>>(None);
    // Remote document state — set by the collab callback, consumed by EditorComponent
    // to update the contenteditable DOM when a collaborator makes changes.
    let (remote_state, set_remote_state) = signal::<Option<EditorState>>(None);
    let (outline_visible, set_outline_visible) = signal(false);
    let (share_visible, set_share_visible) = signal(false);
    // #152: search/ask visibility live in the shell ctx (the sidebar Search/
    // Ask entries set them); the dialogs themselves stay mounted here.
    let search_visible = ctx.search_open;
    let set_search_visible = ctx.search_open;
    // Phase 6 M-6.2 piece B: Ask dialog visibility, same shape as
    // the search dialog. Sidebar entry opens it.
    let ask_visible = ctx.ask_open;
    let set_ask_visible = ctx.ask_open;
    // Piece C: install the ask_bridge so the Global-scoped
    // `ask.open` palette command can flip the dialog open. Cleared
    // on unmount.
    Effect::new(move |_| {
        crate::commands::ask_bridge::set_ask_open(Some(Callback::new(move |()| {
            set_ask_visible.set(true);
        })));
    });
    on_cleanup(|| {
        crate::commands::ask_bridge::set_ask_open(None);
    });
    // M-P4 piece B: Ctrl+K and Ctrl+Shift+P share the same search
    // dialog with different starting modes. The keydown handler
    // writes here before flipping `search_visible`; the dialog's
    // open-Effect reads it once to decide whether to pre-fill the
    // action `>` prefix.
    let (palette_initial_mode, set_palette_initial_mode) =
        signal(crate::components::search_dialog::PaletteMode::Search);
    let (folder_id, set_folder_id) = signal::<Option<String>>(None);
    let (conversation_visible, set_conversation_visible) = signal(false);
    // Mobile-only: opens the sidebar as a slide-in drawer. #152: the drawer
    // state + backdrop live in the shell now; the header hamburger toggles it.
    let set_mobile_sidebar_open = ctx.drawer_open;
    // Mobile-only: editor content zoom factor driven by two-finger pinch
    // gestures on the editor surface. Applied via CSS `zoom` on the wrapper
    // so headings/lists/code all scale proportionally without re-flowing
    // around scaled-down line heights.
    let (editor_zoom, set_editor_zoom) = signal(1.0f64);
    let (comments_visible, set_comments_visible) = signal(true);
    // #99: remote collaborators' cursors can obscure text; View → Show
    // Cursors toggles their rendering (default on).
    let (cursors_visible, set_cursors_visible) = signal(true);
    // #100: focus mode hides the sidebar + menu bar to maximize editing
    // space. Toggled from View, Ctrl+Shift+F, or the header toggle button.
    // #152: in the shell ctx so the persistent sidebar's `.app-layout` gets
    // the `.focus-mode` class.
    let focus_mode = ctx.focus_mode;
    let set_focus_mode = ctx.focus_mode;
    // #145: Expand — a genuinely clutter-free, full-screen editor. A superset
    // of focus mode: it also requests the browser Fullscreen API and drops the
    // doc header, leaving only the editor. Falls back to chrome-hiding if
    // Fullscreen is denied/unavailable. #152: in the shell ctx (drives the
    // `.expanded` class on the persistent app-layout).
    let expanded = ctx.expanded;
    let set_expanded = ctx.expanded;
    // #139: editor view options — block line numbers + page-break guides.
    // Persisted as per-browser preferences so they survive doc navigation.
    // #152: seeded once in `AppShell` and held in the shell ctx so the
    // toggle classes ride the persistent app-layout; toggles still persist
    // via `save_bool_pref` below.
    let line_numbers_visible = ctx.show_line_numbers;
    let set_line_numbers_visible = ctx.show_line_numbers;
    let page_breaks_visible = ctx.show_page_breaks;
    let set_page_breaks_visible = ctx.show_page_breaks;

    // #145: enter/leave Expand. Entering also turns on focus mode so the two
    // stay coherent; leaving drops fullscreen but keeps focus mode (the user
    // can exit that separately). State is also synced from `fullscreenchange`
    // below, so pressing Esc out of browser fullscreen collapses Expand too.
    let toggle_expand = Callback::new(move |()| {
        let now = !expanded.get_untracked();
        set_expanded.set(now);
        if now {
            set_focus_mode.set(true);
            request_app_fullscreen();
        } else {
            exit_app_fullscreen();
        }
    });

    // Sync Expand state with the browser: a `fullscreenchange` to "no
    // fullscreen element" (e.g. the user pressed Esc) collapses Expand.
    {
        let cb: Closure<dyn Fn(web_sys::Event)> = Closure::wrap(Box::new(move |_| {
            let in_fs = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.fullscreen_element())
                .is_some();
            if !in_fs {
                set_expanded.set(false);
            }
        }));
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc.add_event_listener_with_callback(
                "fullscreenchange",
                cb.as_ref().unchecked_ref(),
            );
        }
        // Leaked deliberately: Leptos `on_cleanup` requires Send + Sync, which
        // wasm-bindgen `Closure`s are not. Matches the activity-listener
        // pattern below — a constant per-mount cost, not per event.
        cb.forget();
    }

    // Inline comment state
    let (pending_block_id, set_pending_block_id) = signal::<Option<String>>(None);
    let (pending_anchor_start, set_pending_anchor_start) = signal::<Option<u32>>(None);
    let (pending_anchor_end, set_pending_anchor_end) = signal::<Option<u32>>(None);
    let (filter_thread_id, set_filter_thread_id) = signal::<Option<String>>(None);
    let (inline_threads, set_inline_threads) = signal::<Vec<InlineThreadInfo>>(Vec::new());
    // Cell-anchored comment threads, derived from the same list_threads
    // fetch as `inline_threads`. Handed to SpreadsheetView for the
    // thread-aware cell hover preview + click-to-open marker.
    let (cell_threads, set_cell_threads) =
        signal::<Vec<crate::components::spreadsheet_view::CellThreadInfo>>(Vec::new());
    // Deep-link target handed to SpreadsheetView when a notification opens
    // a cell comment (#50): the view switches sheet, selects + scrolls the
    // cell, and opens its popup.
    let (focus_cell, set_focus_cell) =
        signal::<Option<crate::components::spreadsheet_view::CellFocus>>(None);
    let (comment_count, set_comment_count) = signal(0usize);
    // Bumped each time a peer's REST write changes a comment thread on this
    // doc (see CollabClient::set_on_comment_event below). The thread-load
    // Effect watches this so a remote comment shows up live for everyone in
    // the room without a manual refresh.
    let (comments_dirty, set_comments_dirty) = signal(0u32);
    // Pulses with the foreign doc id whose CRDT advanced via the WS
    // multi-doc subscribe channel (M-S2 step 10 phase 3). The
    // spreadsheet view watches this signal and invalidates its
    // foreign-doc cache for the named id, triggering a refetch on
    // the next recompute.
    let (foreign_doc_invalidate, set_foreign_doc_invalidate) = signal::<Option<String>>(None);
    // Phase 2a Option A — LiveApp rejection toast. The WS client
    // fires this with the server-side diagnostic when the pre-apply
    // gate refuses an update; the toast tells the user their local
    // change didn't reach the server and to refresh once they're
    // done. Cleared on click or after ~10s.
    let (liveapp_error, set_liveapp_error) = signal::<Option<String>>(None);
    // Handle for the auto-dismiss timer so a reject arriving inside
    // the 10s window cancels the stale timer instead of letting it
    // fire later and clear the newer message. Same shape as
    // `pending_send_timer` / `typing_timer` elsewhere in this file.
    let liveapp_toast_timer: std::rc::Rc<std::cell::RefCell<Option<gloo_timers::callback::Timeout>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    // Comment popup state (for inline comment threads shown near highlighted text)
    let (popup_thread_id, set_popup_thread_id) = signal::<Option<String>>(None);
    let (popup_left, set_popup_left) = signal(0.0f64);
    let (popup_top, set_popup_top) = signal(0.0f64);
    let (popup_is_new, set_popup_is_new) = signal(false);
    let (popup_block_id, set_popup_block_id) = signal::<Option<String>>(None);
    let (popup_anchor_start, set_popup_anchor_start) = signal::<Option<u32>>(None);
    let (popup_anchor_end, set_popup_anchor_end) = signal::<Option<u32>>(None);
    // Block menu state
    let (block_menu_visible, set_block_menu_visible) = signal(false);
    let (block_menu_top, set_block_menu_top) = signal(0.0f64);
    // Remote cursor presence
    let (remote_cursors, set_remote_cursors) = signal::<Vec<RemoteCursor>>(Vec::new());
    // Local "user is typing into a comment thread" state. Set by the
    // conversation pane / comment popup composers; broadcast through
    // awareness so peers can render "X is typing…" in the matching thread.
    let (typing_thread_id, set_typing_thread_id) = signal::<Option<String>>(None);
    // Scroll tick — incremented on editor-container scroll to force overlay re-render
    let (scroll_tick, set_scroll_tick) = signal(0u32);
    // History viewer
    let (history_visible, set_history_visible) = signal(false);
    // At menu state
    let (at_menu_visible, set_at_menu_visible) = signal(false);
    let (at_menu_query, set_at_menu_query) = signal(String::new());
    let (at_menu_left, set_at_menu_left) = signal(0.0f64);
    let (at_menu_top, set_at_menu_top) = signal(0.0f64);
    // #148: highlighted flat index for keyboard nav + results
    // list so the doc-level keydown handler can route ↑/↓/Enter
    // without re-fetching.
    let (at_menu_highlighted, set_at_menu_highlighted) = signal(0usize);
    let (at_menu_results, set_at_menu_results) =
        signal::<Vec<AtMenuItem>>(Vec::new());
    // #148 v2 slice 2 — parallel slash-command menu. Same
    // component, same on_select shape, but the producer skips
    // People/Documents fetch (it's a command menu, not a
    // mention menu). Signals mirror the @-menu one-for-one.
    let (slash_menu_visible, set_slash_menu_visible) = signal(false);
    let (slash_menu_query, set_slash_menu_query) = signal(String::new());
    let (slash_menu_left, set_slash_menu_left) = signal(0.0f64);
    let (slash_menu_top, set_slash_menu_top) = signal(0.0f64);
    let (slash_menu_highlighted, set_slash_menu_highlighted) = signal(0usize);
    let (slash_menu_results, set_slash_menu_results) =
        signal::<Vec<AtMenuItem>>(Vec::new());
    // Track whether WS is connected (Arc for Send+Sync in Callback).
    // The on_change Callback just checks this flag; the actual WS send
    // is done in the editor_component dispatch, not in the debounced save.
    let ws_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // CollabClient lives in Rc (not Send) — only used from Effects and editor dispatch
    let collab_client: std::rc::Rc<std::cell::RefCell<Option<CollabClient>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    // Track which doc_id the current CollabClient is for, so we can reuse it on reconnect.
    let collab_doc_id: std::rc::Rc<std::cell::RefCell<String>> =
        std::rc::Rc::new(std::cell::RefCell::new(String::new()));

    // #152: disconnect the WebSocket when this page unmounts on a *client-side*
    // route change. The CollabClient is pinned alive by the deliberately
    // `mem::forget`'d activity/visibility listeners (they hold a clone of this
    // `Rc`), so its `Drop` — which disconnects — never runs on a route change.
    // Without this, navigating away from a document in-app would leak the
    // socket + heartbeat: a lingering connection the server still sees, so
    // collaborators keep rendering this client's ghost cursor/presence. The
    // `Rc` rides in a `SendWrapper` so the `Send + Sync` `on_cleanup` closure
    // can own it (safe: the wasm runtime is single-threaded). `disconnect()`
    // closes the socket even though the struct itself stays leaked.
    let collab_for_unmount =
        send_wrapper::SendWrapper::new(std::rc::Rc::clone(&collab_client));
    on_cleanup(move || {
        if let Some(client) = collab_for_unmount.borrow().as_ref() {
            client.disconnect();
        }
    });

    // Sync-indicator state (M-P3 piece B). The polling loop in
    // poll_sync_state reads the CollabClient's connection_state +
    // pending_count every 500 ms and writes here; SyncIndicator
    // subscribes to render the badge. The Rc clone is cheap and
    // !Send is fine because spawn_local doesn't require Send.
    let (sync_state, set_sync_state) = signal(SyncState::Saved);
    poll_sync_state(std::rc::Rc::clone(&collab_client), set_sync_state);

    // #149: (re)load the doc's folder memberships for the chips bar. `Copy`
    // (captures only Copy signals), so it's reused after add/remove too.
    let reload_doc_folders = move || {
        let id = current_id.get_untracked();
        if id.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            match documents::list_doc_folders(&id).await {
                Ok(folders) => doc_folders.set(folders),
                // Don't swallow: if this is the revert after a failed remove,
                // a silent failure leaves the chips diverged from the server.
                Err(e) => web_sys::console::warn_1(
                    &format!("folder list refresh failed — chips may be stale: {e}").into(),
                ),
            }
        });
    };
    // Auto-refresh whenever the active doc changes.
    Effect::new(move |_| {
        current_id.track();
        reload_doc_folders();
    });

    // Reactively load document when the ID changes
    Effect::new(move |_| {
        let id = doc_id();
        if id.is_empty() || id == current_id.get_untracked() {
            return;
        }
        set_current_id.set(id.clone());
        set_title.set("Loading...".to_string());
        set_error.set(None);
        set_content_loaded.set(false);
        // #149: clear stale folder chips immediately so we don't show the
        // previous doc's folders during the new doc's load round-trip.
        doc_folders.set(Vec::new());

        leptos::task::spawn_local(async move {
            match documents::get_document(&id).await {
                Ok(doc) => {
                    set_title.set(doc.title);
                    set_folder_id.set(doc.folder_id);
                    set_doc_type.set(doc.doc_type);
                    set_is_trashed.set(doc.is_deleted);
                    set_can_request_access.set(doc.can_request_access);
                    set_request_access_state.set(RequestAccessState::Idle);
                    set_can_edit.set(doc.can_edit);
                    set_is_locked.set(doc.locked);
                    set_can_manage_lock.set(doc.can_manage);
                    set_is_template.set(doc.is_template);
                    set_is_favorite.set(doc.is_favorite);
                    set_created_at.set(doc.created_at);
                    set_updated_at.set(doc.updated_at);
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }

            match documents::get_content(&id).await {
                Ok(bytes) => {
                    set_initial_content.set(Some(bytes));
                    set_content_loaded.set(true);
                }
                Err(e) => {
                    set_initial_content.set(None);
                    set_content_loaded.set(true);
                    web_sys::console::warn_1(
                        &format!("Failed to load content: {e}").into(),
                    );
                }
            }
        });
    });

    // Load comment threads for highlights as soon as the document loads,
    // independent of whether the conversation pane is open. Also re-runs
    // whenever `comments_dirty` is bumped — that's how a peer's
    // REST-side comment write becomes visible here without a refresh.
    {
        let load_doc_id = current_id.clone();
        Effect::new(move |_| {
            if !content_loaded.get() {
                return;
            }
            // Track `comments_dirty` so this Effect re-runs on broadcast.
            let _ = comments_dirty.get();
            let id = load_doc_id.get();
            if id.is_empty() {
                return;
            }
            leptos::task::spawn_local(async move {
                match crate::api::comments::list_threads(&id).await {
                    Ok(resp) => {
                        let inline: Vec<InlineThreadInfo> = resp
                            .threads
                            .iter()
                            .filter(|t| t.thread_type == "inline" && t.block_id.is_some())
                            .map(|t| InlineThreadInfo {
                                thread_id: t.thread_id.clone(),
                                block_id: t.block_id.clone().unwrap(),
                                anchor_start: t.anchor_start,
                                anchor_end: t.anchor_end,
                            })
                            .collect();
                        set_comment_count.set(inline.len());
                        set_inline_threads.set(inline);
                        // Cell-anchored threads (block_id `cell-…`) for the
                        // spreadsheet preview/marker. Same fetch — no extra
                        // request. reply_count excludes the opening message.
                        let cells: Vec<crate::components::spreadsheet_view::CellThreadInfo> = resp
                            .threads
                            .iter()
                            .filter(|t| {
                                t.block_id.as_deref().is_some_and(|b| b.starts_with("cell-"))
                            })
                            .map(|t| crate::components::spreadsheet_view::CellThreadInfo {
                                block_id: t.block_id.clone().unwrap(),
                                thread_id: t.thread_id.clone(),
                                first_message: t.first_message.clone(),
                                reply_count: t.message_count.saturating_sub(1),
                            })
                            .collect();
                        set_cell_threads.set(cells);
                    }
                    Err(e) => {
                        web_sys::console::warn_1(
                            &format!("Failed to load comment threads: {e}").into(),
                        );
                    }
                }
            });
        });
    }

    // Deep-link: open (and center) the comment named by `?comment=<tid>`
    // once the document's threads have loaded. A cell comment is handed to
    // SpreadsheetView (sheet-switch + scroll + popup); a document comment
    // centers its block and opens the popup here. The handled-cell guards
    // against reopening on every reactive re-run. (#50)
    {
        let handled: std::rc::Rc<std::cell::RefCell<Option<String>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        Effect::new(move |_| {
            let Some(tid) = query.read().get("comment") else { return };
            if !content_loaded.get() {
                return;
            }
            let cells = cell_threads.get();
            let inlines = inline_threads.get();
            if handled.borrow().as_deref() == Some(tid.as_str()) {
                return;
            }

            // Cell comment → SpreadsheetView handles sheet-switch + centering.
            if let Some(ct) = cells.iter().find(|t| t.thread_id == tid) {
                if let Some((sheet, row, col)) =
                    crate::components::spreadsheet_view::parse_cell_block_id(&ct.block_id)
                {
                    *handled.borrow_mut() = Some(tid.clone());
                    set_focus_cell.set(Some(
                        crate::components::spreadsheet_view::CellFocus {
                            sheet,
                            row,
                            col,
                            thread_id: tid,
                        },
                    ));
                    return;
                }
            }

            // Document inline/block comment → center the block + open popup.
            if let Some(it) = inlines.iter().find(|t| t.thread_id == tid) {
                *handled.borrow_mut() = Some(tid.clone());
                let block_id = it.block_id.clone();
                let tid2 = tid.clone();
                // Defer so the editor DOM is mounted before we query for it.
                gloo_timers::callback::Timeout::new(80, move || {
                    if let Some(d) = web_sys::window().and_then(|w| w.document()) {
                        if let Ok(Some(el)) =
                            d.query_selector(&block_id_selector(&block_id))
                        {
                            el.scroll_into_view_with_bool(true);
                            let rect = crate::components::dom_position::element_viewport_rect(&el);
                            let (pl, pt) = crate::components::dom_position::place_left_margin(
                                &rect, 420.0, 12.0,
                            );
                            set_popup_left.set(pl);
                            set_popup_top.set(pt);
                        }
                    }
                    set_popup_thread_id.set(Some(tid2));
                })
                .forget();
            }
        });
    }

    // Flag to suppress the send_update Effect when state changes come from remote updates.
    // Prevents feedback loops (remote → set_editor_state → send_update → echo back).
    let remote_update_flag = std::rc::Rc::new(std::cell::Cell::new(false));

    // Connect WebSocket for real-time collaboration after content loads.
    let collab_for_ws = std::rc::Rc::clone(&collab_client);
    let collab_doc_id_for_ws = std::rc::Rc::clone(&collab_doc_id);
    let ws_doc_id = current_id.clone();
    let ws_connected_for_ws = std::sync::Arc::clone(&ws_connected);
    let remote_flag_for_ws = std::rc::Rc::clone(&remote_update_flag);
    Effect::new(move |_| {
        // Re-runs on either: content loaded for a new doc, OR the
        // activity tracker bumped reconnect_trigger after the WS dropped.
        let _trigger = reconnect_trigger.get();
        if !content_loaded.get() {
            return;
        }
        let id = ws_doc_id.get_untracked();
        if id.is_empty() {
            return;
        }

        let is_same_doc = *collab_doc_id_for_ws.borrow() == id;

        if is_same_doc {
            // Same document — reuse the existing CollabClient and its persistent
            // yrs::Doc (preserves client_id and CRDT clock). Just disconnect the
            // old WebSocket; the reconnect code below will open a fresh one.
            if let Some(ref client) = *collab_for_ws.borrow() {
                client.disconnect();
            }
        } else {
            // Different document — drop the old client entirely and create a new one.
            if let Some(old_client) = collab_for_ws.borrow_mut().take() {
                old_client.disconnect();
            }

            let initial_bytes = initial_content.get_untracked();
            let client = CollabClient::new(
                id.clone(),
                initial_bytes.as_deref(),
            );

            // Set up remote update callback.
            // Preserves the local cursor/selection position when remote changes arrive.
            // Sets `remote_state` which the EditorComponent watches to update the DOM,
            // and also sets `remote_flag` so the send Effect skips this change.
            let editor_state_for_ws = editor_state.clone();
            let set_remote_state_ws = set_remote_state.clone();
            let remote_flag_for_ws = remote_flag_for_ws.clone();
            let set_inline_threads_ws = set_inline_threads;
            client.set_on_remote_update(Box::new(move |doc| {
                // Carry the local selection through the doc swap using
                // block-relative coordinates — otherwise absolute indices
                // drift by one per character the remote inserts above the
                // selection, and the user's highlight visibly slides.
                let prior = editor_state_for_ws.get_untracked();
                let mut state = crate::editor::state::EditorState::create_default(doc);
                if let Some(ref prior) = prior {
                    state.selection = crate::editor::state::remap_selection_across_doc_swap(
                        &prior.doc,
                        &prior.selection,
                        &state.doc,
                    );
                    // Optimistically carry comment anchors through the swap
                    // too, so a peer's edit doesn't visibly drag this
                    // client's highlights in the window before the editing
                    // client's persisted anchors are refetched. No
                    // persistence here — the peer that made the edit owns
                    // the anchor write (mirrors the undo-stack remap the
                    // EditorComponent does on the same swap).
                    let map = crate::editor::transform::step_map_for_doc_swap(
                        &prior.doc,
                        &state.doc,
                    );
                    set_inline_threads_ws.update(|threads| {
                        crate::components::comment_highlights::remap_thread_anchors(
                            threads,
                            std::slice::from_ref(&map),
                            &prior.doc,
                            &state.doc,
                        );
                    });
                }
                remote_flag_for_ws.set(true);
                set_remote_state_ws.set(Some(state));
            }));

            // #92: let the client fold keystrokes still inside the send
            // debounce into the ydoc before each remote update is applied
            // — otherwise the swap above rebuilds the view from a ydoc
            // that never saw them and they vanish (worst at mount, when
            // the initial SyncStep2 races the first keystrokes).
            client.set_local_doc_provider(Box::new(move || {
                editor_state
                    .try_get_untracked()
                    .flatten()
                    .map(|s| s.doc)
            }));

            // Set up awareness callback for remote cursor presence.
            client.set_on_awareness_update(Box::new(move |cursors| {
                set_remote_cursors.set(cursors);
            }));

            // Comments live in the thread DB rather than the CRDT, so the
            // server emits a side-channel notification when a peer's REST
            // write changes a thread on this doc. Bump `comments_dirty`
            // and the inline-threads Effect re-runs to repaint highlights.
            client.set_on_comment_event(Box::new(move |_payload| {
                set_comments_dirty.update(|n| *n = n.wrapping_add(1));
            }));

            client.set_on_foreign_doc_update(Box::new(move |foreign_id| {
                set_foreign_doc_invalidate.set(Some(foreign_id));
            }));

            let liveapp_toast_timer_cb = liveapp_toast_timer.clone();
            client.set_on_liveapp_error(Box::new(move |diagnostic| {
                set_liveapp_error.set(Some(diagnostic));
                // Store the handle so a second reject inside the
                // window drops (via gloo-timers' Drop impl calling
                // clearTimeout) the stale timer — otherwise the
                // first reject's 10s timeout would fire and clear
                // the newer message early. Same pattern as
                // `pending_send_timer` / `typing_timer` in this
                // file.
                *liveapp_toast_timer_cb.borrow_mut() = Some(
                    gloo_timers::callback::Timeout::new(10_000, move || {
                        set_liveapp_error.set(None);
                    })
                );
            }));

            *collab_for_ws.borrow_mut() = Some(client);
            *collab_doc_id_for_ws.borrow_mut() = id.clone();
        }

        // Request a ws-token and connect (shared by both same-doc reconnect and new-doc).
        let collab_for_connect = std::rc::Rc::clone(&collab_for_ws);
        let ws_connected_for_connect = std::sync::Arc::clone(&ws_connected_for_ws);
        leptos::task::spawn_local(async move {
            match documents::request_ws_token(&id).await {
                Ok(resp) => {
                    let origin = web_sys::window()
                        .and_then(|w| w.location().origin().ok())
                        .unwrap_or_default();
                    let ws_origin = if origin.starts_with("https") {
                        origin.replacen("https", "wss", 1)
                    } else {
                        let api_origin = origin.replacen("http", "ws", 1);
                        if api_origin.contains(":8080") {
                            api_origin.replace(":8080", ":3000")
                        } else {
                            api_origin
                        }
                    };
                    // #46: no `?token=` on the URL — the token rides in the
                    // WebSocket subprotocol (see ws_client::connect), keeping
                    // it out of browser history and HTTP access logs.
                    let ws_url = format!("{ws_origin}/api/v1/documents/{id}/ws");

                    if let Some(ref client) = *collab_for_connect.borrow() {
                        client.connect(&ws_url, &resp.token, std::sync::Arc::clone(&ws_connected_for_connect));
                        crate::editor::debug::log("collab", "WebSocket connecting", &[
                            ("doc_id", &id),
                        ]);
                    }
                }
                Err(e) => {
                    crate::editor::debug::warn("collab", &format!("ws-token request failed: {e}"));
                }
            }
        });
    });

    // ── Activity tracker ──
    //
    // The CollabClient's heartbeat sends Pings only while the user has been
    // active within IDLE_DISCONNECT_MS (30 min). Past that, the connection
    // drops; coming back to the page must trigger a fresh connect. These
    // listeners record activity on every plausible "user is here" signal
    // and bump `reconnect_trigger` when the activity comes in on a closed
    // session — that signal is dependency of the connect Effect above and
    // re-runs the same-doc reconnect branch, including the SyncStep1/2
    // catch-up.
    let collab_for_activity = std::rc::Rc::clone(&collab_client);
    let on_activity: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || {
        if let Some(ref client) = *collab_for_activity.borrow() {
            client.record_activity();
            if !client.is_connected() {
                // try_update tolerates a disposed signal — the listeners
                // registered below outlive the page mount (we can't use
                // on_cleanup; see comment further down), so a stray
                // mousemove / keydown after the user has navigated away
                // would otherwise panic in dev builds. Returns None
                // silently in that case.
                let _ = set_reconnect_trigger.try_update(|n| *n += 1);
            }
        }
    });

    // Mousemove fires at display-refresh rate; throttle to 1Hz so we
    // don't beat the activity callback to death for no signal value.
    let last_mouse_fire = std::rc::Rc::new(std::cell::Cell::new(0.0_f64));
    // Keydown is throttled too (0.5Hz): while the WS is down, `on_activity`
    // bumps the reconnect trigger, and fast typing (~6ms/key) would fire a
    // reconnect on every keystroke — saturating the per-IP WS-upgrade rate
    // limit and, under load, corrupting the contenteditable caret.
    let last_key_fire = std::rc::Rc::new(std::cell::Cell::new(0.0_f64));

    if let (Some(window), Some(document)) = (
        web_sys::window(),
        web_sys::window().and_then(|w| w.document()),
    ) {
        // Closures registered on Window/Document outlive the page if not
        // explicitly removed — collect them so on_cleanup can detach.
        let registrations: std::rc::Rc<
            std::cell::RefCell<Vec<(web_sys::EventTarget, &'static str, Closure<dyn Fn(web_sys::Event)>)>>,
        > = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

        // visibilitychange — fires both on hide and show; we only care
        // about the page becoming visible again.
        {
            let on_activity = std::rc::Rc::clone(&on_activity);
            let document_for_check = document.clone();
            let cb: Closure<dyn Fn(web_sys::Event)> = Closure::wrap(Box::new(move |_| {
                if !document_for_check.hidden() {
                    on_activity();
                }
            }));
            let target: web_sys::EventTarget = document.clone().into();
            target
                .add_event_listener_with_callback("visibilitychange", cb.as_ref().unchecked_ref())
                .ok();
            registrations.borrow_mut().push((target, "visibilitychange", cb));
        }

        // keydown — covers typing in the editor and any keyboard input.
        // Throttled to 0.5Hz (see `last_key_fire`): a 2s floor still
        // reconnects a disconnected typist within 2s of their first
        // keystroke, without firing a reconnect on every character.
        {
            let on_activity = std::rc::Rc::clone(&on_activity);
            let last_fire = std::rc::Rc::clone(&last_key_fire);
            let cb: Closure<dyn Fn(web_sys::Event)> = Closure::wrap(Box::new(move |_| {
                let now = web_sys::window()
                    .and_then(|w| w.performance())
                    .map(|p| p.now())
                    .unwrap_or(0.0);
                if now - last_fire.get() < 2000.0 {
                    return;
                }
                last_fire.set(now);
                on_activity();
            }));
            let target: web_sys::EventTarget = document.clone().into();
            target
                .add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref())
                .ok();
            registrations.borrow_mut().push((target, "keydown", cb));
        }

        // mousemove — throttled to 1Hz to keep the cost trivial.
        {
            let on_activity = std::rc::Rc::clone(&on_activity);
            let last_fire = std::rc::Rc::clone(&last_mouse_fire);
            let cb: Closure<dyn Fn(web_sys::Event)> = Closure::wrap(Box::new(move |_| {
                let now = web_sys::window()
                    .and_then(|w| w.performance())
                    .map(|p| p.now())
                    .unwrap_or(0.0);
                if now - last_fire.get() < 1000.0 {
                    return;
                }
                last_fire.set(now);
                on_activity();
            }));
            let target: web_sys::EventTarget = document.clone().into();
            target
                .add_event_listener_with_callback("mousemove", cb.as_ref().unchecked_ref())
                .ok();
            registrations.borrow_mut().push((target, "mousemove", cb));
        }

        // window focus — fires when the user returns from another window
        // or app, in addition to (or independent of) visibilitychange.
        {
            let on_activity = std::rc::Rc::clone(&on_activity);
            let cb: Closure<dyn Fn(web_sys::Event)> =
                Closure::wrap(Box::new(move |_| on_activity()));
            let target: web_sys::EventTarget = window.clone().into();
            target
                .add_event_listener_with_callback("focus", cb.as_ref().unchecked_ref())
                .ok();
            registrations.borrow_mut().push((target, "focus", cb));
        }

        // NOTE: cleanup-on-route-change isn't wired here. Leptos's
        // `on_cleanup` requires `Send + Sync`, which wasm-bindgen
        // `Closure`s and `EventTarget` are not (the wasm runtime is
        // single-threaded but the trait bounds don't know that). The
        // four listeners survive route changes, holding an `Rc` to the
        // closure-captured page state. They're no-op-safe — they read
        // through `collab_for_activity` which is the same `Rc` the new
        // page mount uses — so the practical cost is just a constant
        // 4-listener overhead per session, not per route change.
        //
        // We `mem::forget` the registrations so the inner `Vec<…,
        // Closure>` is leaked deliberately. A previous version bound
        // `let _retain = registrations` here, which scoped the
        // ownership only to this `if let` block — so the Closures
        // dropped the instant the block closed, while JS still held
        // the function pointers from `add_event_listener_*`. Every
        // subsequent `visibilitychange` / `keydown` / `mousemove` /
        // `focus` event then panicked in the wasm trampoline with
        // "closure invoked recursively or after being dropped",
        // filling the console (mousemove fires constantly).
        std::mem::forget(registrations);
    }

    // Send incremental yrs updates over WebSocket when connected.
    // Debounced: rapid keystrokes are batched into fewer WS sends.
    // Uses gloo_timers::callback::Timeout instead of spawn_local + TimeoutFuture
    // to avoid re-entrant polling of the wasm-bindgen task runner's RefCell
    // (Leptos Effects use queueMicrotask which shares the same microtask queue).
    let collab_for_send = std::rc::Rc::clone(&collab_client);
    let (prev_doc_hash, set_prev_doc_hash) = signal(0u64);
    let pending_send_timer: std::rc::Rc<std::cell::RefCell<Option<gloo_timers::callback::Timeout>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let remote_flag_for_send = std::rc::Rc::clone(&remote_update_flag);
    Effect::new(move |_| {
        // #94: this Effect re-runs on every editor_state change
        // (every keystroke). The `.get()` reads are intentionally
        // tracking (so they re-subscribe); switching to try_* on
        // the untracked reads guards against the disposal race
        // documented on the on_state_change Callback above.
        let Some(state) = editor_state.get() else { return };

        // Skip remote-originated state changes to prevent feedback loops.
        if remote_flag_for_send.get() {
            remote_flag_for_send.set(false);
            // Still update the hash so the next local change is detected correctly.
            set_prev_doc_hash.set(state.doc.structural_hash());
            return;
        }

        let hash = state.doc.structural_hash();
        let Some(prev_hash) = prev_doc_hash.try_get_untracked() else { return };
        if hash == prev_hash {
            return;
        }
        set_prev_doc_hash.set(hash);

        // Debounce: set a timer. If another change arrives before the timeout,
        // the previous timer is dropped (cancelled) and a new one is set.
        let collab = collab_for_send.clone();
        let timer_ref = pending_send_timer.clone();
        *timer_ref.borrow_mut() = Some(gloo_timers::callback::Timeout::new(
            crate::collab::ws_client::WS_SEND_DEBOUNCE_MS,
            move || {
                if let Some(ref client) = *collab.borrow() {
                    // #92 — two deliberate choices here:
                    //
                    // 1. No `is_synced()` gate. `send_update` already
                    //    handles the pre-sync case correctly (folds into
                    //    the ydoc + buffers; the SyncStep2 handler
                    //    flushes). Gating here made keystrokes typed
                    //    before the initial sync invisible to the CRDT —
                    //    the swap then dropped them (the "first
                    //    keystrokes after mount" bug).
                    //
                    // 2. Read the CURRENT editor state at fire time, not
                    //    a snapshot captured at scheduling. If a remote
                    //    swap lands inside the debounce window, the
                    //    snapshot is stale, and diffing it against the
                    //    post-merge baseline would revert the peer's
                    //    content (child-list reconciliation reads
                    //    "missing" as "deleted").
                    let Some(current) = editor_state.try_get_untracked().flatten() else {
                        return;
                    };
                    client.send_update(&current.doc);
                }
            },
        ));
    });

    // Forward remote updates to editor_state for spreadsheet mode.
    // EditorComponent handles remote_state internally; SpreadsheetView reads editor_state,
    // so we bridge the two signals here.
    let remote_flag_for_spreadsheet = std::rc::Rc::clone(&remote_update_flag);
    Effect::new(move |_| {
        if doc_type.get() != "spreadsheet" { return; }
        let Some(state) = remote_state.get() else { return; };
        // Don't let the post-handshake remote callback for a still-empty
        // server ydoc (decoded as a bare-Paragraph fallback) clobber the
        // locally-initialized table — see remote_doc_degrades_spreadsheet.
        let current = editor_state.get_untracked();
        if remote_doc_degrades_spreadsheet(
            current.as_ref().map(|s| &s.doc),
            &state.doc,
        ) {
            return;
        }
        remote_flag_for_spreadsheet.set(true);
        set_editor_state.set(Some(state));
    });

    // Initialize the spreadsheet EditorState once content has loaded.
    // Must run as an Effect, not inside the view closure: writing a signal
    // during a reactive render stalls the closure before it returns the view,
    // leaving the page on "Loading document..." forever.
    let spreadsheet_initialized: std::rc::Rc<std::cell::RefCell<String>> =
        std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    Effect::new(move |_| {
        if !content_loaded.get() || doc_type.get() != "spreadsheet" {
            return;
        }
        let id = current_id.get_untracked();
        if id.is_empty() {
            return;
        }
        if *spreadsheet_initialized.borrow() == id {
            return; // already initialized for this doc
        }
        *spreadsheet_initialized.borrow_mut() = id;

        let content = initial_content.get_untracked();
        let doc = if let Some(ref bytes) = content {
            let decoded = crate::editor::yrs_bridge::ydoc_bytes_to_doc(bytes)
                .unwrap_or_else(|_| crate::editor::model::Node::empty_doc());
            if has_table(&decoded) {
                decoded
            } else {
                create_default_spreadsheet_doc()
            }
        } else {
            create_default_spreadsheet_doc()
        };
        set_editor_state.set(Some(EditorState::create_default(doc)));
    });

    // Send local cursor/selection position as awareness updates.
    let collab_for_awareness = std::rc::Rc::clone(&collab_client);
    let (prev_sel_hash, set_prev_sel_hash) = signal(0u64);
    Effect::new(move |_| {
        // #94: per-keystroke Effect; use try_* on untracked reads.
        let Some(state) = editor_state.get() else { return };
        // Track typing state alongside selection so the effect re-fires
        // when the user starts/stops typing into a comment thread, even
        // when the cursor in the doc didn't move.
        let typing = typing_thread_id.get();
        // Quick change detection on selection + typing
        let sel_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            state.selection.from().hash(&mut h);
            state.selection.to().hash(&mut h);
            typing.hash(&mut h);
            h.finish()
        };
        let Some(prev_hash) = prev_sel_hash.try_get_untracked() else { return };
        if sel_hash == prev_hash {
            return;
        }
        set_prev_sel_hash.set(sel_hash);

        if let Some(ref client) = *collab_for_awareness.borrow() {
            if client.is_synced() {
                let auth = crate::api::client::get_auth();
                let user_id = auth.as_ref().map(|a| a.user_id.as_str()).unwrap_or("unknown");
                let name = auth.as_ref().map(|a| a.name.as_str()).unwrap_or("Anonymous");
                let color_idx = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    user_id.hash(&mut h);
                    (h.finish() % 12) as u8
                };
                let from = state.selection.from();
                let to = state.selection.to();

                // Convert absolute positions to block-relative (block_id, char_offset).
                // These are portable across clients regardless of DOM structure differences.
                use crate::editor::state::find_block_at;
                let pos_to_block = |pos: usize| -> Option<(String, u32)> {
                    let block = find_block_at(&state.doc, pos)?;
                    let block_id = match block.attrs.get("blockId") {
                        Some(id) => id.clone(),
                        None => {
                            // Blocks without blockId (e.g., from old documents)
                            // can't be addressed by the remote client.
                            return None;
                        }
                    };
                    let char_offset = (pos.saturating_sub(block.content_start)) as u32;
                    Some((block_id, char_offset))
                };

                let cursor_block = pos_to_block(to);
                let typing_ref = typing.as_deref();
                if from == to {
                    client.send_awareness(
                        user_id, name, color_idx,
                        cursor_block.as_ref().map(|(b, o)| (b.as_str(), *o)),
                        None, None,
                        typing_ref,
                    );
                } else {
                    let anchor_block = pos_to_block(from);
                    let head_block = pos_to_block(to);
                    client.send_awareness(
                        user_id, name, color_idx,
                        cursor_block.as_ref().map(|(b, o)| (b.as_str(), *o)),
                        anchor_block.as_ref().map(|(b, o)| (b.as_str(), *o)),
                        head_block.as_ref().map(|(b, o)| (b.as_str(), *o)),
                        typing_ref,
                    );
                }
            }
        }
    });

    // Auto-save with REST fallback.
    // When WebSocket is connected, skip REST save (the editor_component
    // dispatch sends incremental updates via WS directly).
    // When disconnected, use debounced REST PUT as before.
    let save_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let save_doc_id = current_id.clone();
    let ws_connected_for_save = std::sync::Arc::clone(&ws_connected);
    // Track title changes separately so we can save the title even when WS handles content.
    let title_save_doc_id = save_doc_id.clone();
    let (prev_title, set_prev_title) = signal(String::new());
    let pending_title_timer: std::rc::Rc<std::cell::RefCell<Option<gloo_timers::callback::Timeout>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    Effect::new(move |_| {
        // #94: title-update Effect; use try_* on untracked reads.
        let current_title = title.get();
        let Some(prev) = prev_title.try_get_untracked() else { return };
        if current_title == prev || current_title == "Loading..." {
            return;
        }
        set_prev_title.set(current_title.clone());
        let Some(id) = title_save_doc_id.try_get_untracked() else { return };
        if id.is_empty() {
            return;
        }
        // Reflect the new title in the URL slug. For non-spreadsheet
        // docs `on_state_change` already does this off the doc's first
        // block; spreadsheets keep their title separately, so the slug
        // update has to ride this title-change Effect or the URL stays
        // stuck on whatever slug was loaded.
        {
            let slug = slugify(&current_title);
            let new_url = format!("/d/{id}/{slug}");
            replace_url_if_changed(&new_url);
        }
        let timer_ref = pending_title_timer.clone();
        *timer_ref.borrow_mut() = Some(gloo_timers::callback::Timeout::new(1000, move || {
            leptos::task::spawn_local(async move {
                if let Err(e) = documents::update_document_title(&id, &current_title).await {
                    web_sys::console::error_1(&format!("Title save failed: {e}").into());
                }
            });
        }));
    });

    // Bridges from SpreadsheetView's foreign-doc fetch loop to the
    // CollabClient's WS multi-doc subscribe channel. Signals (rather
    // than Callbacks) because the CollabClient is held in
    // `Rc<RefCell<…>>` which isn't `Send + Sync`, and Leptos
    // Callbacks require those bounds. The page-level Effects below
    // observe the queues and dispatch to the WS client.
    let (foreign_subscribe_queue, set_foreign_subscribe_queue) =
        signal::<Vec<String>>(Vec::new());
    let (foreign_unsubscribe_queue, set_foreign_unsubscribe_queue) =
        signal::<Vec<String>>(Vec::new());

    let on_subscribe_foreign = Callback::new(move |foreign_id: String| {
        set_foreign_subscribe_queue.update(|q| q.push(foreign_id));
    });
    let on_unsubscribe_foreign = Callback::new(move |foreign_id: String| {
        set_foreign_unsubscribe_queue.update(|q| q.push(foreign_id));
    });

    let collab_for_foreign_subscribe = std::rc::Rc::clone(&collab_client);
    Effect::new(move |_| {
        let queue = foreign_subscribe_queue.get();
        if queue.is_empty() { return; }
        set_foreign_subscribe_queue.set(Vec::new());
        if let Some(c) = collab_for_foreign_subscribe.borrow().as_ref() {
            for id in queue {
                c.subscribe_foreign_doc(&id);
            }
        }
    });

    // Spreadsheet active-cell presence: SpreadsheetView fires the
    // `on_cell_cursor` callback whenever the active cell or sheet
    // changes; we stash it in a signal and dispatch through the
    // collab client below. Done via a signal because the
    // Callback's Send + Sync bound rules out closing over the
    // non-Send Rc<RefCell<CollabClient>> directly.
    let (cell_cursor_pending, set_cell_cursor_pending) =
        signal::<Option<(String, usize, usize)>>(None);
    let on_cell_cursor = Callback::new(move |coords: (String, usize, usize)| {
        set_cell_cursor_pending.set(Some(coords));
    });
    let collab_for_cell_cursor = std::rc::Rc::clone(&collab_client);
    Effect::new(move |_| {
        let Some((sheet, r, c)) = cell_cursor_pending.get() else { return };
        set_cell_cursor_pending.set(None);
        let collab_ref = collab_for_cell_cursor.borrow();
        let Some(client) = collab_ref.as_ref() else { return };
        if !client.is_synced() { return; }
        let auth = crate::api::client::get_auth();
        let user_id = auth.as_ref().map(|a| a.user_id.as_str()).unwrap_or("unknown");
        let name = auth.as_ref().map(|a| a.name.as_str()).unwrap_or("Anonymous");
        let color_idx = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            user_id.hash(&mut h);
            (h.finish() % 12) as u8
        };
        let block_id = format!("ss:{}:c:{}:{}", sheet, r, c);
        client.send_awareness(
            user_id, name, color_idx,
            Some((block_id.as_str(), 0)),
            None, None,
            None,
        );
    });
    let collab_for_foreign_unsubscribe = std::rc::Rc::clone(&collab_client);
    Effect::new(move |_| {
        let queue = foreign_unsubscribe_queue.get();
        if queue.is_empty() { return; }
        set_foreign_unsubscribe_queue.set(Vec::new());
        if let Some(c) = collab_for_foreign_unsubscribe.borrow().as_ref() {
            for id in queue {
                c.unsubscribe_foreign_doc(&id);
            }
        }
    });

    // Handles for the spreadsheet save callback below — taken before
    // `on_change` moves the originals into its closure.
    let ws_connected_for_save_ss = std::sync::Arc::clone(&ws_connected);
    let save_doc_id_ss = save_doc_id.clone();
    let save_generation_ss = std::sync::Arc::clone(&save_generation);

    let on_change = Callback::new(move |bytes: Vec<u8>| {
        // Skip REST content save if WebSocket is handling persistence.
        // Title is saved separately via the Effect above.
        if ws_connected_for_save.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }

        let id = save_doc_id.get_untracked();
        if id.is_empty() {
            return;
        }
        let generation = save_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let gen_ref = std::sync::Arc::clone(&save_generation);

        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;
            if gen_ref.load(std::sync::atomic::Ordering::Relaxed) != generation {
                return;
            }

            let mut attempts = 0;
            loop {
                attempts += 1;
                match documents::put_content(&id, &bytes).await {
                    Ok(()) => break,
                    Err(crate::api::client::ApiClientError::Http(409, _)) if attempts < 3 => {
                        gloo_timers::future::TimeoutFuture::new(100).await;
                        continue;
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Auto-save failed: {e}").into(),
                        );
                        break;
                    }
                }
            }
        });
    });

    // Spreadsheet variant of the REST auto-save fallback (#121): the
    // spreadsheet fires a content-changed ping instead of pre-encoded
    // bytes, because the per-commit `doc_to_ydoc_bytes` encode was
    // O(doc) and discarded whenever the WebSocket was handling
    // persistence. Here the encode runs only when a save actually
    // flushes (WS down + debounce elapsed + still the latest change),
    // reading the freshest doc from `editor_state` at that moment.
    let on_change_ss = Callback::new(move |_: ()| {
        if ws_connected_for_save_ss.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let id = save_doc_id_ss.get_untracked();
        if id.is_empty() {
            return;
        }
        let generation =
            save_generation_ss.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let gen_ref = std::sync::Arc::clone(&save_generation_ss);

        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(500).await;
            if gen_ref.load(std::sync::atomic::Ordering::Relaxed) != generation {
                return;
            }
            // Encode at flush time. A newer state than the pinged one is
            // fine — autosave wants the latest content anyway, and the
            // generation guard above already collapsed bursts.
            let Some(state) = editor_state.try_get_untracked().flatten() else {
                return;
            };
            let bytes = crate::editor::yrs_bridge::doc_to_ydoc_bytes(&state.doc);

            let mut attempts = 0;
            loop {
                attempts += 1;
                match documents::put_content(&id, &bytes).await {
                    Ok(()) => break,
                    Err(crate::api::client::ApiClientError::Http(409, _)) if attempts < 3 => {
                        gloo_timers::future::TimeoutFuture::new(100).await;
                        continue;
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Auto-save failed: {e}").into(),
                        );
                        break;
                    }
                }
            }
        });
    });

    // Cache of the last first-block text used to derive the URL slug
    // and title. Guards the title/slug/URL computation from firing
    // on selection-only transactions (which trigger `on_state_change`
    // but leave the doc structure untouched) and from redundant
    // recomputation when the first block hasn't changed. Without
    // this guard, a doc whose first block is a Calendar (or any
    // non-text atom that renders lots of DOM) triggered a
    // recompute + `replace_url_if_changed` call on every mouse
    // move / arrow key / selection tick — enough traffic to trip
    // Firefox's History-API throttle and spin the browser CPU.
    // SendWrapper because Leptos Callback::new requires Send + Sync
    // and wasm-bindgen's Rc/RefCell aren't — same pattern the
    // calendar modal callback uses in editor_component.rs.
    let last_first_text: send_wrapper::SendWrapper<
        std::rc::Rc<std::cell::RefCell<Option<String>>>,
    > = send_wrapper::SendWrapper::new(std::rc::Rc::new(std::cell::RefCell::new(None)));
    let last_first_text_cb = last_first_text.clone();
    let on_state_change = Callback::new(move |state: EditorState| {
        // #94: this Callback is invoked from EditorView's dispatch on
        // every keystroke. Use try_* variants on every signal read so
        // an edit that lands after the parent scope has disposed (rare
        // but possible during fast nav-then-type) no-ops instead of
        // panicking. `set()` is already disposed-safe in Leptos 0.7
        // (it discards the try_update result), so writes don't need
        // explicit guards. None on a read means "scope gone; abort
        // the rest of this handler."
        set_editor_state.set(Some(state.clone()));

        // For documents, derive the title from the first block's text content.
        // Spreadsheets keep their explicit title (editable in the header).
        let Some(current_dt) = doc_type.try_get_untracked() else { return };
        if current_dt != "spreadsheet" {
            let first_text = state.doc.child(0).map(|n| n.text_content()).unwrap_or_default();
            // Short-circuit if the first block's text hasn't changed.
            // This covers all selection-only dispatches and any edit
            // deeper in the doc.
            if last_first_text_cb.borrow().as_deref() == Some(first_text.as_str()) {
                return;
            }
            *last_first_text_cb.borrow_mut() = Some(first_text.clone());

            let display_title = if first_text.trim().is_empty() {
                crate::t!("common-untitled")
            } else {
                first_text.clone()
            };
            set_title.set(display_title);

            let slug = slugify(&first_text);
            let Some(id) = current_id.try_get_untracked() else { return };
            if !id.is_empty() {
                let new_url = format!("/d/{id}/{slug}");
                // Only touch the History API when the URL actually
                // changes. `on_state_change` fires on every dispatch —
                // including remote CRDT frames — but the derived slug is
                // stable for most edits (and always stable for a doc
                // whose first block is a non-text block like a calendar).
                // Calling replaceState on every no-op tripped Firefox's
                // "Too many calls to Location or History APIs" throttle.
                replace_url_if_changed(&new_url);
            }
        }
    });

    // Outline navigation: scroll to heading position.
    // Finds the nth heading element in the editor DOM and scrolls it into view.
    let on_outline_navigate = Callback::new(move |pos: usize| {
        if let Some(state) = editor_state.get_untracked() {
            let entries = crate::components::document_outline::extract_outline(&state.doc);
            if let Some(idx) = entries.iter().position(|e| e.position == pos) {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    let selector = ".editor-content h1, .editor-content h2, .editor-content h3";
                    if let Ok(nodes) = doc.query_selector_all(selector) {
                        if let Some(node) = nodes.item(idx as u32) {
                            if let Ok(el) = node.dyn_into::<web_sys::Element>() {
                                el.scroll_into_view_with_bool(true);
                            }
                        }
                    }
                }
            }
        }
    });

    // Phase 5 M-P4 piece B: install the editor-bridge callback so
    // editor-scoped palette commands (Bold, Heading, etc.) can
    // dispatch through `on_command` without threading it down via
    // props. `on_cleanup` clears it so a stale Callback can't fire
    // after the page unmounts. Effect re-runs on signal changes
    // but the inner set only writes when the value differs, so
    // there's no flicker.
    on_cleanup(|| {
        crate::commands::editor_bridge::set_editor_cmd(None);
    });

    // #148 @-ask flow: signals owned by the doc page so
    // `OpenAskDialog` can capture the prompt + insert range
    // before flipping the shared `ask_visible`.
    let (ask_initial_prompt, set_ask_initial_prompt) =
        signal::<Option<String>>(None);
    let (ask_insert_range, set_ask_insert_range) =
        signal::<Option<(usize, usize)>>(None);
    let (ask_mode, set_ask_mode) =
        signal::<crate::api::ask::AskMode>(crate::api::ask::AskMode::Agent);
    let (ask_hidden_suffix, set_ask_hidden_suffix) = signal::<Option<String>>(None);

    // Toolbar dispatches commands via signal.
    // InsertComment is handled here (opens comment pane), everything else goes to editor.
    let on_command = Callback::new(move |cmd: ToolbarCommand| {
        if matches!(cmd, ToolbarCommand::InsertComment) {
            open_comment_pane(
                &editor_state,
                &set_pending_block_id,
                &set_pending_anchor_start,
                &set_pending_anchor_end,
                &set_conversation_visible,
                &conversation_visible,
            );
            return;
        }
        // #148 @-ask — capture prompt + range, flip AskDialog
        // open. The dialog's on_insert callback (see the mount
        // point below) issues the actual InsertAiText once the
        // assistant stream is Done and the user clicks Insert.
        if let ToolbarCommand::OpenAskDialog { prompt, insert_range, mode, hidden_suffix } = cmd {
            set_ask_initial_prompt.set(Some(prompt));
            set_ask_insert_range.set(Some(insert_range));
            set_ask_mode.set(mode);
            set_ask_hidden_suffix.set(hidden_suffix);
            set_ask_visible.set(true);
            return;
        }
        set_toolbar_command.set(Some(cmd));
    });

    // Detect `@` mention trigger + `/` slash-command trigger.
    // Both use the same word-boundary rule (see
    // `detect_menu_trigger` at the bottom of this file); the only
    // difference is the trigger char and which set of signals gets
    // written. Extracting the rule keeps the two Effects skin-thin
    // and lets it be unit-tested in isolation.
    Effect::new(move |_| {
        // #94: @-menu Effect; use try_* on untracked reads.
        let Some(state) = editor_state.get() else {
            return;
        };
        let pos = state.selection.from();
        if !state.selection.empty() {
            if at_menu_visible.try_get_untracked() == Some(true) {
                set_at_menu_visible.set(false);
            }
            return;
        }
        let text_before = state.doc.text_before(pos).unwrap_or_default();
        if let Some(query) = detect_menu_trigger(&text_before, '@') {
            set_at_menu_query.set(query);
            if at_menu_visible.try_get_untracked() == Some(false) {
                if let Some(sel_rect) =
                    crate::components::dom_position::selection_viewport_rect()
                {
                    let (ml, mt) = crate::components::dom_position::place_below(
                        &sel_rect, 280.0, 4.0,
                    );
                    set_at_menu_left.set(ml);
                    set_at_menu_top.set(mt);
                }
                set_at_menu_visible.set(true);
            }
            return;
        }
        if at_menu_visible.get_untracked() {
            set_at_menu_visible.set(false);
        }
    });

    // #148 v2 slice 2 — parallel `/` slash-command trigger. Same
    // detector, different trigger char and signal set.
    Effect::new(move |_| {
        let Some(state) = editor_state.get() else {
            return;
        };
        let pos = state.selection.from();
        if !state.selection.empty() {
            if slash_menu_visible.try_get_untracked() == Some(true) {
                set_slash_menu_visible.set(false);
            }
            return;
        }
        let text_before = state.doc.text_before(pos).unwrap_or_default();
        if let Some(query) = detect_menu_trigger(&text_before, '/') {
            set_slash_menu_query.set(query);
            if slash_menu_visible.try_get_untracked() == Some(false) {
                if let Some(sel_rect) =
                    crate::components::dom_position::selection_viewport_rect()
                {
                    let (ml, mt) = crate::components::dom_position::place_below(
                        &sel_rect, 280.0, 4.0,
                    );
                    set_slash_menu_left.set(ml);
                    set_slash_menu_top.set(mt);
                }
                set_slash_menu_visible.set(true);
            }
            return;
        }
        if slash_menu_visible.get_untracked() {
            set_slash_menu_visible.set(false);
        }
    });

    // Phase 5 M-P4 piece B: install the editor-bridge with this
    // page's on_command Callback. The palette's editor-scoped
    // commands dispatch through this. Effect tracks no signals, so
    // it runs once on mount and the registration sticks until
    // on_cleanup fires above.
    Effect::new(move |_| {
        crate::commands::editor_bridge::set_editor_cmd(Some(on_command));
    });

    // Block menu command handler (reuses the same command dispatch).
    let on_block_command = Callback::new(move |cmd: ToolbarCommand| {
        set_toolbar_command.set(Some(cmd));
        set_block_menu_visible.set(false);
    });

    // AtMenu / slash-menu selection (#148 v1 + v2 slice 2):
    // fans out to insert commands, user mention, doc link,
    // live-app inserts, or opens the AskDialog for @-ask. Each
    // match arm builds the right `ToolbarCommand` and dispatches
    // through `on_command`; the trigger range is derived from
    // the caret + query length + trigger-char (1 byte for both
    // `@` and `/`) so the replace atomically clears the trigger
    // text. Called from both `on_at_select` and `on_slash_select`
    // — each of those closes its own menu, then calls this with
    // its own query length.
    let apply_menu_select = move |item: AtMenuItem, query_len: usize| {
        let Some(state) = editor_state.get_untracked() else {
            return;
        };
        if !state.selection.empty() {
            return;
        }
        let pos = state.selection.from();
        let from = pos.saturating_sub(query_len + 1); // +1 for the trigger char
        let trigger_range = (from, pos);
        let cmd = match item.kind {
            AtMenuItemKind::UserMention { user_id, display } => {
                ToolbarCommand::InsertUserMention {
                    from,
                    to: pos,
                    display,
                    user_id,
                }
            }
            AtMenuItemKind::DocumentLink { doc_id, title } => {
                ToolbarCommand::InsertDocLink {
                    from,
                    to: pos,
                    title,
                    href: format!("/d/{doc_id}"),
                }
            }
            AtMenuItemKind::InsertLiveApp { id } => {
                // #148 review finding #1: block-shape inserts need the
                // trigger `@kanban`/`@table`/`@image`/`@hr`/`@code` text
                // cleared BEFORE the block lands, but the two signal
                // writes must reach the editor in separate reactive
                // flushes or Leptos coalesces them and the last write
                // wins. Deferred-dispatch pattern below.
                clear_at_trigger_then_dispatch(
                    from,
                    pos,
                    ToolbarCommand::InsertLiveApp(id),
                    set_toolbar_command,
                );
                return;
            }
            AtMenuItemKind::InsertTable => {
                clear_at_trigger_then_dispatch(
                    from,
                    pos,
                    ToolbarCommand::InsertTable,
                    set_toolbar_command,
                );
                return;
            }
            AtMenuItemKind::InsertImage => {
                clear_at_trigger_then_dispatch(
                    from,
                    pos,
                    ToolbarCommand::UploadImage,
                    set_toolbar_command,
                );
                return;
            }
            AtMenuItemKind::InsertHorizontalRule => {
                clear_at_trigger_then_dispatch(
                    from,
                    pos,
                    ToolbarCommand::InsertHorizontalRule,
                    set_toolbar_command,
                );
                return;
            }
            AtMenuItemKind::SetCodeBlock => {
                clear_at_trigger_then_dispatch(
                    from,
                    pos,
                    ToolbarCommand::SetCodeBlock,
                    set_toolbar_command,
                );
                return;
            }
            AtMenuItemKind::AskAi { prompt } => ToolbarCommand::OpenAskDialog {
                prompt,
                insert_range: (from, pos),
                // Free-form Ask AI runs with RAG tools — the user
                // is asking about their doc corpus, not composing
                // a self-contained prompt.
                mode: crate::api::ask::AskMode::Agent,
                hidden_suffix: None,
            },
            AtMenuItemKind::AskWithDirective { directive, user_input } => {
                // #148 v2 — split the directive prompt into
                // (visible instruction, hidden suffix). The user
                // sees a short editable instruction in the input
                // ("Summarize this document concisely.");
                // the source text + scope guard ride invisibly
                // via `hidden_suffix`. See `compose_directive_parts`.
                let (source_text, scope) =
                    crate::editor::commands::plain_text_from_state(&state);
                let (instruction, suffix) =
                    crate::components::at_menu::compose_directive_parts(
                        &directive,
                        &user_input,
                        &source_text,
                        scope,
                    );
                ToolbarCommand::OpenAskDialog {
                    prompt: instruction,
                    insert_range: (from, pos),
                    // Directive wrappers already carry their
                    // source content in the hidden suffix; RAG
                    // tools would pull unrelated docs and
                    // misinterpret "the following document" as a
                    // search hint.
                    mode: crate::api::ask::AskMode::Direct,
                    hidden_suffix: suffix,
                }
            }
            AtMenuItemKind::InsertDate { style } => {
                // #148 v2 slice 3 — format `now` in the requested
                // style and replace the trigger range with the
                // resulting text. `format_date_now` reads
                // `js_sys::Date::now()` at select-time so the
                // inserted string reflects the user's clock, not
                // the menu-render time.
                let text = crate::components::at_menu::format_date_now(style);
                ToolbarCommand::ReplaceRange { from, to: pos, text }
            }
            AtMenuItemKind::InsertEmoji { emoji } => {
                // #148 v2 slice 5 — replace the trigger range
                // (e.g. `@fire`) with the single emoji character.
                // No new command variant needed — same shape as
                // @date.
                ToolbarCommand::ReplaceRange {
                    from,
                    to: pos,
                    text: emoji.to_string(),
                }
            }
        };
        let _ = trigger_range;
        // #148 v2 review finding #2: route through `on_command` so
        // page-scope commands (currently just `OpenAskDialog`) are
        // intercepted. Writing straight into `set_toolbar_command`
        // routes `OpenAskDialog` into editor_component's no-op arm
        // — the dialog never opens. For every OTHER variant,
        // `on_command`'s fallthrough writes `set_toolbar_command`
        // byte-identically to the old direct call.
        on_command.run(cmd);
    };

    let on_at_select = Callback::new(move |item: AtMenuItem| {
        set_at_menu_visible.set(false);
        let query_len = at_menu_query.get_untracked().chars().count();
        apply_menu_select(item, query_len);
    });

    let on_slash_select = Callback::new(move |item: AtMenuItem| {
        set_slash_menu_visible.set(false);
        let query_len = slash_menu_query.get_untracked().chars().count();
        apply_menu_select(item, query_len);
    });

    let on_at_close = Callback::new(move |_: ()| {
        set_at_menu_visible.set(false);
    });

    let on_slash_close = Callback::new(move |_: ()| {
        set_slash_menu_visible.set(false);
    });

    // Scroll callback for the editor container — increments scroll_tick
    // so fixed-position overlays (comment highlights, cursor overlay) re-render.
    // Throttled via requestAnimationFrame.
    let scroll_raf_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let on_editor_scroll = Callback::new(move |_: ()| {
        if scroll_raf_pending.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        scroll_raf_pending.store(true, std::sync::atomic::Ordering::Relaxed);
        let pending = scroll_raf_pending.clone();
        let cb = Closure::once_into_js(move || {
            pending.store(false, std::sync::atomic::Ordering::Relaxed);
            set_scroll_tick.set(scroll_tick.get_untracked().wrapping_add(1));
            // Dismiss transient menus on editor scroll (they'd render at stale positions)
            set_at_menu_visible.set(false);
        });
        let _ = web_sys::window().map(|w| w.request_animation_frame(cb.as_ref().unchecked_ref()));
    });

    // Re-render cursor overlays on window resize and scroll.
    // Positions are viewport-relative (position: fixed + getBoundingClientRect),
    // so they must be recalculated when the viewport changes.
    {
        if let Some(window) = web_sys::window() {
            // MUST be `Fn`, not `FnMut`. The capture-phase scroll listener
            // fires for every nested scroll in the document; setting
            // `at_menu_visible=false` here can synchronously unmount the @
            // menu, which mutates layout and immediately fires another
            // scroll event in the capture phase. With FnMut, that
            // re-entry trips the wasm-bindgen RefCell guard and panics
            // with "closure invoked recursively or after being dropped".
            // The body only does signal writes (`WriteSignal::set` takes
            // `&self`), so `Fn` is sufficient.
            let on_viewport_change = Closure::<dyn Fn()>::wrap(Box::new(move || {
                set_scroll_tick.set(scroll_tick.get_untracked().wrapping_add(1));
                // Dismiss transient menus on scroll (they'd render at stale positions)
                set_at_menu_visible.set(false);
            }));
            // Hold one function reference to register with and detach with —
            // remove_event_listener only matches the exact listener it was
            // added with (and must repeat the capture flag).
            let func: js_sys::Function =
                on_viewport_change.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let _ = window.add_event_listener_with_callback("resize", &func);
            // Also listen for scroll on the capturing phase — catches scroll events
            // from any element (editor-container, main-content, or the page itself).
            let _ = window.add_event_listener_with_callback_and_bool(
                "scroll", &func, true, // capture phase
            );

            // #6: detach on unmount instead of `forget()`-leaking. This is an
            // SPA — the page unmounts on route change without a reload, so a
            // forgotten listener (holding its captured WriteSignals) accrued
            // on every away-and-back visit. The non-Send handles ride in a
            // SendWrapper so the `Send + Sync` `on_cleanup` closure can own
            // them (safe: the wasm runtime is single-threaded and cleanup
            // runs on that thread). We remove the listeners BEFORE the Closure
            // drops, so JS can never invoke a freed closure — the failure mode
            // the old `forget()` avoided by never dropping at all.
            let teardown =
                send_wrapper::SendWrapper::new((window.clone(), func, on_viewport_change));
            on_cleanup(move || {
                let (window, func, _closure) = teardown.take();
                let _ = window.remove_event_listener_with_callback("resize", &func);
                let _ = window.remove_event_listener_with_callback_and_bool("scroll", &func, true);
                // `_closure` drops here, after both listeners are detached.
            });
        }
    }

    // Remap comment anchor positions when the document changes.
    // Receives (step_maps, old_doc) from EditorComponent after each doc-changing transaction.
    // Debounced persist: saves updated anchors to server after 2s of inactivity.
    let anchor_save_gen = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let on_editor_mapping = Callback::new(
        move |(step_maps, old_doc): (Vec<crate::editor::transform::StepMap>, crate::editor::model::Node)| {
            if step_maps.is_empty() {
                return;
            }
            let Some(new_state) = editor_state.get_untracked() else { return };
            let new_doc = &new_state.doc;

            let mut changed: Vec<(String, u32, u32)> = Vec::new();
            set_inline_threads.update(|threads| {
                changed = crate::components::comment_highlights::remap_thread_anchors(
                    threads, &step_maps, &old_doc, new_doc,
                );
            });

            // Debounce: bump generation; only the latest batch persists after 2s.
            if !changed.is_empty() {
                let generation = anchor_save_gen.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                let gen_ref = std::sync::Arc::clone(&anchor_save_gen);
                leptos::task::spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(2_000).await;
                    if gen_ref.load(std::sync::atomic::Ordering::Relaxed) != generation {
                        return; // superseded by a newer edit
                    }
                    for (tid, start, end) in changed {
                        let _ = crate::api::comments::update_thread_anchors(&tid, start, end).await;
                    }
                });
            }
        },
    );

    // Open the comment popup at the current editor selection. Shared
    // by the floating SelectionToolbar (\u{1F4AC} button) and the
    // editor's right-click context menu (Comment item) so the two
    // entry points produce identical popup state.
    let request_comment = Callback::new(move |()| {
        let Some(state) = editor_state.get_untracked() else { return };
        let from = state.selection.from();
        let to = state.selection.to();
        let Some(bid) = state.doc.block_id_at(from) else { return };
        let block = crate::editor::state::find_block_at(&state.doc, from);
        let (a_start, a_end) = if from != to {
            if let Some(b) = &block {
                let rel_from = (from.saturating_sub(b.content_start)) as u32;
                let rel_to = (to.saturating_sub(b.content_start)).min(b.content.size()) as u32;
                if rel_from < rel_to { (Some(rel_from), Some(rel_to)) }
                else { (None, None) }
            } else { (None, None) }
        } else { (None, None) };

        if let Some(sel_rect) = crate::components::dom_position::selection_viewport_rect() {
            let (pl, pt) = crate::components::dom_position::place_left_margin(&sel_rect, 420.0, 12.0);
            set_popup_left.set(pl);
            set_popup_top.set(pt);
        }

        set_popup_block_id.set(Some(bid));
        set_popup_anchor_start.set(a_start);
        set_popup_anchor_end.set(a_end);
        set_popup_is_new.set(true);
        set_popup_thread_id.set(None);
    });

    // Shared gutter-detection logic used by both mouse hover (desktop) and
    // touchstart (mobile). Returns true and shows the block menu when
    // (x, target) falls within the left gutter (<40px) of a block-level
    // element inside `.editor-content`. Returns false otherwise.
    let try_show_block_menu_at = move |x: f64, target: Option<web_sys::Element>| -> bool {
        let Some(target) = target else { return false };
        // Any top-level block in the editor counts as a hover
        // target — plain textblocks by tag name, plus live-app
        // wrappers that render as `<div class="…-block">`. The
        // check has to accept both because the gutter menu is
        // the discovery affordance for the 📅 / 📋 icons, and
        // if the user hovers the left margin of an *existing*
        // calendar or kanban block that's the moment they're
        // most likely looking for those options.
        let is_hover_target = |el: &web_sys::Element| -> bool {
            let tag = el.tag_name().to_lowercase();
            if matches!(tag.as_str(), "p" | "h1" | "h2" | "h3" | "blockquote" | "hr") {
                return true;
            }
            let cl = el.class_list();
            cl.contains("calendar-block")
                || cl.contains("kanban-block")
                || cl.contains("spreadsheet-block")
        };
        let mut el = Some(target);
        while let Some(ref current) = el {
            if is_hover_target(current) {
                if let Some(parent) = current.closest(".editor-content").ok().flatten() {
                    let rect = current.get_bounding_client_rect();
                    let editor_rect = parent.get_bounding_client_rect();
                    if x < editor_rect.left() + 40.0 {
                        set_block_menu_top.set(rect.top());
                        set_block_menu_visible.set(true);
                        return true;
                    }
                }
                return false;
            }
            el = current.parent_element();
        }
        false
    };

    // Mousemove on editor area: detect block hover for BlockMenu.
    let on_editor_mousemove = move |ev: web_sys::MouseEvent| {
        let x = ev.client_x() as f64;
        let target = ev.target().and_then(|t| t.dyn_ref::<web_sys::Element>().cloned());
        if !try_show_block_menu_at(x, target) {
            set_block_menu_visible.set(false);
        }
    };

    // Touchstart on editor area: same gutter detection for mobile. A tap
    // in the left 40px of any block surfaces the block menu for that
    // block. Tap-on-content (outside the gutter) leaves the menu state
    // alone so it doesn't fight the editor's own touch handling.
    let on_editor_touchstart = move |ev: web_sys::TouchEvent| {
        let touches = ev.touches();
        if touches.length() != 1 {
            return;
        }
        let Some(t) = touches.item(0) else { return };
        let x = t.client_x() as f64;
        let y = t.client_y() as f64;
        let target = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.element_from_point(x as f32, y as f32));
        if !try_show_block_menu_at(x, target) {
            set_block_menu_visible.set(false);
        }
    };

    // Two-finger pinch zoom on the editor wrapper: while the gesture is
    // active we own it (preventDefault stops the browser's page-level
    // pinch-to-zoom), reading finger distance and applying the ratio to
    // the wrapper's CSS `zoom`. Stored start state lets us anchor each
    // gesture to wherever the user picked up — pinching out from 1.5x
    // grows from 1.5x, not from 1x. Clamped to a sensible range.
    // Pinch state lives in a signal (rather than Rc<RefCell>) because the
    // touch handlers below get captured by the outer `move ||` view
    // closure, which Leptos requires to be Send. ReadSignal/WriteSignal
    // are Send + Copy; Rc is neither.
    let (pinch_state, set_pinch_state) = signal::<Option<(f64, f64)>>(None);

    let pinch_distance = |ev: &web_sys::TouchEvent| -> Option<f64> {
        let touches = ev.touches();
        if touches.length() < 2 {
            return None;
        }
        let a = touches.item(0)?;
        let b = touches.item(1)?;
        let dx = a.client_x() as f64 - b.client_x() as f64;
        let dy = a.client_y() as f64 - b.client_y() as f64;
        Some((dx * dx + dy * dy).sqrt())
    };

    let on_editor_pinch_start = move |ev: web_sys::TouchEvent| {
        if let Some(d) = pinch_distance(&ev) {
            ev.prevent_default();
            set_pinch_state.set(Some((d, editor_zoom.get_untracked())));
        }
    };

    let on_editor_pinch_move = move |ev: web_sys::TouchEvent| {
        let Some(d_now) = pinch_distance(&ev) else {
            return;
        };
        let Some((d_start, z_start)) = pinch_state.get_untracked() else {
            return;
        };
        ev.prevent_default();
        if d_start <= 0.0 {
            return;
        }
        let factor = d_now / d_start;
        let next = (z_start * factor).clamp(0.5, 3.0);
        set_editor_zoom.set(next);
    };

    let on_editor_pinch_end = move |ev: web_sys::TouchEvent| {
        if ev.touches().length() < 2 {
            set_pinch_state.set(None);
        }
    };
    let on_editor_pinch_cancel = on_editor_pinch_end;

    // Global keydown handler for outline toggle
    let on_page_keydown = move |ev: web_sys::KeyboardEvent| {
        // #148 @-menu navigation: while the @-menu is visible,
        // ↑/↓ move the highlight, Enter/Tab activates the
        // highlighted item, Escape closes. Runs first so the
        // menu absorbs Enter before it reaches the editor
        // (Enter would otherwise insert a paragraph break).
        if at_menu_visible.get_untracked() {
            let key = ev.key();
            let items = at_menu_results.get_untracked();
            let n = items.len();
            match key.as_str() {
                "ArrowDown" => {
                    ev.prevent_default();
                    if n > 0 {
                        let cur = at_menu_highlighted.get_untracked();
                        set_at_menu_highlighted.set((cur + 1) % n);
                    }
                    return;
                }
                "ArrowUp" => {
                    ev.prevent_default();
                    if n > 0 {
                        let cur = at_menu_highlighted.get_untracked();
                        set_at_menu_highlighted
                            .set(if cur == 0 { n - 1 } else { cur - 1 });
                    }
                    return;
                }
                "Enter" | "Tab" => {
                    ev.prevent_default();
                    let cur = at_menu_highlighted.get_untracked();
                    if let Some(item) = items.get(cur).cloned() {
                        on_at_select.run(item);
                    }
                    return;
                }
                "Escape" => {
                    ev.prevent_default();
                    set_at_menu_visible.set(false);
                    return;
                }
                _ => {}
            }
        }
        // #148 v2 slice 2 — parallel keyboard nav for the slash
        // menu. Same shape as the @-menu branch above but routes
        // through the slash_menu_* signals + on_slash_select.
        if slash_menu_visible.get_untracked() {
            let key = ev.key();
            let items = slash_menu_results.get_untracked();
            let n = items.len();
            match key.as_str() {
                "ArrowDown" => {
                    ev.prevent_default();
                    if n > 0 {
                        let cur = slash_menu_highlighted.get_untracked();
                        set_slash_menu_highlighted.set((cur + 1) % n);
                    }
                    return;
                }
                "ArrowUp" => {
                    ev.prevent_default();
                    if n > 0 {
                        let cur = slash_menu_highlighted.get_untracked();
                        set_slash_menu_highlighted
                            .set(if cur == 0 { n - 1 } else { cur - 1 });
                    }
                    return;
                }
                "Enter" | "Tab" => {
                    ev.prevent_default();
                    let cur = slash_menu_highlighted.get_untracked();
                    if let Some(item) = items.get(cur).cloned() {
                        on_slash_select.run(item);
                    }
                    return;
                }
                "Escape" => {
                    ev.prevent_default();
                    set_slash_menu_visible.set(false);
                    return;
                }
                _ => {}
            }
        }
        let ctrl_or_meta = ev.ctrl_key() || ev.meta_key();
        if ctrl_or_meta && ev.shift_key() && ev.key().to_lowercase() == "o" {
            ev.prevent_default();
            set_outline_visible.set(!outline_visible.get_untracked());
        }
        if ctrl_or_meta && ev.alt_key() && ev.key().to_lowercase() == "c" {
            ev.prevent_default();
            open_comment_pane(
                &editor_state,
                &set_pending_block_id,
                &set_pending_anchor_start,
                &set_pending_anchor_end,
                &set_conversation_visible,
                &conversation_visible,
            );
        }
        if ctrl_or_meta && ev.shift_key() && ev.key().to_lowercase() == "h" {
            ev.prevent_default();
            set_history_visible.set(!history_visible.get_untracked());
        }
        // #100: Ctrl/Cmd+Shift+F toggles focus mode.
        if ctrl_or_meta && ev.shift_key() && ev.key().to_lowercase() == "f" {
            ev.prevent_default();
            set_focus_mode.set(!focus_mode.get_untracked());
        }
        // #145: Esc collapses Expand. Real browser-fullscreen also exits on
        // Esc (handled via fullscreenchange); this covers the fallback path
        // where the Fullscreen API was unavailable.
        if ev.key() == "Escape" && expanded.get_untracked() {
            set_expanded.set(false);
            exit_app_fullscreen();
        }
        // #147: Ctrl/Cmd+F opens the in-app Find & Replace bar (overrides
        // browser find). Shift+F is focus mode, handled above.
        if ctrl_or_meta && !ev.shift_key() && ev.key().to_lowercase() == "f" {
            ev.prevent_default();
            set_find_visible.set(true);
        }
        if ctrl_or_meta && ev.key().to_lowercase() == "k" {
            ev.prevent_default();
            // Open in Search mode — the existing `>` prefix path
            // still lets the user flip to Action mode after open.
            set_palette_initial_mode.set(crate::components::search_dialog::PaletteMode::Search);
            set_search_visible.set(true);
        }
        // Phase 5 M-P4 piece B: Ctrl+Shift+P opens directly in
        // Action mode. The dialog's open-Effect pre-fills `>` so
        // the action filter is live immediately.
        if ctrl_or_meta && ev.shift_key() && ev.key().to_lowercase() == "p" {
            ev.prevent_default();
            set_palette_initial_mode.set(crate::components::search_dialog::PaletteMode::Action);
            set_search_visible.set(true);
        }
    };

    // Handle document-level actions from the menu bar.
    // Capture navigate at definition time (inside Router context).
    let navigate_for_action = leptos_router::hooks::use_navigate();
    let on_doc_action = Callback::new(move |action: DocAction| {
        match action {
            DocAction::NewDocument => {
                let navigate = navigate_for_action.clone();
                leptos::task::spawn_local(async move {
                    match documents::create_document("Untitled", None).await {
                        Ok(doc) => { navigate(&format!("/d/{}", doc.id), Default::default()); }
                        Err(e) => { web_sys::console::error_1(&format!("New doc failed: {e}").into()); }
                    }
                });
            }
            DocAction::Share => {
                set_share_visible.set(true);
            }
            DocAction::CopyLink => {
                if let Some(window) = web_sys::window() {
                    // #101: copy the canonical /d/:id URL (stable opaque doc
                    // id), not location.href — the page URL may carry a slug
                    // or a ?comment= deep-link query that shouldn't ride along.
                    let did = current_id.get_untracked();
                    let canonical = if did.is_empty() {
                        window.location().href().ok()
                    } else {
                        window.location().origin().ok().map(|o| format!("{o}/d/{did}"))
                    };
                    if let Some(href) = canonical {
                        // Use wasm_bindgen to call clipboard.writeText safely (no eval/Function).
                        let promise = js_sys::Reflect::get(
                            &window.navigator(),
                            &"clipboard".into(),
                        )
                        .and_then(|clip| {
                            js_sys::Reflect::get(&clip, &"writeText".into())
                        })
                        .and_then(|func| {
                            func.dyn_into::<js_sys::Function>()
                        });
                        if let Ok(write_text) = promise {
                            let clip = js_sys::Reflect::get(
                                &window.navigator(),
                                &"clipboard".into(),
                            ).unwrap_or(wasm_bindgen::JsValue::NULL);
                            let _ = write_text.call1(&clip, &href.into());
                        }
                    }
                }
            }
            DocAction::ExportHtml => {
                spawn_export(current_id.get_untracked(), title.get_untracked(), ExportFormat::Html);
            }
            DocAction::ExportMarkdown => {
                spawn_export(current_id.get_untracked(), title.get_untracked(), ExportFormat::Markdown);
            }
            DocAction::ExportCsv => {
                spawn_export(current_id.get_untracked(), title.get_untracked(), ExportFormat::Csv);
            }
            DocAction::ExportXlsx => {
                spawn_export(current_id.get_untracked(), title.get_untracked(), ExportFormat::Xlsx);
            }
            DocAction::Print => {
                if let Some(window) = web_sys::window() {
                    let _ = window.print();
                }
            }
            DocAction::DocumentHistory => {
                set_history_visible.set(!history_visible.get_untracked());
            }
            DocAction::DeleteDocument => {
                set_confirm_delete_visible.set(true);
            }
            DocAction::ToggleConversation => {
                set_conversation_visible.set(!conversation_visible.get_untracked());
            }
            DocAction::ToggleOutline => {
                set_outline_visible.set(!outline_visible.get_untracked());
            }
            DocAction::ToggleComments => {
                set_comments_visible.set(!comments_visible.get_untracked());
            }
            DocAction::ToggleCursors => {
                set_cursors_visible.set(!cursors_visible.get_untracked());
            }
            DocAction::ToggleFocusMode => {
                set_focus_mode.set(!focus_mode.get_untracked());
            }
            DocAction::ToggleLineNumbers => {
                let v = !line_numbers_visible.get_untracked();
                set_line_numbers_visible.set(v);
                save_bool_pref(PREF_LINE_NUMBERS, v);
            }
            DocAction::TogglePageBreaks => {
                let v = !page_breaks_visible.get_untracked();
                set_page_breaks_visible.set(v);
                save_bool_pref(PREF_PAGE_BREAKS, v);
            }
            // #141: open the read-only Document Details panel.
            DocAction::DocumentDetails => {
                set_details_visible.set(true);
            }
            // #146: open the folder picker; the move happens in its on_pick.
            DocAction::MoveToFolder => {
                set_move_picker_visible.set(true);
            }
            // #146: rename. A spreadsheet carries an explicit title, so we
            // prompt and set it (the debounced title-save Effect persists it).
            // A document's title IS its first line (derived in
            // `on_state_change`), so a prompt-set would just be overwritten on
            // the next keystroke — instead we focus the editor and select that
            // first line for the user to retype.
            DocAction::RenameDocument => {
                if doc_type.get_untracked() == "spreadsheet" {
                    if let Some(window) = web_sys::window() {
                        let current = title.get_untracked();
                        if let Ok(Some(entered)) = window.prompt_with_message_and_default(
                            &crate::t!("document-rename-prompt"),
                            &current,
                        ) {
                            let entered = entered.trim().to_string();
                            if !entered.is_empty() {
                                set_title.set(entered);
                            }
                        }
                    }
                } else if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                    if let Some(content) =
                        document.query_selector(".editor-content").ok().flatten()
                    {
                        if let Ok(el) = content.clone().dyn_into::<web_sys::HtmlElement>() {
                            let _ = el.focus();
                        }
                        if let (Some(first), Some(sel)) = (
                            content.first_element_child(),
                            web_sys::window().and_then(|w| w.get_selection().ok().flatten()),
                        ) {
                            if let Ok(range) = document.create_range() {
                                let _ = range.select_node_contents(&first);
                                let _ = sel.remove_all_ranges();
                                let _ = sel.add_range(&range);
                            }
                        }
                    }
                }
            }
            // #146: open the Duplicate dialog (name + destination folder +
            // share warning). The actual copy runs in the dialog's on_confirm.
            DocAction::DuplicateDocument => {
                set_duplicate_dialog_visible.set(true);
            }
            // #147: open the in-app Find & Replace bar.
            DocAction::OpenFindReplace => {
                set_find_visible.set(true);
            }
            // #140: owner toggles the doc-wide edit lock. Optimistically flip
            // the local signal (so the editor + banner react immediately), then
            // persist; revert on failure. The server is the authority — it
            // also drops WS/REST writes once locked.
            DocAction::ToggleLockEdits => {
                let did = current_id.get_untracked();
                if !did.is_empty() {
                    let target = !is_locked.get_untracked();
                    set_is_locked.set(target);
                    leptos::task::spawn_local(async move {
                        if let Err(e) = documents::set_document_lock(&did, target).await {
                            set_is_locked.set(!target);
                            web_sys::console::error_1(
                                &format!("Toggle lock failed: {e}").into(),
                            );
                        }
                    });
                }
            }
            // #142: mark / unmark template. Optimistic flip + revert on
            // failure — no re-fetch of metadata. Earlier revisions re-fetched
            // to observe the server's auto-lock side effect, but the server
            // no longer auto-locks on mark, so the fetch has no purpose. And
            // it wasn't free: `set_is_locked.set(doc.locked)` on success
            // fired listeners even when the value was unchanged, which
            // triggered the editor's reactive closure to re-run and remount
            // <EditorComponent> with the original (empty) initial_content
            // bytes — losing any local typing since the last put_content.
            DocAction::MarkAsTemplate | DocAction::UnmarkTemplate => {
                let did = current_id.get_untracked();
                if !did.is_empty() {
                    let target = matches!(action, DocAction::MarkAsTemplate);
                    set_is_template.set(target);
                    leptos::task::spawn_local(async move {
                        if let Err(e) = documents::set_document_template(&did, target).await {
                            set_is_template.set(!target);
                            web_sys::console::error_1(
                                &format!("Set template failed: {e}").into(),
                            );
                        }
                    });
                }
            }
            // #142: open the shell-mounted template picker. The modal itself
            // owns the copy + navigate (so the same picker serves the sidebar
            // entry and the home page).
            DocAction::NewFromTemplate => {
                ctx.template_picker_open.set(true);
            }
        }
    });

    // #144: star-dropdown action helpers. All capture Copy signals, so they
    // stay Copy and can be reused across the menu's click handlers.
    let reload_doc_collections = move || {
        let id = current_id.get_untracked();
        if id.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            if let Ok(list) = documents::list_doc_collections(&id).await {
                doc_collections.set(list);
            }
        });
    };
    // Add/remove the star. Optimistic; reverts on failure; bumps the sidebar
    // Favorites tick after the write lands (see the original toggle's note).
    let toggle_favorite = move || {
        let now = !is_favorite.get_untracked();
        set_is_favorite.set(now);
        let id = current_id.get_untracked();
        leptos::task::spawn_local(async move {
            let res = if now {
                documents::add_favorite(&id).await
            } else {
                documents::remove_favorite(&id).await
            };
            match res {
                Ok(_) => set_favorites_dirty.update(|n| *n = n.wrapping_add(1)),
                Err(_) => {
                    set_is_favorite.set(!now);
                    set_favorites_dirty.update(|n| *n = n.wrapping_add(1));
                }
            }
        });
    };
    // Toggle the current doc's membership in collection `cid`. Optimistic on the
    // in-menu checkmark; reverts on failure; bumps the sidebar Collections tick.
    let toggle_collection = move |cid: String, currently_in: bool| {
        let target = !currently_in;
        doc_collections.update(|list| {
            if let Some(c) = list.iter_mut().find(|c| c.id == cid) {
                c.contains = target;
            }
        });
        let id = current_id.get_untracked();
        leptos::task::spawn_local(async move {
            let res = if target {
                documents::add_doc_to_collection(&id, &cid).await
            } else {
                documents::remove_doc_from_collection(&id, &cid).await
            };
            match res {
                Ok(_) => set_collections_dirty.update(|n| *n = n.wrapping_add(1)),
                Err(_) => {
                    doc_collections.update(|list| {
                        if let Some(c) = list.iter_mut().find(|c| c.id == cid) {
                            c.contains = currently_in;
                        }
                    });
                }
            }
        });
    };
    // "New Collection…" — prompt for a name, create it containing this doc.
    let new_collection = move || {
        let Some(window) = web_sys::window() else { return };
        let Ok(Some(name)) = window.prompt_with_message(&crate::t!("collection-new-prompt")) else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let id = current_id.get_untracked();
        if id.is_empty() {
            return;
        }
        leptos::task::spawn_local(async move {
            if documents::create_doc_collection(&id, &name).await.is_ok() {
                if let Ok(list) = documents::list_doc_collections(&id).await {
                    doc_collections.set(list);
                }
                set_collections_dirty.update(|n| *n = n.wrapping_add(1));
            }
        });
    };

    view! {
        // #152: the sidebar + `.app-layout` flex wrapper now live in
        // `AppShell` (this page renders inside its `<Outlet/>`). `.doc-shell`
        // is the editor's flex-sibling slot next to the persistent sidebar;
        // it hosts the editor-area event handlers. The focus-mode / expanded
        // / line-number / page-break classes moved to the shell's app-layout
        // (driven by `ShellCtx`), so the existing `.app-layout.*` CSS keeps
        // matching its descendants.
        <div
            class="doc-shell"
            on:keydown=on_page_keydown
            on:mousemove=on_editor_mousemove
            on:touchstart=on_editor_touchstart
        >
            // #134: focus mode is entered/exited from the always-visible
            // header toggle (`.focus-toggle-btn` below), which flips its
            // icon (expand ⤢ ↔ exit ✕) with the state. The View-menu toggle
            // and Ctrl+Shift+F still work. (The old floating "Exit focus"
            // button lived inside a focus-gated `<Show>` and needed a
            // deferred close to avoid tearing itself down mid-click; the
            // header button doesn't — the doc header stays mounted in focus
            // mode, so it just re-renders.)
            <main id="main-content" tabindex="-1" class="main-content">
                {move || error.get().map(|e| view! {
                    <div style="color: var(--color-error); padding: var(--space-md);" role="alert">
                        {e}
                    </div>
                })}

                {move || liveapp_error.get().map(|diag| view! {
                    <div
                        class="collab-liveapp-toast"
                        role="alert"
                        on:click=move |_| set_liveapp_error.set(None)
                        title=diag.clone()
                    >
                        {crate::t!("collab-liveapp-rejected-toast")}
                    </div>
                })}

                <Show when=move || is_trashed.get()>
                    <div class="trash-banner">
                        <span class="trash-banner-message">
                            {crate::t!("document-trash-banner")}
                        </span>
                        <button
                            class="btn btn-primary"
                            on:click=move |_| set_restore_picker_visible.set(true)
                        >{crate::t!("document-trash-restore")}</button>
                        <button
                            class="btn btn-danger"
                            on:click=move |_| set_confirm_purge_visible.set(true)
                        >{crate::t!("document-trash-delete-forever")}</button>
                    </div>
                </Show>

                // #140: doc-wide edit-lock banner. Shown to everyone while
                // locked (and not trashed); the owner gets an inline Unlock
                // button (same path as the Format-menu toggle).
                <Show when=move || is_locked.get() && !is_trashed.get()>
                    <div class="lock-banner" role="status">
                        <span class="lock-banner-message">
                            {crate::t!("document-locked-banner")}
                        </span>
                        <Show when=move || can_manage_lock.get()>
                            <button
                                class="btn btn-primary"
                                on:click=move |_| on_doc_action.run(DocAction::ToggleLockEdits)
                            >{crate::t!("document-locked-unlock")}</button>
                        </Show>
                    </div>
                </Show>

                // #110: a view-only link viewer (eligibility decided by the
                // backend) can ask the owner for edit access. Hidden once the
                // request lands ("Sent"); re-enabled on failure so they retry.
                <Show when=move || can_request_access.get() && !is_trashed.get()>
                    <div class="link-viewer-banner">
                        <span class="link-viewer-banner-message">
                            {crate::t!("share-link-request-banner")}
                        </span>
                        <button
                            class="btn btn-primary"
                            prop:disabled=move || matches!(
                                request_access_state.get(),
                                RequestAccessState::Sending | RequestAccessState::Sent
                            )
                            on:click=move |_| {
                                if request_access_state.get_untracked()
                                    == RequestAccessState::Sending
                                {
                                    return;
                                }
                                set_request_access_state.set(RequestAccessState::Sending);
                                let id = current_id.get_untracked();
                                leptos::task::spawn_local(async move {
                                    match crate::api::sharing::request_access(&id).await {
                                        Ok(()) => set_request_access_state
                                            .set(RequestAccessState::Sent),
                                        Err(_) => set_request_access_state
                                            .set(RequestAccessState::Failed),
                                    }
                                });
                            }
                        >
                            {move || match request_access_state.get() {
                                RequestAccessState::Idle => {
                                    crate::t!("share-link-request-button")
                                }
                                RequestAccessState::Sending => {
                                    crate::t!("share-link-request-sending")
                                }
                                RequestAccessState::Sent => {
                                    crate::t!("share-link-request-sent")
                                }
                                RequestAccessState::Failed => {
                                    crate::t!("share-link-request-retry")
                                }
                            }}
                        </button>
                    </div>
                </Show>

                <div class="doc-header">
                    <button
                        class="mobile-menu-toggle"
                        aria-label=crate::t!("common-open-navigation")
                        on:click=move |_| set_mobile_sidebar_open.update(|v| *v = !*v)
                    >"\u{2630}"</button>
                    {move || {
                        if doc_type.get() == "spreadsheet" {
                            view! {
                                <input
                                    type="text"
                                    class="doc-title doc-title-editable"
                                    prop:value=move || title.get()
                                    prop:disabled=move || is_trashed.get()
                                    on:input=move |e| set_title.set(event_target_value(&e))
                                    on:blur=move |_| {
                                        // Title is persisted by the title-save Effect
                                    }
                                    on:keydown=move |e: web_sys::KeyboardEvent| {
                                        if e.key() == "Enter" {
                                            e.prevent_default();
                                            if let Some(el) = e.target() {
                                                if let Ok(el) = el.dyn_into::<web_sys::HtmlElement>() {
                                                    let _ = el.blur();
                                                }
                                            }
                                        }
                                    }
                                />
                            }.into_any()
                        } else {
                            // Truncating display: full title surfaces on hover
                            // via the `title` attribute and as the accessible
                            // name for screen readers.
                            view! {
                                <div
                                    class="doc-title"
                                    title=move || title.get()
                                >{title}</div>
                            }.into_any()
                        }
                    }}
                    // #142/#149: folder-membership chips + Add-to-folder button.
                    // Was its own row below the header; now inline so the header
                    // uses the horizontal space instead of stacking another
                    // strip. `flex: 1 1 0; min-width: 0` lets the chips take the
                    // slack between the title and the header-actions cluster
                    // (which pushes right via `margin-inline-start: auto`), and
                    // overflow scrolls horizontally rather than pushing buttons
                    // off-screen. Hidden in focus mode via CSS.
                    <div class="doc-folders-inline">
                        {move || {
                            doc_folders.get().into_iter().map(|f| {
                                let is_primary = f.is_primary;
                                let fid = f.id.clone();
                                view! {
                                    <span class="doc-folder-chip" class:primary=is_primary>
                                        <span class="doc-folder-chip-name">{f.title}</span>
                                        {(!is_primary).then(|| {
                                            let fid = fid.clone();
                                            view! {
                                                <button
                                                    class="doc-folder-chip-remove"
                                                    title=crate::t!("document-folder-remove")
                                                    aria-label=crate::t!("document-folder-remove")
                                                    on:click=move |_| {
                                                        let id = current_id.get_untracked();
                                                        let fid = fid.clone();
                                                        doc_folders.update(|l| l.retain(|x| x.id != fid));
                                                        leptos::task::spawn_local(async move {
                                                            if documents::remove_doc_from_folder(&id, &fid)
                                                                .await
                                                                .is_err()
                                                            {
                                                                reload_doc_folders();
                                                            }
                                                        });
                                                    }
                                                >"\u{2715}"</button>
                                            }
                                        })}
                                    </span>
                                }
                            }).collect::<Vec<_>>()
                        }}
                        <button
                            class="doc-folder-add-btn"
                            title=crate::t!("document-folder-add")
                            aria-label=crate::t!("document-folder-add")
                            on:click=move |_| set_add_folder_picker_visible.set(true)
                        >{crate::t!("document-folder-add")}</button>
                    </div>
                    <div class="doc-header-actions">
                        <SyncIndicator state=sync_state.into() />
                        {move || crate::api::client::get_auth().map(|auth| {
                            view! { <span class="user-name">{auth.name.clone()}</span> }
                        })}
                        <NotificationBell />
                        // #144: favorite star + dropdown (Added to Favorites /
                        // Collections / Remove from Favorites). The star opens
                        // the menu; the glyph reflects the favorited state.
                        <div class="favorite-menu-wrapper">
                            <button
                                class="favorite-toggle-btn"
                                class:active=move || is_favorite.get()
                                title=crate::t!("document-favorite-menu")
                                aria-label=crate::t!("document-favorite-menu")
                                aria-haspopup="true"
                                aria-expanded=move || fav_menu_open.get().to_string()
                                on:click=move |_| {
                                    let opening = !fav_menu_open.get_untracked();
                                    set_fav_menu_open.set(opening);
                                    if opening {
                                        reload_doc_collections();
                                    }
                                }
                            >{move || if is_favorite.get() { "\u{2605}" } else { "\u{2606}" }}</button>
                            <Show when=move || fav_menu_open.get()>
                                <div
                                    class="favorite-menu-backdrop"
                                    on:click=move |_| set_fav_menu_open.set(false)
                                ></div>
                                <div class="favorite-menu-dropdown">
                                    // Favorite state row: a check + "Added to
                                    // Favorites" when starred, else "Add to
                                    // Favorites". Clicking it toggles the star.
                                    <button
                                        class="favorite-menu-item"
                                        on:click=move |_| toggle_favorite()
                                    >
                                        <span class="favorite-menu-check">
                                            {move || if is_favorite.get() { "\u{2713}" } else { "" }}
                                        </span>
                                        <span>{move || if is_favorite.get() {
                                            crate::t!("favorite-menu-added")
                                        } else {
                                            crate::t!("favorite-menu-add")
                                        }}</span>
                                    </button>

                                    <div class="favorite-menu-sep"></div>
                                    <div class="favorite-menu-section-title">
                                        {crate::t!("favorite-menu-collections")}
                                    </div>
                                    {move || {
                                        doc_collections.get().into_iter().map(|c| {
                                            let cid = c.id.clone();
                                            let contains = c.contains;
                                            view! {
                                                <button
                                                    class="favorite-menu-item"
                                                    on:click=move |_| toggle_collection(cid.clone(), contains)
                                                >
                                                    <span class="favorite-menu-check">
                                                        {if contains { "\u{2713}" } else { "" }}
                                                    </span>
                                                    <span>{c.name}</span>
                                                </button>
                                            }
                                        }).collect::<Vec<_>>()
                                    }}
                                    <button
                                        class="favorite-menu-item favorite-menu-new"
                                        on:click=move |_| new_collection()
                                    >
                                        <span class="favorite-menu-check"></span>
                                        <span>{crate::t!("favorite-menu-new-collection")}</span>
                                    </button>

                                    <Show when=move || is_favorite.get()>
                                        <div class="favorite-menu-sep"></div>
                                        <button
                                            class="favorite-menu-item"
                                            on:click=move |_| { toggle_favorite(); set_fav_menu_open.set(false); }
                                        >
                                            <span class="favorite-menu-star">"\u{2605}"</span>
                                            <span>{crate::t!("favorite-menu-remove")}</span>
                                        </button>
                                    </Show>
                                </div>
                            </Show>
                        </div>
                        // #134: single focus/expand toggle. Stays visible in
                        // focus mode (only the sidebar + menu bar hide), and
                        // flips to its opposite (✕ collapse) so reversing is
                        // one click on the same button. Inline state flip is
                        // safe — the button isn't unmounted on click.
                        <button
                            class="focus-toggle-btn"
                            class:active=move || focus_mode.get()
                            title=move || if focus_mode.get() {
                                crate::t!("document-focus-exit")
                            } else {
                                crate::t!("document-focus-enter")
                            }
                            aria-label=move || if focus_mode.get() {
                                crate::t!("document-focus-exit")
                            } else {
                                crate::t!("document-focus-enter")
                            }
                            on:click=move |_| set_focus_mode.set(!focus_mode.get_untracked())
                        >{move || if focus_mode.get() { "\u{2715}" } else { "\u{2922}" }}</button>
                        // #145: Expand — true full-screen, distraction-free
                        // editor (Fullscreen API + chrome drop). Exits via Esc
                        // or the floating collapse button rendered below.
                        <button
                            class="expand-toggle-btn"
                            class:active=move || expanded.get()
                            title=crate::t!("document-expand-enter")
                            aria-label=crate::t!("document-expand-enter")
                            on:click=move |_| toggle_expand.run(())
                        >"\u{2197}"</button>
                        // #142: fast-path copy from a template. Shown only when
                        // this doc is a template. Skips the picker (the user is
                        // already looking at the template they want) and lands
                        // in the caller's Private folder, same default as the
                        // sidebar/home flow.
                        <Show when=move || is_template.get()>
                            <button
                                class="use-template-button"
                                title=crate::t!("document-use-template")
                                on:click=move |_| {
                                    let did = current_id.get_untracked();
                                    if did.is_empty() { return; }
                                    leptos::task::spawn_local(async move {
                                        match documents::copy_document(
                                            &did,
                                            &documents::CopyDocumentRequest::default(),
                                        ).await {
                                            Ok(doc) => hard_navigate(&format!("/d/{}", doc.id)),
                                            Err(e) => web_sys::console::error_1(
                                                &format!("Use template failed: {e}").into(),
                                            ),
                                        }
                                    });
                                }
                            >{crate::t!("document-use-template")}</button>
                        </Show>
                        <button
                            class="share-button"
                            title=crate::t!("document-share-tooltip")
                            aria-label=crate::t!("document-share-tooltip")
                            on:click=move |_| set_share_visible.set(true)
                        >"\u{1F4E4}"</button>
                        // Phone breakpoint: the menu bar is hidden ≤640px
                        // (responsive.css), so its document-level actions
                        // surface here instead — a `⋯` button opening the
                        // same Document menu. Formatting is already covered
                        // on mobile by the bottom toolbar + overflow panel.
                        <div class="mobile-doc-actions">
                            <button
                                class="toolbar-btn"
                                aria-haspopup="menu"
                                aria-label=crate::t!("sidebar-doc-actions-aria")
                                aria-expanded=move || mobile_doc_menu_open.get().to_string()
                                on:click=move |_| mobile_doc_menu_open.update(|o| *o = !*o)
                            >"\u{22EF}"</button>
                            <AnchoredMenu
                                open=mobile_doc_menu_open
                                entries=Callback::new(move |()| document_menu_entries(
                                    on_doc_action,
                                    is_template,
                                ))
                                on_close=Callback::new(move |()| mobile_doc_menu_open.set(false))
                                class="mobile-doc-menu"
                            />
                        </div>
                    </div>
                </div>

                <MenuBar
                    on_command=on_command
                    on_doc_action=on_doc_action
                    conversation_visible=conversation_visible
                    outline_visible=outline_visible
                    comments_visible=comments_visible
                    cursors_visible=cursors_visible
                    focus_mode=focus_mode.read_only()
                    line_numbers_visible=line_numbers_visible.read_only()
                    page_breaks_visible=page_breaks_visible.read_only()
                    locked=is_locked
                    can_manage_lock=can_manage_lock
                    is_template=is_template
                />

                // #147: in-app Find & Replace bar (below the menu bar).
                <FindReplaceBar
                    visible=find_visible
                    editor_state=editor_state
                    on_command=on_command
                    on_close=Callback::new(move |_| set_find_visible.set(false))
                />

                <Toolbar
                    editor_state=editor_state
                    on_command=on_command
                    comment_count=Signal::derive(move || comment_count.get())
                />

                // Editor + side panels in a row
                // Pinch handlers + the --editor-zoom CSS variable live on
                // .editor-with-panels directly. CSS custom properties inherit,
                // so .editor-content (a deeper descendant) still picks up the
                // value. Avoids inserting an extra flex layer that would
                // break the existing `.editor-with-panels .editor-container`
                // sizing.
                <div
                    class="editor-with-panels"
                    style:--editor-zoom=move || format!("{}", editor_zoom.get())
                    on:touchstart=on_editor_pinch_start
                    on:touchmove=on_editor_pinch_move
                    on:touchend=on_editor_pinch_end
                    on:touchcancel=on_editor_pinch_cancel
                >
                {move || {
                    if content_loaded.get() {
                        if doc_type.get() == "spreadsheet" {
                            // EditorState is initialized by the Effect above;
                            // SpreadsheetView renders its grid from editor_state.
                            return view! {
                                <SpreadsheetView
                                    editor_state=editor_state
                                    on_state_change=on_state_change.clone()
                                    on_change=on_change_ss.clone()
                                    doc_id=doc_id()
                                    on_subscribe_foreign=on_subscribe_foreign
                                    on_unsubscribe_foreign=on_unsubscribe_foreign
                                    foreign_doc_invalidate=foreign_doc_invalidate
                                    toolbar_command=toolbar_command
                                    set_toolbar_command=set_toolbar_command
                                    on_cell_cursor=on_cell_cursor
                                    remote_cursors=remote_cursors
                                    cursors_enabled=cursors_visible
                                    on_open_cell_comment=Callback::new(
                                        move |open: crate::components::spreadsheet_view::CellCommentOpen| {
                                            // The popup mounted at the page level shares state
                                            // with editor inline comments; for cells we drive it
                                            // with thread_id set, block_id = the same id (cell-…
                                            // shape passes the 4-32 alphanumeric validator), no
                                            // anchor offsets, is_new = false (the spreadsheet
                                            // pre-created the thread before firing this).
                                            set_popup_block_id.set(Some(open.block_id));
                                            set_popup_anchor_start.set(None);
                                            set_popup_anchor_end.set(None);
                                            set_popup_is_new.set(false);
                                            set_popup_left.set(open.left);
                                            set_popup_top.set(open.top);
                                            set_popup_thread_id.set(Some(open.thread_id));
                                        },
                                    )
                                    cell_threads=cell_threads
                                    focus_cell=focus_cell
                                />
                            }.into_any();
                        }
                        let content = initial_content.get();
                        view! {
                            <EditorComponent props=EditorProps {
                                initial_content: content,
                                on_change: on_change.clone(),
                                on_state_change: on_state_change.clone(),
                                command_signal: toolbar_command,
                                remote_state: remote_state,
                                doc_id: current_id.get_untracked(),
                                on_scroll: Some(on_editor_scroll.clone()),
                                on_mapping: Some(on_editor_mapping.clone()),
                                on_request_comment: Some(request_comment),
                                // #111/#140: read-only when trashed, View-only,
                                // OR the doc is locked (a doc-wide freeze for
                                // everyone). The WS still delivers live remote
                                // updates into the editor; this only blocks
                                // local input (the server also drops writes).
                                readonly: is_trashed.get() || !can_edit.get() || is_locked.get(),
                            } />
                        }.into_any()
                    } else {
                        view! {
                            <div class="editor-container">
                                <div class="editor-content" style="color: var(--color-text-secondary);">
                                    {crate::t!("document-loading")}
                                </div>
                            </div>
                        }.into_any()
                    }
                }}

                <Show when=move || outline_visible.get()>
                    <div
                        class="drawer-backdrop"
                        on:click=move |_| a11y::defer(move || set_outline_visible.set(false))
                    ></div>
                </Show>
                <DocumentOutline
                    editor_state=editor_state
                    visible=outline_visible
                    on_navigate=on_outline_navigate
                    on_close=Callback::new(move |()| set_outline_visible.set(false))
                    doc_id=Signal::derive(doc_id)
                />

                <Show when=move || conversation_visible.get()>
                    <div
                        class="drawer-backdrop"
                        on:click=move |_| a11y::defer(move || set_conversation_visible.set(false))
                    ></div>
                </Show>
                <ConversationPane
                    visible=conversation_visible
                    doc_id=current_id
                    editor_state=editor_state
                    pending_block_id=pending_block_id
                    pending_anchor_start=pending_anchor_start
                    pending_anchor_end=pending_anchor_end
                    on_block_used=Callback::new(move |_| {
                        set_pending_block_id.set(None);
                        set_pending_anchor_start.set(None);
                        set_pending_anchor_end.set(None);
                    })
                    on_threads_loaded=Callback::new(move |threads: Vec<InlineThreadInfo>| {
                        set_comment_count.set(threads.len());
                        set_inline_threads.set(threads);
                    })
                    filter_thread_id=filter_thread_id
                    remote_cursors=remote_cursors
                    on_typing_change=Callback::new(move |tid: Option<String>| {
                        set_typing_thread_id.set(tid);
                    })
                    comments_dirty=comments_dirty
                    on_thread_click=Callback::new(move |(thread_id, block_id): (String, Option<String>)| {
                        // Scroll the commented block into view and open the popup.
                        if let Some(ref bid) = block_id {
                            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                                let selector = block_id_selector(bid);
                                if let Ok(Some(el)) = doc.query_selector(&selector) {
                                    el.scroll_into_view_with_bool(true);
                                    // Position popup after scroll settles
                                    let rect = crate::components::dom_position::element_viewport_rect(&el);
                                    let (pl, pt) = crate::components::dom_position::place_left_margin(&rect, 420.0, 12.0);
                                    set_popup_left.set(pl);
                                    set_popup_top.set(pt);
                                }
                            }
                        }
                        set_popup_thread_id.set(Some(thread_id));
                    })
                />
                </div> // editor-with-panels
            </main>

            <ShareDialog
                visible=share_visible
                on_close=Callback::new(move |_| set_share_visible.set(false))
                folder_id=folder_id
                doc_id=current_id
            />

            <SearchDialog
                visible=search_visible.read_only()
                on_close=Callback::new(move |_| set_search_visible.set(false))
                scope=crate::commands::CommandScope::Editor
                initial_mode=palette_initial_mode
            />
            <crate::components::ask_dialog::AskDialog
                visible=ask_visible
                on_close=Callback::new(move |_| {
                    set_ask_visible.set(false);
                    set_ask_initial_prompt.set(None);
                    set_ask_insert_range.set(None);
                    set_ask_mode.set(crate::api::ask::AskMode::Agent);
                    set_ask_hidden_suffix.set(None);
                })
                initial_prompt=ask_initial_prompt
                ask_mode=ask_mode
                hidden_suffix=ask_hidden_suffix
                on_insert=Some(Callback::new(move |text: String| {
                    // #148: land the assistant text at the trigger
                    // range captured when `OpenAskDialog`
                    // dispatched. When no range is present (dialog
                    // opened via sidebar, not @-ask), the
                    // `on_insert` button in AskDialog doesn't
                    // render, so this branch is unreachable in
                    // that case.
                    if let Some((from, to)) = ask_insert_range.get_untracked() {
                        set_toolbar_command.set(Some(ToolbarCommand::InsertAiText {
                            from,
                            to,
                            text,
                        }));
                        set_ask_insert_range.set(None);
                        set_ask_initial_prompt.set(None);
                    }
                }))
            />

            <ConfirmDialog
                visible=confirm_delete_visible
                title=crate::t!("document-trash-dialog-title")
                message=crate::t!("document-trash-dialog-message")
                confirm_label=crate::t!("document-trash-dialog-confirm")
                destructive=true
                on_cancel=Callback::new(move |_| set_confirm_delete_visible.set(false))
                on_confirm=Callback::new(move |_| {
                    set_confirm_delete_visible.set(false);
                    let id = current_id.get_untracked();
                    leptos::task::spawn_local(async move {
                        match documents::delete_document(&id).await {
                            Ok(()) => hard_navigate("/"),
                            Err(e) => {
                                web_sys::console::error_1(&format!("Delete failed: {e}").into());
                            }
                        }
                    });
                })
            />

            <ConfirmDialog
                visible=confirm_purge_visible
                title=crate::t!("document-purge-dialog-title")
                message=crate::t!("document-purge-dialog-message")
                confirm_label=crate::t!("document-purge-dialog-confirm")
                destructive=true
                on_cancel=Callback::new(move |_| set_confirm_purge_visible.set(false))
                on_confirm=Callback::new(move |_| {
                    set_confirm_purge_visible.set(false);
                    let id = current_id.get_untracked();
                    leptos::task::spawn_local(async move {
                        match documents::purge_document(&id).await {
                            Ok(()) => hard_navigate("/"),
                            Err(e) => {
                                web_sys::console::error_1(&format!("Purge failed: {e}").into());
                            }
                        }
                    });
                })
            />

            <FolderPickerDialog
                visible=restore_picker_visible
                title=crate::t!("document-restore-folder-title")
                confirm_label=crate::t!("common-restore-here")
                on_close=Callback::new(move |_| set_restore_picker_visible.set(false))
                on_pick=Callback::new(move |folder_id: String| {
                    set_restore_picker_visible.set(false);
                    let id = current_id.get_untracked();
                    leptos::task::spawn_local(async move {
                        match documents::restore_document(&id, &folder_id).await {
                            Ok(()) => hard_navigate("/"),
                            Err(e) => {
                                web_sys::console::error_1(&format!("Restore failed: {e}").into());
                            }
                        }
                    });
                })
            />

            // #146: Move to Folder picker (distinct from the restore picker
            // above). On pick, move the doc and reflect the new folder in the
            // header signal — the doc stays open.
            // #149: "Add to folder" — adds an ADDITIONAL membership (the
            // primary is unchanged; that's what Move below is for).
            <FolderPickerDialog
                visible=add_folder_picker_visible
                title=crate::t!("document-folder-add-title")
                confirm_label=crate::t!("document-folder-add-confirm")
                on_close=Callback::new(move |_| set_add_folder_picker_visible.set(false))
                on_pick=Callback::new(move |folder_id: String| {
                    set_add_folder_picker_visible.set(false);
                    let id = current_id.get_untracked();
                    leptos::task::spawn_local(async move {
                        match documents::add_doc_to_folder(&id, &folder_id).await {
                            Ok(_) => reload_doc_folders(),
                            Err(e) => web_sys::console::error_1(
                                &format!("Add to folder failed: {e}").into(),
                            ),
                        }
                    });
                })
            />

            <FolderPickerDialog
                visible=move_picker_visible
                title=crate::t!("document-move-folder-title")
                confirm_label=crate::t!("document-move-here")
                on_close=Callback::new(move |_| set_move_picker_visible.set(false))
                on_pick=Callback::new(move |folder_id: String| {
                    set_move_picker_visible.set(false);
                    let id = current_id.get_untracked();
                    let target = folder_id.clone();
                    leptos::task::spawn_local(async move {
                        match documents::bulk_move(vec![id], &target).await {
                            Ok(_) => set_folder_id.set(Some(target)),
                            Err(e) => {
                                web_sys::console::error_1(&format!("Move failed: {e}").into());
                            }
                        }
                    });
                })
            />

            // #146 follow-up: Duplicate dialog — name + destination folder
            // (default = source folder, Home easy) + a share-permission warning.
            <DuplicateDialog
                visible=duplicate_dialog_visible
                initial_name=title
                source_folder_id=folder_id
                on_close=Callback::new(move |_| set_duplicate_dialog_visible.set(false))
                on_confirm=Callback::new(move |(new_name, folder): (String, String)| {
                    set_duplicate_dialog_visible.set(false);
                    let old_id = current_id.get_untracked();
                    let is_spreadsheet = doc_type.get_untracked() == "spreadsheet";
                    // Snapshot the *current in-memory* content. get_content(old_id)
                    // races the source's async WS persistence and could return
                    // content without the just-typed edits (duplicateCopiedContent
                    // doctor failure). The live editor_state always has them.
                    let local_bytes = editor_state
                        .get_untracked()
                        .map(|s| crate::editor::yrs_bridge::doc_to_ydoc_bytes(&s.doc));
                    leptos::task::spawn_local(async move {
                        let created = if is_spreadsheet {
                            documents::create_spreadsheet(&new_name, Some(&folder)).await
                        } else {
                            documents::create_document(&new_name, Some(&folder)).await
                        };
                        match created {
                            Ok(doc) => {
                                // Prefer the local snapshot; fall back to the server
                                // copy only if the editor state wasn't available.
                                let bytes = match local_bytes {
                                    Some(b) => Some(b),
                                    None => documents::get_content(&old_id).await.ok(),
                                };
                                if let Some(bytes) = bytes {
                                    if let Err(e) = documents::put_content(&doc.id, &bytes).await {
                                        web_sys::console::error_1(
                                            &format!("Duplicate: put_content failed: {e}").into(),
                                        );
                                    }
                                }
                                hard_navigate(&format!("/d/{}", doc.id));
                            }
                            Err(e) => web_sys::console::error_1(
                                &format!("Duplicate failed: {e}").into(),
                            ),
                        }
                    });
                })
            />

            <CursorOverlay cursors=remote_cursors scroll_tick=scroll_tick enabled=cursors_visible />

            // #139: measured line-number / page-break gutter (View toggles).
            <EditorGutterOverlay
                editor_state=editor_state
                scroll_tick=scroll_tick
                line_numbers=line_numbers_visible
                page_breaks=page_breaks_visible
            />

            // #145: floating collapse button — the only chrome left in Expand,
            // since the doc header (and its expand toggle) are hidden.
            <Show when=move || expanded.get()>
                <button
                    class="expand-collapse-fab"
                    title=crate::t!("document-expand-exit")
                    aria-label=crate::t!("document-expand-exit")
                    on:click=move |_| toggle_expand.run(())
                >"\u{2715}"</button>
            </Show>

            // #141: read-only Document Details panel.
            <DocumentDetailsDialog
                visible=details_visible
                title=title
                doc_type=doc_type
                created_at=created_at
                updated_at=updated_at
                editor_state=editor_state
                on_close=Callback::new(move |_| set_details_visible.set(false))
            />

            <Show when=move || comments_visible.get()>
                <CommentHighlights
                    threads=inline_threads
                    editor_state=editor_state
                    scroll_tick=scroll_tick
                    on_click=Callback::new(move |(thread_id, left, top): (String, f64, f64)| {
                        set_popup_thread_id.set(Some(thread_id));
                        set_popup_left.set(left);
                        set_popup_top.set(top);
                    })
                />
                <AddCommentBubble
                    editor_state=editor_state
                    threads=inline_threads
                    scroll_tick=scroll_tick
                    on_click=Callback::new(move |(block_id, left, top): (String, f64, f64)| {
                        // Open CommentPopup as a new block-level comment
                        // (no text-anchor range), mirroring the
                        // SelectionCommand::Comment path below.
                        set_popup_block_id.set(Some(block_id));
                        set_popup_anchor_start.set(None);
                        set_popup_anchor_end.set(None);
                        set_popup_left.set(left);
                        set_popup_top.set(top);
                        set_popup_is_new.set(true);
                        set_popup_thread_id.set(None);
                    })
                />
            </Show>

            <CommentPopup
                thread_id=popup_thread_id
                left=popup_left
                top=popup_top
                doc_id=current_id
                block_id=popup_block_id
                anchor_start=popup_anchor_start
                anchor_end=popup_anchor_end
                is_new=popup_is_new
                comments_dirty=comments_dirty
                on_close=Callback::new(move |_| {
                    set_popup_thread_id.set(None);
                    set_popup_is_new.set(false);
                    set_popup_block_id.set(None);
                    // Re-run the thread-load Effect so the author's own
                    // edits refresh the inline highlights and the
                    // spreadsheet cell preview (first message + reply
                    // count) without waiting for a peer event or reload.
                    set_comments_dirty.update(|n| *n += 1);
                })
                on_thread_created=Callback::new(move |_tid: String| {
                    // A new thread (e.g. a cell comment) should appear in
                    // the conversation pane and cell preview immediately.
                    set_comments_dirty.update(|n| *n += 1);
                })
                on_prev=Callback::new(move |_| {
                    navigate_comment(-1, &inline_threads, &popup_thread_id, &set_popup_thread_id, &set_popup_left, &set_popup_top);
                })
                on_next=Callback::new(move |_| {
                    navigate_comment(1, &inline_threads, &popup_thread_id, &set_popup_thread_id, &set_popup_left, &set_popup_top);
                })
            />

            <SelectionToolbar
                editor_state=editor_state
                scroll_tick=scroll_tick
                on_command=Callback::new(move |cmd: SelectionCommand| {
                    match cmd {
                        SelectionCommand::Comment => request_comment.run(()),
                    }
                })
            />

            <HistoryViewer
                visible=history_visible
                doc_id=current_id
                on_jump=Callback::new(move |()| {
                    // On phones the pane takes over the whole editor area
                    // (see responsive.css `.history-pane` ≤640px), so a
                    // jump-to-block must also close the pane — otherwise
                    // the user lands on a doc they can't see. On larger
                    // viewports the pane sits beside the editor; leave it
                    // open so the user can keep comparing versions.
                    let is_phone = web_sys::window()
                        .and_then(|w| w.match_media("(max-width: 640px)").ok().flatten())
                        .map(|m| m.matches())
                        .unwrap_or(false);
                    if is_phone {
                        set_history_visible.set(false);
                    }
                })
            />

            <BlockMenu
                visible=block_menu_visible
                on_command=on_block_command
                top=block_menu_top
            />

            <AtMenu
                visible=at_menu_visible
                query=at_menu_query
                left=at_menu_left
                top=at_menu_top
                on_select=on_at_select
                on_close=on_at_close
                highlighted=at_menu_highlighted
                set_highlighted=set_at_menu_highlighted
                results=at_menu_results
                set_results=set_at_menu_results
            />

            <AtMenu
                visible=slash_menu_visible
                query=slash_menu_query
                left=slash_menu_left
                top=slash_menu_top
                on_select=on_slash_select
                on_close=on_slash_close
                highlighted=slash_menu_highlighted
                set_highlighted=set_slash_menu_highlighted
                results=slash_menu_results
                set_results=set_slash_menu_results
                mode=crate::components::at_menu::AtMenuMode::Slash
            />
        </div>
    }
    .into_any()
}

/// #148 review finding #1: dispatch `ReplaceRange { text: "" }` to
/// clear the `@query` trigger text, then defer the block-shape
/// insert command by one microtask so Leptos treats the two
/// signal writes as separate reactive flushes. Coalescing the two
/// writes in the same tick would let the second overwrite the
/// first, leaving `@kanban` (or `@table`, etc.) uncleared in the
/// paragraph while the block still inserts below.
fn clear_at_trigger_then_dispatch(
    from: usize,
    to: usize,
    follow_up: ToolbarCommand,
    set_toolbar_command: WriteSignal<Option<ToolbarCommand>>,
) {
    set_toolbar_command.set(Some(ToolbarCommand::ReplaceRange {
        from,
        to,
        text: String::new(),
    }));
    leptos::task::spawn_local(async move {
        gloo_timers::future::TimeoutFuture::new(0).await;
        set_toolbar_command.set(Some(follow_up));
    });
}

/// #148 v2 slice 2 review finding #1: shared word-boundary
/// trigger detector for both `@` and `/` menus. Returns
/// `Some(query)` when `trigger` occurs at a word boundary in
/// `text_before` (start of text OR preceded by ASCII space /
/// newline) AND the text after the trigger contains no space.
/// Returns `None` otherwise.
///
/// The rule matches what the two pre-existing per-trigger
/// Effects were doing inline; extracting it lets a future rule
/// change (e.g. tab-counts-as-boundary, tightening the `/x/`
/// collapse) happen in exactly one place and be unit-tested
/// without a reactive context.
fn detect_menu_trigger(text_before: &str, trigger: char) -> Option<String> {
    let idx = text_before.rfind(trigger)?;
    let query = &text_before[idx + trigger.len_utf8()..];
    if query.contains(' ') {
        return None;
    }
    let before = if idx > 0 {
        text_before.as_bytes().get(idx - 1).copied()
    } else {
        Some(b' ')
    };
    if before.map_or(true, |c| c == b' ' || c == b'\n') {
        Some(query.to_string())
    } else {
        None
    }
}

/// Open the comment pane. Finds the block ID at the cursor position
/// and sets it as pending for inline comment creation.
fn open_comment_pane(
    editor_state: &ReadSignal<Option<EditorState>>,
    set_pending_block_id: &WriteSignal<Option<String>>,
    set_pending_anchor_start: &WriteSignal<Option<u32>>,
    set_pending_anchor_end: &WriteSignal<Option<u32>>,
    set_conversation_visible: &WriteSignal<bool>,
    conversation_visible: &ReadSignal<bool>,
) {
    if let Some(state) = editor_state.get_untracked() {
        let from = state.selection.from();
        let to = state.selection.to();
        if let Some(block_id) = state.doc.block_id_at(from) {
            // Compute selection offsets relative to the block's content start
            let block = crate::editor::state::find_block_at(&state.doc, from);
            let (anchor_start, anchor_end) = if from != to {
                if let Some(b) = &block {
                    let rel_from = (from.saturating_sub(b.content_start)) as u32;
                    let rel_to = (to.saturating_sub(b.content_start)).min(b.content.size()) as u32;
                    if rel_from < rel_to {
                        (Some(rel_from), Some(rel_to))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                }
            } else {
                (None, None) // cursor, no text selection
            };
            set_pending_block_id.set(Some(block_id));
            set_pending_anchor_start.set(anchor_start);
            set_pending_anchor_end.set(anchor_end);
            set_conversation_visible.set(true);
            return;
        }
    }
    set_pending_block_id.set(None);
    set_pending_anchor_start.set(None);
    set_pending_anchor_end.set(None);
    set_conversation_visible.set(!conversation_visible.get_untracked());
}

/// Navigate to the prev/next comment thread in the popup.
/// `direction`: -1 for prev, +1 for next.
fn navigate_comment(
    direction: i32,
    inline_threads: &ReadSignal<Vec<InlineThreadInfo>>,
    popup_thread_id: &ReadSignal<Option<String>>,
    set_popup_thread_id: &WriteSignal<Option<String>>,
    set_popup_left: &WriteSignal<f64>,
    set_popup_top: &WriteSignal<f64>,
) {
    let items = inline_threads.get_untracked();
    if items.is_empty() {
        return;
    }
    let current_tid = popup_thread_id.get_untracked();
    let current_idx = current_tid
        .as_ref()
        .and_then(|tid| items.iter().position(|t| t.thread_id == *tid))
        .map(|i| i as i32)
        .unwrap_or(-1);

    let count = items.len() as i32;
    let new_idx = if current_idx < 0 {
        if direction > 0 { 0 } else { count - 1 }
    } else {
        (current_idx + direction).rem_euclid(count)
    } as usize;

    let target = &items[new_idx];

    // Scroll block into view and position popup
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        let selector = block_id_selector(&target.block_id);
        if let Ok(Some(el)) = doc.query_selector(&selector) {
            el.scroll_into_view_with_bool(true);
            let rect = crate::components::dom_position::element_viewport_rect(&el);
            let (pl, pt) = crate::components::dom_position::place_left_margin(&rect, 420.0, 12.0);
            set_popup_left.set(pl);
            set_popup_top.set(pt);
        }
    }

    set_popup_thread_id.set(Some(target.thread_id.clone()));
}

/// Convert text to a URL-safe slug.
/// Check if a document contains a table node.
fn has_table(doc: &crate::editor::model::Node) -> bool {
    if let crate::editor::model::Node::Element { content, .. } = doc {
        content.children.iter().any(|c| c.node_type() == Some(crate::editor::model::NodeType::Table))
    } else {
        false
    }
}

/// Should the spreadsheet bridge skip applying a remote state because it
/// would degrade a table doc to a non-table doc?
///
/// On a brand-new spreadsheet, the server-side ydoc is still empty when
/// the WS sync handshake completes, so the post-STEP2 remote callback
/// (added in 04f3cc0 to catch peer edits that landed between the REST
/// snapshot load and the WS connect) decodes an empty doc and hands back
/// `read_doc_from_ydoc`'s bare-Paragraph fallback. Applying that would
/// overwrite the locally-initialized default table, leaving the toolbar
/// in document mode and collapsing sheet tabs on the next persist
/// round-trip. A *genuine* peer edit always carries the Table in the
/// STEP2 diff, so it passes this guard untouched.
fn remote_doc_degrades_spreadsheet(
    current: Option<&crate::editor::model::Node>,
    incoming: &crate::editor::model::Node,
) -> bool {
    !has_table(incoming) && current.is_some_and(has_table)
}

/// Create a default spreadsheet document with an empty 10x10 table.
fn create_default_spreadsheet_doc() -> crate::editor::model::Node {
    use crate::editor::model::{Fragment, Node, NodeType};
    let rows: Vec<Node> = (0..10)
        .map(|_| {
            let cells: Vec<Node> = (0..10)
                .map(|_| {
                    Node::element_with_content(
                        NodeType::TableCell,
                        Fragment::from(vec![Node::element(NodeType::Paragraph)]),
                    )
                })
                .collect();
            Node::element_with_content(NodeType::TableRow, Fragment::from(cells))
        })
        .collect();
    let table = Node::element_with_content(NodeType::Table, Fragment::from(rows));
    Node::element_with_content(NodeType::Doc, Fragment::from(vec![table]))
}

/// Full-page navigation. Using `window.location.set_href` (instead of
/// leptos_router's `use_navigate`) matches the convention used elsewhere
/// in this codebase (see home.rs, sidebar.rs, search_dialog.rs). It also
/// sidesteps a real pitfall: `use_navigate()` must be called from inside
/// a Router context — calling it from a DOM-event callback (as we do on
/// dialog confirm) silently panics and swallows the rest of the handler,
/// so the API request never fires. A hard reload also guarantees the
/// CollabClient/WebSocket/editor state are torn down cleanly after a
/// delete/restore/purge, which is what we want.
fn hard_navigate(path: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(path);
    }
}

/// `history.replaceState` to `new_path`, but only when it differs from
/// the current path (comparing pathname; query/hash are left alone).
/// The slug-reflecting effects call this on every editor dispatch —
/// including remote CRDT frames — so guarding against the no-op case is
/// what keeps us under the browser's History-API rate limit.
fn replace_url_if_changed(new_path: &str) {
    let Some(window) = web_sys::window() else { return };
    let current = window.location().pathname().unwrap_or_default();
    if current == new_path {
        return;
    }
    let _ = window.history().and_then(|h| {
        h.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(new_path))
    });
}

fn slugify(text: &str) -> String {
    let slug: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse consecutive hyphens and trim
    let mut result = String::new();
    for c in slug.chars() {
        if c == '-' && result.ends_with('-') {
            continue;
        }
        result.push(c);
    }
    let result = result.trim_matches('-').to_string();
    if result.is_empty() {
        "untitled".to_string()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::block_id_selector;
    use super::remote_doc_degrades_spreadsheet;
    use crate::editor::model::{Fragment, Node, NodeType};

    fn doc_with_table() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Table,
                Fragment::empty(),
            )]),
        )
    }

    fn paragraph_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::empty(),
            )]),
        )
    }

    // Regression for the 2026-06-12 Playwright failures (spreadsheet
    // toolbar + sheet tabs): on a brand-new spreadsheet the post-STEP2
    // remote callback decodes the still-empty server ydoc as a bare
    // Paragraph doc; the bridge must not let it overwrite the
    // locally-initialized table.
    #[test]
    fn paragraph_fallback_must_not_clobber_local_table() {
        let current = doc_with_table();
        assert!(remote_doc_degrades_spreadsheet(
            Some(&current),
            &paragraph_doc(),
        ));
    }

    #[test]
    fn genuine_peer_table_edit_is_applied() {
        // A real peer edit carries the Table in the STEP2 diff — the
        // guard must pass it through.
        let current = doc_with_table();
        assert!(!remote_doc_degrades_spreadsheet(
            Some(&current),
            &doc_with_table(),
        ));
    }

    #[test]
    fn remote_state_applies_when_no_local_state_yet() {
        // Before the init Effect has produced any editor_state, whatever
        // arrives is strictly more information — apply it.
        assert!(!remote_doc_degrades_spreadsheet(None, &paragraph_doc()));
    }

    #[test]
    fn table_arriving_over_non_table_state_is_applied() {
        // Upgrade direction (local state degraded or not yet a table,
        // remote brings the table) must never be blocked.
        let current = paragraph_doc();
        assert!(!remote_doc_degrades_spreadsheet(
            Some(&current),
            &doc_with_table(),
        ));
    }

    #[test]
    fn block_id_selector_passes_normal_ids_through() {
        assert_eq!(
            block_id_selector("ss:Sheet1:c:5:3"),
            r#"[data-block-id="ss:Sheet1:c:5:3"]"#
        );
        assert_eq!(
            block_id_selector("block_abc-123"),
            r#"[data-block-id="block_abc-123"]"#
        );
    }

    #[test]
    fn block_id_selector_escapes_quote_and_backslash() {
        // #5: a `"` or `\` in the id must be escaped so it can't break out
        // of the selector string (which would throw a DOMException and
        // silently no-op the scroll/popup lookup). Backslash is escaped
        // first so the inserted escapes aren't re-escaped.
        assert_eq!(
            block_id_selector(r#"a"]{evil}["#),
            r#"[data-block-id="a\"]{evil}["]"#
        );
        assert_eq!(
            block_id_selector(r#"a\b"#),
            r#"[data-block-id="a\\b"]"#
        );
    }

    // --- detect_menu_trigger --------------------------------------

    use super::detect_menu_trigger;

    #[test]
    fn detect_menu_trigger_fires_at_start_of_text() {
        assert_eq!(detect_menu_trigger("@", '@'), Some(String::new()));
        assert_eq!(detect_menu_trigger("/", '/'), Some(String::new()));
        assert_eq!(
            detect_menu_trigger("@alice", '@'),
            Some("alice".to_string())
        );
        assert_eq!(
            detect_menu_trigger("/table", '/'),
            Some("table".to_string())
        );
    }

    #[test]
    fn detect_menu_trigger_fires_after_space_or_newline() {
        assert_eq!(
            detect_menu_trigger("hi @bob", '@'),
            Some("bob".to_string())
        );
        assert_eq!(
            detect_menu_trigger("foo\n/date", '/'),
            Some("date".to_string())
        );
    }

    #[test]
    fn detect_menu_trigger_rejects_non_boundary() {
        // Email `foo@bar` should NOT trigger the @-menu.
        assert_eq!(detect_menu_trigger("foo@bar", '@'), None);
        // `and/or` should NOT trigger the slash-menu.
        assert_eq!(detect_menu_trigger("and/or", '/'), None);
    }

    #[test]
    fn detect_menu_trigger_rejects_query_containing_space() {
        // Once the user types a space, the trigger's over.
        assert_eq!(detect_menu_trigger("@alice foo", '@'), None);
        assert_eq!(detect_menu_trigger("/table x", '/'), None);
    }

    #[test]
    fn detect_menu_trigger_uses_last_trigger_in_text() {
        // If the user typed two triggers, only the RIGHTMOST one is
        // active (the caret is at the end). `foo @a @b` — the
        // menu tracks `@b`, not `@a`.
        assert_eq!(
            detect_menu_trigger("foo @a @b", '@'),
            Some("b".to_string())
        );
    }

    #[test]
    fn detect_menu_trigger_rejects_utf8_boundary_char() {
        // A multibyte char (é) before the trigger is not a
        // word boundary — the byte at idx-1 is a UTF-8
        // continuation, not space/newline, so no fire.
        assert_eq!(detect_menu_trigger("café/table", '/'), None);
        assert_eq!(detect_menu_trigger("café@name", '@'), None);
    }
}
