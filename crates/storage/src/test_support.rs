// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Test-only builders for repo tests that must not touch DynamoDB Local.
//!
//! `replaying_dynamo` answers each SDK call with the next canned JSON
//! body and records the request actually put on the wire, so a test can
//! assert request *shape* (ConditionExpression, Limit, ExclusiveStartKey)
//! — the only way to pin behaviour DynamoDB Local cannot distinguish.

use crate::dynamo::DynamoClient;
use aws_smithy_runtime::client::http::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;

/// A `DynamoClient` whose HTTP layer replays `responses` (all HTTP 200)
/// in order and records every request the SDK emitted.
pub(crate) fn replaying_dynamo(responses: Vec<&str>) -> (DynamoClient, StaticReplayClient) {
    replaying_dynamo_with_status(responses.into_iter().map(|b| (200, b)).collect())
}

/// Like `replaying_dynamo` but each response carries its own HTTP status,
/// for error-path tests (DynamoDB signals service errors with 400 and a
/// `__type` field in the body).
pub(crate) fn replaying_dynamo_with_status(
    responses: Vec<(u16, &str)>,
) -> (DynamoClient, StaticReplayClient) {
    let replay = StaticReplayClient::new(
        responses
            .into_iter()
            .map(|(status, body)| {
                ReplayEvent::new(
                    http::Request::builder()
                        .uri("http://localhost/")
                        .body(SdkBody::from("{}"))
                        .unwrap(),
                    http::Response::builder()
                        .status(status)
                        .body(SdkBody::from(body))
                        .unwrap(),
                )
            })
            .collect(),
    );
    let conf = aws_sdk_dynamodb::config::Builder::new()
        .endpoint_url("http://localhost:9999")
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "k", "s", None, None, "test",
        ))
        .http_client(replay.clone())
        .behavior_version_latest()
        .build();
    let db = DynamoClient::new(
        aws_sdk_dynamodb::Client::from_conf(conf),
        "test-table".to_string(),
    );
    (db, replay)
}

/// A replayed `ConditionalCheckFailedException` body; pair with status
/// 400 in `replaying_dynamo_with_status`.
pub(crate) const CONDITIONAL_CHECK_FAILED: &str = r#"{"__type":"com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException","message":"The conditional request failed"}"#;

/// Body of the `idx`-th request the SDK emitted, as UTF-8.
pub(crate) fn request_body(replay: &StaticReplayClient, idx: usize) -> String {
    let reqs: Vec<_> = replay.actual_requests().collect();
    let req = reqs
        .get(idx)
        .unwrap_or_else(|| panic!("no request #{idx}; {} recorded", reqs.len()));
    String::from_utf8(req.body().bytes().expect("in-memory body").to_vec()).expect("utf-8 body")
}

/// `x-amz-target` of the `idx`-th request, e.g. `DynamoDB_20120810.Query`.
pub(crate) fn request_target(replay: &StaticReplayClient, idx: usize) -> String {
    let reqs: Vec<_> = replay.actual_requests().collect();
    let req = reqs
        .get(idx)
        .unwrap_or_else(|| panic!("no request #{idx}; {} recorded", reqs.len()));
    req.headers().get("x-amz-target").unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn replaying_dynamo_records_the_request_it_answered() {
        let (db, replay) = replaying_dynamo(vec![r#"{"Items":[]}"#]);
        let items = db.query("PK1", None).await.expect("query");
        assert!(items.is_empty());
        assert_eq!(request_target(&replay, 0), "DynamoDB_20120810.Query");
        assert!(request_body(&replay, 0).contains(r#""TableName":"test-table""#));
    }
}
