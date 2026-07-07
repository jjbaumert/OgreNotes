// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Admin users page — list, search by email prefix, paginate, and
//! per-row actions (disable / enable / promote / demote). The page
//! holds one page of results at a time; navigation is via the
//! cursor returned by the previous response.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::admin::{self, AdminUser};

use super::AdminGate;

#[component]
pub fn AdminUsersPage() -> impl IntoView {
    view! {
        <AdminGate>
            <UsersTable />
        </AdminGate>
    }
}

#[component]
fn UsersTable() -> impl IntoView {
    let (users, set_users) = signal::<Vec<AdminUser>>(Vec::new());
    let (cursor_stack, set_cursor_stack) = signal::<Vec<Option<String>>>(vec![None]);
    let (next_cursor, set_next_cursor) = signal::<Option<String>>(None);
    let (email_prefix, set_email_prefix) = signal::<String>(String::new());
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal::<bool>(false);

    // `reload` fetches the current page (top of cursor_stack) plus the
    // current email_prefix. Defined once and reused by initial mount,
    // search box, pagination buttons, and every mutation's "refresh
    // after" follow-up.
    let reload = move || {
        set_busy.set(true);
        set_error.set(None);
        let prefix = email_prefix.get();
        let cursor = cursor_stack.with(|s| s.last().cloned().flatten());
        spawn_local(async move {
            match admin::list_users(cursor.as_deref(), Some(prefix.as_str())).await {
                Ok(list) => {
                    set_users.set(list.users);
                    set_next_cursor.set(list.next_cursor);
                }
                Err(e) => set_error.set(Some(crate::t!("admin-users-error-list-failed", err = format!("{e:?}")))),
            }
            set_busy.set(false);
        });
    };

    // Initial load.
    reload();

    let on_search_input = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev);
        set_email_prefix.set(v);
        // Reset to first page on any search change — prior cursors no
        // longer correspond to a meaningful position under the new
        // filter.
        set_cursor_stack.set(vec![None]);
        reload();
    };

    let on_next = move |_| {
        if let Some(c) = next_cursor.get() {
            set_cursor_stack.update(|s| s.push(Some(c)));
            reload();
        }
    };

    let on_prev = move |_| {
        set_cursor_stack.update(|s| {
            if s.len() > 1 {
                s.pop();
            }
        });
        reload();
    };

    let do_action =
        move |action: UserAction, target_id: String| {
            set_busy.set(true);
            set_error.set(None);
            spawn_local(async move {
                let result = match action {
                    UserAction::Disable => admin::disable_user(&target_id).await,
                    UserAction::Enable => admin::enable_user(&target_id).await,
                    UserAction::Promote => admin::promote_user(&target_id).await,
                    UserAction::Demote => admin::demote_user(&target_id).await,
                };
                match result {
                    Ok(()) => {
                        // Refresh the same page so the row's
                        // is_admin / is_disabled badges reflect the
                        // mutation. `reload` resets `busy = true`
                        // again; that's intentional (the second fetch
                        // starts immediately).
                        reload();
                    }
                    Err(e) => {
                        set_error.set(Some(crate::t!("admin-users-error-action-failed", action = format!("{action:?}"), err = format!("{e:?}"))));
                        set_busy.set(false);
                    }
                }
            });
        };

    view! {
        <main id="main-content" tabindex="-1" class="admin-users">
            <h1>{crate::t!("admin-users-title")}</h1>

            <input
                type="text"
                placeholder=crate::t!("admin-users-search-placeholder")
                prop:value=move || email_prefix.get()
                on:input=on_search_input
                class="admin-search"
            />

            {move || error.get().map(|msg| view! {
                <div class="admin-error">{msg}</div>
            })}

            <table class="admin-users-table">
                <thead>
                    <tr>
                        <th>{crate::t!("admin-users-th-email")}</th>
                        <th>{crate::t!("admin-users-th-name")}</th>
                        <th>{crate::t!("admin-users-th-role")}</th>
                        <th>{crate::t!("admin-users-th-state")}</th>
                        <th>{crate::t!("admin-users-th-last-active")}</th>
                        <th>{crate::t!("admin-users-th-actions")}</th>
                    </tr>
                </thead>
                <tbody>
                    <For
                        each=move || users.get()
                        key=|u| u.id.clone()
                        children=move |u: AdminUser| {
                            let id_for_disable = u.id.clone();
                            let id_for_enable = u.id.clone();
                            let id_for_promote = u.id.clone();
                            let id_for_demote = u.id.clone();
                            let is_admin = u.is_admin;
                            let is_disabled = u.is_disabled;
                            view! {
                                <tr>
                                    <td>{u.email.clone()}</td>
                                    <td>{u.name.clone()}</td>
                                    <td>{if is_admin { crate::t!("admin-role-admin") } else { crate::t!("admin-role-user") }}</td>
                                    <td>{if is_disabled { crate::t!("admin-status-disabled") } else { crate::t!("admin-status-active") }}</td>
                                    <td>{format_last_active(u.last_active_at)}</td>
                                    <td class="admin-row-actions">
                                        {if is_disabled {
                                            view! {
                                                <button
                                                    disabled=move || busy.get()
                                                    on:click=move |_| do_action(UserAction::Enable, id_for_enable.clone())
                                                >{crate::t!("admin-users-enable")}</button>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <button
                                                    disabled=move || busy.get()
                                                    on:click=move |_| do_action(UserAction::Disable, id_for_disable.clone())
                                                >{crate::t!("admin-users-disable")}</button>
                                            }.into_any()
                                        }}
                                        {if is_admin {
                                            view! {
                                                <button
                                                    disabled=move || busy.get()
                                                    on:click=move |_| do_action(UserAction::Demote, id_for_demote.clone())
                                                >{crate::t!("admin-users-demote")}</button>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <button
                                                    disabled=move || busy.get()
                                                    on:click=move |_| do_action(UserAction::Promote, id_for_promote.clone())
                                                >{crate::t!("admin-users-promote")}</button>
                                            }.into_any()
                                        }}
                                    </td>
                                </tr>
                            }
                        }
                    />
                </tbody>
            </table>

            <div class="admin-pagination">
                <button
                    disabled=move || busy.get() || cursor_stack.with(|s| s.len() <= 1)
                    on:click=on_prev
                >{crate::t!("admin-users-prev")}</button>
                <button
                    disabled=move || busy.get() || next_cursor.get().is_none()
                    on:click=on_next
                >{crate::t!("admin-users-next")}</button>
            </div>
        </main>
    }
}

#[derive(Clone, Copy, Debug)]
enum UserAction {
    Disable,
    Enable,
    Promote,
    Demote,
}

fn format_last_active(usec: i64) -> String {
    if usec == 0 {
        return crate::t!("admin-status-never");
    }
    crate::i18n::format_date(usec, crate::i18n::DateStyle::Long)
}
