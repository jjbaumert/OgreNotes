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
