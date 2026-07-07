// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Admin metrics page — flat tables for the in-process counter /
//! gauge / histogram registries. A "Refresh" button re-fetches the
//! snapshot; we deliberately don't auto-poll so an operator on a
//! shared dashboard doesn't burn server time without intent.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::admin::{self, HistogramSummary, MetricsSnapshot};

use super::AdminGate;

#[component]
pub fn AdminMetricsPage() -> impl IntoView {
    view! {
        <AdminGate>
            <MetricsView />
        </AdminGate>
    }
}

#[component]
fn MetricsView() -> impl IntoView {
    let (snapshot, set_snapshot) = signal::<Option<MetricsSnapshot>>(None);
    let (error, set_error) = signal::<Option<String>>(None);
    let (busy, set_busy) = signal::<bool>(false);

    let refresh = move || {
        set_busy.set(true);
        set_error.set(None);
        spawn_local(async move {
            match admin::metrics().await {
                Ok(s) => set_snapshot.set(Some(s)),
                Err(e) => set_error.set(Some(crate::t!("admin-metrics-error-fetch-failed", err = format!("{e:?}")))),
            }
            set_busy.set(false);
        });
    };

    refresh();

    view! {
        <main id="main-content" tabindex="-1" class="admin-metrics">
            <h1>{crate::t!("admin-metrics-title")}</h1>

            <button on:click=move |_| refresh() disabled=move || busy.get()>
                {crate::t!("admin-metrics-refresh")}
            </button>

            {move || error.get().map(|msg| view! {
                <div class="admin-error">{msg}</div>
            })}

            {move || snapshot.get().map(|s| {
                // Move each map's entries out by value so the views
                // generated below outlive `s` (which the closure
                // drops at end of scope).
                let counters: Vec<(String, u64)> = s.counters.into_iter().collect();
                let gauges: Vec<(String, i64)> = s.gauges.into_iter().collect();
                let histograms: Vec<(String, HistogramSummary)> =
                    s.histograms.into_iter().collect();
                view! {
                    <section>
                        <h2>{crate::t!("admin-metrics-counters")}</h2>
                        <table class="admin-metrics-table">
                            <thead><tr><th>{crate::t!("admin-metrics-th-key")}</th><th>{crate::t!("admin-metrics-th-value")}</th></tr></thead>
                            <tbody>
                                {counters
                                    .into_iter()
                                    .map(|(k, v)| view! { <tr><td>{k}</td><td>{v}</td></tr> })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </section>

                    <section>
                        <h2>{crate::t!("admin-metrics-gauges")}</h2>
                        <table class="admin-metrics-table">
                            <thead><tr><th>{crate::t!("admin-metrics-th-key")}</th><th>{crate::t!("admin-metrics-th-value")}</th></tr></thead>
                            <tbody>
                                {gauges
                                    .into_iter()
                                    .map(|(k, v)| view! { <tr><td>{k}</td><td>{v}</td></tr> })
                                    .collect_view()}
                            </tbody>
                        </table>
                    </section>

                    <section>
                        <h2>{crate::t!("admin-metrics-histograms")}</h2>
                        <table class="admin-metrics-table">
                            <thead>
                                <tr>
                                    <th>{crate::t!("admin-metrics-th-key")}</th>
                                    <th>{crate::t!("admin-metrics-th-count")}</th>
                                    <th>{crate::t!("admin-metrics-th-sum")}</th>
                                    <th>{crate::t!("admin-metrics-th-min")}</th>
                                    <th>{crate::t!("admin-metrics-th-max")}</th>
                                </tr>
                            </thead>
                            <tbody>
                                {histograms
                                    .into_iter()
                                    .map(|(k, h)| histogram_row(k, h))
                                    .collect_view()}
                            </tbody>
                        </table>
                    </section>
                }
            })}
        </main>
    }
}

fn histogram_row(key: String, h: HistogramSummary) -> impl IntoView {
    view! {
        <tr>
            <td>{key}</td>
            <td>{h.count}</td>
            <td>{h.sum}</td>
            <td>{h.min}</td>
            <td>{h.max}</td>
        </tr>
    }
}
