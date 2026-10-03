// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Startup recovery through the real S3 HTTP client and filesystem index.

use ogrenotes_api::search_backup::{encode, restore_if_empty};
use ogrenotes_search::{IndexSnapshot, SearchDocument, SearchIndex, SearchQuery};
use ogrenotes_storage::s3::S3Client;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

async fn serve_snapshot(snapshot: &IndexSnapshot) -> (MockServer, S3Client) {
    let server = MockServer::start().await;
    for verb in ["HEAD", "GET"] {
        Mock::given(method(verb))
            .and(path("/backup/search-index/snapshot.zip"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(encode(snapshot, 4242).unwrap()),
            )
            .mount(&server)
            .await;
    }
    let config = aws_sdk_s3::config::Builder::new()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new("us-east-1"))
        .credentials_provider(aws_sdk_s3::config::Credentials::new(
            "test", "test", None, None, "test",
        ))
        .endpoint_url(server.uri())
        .force_path_style(true)
        .build();
    (
        server,
        S3Client::new(aws_sdk_s3::Client::from_conf(config), "backup".into()),
    )
}

fn populated_snapshot() -> IndexSnapshot {
    let src = tempfile::tempdir().unwrap();
    let index = SearchIndex::open_or_create(src.path()).unwrap();
    index
        .index_document(&SearchDocument {
            doc_id: "recovered".into(),
            title: "Recovery".into(),
            body: "capybara".into(),
            owner_id: "u1".into(),
            doc_type: "document".into(),
            folder_id: None,
            workspace_id: None,
            updated_at: 1,
            created_at: 1,
        })
        .unwrap();
    index.snapshot().unwrap().unwrap()
}

#[tokio::test]
async fn incomplete_s3_snapshot_falls_back_to_an_openable_index() {
    let mut snapshot = populated_snapshot();
    snapshot.files.retain(|(name, _)| name == "meta.json");
    let (_server, s3) = serve_snapshot(&snapshot).await;
    let dst = tempfile::tempdir().unwrap();
    let dir = dst.path().join("index");
    assert_eq!(restore_if_empty(&s3, &dir).await, None);
    assert!(!dir.join("meta.json").exists());
    // main.rs performs this immediately after restore_if_empty.
    assert!(SearchIndex::open_or_create(&dir).is_ok());
}

#[tokio::test]
async fn failed_s3_restore_preserves_destination_and_a_retry_finds_documents() {
    let snapshot = populated_snapshot();
    let mut broken = snapshot.clone();
    broken.files.push(("x".repeat(4096), vec![1]));
    let (_server, s3) = serve_snapshot(&broken).await;
    let dst = tempfile::tempdir().unwrap();
    let dir = dst.path().join("index");
    std::fs::create_dir(&dir).unwrap();
    let notes = dir.join("operator-notes");
    std::fs::write(&notes, "keep me").unwrap();
    assert_eq!(restore_if_empty(&s3, &dir).await, None);
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "keep me");
    assert!(!dir.join("meta.json").exists());
    std::fs::remove_file(notes).unwrap();

    let (_server, s3) = serve_snapshot(&snapshot).await;
    assert_eq!(restore_if_empty(&s3, &dir).await, Some(4242));
    let index = SearchIndex::open_or_create(&dir).unwrap();
    let query = SearchQuery {
        text: "capybara".into(),
        doc_type: None,
        owner_id: None,
        folder_id: None,
        limit: 10,
        offset: 0,
    };
    assert_eq!(index.search(&query).unwrap()[0].doc_id, "recovered");
    assert_eq!(
        restore_if_empty(&s3, &dir).await,
        None,
        "existing index is preserved"
    );
    assert_eq!(index.search(&query).unwrap()[0].doc_id, "recovered");
}

/// Kill a real startup restore while it is writing a snapshot file. The OS
/// file-size limit terminates the child, bypassing Rust destructors and the
/// API's error handler, without adding fault hooks to production code.
#[cfg(unix)]
#[tokio::test]
async fn interrupted_startup_restore_recovers_on_restart() {
    const CHILD_DIR: &str = "OGRE_RESTORE_CRASH_TEST_DIR";
    if let Some(root) = std::env::var_os(CHILD_DIR) {
        let root = std::path::PathBuf::from(root);
        let (snapshot, _) = ogrenotes_api::search_backup::decode(
            &std::fs::read(root.join("snapshot.zip")).unwrap(),
        )
        .unwrap();
        let (_server, s3) = serve_snapshot(&snapshot).await;
        restore_if_empty(&s3, &root.join("index")).await;
        panic!("file-size limit did not interrupt the restore");
    }

    let snapshot = populated_snapshot();
    let mut interrupted = snapshot.clone();
    interrupted
        .files
        .push(("interrupted-segment".into(), vec![42; 1024 * 1024]));
    let dst = tempfile::tempdir().unwrap();
    std::fs::write(
        dst.path().join("snapshot.zip"),
        encode(&interrupted, 4242).unwrap(),
    )
    .unwrap();
    let output = std::process::Command::new("sh")
        .args([
            "-c",
            "ulimit -c 0; ulimit -f 64; exec \"$@\"",
            "restore-crash-test",
        ])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "interrupted_startup_restore_recovers_on_restart",
            "--nocapture",
        ])
        .env(CHILD_DIR, dst.path())
        .output()
        .unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert!(
        output.status.signal().is_some(),
        "child must be killed by the OS: {output:?}"
    );
    let abandoned = std::fs::read_dir(dst.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".ogrenotes-restore-")
        })
        .expect("interrupted restore must leave its private staging directory");
    assert!(abandoned.join("meta.json").exists());
    let partial = std::fs::metadata(abandoned.join("interrupted-segment"))
        .unwrap()
        .len();
    assert!(
        partial > 0 && partial < 1024 * 1024,
        "child must die during a file write"
    );
    let dir = dst.path().join("index");
    assert!(!dir.exists(), "partial index must never be published");

    // Repeat precisely the startup sequence with a valid S3 snapshot.
    let (_server, s3) = serve_snapshot(&snapshot).await;
    assert_eq!(restore_if_empty(&s3, &dir).await, Some(4242));
    let index = SearchIndex::open_or_create(&dir).unwrap();
    let hits = index
        .search(&SearchQuery {
            text: "capybara".into(),
            doc_type: None,
            owner_id: None,
            folder_id: None,
            limit: 10,
            offset: 0,
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].doc_id, "recovered");
    assert!(
        abandoned.exists(),
        "restart must not delete unowned siblings"
    );
}
