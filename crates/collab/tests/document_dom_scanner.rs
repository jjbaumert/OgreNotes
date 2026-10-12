// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Actual diagnostic CLI against isolated DynamoDB Local/MinIO resources.
//! Feature-gated: CI's infrastructure job explicitly owns this target.
#![cfg(feature = "replay")]

use aws_sdk_dynamodb::{
    Client as Dynamo,
    types::{
        AttributeDefinition, AttributeValue as A, BillingMode, KeySchemaElement, KeyType,
        ScalarAttributeType,
    },
};
use aws_sdk_s3::Client as S3;
use ogrenotes_collab::document::OgreDoc;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    time::Duration,
};
use yrs::{
    Map, ReadTxn, StateVector, Text, Transact, WriteTxn,
    types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim},
};

type Item = HashMap<String, A>;

fn clients() -> (Dynamo, S3) {
    let creds = aws_sdk_dynamodb::config::Credentials::new(
        "minioadmin",
        "minioadmin",
        None,
        None,
        "isolated-test",
    );
    let ddb = aws_sdk_dynamodb::config::Builder::new()
        .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
        .endpoint_url("http://127.0.0.1:8000")
        .credentials_provider(creds.clone())
        .behavior_version_latest()
        .build();
    let s3 = aws_sdk_s3::config::Builder::new()
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .endpoint_url("http://127.0.0.1:9000")
        .force_path_style(true)
        .credentials_provider(creds)
        .behavior_version_latest()
        .build();
    (Dynamo::from_conf(ddb), S3::from_conf(s3))
}

fn available() -> bool {
    let present = [8000, 9000].into_iter().all(|p| {
        TcpStream::connect_timeout(
            &format!("127.0.0.1:{p}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
    });
    if !present && std::env::var_os("CI").is_some() {
        panic!("scanner contracts require DynamoDB Local :8000 and MinIO :9000");
    }
    if !present {
        eprintln!("SKIPPED: scanner fixtures require local DynamoDB and MinIO");
    }
    present
}

struct Fixture {
    ddb: Dynamo,
    s3: S3,
    table: String,
    bucket: String,
}
impl Fixture {
    async fn new() -> Self {
        let (ddb, s3) = clients();
        let name = format!(
            "dom-scan-test-{}-{}",
            std::process::id(),
            ogrenotes_common::time::now_usec()
        );
        ddb.create_table()
            .table_name(&name)
            .billing_mode(BillingMode::PayPerRequest)
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
            .send()
            .await
            .unwrap();
        s3.create_bucket().bucket(&name).send().await.unwrap();
        Self {
            ddb,
            s3,
            table: name.clone(),
            bucket: name,
        }
    }
    async fn item(&self, item: Item) {
        self.ddb
            .put_item()
            .table_name(&self.table)
            .set_item(Some(item))
            .send()
            .await
            .unwrap();
    }
    async fn blob(&self, key: &str, bytes: Vec<u8>) {
        self.s3
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(bytes.into())
            .send()
            .await
            .unwrap();
    }
    async fn doc(&self, id: &str, bytes: Vec<u8>) {
        let key = format!("docs/{id}/snapshots/1.bin");
        self.blob(&key, bytes).await;
        self.item(meta(id, &key, 1)).await;
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_scan_document_dom"));
        command
            .env("AWS_ACCESS_KEY_ID", "minioadmin")
            .env("AWS_SECRET_ACCESS_KEY", "minioadmin")
            .env("AWS_ENDPOINT_URL_DYNAMODB", "http://127.0.0.1:8000")
            .env("AWS_ENDPOINT_URL_S3", "http://127.0.0.1:9000")
            .env("AWS_EC2_METADATA_DISABLED", "true")
            .env_remove("AWS_PROFILE")
            .env_remove("AWS_SESSION_TOKEN")
            .env_remove("AWS_SECURITY_TOKEN")
            .env_remove("AWS_ENDPOINT_URL")
            .args([
                "--table",
                &self.table,
                "--bucket",
                &self.bucket,
                "--region",
                "us-east-1",
            ]);
        command
    }
    fn scan(&self, flags: &[&str]) -> (i32, Vec<Value>) {
        run(self.command().args(flags))
    }
    async fn identity(&self) -> (Vec<BTreeMap<String, String>>, BTreeMap<String, String>) {
        let items = self
            .ddb
            .scan()
            .table_name(&self.table)
            .send()
            .await
            .unwrap()
            .items
            .unwrap_or_default();
        let mut rows: Vec<BTreeMap<String, String>> = items
            .into_iter()
            .map(|i| i.into_iter().map(|(k, v)| (k, format!("{v:?}"))).collect())
            .collect();
        rows.sort();
        let keys = self
            .s3
            .list_objects_v2()
            .bucket(&self.bucket)
            .send()
            .await
            .unwrap()
            .contents
            .unwrap_or_default()
            .into_iter()
            .map(|o| (o.key.unwrap(), o.e_tag.unwrap()))
            .collect();
        (rows, keys)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Fresh clients/runtime in a separate thread allow cleanup even after an
        // assertion panic, without awaiting the test runtime being unwound.
        let table = self.table.clone();
        let bucket = self.bucket.clone();
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(async move {
                    let (ddb, s3) = clients();
                    if let Ok(list) = s3.list_objects_v2().bucket(&bucket).send().await {
                        for object in list.contents() {
                            if let Some(key) = object.key() {
                                let _ = s3.delete_object().bucket(&bucket).key(key).send().await;
                            }
                        }
                    }
                    let _ = s3.delete_bucket().bucket(&bucket).send().await;
                    let _ = ddb.delete_table().table_name(&table).send().await;
                })
        })
        .join()
        .unwrap();
    }
}

