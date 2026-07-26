// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Infra-gated integration test for `ImportRepo` against DynamoDB Local.
//!
//! Mirrors the `require_infra!` / `INFRA_AVAILABLE` idiom from
//! `crates/api/tests/common/mod.rs`, scoped to just DynamoDB — `ImportRepo`
//! doesn't touch S3 or Redis. Each test gets its own throwaway table so
//! tests can run in parallel without clobbering each other.

use std::sync::LazyLock;

use aws_sdk_dynamodb::types::{AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType};
use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::models::import::{ImportRecord, ImportStatus};
use ogrenotes_storage::models::import_inventory::{ThreadRow, ThreadState};
use ogrenotes_storage::repo::import_repo::ImportRepo;

static INFRA_AVAILABLE: LazyLock<bool> = LazyLock::new(|| {
    use std::net::TcpStream;
    use std::time::Duration;
    let timeout = Duration::from_millis(200);
    let available = TcpStream::connect_timeout(&"127.0.0.1:8000".parse().unwrap(), timeout).is_ok();
    if !available {
        eprintln!(
            "Integration test infra unavailable (DynamoDB Local on :8000). \
             Run `docker compose up -d` to start services."
        );
    }
    available
});

/// Call at the top of every test in this file. Locally, skips (with a
/// stderr note) when DynamoDB Local isn't running. In CI (`CI` env var
/// set), panics instead — a missing service in CI should fail loud, not
/// silently green. Opt out with `SKIP_INFRA_TESTS=1`.
macro_rules! require_infra {
    () => {
        if !*INFRA_AVAILABLE {
            if std::env::var("SKIP_INFRA_TESTS").is_ok() {
                eprintln!("SKIPPED: SKIP_INFRA_TESTS=1 and infra unavailable");
                return;
            }
            if std::env::var("CI").is_ok() {
                panic!(
                    "Integration infra unavailable (DynamoDB Local). Bring it up \
                     with `docker compose up -d`, or set SKIP_INFRA_TESTS=1 to \
                     explicitly skip."
                );
            }
            eprintln!("SKIPPED: DynamoDB Local unavailable (see eprintln above).");
            return;
        }
    };
}

async fn test_repo() -> (ImportRepo, String) {
    let table_name = format!("test-import-{}", new_id());

    let dynamo_config = aws_sdk_dynamodb::config::Builder::new()
        .endpoint_url("http://127.0.0.1:8000")
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "fakekey", "fakesecret", None, None, "test",
        ))
        .behavior_version_latest()
        .build();
    let client = aws_sdk_dynamodb::Client::from_conf(dynamo_config);

    client
        .create_table()
        .table_name(&table_name)
        .billing_mode(BillingMode::PayPerRequest)
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("PK")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("SK")
                .key_type(KeyType::Range)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("PK")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("SK")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .expect("create_table");

    let repo = ImportRepo::new(DynamoClient::new(client, table_name.clone()));
    (repo, table_name)
}

