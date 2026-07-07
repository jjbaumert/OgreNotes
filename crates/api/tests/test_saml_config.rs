// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E4 piece B — per-workspace SAML IdP config admin
//! endpoints. The SP-metadata + login-redirect + ACS surfaces land
//! with pieces C/D; this file only exercises the config CRUD.

mod common;

use hyper::Method;

fn sample_metadata_xml() -> String {
    // Real-parseable cert + HTTP-Redirect endpoint — the same fixture
    // the ACS tests use. The cert is required since gap-001 made
    // put_saml_config validate signing-cert presence.
    common::saml_test_idp_metadata()
}

async fn create_workspace(app: &common::TestApp, email: &str) -> (String, String) {
    let (_, token) = app.create_user(email).await;
    let (status, json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "Acme" })),
        )
        .await;
    assert_eq!(status, 201);
    (token, json["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn put_saml_config_persists_and_get_returns_it() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (token, ws_id) = create_workspace(&app, "saml-owner@test.com").await;

    let body = serde_json::json!({
        "idpEntityId": "https://idp.example.com/metadata",
        "idpMetadataXml": sample_metadata_xml(),
    });
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(body),
        )
        .await;
    assert_eq!(status, 204);

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        json["idpEntityId"].as_str().unwrap(),
        "https://idp.example.com/metadata"
    );
    assert_eq!(json["attributeEmail"].as_str().unwrap(), "email");
    assert_eq!(json["attributeName"].as_str().unwrap(), "name");
    assert!(json["idpMetadataXml"]
        .as_str()
        .unwrap()
        .contains("EntityDescriptor"));

    app.cleanup().await;
}

#[tokio::test]
async fn put_saml_config_rejects_empty_xml() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (token, ws_id) = create_workspace(&app, "saml-empty@test.com").await;

    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/metadata",
                "idpMetadataXml": "",
            })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn put_saml_config_rejects_metadata_with_no_signing_cert() {
    // Gap-001: metadata that parses as SAML but has no signing
    // KeyDescriptor must be refused. Without this check, samael's
    // verify path silently skips signature validation for that
    // workspace and accepts any assertion.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (token, ws_id) = create_workspace(&app, "no-cert@test.com").await;

    let cert_free = r#"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.test/saml/test">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.example.test/sso/redirect"/>
  </IDPSSODescriptor>
</EntityDescriptor>"#;

    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.test/saml/test",
                "idpMetadataXml": cert_free,
            })),
        )
        .await;
    assert_eq!(status, 400, "cert-free metadata must be rejected");

    app.cleanup().await;
}

#[tokio::test]
async fn put_saml_config_rejects_metadata_with_unparseable_cert() {
    // Gap-001 cousin: metadata declares a signing KeyDescriptor but
    // the <X509Certificate> body is garbage. samael's verify path
    // would surface FailedToParseCert at every ACS request — better
    // to fail closed at upload time so the admin gets immediate
    // feedback.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (token, ws_id) = create_workspace(&app, "bad-cert@test.com").await;

    let bad_cert = r#"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.test/saml/test">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <KeyDescriptor use="signing">
      <KeyInfo xmlns="http://www.w3.org/2000/09/xmldsig#">
        <X509Data><X509Certificate>NOTAREALCERT</X509Certificate></X509Data>
      </KeyInfo>
    </KeyDescriptor>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.example.test/sso/redirect"/>
  </IDPSSODescriptor>
</EntityDescriptor>"#;

    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.test/saml/test",
                "idpMetadataXml": bad_cert,
            })),
        )
        .await;
    assert_eq!(status, 400, "unparseable cert must be rejected");

    app.cleanup().await;
}

#[tokio::test]
async fn put_saml_config_rejects_non_xml_blob() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (token, ws_id) = create_workspace(&app, "saml-nonxml@test.com").await;

    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/metadata",
                "idpMetadataXml": "not xml at all just text",
            })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn put_saml_config_rejects_oversized_xml() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (token, ws_id) = create_workspace(&app, "saml-oversized@test.com").await;

    let huge = "<x>".to_string() + &"a".repeat(70 * 1024) + "</x>";
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/metadata",
                "idpMetadataXml": huge,
            })),
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}

#[tokio::test]
async fn put_saml_config_requires_workspace_admin() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id) = create_workspace(&app, "saml-acl-owner@test.com").await;

    let (_, intruder_token) = app.create_user("saml-acl-intruder@test.com").await;
    let (status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&intruder_token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/metadata",
                "idpMetadataXml": sample_metadata_xml(),
            })),
        )
        .await;
    assert_eq!(status, 403);

    app.cleanup().await;
}

#[tokio::test]
async fn delete_saml_config_removes_row() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (token, ws_id) = create_workspace(&app, "saml-del@test.com").await;

    let _ = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/metadata",
                "idpMetadataXml": sample_metadata_xml(),
            })),
        )
        .await;
    let (status, _) = app
        .json_request(
            Method::DELETE,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 204);

    let (status, json) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert!(
        json.is_null(),
        "GET after DELETE must return null body, got: {json}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn put_saml_config_preserves_created_at_across_updates() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (token, ws_id) = create_workspace(&app, "saml-updated@test.com").await;

    let _ = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/v1",
                "idpMetadataXml": sample_metadata_xml(),
            })),
        )
        .await;
    let (_, first) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            None,
        )
        .await;
    let created_at_1 = first["createdAt"].as_i64().unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let _ = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/v2",
                "idpMetadataXml": sample_metadata_xml(),
            })),
        )
        .await;
    let (_, second) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            None,
        )
        .await;

    assert_eq!(
        second["createdAt"].as_i64().unwrap(),
        created_at_1,
        "createdAt must persist across PUTs"
    );
    assert!(
        second["updatedAt"].as_i64().unwrap() > created_at_1,
        "updatedAt must move forward on a second PUT"
    );
    assert_eq!(
        second["idpEntityId"].as_str().unwrap(),
        "https://idp.example.com/v2"
    );

    app.cleanup().await;
}