fn run(command: &mut Command) -> (i32, Vec<Value>) {
    let output = command.output().unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    let rows: Vec<Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).unwrap_or_else(|_| panic!("non-JSON scanner output: {s}")))
        .collect();
    let code = output.status.code().unwrap_or(-1);
    if let Some(directory) = std::env::var_os("DOM_SCANNER_EVIDENCE_DIR") {
        let path = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&path).unwrap();
        let name = format!(
            "scan-{}-{}.ndjson",
            std::process::id(),
            ogrenotes_common::time::now_usec()
        );
        std::fs::write(path.join(name), &text).unwrap();
    }
    assert!(
        rows.last().is_some_and(|r| r["kind"] == "summary"),
        "missing final summary; status={code}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(rows.last().unwrap()["exit_code"], code);
    assert!(
        !text.contains("PrivateBodyMarker"),
        "raw document text leaked"
    );
    (code, rows)
}
fn report<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["kind"] == "document" && r["doc_id"] == id)
        .unwrap()
}
fn summary(rows: &[Value]) -> &Value {
    rows.last().unwrap()
}
fn meta(id: &str, key: &str, version: u64) -> Item {
    [
        ("PK", A::S(format!("DOC#{id}"))),
        ("SK", A::S("METADATA".into())),
        ("doc_id", A::S(id.into())),
        ("doc_type", A::S("document".into())),
        ("is_deleted", A::Bool(false)),
        ("snapshot_version", A::N(version.to_string())),
        ("snapshot_s3_key", A::S(key.into())),
        ("updated_at", A::N("1".into())),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}
fn update(id: &str, clock: &str, bytes: Vec<u8>) -> Item {
    [
        ("PK", A::S(format!("DOC#{id}"))),
        ("SK", A::S(format!("UPDATE#{clock}"))),
        ("update_bytes", A::B(bytes.into())),
        ("user_id", A::S("retained-writer".into())),
        ("created_at", A::N("1700000000000000".into())),
        ("client_version", A::S("0.2.0".into())),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v))
    .collect()
}
fn raw_root() -> Vec<u8> {
    let doc = yrs::Doc::new();
    {
        let mut txn = doc.transact_mut();
        txn.get_or_insert_xml_fragment("content").insert(
            &mut txn,
            0,
            XmlTextPrelim::new("PrivateBodyMarker"),
        );
        txn.encode_state_as_update_v1(&StateVector::default())
    }
}
fn bad_list() -> Vec<u8> {
    let doc = yrs::Doc::new();
    {
        let mut txn = doc.transact_mut();
        let root = txn.get_or_insert_xml_fragment("content");
        let list = root.insert(&mut txn, 0, XmlElementPrelim::empty("bullet_list"));
        list.insert(&mut txn, 0, XmlTextPrelim::new("PrivateBodyMarker"));
        txn.encode_state_as_update_v1(&StateVector::default())
    }
}
fn edits() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let doc = OgreDoc::new();
    let snapshot = doc.to_state_bytes();
    let before = doc.inner().transact().state_vector();
    let first = {
        let mut txn = doc.inner().transact_mut();
        let root = txn.get_xml_fragment("content").unwrap();
        let list = root.insert(&mut txn, 1, XmlElementPrelim::empty("bullet_list"));
        list.insert(&mut txn, 0, XmlTextPrelim::new("PrivateBodyMarker"));
        txn.encode_state_as_update_v1(&before)
    };
    let before = doc.inner().transact().state_vector();
    let second = {
        let mut txn = doc.inner().transact_mut();
        txn.get_xml_fragment("content")
            .unwrap()
            .remove_range(&mut txn, 1, 1);
        txn.encode_state_as_update_v1(&before)
    };
    (snapshot, first, second)
}

