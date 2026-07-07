// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! In-process metrics recorder: atomic counters, gauges, and summary histograms.
//!
//! The recorder is a process-global `OnceLock` populated by [`crate::metrics::init`].
//! All instrumentation call sites (counter/gauge/histogram helpers) look up their
//! slot here. Snapshots feed the EMF emitter task and the admin /metrics endpoint.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicI64, Ordering};
use std::sync::{Mutex, OnceLock};

use dashmap::DashMap;

/// A metric name + sorted label pairs. Hashed+compared as a tuple.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct MetricKey {
    pub name: &'static str,
    /// Sorted by key so two otherwise-identical keys always hash equal.
    pub labels: Vec<(&'static str, String)>,
}

impl MetricKey {
    pub fn new(name: &'static str, labels: &[(&'static str, &str)]) -> Self {
        let mut owned: Vec<(&'static str, String)> =
            labels.iter().map(|(k, v)| (*k, (*v).to_string())).collect();
        owned.sort_by(|a, b| a.0.cmp(b.0));
        Self { name, labels: owned }
    }
}

/// Histogram summary: count, sum, min, max. Sufficient for CloudWatch EMF
/// StatisticSet and for the admin-snapshot debug view.
#[derive(Debug, Default, Clone)]
pub struct HistogramSummary {
    pub count: u64,
    pub sum: f64,
    pub min: f64,
    pub max: f64,
}

impl HistogramSummary {
    fn record(&mut self, value: f64) {
        if self.count == 0 {
            self.min = value;
            self.max = value;
        } else {
            if value < self.min { self.min = value; }
            if value > self.max { self.max = value; }
        }
        self.count += 1;
        self.sum += value;
    }
}

#[derive(Default)]
pub struct Recorder {
    counters: DashMap<MetricKey, AtomicU64>,
    gauges: DashMap<MetricKey, AtomicI64>,
    /// Histograms held behind a Mutex — record path is rare and brief.
    histograms: DashMap<MetricKey, Mutex<HistogramSummary>>,
}

impl Recorder {
    pub fn counter_add(&self, key: MetricKey, value: u64) {
        if let Some(entry) = self.counters.get(&key) {
            entry.fetch_add(value, Ordering::Relaxed);
            return;
        }
        self.counters
            .entry(key)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(value, Ordering::Relaxed);
    }

    pub fn gauge_set(&self, key: MetricKey, value: i64) {
        self.gauges
            .entry(key)
            .or_insert_with(|| AtomicI64::new(0))
            .store(value, Ordering::Relaxed);
    }

    pub fn gauge_add(&self, key: MetricKey, delta: i64) {
        self.gauges
            .entry(key)
            .or_insert_with(|| AtomicI64::new(0))
            .fetch_add(delta, Ordering::Relaxed);
    }

    pub fn histogram_record(&self, key: MetricKey, value: f64) {
        let entry = self
            .histograms
            .entry(key)
            .or_insert_with(|| Mutex::new(HistogramSummary::default()));
        let mut guard = entry.lock().unwrap_or_else(|e| e.into_inner());
        guard.record(value);
    }

    /// Produce a point-in-time snapshot. Histograms are returned as current
    /// summaries without resetting — the EMF emitter resets via [`drain_histograms`].
    pub fn snapshot(&self) -> MetricsSnapshot {
        let mut counters: BTreeMap<String, u64> = BTreeMap::new();
        for entry in self.counters.iter() {
            counters.insert(format_key(entry.key()), entry.value().load(Ordering::Relaxed));
        }
        let mut gauges: BTreeMap<String, i64> = BTreeMap::new();
        for entry in self.gauges.iter() {
            gauges.insert(format_key(entry.key()), entry.value().load(Ordering::Relaxed));
        }
        let mut histograms: BTreeMap<String, HistogramSummary> = BTreeMap::new();
        for entry in self.histograms.iter() {
            let guard = entry.value().lock().unwrap_or_else(|e| e.into_inner());
            histograms.insert(format_key(entry.key()), guard.clone());
        }
        MetricsSnapshot { counters, gauges, histograms }
    }

    /// Drain all histograms — returns their current state and resets them to zero.
    /// Called by the EMF emitter so each flush reports delta since the last flush.
    pub fn drain_histograms(&self) -> Vec<(MetricKey, HistogramSummary)> {
        let mut out = Vec::new();
        for entry in self.histograms.iter() {
            let mut guard = entry.value().lock().unwrap_or_else(|e| e.into_inner());
            if guard.count > 0 {
                let taken = guard.clone();
                *guard = HistogramSummary::default();
                out.push((entry.key().clone(), taken));
            }
        }
        out
    }

    /// Raw access for the EMF emitter — returns counter/gauge pairs in
    /// their structured form (without formatting) so EMF can split labels
    /// into dimensions.
    pub fn raw_counters(&self) -> Vec<(MetricKey, u64)> {
        self.counters
            .iter()
            .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
            .collect()
    }

    pub fn raw_gauges(&self) -> Vec<(MetricKey, i64)> {
        self.gauges
            .iter()
            .map(|e| (e.key().clone(), e.value().load(Ordering::Relaxed)))
            .collect()
    }
}

