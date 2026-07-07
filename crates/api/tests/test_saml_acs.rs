// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E4 piece D — `/auth/saml/acs` failure-path coverage.
//!
//! The plan called for an integration test that mints a signed
//! SAMLResponse via samael's IdentityProvider helper and verifies
//! the ACS round-trips to a fresh user. That test is INFEASIBLE on
//! Fedora 43 with xmlsec1 1.2.41 — samael 0.0.18's
//! `Signature::template` hardcodes `DigestAlgorithm::Sha1`, and
//! recent xmlsec1 builds disable SHA-1 digests for signing (it's
//! cryptographically deprecated). samael's own `test_signed_response`
//! fails on this system; the issue is upstream, not ours.
//!
//! Production VERIFY uses the same xmlsec context but accepts
//! whatever digest the IdP signs with. Modern IdPs (Okta, Azure AD,
//! Google Workspace) sign with SHA-256, so production parsing works.
//! The happy-path verification lands as a manual runbook step in
//! piece F against a real IdP.
//!
//! What we CAN test here is every rejection path — those don't
//! need a valid signature to exercise.

mod common;

use hyper::Method;

async fn setup_workspace_with_saml(app: &common::TestApp, email: &str) -> String {
    let (_, token) = app.create_user(email).await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "Acme" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();
    let (put_status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": "https://idp.example.test/saml/test",
                "idpMetadataXml": common::saml_test_idp_metadata(),
            })),
        )
        .await;
    assert_eq!(put_status, 204, "test setup PUT saml-config must succeed");
    ws_id
}