#[tokio::test]
async fn inventory_paginates_filtered_empty_pages_and_never_writes_storage() {
    if !available() {
        return;
    }
    let f = Fixture::new().await;
    for i in 0..5 {
        f.item(
            [
                ("PK".into(), A::S(format!("UNRELATED#{i}"))),
                ("SK".into(), A::S("MEMBER#x".into())),
            ]
            .into(),
        )
        .await;
    }
    f.doc("healthy", OgreDoc::new().to_state_bytes()).await;
    f.doc("root-text", raw_root()).await;
    f.doc("orphan-list", bad_list()).await;
    let mut sheet = meta("sheet", "never-fetched", 1);
    sheet.insert("doc_type".into(), A::S("spreadsheet".into()));
    f.item(sheet).await;
    let before = f.identity().await;
    let (code, rows) = f.scan(&["--all", "--page-size", "1"]);
    assert_eq!(code, 1);
    assert_eq!(summary(&rows)["documents"], 4);
    assert_eq!(summary(&rows)["valid_current"], 1);
    assert_eq!(summary(&rows)["invalid_current"], 2);
    assert_eq!(report(&rows, "sheet")["status"], "skipped");
    assert_eq!(
        report(&rows, "root-text")["current"]["violations"][0]["rule"],
        "raw_text_outside_textblock"
    );
    assert_eq!(report(&rows, "orphan-list")["status"], "invalid");
    assert_eq!(before, f.identity().await, "scanner mutated DynamoDB or S3");
}

#[tokio::test]
async fn replay_reports_transition_and_s3_payload_precedence_after_repair() {
    if !available() {
        return;
    }
    let f = Fixture::new().await;
    let (snapshot, first, second) = edits();
    f.doc("repaired", snapshot).await;
    f.item(update("repaired", "00001", first)).await;
    let key = "docs/repaired/updates/00002.bin";
    f.blob(key, second).await;
    let mut row = update("repaired", "00002", vec![255]);
    row.insert("update_s3_key".into(), A::S(key.into()));
    f.item(row).await;
    let (code, rows) = f.scan(&["--doc-id", "repaired", "--page-size", "1"]);
    let doc = report(&rows, "repaired");
    assert_eq!(code, 1);
    assert_eq!(doc["status"], "valid");
    assert_eq!(doc["updates_applied"], 2);
    assert_eq!(doc["first_invalid_update"]["clock"], "00001");
    assert_eq!(doc["first_invalid_update"]["user_id"], "retained-writer");
    assert_eq!(doc["first_invalid_update"]["client_version"], "0.2.0");
    assert_eq!(summary(&rows)["intermediate_invalid"], 1);
    assert_eq!(doc["read_stable"], true);
}

#[tokio::test]
async fn unresolved_dependencies_never_look_valid_but_can_resolve_later() {
    if !available() {
        return;
    }
    let f = Fixture::new().await;
    let (snapshot, first, second) = edits();
    f.doc("delayed", snapshot).await;
    f.item(update("delayed", "00001", second)).await;
    let (code, rows) = f.scan(&["--doc-id", "delayed"]);
    assert_eq!(code, 2);
    assert_eq!(report(&rows, "delayed")["status"], "inconclusive");
    assert_eq!(
        report(&rows, "delayed")["current"]["unresolved_dependencies"],
        true
    );
    f.item(update("delayed", "00002", first)).await;
    let (code, rows) = f.scan(&["--doc-id", "delayed", "--page-size", "1"]);
    assert_eq!(code, 0);
    assert_eq!(report(&rows, "delayed")["status"], "valid");
    assert_eq!(report(&rows, "delayed")["updates_applied"], 2);
}

