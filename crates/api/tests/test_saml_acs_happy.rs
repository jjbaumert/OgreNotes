// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 4 M-E4 — `/auth/saml/acs` HAPPY-PATH coverage.
//!
//! Companion to `test_saml_acs.rs`, whose header explains why it only
//! covers rejection paths: samael 0.0.18's `Signature::template`
//! builder hardcodes `DigestAlgorithm::Sha1`, and modern xmlsec1
//! refuses to *sign* with SHA-1, so the obvious "mint a signed
//! response via samael's IdP helper" approach can't run on this host.
//! The happy path was therefore punted to a manual runbook step.
//!
//! This test closes that gap WITHOUT a real IdP. We mint a
//! SHA-256-signed assertion by shelling out to the `xmlsec1` CLI (the
//! same xmlsec engine the server links for verification), embed our
//! own signing cert in the workspace's IdP metadata, store a pending
//! AuthnRequest in Redis, and POST the response to the real ACS route.
//! The signing key is a throwaway RSA-2048 keypair (see constants);
//! test-only, never reused.
//!
//! ── Host caveat (why these can skip) ────────────────────────────
//! samael 0.0.18's xmlsec FFI bindings are ABI-incompatible with some
//! xmlsec1 builds (notably 1.2.41 on Fedora 43): `signValueNode` is
//! read at the wrong struct offset, so xmlsec aborts BOTH signing and
//! verification with `signValueNode == NULL` at xmldsig.c:442 before
//! any crypto runs. This is the same upstream breakage test_saml_acs.rs
//! documents. Where it bites, samael's *verify* — the production code
//! under test — can't run in-process at all, so this test can't go
//! green; it `skip`s via a capability probe. On the Debian-bookworm
//! production/CI runtime, in-process xmlsec works and the test runs for
//! real. (The `xmlsec1` CLI verifies our signature fine even on the
//! affected host, which is how we know the signing itself is correct.)

mod common;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hyper::Method;

