// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;

use crate::api::client;
use crate::api::documents;
use crate::api::folders::{self, ChildResponse, FolderResponse};
use crate::components::confirm_dialog::ConfirmDialog;
use crate::components::file_browser::FileBrowser;
use crate::components::folder_picker::FolderPickerDialog;
use crate::components::search_dialog::SearchDialog;

#[component]
pub fn HomePage() -> impl IntoView {
    // #152: shared shell state (the sidebar + drawer live in `AppShell`).
    // search/ask/drawer signals are aliased to ctx fields below so the
    // existing call sites are unchanged; the Home nav reset is registered
    // into `ctx.home_reset` while this page is mounted.
    let ctx = use_context::<crate::components::app_shell::ShellCtx>()
        .expect("ShellCtx provided by AppShell");

    // #152: client-side navigation for opening/creating documents — keeps the
    // persistent shell mounted (no full reload, no flash). Bound here at
    // component setup (use_navigate must not be called from a DOM callback);
    // the returned closure is cloned into each handler.
    let navigate = use_navigate();

    let (folder, set_folder) = signal::<Option<FolderResponse>>(None);
    let (error, set_error) = signal::<Option<String>>(None);
    let (home_folder_id, set_home_folder_id) = signal::<Option<String>>(None);
    let (trash_folder_id, set_trash_folder_id) = signal::<Option<String>>(None);
    // #142: id of the user's Private system folder. Splices in as a synthetic
    // row under Home so template copies (and anything else routed to Private)
    // is reachable from the file browser.
    let (private_folder_id, set_private_folder_id) = signal::<Option<String>>(None);
    // #152: search/ask visibility live in the shell ctx (set by the sidebar
    // Search/Ask entries); the dialogs themselves stay mounted on this page.
    let search_visible = ctx.search_open;
    let set_search_visible = ctx.search_open;
    // Phase 6 M-6.2 piece B: Ask dialog visibility. Opened from the
    // sidebar entry; reset on close.
    let ask_visible = ctx.ask_open;
    let set_ask_visible = ctx.ask_open;
    // M-P4 piece B: Ctrl+K (Search) and Ctrl+Shift+P (Action)
    // share the dialog. The keydown handler below writes here
    // before flipping search_visible; the dialog reads it on open.
    let (palette_initial_mode, set_palette_initial_mode) =
        signal(crate::components::search_dialog::PaletteMode::Search);
    // M-P5 piece D: drag-over state for the home-page import drop
    // zone. `drag_depth` debounces the overlay across child-element
    // dragleave events — incrementing on dragenter and decrementing
    // on dragleave gives us a "still dragging within the page"
    // boolean without flicker as the cursor crosses child borders.
    let (drag_depth, set_drag_depth) = signal(0i32);
    let dragging = Signal::derive(move || drag_depth.get() > 0);
    let (import_error, set_import_error) = signal::<Option<String>>(None);

    // Phase 5 M-P7 piece C: selected doc ids drive the bulk action
    // bar. Folder ids never enter this set — bulk move-folder /
    // delete-folder isn't in v1; the FileBrowser hides checkboxes
    // for folder rows. The hashset stays in-memory only; navigation
    // away clears it via the on_cleanup of the page's signals.
    let (selected_ids, set_selected_ids) =
        signal::<std::collections::HashSet<String>>(std::collections::HashSet::new());
    let (bulk_error, set_bulk_error) = signal::<Option<String>>(None);
    let (confirm_bulk_delete, set_confirm_bulk_delete) = signal(false);
    // #152: drawer state + backdrop live in the shell; the header hamburger
    // toggles it.
    let set_mobile_sidebar_open = ctx.drawer_open;
    // Clickable breadcrumb trail Home → … → current folder. Pushed on
    // descent via FileBrowser clicks, truncated on crumb click.
    let (breadcrumbs, set_breadcrumbs) = signal::<Vec<Crumb>>(Vec::new());
    // Targets for the shared Restore / Delete-forever dialogs (trash view).
    let (restore_target_id, set_restore_target_id) = signal::<Option<String>>(None);
    let (purge_target_id, set_purge_target_id) = signal::<Option<String>>(None);
    // #150: folder pending a delete confirmation (only set once we've
    // confirmed the folder is empty — we never orphan its contents).
    let (delete_folder_target, set_delete_folder_target) = signal::<Option<String>>(None);

    if !client::is_authenticated() {
        let navigate = use_navigate();
        navigate("/login", Default::default());
        return view! { <div>{crate::t!("common-redirecting-login")}</div> }.into_any();
    }

    // Global Ctrl+K / Cmd+K keyboard shortcut. Registered on the document
    // so it fires regardless of focus, and removed on `HomePage` unmount
    // so soft-navigating Home → Doc → Home doesn't accumulate stale
    // listeners (each one would fire on every keystroke for the rest of
    // the session, writing into a dead reactive scope).
    //
    // MUST be `Fn`, not `FnMut`. This listener fires on EVERY document
    // keydown — Ctrl+K is rare, but the FnMut wrapper takes the RefCell
    // guard on every entry. If anything in the call chain (signal write,
    // focus shift, dialog mount) synchronously re-enters keydown, we
    // panic with "closure invoked recursively or after being dropped".
    // Body only writes a signal (`WriteSignal::set` takes `&self`), so
    // `Fn` is sufficient and re-entry-safe.
    {
        let set_vis = set_search_visible;
        let set_mode = set_palette_initial_mode;
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let cb = Closure::<dyn Fn(web_sys::KeyboardEvent)>::wrap(Box::new(
                move |e: web_sys::KeyboardEvent| {
                    let ctrl_or_meta = e.ctrl_key() || e.meta_key();
                    if ctrl_or_meta && e.key().to_lowercase() == "k" {
                        e.prevent_default();
                        set_mode.set(
                            crate::components::search_dialog::PaletteMode::Search,
                        );
                        set_vis.set(true);
                    }
                    // Phase 5 M-P4 piece B: Ctrl+Shift+P opens the
                    // palette directly in Action mode.
                    if ctrl_or_meta && e.shift_key() && e.key().to_lowercase() == "p" {
                        e.prevent_default();
                        set_mode.set(
                            crate::components::search_dialog::PaletteMode::Action,
                        );
                        set_vis.set(true);
                    }
                },
            ));
            let cb_js = cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let _ = doc.add_event_listener_with_callback("keydown", &cb_js);
            // Leak the Closure so the JS-side function pointer stays
            // valid for the lifetime of the listener; the explicit
            // removeEventListener in on_cleanup is what actually detaches
            // the handler. Cumulative leak is sub-KB per Home visit.
            cb.forget();
            let doc_for_cleanup = doc.clone();
            leptos::prelude::on_cleanup(move || {
                let _ = doc_for_cleanup
                    .remove_event_listener_with_callback("keydown", &cb_js);
            });
        }
    }

    // M-P5 piece D: document-level drag/drop import. Three listeners
    // (dragenter / dragleave / drop) coordinate the overlay state +
    // file handoff. `dragover` also gets a preventDefault listener
    // because the browser otherwise rejects the drop entirely.
    //
    // Mirrors the keydown-listener leak/cleanup pattern above —
    // each Closure is `forget()`-ed to keep the JS-side function
    // pointer alive, and the matching removeEventListener fires on
    // HomePage unmount.
    {
        let nav = use_navigate();
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            // dragover — required for the browser to fire `drop`.
            let dragover_cb = Closure::<dyn Fn(web_sys::DragEvent)>::wrap(Box::new(
                move |e: web_sys::DragEvent| {
                    // Only intercept events that carry a file
                    // payload; ignore drag-and-drop of text or
                    // internal UI elements.
                    if drag_carries_file(&e) {
                        e.prevent_default();
                    }
                },
            ));
            let dragenter_cb = Closure::<dyn Fn(web_sys::DragEvent)>::wrap(Box::new(
                move |e: web_sys::DragEvent| {
                    if drag_carries_file(&e) {
                        e.prevent_default();
                        set_drag_depth.update(|n| *n += 1);
                    }
                },
            ));
            let dragleave_cb = Closure::<dyn Fn(web_sys::DragEvent)>::wrap(Box::new(
                move |e: web_sys::DragEvent| {
                    if drag_carries_file(&e) {
                        set_drag_depth.update(|n| {
                            if *n > 0 { *n -= 1 }
                        });
                    }
                },
            ));
            let drop_cb = {
                let nav = nav.clone();
                Closure::<dyn Fn(web_sys::DragEvent)>::wrap(Box::new(
                    move |e: web_sys::DragEvent| {
                        if !drag_carries_file(&e) {
                            return;
                        }
                        e.prevent_default();
                        set_drag_depth.set(0);
                        let Some(dt) = e.data_transfer() else { return };
                        let Some(files) = dt.files() else { return };
                        if files.length() == 0 {
                            return;
                        }
                        let Some(file) = files.get(0) else { return };
                        let target_folder = folder.get_untracked().map(|f| f.id);
                        let nav = nav.clone();
                        leptos::task::spawn_local(async move {
                            // Binary formats (DOCX, PDF) take the async
                            // import-job path: the conversion runs on the
                            // worker, so show a progress message in the
                            // toast region while we poll for the result.
                            // The text formats import synchronously.
                            let lowered = file.name().to_lowercase();
                            if lowered.ends_with(".docx") || lowered.ends_with(".pdf") {
                                set_import_error.set(Some(format!(
                                    "Converting {}… this can take a few seconds.",
                                    file.name()
                                )));
                                match import_dropped_async(&file, target_folder.as_deref()).await {
                                    Ok(doc_id) => {
                                        set_import_error.set(None);
                                        nav(&format!("/d/{doc_id}/doc"), Default::default());
                                    }
                                    Err(msg) => set_import_error.set(Some(msg)),
                                }
                            } else {
                                match import_dropped_file(&file, target_folder.as_deref()).await {
                                    Ok(doc) => {
                                        set_import_error.set(None);
                                        nav(&format!("/d/{}/doc", doc.id), Default::default());
                                    }
                                    Err(msg) => set_import_error.set(Some(msg)),
                                }
                            }
                        });
                    },
                ))
            };
            let dragover_fn = dragover_cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let dragenter_fn = dragenter_cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let dragleave_fn = dragleave_cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let drop_fn = drop_cb.as_ref().unchecked_ref::<js_sys::Function>().clone();
            let _ = doc.add_event_listener_with_callback("dragover", &dragover_fn);
            let _ = doc.add_event_listener_with_callback("dragenter", &dragenter_fn);
            let _ = doc.add_event_listener_with_callback("dragleave", &dragleave_fn);
            let _ = doc.add_event_listener_with_callback("drop", &drop_fn);
            dragover_cb.forget();
            dragenter_cb.forget();
            dragleave_cb.forget();
            drop_cb.forget();
            let doc_for_cleanup = doc.clone();
            leptos::prelude::on_cleanup(move || {
                let _ = doc_for_cleanup.remove_event_listener_with_callback("dragover", &dragover_fn);
                let _ = doc_for_cleanup.remove_event_listener_with_callback("dragenter", &dragenter_fn);
                let _ = doc_for_cleanup.remove_event_listener_with_callback("dragleave", &dragleave_fn);
                let _ = doc_for_cleanup.remove_event_listener_with_callback("drop", &drop_fn);
            });
        }
    }

    // Load user info and home folder on mount.
    {
        let set_folder = set_folder.clone();
        let set_error = set_error.clone();
        let set_home_folder_id = set_home_folder_id.clone();
        let set_trash_folder_id = set_trash_folder_id.clone();
        let set_private_folder_id = set_private_folder_id.clone();
        let set_breadcrumbs = set_breadcrumbs.clone();
        leptos::task::spawn_local(async move {
            match client::api_get::<UserMeResponse>("/users/me").await {
                Ok(user) => {
                    set_home_folder_id.set(Some(user.home_folder_id.clone()));
                    set_trash_folder_id.set(Some(user.trash_folder_id.clone()));
                    set_private_folder_id.set(Some(user.private_folder_id.clone()));
                    // The /trash route renders this same page; open the
                    // Trash folder instead of Home when we land there
                    // (#104). The trash folder carries `is_trash`, which
                    // drives the trash-mode view; the synthetic Trash row
                    // is only spliced into Home, so skip it here.
                    let on_trash = web_sys::window()
                        .and_then(|w| w.location().pathname().ok())
                        .map(|p| p == "/trash")
                        .unwrap_or(false);
                    let target = if on_trash {
                        &user.trash_folder_id
                    } else {
                        &user.home_folder_id
                    };
                    match folders::get_folder(target).await {
                        Ok(f) => {
                            set_breadcrumbs.set(vec![Crumb {
                                id: f.id.clone(),
                                title: f.title.clone(),
                            }]);
                            let f = if on_trash {
                                f
                            } else {
                                let f = splice_trash_row(f, &user.trash_folder_id);
                                splice_private_row(f, &user.private_folder_id)
                            };
                            set_folder.set(Some(f));
                        }
                        Err(e) => set_error.set(Some(e.to_string())),
                    }
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    }

    // Reload the currently-viewed folder. Splices the Trash row back in if we
    // happen to be viewing Home.
    let refresh_folder = move || {
        let current = folder.get_untracked();
        let Some(current_id) = current.as_ref().map(|f| f.id.clone()) else {
            return;
        };
        let home_id = home_folder_id.get_untracked();
        let trash_id = trash_folder_id.get_untracked();
        let private_id = private_folder_id.get_untracked();
        leptos::task::spawn_local(async move {
            match folders::get_folder(&current_id).await {
                Ok(mut f) => {
                    if let (Some(home), Some(trash)) = (&home_id, &trash_id) {
                        if &f.id == home {
                            f = splice_trash_row(f, trash);
                            if let Some(private) = &private_id {
                                f = splice_private_row(f, private);
                            }
                        }
                    }
                    set_folder.set(Some(f));
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    };

    let on_create_document = {
        let navigate = navigate.clone();
        move |_| {
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                match documents::create_document("Untitled", None).await {
                    Ok(doc) => {
                        navigate(&format!("/d/{}/untitled", doc.id), Default::default());
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to create document: {e}").into(),
                        );
                    }
                }
            });
        }
    };

    let on_create_spreadsheet = {
        let navigate = navigate.clone();
        move |_| {
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                match documents::create_spreadsheet("Untitled Spreadsheet", None).await {
                    Ok(doc) => {
                        navigate(
                            &format!("/d/{}/untitled-spreadsheet", doc.id),
                            Default::default(),
                        );
                    }
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to create spreadsheet: {e}").into(),
                        );
                    }
                }
            });
        }
    };

    let on_create_folder = {
        let refresh = refresh_folder.clone();
        move |_| {
            let refresh = refresh.clone();
            let set_error = set_error.clone();
            leptos::task::spawn_local(async move {
                match folders::create_folder("New Folder", None).await {
                    Ok(_) => refresh(),
                    Err(e) => set_error.set(Some(e.to_string())),
                }
            });
        }
    };

    // Descending navigation into a folder: push a new crumb. Callers can
    // assume the clicked folder is a direct child of the currently-viewed
    // folder (FileBrowser only shows direct children).
    let on_navigate_folder = Callback::new(move |folder_id: String| {
        let home_id = home_folder_id.get_untracked();
        let trash_id = trash_folder_id.get_untracked();
        let private_id = private_folder_id.get_untracked();
        leptos::task::spawn_local(async move {
            match folders::get_folder(&folder_id).await {
                Ok(mut f) => {
                    if let (Some(home), Some(trash)) = (&home_id, &trash_id) {
                        if &f.id == home {
                            f = splice_trash_row(f, trash);
                            if let Some(private) = &private_id {
                                f = splice_private_row(f, private);
                            }
                        }
                    }
                    set_breadcrumbs.update(|b| {
                        b.push(Crumb {
                            id: f.id.clone(),
                            title: f.title.clone(),
                        });
                    });
                    set_folder.set(Some(f));
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    });

    // Click on an intermediate breadcrumb: truncate the trail and load
    // that folder. Index 0 is Home.
    let on_breadcrumb = Callback::new(move |index: usize| {
        let crumbs = breadcrumbs.get_untracked();
        if index >= crumbs.len() {
            return;
        }
        let target_id = crumbs[index].id.clone();
        let home_id = home_folder_id.get_untracked();
        let trash_id = trash_folder_id.get_untracked();
        let private_id = private_folder_id.get_untracked();
        set_breadcrumbs.update(|b| b.truncate(index + 1));
        leptos::task::spawn_local(async move {
            match folders::get_folder(&target_id).await {
                Ok(mut f) => {
                    if let (Some(home), Some(trash)) = (&home_id, &trash_id) {
                        if &f.id == home {
                            f = splice_trash_row(f, trash);
                            if let Some(private) = &private_id {
                                f = splice_private_row(f, private);
                            }
                        }
                    }
                    set_folder.set(Some(f));
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    });

    let on_open_document = Callback::new({
        let navigate = navigate.clone();
        move |doc_id: String| {
            navigate(&format!("/d/{doc_id}/doc"), Default::default());
        }
    });

    // M-P7 piece C: checkbox click → toggle the id in/out of the
    // selection. Standalone callback so both the normal and trash
    // FileBrowser invocations can share it; refresh-on-bulk wipes
    // the set when an op succeeds.
    let on_toggle_select = Callback::new(move |doc_id: String| {
        set_selected_ids.update(|s| {
            if !s.remove(&doc_id) {
                s.insert(doc_id);
            }
        });
    });

    // Selection bar action: bulk delete. Confirmation modal gates
    // the actual call so an accidental Delete on a 50-doc selection
    // isn't a single-keypress regret.
    let refresh_after_bulk = {
        let refresh = refresh_folder.clone();
        move || {
            set_selected_ids.set(std::collections::HashSet::new());
            refresh();
        }
    };
    let on_bulk_delete_confirm = Callback::new({
        let refresh_after = refresh_after_bulk.clone();
        move |_: ()| {
            let ids: Vec<String> = selected_ids.get_untracked().into_iter().collect();
            if ids.is_empty() {
                set_confirm_bulk_delete.set(false);
                return;
            }
            let refresh_after = refresh_after.clone();
            leptos::task::spawn_local(async move {
                match documents::bulk_delete(ids).await {
                    Ok(resp) => {
                        if resp.failed > 0 {
                            set_bulk_error.set(Some(format!(
                                "Deleted {} of {} — {} failed",
                                resp.succeeded,
                                resp.succeeded + resp.failed,
                                resp.failed,
                            )));
                        } else {
                            set_bulk_error.set(None);
                        }
                        refresh_after();
                    }
                    Err(e) => set_bulk_error.set(Some(e.to_string())),
                }
                set_confirm_bulk_delete.set(false);
            });
        }
    });

    let on_restore_row = Callback::new(move |doc_id: String| {
        set_restore_target_id.set(Some(doc_id));
    });
    let on_purge_row = Callback::new(move |doc_id: String| {
        set_purge_target_id.set(Some(doc_id));
    });

    // #150: rename a folder via a simple prompt (mirrors the document rename
    // flow), then refresh the listing so the new name shows immediately.
    let on_rename_folder = Callback::new({
        let refresh = refresh_folder.clone();
        move |(folder_id, current): (String, String)| {
            let Some(window) = web_sys::window() else { return };
            let Ok(Some(entered)) = window.prompt_with_message_and_default(
                &crate::t!("folder-rename-prompt"),
                &current,
            ) else {
                return;
            };
            let trimmed = entered.trim().to_string();
            if trimmed.is_empty() || trimmed == current {
                return;
            }
            let refresh = refresh.clone();
            leptos::task::spawn_local(async move {
                match folders::update_folder(&folder_id, Some(&trimmed), None).await {
                    Ok(()) => refresh(),
                    Err(e) => {
                        web_sys::console::error_1(&format!("Rename failed: {e}").into())
                    }
                }
            });
        }
    });

    // #150: delete a folder — but only when it's empty, so contained
    // documents are never orphaned (the backend doesn't cascade). Fetch the
    // folder first: non-empty → surface a clear "move/remove contents" notice
    // and stop; empty → open the confirmation dialog.
    let on_delete_folder = Callback::new(move |folder_id: String| {
        leptos::task::spawn_local(async move {
            match folders::get_folder(&folder_id).await {
                Ok(f) => {
                    if f.children.is_empty() {
                        set_delete_folder_target.set(Some(folder_id));
                    } else {
                        set_error.set(Some(crate::t!("folder-delete-not-empty")));
                    }
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    });

    // The dialogs' close handlers (search/ask open is driven by the sidebar
    // in the shell now, so the open-callbacks moved there).
    let close_search = Callback::new(move |()| set_search_visible.set(false));
    let close_ask = Callback::new(move |()| set_ask_visible.set(false));

    // #152: the Home nav reset. Registered into `ctx.home_reset` while this
    // page is mounted; the shell's Home handler runs it (so Home-on-Home
    // resets in place) and otherwise client-side-navigates to "/". Because
    // the sidebar lives in the persistent shell, navigation never remounts
    // it — so this only needs to fix up the page's OWN in-memory state:
    // a true no-op when already showing the home root, an in-memory reload
    // when in a subfolder (URL still "/"), and a route change only for the
    // /trash → / case (which remounts just the outlet content, not the
    // sidebar).
    let nav_home = use_navigate();
    let home_reset = Callback::new(move |()| {
        let on_root_url = web_sys::window()
            .and_then(|w| w.location().pathname().ok())
            .map(|p| p == "/")
            .unwrap_or(false);
        let Some(home_id) = home_folder_id.get_untracked() else {
            // Home id not resolved yet — fall back to a navigate so the
            // remounted page loads it. No-op if we're already on "/".
            if !on_root_url {
                nav_home("/", Default::default());
            }
            return;
        };
        // Already on "/" AND already showing the home root → nothing to do.
        // A true no-op means zero re-render and zero flash.
        let showing_home = folder.get_untracked().map(|f| f.id) == Some(home_id.clone());
        if on_root_url && showing_home {
            return;
        }
        // Only the /trash route needs the URL changed; that swap remounts
        // (unavoidable while /trash is its own route) but it's not the
        // already-on-Home case the flash report is about.
        if !on_root_url {
            nav_home("/", Default::default());
        }
        let trash_id = trash_folder_id.get_untracked();
        let private_id = private_folder_id.get_untracked();
        leptos::task::spawn_local(async move {
            match folders::get_folder(&home_id).await {
                Ok(mut f) => {
                    set_breadcrumbs.set(vec![Crumb {
                        id: f.id.clone(),
                        title: f.title.clone(),
                    }]);
                    if let Some(trash) = &trash_id {
                        f = splice_trash_row(f, trash);
                    }
                    if let Some(private) = &private_id {
                        f = splice_private_row(f, private);
                    }
                    set_folder.set(Some(f));
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    });
    // Register the reset while this page owns the home view; clear it on
    // unmount so the shell's Home nav falls back to a plain navigate("/")
    // from other pages.
    ctx.home_reset.set(Some(home_reset));
    on_cleanup(move || ctx.home_reset.set(None));

    // Phase 6 M-6.2 piece C: install the ask_bridge so the
    // Global-scoped `ask.open` palette command can flip the dialog
    // open. Mirror of the editor_bridge pattern. Cleared on unmount
    // so a navigation to a no-Ask page (admin) silences the
    // palette command instead of opening a now-detached dialog.
    Effect::new(move |_| {
        crate::commands::ask_bridge::set_ask_open(Some(Callback::new(move |()| {
            set_ask_visible.set(true);
        })));
    });
    on_cleanup(|| {
        crate::commands::ask_bridge::set_ask_open(None);
    });

    let in_trash = Signal::derive(move || folder.with(|f| f.as_ref().map(|f| f.is_trash).unwrap_or(false)));

    view! {
        // #152: the sidebar + drawer live in `AppShell`; this page renders as
        // the Outlet content. `<main class="main-content">` is the flex
        // sibling of the persistent sidebar; the dialogs/overlays below are
        // position:fixed so they don't participate in the flex row.
        <>
            <main id="main-content" tabindex="-1" class="main-content">
                <div class="file-browser">
                    <div class="action-bar">
                        <button
                            class="mobile-menu-toggle"
                            aria-label=crate::t!("common-open-navigation")
                            on:click=move |_| set_mobile_sidebar_open.update(|v| *v = !*v)
                        >"\u{2630}"</button>
                        <button class="btn btn-primary" on:click=on_create_document>
                            {crate::t!("home-new-document")}
                        </button>
                        // #142: open the shell-mounted template picker. The
                        // copy + navigate happens inside the modal so the
                        // same picker serves every entry point.
                        <button
                            class="btn btn-secondary"
                            on:click=move |_| ctx.template_picker_open.set(true)
                        >
                            {crate::t!("home-new-from-template")}
                        </button>
                        <button class="btn btn-primary" on:click=on_create_spreadsheet>
                            {crate::t!("home-new-spreadsheet")}
                        </button>
                        <button class="btn btn-secondary" on:click=on_create_folder>
                            {crate::t!("home-new-folder")}
                        </button>
                    </div>

                    {move || error.get().map(|e| view! {
                        <div role="alert" style="color: var(--color-error); margin-bottom: var(--space-md);">
                            {e}
                        </div>
                    })}

                    <nav class="breadcrumbs" aria-label=crate::t!("a11y-breadcrumb-label")>
                        {move || {
                            let crumbs = breadcrumbs.get();
                            let last = crumbs.len().saturating_sub(1);
                            crumbs.into_iter().enumerate().map(|(i, c)| {
                                let title = c.title.clone();
                                let is_last = i == last;
                                let sep = if i > 0 {
                                    view! {
                                        <span class="breadcrumb-sep">" / "</span>
                                    }.into_any()
                                } else {
                                    ().into_any()
                                };
                                let crumb = if is_last {
                                    view! {
                                        <span class="breadcrumb-current" aria-current="page">{title}</span>
                                    }.into_any()
                                } else {
                                    view! {
                                        <span
                                            class="breadcrumb-link"
                                            on:click=move |_| on_breadcrumb.run(i)
                                        >{title}</span>
                                    }.into_any()
                                };
                                view! { <>{sep}{crumb}</> }
                            }).collect::<Vec<_>>()
                        }}
                    </nav>

                    {move || if in_trash.get() {
                        view! {
                            <FileBrowser
                                folder=folder
                                on_navigate_folder=on_navigate_folder
                                on_open_document=on_open_document
                                trash_mode=true
                                on_restore=on_restore_row
                                on_purge=on_purge_row
                                selected_ids=selected_ids
                                on_toggle_select=on_toggle_select
                            />
                        }.into_any()
                    } else {
                        view! {
                            <FileBrowser
                                folder=folder
                                on_navigate_folder=on_navigate_folder
                                on_open_document=on_open_document
                                selected_ids=selected_ids
                                on_toggle_select=on_toggle_select
                                on_rename_folder=on_rename_folder
                                on_delete_folder=on_delete_folder
                                protected_folder_ids=vec![
                                    trash_folder_id.get().unwrap_or_default(),
                                    private_folder_id.get().unwrap_or_default(),
                                ]
                            />
                        }.into_any()
                    }}
                </div>
            </main>
            <SearchDialog
                visible=search_visible.read_only()
                on_close=close_search
                scope=crate::commands::CommandScope::Home
                initial_mode=palette_initial_mode
            />
            // Phase 6 M-6.2 piece B: Ask agentic dialog. Hidden by
            // default; opened from the sidebar Ask entry.
            <crate::components::ask_dialog::AskDialog
                visible=Signal::from(ask_visible)
                on_close=close_ask
            />

            // M-P5 piece D — full-page drop overlay. Mounted only
            // while a file is being dragged over the window so it
            // doesn't intercept clicks otherwise. The actual drop
            // listener is on the document; the overlay is purely
            // a visual "yes, you can drop here" affordance.
            <Show when=move || dragging.get()>
                <div class="home-drop-overlay">
                    <div class="home-drop-overlay-body">
                        <div class="home-drop-overlay-icon">"\u{1F4C4}"</div>
                        <div class="home-drop-overlay-title">
                            {crate::t!("home-drop-title")}
                        </div>
                        <div class="home-drop-overlay-hint">
                            {crate::t!("home-drop-hint")}
                        </div>
                    </div>
                </div>
            </Show>

            // Import error toast — visible when the most recent
            // drop produced an actionable failure (unsupported
            // type, oversize, server error). Auto-dismisses on
            // click; cleared by the next successful import.
            <Show when=move || import_error.get().is_some()>
                <div
                    class="home-import-error"
                    on:click=move |_| set_import_error.set(None)
                    role="alert"
                >
                    {move || import_error.get().unwrap_or_default()}
                </div>
            </Show>

            <FolderPickerDialog
                visible=Signal::derive(move || restore_target_id.get().is_some())
                title=crate::t!("document-restore-folder-title")
                confirm_label=crate::t!("common-restore-here")
                on_close=Callback::new(move |_| set_restore_target_id.set(None))
                on_pick=Callback::new({
                    let refresh = refresh_folder.clone();
                    move |target: String| {
                        let Some(doc_id) = restore_target_id.get_untracked() else { return };
                        set_restore_target_id.set(None);
                        let refresh = refresh.clone();
                        leptos::task::spawn_local(async move {
                            match documents::restore_document(&doc_id, &target).await {
                                Ok(()) => refresh(),
                                Err(e) => web_sys::console::error_1(
                                    &format!("Restore failed: {e}").into(),
                                ),
                            }
                        });
                    }
                })
            />

            <ConfirmDialog
                visible=Signal::derive(move || purge_target_id.get().is_some())
                title=crate::t!("document-purge-dialog-title")
                message=crate::t!("document-purge-dialog-message")
                confirm_label=crate::t!("document-purge-dialog-confirm")
                destructive=true
                on_cancel=Callback::new(move |_| set_purge_target_id.set(None))
                on_confirm=Callback::new({
                    let refresh = refresh_folder.clone();
                    move |_| {
                        let Some(doc_id) = purge_target_id.get_untracked() else { return };
                        set_purge_target_id.set(None);
                        let refresh = refresh.clone();
                        leptos::task::spawn_local(async move {
                            match documents::purge_document(&doc_id).await {
                                Ok(()) => refresh(),
                                Err(e) => web_sys::console::error_1(
                                    &format!("Purge failed: {e}").into(),
                                ),
                            }
                        });
                    }
                })
            />

            // #150: confirm deleting an (empty) folder.
            <ConfirmDialog
                visible=Signal::derive(move || delete_folder_target.get().is_some())
                title=crate::t!("folder-delete-dialog-title")
                message=crate::t!("folder-delete-dialog-message")
                confirm_label=crate::t!("folder-delete-dialog-confirm")
                destructive=true
                on_cancel=Callback::new(move |_| set_delete_folder_target.set(None))
                on_confirm=Callback::new({
                    let refresh = refresh_folder.clone();
                    move |_| {
                        let Some(folder_id) = delete_folder_target.get_untracked() else { return };
                        set_delete_folder_target.set(None);
                        let refresh = refresh.clone();
                        leptos::task::spawn_local(async move {
                            match folders::delete_folder(&folder_id).await {
                                Ok(()) => refresh(),
                                Err(e) => web_sys::console::error_1(
                                    &format!("Folder delete failed: {e}").into(),
                                ),
                            }
                        });
                    }
                })
            />

            // M-P7 piece C — selection bar. Appears at top of
            // viewport whenever the user has > 0 docs selected; the
            // count is the only visual; Delete fires the bulk-delete
            // confirm modal; Cancel clears the selection. Move /
            // Share buttons are placeholders for the v1 push — they
            // wire the bulk_move / bulk_share API helpers but the
            // pickers (folder destination, recipient user) need
            // additional UI scope.
            <Show when=move || !selected_ids.with(|s| s.is_empty())>
                <div class="bulk-selection-bar" role="region" aria-live="polite">
                    <span class="bulk-selection-count">
                        {move || crate::t!(
                            "bulk-selection-count",
                            count = selected_ids.with(|s| s.len()) as i64,
                        )}
                    </span>
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| set_selected_ids.set(std::collections::HashSet::new())
                    >{crate::t!("bulk-selection-cancel")}</button>
                    <button
                        class="btn btn-danger"
                        on:click=move |_| set_confirm_bulk_delete.set(true)
                    >{crate::t!("bulk-selection-delete")}</button>
                </div>
            </Show>

            // Confirm dialog for bulk delete. The message text is
            // static rather than reactive — the selection-count
            // surface is already in the bar above; the dialog just
            // needs to gate the destructive op.
            <ConfirmDialog
                visible=confirm_bulk_delete
                title=crate::t!("bulk-delete-dialog-title")
                message=crate::t!("bulk-delete-dialog-message")
                confirm_label=crate::t!("bulk-delete-dialog-confirm")
                destructive=true
                on_cancel=Callback::new(move |_| set_confirm_bulk_delete.set(false))
                on_confirm=Callback::new(move |_| on_bulk_delete_confirm.run(()))
            />

            // Error toast for failed bulk ops (also shown when a
            // partial-failure result has > 0 failed entries).
            <Show when=move || bulk_error.get().is_some()>
                <div
                    class="home-import-error"
                    on:click=move |_| set_bulk_error.set(None)
                    role="alert"
                >
                    {move || bulk_error.get().unwrap_or_default()}
                </div>
            </Show>
        </>
    }
    .into_any()
}

/// Append a synthetic Trash row to the Home folder's children so the user has
/// somewhere to click from the file browser. Trash is not a real child of Home
/// in storage (its parent_id is None), so we splice it in client-side.
fn splice_trash_row(mut home: FolderResponse, trash_id: &str) -> FolderResponse {
    if home.children.iter().any(|c| c.child_id == trash_id) {
        return home;
    }
    home.children.push(ChildResponse {
        child_id: trash_id.to_string(),
        child_type: "folder".to_string(),
        title: "Trash".to_string(),
        added_at: home.created_at,
        is_deleted: false,
    });
    home
}

/// #142: same splice as `splice_trash_row`, for Private. Template copies
/// (and any other flow that routes to Private) live behind this — without
/// the splice there's no path to Private in the file-browser UI at all,
/// so a copy lands "somewhere the user can't reach."
fn splice_private_row(mut home: FolderResponse, private_id: &str) -> FolderResponse {
    if home.children.iter().any(|c| c.child_id == private_id) {
        return home;
    }
    home.children.push(ChildResponse {
        child_id: private_id.to_string(),
        child_type: "folder".to_string(),
        title: "Private".to_string(),
        added_at: home.created_at,
        is_deleted: false,
    });
    home
}

#[derive(Clone)]
struct Crumb {
    id: String,
    title: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserMeResponse {
    home_folder_id: String,
    trash_folder_id: String,
    // #142: needed to surface Private in the Home view — template copies
    // land there by default (per design/templates.md), and without a UI
    // path Private is effectively hidden.
    private_folder_id: String,
}

// ─── Drag-and-drop import (Phase 5 M-P5 piece D) ─────────────────

/// Client-side cap on dropped-file size. Matches the route's 1 MB
/// body limit; rejecting early avoids the round-trip + the user
/// staring at a hanging UI while a 50 MB document slowly fails.
const IMPORT_MAX_BYTES: u64 = 1024 * 1024;

/// Binary imports (DOCX, PDF) get a larger cap than the text formats —
/// the import-job route allows 10 MB (matching the binary XLSX import),
/// and a real Word/PDF document with formatting/images routinely
/// exceeds the 1 MB text limit.
const ASYNC_IMPORT_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// True when a DragEvent's DataTransfer advertises a file payload
/// (rather than text / an internal UI element). Drag/drop fires
/// on every keystroke-equivalent gesture inside the page; we only
/// want to react when the user is dragging something importable.
///
/// Inspects `types`: the spec puts `"Files"` in the list when the
/// drag has at least one file. Avoids reading `files` directly,
/// which can be empty during `dragenter`/`dragover` even when the
/// drop will carry one (browsers fill it in late).
fn drag_carries_file(e: &web_sys::DragEvent) -> bool {
    let Some(dt) = e.data_transfer() else { return false };
    let types = dt.types();
    for i in 0..types.length() {
        if let Some(s) = types.get(i).as_string() {
            if s == "Files" {
                return true;
            }
        }
    }
    false
}

/// Read a dropped file and POST `/api/v1/documents/import`. The
/// caller is the document-level `drop` listener; on success the
/// caller navigates to the new doc.
///
/// Format detection is filename-extension-only — same posture as
/// every other "drag a file in" UI. The route accepts `markdown`
/// or `html`; anything else returns a friendly error string.
async fn import_dropped_file(
    file: &web_sys::File,
    folder_id: Option<&str>,
) -> Result<crate::api::documents::DocumentResponse, String> {
    let name = file.name();
    let size = file.size() as u64;
    if size > IMPORT_MAX_BYTES {
        return Err(format!(
            "{name} is {size} bytes; import limit is {IMPORT_MAX_BYTES} bytes"
        ));
    }
    let lowered = name.to_lowercase();
    let format = if lowered.ends_with(".md") || lowered.ends_with(".markdown") {
        "markdown"
    } else if lowered.ends_with(".html") || lowered.ends_with(".htm") {
        "html"
    } else {
        return Err(format!(
            "unsupported file type: {name} — use .md, .markdown, .html, .htm, .docx, or .pdf"
        ));
    };
    // Title default: filename minus extension. Server enforces its
    // own title rules; we just want something sensible by default.
    let stem = name
        .rsplit_once('.')
        .map(|(s, _)| s.to_string())
        .unwrap_or_else(|| name.clone());
    let title = if stem.trim().is_empty() {
        crate::t!("home-import-default-title")
    } else {
        stem
    };

    let text_promise = file.text();
    let text_js = wasm_bindgen_futures::JsFuture::from(text_promise)
        .await
        .map_err(|e| format!("failed to read {name}: {e:?}"))?;
    let content = text_js
        .as_string()
        .ok_or_else(|| format!("{name} did not yield a UTF-8 string"))?;

    crate::api::documents::import_document_from_text(format, &title, &content, folder_id)
        .await
        .map_err(|e| format!("import failed: {e}"))
}

/// Async binary drop path (M-6.5 / M-6.6 piece D) for DOCX and PDF.
/// Applies the same size guard as `import_dropped_file`, then hands the
/// binary off to the import-job + poll flow. Returns the new document
/// id so the caller can navigate to it. Distinct from
/// `import_dropped_file` because these formats are binary (no
/// `file.text()`) and convert on the worker.
async fn import_dropped_async(
    file: &web_sys::File,
    folder_id: Option<&str>,
) -> Result<String, String> {
    let name = file.name();
    let size = file.size() as u64;
    if size > ASYNC_IMPORT_MAX_BYTES {
        return Err(format!(
            "{name} is {size} bytes; import limit is {ASYNC_IMPORT_MAX_BYTES} bytes"
        ));
    }
    crate::api::documents::import_document_via_job(file, folder_id)
        .await
        .map_err(|e| format!("import failed: {e}"))
}