#[tokio::test]
async fn history_detects_invalid_old_snapshots_without_accusing_current_state() {
    if !available() {
        return;
    }
    let f = Fixture::new().await;
    f.blob(
        "docs/history/snapshots/1.bin",
        OgreDoc::new().to_state_bytes(),
    )
    .await;
    f.blob("docs/history/snapshots/2.bin", bad_list()).await;
    f.blob(
        "docs/history/snapshots/3.bin",
        OgreDoc::new().to_state_bytes(),
    )
    .await;
    f.item(meta("history", "docs/history/snapshots/3.bin", 3))
        .await;
    f.item(
        [
            ("PK".into(), A::S("DOC#history".into())),
            ("SK".into(), A::S("SNAPSHOT#00000000000000000002".into())),
            ("s3_key".into(), A::S("docs/history/snapshots/2.bin".into())),
            ("version".into(), A::N("2".into())),
            ("user_id".into(), A::S("system".into())),
        ]
        .into(),
    )
    .await;
    let (code, rows) = f.scan(&["--doc-id", "history", "--history", "--page-size", "1"]);
    assert_eq!(code, 1);
    assert_eq!(report(&rows, "history")["status"], "valid");
    assert_eq!(summary(&rows)["invalid_current"], 0);
    assert_eq!(summary(&rows)["invalid_history"], 1);
    assert!(rows.iter().any(|r| r["kind"] == "history"
        && r["snapshot"]["version"] == 1
        && r["snapshot"]["indexed"] == false));
    assert!(rows.iter().any(|r| r["kind"] == "history"
        && r["status"] == "invalid"
        && r["snapshot"]["user_id"] == "system"));
}

#[tokio::test]
async fn missing_corrupt_and_malformed_sources_are_errors_not_clean_documents() {
    if !available() {
        return;
    }
    let f = Fixture::new().await;
    f.item(meta("missing", "missing-object", 1)).await;
    f.doc("corrupt", vec![255]).await;
    let mut malformed = meta("malformed", "missing-object", 1);
    malformed.insert("doc_id".into(), A::S("another-id".into()));
    f.item(malformed).await;
    for id in ["missing", "corrupt", "malformed", "absent"] {
        let (code, rows) = f.scan(&["--doc-id", id]);
        assert_eq!(code, 2, "{id}");
        assert_eq!(report(&rows, id)["status"], "error");
        assert_eq!(summary(&rows)["valid_current"], 0);
    }
    let (code, rows) = f.scan(&["--all", "--page-size", "1"]);
    assert_eq!(code, 2);
    assert_eq!(summary(&rows)["documents"], 3);
    assert_eq!(summary(&rows)["errors"], 3);
}

#[tokio::test]
async fn resource_limits_and_deleted_scope_are_explicit() {
    if !available() {
        return;
    }
    let f = Fixture::new().await;
    let (snapshot, first, second) = edits();
    f.doc("limits", snapshot).await;
    f.item(update("limits", "00001", first)).await;
    f.item(update("limits", "00002", second)).await;
    for flags in [
        vec!["--doc-id", "limits", "--max-bytes", "1"],
        vec!["--doc-id", "limits", "--max-nodes", "1"],
        vec!["--doc-id", "limits", "--max-rows", "1"],
        vec!["--all", "--page-size", "1", "--max-pages", "1"],
    ] {
        let (code, rows) = f.scan(&flags);
        assert_eq!(code, 2, "{flags:?}");
        assert!(
            !summary(&rows)["inventory_complete"].as_bool().unwrap()
                || summary(&rows)["errors"].as_u64().unwrap()
                    + summary(&rows)["incomplete"].as_u64().unwrap()
                    > 0
        );
    }
    f.doc("deleted", raw_root()).await;
    let mut row = meta("deleted", "docs/deleted/snapshots/1.bin", 1);
    row.insert("is_deleted".into(), A::Bool(true));
    f.item(row).await;
    let (code, rows) = f.scan(&["--doc-id", "deleted"]);
    assert_eq!(code, 0);
    assert_eq!(report(&rows, "deleted")["status"], "skipped");
    let (code, rows) = f.scan(&["--doc-id", "deleted", "--include-deleted"]);
    assert_eq!(code, 1);
    assert_eq!(report(&rows, "deleted")["status"], "invalid");
    let (code, rows) = f.scan(&["--all", "--max-documents", "1"]);
    assert_eq!(code, 2);
    assert_eq!(summary(&rows)["inventory_complete"], false);
}

