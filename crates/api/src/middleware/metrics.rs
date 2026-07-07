// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Per-request metrics middleware.
//!
//! Counts every HTTP request (by method/route/status) and records request
//! latency into the histogram facade. Runs alongside the existing
//! `TraceLayer`; does not duplicate its work.

use std::time::Instant;

use axum::body::Body;
use axum::extract::{MatchedPath, Request};
use axum::http::Response;
use axum::middleware::Next;

use ogrenotes_common::metrics::{counter, histogram, MetricKey};

pub async fn track(req: Request, next: Next) -> Response<Body> {
    let method = req.method().as_str().to_string();
    // Use the matched route template (e.g. `/documents/{id}`) instead of the
    // concrete path, so we don't explode cardinality by doc_id.
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|mp| mp.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let start = Instant::now();
    let response = next.run(req).await;
    let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
    let status = response.status().as_u16().to_string();

    counter::inc(MetricKey::new(
        "api.requests_total",
        &[("method", &method), ("route", &route), ("status", &status)],
    ));
    histogram::record(
        MetricKey::new(
            "api.request_latency_ms",
            &[("method", &method), ("route", &route), ("status", &status)],
        ),
        latency_ms,
    );

    response
}