// ─── Throwaway signing material ──────────────────────────────────
//
// Generated once with:
//   openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 36500 \
//     -subj /CN=ogrenotes-saml-happy-test -keyout k.pem -out c.pem
//   openssl rsa -traditional -in k.pem -outform DER | base64 -w0
//   openssl x509 -in c.pem -outform DER | base64 -w0
// PKCS#1 (RSAPrivateKey) DER private key, base64. samael's sign path
// loads RSA keys via openssl's `private_key_to_der` (PKCS#1), so the
// key MUST be traditional PKCS#1 — a PKCS#8 wrapper fails to load and
// xmlsec then reports a misleading `signValueNode == NULL`. NOT a
// production key — test only, never reused.
const SIGNING_KEY_PKCS1_DER_B64: &str = "MIIEogIBAAKCAQEA3nk7E05C7G33ge2NA0PHFrV57I+RFNGGlYiVsUPKfVpP7ie1HC3YygBczWUa1bl5bJbmlrY0cTH9Cqx4/Uag6PjUfEOR39gh57kS711zi6CDkCrPcf+ibLQnweJnrVVrE4c4y0spJhkVr694E4uCuRtcbbzA1iu7xIziSyqZiPSgwaqLxNtqXus/FsCI+ciKeLSv7vCaoVnp1LGuxmI5yk6qwa+xhavOS2bCCUvYycEyp1Cv/E+E21sgIxl/Cb48v6W7duJV60WlCMBYvvqB1Vfs0+MLrBTqjQd7p0+scczXVEg5vHrHOnZtwZPAsg3vq8lSAWiO9u+1TsX6DW/8uQIDAQABAoIBABrzZj3wOui/6J43jB/jX9SnLupCuSFjwZFPXsz+6KKXZHv2GPldL3hHI3bpYt8VzTkjrbL/xyoYU25t1uld20Pl0v3rxJnwqajT8ZCISmVVkGKQYRmPDZrsFy3kcslbgfF3beCozgcUvl0OXXZGrhMlFqUfmt/HJAPLSmWvNzLRL92lG6bVECcJxg42Lwm2vAyGnBmqgWzmYeG6GgtXk8B1tXQAaBgTqJ0f8Egnu4zLpNjbQ64l5HYL5Pi1w+wiatPEXAYd6SEK8KyouMv4p2SwwXtcwvU5FT5LbARLPWJFWhG2Tyi4ympIR1jsbIkKEov90wIKw21xr/FuzDchlcECgYEA+n8U85mWj2sutydIaAXLOxbrwg6sWdKts2Jovkaftf6LKA6bJT5X3sf9eMJZDM6wlYcI9BoBdUI29R+u23kjhj7Y10N1Qqerkb8ujcxr3d1yf3qYpLEWxyjNqoAqjvDAp1Tp8qMPc/wCiBAQkPvTE+Sp467RDZ1aDcryg8y5wZkCgYEA41yIxHvC5qM5MdWFQq0rxkbIYpHBSadMxFH+AoBmmxuYXqarpwCIwS0nkwCCUfL7Nvdfv0Gkf3EEaABKCwzGwrfN12BSMwTJYS4A5NNpBe7pREwaSml4aW5o7s2b3gmtdx6cAbD0oJ8HI6MnnL+/Khk466eWrQsIS+lXWMBDSCECgYAR0bR117kkHqXGFZ9K9w6L94dx2IVeJmSA3EFDN9bopWDUyqUyswqhKGzZiEm5ZYKeQGrconT0GG+8ZDKWHjnutM3MElpnEXJc/dKb96y8raIVe20cWhSaukZXGKLuZCXwQVQbFIpm38h2UV48Ug2j3qJPNgJdC5J6ZLN3uLqGEQKBgFtyxLAC94m87SxWLZt7+7dskPzUk2IEoKP2Nqza6GpK1yZ681/gnyDUAK7n7YL4sIKTTTeoN3nrA1KxixaWtPts4qZWX7mVm0ozLrjbL8rrJXgLBCgZ9Ay0FBC5MpBEZDkdXrJvcnWIgV6cKTqrBUDxlCt05O4FGfkuiatw6Z8BAoGABLZ/KAWWhhgo30CeLmZPKR8KTJOGKspJAbyZYS+JU0/sil6KoTsDw2FKVcKDu+YQXWrCLdG7N31Udo1RnBIHV74iu1/75e1K7Ef2pvoJxfqkmanjNQ0Ps3mrTUcsVDe5Dr33GaLGmxBkotjCPh2NCV4ljxwlr0JxvEC6RBZxdV8=";
/// The matching self-signed X509 cert (DER, base64) — embedded in the
/// IdP metadata `<X509Certificate>` so ACS verifies against it.
const SIGNING_CERT_DER_B64: &str = "MIIDKzCCAhOgAwIBAgIUJxcSVNvJnSWxMmPmx7kM8jOozJcwDQYJKoZIhvcNAQELBQAwJDEiMCAGA1UEAwwZb2dyZW5vdGVzLXNhbWwtaGFwcHktdGVzdDAgFw0yNjA2MjYwMjUxMzhaGA8yMTI2MDYwMjAyNTEzOFowJDEiMCAGA1UEAwwZb2dyZW5vdGVzLXNhbWwtaGFwcHktdGVzdDCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAN55OxNOQuxt94HtjQNDxxa1eeyPkRTRhpWIlbFDyn1aT+4ntRwt2MoAXM1lGtW5eWyW5pa2NHEx/QqseP1GoOj41HxDkd/YIee5Eu9dc4ugg5Aqz3H/omy0J8HiZ61VaxOHOMtLKSYZFa+veBOLgrkbXG28wNYru8SM4ksqmYj0oMGqi8Tbal7rPxbAiPnIini0r+7wmqFZ6dSxrsZiOcpOqsGvsYWrzktmwglL2MnBMqdQr/xPhNtbICMZfwm+PL+lu3biVetFpQjAWL76gdVX7NPjC6wU6o0He6dPrHHM11RIObx6xzp2bcGTwLIN76vJUgFojvbvtU7F+g1v/LkCAwEAAaNTMFEwHQYDVR0OBBYEFJIMcps/7lm5RBYF06YTQTfgHBWgMB8GA1UdIwQYMBaAFJIMcps/7lm5RBYF06YTQTfgHBWgMA8GA1UdEwEB/wQFMAMBAf8wDQYJKoZIhvcNAQELBQADggEBADdoJKnK3FJOiLKAWWht2LwOyaYRdOoPhgkSLy8nhEMWNSII9d+80UUA3NG7LryP/Drrf+0PKU9dPsIuGfSINTE9uChHTBpLJxu4k9nYevuwYfJ+t7PREh/mtyI7AH0cZAJKUeJoemUDdhubXF0yYhKPdfcn6ONfDivxN3Kn3SVMFCByFci448CMTfM4C38CHZTRPmOwcP2BWERcZ0ANpy6M7o+fZuD48WEB0kke5OiXgjoRFW1A+v4fm44TMabLqfzW39CqJAnNSM/uZyRsCI7TFv4ONkqmbyxbkiYM3iydESZIupBcZDX0IR9LvHQHuP4h0i/PZ0XSD2LHORImr04=";

