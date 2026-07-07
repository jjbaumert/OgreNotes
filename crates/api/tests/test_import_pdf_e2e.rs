// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.6 piece E — PDF import/export end-to-end round-trip.
//!
//! Mirror of the DOCX e2e: the harness runs no worker process, so we
//! drive `worker_mode::execute_import_pdf` directly against the real
//! DynamoDB + S3, then check the created document is retrievable and
//! re-exportable. PDF is plain-text / lossy, so assertions are
//! content-based (the text survives) rather than structure-equality.
//!
//! Flow: build a known doc → `to_pdf` (the fixture) → stage to S3 →
//! worker import → the new doc's text exports intact → export to PDF
//! over HTTP → reimport and assert the text survived the full loop.

mod common;

use hyper::Method;
use ogrenotes_collab::schema::NodeType;
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::{Doc, Transact, WriteTxn};

const LINES: [&str; 3] = ["Annual report", "Profits were up.", "Costs held flat."];

/// Build the known fixture: three plain paragraphs (PDF carries no
/// structure, so heading/table fidelity isn't the point here).
fn fixture_doc() -> Doc {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");
        for (i, text) in LINES.iter().enumerate() {
            let p = frag.insert(
                &mut txn,
                i as u32,
                XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
            );
            p.insert(&mut txn, 0, XmlTextPrelim::new(*text));
        }
    }
    doc
}

#[tokio::test]
async fn pdf_import_export_round_trip_through_system() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("pdf-e2e@test.com").await;

    let home_folder = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .expect("user lookup")
        .expect("user exists")
        .home_folder_id;

    // The "known PDF": our exporter's output for the fixture doc.
    let fixture_pdf = ogrenotes_collab::export::to_pdf(&fixture_doc());
    assert!(!fixture_pdf.is_empty(), "fixture pdf should be non-empty");
    assert_eq!(&fixture_pdf[..4], b"%PDF", "fixture should be a real PDF");

    // Stage it where the worker expects to find it.
    let s3_key = format!("imports/{user_id}/e2e.pdf");
    app.state
        .doc_repo
        .s3()
        .put_object(&s3_key, fixture_pdf.clone())
        .await
        .expect("stage upload to S3");

    // Drive the worker's PDF import against the real repos.
    let doc_id = ogrenotes_api::worker_mode::execute_import_pdf(
        &app.state.doc_repo,
        &app.state.folder_repo,
        app.state.doc_repo.s3(),
        &s3_key,
        "Annual report",
        Some(&home_folder),
        &user_id,
    )
    .await
    .expect("worker pdf import should succeed");

    // The created document is retrievable and carries the text.
    let (status, html) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/html"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200, "imported doc should export");
    let html = String::from_utf8(html).expect("utf8 html");
    for line in LINES {
        assert!(html.contains(line), "text {line:?} lost on import: {html}");
    }

    // Round-trip out to PDF over HTTP, reimport, assert the text survived.
    let (status, exported_pdf) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/pdf"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200, "pdf export should succeed");
    assert_eq!(&exported_pdf[..4], b"%PDF", "export should be a real PDF");

    let reimported = ogrenotes_collab::import_pdf::from_pdf(&exported_pdf)
        .expect("exported pdf should reimport");
    let text = ogrenotes_collab::export::to_html(&reimported);
    for line in LINES {
        assert!(text.contains(line), "text {line:?} lost on round-trip: {text}");
    }

    app.cleanup().await;
}

#[tokio::test]
async fn worker_pdf_import_rejects_missing_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, _token) = app.create_user("pdf-e2e-nofolder@test.com").await;

    let fixture_pdf = ogrenotes_collab::export::to_pdf(&fixture_doc());
    let s3_key = format!("imports/{user_id}/nofolder.pdf");
    app.state
        .doc_repo
        .s3()
        .put_object(&s3_key, fixture_pdf)
        .await
        .expect("stage upload");

    // A job with no folder_id was never authorized — the worker must
    // reject it rather than invent a destination.
    let result = ogrenotes_api::worker_mode::execute_import_pdf(
        &app.state.doc_repo,
        &app.state.folder_repo,
        app.state.doc_repo.s3(),
        &s3_key,
        "No Folder",
        None,
        &user_id,
    )
    .await;
    assert!(result.is_err(), "missing folder_id should be rejected");
    assert!(
        result.unwrap_err().contains("never authorized"),
        "error should explain the authorization gap"
    );

    app.cleanup().await;
}