#[tokio::test]
async fn non_fragment_roots_and_deep_trees_cannot_silently_pass() {
    if !available() {
        return;
    }
    let f = Fixture::new().await;
    let text = yrs::Doc::new();
    {
        let mut txn = text.transact_mut();
        txn.get_or_insert_text("content")
            .insert(&mut txn, 0, "PrivateBodyMarker");
    }
    f.doc(
        "text-root",
        text.transact()
            .encode_state_as_update_v1(&StateVector::default()),
    )
    .await;
    let map = yrs::Doc::new();
    {
        let mut txn = map.transact_mut();
        txn.get_or_insert_map("content")
            .insert(&mut txn, "bad", "PrivateBodyMarker");
    }
    f.doc(
        "map-root",
        map.transact()
            .encode_state_as_update_v1(&StateVector::default()),
    )
    .await;
    for id in ["text-root", "map-root"] {
        let (code, rows) = f.scan(&["--doc-id", id]);
        assert_eq!(code, 1);
        assert_eq!(report(&rows, id)["status"], "invalid");
    }
    let deep = yrs::Doc::new();
    {
        let mut txn = deep.transact_mut();
        let root = txn.get_or_insert_xml_fragment("content");
        let mut el = root.insert(&mut txn, 0, XmlElementPrelim::empty("blockquote"));
        for _ in 0..12 {
            el = el.insert(&mut txn, 0, XmlElementPrelim::empty("blockquote"));
        }
    }
    f.doc(
        "deep",
        deep.transact()
            .encode_state_as_update_v1(&StateVector::default()),
    )
    .await;
    let (code, rows) = f.scan(&["--doc-id", "deep", "--max-depth", "3"]);
    assert_eq!(code, 2);
    assert_eq!(report(&rows, "deep")["status"], "inconclusive");
}

fn proxy(
    bytes: Vec<u8>,
    drift_etag: bool,
    on_head: impl FnOnce() + Send + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = std::thread::spawn(move || {
        let mut callback = Some(on_head);
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(20)))
                .unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            let head = request.starts_with(b"HEAD ");
            if head {
                callback.take().unwrap()();
            }
            let etag = if head && drift_etag {
                "changed"
            } else {
                "captured"
            };
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"{etag}\"\r\nConnection: close\r\n\r\n",bytes.len()).unwrap();
            if !head {
                socket.write_all(&bytes).unwrap();
            }
        }
    });
    (address, task)
}

#[tokio::test]
async fn changed_s3_and_dynamodb_state_is_inconclusive_in_the_real_cli() {
    if !available() {
        return;
    }
    let f = Fixture::new().await;
    f.doc("racy", OgreDoc::new().to_state_bytes()).await;
    let (address, task) = proxy(OgreDoc::new().to_state_bytes(), true, || {});
    let (code, rows) = run(f
        .command()
        .env("AWS_ENDPOINT_URL_S3", address)
        .args(["--doc-id", "racy"]));
    task.join().unwrap();
    assert_eq!(code, 2);
    assert_eq!(report(&rows, "racy")["status"], "inconclusive");
    assert_eq!(report(&rows, "racy")["read_stable"], false);
    let table = f.table.clone();
    let (_, first, _) = edits();
    let (address, task) = proxy(OgreDoc::new().to_state_bytes(), false, move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let (ddb, _) = clients();
                ddb.put_item()
                    .table_name(table)
                    .set_item(Some(update("racy", "00001", first)))
                    .send()
                    .await
                    .unwrap();
            });
    });
    let (code, rows) = run(f
        .command()
        .env("AWS_ENDPOINT_URL_S3", address)
        .args(["--doc-id", "racy"]));
    task.join().unwrap();
    assert_eq!(code, 2);
    assert_eq!(report(&rows, "racy")["status"], "inconclusive");
    assert_eq!(report(&rows, "racy")["read_stable"], false);
}