const IDP_ENTITY_ID: &str = "https://idp.example.test/saml/test";
// Mirrors `frontend_origin` in the test harness (common/mod.rs).
const ORIGIN: &str = "http://localhost:8080";

/// IdP metadata embedding OUR signing cert, so the ACS verification
/// step checks the assertion against a key we hold the private half
/// of. Shape matches `common::saml_test_idp_metadata`, cert swapped.
fn idp_metadata_with_signing_cert() -> String {
    format!(
        r##"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="{IDP_ENTITY_ID}">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <KeyDescriptor use="signing">
      <KeyInfo xmlns="http://www.w3.org/2000/09/xmldsig#">
        <X509Data><X509Certificate>{SIGNING_CERT_DER_B64}</X509Certificate></X509Data>
      </KeyInfo>
    </KeyDescriptor>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.example.test/sso/redirect"/>
  </IDPSSODescriptor>
</EntityDescriptor>"##
    )
}

/// Build a SAMLResponse with all the fields the ACS handler checks,
/// embed a SHA-256 `<ds:Signature>` template over the Response, and
/// sign it in-process. samael's verify path reduces the document to
/// the signed element and parses it as a `Response`, so we sign the
/// *Response* (not just the Assertion) — signing only the Assertion
/// would reduce to a subtree that doesn't parse as a Response.
fn build_signed_saml_response(request_id: &str, name_id: &str, email: &str) -> String {
    use chrono::{Duration, SecondsFormat, Utc};
    let now = Utc::now();
    let issue_instant = now.to_rfc3339_opts(SecondsFormat::Secs, true);
    // Window the assertion comfortably inside samael's bounds
    // (max_issue_delay 90s, max_clock_skew 60s).
    let not_before = (now - Duration::seconds(60)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let not_on_or_after = (now + Duration::seconds(300)).to_rfc3339_opts(SecondsFormat::Secs, true);

    // XML ID attributes must be NCNames — prefix with `_` so they
    // never start with a digit (nanoid's alphabet can).
    let response_id = format!("_resp_{}", nanoid::nanoid!(20));
    let assertion_id = format!("_asrt_{}", nanoid::nanoid!(20));
    let acs_url = format!("{ORIGIN}/api/v1/auth/saml/acs");
    let audience = format!("{ORIGIN}/api/v1/auth/saml/metadata");

    let unsigned = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
                xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"
                ID="{response_id}" Version="2.0"
                IssueInstant="{issue_instant}"
                Destination="{acs_url}"
                InResponseTo="{request_id}">
  <saml:Issuer>{IDP_ENTITY_ID}</saml:Issuer>
  <samlp:Status>
    <samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/>
  </samlp:Status>
  <saml:Assertion ID="{assertion_id}" Version="2.0" IssueInstant="{issue_instant}">
    <saml:Issuer>{IDP_ENTITY_ID}</saml:Issuer>
    <ds:Signature xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:SignedInfo><ds:CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/><ds:SignatureMethod Algorithm="http://www.w3.org/2001/04/xmldsig-more#rsa-sha256"/><ds:Reference URI="#{assertion_id}"><ds:Transforms><ds:Transform Algorithm="http://www.w3.org/2000/09/xmldsig#enveloped-signature"/><ds:Transform Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/></ds:Transforms><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/><ds:DigestValue></ds:DigestValue></ds:Reference></ds:SignedInfo><ds:SignatureValue></ds:SignatureValue></ds:Signature>
    <saml:Subject>
      <saml:NameID Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress">{name_id}</saml:NameID>
      <saml:SubjectConfirmation Method="urn:oasis:names:tc:SAML:2.0:cm:bearer">
        <saml:SubjectConfirmationData InResponseTo="{request_id}"
          Recipient="{acs_url}" NotOnOrAfter="{not_on_or_after}"/>
      </saml:SubjectConfirmation>
    </saml:Subject>
    <saml:Conditions NotBefore="{not_before}" NotOnOrAfter="{not_on_or_after}">
      <saml:AudienceRestriction>
        <saml:Audience>{audience}</saml:Audience>
      </saml:AudienceRestriction>
    </saml:Conditions>
    <saml:AuthnStatement AuthnInstant="{issue_instant}">
      <saml:AuthnContext>
        <saml:AuthnContextClassRef>urn:oasis:names:tc:SAML:2.0:ac:classes:PasswordProtectedTransport</saml:AuthnContextClassRef>
      </saml:AuthnContext>
    </saml:AuthnStatement>
    <saml:AttributeStatement>
      <saml:Attribute Name="email">
        <saml:AttributeValue>{email}</saml:AttributeValue>
      </saml:Attribute>
      <saml:Attribute Name="name">
        <saml:AttributeValue>Saml Happy User</saml:AttributeValue>
      </saml:Attribute>
    </saml:AttributeStatement>
  </saml:Assertion>
</samlp:Response>"##
    );

    sign_with_xmlsec1(&unsigned)
}

