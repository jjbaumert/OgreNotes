// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! In-process metrics facade.
//!
//! Entry points for instrumentation are the `counter`, `gauge`, and `histogram`
//! submodules. A tiny [`MetricKey`] carries name + labels; all state lives in
//! a process-global [`recorder::Recorder`].
//!
//! The facade is intentionally narrower than what a full metrics-crate facade
//! provides — you get counter add, gauge set/add, histogram record, and a
//! snapshot for the admin endpoint. EMF emission and rolling-user tracking are
//! helpers built on top.

pub mod emf;
pub mod recorder;
pub mod rolling_users;
pub mod tracing_layer;

pub use recorder::{HistogramSummary, MetricKey, MetricsSnapshot};
pub use rolling_users::RollingUsers;
pub use tracing_layer::LogEventCounterLayer;

use recorder::global;

/// Initialise the process-global recorder. Safe to call exactly once at boot.
pub fn init() {
    recorder::init();
}

/// Snapshot of all metrics. Used by the admin /metrics endpoint.
pub fn snapshot() -> MetricsSnapshot {
    global().snapshot()
}

/// Counter primitives.
pub mod counter {
    use super::*;

    pub fn add(key: MetricKey, value: u64) {
        global().counter_add(key, value);
    }

    pub fn inc(key: MetricKey) {
        global().counter_add(key, 1);
    }
}

/// Gauge primitives.
pub mod gauge {
    use super::*;

    pub fn set(key: MetricKey, value: i64) {
        global().gauge_set(key, value);
    }

    pub fn add(key: MetricKey, delta: i64) {
        global().gauge_add(key, delta);
    }
}

/// Histogram primitives.
pub mod histogram {
    use super::*;

    pub fn record(key: MetricKey, value: f64) {
        global().histogram_record(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests exercise the public facade against the process-global
    // recorder — the path every instrumentation call site in the workspace
    // actually uses. Metric names are unique to this module so parallel
    // tests sharing the global recorder cannot interfere.

    #[test]
    fn facade_counter_inc_and_add_reach_global_recorder() {
        let key = MetricKey::new("facade_test.counter_total", &[]);
        counter::inc(key.clone());
        counter::add(key.clone(), 4);
        let snap = snapshot();
        assert_eq!(snap.counters["facade_test.counter_total"], 5);
    }

    #[test]
    fn facade_gauge_set_and_add_reach_global_recorder() {
        let key = MetricKey::new("facade_test.gauge", &[]);
        gauge::set(key.clone(), 10);
        gauge::add(key.clone(), -3);
        let snap = snapshot();
        assert_eq!(snap.gauges["facade_test.gauge"], 7);
    }

    #[test]
    fn facade_histogram_record_reaches_global_recorder() {
        let key = MetricKey::new("facade_test.hist_ms", &[]);
        histogram::record(key.clone(), 2.0);
        histogram::record(key.clone(), 8.0);
        let snap = snapshot();
        let h = &snap.histograms["facade_test.hist_ms"];
        assert_eq!(h.count, 2);
        assert_eq!(h.sum, 10.0);
    }

    #[test]
    fn init_is_idempotent_and_preserves_state() {
        // init() after the global is already populated must not wipe
        // previously recorded metrics (OnceLock::set is a no-op then).
        let key = MetricKey::new("facade_test.init_total", &[]);
        counter::inc(key.clone());
        init();
        let snap = snapshot();
        assert_eq!(snap.counters["facade_test.init_total"], 1);
    }
}
