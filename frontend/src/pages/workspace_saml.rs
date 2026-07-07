// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `/workspaces/:id/saml` — workspace admin "Configure SAML" UI
//! (Phase 4 M-E4 piece E).
//!
//! Two responsibilities:
//!   1. Surface the SP metadata URL (with copy-to-clipboard) so the
//!      admin can paste it into their IdP's "add a service provider"
//!      flow.
//!   2. Accept the IdP metadata XML the admin pastes back. Save /
//!      update / delete the workspace SAML config.
//!
//! Server-side enforces workspace-admin gating; non-admins hitting
//! this page see the 403 error from the GET and the form disables.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::{use_navigate, use_params_map};
use wasm_bindgen::JsCast;

use crate::api::client;
use crate::api::saml_config::{self, PutSamlConfigRequest, SamlConfig};

#[component]
pub fn WorkspaceSamlPage() -> impl IntoView {
    let params = use_params_map();
    let workspace_id = params
        .read_untracked()
        .get("id")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let navigate = use_navigate();

    if !client::is_authenticated() {
        let nav = navigate.clone();
        nav("/login", Default::default());
        return view! { <div>{crate::t!("common-redirecting-login")}</div> }.into_any();
    }

    let (config, set_config) = signal::<Option<SamlConfig>>(None);
    let (entity_id_input, set_entity_id_input) = signal::<String>(String::new());
    let (metadata_xml_input, set_metadata_xml_input) = signal::<String>(String::new());
    let (email_attr_input, set_email_attr_input) = signal::<String>("email".to_string());
    let (name_attr_input, set_name_attr_input) = signal::<String>("name".to_string());
    let (error, set_error) = signal::<Option<String>>(None);
    let (status_msg, set_status_msg) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal(false);

    let sp_metadata_url = {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        format!("{origin}/api/v1/auth/saml/metadata")
    };

    // Stash the load body as a standalone async function so the
    // mount-effect and the save-success path can both fire it
    // without us having to make `load_config` a `Rc<Fn>` —
    // closures aren't Clone in Rust, and Leptos's stored-value
    // dance is heavier than the inline spawn_local.
    let trigger_load = {
        let workspace_id = workspace_id.clone();
        move || {
            let workspace_id = workspace_id.clone();
            set_busy.set(true);
            set_error.set(None);
            spawn_local(async move {
                match saml_config::get_config(&workspace_id).await {
                    Ok(c) => {
                        if let Some(ref cfg) = c {
                            set_entity_id_input.set(cfg.idp_entity_id.clone());
                            set_metadata_xml_input.set(cfg.idp_metadata_xml.clone());
                            set_email_attr_input.set(cfg.attribute_email.clone());
                            set_name_attr_input.set(cfg.attribute_name.clone());
                        }
                        set_config.set(c);
                    }
                    Err(e) => set_error.set(Some(crate::t!("saml-error-load-failed", err = format!("{e:?}")))),
                }
                set_busy.set(false);
            });
        }
    };

    // One-shot load at mount. Effect::new guards against a Leptos
    // re-mount cycle firing two parallel GETs (same pattern the
    // MFA enroll page uses).
    Effect::new({
        let trigger_load = trigger_load.clone();
        move |has_run: Option<bool>| {
            if has_run == Some(true) {
                return true;
            }
            trigger_load();
            true
        }
    });

    let on_save = {
        let workspace_id = workspace_id.clone();
        let trigger_load = trigger_load.clone();
        move |_| {
            let workspace_id = workspace_id.clone();
            let entity_id = entity_id_input.get().trim().to_string();
            let metadata = metadata_xml_input.get();
            let email_attr = email_attr_input.get();
            let name_attr = name_attr_input.get();
            if entity_id.is_empty() {
                set_error.set(Some(crate::t!("saml-error-entity-id-required")));
                return;
            }
            if metadata.trim().is_empty() {
                set_error.set(Some(crate::t!("saml-error-metadata-required")));
                return;
            }
            set_busy.set(true);
            set_error.set(None);
            set_status_msg.set(None);
            let req = PutSamlConfigRequest {
                idp_entity_id: entity_id,
                idp_metadata_xml: metadata,
                attribute_email: email_attr,
                attribute_name: name_attr,
            };
            let trigger_load = trigger_load.clone();
            spawn_local(async move {
                // Track save success locally so we can sequence the
                // reload AFTER set_busy(false). Calling trigger_load()
                // inline would set busy=true again from inside its
                // closure, and the set_busy(false) below would then
                // immediately stomp the GET's busy flag — leaving the
                // spinner off while the GET is still in flight.
                let saved = match saml_config::put_config(&workspace_id, &req).await {
                    Ok(()) => {
                        set_status_msg.set(Some(crate::t!("saml-status-saved")));
                        true
                    }
                    Err(e) => {
                        set_error.set(Some(crate::t!("saml-error-save-failed", err = format!("{e:?}"))));
                        false
                    }
                };
                set_busy.set(false);
                if saved {
                    trigger_load();
                }
            });
        }
    };

    let on_delete = {
        let workspace_id = workspace_id.clone();
        move |_| {
            let workspace_id = workspace_id.clone();
            set_busy.set(true);
            set_error.set(None);
            spawn_local(async move {
                match saml_config::delete_config(&workspace_id).await {
                    Ok(()) => {
                        set_status_msg.set(Some(crate::t!("saml-status-removed")));
                        set_config.set(None);
                        set_entity_id_input.set(String::new());
                        set_metadata_xml_input.set(String::new());
                        set_email_attr_input.set("email".to_string());
                        set_name_attr_input.set("name".to_string());
                    }
                    Err(e) => set_error.set(Some(crate::t!("saml-error-delete-failed", err = format!("{e:?}")))),
                }
                set_busy.set(false);
            });
        }
    };

    let on_copy_sp_url = {
        let url = sp_metadata_url.clone();
        move |_| {
            // Browser clipboard write. Best-effort — if denied (e.g.
            // browser permission policy), the user falls back to
            // manual select-and-copy on the input element below.
            if let Some(window) = web_sys::window() {
                let clipboard = window.navigator().clipboard();
                let _ = clipboard.write_text(&url);
                set_status_msg.set(Some(crate::t!("saml-status-copied")));
            }
        }
    };

    let on_select_sp_url = move |ev: leptos::ev::MouseEvent| {
        // Convenience: click the readonly input → select all so the
        // admin can Ctrl+C if the clipboard API was denied.
        if let Some(target) = ev.target() {
            if let Ok(input) = target.dyn_into::<web_sys::HtmlInputElement>() {
                let _ = input.select();
            }
        }
    };

    view! {
        <main id="main-content" tabindex="-1" class="workspace-saml-page">
            <div class="workspace-saml-card">
                <h1>{crate::t!("saml-title")}</h1>
                <p class="workspace-saml-subtitle">
                    {crate::t!("saml-subtitle-prefix")}" "
                    <code>{format!("/api/v1/auth/saml/login?workspace={}", workspace_id)}</code>
                    " "{crate::t!("saml-subtitle-suffix")}
                </p>

                {
                    // Retry button when the mount-time GET failed. The
                    // Effect-guard is one-shot, so without an explicit
                    // retry the user is stuck on an error screen until
                    // they reload the page.
                    let trigger_load = trigger_load.clone();
                    move || error.get().map(|e| {
                        let tl = trigger_load.clone();
                        view! {
                            <div class="workspace-saml-error">
                                {e}
                                " "
                                <button
                                    class="workspace-saml-retry"
                                    on:click=move |_| tl()
                                    disabled=move || busy.get()
                                >{crate::t!("admin-retry")}</button>
                            </div>
                        }
                    })
                }
                {move || status_msg.get().map(|m| view! {
                    <div class="workspace-saml-status" role="status" aria-live="polite">{m}</div>
                })}

                // ── SP metadata (read-only, for the admin to copy) ──
                <section class="workspace-saml-section">
                    <h2>{crate::t!("saml-sp-heading")}</h2>
                    <p class="workspace-saml-help">
                        {crate::t!("saml-sp-help")}
                    </p>
                    <div class="workspace-saml-url-row">
                        <input
                            type="text"
                            class="workspace-saml-sp-url"
                            readonly
                            prop:value=sp_metadata_url.clone()
                            on:click=on_select_sp_url
                        />
                        <button on:click=on_copy_sp_url disabled=move || busy.get()>
                            {crate::t!("saml-copy")}
                        </button>
                    </div>
                </section>

                // ── IdP metadata upload form ─────────────────────────
                <section class="workspace-saml-section">
                    <h2>{crate::t!("saml-idp-heading")}</h2>
                    <p class="workspace-saml-help">
                        {crate::t!("saml-idp-help")}
                    </p>

                    <label>{crate::t!("saml-label-entity-id")}</label>
                    <input
                        type="text"
                        placeholder=crate::t!("saml-placeholder-entity-id")
                        prop:value=move || entity_id_input.get()
                        on:input=move |ev| set_entity_id_input.set(event_target_value(&ev))
                    />

                    <label>{crate::t!("saml-label-metadata-xml")}</label>
                    <textarea
                        rows="14"
                        placeholder=r#"<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata" entityID="…">…</EntityDescriptor>"#
                        prop:value=move || metadata_xml_input.get()
                        on:input=move |ev| set_metadata_xml_input.set(event_target_value(&ev))
                    />

                    <div class="workspace-saml-attrs">
                        <label>{crate::t!("saml-label-email-attr")}</label>
                        <input
                            type="text"
                            prop:value=move || email_attr_input.get()
                            on:input=move |ev| set_email_attr_input.set(event_target_value(&ev))
                        />
                        <label>{crate::t!("saml-label-name-attr")}</label>
                        <input
                            type="text"
                            prop:value=move || name_attr_input.get()
                            on:input=move |ev| set_name_attr_input.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="workspace-saml-actions">
                        <button
                            class="workspace-saml-save"
                            on:click=on_save
                            disabled=move || busy.get()
                        >
                            {move || if config.with(Option::is_some) { crate::t!("saml-update") } else { crate::t!("saml-save") }}
                        </button>
                        {move || if config.with(Option::is_some) {
                            view! {
                                <button
                                    class="workspace-saml-delete"
                                    on:click=on_delete.clone()
                                    disabled=move || busy.get()
                                >{crate::t!("saml-remove")}</button>
                            }.into_any()
                        } else {
                            view! { <div></div> }.into_any()
                        }}
                    </div>

                    {move || config.with(|c| c.as_ref().map(|cfg| view! {
                        <p class="workspace-saml-meta">
                            {crate::t!("saml-meta-first-configured")}" "
                            {crate::i18n::format_date(cfg.created_at, crate::i18n::DateStyle::Long)}
                            {crate::t!("saml-meta-last-updated")}" "
                            {crate::i18n::format_date(cfg.updated_at, crate::i18n::DateStyle::Long)}
                            "."
                        </p>
                    }))}
                </section>
            </div>
        </main>
    }
    .into_any()
}

