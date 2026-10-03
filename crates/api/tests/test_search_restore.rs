// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Startup recovery through the real S3 HTTP client and filesystem index.

mod common;

use ogrenotes_api::search_backup::{encode, initialize, restore_if_empty};
use ogrenotes_search::{IndexSnapshot, SearchDocument, SearchIndex, SearchQuery};
use ogrenotes_storage::s3::S3Client;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

// Reindex admission is process-global even when TestApp storage is isolated.
// Give each recovery scenario exclusive ownership of that production guard.
static REINDEX_SCENARIO: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    // Startup initialization performs this after restore_if_empty.
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
        let _ = initialize(&s3, &root.join("volume")).await;
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
    let volume = dst.path().join("volume");
    let abandoned = std::fs::read_dir(&volume)
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
    let dir = volume.join("index");
    assert!(!dir.exists(), "partial index must never be published");
    assert!(volume.join(".ogrenotes-reindex-pending").exists());

    // Repeat the production startup path with a valid S3 snapshot.
    let (_server, s3) = serve_snapshot(&snapshot).await;
    let startup = initialize(&s3, &volume).await.unwrap();
    assert!(startup.reindex.is_some());
    let index = startup.index;
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
        !abandoned.exists(),
        "restart must reclaim its tagged interrupted stage before copying again"
    );
}

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        doc_type: None,
        owner_id: None,
        folder_id: None,
        limit: 10,
        offset: 0,
    }
}

#[tokio::test]
async fn failed_full_startup_does_not_disable_recovery_on_restart() {
    common::require_infra!();
    let _reindex_scenario = REINDEX_SCENARIO.lock().await;
    use ogrenotes_storage::{dynamo::DynamoClient, repo::doc_repo::DocRepo};
    use std::sync::Arc;

    let mut app = common::TestApp::new().await;
    let (owner_id, token) = app.create_user("startup-rebuild@test.com").await;
    let older = app
        .create_doc(&token, "capybara persisted document", None)
        .await;
    let valid = populated_snapshot();
    let mut corrupt = valid.clone();
    corrupt.files.retain(|(name, _)| name == "meta.json");
    let (_server, s3) = serve_snapshot(&corrupt).await;
    let volume = tempfile::tempdir().unwrap();
    let startup = initialize(&s3, volume.path()).await.unwrap();
    app.state.search_index = Arc::new(startup.index);

    // Fail the real startup scan against a missing DynamoDB table. The
    // fallback index already has meta.json, exactly as in server startup.
    let mut unavailable = app.state.clone();
    unavailable.doc_repo = Arc::new(DocRepo::new(
        DynamoClient::new(
            app.dynamo_client().clone(),
            format!("{}-missing", app.table_name),
        ),
        app.state.doc_repo.s3().clone(),
    ));
    let error = startup.reindex.unwrap().run(&unavailable).await.unwrap_err();
    assert!(error.starts_with("list documents:"), "unexpected failure: {error}");
    drop(unavailable);

    // A document indexed after the failed startup must survive restarting
    // while an older S3 snapshot is available.
    let recent = app.create_doc(&token, "puffin recent document", None).await;
    app.state
        .search_index
        .index_document(&SearchDocument {
            doc_id: recent.clone(),
            title: "puffin recent document".into(),
            body: "".into(),
            owner_id,
            doc_type: "document".into(),
            folder_id: None,
            workspace_id: None,
            updated_at: 1,
            created_at: 1,
        })
        .unwrap();
    app.state.search_index = Arc::new(SearchIndex::open_in_memory().unwrap());

    let (_server, s3) = serve_snapshot(&valid).await;
    let restarted = initialize(&s3, volume.path()).await.unwrap();
    assert_eq!(
        restarted.index.search(&query("puffin")).unwrap()[0].doc_id,
        recent
    );
    assert!(
        restarted
            .index
            .search(&query("capybara"))
            .unwrap()
            .is_empty(),
        "the old snapshot must not replace the fallback index"
    );
    app.state.search_index = Arc::new(restarted.index);
    restarted
        .reindex
        .expect("unfinished rebuild must resume")
        .run(&app.state)
        .await
        .unwrap();
    assert_eq!(
        app.state.search_index.search(&query("capybara")).unwrap()[0].doc_id,
        older
    );
    assert_eq!(
        app.state.search_index.search(&query("puffin")).unwrap()[0].doc_id,
        recent
    );
    app.state.search_index = Arc::new(SearchIndex::open_in_memory().unwrap());

    let finished = initialize(&s3, volume.path()).await.unwrap();
    assert!(
        finished.reindex.is_none(),
        "completed rebuild must clear the marker"
    );
    assert_eq!(
        finished.index.search(&query("capybara")).unwrap()[0].doc_id,
        older
    );
    app.cleanup().await;
}

