use leptos::prelude::*;

use crate::api::{sharing, users};

/// Share dialog component for managing folder/document access.
#[component]
pub fn ShareDialog(
    /// Whether the dialog is visible.
    visible: ReadSignal<bool>,
    /// Callback to close the dialog.
    on_close: Callback<()>,
    /// Folder ID to share (from the document's parent folder).
    folder_id: ReadSignal<Option<String>>,
) -> impl IntoView {
    let (email_input, set_email_input) = signal(String::new());
    let (access_level, set_access_level) = signal("EDIT".to_string());
    let (status_msg, set_status_msg) = signal(String::new());
    let (status_error, set_status_error) = signal(false);
    let (members, set_members) = signal::<Vec<MemberEntry>>(Vec::new());
    let (loading, set_loading) = signal(false);

    // Load existing members when dialog opens.
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        let Some(fid) = folder_id.get() else {
            return;
        };
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match sharing::list_members(&fid).await {
                Ok(resp) => {
                    let entries: Vec<MemberEntry> = resp
                        .members
                        .into_iter()
                        .map(|m| MemberEntry {
                            user_id: m.user_id,
                            name: m.name,
                            email: m.email,
                            access_level: m.access_level,
                        })
                        .collect();
                    set_members.set(entries);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load members: {e}").into(),
                    );
                }
            }
            set_loading.set(false);
        });
    });

    let do_share = move || {
        let email = email_input.get_untracked();
        let level = access_level.get_untracked();
        let Some(fid) = folder_id.get_untracked() else {
            set_status_msg.set("Document has no folder — cannot share".to_string());
            set_status_error.set(true);
            return;
        };

        if email.trim().is_empty() {
            set_status_msg.set("Enter an email address".to_string());
            set_status_error.set(true);
            return;
        }

        set_status_msg.set("Searching...".to_string());
        set_status_error.set(false);

        leptos::task::spawn_local(async move {
            // Look up user by email.
            let search_result = match users::search_by_email(&email).await {
                Ok(resp) => resp,
                Err(e) => {
                    set_status_msg.set(format!("Search failed: {e}"));
                    set_status_error.set(true);
                    return;
                }
            };

            let Some(user) = search_result.users.into_iter().next() else {
                set_status_msg.set(format!("No user found with email '{email}'"));
                set_status_error.set(true);
                return;
            };

            // Add as folder member.
            match sharing::add_member(&fid, &user.user_id, &level).await {
                Ok(()) => {
                    set_status_msg.set(format!("Shared with {}", user.name));
                    set_status_error.set(false);
                    set_email_input.set(String::new());
                    // Refresh member list.
                    if let Ok(resp) = sharing::list_members(&fid).await {
                        let entries: Vec<MemberEntry> = resp
                            .members
                            .into_iter()
                            .map(|m| MemberEntry {
                                user_id: m.user_id,
                                name: m.name,
                                email: m.email,
                                access_level: m.access_level,
                            })
                            .collect();
                        set_members.set(entries);
                    }
                }
                Err(e) => {
                    set_status_msg.set(format!("Failed to share: {e}"));
                    set_status_error.set(true);
                }
            }
        });
    };

    let do_share_click = do_share.clone();

    view! {
        <Show when=move || visible.get()>
            <div class="share-backdrop" on:click=move |_| on_close.run(())>
                <div class="share-dialog" on:click=move |e: web_sys::MouseEvent| e.stop_propagation()>
                    <div class="share-header">
                        <h3>"Share"</h3>
                        <button class="share-close" on:click=move |_| on_close.run(())>
                            "\u{2715}"
                        </button>
                    </div>

                    <div class="share-body">
                        <div class="share-input-row">
                            <input
                                type="email"
                                class="share-email-input"
                                placeholder="Enter email address"
                                prop:value=move || email_input.get()
                                on:input=move |e| {
                                    set_email_input.set(event_target_value(&e));
                                    set_status_msg.set(String::new());
                                }
                                on:keydown=move |e: web_sys::KeyboardEvent| {
                                    if e.key() == "Enter" {
                                        e.prevent_default();
                                        do_share();
                                    }
                                }
                            />
                            <select
                                class="share-level-select"
                                prop:value=move || access_level.get()
                                on:change=move |e| {
                                    set_access_level.set(event_target_value(&e));
                                }
                            >
                                <option value="EDIT">"Can Edit"</option>
                                <option value="COMMENT">"Can Comment"</option>
                                <option value="VIEW">"Can View"</option>
                            </select>
                            <button
                                class="share-btn"
                                on:click=move |_| do_share_click()
                            >"Share"</button>
                        </div>

                        {move || {
                            let msg = status_msg.get();
                            if msg.is_empty() {
                                view! { <div></div> }.into_any()
                            } else {
                                let class = if status_error.get() { "share-status share-error" } else { "share-status share-success" };
                                view! {
                                    <div class=class>{msg}</div>
                                }.into_any()
                            }
                        }}

                        // Member list
                        <Show when=move || !members.get().is_empty()>
                            <div class="share-members">
                                <h4>"Current members"</h4>
                                {move || {
                                    members.get().into_iter().map(|m| {
                                        let uid = m.user_id.clone();
                                        let level = m.access_level.clone();
                                        let is_owner = level == "OWN";
                                        view! {
                                            <div class="share-member-row">
                                                <span class="share-member-id">{format!("{} ({})", m.name, m.email)}</span>
                                                <span class="share-member-level">{format_access_level(&level)}</span>
                                                <Show when=move || !is_owner>
                                                    <button
                                                        class="share-member-remove"
                                                        on:click={
                                                            let uid = uid.clone();
                                                            move |_| {
                                                                let uid = uid.clone();
                                                                let fid = folder_id.get_untracked();
                                                                leptos::task::spawn_local(async move {
                                                                    if let Some(fid) = fid {
                                                                        let _ = sharing::remove_member(&fid, &uid).await;
                                                                        if let Ok(resp) = sharing::list_members(&fid).await {
                                                                            let entries: Vec<MemberEntry> = resp
                                                                                .members
                                                                                .into_iter()
                                                                                .map(|m| MemberEntry {
                                                                                    user_id: m.user_id,
                                                                                    name: m.name,
                                                                                    email: m.email,
                                                                                    access_level: m.access_level,
                                                                                })
                                                                                .collect();
                                                                            set_members.set(entries);
                                                                        }
                                                                    }
                                                                });
                                                            }
                                                        }
                                                    >"\u{2715}"</button>
                                                </Show>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()
                                }}
                            </div>
                        </Show>
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[derive(Clone)]
struct MemberEntry {
    user_id: String,
    name: String,
    email: String,
    access_level: String,
}

fn format_access_level(level: &str) -> String {
    match level {
        "OWN" => "Owner",
        "EDIT" => "Can Edit",
        "COMMENT" => "Can Comment",
        "VIEW" => "Can View",
        other => other,
    }
    .to_string()
}
