//! Exercise the rate gate through real HTTP requests to a local Quip stub.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ogrenotes_quip_import::{QuipClient, QuipToken};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[tokio::test]
async fn concurrent_requests_after_bucket_exhaustion_reach_quip_in_separate_slots() {
    let server = MockServer::start().await;
    let arrivals = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&arrivals);
    Mock::given(method("GET"))
        .and(path("/1/users/current"))
        .respond_with(move |_: &Request| {
            observed.lock().unwrap().push(Instant::now());
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "local-user", "name": "Local fixture", "private_folder_id": "local-folder"
            }))
        })
        .expect(48)
        .mount(&server)
        .await;
    let client = QuipClient::new(Some(server.uri()));
    let token = QuipToken::new("local-fixture-token".into());

    // Consume the default 45-token burst through the same public client.
    for _ in 0..45 {
        client.current_user(&token).await.unwrap();
    }
    let (a, b, c) = tokio::join!(
        client.current_user(&token),
        client.current_user(&token),
        client.current_user(&token),
    );
    for response in [a, b, c] {
        response.unwrap();
    }

    let arrivals = arrivals.lock().unwrap();
    let gaps: Vec<_> = arrivals[45..]
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .collect();
    eprintln!("post-burst HTTP arrival gaps: {gaps:?}");
    // The target interval is 1333 ms. Allow scheduler/network variance but
    // reject the original same-token burst, whose gaps are near zero.
    assert!(
        gaps.iter().all(|gap| *gap >= Duration::from_millis(1_100)),
        "concurrent callers reused one future token: {gaps:?}"
    );
}

use ogrenotes_quip_import::throttle::plan_delay;
use ogrenotes_quip_import::{RateState, Throttle};
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;
use tokio::task::JoinSet;
use tokio::time::{Instant as TokioInstant, advance};

async fn assert_pending<F: Future<Output = ()>>(mut future: Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(
            future.as_mut().poll(cx).is_pending(),
            "request admitted too early"
        );
        Poll::Ready(())
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn sustained_concurrent_acquires_obey_cumulative_bucket_capacity() {
    let throttle = Arc::new(Throttle::with_rate(3));
    let start = TokioInstant::now();
    let mut grants = Vec::new();
    for _ in 0..3 {
        throttle.acquire().await;
        grants.push(start.elapsed());
    }
    let mut callers = JoinSet::new();
    for _ in 0..6 {
        let throttle = Arc::clone(&throttle);
        callers.spawn(async move {
            let mut grants = Vec::new();
            for _ in 0..4 {
                throttle.acquire().await;
                grants.push(start.elapsed());
            }
            grants
        });
    }
    while let Some(result) = callers.join_next().await {
        grants.extend(result.unwrap());
    }
    grants.sort();
    assert_eq!(grants.len(), 27);
    for (index, elapsed) in grants.iter().enumerate() {
        let budget = 3 + elapsed.as_millis() * 3 / 60_000;
        assert!(
            index as u128 + 1 <= budget,
            "grant {} at {elapsed:?} exceeds {budget}",
            index + 1
        );
    }
    assert!(
        grants.last().unwrap() <= &Duration::from_secs(481),
        "waiters did not make progress"
    );
}

#[tokio::test(start_paused = true)]
async fn server_reset_does_not_release_more_than_bucket_capacity() {
    let throttle = Arc::new(Throttle::with_rate(2));
    let reset = ogrenotes_common::time::now_usec() / 1000 + 10_000;
    throttle.observe_headers(Some(0), Some(reset));
    let start = TokioInstant::now();
    let mut callers = JoinSet::new();
    for _ in 0..5 {
        let throttle = Arc::clone(&throttle);
        callers.spawn(async move {
            throttle.acquire().await;
            start.elapsed()
        });
    }
    let mut grants = Vec::new();
    while let Some(result) = callers.join_next().await {
        grants.push(result.unwrap());
    }
    grants.sort();
    assert!(grants[0] >= Duration::from_millis(9_999));
    assert!(grants[1] < Duration::from_secs(11));
    for pair in grants[1..].windows(2) {
        assert!(pair[1] - pair[0] >= Duration::from_secs(30));
    }
}

#[tokio::test(start_paused = true)]
async fn sleeping_acquire_rechecks_extended_and_shortened_reset_headers() {
    let throttle = Throttle::with_rate(60);
    for _ in 0..60 {
        throttle.acquire().await;
    }
    let epoch = ogrenotes_common::time::now_usec() / 1000;
    let mut waiting = Box::pin(throttle.acquire());
    assert_pending(waiting.as_mut()).await;
    advance(Duration::from_millis(500)).await;
    throttle.observe_headers(Some(0), Some(epoch + 10_000));
    assert_pending(waiting.as_mut()).await;
    advance(Duration::from_millis(1_500)).await;
    // Its original token is now available, but the newer header forbids it.
    assert_pending(waiting.as_mut()).await;
    throttle.observe_headers(Some(1), Some(epoch + 10_000));
    // A fresh positive hint removes the floor and wakes the old sleep now.
    std::future::poll_fn(|cx| {
        assert!(
            waiting.as_mut().poll(cx).is_ready(),
            "fresh headers did not wake admission"
        );
        Poll::Ready(())
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn cancelling_waiters_does_not_spend_future_tokens() {
    let throttle = Throttle::with_rate(60);
    for _ in 0..60 {
        throttle.acquire().await;
    }
    let mut cancelled = Box::pin(throttle.acquire());
    assert_pending(cancelled.as_mut()).await;
    let mut survivor = Box::pin(throttle.acquire());
    assert_pending(survivor.as_mut()).await;
    drop(cancelled);
    advance(Duration::from_secs(1)).await;
    std::future::poll_fn(|cx| {
        assert!(
            survivor.as_mut().poll(cx).is_ready(),
            "cancelled waiter stranded a token"
        );
        Poll::Ready(())
    })
    .await;
    let start = TokioInstant::now();
    throttle.acquire().await;
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "the admitted token was not spent"
    );
}

#[test]
fn calculator_reserves_distinct_tokens_at_the_reset_floor() {
    let mut state = RateState::full(2, 0);
    state.remaining_hint = Some(0);
    state.reset_at_ms = Some(10_000);
    let delays: Vec<_> = (0..5).map(|_| plan_delay(&mut state, 0, 2)).collect();
    assert_eq!(delays, [10_000, 10_000, 40_000, 70_000, 100_000]);
}

#[test]
fn calculator_does_not_refill_twice_after_a_backward_clock_jump() {
    let mut state = RateState::full(60, 1_000);
    for _ in 0..60 {
        assert_eq!(plan_delay(&mut state, 1_000, 60), 0);
    }
    assert_eq!(plan_delay(&mut state, 0, 60), 2_000);
    assert_eq!(plan_delay(&mut state, 1_000, 60), 2_000);
    assert_eq!(plan_delay(&mut state, 2_000, 60), 2_000);
}