fn sample_record() -> ImportRecord {
    let now = now_usec();
    ImportRecord {
        import_id: new_id(),
        owner_id: new_id(),
        status: ImportStatus::Scoping,
        phase: 0,
        quip_user_id: Some("quip-user-1".to_string()),
        target_folder_id: Some("folder-1".to_string()),
        selected_roots: vec!["root-a".to_string(), "root-b".to_string()],
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn create_then_get_round_trips_every_field() {
    require_infra!();
    let (repo, _table) = test_repo().await;
    let record = sample_record();

    repo.create(&record).await.expect("create");

    let fetched = repo
        .get(&record.import_id)
        .await
        .expect("get")
        .expect("record must exist after create");

    assert_eq!(fetched, record);
}

#[tokio::test]
async fn get_missing_import_returns_none() {
    require_infra!();
    let (repo, _table) = test_repo().await;

    let fetched = repo.get(&new_id()).await.expect("get");
    assert!(fetched.is_none());
}

#[tokio::test]
async fn set_status_changes_status_and_bumps_updated_at() {
    require_infra!();
    let (repo, _table) = test_repo().await;
    let mut record = sample_record();
    // Give updated_at a value clearly in the past so a bump is unambiguous
    // even at whatever timer resolution the test host has.
    record.created_at -= 1_000_000;
    record.updated_at -= 1_000_000;
    repo.create(&record).await.expect("create");

    repo.set_status(&record.import_id, ImportStatus::Running)
        .await
        .expect("set_status");

    let fetched = repo
        .get(&record.import_id)
        .await
        .expect("get")
        .expect("record must still exist");

    assert_eq!(fetched.status, ImportStatus::Running);
    assert!(
        fetched.updated_at > record.updated_at,
        "updated_at must be bumped: before={}, after={}",
        record.updated_at,
        fetched.updated_at
    );
    // set_status must not disturb unrelated fields.
    assert_eq!(fetched.owner_id, record.owner_id);
    assert_eq!(fetched.selected_roots, record.selected_roots);
    assert_eq!(fetched.created_at, record.created_at);
}

#[tokio::test]
async fn create_twice_conflicts() {
    require_infra!();
    let (repo, _table) = test_repo().await;
    let record = sample_record();

    repo.create(&record).await.expect("first create must succeed");

    let mut dup = record.clone();
    dup.owner_id = new_id(); // even a different payload must still conflict
    let err = repo
        .create(&dup)
        .await
        .expect_err("second create for the same import_id must fail");

    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains("conditional"),
        "expected a conditional-check failure, got: {msg}"
    );
}

#[tokio::test]
async fn import_record_never_carries_a_token_field() {
    // #NNN Phase-0 contract: the token lives only in the TokenStore
    // (Task 4), never in the durable IMPORT#<id>/META manifest. This is
    // compile-enforced by the struct shape (no `token` field on
    // `ImportRecord`), but pinning it here too documents the contract at
    // the integration boundary and would catch a hand-rolled item gaining
    // a stray `token`/`secret` attribute even if the struct itself grew
    // an unrelated field with a different name.
    require_infra!();
    let (repo, table) = test_repo().await;
    let record = sample_record();
    repo.create(&record).await.expect("create");

    let dynamo_config = aws_sdk_dynamodb::config::Builder::new()
        .endpoint_url("http://127.0.0.1:8000")
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "fakekey", "fakesecret", None, None, "test",
        ))
        .behavior_version_latest()
        .build();
    let client = aws_sdk_dynamodb::Client::from_conf(dynamo_config);
    let raw = client
        .get_item()
        .table_name(&table)
        .key("PK", aws_sdk_dynamodb::types::AttributeValue::S(record.pk()))
        .key(
            "SK",
            aws_sdk_dynamodb::types::AttributeValue::S(ImportRecord::sk().to_string()),
        )
        .send()
        .await
        .expect("raw get_item")
        .item
        .expect("item must exist");

    assert!(!raw.contains_key("token"));
    assert!(!raw.contains_key("secret"));
}

fn pending_thread(quip_thread_id: &str, state: ThreadState) -> ThreadRow {
    ThreadRow {
        quip_thread_id: quip_thread_id.to_string(),
        owner_id: "u1".to_string(),
        title: "Doc".to_string(),
        thread_type: "document".to_string(),
        updated_usec: 42,
        member_folders: vec!["qf1".to_string()],
        first_folder: "qf1".to_string(),
        state,
        ogre_doc_id: None,
    }
}

/// Resume-safety invariant: a re-run of inventory BFS re-discovers the
/// same thread and tries to (re)insert it as `Pending`. `put_thread` must
/// be insert-if-absent — it must never clobber a row that Phase 2 has
/// already advanced past `Pending`.
#[tokio::test]
async fn put_thread_is_insert_if_absent() {
    require_infra!();
    let (repo, _table) = test_repo().await;
    let record = sample_record();
    repo.create(&record).await.expect("create");

    let t0 = pending_thread("qt1", ThreadState::Pending);
    let mut advanced = t0.clone();
    advanced.state = ThreadState::ContentDone;

    // Seed the advanced (Phase-2-progressed) row.
    repo.put_thread(&record.import_id, &advanced)
        .await
        .expect("seed advanced");
    // A second inventory run tries to (re)insert the Pending version.
    repo.put_thread(&record.import_id, &t0)
        .await
        .expect("re-run insert-if-absent");

    let rows = repo.list_threads(&record.import_id).await.expect("list_threads");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].state,
        ThreadState::ContentDone,
        "re-run must not downgrade"
    );
}

