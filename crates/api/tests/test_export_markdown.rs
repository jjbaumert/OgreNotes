// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

mod common;

use hyper::Method;
use yrs::types::xml::{XmlElementPrelim, XmlTextPrelim};
use yrs::{Any, Doc, Text, Transact, WriteTxn, XmlFragment};

const TABLE: &str = include_str!("fixtures/primitive-table.md");

fn table_document() -> Vec<u8> {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        let content = txn.get_or_insert_xml_fragment("content");
        let table = content.insert(&mut txn, 0, XmlElementPrelim::empty("table"));
        for (index, line) in TABLE.lines().enumerate() {
            if index == 1 {
                continue;
            }
            let row_index = table.len(&txn);
            let row = table.insert(&mut txn, row_index, XmlElementPrelim::empty("table_row"));
            for (column, value) in line.trim_matches('|').split('|').enumerate() {
                let cell = row.insert(
                    &mut txn,
                    column as u32,
                    XmlElementPrelim::empty(if index == 0 {
                        "table_header"
                    } else {
                        "table_cell"
                    }),
                );
                let para = cell.insert(&mut txn, 0, XmlElementPrelim::empty("paragraph"));
                let text = para.insert(&mut txn, 0, XmlTextPrelim::new(""));
                let value = value.trim();
                if let Some(code) = value.strip_prefix('`').and_then(|v| v.strip_suffix('`')) {
                    text.insert_with_attributes(
                        &mut txn,
                        0,
                        code,
                        [("code".into(), Any::Bool(true))].into(),
                    );
                } else {
                    text.insert(&mut txn, 0, value);
                }
            }
        }
    }
    use yrs::{ReadTxn, StateVector};
    doc.transact()
        .encode_state_as_update_v1(&StateVector::default())
}

#[tokio::test]
async fn markdown_export_preserves_table_rows_and_inline_code() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let token = app.create_user_token("markdown-table@test.com").await;
    let doc_id = app.create_doc(&token, "Primitive timings", None).await;
    let (status, _) = app
        .bytes_request(
            Method::PUT,
            &format!("/api/v1/documents/{doc_id}/content"),
            Some(&token),
            table_document(),
            "application/octet-stream",
        )
        .await;
    assert_eq!(status, 204);
    let (status, bytes) = app
        .bytes_request(
            Method::GET,
            &format!("/api/v1/documents/{doc_id}/export/markdown"),
            Some(&token),
            Vec::new(),
            "application/json",
        )
        .await;
    app.cleanup().await;
    assert_eq!(status, 200);
    let exported = String::from_utf8(bytes).unwrap();
    assert_eq!(exported.trim_end(), TABLE.trim_end());
}