#[tokio::test]
async fn inspection_limits_cannot_be_hidden_by_a_smaller_final_tree() {
    if !available() {
        return;
    }
    let fixture = Fixture::new().await;
    let doc = OgreDoc::new();
    {
        let mut txn = doc.inner().transact_mut();
        txn.get_or_insert_xml_fragment("content").insert(
            &mut txn,
            1,
            XmlElementPrelim::empty("paragraph"),
        );
    }
    let snapshot = doc.to_state_bytes();
    let before = doc.inner().transact().state_vector();
    let repair = {
        let mut txn = doc.inner().transact_mut();
        txn.get_or_insert_xml_fragment("content")
            .remove_range(&mut txn, 1, 1);
        txn.encode_state_as_update_v1(&before)
    };
    fixture.doc("shrinking", snapshot).await;
    fixture.item(update("shrinking", "00001", repair)).await;
    let (code, rows) = fixture.scan(&["--doc-id", "shrinking", "--max-nodes", "1"]);
    assert_eq!(code, 2, "an uninspected snapshot cannot yield a clean scan");
    assert_eq!(summary(&rows)["incomplete"], 1);
}

#[tokio::test]
async fn inspection_limits_do_not_present_partial_replay_as_current() {
    if !available() {
        return;
    }
    let fixture = Fixture::new().await;
    let doc = OgreDoc::new();
    let snapshot = doc.to_state_bytes();
    let before = doc.inner().transact().state_vector();
    let first = {
        let mut txn = doc.inner().transact_mut();
        let root = txn.get_or_insert_xml_fragment("content");
        let element = root.insert(&mut txn, 1, XmlElementPrelim::empty("invalid-tag"));
        element.insert(&mut txn, 0, XmlTextPrelim::new("PrivateBodyMarker"));
        txn.encode_state_as_update_v1(&before)
    };
    let before = doc.inner().transact().state_vector();
    let repair = {
        let mut txn = doc.inner().transact_mut();
        txn.get_or_insert_xml_fragment("content")
            .remove_range(&mut txn, 1, 1);
        txn.encode_state_as_update_v1(&before)
    };
    fixture.doc("repaired-after-limit", snapshot).await;
    fixture
        .item(update("repaired-after-limit", "00001", first))
        .await;
    fixture
        .item(update("repaired-after-limit", "00002", repair))
        .await;
    let (code, rows) = fixture.scan(&["--doc-id", "repaired-after-limit", "--max-depth", "1"]);
    assert_eq!(code, 2);
    assert_eq!(
        report(&rows, "repaired-after-limit")["status"],
        "inconclusive"
    );
    assert_eq!(
        report(&rows, "repaired-after-limit")["current"]["status"],
        "inconclusive"
    );
    assert_eq!(summary(&rows)["invalid_current"], 0);
    assert_eq!(summary(&rows)["intermediate_invalid"], 1);
}

#[tokio::test]
async fn stalled_s3_bodies_are_bounded_operational_errors() {
    if !available() {
        return;
    }
    let fixture = Fixture::new().await;
    fixture
        .doc("stalled", OgreDoc::new().to_state_bytes())
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let task = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = socket.read(&mut buf).unwrap();
            assert_ne!(n, 0);
            request.extend_from_slice(&buf[..n]);
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nETag: \"stalled\"\r\nConnection: close\r\n\r\n").unwrap();
        std::thread::sleep(Duration::from_millis(1600));
    });
    let (code, rows) = run(fixture.command().env("AWS_ENDPOINT_URL_S3", address).args([
        "--doc-id",
        "stalled",
        "--timeout-seconds",
        "1",
    ]));
    task.join().unwrap();
    assert_eq!(code, 2);
    assert_eq!(report(&rows, "stalled")["status"], "error");
    assert_eq!(
        report(&rows, "stalled")["error"],
        "S3 object body deadline exceeded"
    );
}