fn format_key(key: &MetricKey) -> String {
    if key.labels.is_empty() {
        return key.name.to_string();
    }
    let mut s = String::with_capacity(key.name.len() + 16);
    s.push_str(key.name);
    s.push('{');
    for (i, (k, v)) in key.labels.iter().enumerate() {
        if i > 0 { s.push(','); }
        s.push_str(k);
        s.push('=');
        s.push('"');
        s.push_str(v);
        s.push('"');
    }
    s.push('}');
    s
}

/// JSON-serializable snapshot of all counters/gauges/histograms.
/// Returned by the admin endpoint and consumed by tests.
#[derive(Debug, serde::Serialize)]
pub struct MetricsSnapshot {
    pub counters: BTreeMap<String, u64>,
    pub gauges: BTreeMap<String, i64>,
    pub histograms: BTreeMap<String, HistogramSummary>,
}

impl serde::Serialize for HistogramSummary {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("HistogramSummary", 4)?;
        st.serialize_field("count", &self.count)?;
        st.serialize_field("sum", &self.sum)?;
        st.serialize_field("min", &self.min)?;
        st.serialize_field("max", &self.max)?;
        st.end()
    }
}

// ── Process-global recorder ─────────────────────────────────────

static RECORDER: OnceLock<Recorder> = OnceLock::new();

pub fn global() -> &'static Recorder {
    RECORDER.get_or_init(Recorder::default)
}

pub fn init() {
    let _ = RECORDER.set(Recorder::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_increments() {
        let r = Recorder::default();
        let k = MetricKey::new("foo.total", &[]);
        r.counter_add(k.clone(), 1);
        r.counter_add(k.clone(), 2);
        let snap = r.snapshot();
        assert_eq!(snap.counters["foo.total"], 3);
    }

    #[test]
    fn gauge_set_and_add() {
        let r = Recorder::default();
        let k = MetricKey::new("conns", &[]);
        r.gauge_set(k.clone(), 5);
        r.gauge_add(k.clone(), 2);
        r.gauge_add(k.clone(), -1);
        let snap = r.snapshot();
        assert_eq!(snap.gauges["conns"], 6);
    }

    #[test]
    fn histogram_tracks_stats() {
        let r = Recorder::default();
        let k = MetricKey::new("lat", &[]);
        r.histogram_record(k.clone(), 1.0);
        r.histogram_record(k.clone(), 5.0);
        r.histogram_record(k.clone(), 3.0);
        let snap = r.snapshot();
        let h = &snap.histograms["lat"];
        assert_eq!(h.count, 3);
        assert_eq!(h.sum, 9.0);
        assert_eq!(h.min, 1.0);
        assert_eq!(h.max, 5.0);
    }

    #[test]
    fn labeled_keys_are_distinct() {
        let r = Recorder::default();
        r.counter_add(MetricKey::new("req", &[("route", "/a")]), 1);
        r.counter_add(MetricKey::new("req", &[("route", "/b")]), 4);
        let snap = r.snapshot();
        assert_eq!(snap.counters[r#"req{route="/a"}"#], 1);
        assert_eq!(snap.counters[r#"req{route="/b"}"#], 4);
    }

    #[test]
    fn labels_are_sorted_for_stable_hashing() {
        let k1 = MetricKey::new("m", &[("a", "1"), ("b", "2")]);
        let k2 = MetricKey::new("m", &[("b", "2"), ("a", "1")]);
        assert_eq!(k1, k2);
    }

    #[test]
    fn drain_histograms_resets() {
        let r = Recorder::default();
        let k = MetricKey::new("lat", &[]);
        r.histogram_record(k.clone(), 2.0);
        let drained = r.drain_histograms();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].1.count, 1);
        // Second drain finds nothing (reset to zero count).
        let drained2 = r.drain_histograms();
        assert!(drained2.is_empty());
    }

    #[test]
    fn raw_accessors_return_structured_keys_for_emf() {
        // The EMF emitter consumes raw_counters/raw_gauges (not the formatted
        // snapshot) so it can split labels into CloudWatch dimensions. These
        // accessors had no coverage.
        let r = Recorder::default();
        r.counter_add(MetricKey::new("req", &[("route", "/a")]), 5);
        r.gauge_set(MetricKey::new("conns", &[]), 9);

        let counters = r.raw_counters();
        assert_eq!(counters.len(), 1);
        assert_eq!(counters[0].0, MetricKey::new("req", &[("route", "/a")]));
        assert_eq!(counters[0].1, 5);

        let gauges = r.raw_gauges();
        assert_eq!(gauges.len(), 1);
        assert_eq!(gauges[0].0, MetricKey::new("conns", &[]));
        assert_eq!(gauges[0].1, 9);
    }

    #[test]
    fn format_key_renders_labels_sorted_by_key() {
        // Labels are sorted at MetricKey construction, so the snapshot key is
        // deterministic regardless of the order they were passed in.
        let r = Recorder::default();
        r.counter_add(MetricKey::new("m", &[("b", "2"), ("a", "1")]), 1);
        let snap = r.snapshot();
        assert!(
            snap.counters.contains_key(r#"m{a="1",b="2"}"#),
            "expected sorted-label key, got {:?}",
            snap.counters.keys().collect::<Vec<_>>()
        );
    }
}
