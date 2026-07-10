// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! CloudWatch Embedded Metric Format (EMF) serializer.
//!
//! EMF documents are written to stdout as JSON lines; CloudWatch Logs
//! automatically extracts them into the `OgreNotes` metric namespace.
//! No scrape endpoint, no sidecar — the running container's log stream is
//! the delivery channel. See AWS docs: EMF spec v1.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use super::recorder::{global, MetricKey, Recorder};

const NAMESPACE: &str = "OgreNotes";
/// CloudWatch EMF caps at 100 metric definitions per document.
const MAX_METRICS_PER_DOC: usize = 100;

/// Per-emitter-task memory of the last-observed counter value per key. Used
/// to compute the per-flush delta for CloudWatch. The recorder keeps the
/// cumulative total (used by the admin /metrics endpoint); this mutex holds
/// only the last-seen value so `delta = current - previous`.
///
/// A process-global `Mutex<HashMap>` is fine here — the EMF emitter is a
/// single task flushing on a timer, so there's no contention in practice.
struct LastFlush {
    counters: HashMap<MetricKey, u64>,
}

static LAST_FLUSH: Mutex<Option<LastFlush>> = Mutex::new(None);

fn counter_delta(key: &MetricKey, current: u64, store: &mut LastFlush) -> u64 {
    let previous = store.counters.get(key).copied().unwrap_or(0);
    store.counters.insert(key.clone(), current);
    // Counters only ever increase, so `current >= previous`; saturating_sub
    // is defence-in-depth against a future reset/overflow path.
    current.saturating_sub(previous)
}

/// Spawn the EMF flusher. Emits one or more EMF JSON-line documents every
/// `interval`, reporting counter **deltas** (events-per-interval) and
/// histogram **deltas** (drain-and-reset), plus current gauge values.
pub fn spawn_emf_emitter(deploy_env: String, interval: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await; // drop immediate first tick
        loop {
            ticker.tick().await;
            emit_once(global(), &deploy_env);
        }
    })
}

pub fn emit_once(recorder: &Recorder, deploy_env: &str) {
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    for doc in collect_emf_docs(recorder, deploy_env, ts_ms) {
        // Print as a single JSON line — CloudWatch Logs ingests line-by-line.
        println!("{doc}");
    }
}