/// Fill the `<ds:Signature>` template by shelling out to the `xmlsec1`
/// CLI — the same xmlsec engine the server uses to *verify*, just
/// driven from the command line. We don't sign in-process via
/// `samael::crypto::sign_xml` because that path hardcodes
/// `XmlSecKeyFormat::Der`, which fails to load an RSA key on the
/// xmlsec1 1.2.41 build here (it reports a misleading `signValueNode
/// == NULL`); the CLI's `--privkey-der` loads the same key fine.
///
/// `--id-attr:ID Assertion` tells xmlsec which attribute the
/// `URI="#..."` reference resolves against (we sign the Assertion, the
/// `WantAssertionsSigned` case samael prunes around). Panics with
/// xmlsec's stderr on failure so template/key drift is obvious.
fn sign_with_xmlsec1(unsigned_xml: &str) -> String {
    use std::io::Write;
    use std::process::Command;

    let tmp = std::env::temp_dir();
    let stamp = nanoid::nanoid!(16);
    let xml_path = tmp.join(format!("ogre_saml_{stamp}.xml"));
    let key_path = tmp.join(format!("ogre_saml_{stamp}.der"));

    std::fs::write(&xml_path, unsigned_xml).unwrap();
    {
        let key_der = B64.decode(SIGNING_KEY_PKCS1_DER_B64).unwrap();
        let mut f = std::fs::File::create(&key_path).unwrap();
        f.write_all(&key_der).unwrap();
    }

    let out = Command::new("xmlsec1")
        .args([
            "--sign",
            "--privkey-der",
            key_path.to_str().unwrap(),
            "--id-attr:ID",
            "Assertion",
            xml_path.to_str().unwrap(),
        ])
        .output()
        .expect("xmlsec1 must be invocable");

    let _ = std::fs::remove_file(&xml_path);
    let _ = std::fs::remove_file(&key_path);

    assert!(
        out.status.success() && !out.stdout.is_empty(),
        "xmlsec1 signing failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("signed XML is UTF-8")
}

/// The happy-path tests need the `xmlsec1` CLI to mint a signed
/// assertion. It ships with the xmlsec1 dev libraries samael links
/// against, so it's present wherever this crate builds — but skip
/// rather than hard-fail if a stripped CI image lacks the binary.
fn xmlsec1_available() -> bool {
    std::process::Command::new("xmlsec1")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// True iff this host can mint a signed assertion (xmlsec1 CLI present)
/// AND samael's *in-process* xmlsec can verify it — the exact pair of
/// capabilities the ACS route exercises. The verify probe is what
/// distinguishes a working CI/Debian runtime from the Fedora dev box
/// where samael's bindings abort at xmldsig.c:442 (see file header):
/// we sign a throwaway assertion via the CLI, then ask samael to
/// verify it against our cert exactly as the route does.
fn inprocess_saml_verify_works() -> bool {
    if !xmlsec1_available() {
        return false;
    }
    let signed = build_signed_saml_response("_req_probe", "probe@idp.test", "probe@idp.test");
    let Ok(cert_der) = B64.decode(SIGNING_CERT_DER_B64) else {
        return false;
    };
    samael::crypto::verify_signed_xml(signed.as_bytes(), &cert_der, Some("ID")).is_ok()
}

macro_rules! require_inprocess_saml_verify {
    () => {
        if !inprocess_saml_verify_works() {
            // A bare skip reports as a green "ok", which would hide
            // whether this ever runs in CI. Set OGRE_REQUIRE_SAML_HAPPY=1
            // in an environment whose xmlsec is known good (CI) to turn
            // the skip into a hard failure, so the happy path is real,
            // enforced coverage there and a silent skip only on affected
            // dev hosts.
            if std::env::var("OGRE_REQUIRE_SAML_HAPPY").is_ok() {
                panic!(
                    "OGRE_REQUIRE_SAML_HAPPY is set but in-process xmlsec cannot \
                     verify a known-good signature on this host — the SAML happy \
                     path is NOT being exercised (samael/xmlsec issue, see file header)."
                );
            }
            eprintln!(
                "SKIP: in-process xmlsec can't verify signatures on this host \
                 (samael 0.0.18 / xmlsec1 ABI mismatch — see file header). \
                 Set OGRE_REQUIRE_SAML_HAPPY=1 on a known-good runtime to enforce."
            );
            return;
        }
    };
}

/// Owner user + workspace + SAML config wired to OUR signing cert.
async fn setup_workspace(app: &common::TestApp, owner_email: &str) -> String {
    let (_, token) = app.create_user(owner_email).await;
    let (_, ws_json) = app
        .json_request(
            Method::POST,
            "/api/v1/workspaces",
            Some(&token),
            Some(serde_json::json!({ "name": "Acme SAML" })),
        )
        .await;
    let ws_id = ws_json["id"].as_str().unwrap().to_string();
    let (put_status, _) = app
        .json_request(
            Method::PUT,
            &format!("/api/v1/workspaces/{ws_id}/saml-config"),
            Some(&token),
            Some(serde_json::json!({
                "idpEntityId": IDP_ENTITY_ID,
                "idpMetadataXml": idp_metadata_with_signing_cert(),
            })),
        )
        .await;
    assert_eq!(put_status, 204, "PUT saml-config setup must succeed");
    ws_id
}

/// POST a SAMLResponse to ACS and return (status, set_cookie_present,
/// body_bytes). Goes through the router directly (rather than
/// `raw_request`) so we can inspect response headers for the session
/// cookie that proves a session was actually minted.
async fn post_acs(
    app: &common::TestApp,
    saml_response_b64: &str,
    relay_state: &str,
) -> (u16, bool, Vec<u8>) {
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let body = format!(
        "SAMLResponse={}&RelayState={}",
        urlencoding::encode(saml_response_b64),
        urlencoding::encode(relay_state),
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/auth/saml/acs")
        .header("content-type", "application/x-www-form-urlencoded")
        .header("x-forwarded-for", app.default_xff.clone())
        .body(axum::body::Body::from(body))
        .unwrap();

    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let has_cookie = resp
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().map(|s| s.contains("refresh")).unwrap_or(false));
    let bytes = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, has_cookie, bytes)
}

#[test]
fn signing_produces_a_non_empty_signature_value() {
    // Fast, infra-free guard on the signing harness itself: if
    // sign_xml can't fill the SHA-256 template (key-format or
    // structure drift), this fails in milliseconds instead of after
    // the integration setup. The ACS tests below assume this works.
    if !xmlsec1_available() {
        eprintln!("SKIP: xmlsec1 CLI not found — cannot mint a signed assertion");
        return;
    }
    let signed = build_signed_saml_response("_req_smoke", "smoke@idp.test", "smoke@idp.test");
    assert!(
        signed.contains("<ds:SignatureValue>") && !signed.contains("<ds:SignatureValue></ds:SignatureValue>"),
        "SignatureValue must be populated after signing; got:\n{signed}"
    );
    assert!(signed.contains("<ds:DigestValue>") && !signed.contains("<ds:DigestValue></ds:DigestValue>"),
        "DigestValue must be populated after signing");
}

#[tokio::test]
async fn acs_accepts_valid_sha256_signed_response() {
    // The happy path the rejection-only suite couldn't exercise: a
    // correctly SHA-256-signed assertion, bound to a pending
    // SP-initiated AuthnRequest, mints a session for the JIT user.
    common::require_infra!();
    require_inprocess_saml_verify!();
    let app = common::TestApp::new().await;
    let ws_id = setup_workspace(&app, "saml-happy-owner@test.com").await;

    // Mint a pending AuthnRequest exactly as `/auth/saml/login` would,
    // so the InResponseTo CSRF gate (#82) finds the matching Redis row.
    let request_id = format!("_req_{}", nanoid::nanoid!(24));
    let stored = app
        .state
        .redis_session
        .try_store_saml_authn_request(&request_id, &ws_id, 300)
        .await
        .unwrap();
    assert!(stored, "AuthnRequest must store");

    let name_id = "saml-happy-user@idp.example.test";
    let signed = build_signed_saml_response(&request_id, name_id, name_id);
    let b64 = B64.encode(signed.as_bytes());

    let (status, has_cookie, body) = post_acs(&app, &b64, &ws_id).await;
    assert_eq!(
        status,
        200,
        "valid signed assertion must be accepted; body: {}",
        String::from_utf8_lossy(&body)
    );
    assert!(has_cookie, "a refresh-session cookie must be set on success");

    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["email"].as_str(),
        Some(name_id),
        "session is for the JIT-provisioned SAML user"
    );
    assert!(
        json["user_id"].as_str().is_some_and(|s| !s.is_empty()),
        "a user_id must be issued"
    );

    app.cleanup().await;
}

