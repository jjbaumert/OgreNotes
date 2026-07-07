// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.2 piece D — relationship CRUD panel.
//!
//! Mounted inside the document outline drawer. Shows the active
//! document's outbound relationships (this doc → other doc) with
//! a per-row Remove button and an "Add link" affordance that opens
//! a tiny inline picker:
//!
//!   ┌─ Related ─────────────────────────┐
//!   │ Implements → Auth Design          │
//!   │   [Remove]                        │
//!   │ References → System Architecture  │
//!   │   [Remove]                        │
//!   │ + Add link                        │
//!   └───────────────────────────────────┘
//!
//! The picker is a typeahead against `/api/v1/search` — type to
//! filter, pick a result, choose a relation type, confirm. No
//! full-corpus tree like the FolderPicker; relationships are
//! sparse so search-as-you-type is the right interaction.
//!
//! Inbound relationships (other doc → this doc, i.e. RREL# rows)
//! aren't exposed by the v1 GET endpoint and would need a separate
//! handler. Deferred to v2 — the outbound list is enough for the
//! authoring flow.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::relationships::{self, RelationType, RelationshipDto};
use crate::api::search;

#[component]
pub fn RelationshipPanel(
    /// Document id whose relationships we manage.
    #[prop(into)] doc_id: Signal<String>,
) -> impl IntoView {
    let (rels, set_rels) = signal::<Vec<RelationshipDto>>(Vec::new());
    let (loading, set_loading) = signal(true);
    let (error, set_error) = signal::<Option<String>>(None);
    // Picker visibility + state.
    let (picker_open, set_picker_open) = signal(false);
    let (picker_query, set_picker_query) = signal::<String>(String::new());
    let (picker_results, set_picker_results) =
        signal::<Vec<search::SearchResultItem>>(Vec::new());
    let (picker_picked, set_picker_picked) =
        signal::<Option<search::SearchResultItem>>(None);
    let (picker_relation, set_picker_relation) = signal::<RelationType>(RelationType::References);

    // Initial load + reload when doc_id changes.
    let refresh = move || {
        let id = doc_id.get();
        if id.is_empty() {
            return;
        }
        set_loading.set(true);
        spawn_local(async move {
            match relationships::list(&id, None).await {
                Ok(list) => {
                    set_rels.set(list);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
            set_loading.set(false);
        });
    };
    Effect::new(move |_| {
        let _ = doc_id.get();
        refresh();
    });

    // Picker typeahead — debounced via a generation counter, same
    // pattern as the global search dialog uses.
    let (search_seq, set_search_seq) = signal(0u32);
    let on_query_input = move |ev: web_sys::Event| {
        let q = event_target_value(&ev);
        set_picker_query.set(q.clone());
        set_picker_picked.set(None);
        if q.trim().is_empty() {
            set_picker_results.set(Vec::new());
            return;
        }
        set_search_seq.update(|g| *g = g.wrapping_add(1));
        let seq = search_seq.get_untracked();
        spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(150).await;
            if seq != search_seq.get_untracked() {
                return;
            }
            match search::search(q.trim(), Some(8)).await {
                Ok(resp) => set_picker_results.set(resp.results),
                Err(_) => set_picker_results.set(Vec::new()),
            }
        });
    };

    let close_picker = move || {
        set_picker_open.set(false);
        set_picker_query.set(String::new());
        set_picker_results.set(Vec::new());
        set_picker_picked.set(None);
        set_picker_relation.set(RelationType::References);
    };

    let submit_picker = move || {
        let Some(target) = picker_picked.get_untracked() else {
            return;
        };
        let id = doc_id.get_untracked();
        let rel = picker_relation.get_untracked();
        if id == target.id {
            // Backend rejects self-references with 400; pre-empt
            // the round trip with a local error.
            set_error.set(Some(crate::t!("relationship-error-self")));
            return;
        }
        spawn_local(async move {
            match relationships::create(&id, &target.id, rel).await {
                Ok(()) => {
                    set_error.set(None);
                    close_picker();
                    // Optimistic-style refresh — the create returned
                    // 201 with no body, so we re-fetch to get the
                    // canonical row including created_at.
                    refresh();
                }
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    };

    let delete_one = move |target_id: String, relation_type: RelationType| {
        let id = doc_id.get_untracked();
        spawn_local(async move {
            match relationships::delete(&id, relation_type, &target_id).await {
                Ok(()) => refresh(),
                Err(e) => set_error.set(Some(e.to_string())),
            }
        });
    };

    view! {
        <div class="relationship-panel">
            <div class="relationship-panel-header">
                <span>{crate::t!("relationship-heading")}</span>
                <button
                    class="relationship-add-btn"
                    aria-label=crate::t!("relationship-add-aria")
                    on:click=move |_| set_picker_open.set(true)
                >"+"</button>
            </div>

            {move || error.get().map(|e| view! {
                <div class="relationship-error" role="alert">{e}</div>
            })}

            <Show
                when=move || !loading.get() && rels.get().is_empty() && error.get().is_none()
            >
                <div class="relationship-empty">
                    {crate::t!("relationship-empty")}
                </div>
            </Show>

            <ul class="relationship-list">
                {move || rels.get().into_iter().map(|r| {
                    // Decode the wire string to the typed enum so
                    // we can route to the label key. Unknown types
                    // pass through as raw strings (forward compat
                    // if the backend adds variants).
                    let typed = match r.relation_type.as_str() {
                        "implements" => Some(RelationType::Implements),
                        "derived-from" => Some(RelationType::DerivedFrom),
                        "depends-on" => Some(RelationType::DependsOn),
                        "references" => Some(RelationType::References),
                        "supersedes" => Some(RelationType::Supersedes),
                        _ => None,
                    };
                    let label = typed
                        .map(|t| t.label())
                        .unwrap_or_else(|| r.relation_type.clone());
                    let target = r.target_doc_id.clone();
                    let target_href = format!("/d/{target}/doc");
                    let target_for_delete = target.clone();
                    view! {
                        <li class="relationship-item">
                            <span class="relationship-type">{label}</span>
                            <span class="relationship-arrow" aria-hidden="true">"\u{2192}"</span>
                            <a
                                class="relationship-link"
                                href=target_href
                                target="_blank"
                                rel="noopener"
                            >{target}</a>
                            <Show when=move || typed.is_some()>
                                <button
                                    class="relationship-remove-btn"
                                    aria-label=crate::t!("relationship-remove-aria")
                                    on:click={
                                        let target_id = target_for_delete.clone();
                                        let delete_one = delete_one.clone();
                                        move |_| {
                                            if let Some(t) = typed {
                                                delete_one(target_id.clone(), t);
                                            }
                                        }
                                    }
                                >"\u{2715}"</button>
                            </Show>
                        </li>
                    }
                }).collect::<Vec<_>>()}
            </ul>

            <Show when=move || picker_open.get()>
                <div class="relationship-picker">
                    <div class="relationship-picker-row">
                        <select
                            class="relationship-type-select"
                            aria-label=crate::t!("relationship-type-aria")
                            on:change=move |ev| {
                                let val = event_target_value(&ev);
                                let next = match val.as_str() {
                                    "implements" => RelationType::Implements,
                                    "derived-from" => RelationType::DerivedFrom,
                                    "depends-on" => RelationType::DependsOn,
                                    "supersedes" => RelationType::Supersedes,
                                    _ => RelationType::References,
                                };
                                set_picker_relation.set(next);
                            }
                        >
                            {RelationType::all().iter().map(|t| {
                                let val = t.as_str();
                                let label = t.label();
                                view! {
                                    <option value=val>{label}</option>
                                }
                            }).collect::<Vec<_>>()}
                        </select>
                    </div>
                    <input
                        type="text"
                        class="relationship-picker-input"
                        placeholder=crate::t!("relationship-picker-placeholder")
                        aria-label=crate::t!("relationship-picker-aria")
                        prop:value=move || picker_query.get()
                        on:input=on_query_input
                    />
                    <Show when=move || !picker_results.get().is_empty()>
                        <ul class="relationship-picker-results">
                            {move || picker_results.get().into_iter().map(|r| {
                                let pick = r.clone();
                                let is_picked = picker_picked.get()
                                    .as_ref().map(|p| p.id == r.id).unwrap_or(false);
                                view! {
                                    <li
                                        class="relationship-picker-result"
                                        class:is-selected=is_picked
                                        on:click=move |_| set_picker_picked.set(Some(pick.clone()))
                                    >{r.title}</li>
                                }
                            }).collect::<Vec<_>>()}
                        </ul>
                    </Show>
                    <div class="relationship-picker-actions">
                        <button
                            class="btn btn-secondary"
                            on:click=move |_| close_picker()
                        >{crate::t!("common-cancel")}</button>
                        <button
                            class="btn btn-primary"
                            disabled=move || picker_picked.get().is_none()
                            on:click=move |_| submit_picker()
                        >{crate::t!("relationship-picker-confirm")}</button>
                    </div>
                </div>
            </Show>
        </div>
    }
}
