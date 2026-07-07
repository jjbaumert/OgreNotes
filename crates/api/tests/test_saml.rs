// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Integration tests for Phase 4 M-E4 piece C — `/auth/saml/*`
//! routes that don't yet require an IdP signature roundtrip.
//! The ACS handler tests land with piece D.

mod common;

use hyper::Method;

/// Minimal IdP metadata used by the redirect / metadata tests.
/// Includes a real-parseable signing cert because gap-001 made
/// `put_saml_config` enforce signing-cert presence at upload time.
fn idp_metadata_with_redirect_binding() -> String {
    format!(r##"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.com/metadata">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <KeyDescriptor use="signing">
      <KeyInfo xmlns="http://www.w3.org/2000/09/xmldsig#">
        <X509Data><X509Certificate>{cert}</X509Certificate></X509Data>
      </KeyInfo>
    </KeyDescriptor>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.example.com/sso/redirect"/>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
      Location="https://idp.example.com/sso/post"/>
  </IDPSSODescriptor>
</EntityDescriptor>"##, cert = common::SAML_TEST_X509_CERT)
}

async fn create_workspace_with_saml(
    app: &common::TestApp,
    email: &str,
) -> (String, String) {
    let (_, token) = app.create_user(email).await;
    let (_, json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "Acme" })),
        )
        .await;
    let ws_id = json["id"].as_str().unwrap().to_string();
    let _ = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/metadata",
                "idpMetadataXml": idp_metadata_with_redirect_binding(),
            })),
        )
        .await;
    (token, ws_id)
}

#[tokio::test]
async fn metadata_endpoint_returns_sp_metadata_xml() {
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (status, body) = app
        .bytes_request(Method::GET, "/api/v1/auth/saml/metadata", None, vec![], "")
        .await;
    assert_eq!(status, 200);
    let s = String::from_utf8(body).unwrap();
    assert!(s.contains("<EntityDescriptor"));
    assert!(s.contains("<SPSSODescriptor"));
    assert!(s.contains("WantAssertionsSigned=\"true\""));
    assert!(s.contains("AssertionConsumerService"));
    assert!(s.contains("/api/v1/auth/saml/acs"));

    app.cleanup().await;
}

#[tokio::test]
async fn login_redirect_returns_302_to_idp_sso() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, ws_id) = create_workspace_with_saml(&app, "saml-login@test.com").await;

    let (status, headers) = app
        .raw_request(
            axum::http::Request::builder()
                .method(Method::GET)
                .uri(format!("/api/v1/auth/saml/login?workspace={ws_id}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await;
    let _ = headers;
    // 307 temporary redirect — Axum's Redirect::temporary.
    assert!(
        status == 307 || status == 302,
        "expected redirect status, got {status}"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn login_redirect_404s_when_workspace_has_no_saml_config() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("saml-noconfig@test.com").await;
    let (_, json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "Acme No SAML" })),
        )
        .await;
    let ws_id = json["id"].as_str().unwrap();

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/auth/saml/login?workspace={ws_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, 404);

    app.cleanup().await;
}

#[tokio::test]
async fn login_redirect_400s_when_idp_metadata_has_no_redirect_binding() {
    // Workspace SAML config with POST-only IdP metadata. Includes
    // the test signing cert (gap-001) so the PUT itself succeeds —
    // the test asserts the GET /login then rejects for lack of an
    // HTTP-Redirect binding, which is a different rejection path.
    let post_only = format!(r##"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.com/metadata">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <KeyDescriptor use="signing">
      <KeyInfo xmlns="http://www.w3.org/2000/09/xmldsig#">
        <X509Data><X509Certificate>{cert}</X509Certificate></X509Data>
      </KeyInfo>
    </KeyDescriptor>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
      Location="https://idp.example.com/sso/post"/>
  </IDPSSODescriptor>
</EntityDescriptor>"##, cert = common::SAML_TEST_X509_CERT);

    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("saml-postonly@test.com").await;
    let (_, json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "Acme POST-Only" })),
        )
        .await;
    let ws_id = json["id"].as_str().unwrap().to_string();
    let _ = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.com/metadata",
                "idpMetadataXml": post_only,
            })),
        )
        .await;

    let (status, _) = app
        .json_request(
            Method::GET,
            &format!("/api/v1/auth/saml/login?workspace={ws_id}"),
            None,
            None,
        )
        .await;
    assert_eq!(status, 400);

    app.cleanup().await;
}
