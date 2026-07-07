// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 6 M-6.5 piece E — DOCX import/export end-to-end round-trip.
//!
//! Closes the gap the route-only tests leave: the worker's import
//! *execution* (S3 fetch → parse → persist a real document) and the
//! full round-trip through the system. The test harness runs no worker
//! process, so we drive the worker's public import entry point
//! (`worker_mode::execute_import_docx`) directly against the harness's
//! real DynamoDB + S3, then assert the created document is retrievable
//! and re-exportable.
//!
//! Flow: build a known document → `to_docx` (the fixture) → stage to S3
//! → worker import → the new doc exports back to HTML with its content
//! intact → export it to DOCX over HTTP → reimport and assert the
//! structure survived the full loop.

mod common;

use hyper::Method;
use ogrenotes_collab::schema::NodeType;
use yrs::types::xml::{XmlElementPrelim, XmlFragment, XmlTextPrelim};
use yrs::{Doc, Transact, WriteTxn, Xml};

/// Build the known fixture document: an H1, a body paragraph, and a
/// 2x2 table. This is the "known DOCX" once run through `to_docx`.
fn fixture_doc() -> Doc {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let frag = txn.get_or_insert_xml_fragment("content");

        let h = frag.insert(&mut txn, 0, XmlElementPrelim::empty(NodeType::Heading.tag_name()));
        h.insert_attribute(&mut txn, "level", "1");
        h.insert(&mut txn, 0, XmlTextPrelim::new("Quarterly Report"));

        let p = frag.insert(&mut txn, 1, XmlElementPrelim::empty(NodeType::Paragraph.tag_name()));
        p.insert(&mut txn, 0, XmlTextPrelim::new("Summary text"));

        let table = frag.insert(&mut txn, 2, XmlElementPrelim::empty(NodeType::Table.tag_name()));
        for (ri, cells) in [["A1", "B1"], ["A2", "B2"]].iter().enumerate() {
            let row = table.insert(
                &mut txn,
                ri as u32,
                XmlElementPrelim::empty(NodeType::TableRow.tag_name()),
            );
            for (ci, val) in cells.iter().enumerate() {
                let cell = row.insert(
                    &mut txn,
                    ci as u32,
                    XmlElementPrelim::empty(NodeType::TableCell.tag_name()),
                );
                let cp = cell.insert(
                    &mut txn,
                    0,
                    XmlElementPrelim::empty(NodeType::Paragraph.tag_name()),
                );
                cp.insert(&mut txn, 0, XmlTextPrelim::new(*val));
            }
        }
    }
    doc
}

#[tokio::test]
async fn docx_import_export_round_trip_through_system() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, token) = app.create_user("docx-e2e@test.com").await;

    let home_folder = app
        .state
        .user_repo
        .get_by_id(&user_id)
        .await
        .expect("user lookup")
        .expect("user exists")
        .home_folder_id;

    // The "known DOCX": our exporter's output for the fixture doc. This
    // is a complete, valid OOXML package (docx-rs builds the full
    // container), not a bare document.xml.
    let fixture_docx = ogrenotes_collab::export::to_docx(&fixture_doc());
    assert!(!fixture_docx.is_empty(), "fixture docx should be non-empty");
    assert_eq!(&fixture_docx[..4], b"PK\x03\x04", "fixture should be a real OOXML package");

    // Stage it where the worker expects to find it, exactly as the
    // import-job route would.
    let s3_key = format!("imports/{user_id}/e2e.docx");
    app.state
        .doc_repo
        .s3()
        .put_object(&s3_key, fixture_docx.clone())
        .await
        .expect("stage upload to S3");

    // Drive the worker's import execution against the real repos.
    let doc_id = ogrenotes_api::worker_mode::execute_import_docx(
        &app.state.doc_repo,
        &app.state.folder_repo,
        app.state.doc_repo.s3(),
        &s3_key,
        "Quarterly Report",
        Some(&home_folder),
        &user_id,
    )
    .await
    .expect("worker import should succeed");

    // The created document is retrievable and carries the imported
    // content (export to HTML for a readable assertion).
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
    assert!(html.contains("<h1>Quarterly Report</h1>"), "heading lost: {html}");
    assert!(html.contains("Summary text"), "paragraph lost: {html}");
    for cell in ["A1", "B1", "A2", "B2"] {
        assert!(html.contains(cell), "table cell {cell} lost: {html}");
    }

    // Round-trip the other way: export the document back to DOCX over
    // HTTP, then reimport and assert the structure survived the loop.
    let (status, exported_docx) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/docx"),
            Some(&token),
            Vec::new(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 200, "docx export should succeed");
    assert_eq!(&exported_docx[..4], b"PK\x03\x04", "export should be a real OOXML package");

    let reimported = ogrenotes_collab::import_docx::from_docx(&exported_docx)
        .expect("exported docx should reimport");
    let reimported_html = ogrenotes_collab::export::to_html(&reimported);
    assert!(reimported_html.contains("<h1>Quarterly Report</h1>"), "heading lost on round-trip: {reimported_html}");
    assert!(reimported_html.contains("Summary text"), "paragraph lost on round-trip");
    for cell in ["A1", "B1", "A2", "B2"] {
        assert!(reimported_html.contains(cell), "table cell {cell} lost on round-trip");
    }

    app.cleanup().await;
}

#[tokio::test]
async fn worker_import_rejects_missing_folder() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (user_id, _token) = app.create_user("docx-e2e-nofolder@test.com").await;

    let fixture_docx = ogrenotes_collab::export::to_docx(&fixture_doc());
    let s3_key = format!("imports/{user_id}/nofolder.docx");
    app.state
        .doc_repo
        .s3()
        .put_object(&s3_key, fixture_docx)
        .await
        .expect("stage upload");

    // A job with no folder_id was never authorized — the worker must
    // reject it rather than invent a destination.
    let result = ogrenotes_api::worker_mode::execute_import_docx(
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