/// Load-bearing dual-worker mutual-exclusion contract that Phase 3
/// depends on: only one runner can hold the inventory lease at a time,
/// a heartbeat keeps a live claim from being stolen, a stale claim (no
/// heartbeat within `stale_ms`) can be taken over, and `clear_runner_claim`
/// releases the lease immediately for the next claimant.
#[tokio::test]
async fn claim_runner_lease_contract() {
    require_infra!();
    let (repo, _table) = test_repo().await;
    let record = sample_record();
    repo.create(&record).await.expect("create");

    const NOW_MS: i64 = 1_000_000;
    const STALE_MS: i64 = 30_000;

    // inst-A acquires the lease; inst-B is refused while it's live.
    assert!(
        repo.claim_runner(&record.import_id, "inst-A", NOW_MS, STALE_MS)
            .await
            .expect("claim by inst-A"),
        "first claim must succeed"
    );
    assert!(
        !repo
            .claim_runner(&record.import_id, "inst-B", NOW_MS, STALE_MS)
            .await
            .expect("claim attempt by inst-B"),
        "a live claim must not be stealable"
    );

    // inst-A's heartbeat refreshes the lease.
    repo.heartbeat_runner(&record.import_id, "inst-A", NOW_MS)
        .await
        .expect("heartbeat by inst-A");

    // Once the heartbeat is older than stale_ms, inst-B can take over.
    assert!(
        repo.claim_runner(&record.import_id, "inst-B", NOW_MS + STALE_MS + 1, STALE_MS)
            .await
            .expect("claim attempt by inst-B after staleness window"),
        "a stale claim must be takeable over"
    );

    // Clearing the claim makes it immediately re-acquirable.
    repo.clear_runner_claim(&record.import_id)
        .await
        .expect("clear_runner_claim");
    assert!(
        repo.claim_runner(&record.import_id, "inst-C", NOW_MS + STALE_MS + 2, STALE_MS)
            .await
            .expect("claim attempt by inst-C after clear"),
        "a cleared claim must be immediately re-acquirable"
    );
}

/// `set_inventory_total` and `set_phase` must durably persist on the
/// `META` row. `phase` is modeled on `ImportRecord`, so it's checked via
/// `ImportRepo::get`; `inventory_total` is a Phase-1-only operational
/// attribute not modeled on `ImportRecord` (deliberately — see task-1
/// report), so it's checked with a raw item read, mirroring the same
/// raw-client pattern `import_record_never_carries_a_token_field` already
/// uses for attributes outside the domain struct.
#[tokio::test]
async fn set_inventory_total_and_set_phase_persist() {
    require_infra!();
    let (repo, table) = test_repo().await;
    let record = sample_record();
    repo.create(&record).await.expect("create");

    repo.set_inventory_total(&record.import_id, 42)
        .await
        .expect("set_inventory_total");
    repo.set_phase(&record.import_id, 2)
        .await
        .expect("set_phase");

    let fetched = repo
        .get(&record.import_id)
        .await
        .expect("get")
        .expect("record must still exist");
    assert_eq!(fetched.phase, 2);

    let dynamo_config = aws_sdk_dynamodb::config::Builder::new()
        .endpoint_url("http://127.0.0.1:8000")
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
            "fakekey", "fakesecret", None, None, "test",
        ))
        .behavior_version_latest()
        .build();
    let client = aws_sdk_dynamodb::Client::from_conf(dynamo_config);
    let raw = client
        .get_item()
        .table_name(&table)
        .key("PK", aws_sdk_dynamodb::types::AttributeValue::S(record.pk()))
        .key(
            "SK",
            aws_sdk_dynamodb::types::AttributeValue::S(ImportRecord::sk().to_string()),
        )
        .send()
        .await
        .expect("raw get_item")
        .item
        .expect("item must exist");

    assert_eq!(
        raw.get("inventory_total").and_then(|v| v.as_n().ok()),
        Some(&"42".to_string())
    );
}
