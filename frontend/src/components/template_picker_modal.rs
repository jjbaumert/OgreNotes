// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// #142: template picker — the modal that opens from "New from Template…"
// (Document menu), the sidebar Templates entry, and "New from Template"
// on the home page. Two states:
//
//   Step 1 — Pick.  List the caller's workspace-visible templates.
//   Step 2 — Fill.  When the picked template has [[placeholders]], show a
//                   field-per-placeholder form; on submit, copy with the
//                   filled `values` dict. When the picked template has no
//                   placeholders, skip step 2 and copy immediately.
//
// The copy defaults to the caller's Private folder (server-side).

use std::collections::HashMap;

use leptos::prelude::*;

use crate::api::documents::{self, CopyDocumentRequest, Gallery, TemplateItem};
use crate::commands::nav_bridge;

#[component]
pub fn TemplatePickerModal(
    /// Visibility flag — the parent owns it and flips it on Use / Cancel.
    visible: ReadSignal<bool>,
    /// Called when the modal should close (backdrop click, Cancel button,
    /// successful pick — the parent flips its own visible signal).
    on_close: Callback<()>,
) -> impl IntoView {
    let (templates, set_templates) = signal::<Vec<TemplateItem>>(Vec::new());
    let (loading, set_loading) = signal(false);
    let (error, set_error) = signal::<Option<String>>(None);
    // Per-row busy state so re-clicks during the network round-trip don't
    // stack copy requests.
    let (busy_id, set_busy_id) = signal::<Option<String>>(None);
    // Step 2 state: the picked template + the values map. `values` is
    // keyed by placeholder string (flat form; server accepts both flat
    // and dot-nested lookups so we don't have to reconstruct nested
    // objects on the client — see `mail_merge::lookup`).
    let (picked, set_picked) = signal::<Option<TemplateItem>>(None);
    let values: RwSignal<HashMap<String, String>> = RwSignal::new(HashMap::new());

    // Load the gallery whenever the modal opens. Cheap: one GET.
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        // Reset per-open so a re-open doesn't show a stale step-2 form.
        set_picked.set(None);
        values.set(HashMap::new());
        set_busy_id.set(None);
        set_error.set(None);
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match documents::list_templates().await {
                Ok(list) => set_templates.set(list),
                Err(e) => set_error.set(Some(e.to_string())),
            }
            set_loading.set(false);
        });
    });

    // Kick off the copy request. Called from both the Step 1 fast-path
    // (template with no placeholders — clicked from the row) and the
    // Step 2 submit path (values form filled).
    let submit_copy = move |template_id: String, values_payload: Option<serde_json::Value>| {
        if busy_id.get_untracked().is_some() {
            return;
        }
        set_busy_id.set(Some(template_id.clone()));
        leptos::task::spawn_local(async move {
            let req = CopyDocumentRequest {
                values: values_payload,
                ..Default::default()
            };
            match documents::copy_document(&template_id, &req).await {
                Ok(doc) => {
                    on_close.run(());
                    nav_bridge::go(&format!("/d/{}", doc.id));
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Copy template failed: {e}").into(),
                    );
                    set_busy_id.set(None);
                }
            }
        });
    };

    // Row click: if placeholders → step 2 form; else immediate copy.
    let on_row_click = {
        let submit_copy = submit_copy.clone();
        move |t: TemplateItem| {
            if t.placeholders.is_empty() {
                submit_copy(t.id.clone(), None);
            } else {
                // Seed the map so `<input value=…>` bindings have keys to
                // read; missing keys would collapse to empty strings but
                // pre-seeding keeps the form deterministic.
                let seed: HashMap<String, String> = t
                    .placeholders
                    .iter()
                    .map(|k| (k.clone(), String::new()))
                    .collect();
                values.set(seed);
                set_picked.set(Some(t));
            }
        }
    };

    // Step 2 submit: build a flat JSON object from the values map and
    // send. Empty strings are still sent — the server treats missing
    // vs. empty distinctly (empty means "use empty text as the
    // replacement," missing means "keep the placeholder raw").
    let on_fill_submit = {
        let submit_copy = submit_copy.clone();
        move || {
            let Some(t) = picked.get_untracked() else {
                return;
            };
            let map = values.get_untracked();
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                obj.insert(k, serde_json::Value::String(v));
            }
            submit_copy(t.id.clone(), Some(serde_json::Value::Object(obj)));
        }
    };

    view! {
        <Show when=move || visible.get()>
            <div class="confirm-backdrop" on:click=move |_| on_close.run(())>
            <div
                class="folder-picker-dialog template-picker-dialog"
                role="dialog"
                aria-modal="true"
                on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
            >
                {move || {
                    if let Some(t) = picked.get() {
                        // ─── Step 2: Fill in values ───────────────
                        let submit = on_fill_submit.clone();
                        let template_id = t.id.clone();
                        view! {
                            <div class="confirm-header">
                                <button
                                    class="toolbar-btn template-picker-back"
                                    aria-label=crate::t!("template-picker-back")
                                    on:click=move |_| set_picked.set(None)
                                >"\u{2190}"</button>
                                <h3>{crate::t!("template-picker-fill-title")}</h3>
                                <button
                                    class="toolbar-btn"
                                    aria-label=crate::t!("modal-close")
                                    on:click=move |_| on_close.run(())
                                >"\u{00D7}"</button>
                            </div>
                            <div class="folder-picker-body template-picker-body template-picker-fill">
                                <p class="template-picker-fill-hint">
                                    {crate::t!("template-picker-fill-hint")}
                                </p>
                                {t.placeholders.iter().map(|key| {
                                    let field_key = key.clone();
                                    let input_key = key.clone();
                                    view! {
                                        <label class="template-picker-field">
                                            <span class="template-picker-field-key">
                                                {field_key.clone()}
                                            </span>
                                            <input
                                                type="text"
                                                class="template-picker-field-input"
                                                prop:value=move || {
                                                    values.get().get(&field_key).cloned().unwrap_or_default()
                                                }
                                                on:input=move |ev| {
                                                    let v = event_target_value(&ev);
                                                    values.update(|m| {
                                                        m.insert(input_key.clone(), v);
                                                    });
                                                }
                                            />
                                        </label>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                            <div class="confirm-actions">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_picked.set(None)
                                >{crate::t!("template-picker-cancel")}</button>
                                <button
                                    class="btn btn-primary"
                                    disabled=move || busy_id.get().as_deref() == Some(template_id.as_str())
                                    on:click=move |_| submit()
                                >{move || if busy_id.get().is_some() {
                                    crate::t!("template-picker-using")
                                } else {
                                    crate::t!("template-picker-create")
                                }}</button>
                            </div>
                        }.into_any()
                    } else {
                        // ─── Step 1: Pick a template ──────────────
                        view! {
                            <div class="confirm-header">
                                <h3>{crate::t!("template-picker-title")}</h3>
                                <button
                                    class="toolbar-btn"
                                    aria-label=crate::t!("modal-close")
                                    on:click=move |_| on_close.run(())
                                >"\u{00D7}"</button>
                            </div>
                            <div class="folder-picker-body template-picker-body">
                                {move || if loading.get() {
                                    view! {
                                        <div class="template-picker-empty">
                                            {crate::t!("template-picker-loading")}
                                        </div>
                                    }.into_any()
                                } else if let Some(e) = error.get() {
                                    view! {
                                        <div class="template-picker-error">{e}</div>
                                    }.into_any()
                                } else {
                                    let list = templates.get();
                                    if list.is_empty() {
                                        view! {
                                            <div class="template-picker-empty">
                                                {crate::t!("template-picker-empty")}
                                            </div>
                                        }.into_any()
                                    } else {
                                        // Phase 3: group rows by the server-assigned gallery tag
                                        // and render each non-empty group under a section
                                        // header. Order: mine → shared → sample so a user's
                                        // own templates surface first.
                                        //
                                        // Rows with `gallery: None` mean the server didn't send
                                        // the field (rollback / drift / stripped by a proxy).
                                        // Fall back to a headerless "Templates" bucket AND log
                                        // to the console so the drift shows up in bug reports —
                                        // silently mislabeling every row as "Your templates"
                                        // (the default the enum had before Option) was worse.
                                        let on_row_click = on_row_click.clone();
                                        let mut mine = Vec::new();
                                        let mut shared = Vec::new();
                                        let mut sample = Vec::new();
                                        let mut untagged = Vec::new();
                                        // Phase 4: one Vec per company gallery (keyed by
                                        // gallery_id) plus the gallery's display name.
                                        // Insertion-ordered so later galleries appear below
                                        // earlier ones (matches DDB SK order — admins can
                                        // control section order via naming). indexmap is
                                        // not in the dep graph; a Vec of (id, name, rows)
                                        // is simpler at this scale.
                                        let mut company_sections: Vec<(String, String, Vec<TemplateItem>)> = Vec::new();
                                        for t in list {
                                            match &t.gallery {
                                                Some(Gallery::Mine) => mine.push(t),
                                                Some(Gallery::Shared) => shared.push(t),
                                                Some(Gallery::Sample) => sample.push(t),
                                                Some(Gallery::Company { gallery_id, gallery_name }) => {
                                                    let gid = gallery_id.clone();
                                                    let gname = gallery_name.clone();
                                                    if let Some(existing) = company_sections
                                                        .iter_mut()
                                                        .find(|(id, _, _)| id == &gid)
                                                    {
                                                        existing.2.push(t);
                                                    } else {
                                                        company_sections.push((gid, gname, vec![t]));
                                                    }
                                                }
                                                Some(Gallery::Unknown) | None => untagged.push(t),
                                            }
                                        }
                                        if !untagged.is_empty() {
                                            web_sys::console::warn_1(
                                                &format!(
                                                    "template picker: {} row(s) missing `gallery` field \
                                                     (server rollback or wire-format drift). \
                                                     Rendering in a fallback section.",
                                                    untagged.len(),
                                                )
                                                .into(),
                                            );
                                        }
                                        // Fixed sections then company sections then untagged.
                                        // Company section labels come from the server (admin-
                                        // chosen names) so they don't route through the Fluent
                                        // catalog.
                                        let mut sections: Vec<(String, Vec<TemplateItem>)> = vec![
                                            (crate::t!("template-picker-section-mine"), mine),
                                            (crate::t!("template-picker-section-shared"), shared),
                                            (crate::t!("template-picker-section-sample"), sample),
                                        ];
                                        for (_gid, name, rows) in company_sections {
                                            sections.push((name, rows));
                                        }
                                        sections.push((crate::t!("template-picker-section-untagged"), untagged));
                                        sections.into_iter().filter(|(_, rows)| !rows.is_empty()).map(|(label, rows)| {
                                            let on_row_click = on_row_click.clone();
                                            view! {
                                                <div class="template-picker-section-title">{label}</div>
                                                {rows.into_iter().map(|t| {
                                                    let id_disabled = t.id.clone();
                                                    let id_label = t.id.clone();
                                                    let has_ph = !t.placeholders.is_empty();
                                                    let t_for_click = t.clone();
                                                    let on_row_click = on_row_click.clone();
                                                    view! {
                                                        <button
                                                            class="template-picker-row"
                                                            disabled=move || busy_id.get().as_deref() == Some(id_disabled.as_str())
                                                            on:click=move |_| on_row_click(t_for_click.clone())
                                                        >
                                                            <span class="template-picker-row-title">{t.title}</span>
                                                            <span class="template-picker-row-action">
                                                                {move || if busy_id.get().as_deref() == Some(id_label.as_str()) {
                                                                    crate::t!("template-picker-using")
                                                                } else if has_ph {
                                                                    crate::t!("template-picker-fill")
                                                                } else {
                                                                    crate::t!("template-picker-use")
                                                                }}
                                                            </span>
                                                        </button>
                                                    }
                                                }).collect::<Vec<_>>()}
                                            }
                                        }).collect::<Vec<_>>().into_any()
                                    }
                                }}
                            </div>
                        }.into_any()
                    }
                }}
            </div>
            </div>
        </Show>
    }
}
