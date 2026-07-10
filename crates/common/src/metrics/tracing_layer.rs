// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Tracing Layer that counts warn / error / info log events.
//!
//! Wired into the tracing subscriber in `main.rs`. Lets CloudWatch answer
//! "how many warnings and errors is the service emitting?" directly from
//! metrics — no need to scan log lines.

use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use super::{counter, recorder::MetricKey};

pub struct LogEventCounterLayer;

impl<S: Subscriber> Layer<S> for LogEventCounterLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        // Only count warn+ — debug/info volume would dominate and the
        // metric is meant to answer "how many warnings and errors?".
        let level = match *meta.level() {
            Level::ERROR => "error",
            Level::WARN => "warn",
            _ => return,
        };
        // Deliberately no `target` label — `meta.target()` is the module
        // path of the emitting site (every crate, module, and 3rd-party
        // dep). Including it would produce unbounded cardinality. Query
        // the raw log line in CloudWatch Logs Insights if you need to
        // split by module.
        counter::add(MetricKey::new("log.events_total", &[("level", level)]), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics;
    use tracing_subscriber::layer::SubscriberExt;

    /// Current value of `log.events_total{level=...}` in the global recorder.
    /// The layer writes to process-global state, so assertions are made on
    /// before/after deltas to stay robust against other tests in the binary.
    fn level_count(level: &str) -> u64 {
        metrics::snapshot()
            .counters
            .get(&format!(r#"log.events_total{{level="{level}"}}"#))
            .copied()
            .unwrap_or(0)
    }

    #[test]
    fn counts_warn_and_error_events_but_not_info() {
        let warn_before = level_count("warn");
        let error_before = level_count("error");

        let subscriber = tracing_subscriber::registry().with(LogEventCounterLayer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!("test warning");
            tracing::warn!("another warning");
            tracing::error!("test error");
            // Below warn — must NOT be counted (volume would dominate).
            tracing::info!("test info");
            tracing::debug!("test debug");
            tracing::trace!("test trace");
        });

        assert_eq!(
            level_count("warn") - warn_before,
            2,
            "each warn event increments the warn counter exactly once"
        );
        assert_eq!(
            level_count("error") - error_before,
            1,
            "each error event increments the error counter exactly once"
        );
        assert_eq!(
            level_count("info"),
            0,
            "info events must never mint a level=\"info\" series"
        );
    }
}