#[cfg(unix)]
#[tokio::test]
async fn startup_restores_when_only_the_configured_volume_is_writable() {
    use std::os::unix::fs::PermissionsExt;
    let (_server, s3) = serve_snapshot(&populated_snapshot()).await;
    let parent = tempfile::tempdir().unwrap();
    let volume = parent.path().join("search-volume");
    std::fs::create_dir(&volume).unwrap();
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let startup = initialize(&s3, &volume).await;
    std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    let startup = startup.expect("staging must live inside the configured volume");
    assert_eq!(
        startup.index.search(&query("capybara")).unwrap()[0].doc_id,
        "recovered"
    );
    assert!(volume.join("index/meta.json").exists());
    assert!(!volume.join("meta.json").exists());
}

#[tokio::test]
async fn startup_preserves_legacy_indexes_at_the_volume_root() {
    let volume = tempfile::tempdir().unwrap();
    let snapshot = populated_snapshot();
    ogrenotes_search::restore_snapshot(volume.path(), &snapshot).unwrap();
    let (_server, s3) = serve_snapshot(&snapshot).await;
    let startup = initialize(&s3, volume.path()).await.unwrap();
    assert!(startup.reindex.is_none());
    assert_eq!(
        startup.index.search(&query("capybara")).unwrap()[0].doc_id,
        "recovered"
    );
    assert!(
        !volume.path().join("index").exists(),
        "legacy index must not be replaced or moved"
    );
}

#[tokio::test]
async fn incomplete_document_rebuild_keeps_marker_until_retry_succeeds() {
    common::require_infra!();
    let _reindex_scenario = REINDEX_SCENARIO.lock().await;
    use std::sync::Arc;
    let mut app = common::TestApp::new().await;
    let (_, token) = app.create_user("partial-rebuild@test.com").await;
    let missing = app
        .create_doc(&token, "capybara temporarily unavailable", None)
        .await;
    let healthy = app
        .create_doc(&token, "puffin healthy document", None)
        .await;
    let meta = app.state.doc_repo.get(&missing).await.unwrap().unwrap();
    let key = meta.snapshot_s3_key.unwrap();
    let saved = app.state.doc_repo.s3().get_object(&key).await.unwrap();
    app.state.doc_repo.s3().delete_object(&key).await.unwrap();

    let mut corrupt = populated_snapshot();
    corrupt.files.retain(|(name, _)| name == "meta.json");
    let (_server, s3) = serve_snapshot(&corrupt).await;
    let volume = tempfile::tempdir().unwrap();
    let startup = initialize(&s3, volume.path()).await.unwrap();
    app.state.search_index = Arc::new(startup.index);
    let error = startup.reindex.unwrap().run(&app.state).await.unwrap_err();
    assert!(
        error.starts_with("1 document(s) failed during reindex;") && error.contains(&missing),
        "the missing document must prevent clearing the recovery marker: {error}"
    );
    assert_eq!(
        app.state.search_index.search(&query("puffin")).unwrap()[0].doc_id,
        healthy,
        "one failed document must not stop other documents making progress"
    );
    app.state.search_index = Arc::new(SearchIndex::open_in_memory().unwrap());

    app.state
        .doc_repo
        .s3()
        .put_object(&key, saved)
        .await
        .unwrap();
    let startup = initialize(&s3, volume.path()).await.unwrap();
    app.state.search_index = Arc::new(startup.index);
    startup
        .reindex
        .expect("partial rebuild must retry")
        .run(&app.state)
        .await
        .unwrap();
    assert_eq!(
        app.state.search_index.search(&query("capybara")).unwrap()[0].doc_id,
        missing
    );
    app.state.search_index = Arc::new(SearchIndex::open_in_memory().unwrap());
    assert!(
        initialize(&s3, volume.path())
            .await
            .unwrap()
            .reindex
            .is_none()
    );
    app.cleanup().await;
}
