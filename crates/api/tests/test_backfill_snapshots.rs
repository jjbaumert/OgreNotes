// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration test for the snapshot backfill migration
//! (`ogrenotes_api::backfill::run_backfill_snapshots`).
//!
//! The migration walks `docs/{id}/snapshots/*.bin` S3 objects and writes the
//! missing SNAPSHOT# rows. Only the pure `parse_snapshot_key` parser was
//! covered before; the S3-walk → get → insert loop (including the
//! already-present idempotency skip) was trapped in the binary's `main`. The
//! logic now lives in the library so this drives it against real MinIO +
//! DynamoDB. Headline property: a re-run inserts nothing.

mod common;

use ogrenotes_api::backfill::run_backfill_snapshots;

#[tokio::test]
async fn backfill_inserts_missing_snapshot_row_and_is_idempotent() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let doc_id = "doc-snap-backfill";
    let version: u64 = 5;
    let key = format!("docs/{doc_id}/snapshots/{version}.bin");

    // Stage a snapshot blob with NO matching SNAPSHOT# row, plus a non-snapshot
    // object that must be skipped by the key parser.
    app.s3_client()
        .put_object()
        .bucket(&app.bucket)
        .key(&key)
        .body(aws_sdk_s3::primitives::ByteStream::from(b"snapshot-bytes".to_vec()))
        .send()
        .await
        .expect("stage snapshot blob");
    app.s3_client()
        .put_object()
        .bucket(&app.bucket)
        .key(format!("docs/{doc_id}/blobs/photo.png"))
        .body(aws_sdk_s3::primitives::ByteStream::from(b"not-a-snapshot".to_vec()))
        .send()
        .await
        .expect("stage non-snapshot blob");

    assert!(
        app.state.snapshot_repo.get(doc_id, version).await.unwrap().is_none(),
        "precondition: no SNAPSHOT# row yet"
    );

    // First run: inserts the missing row, skips the non-snapshot key.
    let first = run_backfill_snapshots(app.s3_client(), &app.bucket, &app.state.snapshot_repo, false)
        .await
        .expect("backfill run 1");
    assert_eq!(first.inserted, 1, "must insert the one missing snapshot row: {first:?}");
    assert!(first.skipped_bad_key >= 1, "the non-snapshot key must be skipped: {first:?}");

    // The row now exists, attributed to the synthetic system user.
    let row = app
        .state
        .snapshot_repo
        .get(doc_id, version)
        .await
        .unwrap()
        .expect("SNAPSHOT# row written");
    assert_eq!(row.s3_key, key);
    assert_eq!(row.user_id, "system");
    assert_eq!(row.version, version);

    // Second run: the row already exists → nothing inserted.
    let second = run_backfill_snapshots(app.s3_client(), &app.bucket, &app.state.snapshot_repo, false)
        .await
        .expect("backfill run 2");
    assert_eq!(second.inserted, 0, "re-run must insert nothing: {second:?}");
    assert!(second.already_present >= 1, "re-run must see the row as already present: {second:?}");

    app.cleanup().await;
}