/// Build the flush's EMF documents without emitting them. Separated from
/// `emit_once` so the document set is observable by tests.
fn collect_emf_docs(recorder: &Recorder, deploy_env: &str, ts_ms: u64) -> Vec<String> {
    // Group metrics by their full label set — keys AND values — so each EMF
    // doc carries exactly one dimension-value combination, per the EMF spec.
    // Keys alone aren't enough: req{route="/a"} and req{route="/b"} share
    // dimension keys, but a single doc has one top-level `route` and one
    // `req` field, so whichever series lands last would silently overwrite
    // the other and drop its data point for the flush interval (issue #10).
    type BucketKey = (Vec<&'static str>, Vec<(String, String)>);
    let mut buckets: std::collections::BTreeMap<BucketKey, Vec<EmfMetric>> =
        std::collections::BTreeMap::new();

    // Hold the LAST_FLUSH lock only for the counter-delta pass; the gauge and
    // histogram loops below don't need it. Scoping it to this block releases
    // the lock at the brace instead of via a manual `drop` further down.
    {
        let mut last = LAST_FLUSH.lock().unwrap_or_else(|e| e.into_inner());
        let store = last.get_or_insert_with(|| LastFlush { counters: HashMap::new() });

        for (key, current) in recorder.raw_counters() {
            let delta = counter_delta(&key, current, store);
            if delta == 0 {
                // Suppress no-op counter entries so a mostly-idle service doesn't
                // emit a hundred zero-valued counters per flush.
                continue;
            }
            let dim_keys: Vec<&'static str> = key.labels.iter().map(|(k, _)| *k).collect();
            let label_values: Vec<(String, String)> =
                key.labels.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
            buckets.entry((dim_keys, label_values.clone())).or_default().push(EmfMetric {
                name: key.name.to_string(),
                unit: "Count",
                values: vec![delta as f64],
                label_values,
                stats: None,
            });
        }
    }

    for (key, value) in recorder.raw_gauges() {
        let dim_keys: Vec<&'static str> = key.labels.iter().map(|(k, _)| *k).collect();
        let label_values: Vec<(String, String)> =
            key.labels.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
        buckets.entry((dim_keys, label_values.clone())).or_default().push(EmfMetric {
            name: key.name.to_string(),
            unit: "None",
            values: vec![value as f64],
            label_values,
            stats: None,
        });
    }
    for (key, summary) in recorder.drain_histograms() {
        let dim_keys: Vec<&'static str> = key.labels.iter().map(|(k, _)| *k).collect();
        let unit = if key.name.ends_with("_ms") {
            "Milliseconds"
        } else if key.name.ends_with("_bytes") {
            "Bytes"
        } else {
            "None"
        };
        let label_values: Vec<(String, String)> =
            key.labels.iter().map(|(k, v)| ((*k).to_string(), v.clone())).collect();
        buckets.entry((dim_keys, label_values.clone())).or_default().push(EmfMetric {
            name: key.name.to_string(),
            unit,
            values: vec![],
            label_values,
            stats: Some(StatSet {
                count: summary.count,
                sum: summary.sum,
                min: summary.min,
                max: summary.max,
            }),
        });
    }

    let mut docs = Vec::new();
    for ((dim_keys, _label_values), metrics) in buckets {
        for chunk in metrics.chunks(MAX_METRICS_PER_DOC) {
            docs.push(build_emf_doc(ts_ms, deploy_env, &dim_keys, chunk));
        }
    }
    docs
}

struct EmfMetric {
    name: String,
    unit: &'static str,
    values: Vec<f64>,
    label_values: Vec<(String, String)>,
    stats: Option<StatSet>,
}

struct StatSet {
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
}

fn build_emf_doc(
    ts_ms: u64,
    deploy_env: &str,
    dim_keys: &[&'static str],
    metrics: &[EmfMetric],
) -> String {
    // Dimensions sets — two entries: [[Environment]] always, plus
    // [[Environment, <label1>, ...]] when the bucket has labels.
    let mut dim_sets: Vec<Vec<String>> = vec![vec!["Environment".to_string()]];
    if !dim_keys.is_empty() {
        let mut detailed = vec!["Environment".to_string()];
        detailed.extend(dim_keys.iter().map(|s| (*s).to_string()));
        dim_sets.push(detailed);
    }

    let metric_defs: Vec<Value> = metrics
        .iter()
        .map(|m| json!({ "Name": m.name, "Unit": m.unit }))
        .collect();

    let mut root = serde_json::Map::new();
    root.insert(
        "_aws".to_string(),
        json!({
            "Timestamp": ts_ms,
            "CloudWatchMetrics": [{
                "Namespace": NAMESPACE,
                "Dimensions": dim_sets,
                "Metrics": metric_defs,
            }]
        }),
    );
    root.insert("Environment".to_string(), json!(deploy_env));

    // Emit the metric values + dimension values at the top level.
    for m in metrics {
        for (k, v) in &m.label_values {
            root.insert(k.clone(), json!(v));
        }
        if let Some(stats) = &m.stats {
            root.insert(m.name.clone(), json!({
                "Count": stats.count,
                "Sum": stats.sum,
                "Min": stats.min,
                "Max": stats.max,
            }));
        } else if m.values.len() == 1 {
            root.insert(m.name.clone(), json!(m.values[0]));
        } else {
            root.insert(m.name.clone(), json!(m.values));
        }
    }

    serde_json::to_string(&Value::Object(root)).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::recorder::MetricKey;

    #[test]
    fn emf_doc_has_aws_block() {
        let r = Recorder::default();
        r.counter_add(MetricKey::new("req_total", &[]), 3);
        r.gauge_set(MetricKey::new("active", &[]), 7);
        r.histogram_record(MetricKey::new("lat_ms", &[]), 42.0);

        // Capture stdout by building the doc directly via the helper.
        // We can at least exercise emit_once path without inspecting stdout
        // (verifying it does not panic). For shape, build one doc manually.
        let doc = build_emf_doc(
            1,
            "test",
            &[],
            &[EmfMetric {
                name: "req_total".to_string(),
                unit: "Count",
                values: vec![3.0],
                label_values: vec![],
                stats: None,
            }],
        );
        let v: Value = serde_json::from_str(&doc).unwrap();
        assert!(v["_aws"]["CloudWatchMetrics"][0]["Namespace"]
            .as_str()
            .unwrap()
            .eq("OgreNotes"));
        assert_eq!(v["Environment"], "test");
        assert_eq!(v["req_total"], 3.0);

        emit_once(&r, "test");
    }

    #[test]
    fn emf_doc_with_labels_includes_dimensions() {
        let doc = build_emf_doc(
            1,
            "prod",
            &["doc_type"],
            &[EmfMetric {
                name: "doc_created_total".to_string(),
                unit: "Count",
                values: vec![5.0],
                label_values: vec![("doc_type".to_string(), "document".to_string())],
                stats: None,
            }],
        );
        let v: Value = serde_json::from_str(&doc).unwrap();
        let dims = &v["_aws"]["CloudWatchMetrics"][0]["Dimensions"];
        assert!(dims.is_array());
        assert_eq!(dims[0][0], "Environment");
        assert_eq!(dims[1][0], "Environment");
        assert_eq!(dims[1][1], "doc_type");
        assert_eq!(v["doc_type"], "document");
    }

    #[test]
    fn counter_delta_first_seen_equals_current() {
        let mut store = LastFlush { counters: HashMap::new() };
        let k = MetricKey::new("foo", &[]);
        assert_eq!(counter_delta(&k, 5, &mut store), 5);
        assert_eq!(counter_delta(&k, 8, &mut store), 3);
        assert_eq!(counter_delta(&k, 8, &mut store), 0);
    }

    #[test]
    fn counter_delta_handles_reset_safely() {
        // If the recorder is ever reset (e.g. process restart carried state
        // forward incorrectly), a "current < previous" case must not panic.
        let mut store = LastFlush { counters: HashMap::new() };
        let k = MetricKey::new("foo", &[]);
        counter_delta(&k, 10, &mut store);
        assert_eq!(counter_delta(&k, 3, &mut store), 0);
    }

    #[test]
    fn emf_doc_lists_metric_definitions_with_units() {
        // The _aws.CloudWatchMetrics[0].Metrics array is how CloudWatch learns
        // which top-level fields are metrics and what unit each carries. If it
        // is wrong or missing, the values are ingested as plain log fields and
        // silently never become metrics. This array had no coverage.
        let doc = build_emf_doc(
            1,
            "prod",
            &[],
            &[
                EmfMetric {
                    name: "a_total".to_string(),
                    unit: "Count",
                    values: vec![2.0],
                    label_values: vec![],
                    stats: None,
                },
                EmfMetric {
                    name: "lat_ms".to_string(),
                    unit: "Milliseconds",
                    values: vec![],
                    label_values: vec![],
                    stats: Some(StatSet { count: 1, sum: 3.0, min: 3.0, max: 3.0 }),
                },
            ],
        );
        let v: Value = serde_json::from_str(&doc).unwrap();
        let defs = v["_aws"]["CloudWatchMetrics"][0]["Metrics"]
            .as_array()
            .expect("Metrics array");
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0]["Name"], "a_total");
        assert_eq!(defs[0]["Unit"], "Count");
        assert_eq!(defs[1]["Name"], "lat_ms");
        assert_eq!(defs[1]["Unit"], "Milliseconds");
        // Both metrics' values are present at the document root.
        assert_eq!(v["a_total"], 2.0);
        assert_eq!(v["lat_ms"]["Sum"], 3.0);
    }

    #[test]
    fn emf_doc_without_labels_has_single_dimension_set() {
        // No labels → exactly one dimension set [["Environment"]]. A stray
        // second (empty-ish) set would make CloudWatch mis-aggregate the doc.
        let doc = build_emf_doc(
            1,
            "test",
            &[],
            &[EmfMetric {
                name: "x".to_string(),
                unit: "Count",
                values: vec![1.0],
                label_values: vec![],
                stats: None,
            }],
        );
        let v: Value = serde_json::from_str(&doc).unwrap();
        let dims = v["_aws"]["CloudWatchMetrics"][0]["Dimensions"]
            .as_array()
            .expect("Dimensions array");
        assert_eq!(dims.len(), 1);
        assert_eq!(dims[0], json!(["Environment"]));
    }

    /// Regression: issue #10 — the same metric name with different label
    /// values must yield one EMF document per dimension-value combination.
    /// Bucketing by label *keys* alone merged them into one document whose
    /// single top-level field silently dropped all but the last series.
    #[test]
    fn same_metric_different_label_values_get_separate_docs() {
        let r = Recorder::default();
        // Unique name: the counter-delta store is process-global.
        r.counter_add(
            MetricKey::new("req_emf_split_test_total", &[("route", "/a")]),
            5,
        );
        r.counter_add(
            MetricKey::new("req_emf_split_test_total", &[("route", "/b")]),
            3,
        );

        let docs: Vec<Value> = collect_emf_docs(&r, "test", 1)
            .iter()
            .map(|d| serde_json::from_str(d).unwrap())
            .collect();

        // Both data points must survive the flush, each in a document whose
        // dimension value matches its own series.
        let find = |route: &str| {
            docs.iter().find(|v| v["route"] == route).unwrap_or_else(|| {
                panic!("no document for route={route}; docs: {docs:?}")
            })
        };
        assert_eq!(find("/a")["req_emf_split_test_total"], 5.0);
        assert_eq!(find("/b")["req_emf_split_test_total"], 3.0);
    }

    #[test]
    fn emf_doc_with_histogram_uses_statset() {
        let doc = build_emf_doc(
            1,
            "prod",
            &[],
            &[EmfMetric {
                name: "lat_ms".to_string(),
                unit: "Milliseconds",
                values: vec![],
                label_values: vec![],
                stats: Some(StatSet { count: 3, sum: 9.0, min: 1.0, max: 5.0 }),
            }],
        );
        let v: Value = serde_json::from_str(&doc).unwrap();
        assert_eq!(v["lat_ms"]["Count"], 3);
        assert_eq!(v["lat_ms"]["Sum"], 9.0);
        assert_eq!(v["lat_ms"]["Min"], 1.0);
        assert_eq!(v["lat_ms"]["Max"], 5.0);
    }
}