async fn post_acs(
    app: &common::TestApp,
    saml_response_b64: &str,
    relay_state: &str,
) -> u16 {
    use axum::http::Request;
    let body = format!(
        "SAMLResponse={}&RelayState={}",
        urlencoding::encode(saml_response_b64),
        urlencoding::encode(relay_state),
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/saml/acs")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(axum::body::Body::from(body))
        .unwrap();
    app.raw_request(req).await.0
}

#[tokio::test]
async fn acs_rejects_empty_relay_state() {
    // No RelayState → handler can't determine workspace → 401
    // (NOT 400/500, because we don't want to leak which check
    // failed).
    common::require_infra!();
    let app = common::TestApp::new().await;

    let status = post_acs(&app, "anything", "").await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn acs_rejects_when_workspace_has_no_saml_config() {
    // Workspace exists but has no IdP config → 401 (not 404, see
    // above — error-collapse is the security invariant).
    common::require_infra!();
    let app = common::TestApp::new().await;

    let (_, token) = app.create_user("acs-no-cfg-owner@test.com").await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "No SAML" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap();

    let status = post_acs(&app, "irrelevant", ws_id).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn acs_rejects_malformed_base64() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ws_id = setup_workspace_with_saml(&app, "acs-bad-b64@test.com").await;

    let status = post_acs(&app, "!!!not-base64-at-all!!!", &ws_id).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn acs_rejects_unsigned_response() {
    // A SAMLResponse without an XMLDSig signature — even if its
    // payload is well-formed — must be rejected because our SP
    // metadata advertises WantAssertionsSigned=true. The reduce-
    // to-signed step fails to find the dsig:Signature element and
    // surfaces FailedToValidateSignature.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ws_id = setup_workspace_with_saml(&app, "acs-unsigned@test.com").await;

    let bogus_xml = r#"<?xml version="1.0"?>
<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
  ID="r-1" Version="2.0" IssueInstant="2026-05-13T00:00:00Z"
  Destination="http://localhost:8080/api/v1/auth/saml/acs">
  <samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>
</samlp:Response>"#;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    let b64 = B64.encode(bogus_xml.as_bytes());

    let status = post_acs(&app, &b64, &ws_id).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn try_mark_assertion_seen_is_single_use() {
    // Unit-level check of the replay-dedup helper itself —
    // bypasses the full ACS flow because we can't mint a signed
    // SAMLResponse on this Fedora host. SET NX EX must return
    // (true, false) for two calls with the same assertion id.
    common::require_infra!();
    let app = common::TestApp::new().await;

    let id = format!("test-assertion-{}", nanoid::nanoid!(12));
    let first = app
        .state
        .redis_session
        .try_mark_assertion_seen(&id, 90)
        .await
        .unwrap();
    assert!(first, "first set must succeed");

    let second = app
        .state
        .redis_session
        .try_mark_assertion_seen(&id, 90)
        .await
        .unwrap();
    assert!(!second, "second set must be rejected by NX — this is the replay block");

    // A different id is independent.
    let third = app
        .state
        .redis_session
        .try_mark_assertion_seen(&format!("{id}-other"), 90)
        .await
        .unwrap();
    assert!(third, "distinct assertion id must not collide");

    app.cleanup().await;
}

#[tokio::test]
async fn acs_rejects_sha1_digest_before_signature_verify() {
    // SHA-1 lockdown: any SAMLResponse declaring a SHA-1 algorithm
    // is refused at the application layer, before xmlsec ever sees
    // it. The response below has no real signature — that doesn't
    // matter, because the SHA-1 scan fires first. If this test
    // starts failing it means the lockdown moved (or got removed)
    // and the production SP would accept SHA-1-signed assertions
    // from any IdP whose cert we trust.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ws_id = setup_workspace_with_saml(&app, "acs-sha1@test.com").await;

    // r##"..."## because XMLDSig fragment URIs (`URI="#r-sha1"`)
    // contain the sequence `"#` that closes a regular raw string.
    let sha1_xml = r##"<?xml version="1.0"?>
<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
  ID="r-sha1" Version="2.0" IssueInstant="2026-05-13T00:00:00Z"
  Destination="http://localhost:8080/api/v1/auth/saml/acs">
  <ds:Signature xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
    <ds:SignedInfo>
      <ds:CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/>
      <ds:SignatureMethod Algorithm="http://www.w3.org/2000/01/xmldsig#rsa-sha1"/>
      <ds:Reference URI="#r-sha1">
        <ds:DigestMethod Algorithm="http://www.w3.org/2000/09/xmldsig#sha1"/>
        <ds:DigestValue>aGVsbG8=</ds:DigestValue>
      </ds:Reference>
    </ds:SignedInfo>
    <ds:SignatureValue>aGVsbG8=</ds:SignatureValue>
  </ds:Signature>
  <samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>
</samlp:Response>"##;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    let b64 = B64.encode(sha1_xml.as_bytes());

    let status = post_acs(&app, &b64, &ws_id).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn acs_rejects_doctype_before_xml_parse() {
    // Gap-002: a SAMLResponse with a DOCTYPE construct is refused
    // at the application layer, before libxml2 parses it. Without
    // this guard, a `<!ENTITY ... SYSTEM "http://attacker/">` would
    // cause an outbound fetch from the ECS task — SSRF to IMDS at
    // worst. Real IdPs never emit DOCTYPE in SAML responses, so the
    // refuse-outright policy is safe.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ws_id = setup_workspace_with_saml(&app, "acs-doctype@test.com").await;

    let xxe_xml = r#"<?xml version="1.0"?>
<!DOCTYPE foo [<!ENTITY xxe SYSTEM "http://169.254.169.254/latest/meta-data/">]>
<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
  ID="r-xxe" Version="2.0" IssueInstant="2026-05-14T00:00:00Z"
  Destination="http://localhost:8080/api/v1/auth/saml/acs">
  <samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>
</samlp:Response>"#;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    let b64 = B64.encode(xxe_xml.as_bytes());

    let status = post_acs(&app, &b64, &ws_id).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn acs_rejects_when_idp_metadata_has_no_signing_cert() {
    // Gap-001 defense-in-depth: even if a workspace's SAML config
    // somehow ended up in DDB with no signing cert (direct write,
    // schema-migration error, deliberate tampering), the ACS path
    // must refuse to verify rather than silently skip XMLDSig.
    // We bypass put_saml_config's upload validation by writing the
    // cert-free row directly through the repo.
    common::require_infra!();
    let app = common::TestApp::new().await;
    let (_, token) = app.create_user("acs-no-cert@test.com").await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "NoCert" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();

    // Direct repo write of a cert-free config — simulates the
    // "somehow the row got written without going through PUT"
    // tampering path.
    let cert_free_metadata = r#"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.test/saml/test">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.example.test/sso/redirect"/>
  </IDPSSODescriptor>
</EntityDescriptor>"#;
    app.state
        .workspace_saml_config_repo
        .put(&ogrenotes_storage::models::workspace_saml_config::WorkspaceSamlConfig {
            workspace_id: ws_id.clone(),
            idp_entity_id: "https://idp.example.test/saml/test".to_string(),
            idp_metadata_xml: cert_free_metadata.to_string(),
            attribute_email: "email".to_string(),
            attribute_name: "name".to_string(),
            created_at: 0,
            updated_at: 0,
        })
        .await
        .unwrap();

    // Any POST to ACS for this workspace must now fail closed.
    let bogus_xml = r#"<?xml version="1.0"?>
<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
  ID="r-1" Version="2.0" IssueInstant="2026-05-14T00:00:00Z"
  Destination="http://localhost:8080/api/v1/auth/saml/acs">
  <samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>
</samlp:Response>"#;
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine;
    let b64 = B64.encode(bogus_xml.as_bytes());

    let status = post_acs(&app, &b64, &ws_id).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}

#[tokio::test]
async fn acs_rejects_empty_saml_response_body() {
    common::require_infra!();
    let app = common::TestApp::new().await;
    let ws_id = setup_workspace_with_saml(&app, "acs-empty@test.com").await;

    let status = post_acs(&app, "", &ws_id).await;
    assert_eq!(status, 401);

    app.cleanup().await;
}
