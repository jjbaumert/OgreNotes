use leptos::prelude::*;

/// Share dialog component for managing folder/document access.
#[component]
pub fn ShareDialog(
    /// Whether the dialog is visible.
    visible: ReadSignal<bool>,
    /// Callback to close the dialog.
    on_close: Callback<()>,
    /// Document or folder ID to share.
    resource_id: String,
) -> impl IntoView {
    let (email_input, set_email_input) = signal(String::new());
    let (access_level, set_access_level) = signal("EDIT".to_string());
    let (status_msg, set_status_msg) = signal(String::new());

    let resource_id_share = resource_id.clone();

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
                                on:click=move |_| {
                                    let email = email_input.get_untracked();
                                    let level = access_level.get_untracked();
                                    set_status_msg.set(format!(
                                        "Share with '{email}' at {level} level (not yet wired)"
                                    ));
                                }
                            >"Share"</button>
                        </div>

                        {move || {
                            let msg = status_msg.get();
                            if msg.is_empty() {
                                view! { <div></div> }.into_any()
                            } else {
                                view! {
                                    <div class="share-status">{msg}</div>
                                }.into_any()
                            }
                        }}
                    </div>
                </div>
            </div>
        </Show>
    }
}
