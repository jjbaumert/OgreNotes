// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Observability boot wiring: spawns the EMF emitter and the state sampler.
//!
//! Called once from `main.rs` after AppState is built. Returns JoinHandles
//! which are dropped immediately — the tasks live for the process lifetime.

use std::sync::Arc;
use std::time::Duration;

use ogrenotes_common::metrics::{emf, gauge, MetricKey, RollingUsers};

use crate::state::AppState;

pub fn spawn(state: AppState, deploy_env: String, rolling_users: Arc<RollingUsers>) {
    // EMF emitter: flushes metrics to stdout every 60s as CloudWatch EMF JSON.
    let _emf = emf::spawn_emf_emitter(deploy_env, Duration::from_secs(60));

    // State sampler: every 30s, read live registry + doc counts and update gauges.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        ticker.tick().await; // drop immediate tick
        loop {
            ticker.tick().await;
            sample(&state, &rolling_users).await;
        }
    });
}

async fn sample(state: &AppState, rolling_users: &RollingUsers) {
    // Active rooms / connections / unique logged-in users with open WS.
    let active_rooms = state.room_registry.room_count();
    gauge::set(MetricKey::new("service.active_rooms", &[]), active_rooms as i64);

    let idle = state.room_registry.idle_rooms(u64::MAX).await;
    gauge::set(MetricKey::new("service.idle_rooms", &[]), idle.len() as i64);

    // Rolling-user sweep.
    let (in_5m, in_60m) = rolling_users.sweep();
    gauge::set(
        MetricKey::new("service.active_users_rolling_5m", &[]),
        in_5m as i64,
    );
    gauge::set(
        MetricKey::new("service.active_users_rolling_60m", &[]),
        in_60m as i64,
    );
}