#[tokio::test]
async fn acs_rejects_replayed_valid_assertion() {
    // Same valid assertion twice: the second POST must fail. This
    // proves the replay-dedup + single-use InResponseTo gates fire on
    // a genuinely-valid signature (the rejection suite can only show
    // they fire on bad input). First consumes the AuthnRequest row and
    // marks the assertion seen; the replay has neither.
    common::require_infra!();
    require_inprocess_saml_verify!();
    let app = common::TestApp::new().await;
    let ws_id = setup_workspace(&app, "saml-replay-owner@test.com").await;

    let request_id = format!("_req_{}", nanoid::nanoid!(24));
    app.state
        .redis_session
        .try_store_saml_authn_request(&request_id, &ws_id, 300)
        .await
        .unwrap();

    let name_id = "saml-replay-user@idp.example.test";
    let signed = build_signed_saml_response(&request_id, name_id, name_id);
    let b64 = B64.encode(signed.as_bytes());

    let (first, _, _) = post_acs(&app, &b64, &ws_id).await;
    assert_eq!(first, 200, "first use of the assertion must succeed");

    let (second, _, _) = post_acs(&app, &b64, &ws_id).await;
    assert_eq!(second, 401, "replay of the same assertion must be rejected");

    app.cleanup().await;
}

/// An accepted assertion writes `SecurityAudit::SamlAssertionAccepted`
/// with the workspace and IdP NameID — emitted BEFORE session mint so the
/// row survives even if a downstream failure aborts the login. This writer
/// had no coverage; the happy-path test above only checks the session.
/// Runs under the same capability probe as the other happy-path tests
/// (skips where in-process xmlsec verification is broken, enforced in CI
/// via OGRE_REQUIRE_SAML_HAPPY).
#[tokio::test]
async fn acs_success_writes_saml_assertion_accepted_audit_row() {
    use ogrenotes_storage::models::security_audit::SecurityAuditAction;

    common::require_infra!();
    require_inprocess_saml_verify!();
    let app = common::TestApp::new().await;
    let ws_id = setup_workspace(&app, "saml-audit-owner@test.com").await;

    let request_id = format!("_req_{}", nanoid::nanoid!(24));
    let stored = app
        .state
        .redis_session
        .try_store_saml_authn_request(&request_id, &ws_id, 300)
        .await
        .unwrap();
    assert!(stored, "AuthnRequest must store");

    let name_id = "saml-audit-user@idp.example.test";
    let signed = build_signed_saml_response(&request_id, name_id, name_id);
    let b64 = B64.encode(signed.as_bytes());

    let (status, _, body) = post_acs(&app, &b64, &ws_id).await;
    assert_eq!(
        status,
        200,
        "valid signed assertion must be accepted; body: {}",
        String::from_utf8_lossy(&body)
    );
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let user_id = json["user_id"].as_str().unwrap().to_string();

    // The writer fires via tokio::spawn — poll with the same 10×20ms
    // bound the audit-writer suite uses.
    let mut found = None;
    for _ in 0..10 {
        let rows = app
            .state
            .security_audit_repo
            .list_for_user(&user_id, 20)
            .await
            .unwrap();
        if let Some(row) = rows.into_iter().find(|r| {
            matches!(
                &r.action,
                SecurityAuditAction::SamlAssertionAccepted { workspace_id: w, name_id: n }
                    if w == &ws_id && n == name_id
            )
        }) {
            found = Some(row);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let row = found.expect("expected SamlAssertionAccepted audit row within 200ms");
    assert_eq!(row.user_id, user_id, "subject is the JIT-provisioned SAML user");
    assert_eq!(row.actor_id, user_id, "self-event: actor == subject");

    app.cleanup().await;
}