#[tokio::test]
async fn unresolved_or_limited_current_trees_remain_inconclusive_with_violations() {
    if !available() {
        return;
    }
    let fixture = Fixture::new().await;
    let (base, _, pending_delete) = edits();
    let bytes = yrs::merge_updates_v1([base, bad_list(), pending_delete]).unwrap();
    fixture.doc("pending-invalid", bytes).await;
    let (code, rows) = fixture.scan(&["--doc-id", "pending-invalid"]);
    assert_eq!(code, 2);
    assert_eq!(
        report(&rows, "pending-invalid")["current"]["unresolved_dependencies"],
        true
    );
    assert_eq!(
        report(&rows, "pending-invalid")["current"]["status"],
        "inconclusive"
    );
    assert_eq!(summary(&rows)["invalid_current"], 0);
    let (code, rows) = fixture.scan(&["--doc-id", "pending-invalid", "--max-nodes", "1"]);
    assert_eq!(code, 2);
    assert_eq!(
        report(&rows, "pending-invalid")["current"]["status"],
        "inconclusive"
    );
}

#[tokio::test]
async fn resolved_snapshot_dependencies_do_not_hide_later_invalid_transitions() {
    if !available() {
        return;
    }
    let fixture = Fixture::new().await;
    let (base, first, repair) = edits();
    let pending_snapshot = yrs::merge_updates_v1([base.clone(), repair.clone()]).unwrap();
    let mut doc = OgreDoc::from_state_bytes(&base).unwrap();
    doc.apply_update(&first).unwrap();
    doc.apply_update(&repair).unwrap();
    let before = doc.inner().transact().state_vector();
    let malformed = {
        let mut txn = doc.inner().transact_mut();
        let root = txn.get_or_insert_xml_fragment("content");
        let list = root.insert(&mut txn, 1, XmlElementPrelim::empty("bullet_list"));
        list.insert(&mut txn, 0, XmlTextPrelim::new("PrivateBodyMarker"));
        txn.encode_state_as_update_v1(&before)
    };
    let before = doc.inner().transact().state_vector();
    let repair_again = {
        let mut txn = doc.inner().transact_mut();
        txn.get_or_insert_xml_fragment("content")
            .remove_range(&mut txn, 1, 1);
        txn.encode_state_as_update_v1(&before)
    };
    fixture.doc("initially-pending", pending_snapshot).await;
    fixture
        .item(update("initially-pending", "00001", first))
        .await;
    fixture
        .item(update("initially-pending", "00002", malformed))
        .await;
    fixture
        .item(update("initially-pending", "00003", repair_again))
        .await;
    let (code, rows) = fixture.scan(&["--doc-id", "initially-pending"]);
    assert_eq!(
        report(&rows, "initially-pending")["snapshot"]["unresolved_dependencies"],
        true
    );
    assert_eq!(
        report(&rows, "initially-pending")["current"]["status"],
        "valid"
    );
    assert_eq!(
        code, 1,
        "a resolved initial dependency must not hide a later invalid state"
    );
    assert_eq!(
        report(&rows, "initially-pending")["first_invalid_update"]["clock"],
        "00002"
    );
    assert_eq!(summary(&rows)["intermediate_invalid"], 1);
}

#[path = "document_dom_scanner/endpoint_profiles.rs"]
mod endpoint_profiles;

#[tokio::test]
async fn hostile_decoder_counts_and_worker_limits_cannot_abort_inventory() {
    if !available() {
        return;
    }
    let fixture = Fixture::new().await;
    fixture
        .doc("allocation-bomb", vec![0xff, 0xff, 0xff, 0xff, 0x0f])
        .await;
    fixture
        .doc("healthy-after-bomb", OgreDoc::new().to_state_bytes())
        .await;
    let (code, rows) = fixture.scan(&["--all", "--page-size", "1"]);
    assert_eq!(code, 2);
    assert_eq!(report(&rows, "allocation-bomb")["status"], "error");
    assert_eq!(report(&rows, "healthy-after-bomb")["status"], "valid");
    assert_eq!(summary(&rows)["inventory_complete"], true);
    let (code, rows) = fixture.scan(&["--doc-id", "healthy-after-bomb", "--max-memory-bytes", "1"]);
    assert_eq!(code, 2);
    assert_eq!(report(&rows, "healthy-after-bomb")["status"], "error");
}
