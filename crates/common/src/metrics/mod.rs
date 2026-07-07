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
