// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `/auth/saml/*` — SP metadata + login redirect (Phase 4 M-E4
//! piece C). The ACS handler lands with piece D.
//!
//! Three handlers will live here once the milestone is complete:
//!   GET  /auth/saml/metadata          — SP metadata XML (this file)
//!   GET  /auth/saml/login?workspace=… — AuthnRequest redirect (this file)
//!   POST /auth/saml/acs               — Assertion Consumer (piece D)
//!
//! samael's `service_provider` module is gated on the `xmlsec`
//! feature (libxml2 + xmlsec1 native deps). We build with
//! `default-features = false` so xmlsec is off. That costs us the
//! convenience helpers (`ServiceProvider::redirect`,
//! `ServiceProvider::metadata`); we hand-roll the equivalents here:
//!
//!   - SP metadata is a small fixed-shape XML document; hand-rolled
//!     so we can omit the SLO endpoint (we don't support SLO in v1)
//!     and skip the KeyDescriptor (we don't sign AuthnRequests).
//!   - HTTP-Redirect binding wraps the AuthnRequest as: DEFLATE
//!     (raw, no zlib wrapper) → Base64 → URL-encode → query param
//!     `SAMLRequest`. Implemented inline below; the spec is
//!     "Bindings for SAML V2.0" §3.4.4.1.

use std::io::Write;

use axum::extract::{Form, Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use flate2::write::DeflateEncoder;
use flate2::Compression;
use samael::metadata::EntityDescriptor;
use samael::service_provider::ServiceProviderBuilder;
use serde::Deserialize;

use ogrenotes_storage::models::security_audit::SecurityAuditAction;

use crate::error::ApiError;
use crate::routes::auth::SessionSource;
use crate::routes::audit::record_security_event;
use crate::state::AppState;

/// TTL on the SAML assertion-replay dedup row in Redis. Matches
/// samael's `max_issue_delay` default (90s). An assertion older
/// than this would already be rejected as expired by the verify
/// step, so retaining the dedup record any longer is wasted
/// storage.
const ASSERTION_REPLAY_TTL_SECS: u64 = 90;

/// #27: clock-skew tolerance for SAML assertion `NotBefore` /
/// `NotOnOrAfter` validation. MUST stay `<= ASSERTION_REPLAY_TTL_SECS`:
/// if the skew window were wider than the replay-dedup TTL, a captured
/// assertion could still be accepted (on the skew tolerance) after its
/// dedup row had expired — a replay race. 60s is ample for an
/// NTP-synced IdP/SP and well inside the 90s dedup window.
const SAML_MAX_CLOCK_SKEW_SECS: i64 = 60;

/// XMLDSig algorithm URNs we reject before signature verification.
/// SHA-1 is cryptographically broken: Shambles (2020) made chosen-
/// prefix collisions feasible for ~$45k of cloud compute, which is
/// well inside the budget of a SAML-targeting attacker. Every
/// modern enterprise IdP (Okta, Entra ID, Workspace, AWS IAM
/// Identity Center, AD FS 2.0+, Auth0, Ping, OneLogin, Shibboleth,
/// SimpleSAMLphp) defaults to SHA-256, so this is industry baseline
/// — not a strict-mode opt-in.
///
/// Implemented as a substring scan against the raw response XML
/// because XMLDSig `Algorithm="…"` attribute values are emitted
/// verbatim as these URNs; any whitespace inside an attribute value
/// would itself be a malformed document. The scan runs before
/// `parse_xml_response` so a SHA-1 response never even reaches
/// xmlsec — defense in depth against future xmlsec versions
/// changing their default-accept policy.
const REJECTED_SHA1_ALGORITHMS: &[&str] = &[
    "http://www.w3.org/2000/09/xmldsig#sha1",       // DigestMethod
    "http://www.w3.org/2000/01/xmldsig#rsa-sha1",   // SignatureMethod (RSA)
    "http://www.w3.org/2000/09/xmldsig#dsa-sha1",   // SignatureMethod (DSA)
    "http://www.w3.org/2000/09/xmldsig#hmac-sha1",  // SignatureMethod (HMAC)
];

/// HTTP-POST binding URN. Used in the AssertionConsumer element of
/// the SP metadata so the IdP knows which binding to POST to.
const HTTP_POST_BINDING: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST";

/// HTTP-Redirect binding URN. The IdP MUST advertise an SSO endpoint
/// under this binding for our login flow — the login handler reads
/// it back at request time.
const HTTP_REDIRECT_BINDING: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect";

/// #80: byte cap on the SAML Subject NameID accepted on the JIT path.
/// Far above any real-world IdP NameID (transient/persistent NameIDs
/// are typically < 256 bytes); bounds a hostile IdP from injecting a
/// 400 KB string into the `external_id` GSI key.
const MAX_NAME_ID_LEN: usize = 1024;

/// #80: is a SAML Subject NameID within the byte cap we'll persist?
fn name_id_within_cap(name_id: &str) -> bool {
    name_id.len() <= MAX_NAME_ID_LEN
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/metadata", get(metadata))
        .route("/login", get(login_redirect))
        .route("/acs", post(acs))
}

// ─── SP metadata ─────────────────────────────────────────────

/// GET /auth/saml/metadata
///
/// Returns the SP metadata XML document. Workspace admins copy this
/// URL (or its body) into their IdP's "add a relying party / service
/// provider" config so the IdP knows where to POST assertions back
/// to. Unsigned in v1.
async fn metadata(State(state): State<AppState>) -> impl IntoResponse {
    let origin = &state.config.frontend_origin;
    let entity_id = format!("{origin}/api/v1/auth/saml/metadata");
    let acs_url = format!("{origin}/api/v1/auth/saml/acs");

    // `WantAssertionsSigned="true"` is the security-relevant
    // assertion: every assertion the IdP sends MUST be signed.
    // Piece D's verify path rejects unsigned. `AuthnRequestsSigned=
    // "false"` because we don't sign AuthnRequests in v1 — that
    // requires an SP keypair that we don't yet provision.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
                  entityID="{entity_id}">
  <SPSSODescriptor AuthnRequestsSigned="false"
                   WantAssertionsSigned="true"
                   protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <NameIDFormat>urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress</NameIDFormat>
    <AssertionConsumerService Binding="{HTTP_POST_BINDING}"
                              Location="{acs_url}"
                              index="0"
                              isDefault="true"/>
  </SPSSODescriptor>
</EntityDescriptor>
"#
    );

    (
        [(header::CONTENT_TYPE, "application/samlmetadata+xml")],
        xml,
    )
}

// ─── Login redirect (AuthnRequest) ───────────────────────────

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginParams {
    workspace: String,
}

/// GET /auth/saml/login?workspace=<id>
///
/// Reads the workspace's IdP metadata, finds the HTTP-Redirect SSO
/// endpoint, builds an unsigned AuthnRequest, encodes it per the
/// HTTP-Redirect binding (DEFLATE → Base64 → URL-encode), and 302s
/// the user to the IdP with `RelayState=<workspace_id>` so the ACS
/// handler (piece D) can recover the workspace context.
async fn login_redirect(
    State(state): State<AppState>,
    Query(params): Query<LoginParams>,
) -> Result<Response, ApiError> {
    let config = state
        .workspace_saml_config_repo
        .get(&params.workspace)
        .await?
        .ok_or(ApiError::NotFound(
            "workspace has no SAML configuration".to_string(),
        ))?;

    // Parse the stored IdP metadata. A broken document surfaces as a
    // 400 here rather than as a confusing browser-side failure
    // inside the IdP's flow.
    let idp_metadata: EntityDescriptor = config.idp_metadata_xml.parse().map_err(|e| {
        tracing::warn!(
            workspace_id = %params.workspace,
            error = ?e,
            "stored IdP metadata failed to parse"
        );
        ApiError::BadRequest("workspace IdP metadata is malformed".to_string())
    })?;

    let sso_url = idp_sso_redirect_url(&idp_metadata).ok_or_else(|| {
        tracing::warn!(
            workspace_id = %params.workspace,
            "IdP metadata has no HTTP-Redirect SSO endpoint"
        );
        ApiError::BadRequest(
            "IdP metadata is missing an HTTP-Redirect SSO endpoint".to_string(),
        )
    })?;

    // #81: the SSO Location is admin-uploaded, stored, then handed to a
    // 302 verbatim. A hostile/careless workspace admin could set it to
    // `javascript:`, `data:`, or a phishing host — an open redirect that
    // lets them attack their own workspace's users. We can't validate the
    // host (any IdP host is legitimate), but we can require an https
    // scheme so the redirect can never become a script/data URI. http is
    // permitted only under dev_mode, mirroring validate_frontend_origin.
    if !is_safe_redirect_scheme(&sso_url, state.config.dev_mode) {
        tracing::warn!(
            workspace_id = %params.workspace,
            "IdP SSO URL rejected: disallowed scheme"
        );
        return Err(ApiError::BadRequest("invalid IdP configuration".to_string()));
    }

    let origin = &state.config.frontend_origin;
    let sp_entity_id = format!("{origin}/api/v1/auth/saml/metadata");
    let acs_url = format!("{origin}/api/v1/auth/saml/acs");

    let (authn_xml, request_id) =
        build_authn_request_xml(&sp_entity_id, &acs_url, &sso_url);

    // #82: Track the outstanding AuthnRequest so the ACS handler
    // can verify the assertion's InResponseTo matches a request
    // *we* issued. A captured assertion that was minted for a
    // different SP at the same IdP carries no InResponseTo for
    // our request — its replay against our /acs is rejected.
    //
    // TTL of 5 min covers the slowest realistic IdP MFA flow. Fail
    // closed: a Redis blip here means the user can't complete
    // login (better than silently weakening the CSRF guard).
    state
        .redis_session
        .try_store_saml_authn_request(
            &request_id,
            &params.workspace,
            SAML_AUTHN_REQUEST_TTL_SECS,
        )
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                workspace_id = %params.workspace,
                "failed to store SAML AuthnRequest in Redis"
            );
            ApiError::Internal("SAML state store".to_string())
        })?;

    let saml_request = encode_for_redirect(&authn_xml).map_err(|e| {
        tracing::error!(error = %e, "failed to encode AuthnRequest");
        ApiError::Internal("SAML encode".to_string())
    })?;

    let redirect_url = format!(
        "{sso_url}{sep}SAMLRequest={req}&RelayState={rs}",
        sso_url = sso_url,
        sep = if sso_url.contains('?') { "&" } else { "?" },
        req = saml_request,
        rs = urlencoding::encode(&params.workspace),
    );

    Ok(Redirect::temporary(&redirect_url).into_response())
}

/// #82: TTL on the pending-AuthnRequest Redis row. 5 min covers the
/// slowest realistic IdP MFA flow (which is the canonical "I went
/// to make coffee" path). Tighter than the 90 s replay window on
/// `try_mark_assertion_seen` because that's bounded by the
/// assertion's `NotOnOrAfter`; this is bounded by user attention.
const SAML_AUTHN_REQUEST_TTL_SECS: u64 = 5 * 60;

/// Walk the IdP metadata for an HTTP-Redirect SSO endpoint. Returns
/// the Location URL of the first match (most IdPs publish exactly
/// one); `None` if none found.
fn idp_sso_redirect_url(metadata: &EntityDescriptor) -> Option<String> {
    metadata
        .idp_sso_descriptors
        .as_ref()?
        .iter()
        .flat_map(|d| d.single_sign_on_services.iter())
        .find(|svc| svc.binding == HTTP_REDIRECT_BINDING)
        .map(|svc| svc.location.clone())
}

/// #81: validate the scheme of an admin-supplied SSO redirect URL.
/// Requires `https`; `http` is permitted only when `allow_http` (set
/// from `dev_mode`) so local/test IdPs keep working. Anything else —
/// `javascript:`, `data:`, `file:`, a scheme-less string — is rejected.
fn is_safe_redirect_scheme(location: &str, allow_http: bool) -> bool {
    let scheme = match location.split_once(':') {
        Some((s, _)) => s.trim().to_ascii_lowercase(),
        None => return false,
    };
    scheme == "https" || (allow_http && scheme == "http")
}

/// Build the AuthnRequest XML document. Hand-rolled rather than
/// going through `samael::schema::AuthnRequest::try_into::<Event>()`
/// because samael's serializer wraps the result in a quick-xml event
/// shape that's awkward to extract from. The shape here matches RFC
/// SAML 2.0 §3.2.1 exactly — element names, namespace prefixes, the
/// `ProtocolBinding="HTTP-POST"` attribute that tells the IdP to
/// POST the response back to our ACS.
///
/// Returns `(xml, request_id)`. The caller stores the `request_id`
/// in Redis (#82) so the ACS handler can later verify the
/// assertion's `InResponseTo` matches an outstanding request
/// *we* issued.
fn build_authn_request_xml(
    sp_entity_id: &str,
    acs_url: &str,
    idp_sso_url: &str,
) -> (String, String) {
    let request_id = format!("_id-{}", nanoid::nanoid!(16));
    let issue_instant = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let xml = format!(
        r#"<samlp:AuthnRequest xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol"
                     xmlns:saml="urn:oasis:names:tc:SAML:2.0:assertion"
                     ID="{request_id}"
                     Version="2.0"
                     IssueInstant="{issue_instant}"
                     Destination="{idp_sso_url}"
                     ProtocolBinding="{HTTP_POST_BINDING}"
                     AssertionConsumerServiceURL="{acs_url}">
  <saml:Issuer>{sp_entity_id}</saml:Issuer>
  <samlp:NameIDPolicy AllowCreate="true"
                      Format="urn:oasis:names:tc:SAML:1.1:nameid-format:emailAddress"/>
</samlp:AuthnRequest>"#
    );
    (xml, request_id)
}

/// Encode an AuthnRequest for the SAML HTTP-Redirect binding.
/// Per "Bindings for SAML V2.0" §3.4.4.1: DEFLATE the XML (no zlib
/// header — raw RFC 1951 deflate), Base64 the result, then URL-
/// encode for embedding in a query string.
fn encode_for_redirect(xml: &str) -> Result<String, std::io::Error> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(xml.as_bytes())?;
    let deflated = encoder.finish()?;
    let base64ed = BASE64_STANDARD.encode(&deflated);
    Ok(urlencoding::encode(&base64ed).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #27: the SAML clock-skew tolerance must never exceed the
    /// assertion-replay dedup TTL, or a captured assertion could be
    /// accepted on the skew window after its dedup row expired (a replay
    /// race). This pins that invariant so neither constant can drift past
    /// it unnoticed.
    #[test]
    fn saml_clock_skew_within_replay_dedup_window() {
        assert!(SAML_MAX_CLOCK_SKEW_SECS >= 0);
        assert!(
            SAML_MAX_CLOCK_SKEW_SECS as u64 <= ASSERTION_REPLAY_TTL_SECS,
            "clock-skew tolerance ({SAML_MAX_CLOCK_SKEW_SECS}s) must be \
             <= the assertion-replay dedup TTL ({ASSERTION_REPLAY_TTL_SECS}s)",
        );
    }

    /// Sample IdP metadata with an HTTP-Redirect SSO endpoint.
    /// Borrowed shape from real Okta/AAD metadata; certs omitted.
    const METADATA_WITH_REDIRECT: &str = r#"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.com/metadata">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
      Location="https://idp.example.com/sso/redirect"/>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
      Location="https://idp.example.com/sso/post"/>
  </IDPSSODescriptor>
</EntityDescriptor>"#;

    /// IdP that only advertises HTTP-POST SSO. Should fail our
    /// redirect-binding lookup.
    const METADATA_POST_ONLY: &str = r#"<?xml version="1.0"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
  entityID="https://idp.example.com/metadata">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST"
      Location="https://idp.example.com/sso/post"/>
  </IDPSSODescriptor>
</EntityDescriptor>"#;

    #[test]
    fn idp_sso_redirect_url_picks_the_redirect_binding() {
        let meta: EntityDescriptor = METADATA_WITH_REDIRECT.parse().unwrap();
        assert_eq!(
            idp_sso_redirect_url(&meta).as_deref(),
            Some("https://idp.example.com/sso/redirect")
        );
    }

    #[test]
    fn idp_sso_redirect_url_returns_none_when_only_post_binding() {
        let meta: EntityDescriptor = METADATA_POST_ONLY.parse().unwrap();
        assert!(idp_sso_redirect_url(&meta).is_none());
    }

    #[test]
    fn rejects_non_https_sso_schemes() {
        // #81: the SSO Location is admin-supplied. Only https (and http
        // under dev_mode) may be redirected to; script/data/file and
        // scheme-less strings are an open-redirect / XSS vector.
        assert!(is_safe_redirect_scheme("https://idp.example.com/sso", false));
        assert!(is_safe_redirect_scheme("HTTPS://idp.example.com/sso", false));

        // http allowed only when dev_mode is on.
        assert!(!is_safe_redirect_scheme("http://idp.example.com/sso", false));
        assert!(is_safe_redirect_scheme("http://idp.example.com/sso", true));

        // Dangerous / malformed schemes rejected even in dev_mode.
        for evil in [
            "javascript:alert(1)",
            "JavaScript:alert(document.cookie)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "ftp://idp.example.com/sso",
            "//idp.example.com/sso", // scheme-relative
            "idp.example.com/sso",   // no scheme
            "",
        ] {
            assert!(
                !is_safe_redirect_scheme(evil, true),
                "must reject disallowed scheme: {evil:?}"
            );
        }
    }

    #[test]
    fn name_id_cap_rejects_oversize_subjects() {
        // #80: NameID is the external_id GSI key — an over-long one is
        // rejected (not truncated, which could alias two subjects).
        assert!(name_id_within_cap("user@example.com"));
        assert!(name_id_within_cap(&"a".repeat(MAX_NAME_ID_LEN)));
        assert!(!name_id_within_cap(&"a".repeat(MAX_NAME_ID_LEN + 1)));
        // A hostile IdP's 400 KB NameID is well over the cap.
        assert!(!name_id_within_cap(&"x".repeat(400 * 1024)));
    }

    #[test]
    fn build_authn_request_xml_has_required_attributes() {
        let (xml, request_id) = build_authn_request_xml(
            "https://sp.example.com/saml/metadata",
            "https://sp.example.com/saml/acs",
            "https://idp.example.com/sso",
        );
        assert!(xml.contains("Version=\"2.0\""));
        assert!(xml.contains("ProtocolBinding=\"urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST\""));
        assert!(xml.contains("AssertionConsumerServiceURL=\"https://sp.example.com/saml/acs\""));
        assert!(xml.contains("Destination=\"https://idp.example.com/sso\""));
        assert!(xml.contains("<saml:Issuer>https://sp.example.com/saml/metadata</saml:Issuer>"));
        // ID must be present; nanoid is 16 chars so the literal is
        // 20 chars including the `_id-` prefix.
        assert!(xml.contains("ID=\"_id-"));
        assert!(request_id.starts_with("_id-"));
        assert!(xml.contains(&format!("ID=\"{request_id}\"")));
    }

    #[test]
    fn encode_for_redirect_round_trips_via_inflate() {
        use flate2::read::DeflateDecoder;
        use std::io::Read;

        let xml = "<samlp:AuthnRequest/>";
        let encoded = encode_for_redirect(xml).unwrap();
        // Reverse: URL-decode → Base64-decode → DEFLATE-decode.
        let b64 = urlencoding::decode(&encoded).unwrap().into_owned();
        let deflated = BASE64_STANDARD.decode(&b64).unwrap();
        let mut decoder = DeflateDecoder::new(&deflated[..]);
        let mut out = String::new();
        decoder.read_to_string(&mut out).unwrap();
        assert_eq!(out, xml);
    }

    #[test]
    fn find_dtd_construct_catches_doctype() {
        let xml = r#"<?xml version="1.0"?>
<!DOCTYPE foo [<!ENTITY x SYSTEM "http://evil/">]>
<Response/>"#;
        assert_eq!(find_dtd_construct(xml), Some("<!DOCTYPE"));
    }

    #[test]
    fn find_dtd_construct_catches_bare_entity_declaration() {
        // `<!ENTITY` without a `<!DOCTYPE` wrapper is malformed but
        // we still want to reject — substring-only scan, no parser
        // context-sensitivity. Defense in depth against a libxml2
        // parser that accepts loose DTDs.
        let xml = "<Response><!ENTITY x \"hi\"/></Response>";
        assert_eq!(find_dtd_construct(xml), Some("<!ENTITY"));
    }

    #[test]
    fn find_dtd_construct_catches_lowercase_doctype() {
        // libxml2 in recovery mode accepts <!doctype despite XML 1.0
        // requiring uppercase. Bypass coverage.
        let xml = "<!doctype foo [<!entity x SYSTEM \"http://evil/\">]><Response/>";
        assert!(find_dtd_construct(xml).is_some());
    }

    #[test]
    fn find_dtd_construct_catches_lowercase_entity() {
        let xml = "<Response><!entity x \"hi\"/></Response>";
        assert_eq!(find_dtd_construct(xml), Some("<!entity"));
    }

    #[test]
    fn find_dtd_construct_catches_whitespace_doctype() {
        // `<! DOCTYPE` (space after `<!`) is invalid XML but libxml2
        // recovery mode has historically accepted it. Known WAF
        // bypass pattern.
        let xml = r#"<?xml version="1.0"?><! DOCTYPE foo><Response/>"#;
        assert!(find_dtd_construct(xml).is_some());
    }

    #[test]
    fn find_dtd_construct_catches_tab_whitespace_after_bang() {
        // XML spec S production includes #x9 (tab). A `<!\tDOCTYPE`
        // is structurally identical to `<! DOCTYPE` for libxml2's
        // recovery-mode parser.
        let xml = "<!\tDOCTYPE foo><Response/>";
        assert!(find_dtd_construct(xml).is_some());
    }

    #[test]
    fn find_dtd_construct_catches_lf_whitespace_after_bang() {
        // XML spec S production includes #xA (LF).
        let xml = "<!\nDOCTYPE foo><Response/>";
        assert!(find_dtd_construct(xml).is_some());
    }

    #[test]
    fn find_dtd_construct_catches_cr_whitespace_after_bang() {
        // XML spec S production includes #xD (CR). Some WAFs
        // canonicalize CRLF to LF — this branch fires before the
        // canonical form is reached.
        let xml = "<!\rDOCTYPE foo><Response/>";
        assert!(find_dtd_construct(xml).is_some());
    }

    #[test]
    fn find_dtd_construct_passes_clean_xml() {
        let xml = r#"<?xml version="1.0"?>
<samlp:Response xmlns:samlp="urn:oasis:names:tc:SAML:2.0:protocol">
  <samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>
</samlp:Response>"#;
        assert!(find_dtd_construct(xml).is_none());
    }

    #[test]
    fn find_sha1_algorithm_returns_none_for_sha256_xml() {
        // r##"..."## because the XML contains `"#` (URI fragment
        // references like `URI="#_a"`) which would close a regular
        // raw string early.
        let xml = r##"<Response>
          <ds:Signature xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
            <ds:SignedInfo>
              <ds:CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/>
              <ds:SignatureMethod Algorithm="http://www.w3.org/2001/04/xmldsig-more#rsa-sha256"/>
              <ds:Reference URI="#_a">
                <ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/>
                <ds:DigestValue>abc=</ds:DigestValue>
              </ds:Reference>
            </ds:SignedInfo>
          </ds:Signature>
        </Response>"##;
        assert!(find_sha1_algorithm(xml).is_none());
    }

    #[test]
    fn find_sha1_algorithm_catches_sha1_digest() {
        let xml = r#"<DigestMethod Algorithm="http://www.w3.org/2000/09/xmldsig#sha1"/>"#;
        assert_eq!(
            find_sha1_algorithm(xml),
            Some("http://www.w3.org/2000/09/xmldsig#sha1"),
        );
    }

    #[test]
    fn find_sha1_algorithm_catches_rsa_sha1_signature() {
        let xml =
            r#"<SignatureMethod Algorithm="http://www.w3.org/2000/01/xmldsig#rsa-sha1"/>"#;
        assert_eq!(
            find_sha1_algorithm(xml),
            Some("http://www.w3.org/2000/01/xmldsig#rsa-sha1"),
        );
    }

    #[test]
    fn find_sha1_algorithm_catches_dsa_sha1_signature() {
        let xml =
            r#"<SignatureMethod Algorithm="http://www.w3.org/2000/09/xmldsig#dsa-sha1"/>"#;
        assert!(find_sha1_algorithm(xml).is_some());
    }

    #[test]
    fn find_sha1_algorithm_catches_hmac_sha1_signature() {
        let xml =
            r#"<SignatureMethod Algorithm="http://www.w3.org/2000/09/xmldsig#hmac-sha1"/>"#;
        assert!(find_sha1_algorithm(xml).is_some());
    }

    #[test]
    fn find_sha1_algorithm_does_not_false_positive_on_sha1_fragment_text() {
        // A fragment like "sha1" inside an unrelated attribute value
        // must not trip the scan.
        let xml = r#"<Response><saml:Attribute Name="sha1-token">x</saml:Attribute></Response>"#;
        assert!(find_sha1_algorithm(xml).is_none());
    }

    #[test]
    fn find_sha1_algorithm_does_not_false_positive_on_xmldsig_namespace_decl() {
        // The XMLDSig namespace URI (`http://www.w3.org/2000/09/xmldsig#`)
        // is a prefix of `#sha1` but is itself NOT a rejected algorithm.
        // A SHA-256 doc that uses the `ds:` prefix must pass.
        let xml = r##"<ds:Signature xmlns:ds="http://www.w3.org/2000/09/xmldsig#">
          <ds:SignedInfo>
            <ds:SignatureMethod Algorithm="http://www.w3.org/2001/04/xmldsig-more#rsa-sha256"/>
            <ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/>
          </ds:SignedInfo>
        </ds:Signature>"##;
        assert!(find_sha1_algorithm(xml).is_none());
    }

    #[test]
    fn find_sha1_algorithm_does_not_false_positive_on_full_urn_in_comment() {
        // The full SHA-1 URN inside an XML comment must not trip the
        // scan — only an actual `Algorithm="…"` declaration counts.
        let xml = r#"<Response>
          <!-- legacy note: do not use http://www.w3.org/2000/09/xmldsig#sha1 -->
          <SignatureMethod Algorithm="http://www.w3.org/2001/04/xmldsig-more#rsa-sha256"/>
        </Response>"#;
        assert!(find_sha1_algorithm(xml).is_none());
    }

    #[test]
    fn find_sha1_algorithm_catches_single_quoted_attribute() {
        // XML attributes can use either '...' or "..." quoting; our
        // scan must catch both. Real IdPs use double quotes but the
        // spec permits single quotes.
        let xml = r#"<DigestMethod Algorithm='http://www.w3.org/2000/09/xmldsig#sha1'/>"#;
        assert!(find_sha1_algorithm(xml).is_some());
    }

    #[test]
    fn encode_for_redirect_output_is_url_safe() {
        // The output is shoved into a query string as-is. Verify
        // no `&`, `=`, `?`, `#`, or whitespace leaks through.
        let xml = "<samlp:AuthnRequest ID=\"abc&def\"/>";
        let encoded = encode_for_redirect(xml).unwrap();
        for bad in ['&', '=', '?', '#', ' '] {
            assert!(
                !encoded.contains(bad),
                "encoded output must not contain {bad:?}: {encoded}"
            );
        }
    }
}

// ─── ACS (Assertion Consumer Service) ────────────────────────

/// SAML ACS form body. RelayState carries the workspace_id the
/// login handler placed there.
#[derive(Deserialize)]
struct AcsForm {
    #[serde(rename = "SAMLResponse")]
    saml_response: String,
    #[serde(rename = "RelayState")]
    relay_state: Option<String>,
}

/// `POST /auth/saml/acs`
///
/// The IdP POSTs a signed SAMLResponse here after the user completes
/// the IdP's auth flow. We:
///   1. Pull workspace_id from RelayState.
///   2. Load that workspace's stored IdP metadata.
///   3. Build a samael ServiceProvider configured with our entity_id
///      and ACS URL, plus the workspace's IdP metadata.
///   4. Base64-decode the body ourselves, scan for SHA-1 algorithm
///      URNs (rejected outright), then call `parse_xml_response` on
///      the decoded XML, which:
///        - verifies XMLDSig signature against the IdP cert
///        - checks Destination matches our ACS URL
///        - checks issuer matches the IdP entity_id
///        - checks status code is Success
///        - checks audience restriction
///        - checks NotBefore / NotOnOrAfter timestamps
///        - returns the inner Assertion
///   5. Extract NameID + email + name attributes.
///   6. JIT user via find_or_create_saml_user (dedupes on
///      external_id = NameID).
///   7. Mint session + refresh cookie via issue_session_response.
///   8. Audit SamlAssertionAccepted.
///   9. Redirect to /auth/complete (cookie-only path) so the
///      frontend hydrates and lands at home.
///
/// Every error path returns ApiError::Unauthorized so an attacker
/// can't distinguish "bad signature" from "expired" from "wrong
/// audience" from "missing email attribute." The structured tracing
/// events on each branch are the server-side forensic trail.
async fn acs(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Form(form): Form<AcsForm>,
) -> Result<Response, ApiError> {
    // Per-source-IP rate limit (#83). The ACS endpoint is
    // unauthenticated by necessity and triggers DDB + Redis + XML
    // parse work on every call — without the limit a single source
    // can degrade SAML SSO and amortize DDB capacity used by the
    // workspace's SAML config + user tables. Matches the per-IP
    // shape used by /auth/login.
    let ip = crate::middleware::rate_limit::ip_identifier(&headers);
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "saml_acs",
        &ip,
        state.config.rate_limit_saml_acs_per_min,
        60,
    )
    .await?;

    // Distinguish None vs Some("") in the log so an integration
    // debugger can tell whether the IdP omitted RelayState entirely
    // (likely missing-config on the IdP side) from sending an
    // empty value (likely an SP-initiated flow whose RelayState
    // was stripped en route).
    let workspace_id = match form.relay_state.as_deref() {
        None => {
            tracing::warn!(
                "SAML ACS hit with no RelayState field (IdP-initiated without state?)"
            );
            return Err(ApiError::Unauthorized);
        }
        Some(s) if s.trim().is_empty() => {
            tracing::warn!("SAML ACS hit with empty RelayState");
            return Err(ApiError::Unauthorized);
        }
        Some(s) => s.to_string(),
    };

    let config = state
        .workspace_saml_config_repo
        .get(&workspace_id)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, workspace_id, "SAML config lookup failed");
            ApiError::Unauthorized
        })?
        .ok_or_else(|| {
            tracing::warn!(workspace_id, "ACS hit for workspace with no SAML config");
            ApiError::Unauthorized
        })?;

    let idp_metadata: EntityDescriptor = config.idp_metadata_xml.parse().map_err(|e| {
        tracing::warn!(error = ?e, workspace_id, "stored IdP metadata unparseable at ACS time");
        ApiError::Unauthorized
    })?;

    let origin = &state.config.frontend_origin;
    let sp = ServiceProviderBuilder::default()
        .entity_id(format!("{origin}/api/v1/auth/saml/metadata"))
        .acs_url(format!("{origin}/api/v1/auth/saml/acs"))
        .idp_metadata(idp_metadata)
        // #27: pin the assertion-acceptance time window explicitly rather
        // than relying on samael's defaults (skew 180s, issue-delay 90s).
        // The default 180s clock-skew is WIDER than our 90s assertion-replay
        // dedup TTL: a captured SAMLResponse could be accepted on the skew
        // tolerance up to 90s after the dedup key expired, a residual replay
        // race (otherwise bounded by the single-use InResponseTo gate). Tie
        // both bounds to the dedup TTL so the window an assertion is
        // accepted can never outlast the replay-dedup row that protects it:
        //   max_issue_delay  == ASSERTION_REPLAY_TTL_SECS (caps age since
        //                       IssueInstant to the dedup lifetime), and
        //   max_clock_skew   <= that (60s — ample for NTP-synced IdP/SP).
        .max_issue_delay(chrono::Duration::seconds(ASSERTION_REPLAY_TTL_SECS as i64))
        .max_clock_skew(chrono::Duration::seconds(SAML_MAX_CLOCK_SKEW_SECS))
        // Despite the flag name, IdP-initiated flows are NOT accepted:
        // the #82 gate below requires the assertion's
        // SubjectConfirmationData.InResponseTo to match a Redis-stored
        // AuthnRequest we minted at `/login`, and rejects any assertion
        // lacking it. We pass `allow_idp_initiated(true)` only to disable
        // samael's *own* request-tracking check — samael validates
        // InResponseTo against outstanding request IDs held on the SP
        // object, which we never populate (we track them in Redis instead),
        // so leaving its check enabled would reject every legitimate
        // SP-initiated assertion. SP-initiated-only enforcement and replay
        // bounding live in our explicit gates below (InResponseTo consume +
        // AudienceRestriction + assertion-replay dedup + `max_issue_delay`).
        .allow_idp_initiated(true)
        .build()
        .map_err(|e| {
            tracing::error!(error = ?e, "failed to build ServiceProvider in ACS");
            ApiError::Unauthorized
        })?;

    // Gap-001 (defense in depth): refuse to verify against an IdP
    // whose stored metadata has no signing cert. put_saml_config
    // already rejects cert-free metadata at upload time, but a
    // direct DDB write or a future schema change could bypass that
    // path. samael's verify path treats `idp_signing_certs() ==
    // Ok(None)` as "skip signature verification" — we must NOT
    // reach that branch. Catch it here and fail closed.
    match sp.idp_signing_certs() {
        Ok(Some(certs)) if !certs.is_empty() => {}
        _ => {
            tracing::warn!(
                workspace_id,
                "stored IdP metadata exposes no signing certs — refusing to skip signature verify"
            );
            return Err(ApiError::Unauthorized);
        }
    }

    // SHA-1 lockdown: decode the response once, refuse any
    // assertion whose XMLDSig declares a SHA-1 algorithm, then hand
    // the *same* decoded XML to samael's `parse_xml_response`. Using
    // one decoded bytes-of-truth across both steps closes the
    // parser-disagreement attack window where two base64 decoders
    // could see different bytes for the same wire input.
    //
    // xmlsec1's default-build accepts SHA-1 on verify — the Fedora
    // 43 dev-host crypto-policies block SHA-1 *signing* only, and
    // our Debian-bookworm production runtime doesn't even do that.
    // Application-layer rejection closes that exposure regardless
    // of which xmlsec build we run.
    let response_bytes = BASE64_STANDARD.decode(&form.saml_response).map_err(|e| {
        tracing::warn!(error = %e, workspace_id, "SAML response is not valid base64");
        ApiError::Unauthorized
    })?;
    let response_xml = std::str::from_utf8(&response_bytes).map_err(|e| {
        tracing::warn!(error = %e, workspace_id, "SAML response is not valid UTF-8");
        ApiError::Unauthorized
    })?;

    // Gap-002: refuse XML containing DTD constructs. samael's
    // signature-verify path runs through libxml2 with the default
    // `no_net: false` parser options, so a `<!DOCTYPE>` with an
    // external `<!ENTITY ... SYSTEM "http://attacker/">` would
    // trigger an outbound fetch during parsing — before signature
    // verification. From an ECS task that reaches IMDS, this is an
    // SSRF→credential-exfil chain. Real IdPs never emit DOCTYPE in
    // SAML responses (the binding spec recommends against it), so
    // rejecting outright is safe and complete.
    if let Some(construct) = find_dtd_construct(response_xml) {
        tracing::warn!(
            workspace_id,
            construct,
            "SAML response contains DTD construct — refusing to parse"
        );
        return Err(ApiError::Unauthorized);
    }

    if let Some(algorithm) = find_sha1_algorithm(response_xml) {
        tracing::warn!(
            workspace_id,
            algorithm,
            "SAML response uses SHA-1 algorithm — rejecting"
        );
        return Err(ApiError::Unauthorized);
    }

    let assertion = sp.parse_xml_response(response_xml, None).map_err(|e| {
        tracing::warn!(error = %e, workspace_id, "SAML response failed validation");
        ApiError::Unauthorized
    })?;

    // Explicit AudienceRestriction enforcement (#79). samael's
    // `validate_assertion` only enters the conditions block via
    // `if let Some(conditions)`, so an assertion that omits
    // `<Conditions>` entirely — or includes `<Conditions>` with no
    // `<AudienceRestriction>` — bypasses the audience check inside
    // the library. That means an assertion minted for a *different*
    // relying party at the same IdP could otherwise replay against
    // our ACS. Modern mainstream IdPs always emit
    // AudienceRestriction; minimalist or misconfigured IdPs may
    // not. Require it ourselves so the SP's invariant doesn't
    // depend on the IdP's configuration.
    let expected_audience = format!("{origin}/api/v1/auth/saml/metadata");
    if !assertion_has_matching_audience(
        assertion.conditions.as_ref(),
        &expected_audience,
    ) {
        tracing::warn!(
            workspace_id,
            assertion_id = %assertion.id,
            expected_audience = %expected_audience,
            "SAML assertion missing AudienceRestriction for this SP"
        );
        return Err(ApiError::Unauthorized);
    }

    // #82: CSRF protection via SP-initiated-only enforcement. Pull
    // `InResponseTo` from the assertion's SubjectConfirmationData
    // and consume the matching Redis row stored by `/login`. An
    // attacker who replays a stolen assertion has no corresponding
    // row (it was never minted by us, or was already consumed by
    // the legitimate flow) — `GETDEL` returns None, we reject.
    //
    // A captured assertion targeted at a *different* SP at the
    // same IdP also fails this gate: its InResponseTo (if present)
    // names a request we never issued. Combined with the
    // AudienceRestriction check just above, the two gates close
    // the cross-SP replay window referenced by #79 and the
    // assertion-replay window referenced by #82.
    let in_response_to = extract_in_response_to(&assertion);
    let Some(request_id) = in_response_to else {
        tracing::warn!(
            workspace_id,
            assertion_id = %assertion.id,
            "SAML assertion missing SubjectConfirmationData.InResponseTo \
             — only SP-initiated flows are accepted"
        );
        return Err(ApiError::Unauthorized);
    };
    let stored_workspace = state
        .redis_session
        .take_saml_authn_request(&request_id)
        .await
        .map_err(|e| {
            tracing::error!(
                error = %e,
                workspace_id,
                request_id = %request_id,
                "Redis lookup for pending AuthnRequest failed"
            );
            // Fail closed (matches the assertion-replay dedup
            // pattern): can't prove this isn't a replay, so don't
            // mint a session.
            ApiError::Unauthorized
        })?;
    let Some(stored_workspace) = stored_workspace else {
        tracing::warn!(
            workspace_id,
            request_id = %request_id,
            "SAML InResponseTo names no pending AuthnRequest \
             (expired, never issued, or already consumed)"
        );
        return Err(ApiError::Unauthorized);
    };
    if stored_workspace != workspace_id {
        // Cross-workspace replay: an attacker stole an assertion
        // for workspace X and is POSTing it with RelayState=Y. The
        // stored binding catches the mismatch.
        tracing::warn!(
            request_id = %request_id,
            workspace_id,
            stored_workspace = %stored_workspace,
            "SAML AuthnRequest workspace mismatch — RelayState lied"
        );
        return Err(ApiError::Unauthorized);
    }

    // Replay protection: SET NX EX on the assertion ID. samael
    // already enforces NotBefore / NotOnOrAfter timestamps inside
    // parse_base64_response (the ±90s `max_issue_delay` window),
    // but without dedup an attacker who captures a single
    // SAMLResponse can replay it from any origin within that
    // window. The Redis SET NX EX collapses the replay window to
    // exactly one consumed assertion.
    if assertion.id.trim().is_empty() {
        tracing::warn!(workspace_id, "SAML assertion missing ID — cannot dedupe");
        return Err(ApiError::Unauthorized);
    }
    let first_use = state
        .redis_session
        .try_mark_assertion_seen(&assertion.id, ASSERTION_REPLAY_TTL_SECS)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, workspace_id, "Redis replay-dedup write failed");
            // Fail closed: a transient Redis error during the
            // dedup write means we can't prove this isn't a
            // replay. Better to fail the legitimate user (retry
            // works) than to risk a replay slipping through.
            ApiError::Unauthorized
        })?;
    if !first_use {
        tracing::warn!(
            workspace_id,
            assertion_id = %assertion.id,
            "SAML assertion replay detected"
        );
        return Err(ApiError::Unauthorized);
    }

    let name_id = assertion
        .subject
        .as_ref()
        .and_then(|s| s.name_id.as_ref())
        .map(|n| n.value.clone())
        .unwrap_or_default();
    if name_id.trim().is_empty() {
        tracing::warn!(workspace_id, "SAML assertion missing Subject NameID");
        return Err(ApiError::Unauthorized);
    }
    // #80: the NameID is an identity key — it becomes the `external_id`
    // GSI key on JIT provisioning. Truncating it (as we do for display
    // fields below) could alias two distinct subjects, so an over-long
    // one is rejected outright rather than capped. The bound is far
    // above any real-world IdP NameID.
    if !name_id_within_cap(&name_id) {
        tracing::warn!(
            workspace_id,
            name_id_len = name_id.len(),
            "SAML assertion NameID exceeds cap; rejecting"
        );
        return Err(ApiError::Unauthorized);
    }

    let (email, name) = extract_email_and_name(
        &assertion,
        &config.attribute_email,
        &config.attribute_name,
    );
    let email = email.ok_or_else(|| {
        tracing::warn!(
            workspace_id,
            attribute = %config.attribute_email,
            "SAML assertion missing email attribute"
        );
        ApiError::Unauthorized
    })?;
    let name = name.unwrap_or_else(|| email.clone());

    // #80: cap IdP-supplied display fields before persistence, mirroring
    // the OAuth path's sanitize_profile (same MAX_EMAIL_LEN / MAX_NAME_LEN).
    // Unlike the NameID above these are not identity keys, so truncating
    // at a char boundary is safe and matches OAuth's behaviour.
    let email = super::auth::truncate_chars(email, super::auth::MAX_EMAIL_LEN);
    let name = super::auth::truncate_chars(name, super::auth::MAX_NAME_LEN);

    let mut user = ogrenotes_auth::user::find_or_create_saml_user(
        &state.user_repo,
        &state.folder_repo,
        &state.workspace_repo,
        &name_id,
        &email,
        &name,
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, workspace_id, "SAML JIT user creation failed");
        ApiError::Unauthorized
    })?;

    if user.is_disabled {
        tracing::info!(
            user_id = %user.user_id,
            workspace_id,
            "SAML login rejected: account disabled"
        );
        return Err(ApiError::Unauthorized);
    }

    // Admin-email auto-promotion runs on SAML too, same as the OAuth and
    // dev-login paths (shared helper; idempotent).
    crate::auth_policy::apply_admin_email_promotion(&state, &mut user).await;

    // Audit BEFORE minting the session — the row is the durable
    // record that this assertion was accepted, even if a downstream
    // failure aborts session-mint. record_security_event handles
    // the spawn + tracing + DDB write in one place (the same
    // helper the MFA path uses).
    record_security_event(
        &state,
        &user.user_id,
        SecurityAuditAction::SamlAssertionAccepted {
            workspace_id: workspace_id.clone(),
            name_id: name_id.clone(),
        },
    );

    // Gap-003: enforce SP-side MFA on SAML logins. The IdP's own
    // MFA (if any) is not a substitute for user-enrolled OgreNotes
    // MFA — those are two different second factors that the user
    // explicitly opted into. The shared helper handles the same
    // mint+store+redirect the OAuth path uses; SAML maps the Redis
    // failure to Unauthorized (fail closed on the IdP-driven flow)
    // rather than Internal (OAuth's choice).
    if user.mfa_enrolled_at.is_some() {
        return crate::routes::auth::redirect_to_mfa_challenge(&state, &user.user_id)
            .await
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    user_id = %user.user_id,
                    workspace_id,
                    "SAML: failed to store MFA pending state"
                );
                ApiError::Unauthorized
            });
    }

    crate::routes::auth::issue_session_response(&state, &user, SessionSource::Saml).await
}

/// XML DTD constructs we refuse outright. The presence of any of
/// these in a SAMLResponse is sufficient to reject — real IdPs do
/// not emit DTDs in SAML 2.0 responses (the W3C XML 1.0 spec
/// permits them but every SAML profile MUST NOT include them in
/// protocol messages). Allowing them is an XXE / SSRF gadget via
/// libxml2's default `no_net: false` parser options inside samael's
/// `reduce_xml_to_signed`.
///
/// XML 1.0 requires markup declarations to be uppercase, but
/// libxml2's recovery mode accepts lowercase forms; we list both
/// to foreclose that bypass. The `find_dtd_construct` function
/// also runs a paranoid `"<! "` prefix scan after the main check
/// to cover whitespace variants (`<! DOCTYPE`) which are invalid
/// XML but historically accepted by libxml2 — a known WAF bypass
/// pattern.
const REJECTED_DTD_CONSTRUCTS: &[&str] = &[
    "<!DOCTYPE", "<!doctype",
    "<!ENTITY", "<!entity",
];

fn find_dtd_construct(xml: &str) -> Option<&'static str> {
    if let Some(found) = REJECTED_DTD_CONSTRUCTS
        .iter()
        .copied()
        .find(|needle| xml.contains(*needle))
    {
        return Some(found);
    }
    // Paranoid catch-all: `<!` followed by any XML whitespace char
    // (S ::= #x20 | #x9 | #xD | #xA per XML 1.0 §2.3) is either a
    // libxml2-recovery DTD construct or malformed XML. Real SAML
    // responses do not contain `<!` at all, so this has no false-
    // positive cost. Byte-window scan because the needles are all
    // ASCII and tab/CR/LF must all be caught — a literal
    // `xml.contains("<! ")` would miss `<!\tDOCTYPE`, defeating the
    // entire point of the paranoid branch.
    let bytes = xml.as_bytes();
    for i in 0..bytes.len().saturating_sub(2) {
        if bytes[i] == b'<'
            && bytes[i + 1] == b'!'
            && matches!(bytes[i + 2], b' ' | b'\t' | b'\r' | b'\n')
        {
            return Some("<! ...");
        }
    }
    None
}

/// Scan raw response XML for a SHA-1 algorithm URN actually being
/// *used* as an XMLDSig `Algorithm` attribute value. Returns the
/// matching URN if found.
///
/// The match requires `Algorithm="…"` or `Algorithm='…'` around the
/// URN so the scan only fires on real declarations — not on the URN
/// appearing inside an XML comment, a `<saml:Attribute Name="…">`
/// value, or a namespace prefix declaration. Without this guard a
/// legitimate SHA-256 assertion that mentions a SHA-1 URN in a
/// comment would be falsely rejected.
fn find_sha1_algorithm(xml: &str) -> Option<&'static str> {
    REJECTED_SHA1_ALGORITHMS.iter().copied().find(|needle| {
        let dq = format!("Algorithm=\"{needle}\"");
        let sq = format!("Algorithm='{needle}'");
        xml.contains(&dq) || xml.contains(&sq)
    })
}

/// Walk the assertion's AttributeStatements for the configured
/// email + name attributes. Match by `Name` (case-sensitive — IdPs
/// publish canonical URIs); first non-empty value wins. Returns
/// `(email, name)` as Options so the caller can decide which
/// missing field is a hard error.
fn extract_email_and_name(
    assertion: &samael::schema::Assertion,
    email_attr_name: &str,
    name_attr_name: &str,
) -> (Option<String>, Option<String>) {
    let mut email = None;
    let mut name = None;
    if let Some(statements) = &assertion.attribute_statements {
        for stmt in statements {
            for attr in &stmt.attributes {
                let attr_name = attr.name.as_deref().unwrap_or("");
                let value = attr
                    .values
                    .iter()
                    .find_map(|v| v.value.clone().filter(|s| !s.trim().is_empty()));
                if attr_name == email_attr_name && email.is_none() {
                    email = value.clone();
                }
                if attr_name == name_attr_name && name.is_none() {
                    name = value;
                }
            }
        }
    }
    (email, name)
}

/// Pull the `InResponseTo` value off the assertion's
/// `SubjectConfirmationData` (#82). The SAML 2.0 spec puts
/// `InResponseTo` either on the outer `Response` (which samael's
/// `parse_xml_response` reads internally but doesn't expose to us)
/// or on the inner `SubjectConfirmationData`. Production IdPs
/// (Okta, Auth0, Azure AD, OneLogin, Google Workspace) all emit
/// it on the latter, which is what we read here. A None return
/// means either the assertion has no `Subject`, no
/// `subject_confirmations`, or no `InResponseTo` attribute on any
/// confirmation's data — all three shapes are rejected by the ACS
/// handler.
fn extract_in_response_to(assertion: &samael::schema::Assertion) -> Option<String> {
    assertion
        .subject
        .as_ref()?
        .subject_confirmations
        .as_ref()?
        .iter()
        .find_map(|c| {
            c.subject_confirmation_data
                .as_ref()
                .and_then(|d| d.in_response_to.clone())
        })
}

/// True iff `conditions` carries at least one `AudienceRestriction`
/// whose `Audience` list contains `expected_audience`. Pure helper
/// extracted from the ACS handler so the audience-enforcement rule
/// (#79) can be unit-tested without standing up a full
/// `Assertion`.
///
/// A missing `Conditions` block, a missing or empty
/// `audience_restrictions` vector, or restrictions whose audiences
/// don't include `expected_audience` all evaluate to false. The
/// caller treats false as a hard reject (401).
fn assertion_has_matching_audience(
    conditions: Option<&samael::schema::Conditions>,
    expected_audience: &str,
) -> bool {
    conditions
        .and_then(|c| c.audience_restrictions.as_ref())
        .map(|restrictions| {
            restrictions.iter().any(|r| {
                r.audience.iter().any(|a| a == expected_audience)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod audience_tests {
    use super::*;
    use samael::schema::{AudienceRestriction, Conditions};

    const SP_AUD: &str = "https://sp.example.com/api/v1/auth/saml/metadata";

    fn cond_with(audiences: Vec<Vec<&str>>) -> Conditions {
        Conditions {
            not_before: None,
            not_on_or_after: None,
            audience_restrictions: Some(
                audiences
                    .into_iter()
                    .map(|aud| AudienceRestriction {
                        audience: aud.into_iter().map(String::from).collect(),
                    })
                    .collect(),
            ),
            one_time_use: None,
            proxy_restriction: None,
        }
    }

    #[test]
    fn rejects_missing_conditions_block() {
        assert!(!assertion_has_matching_audience(None, SP_AUD));
    }

    #[test]
    fn rejects_conditions_with_no_audience_restrictions() {
        let c = Conditions {
            not_before: None,
            not_on_or_after: None,
            audience_restrictions: None,
            one_time_use: None,
            proxy_restriction: None,
        };
        assert!(!assertion_has_matching_audience(Some(&c), SP_AUD));
    }

    #[test]
    fn rejects_empty_audience_restrictions_vec() {
        let c = cond_with(vec![]);
        assert!(!assertion_has_matching_audience(Some(&c), SP_AUD));
    }

    #[test]
    fn rejects_restriction_targeting_another_sp() {
        // Cross-SP replay: assertion minted for a sibling SP at the
        // same IdP. Different `entity_id` → audience mismatch →
        // reject. This is the exact threat #79 calls out.
        let c = cond_with(vec![vec![
            "https://other-sp.example.com/api/v1/auth/saml/metadata",
        ]]);
        assert!(!assertion_has_matching_audience(Some(&c), SP_AUD));
    }

    #[test]
    fn accepts_when_one_restriction_matches() {
        let c = cond_with(vec![vec![SP_AUD]]);
        assert!(assertion_has_matching_audience(Some(&c), SP_AUD));
    }

    #[test]
    fn accepts_when_any_of_several_audiences_matches() {
        // A single AudienceRestriction may list multiple audiences;
        // a match on any one is sufficient. This is per SAML
        // Conditions semantics — the assertion is valid for any
        // listed audience.
        let c = cond_with(vec![vec![
            "https://other.example.com/saml",
            SP_AUD,
            "https://yet-another.example.com/saml",
        ]]);
        assert!(assertion_has_matching_audience(Some(&c), SP_AUD));
    }

    #[test]
    fn accepts_when_one_of_multiple_restrictions_matches() {
        // Multiple <AudienceRestriction> elements are AND-combined
        // in SAML semantics (each must contain a matching audience),
        // but for our SP-side check the question is "is *our* SP
        // named anywhere?" — so any restriction containing our
        // audience is sufficient. A future tightening to require
        // every restriction to name us is possible if the IdP shape
        // calls for it.
        let c = cond_with(vec![
            vec!["https://other.example.com/saml"],
            vec![SP_AUD],
        ]);
        assert!(assertion_has_matching_audience(Some(&c), SP_AUD));
    }

    // ─── #82: InResponseTo extraction ────────────────────────────

    use samael::schema::{Assertion, Subject, SubjectConfirmation, SubjectConfirmationData};

    fn empty_assertion() -> Assertion {
        Assertion {
            id: "test-assertion-id".to_string(),
            issue_instant: chrono::Utc::now(),
            version: "2.0".to_string(),
            issuer: samael::schema::Issuer::default(),
            signature: None,
            subject: None,
            conditions: None,
            authn_statements: None,
            attribute_statements: None,
        }
    }

    fn assertion_with_in_response_to(req_id: Option<&str>) -> Assertion {
        let mut a = empty_assertion();
        a.subject = Some(Subject {
            name_id: None,
            subject_confirmations: Some(vec![SubjectConfirmation {
                method: Some("urn:oasis:names:tc:SAML:2.0:cm:bearer".to_string()),
                name_id: None,
                subject_confirmation_data: Some(SubjectConfirmationData {
                    not_before: None,
                    not_on_or_after: None,
                    recipient: None,
                    in_response_to: req_id.map(String::from),
                    address: None,
                    content: None,
                }),
            }]),
        });
        a
    }

    #[test]
    fn extract_returns_none_when_subject_absent() {
        let a = empty_assertion();
        assert_eq!(extract_in_response_to(&a), None);
    }

    #[test]
    fn extract_returns_none_when_confirmations_empty() {
        let mut a = empty_assertion();
        a.subject = Some(Subject {
            name_id: None,
            subject_confirmations: Some(vec![]),
        });
        assert_eq!(extract_in_response_to(&a), None);
    }

    #[test]
    fn extract_returns_none_when_data_absent() {
        let mut a = empty_assertion();
        a.subject = Some(Subject {
            name_id: None,
            subject_confirmations: Some(vec![SubjectConfirmation {
                method: None,
                name_id: None,
                subject_confirmation_data: None,
            }]),
        });
        assert_eq!(extract_in_response_to(&a), None);
    }

    #[test]
    fn extract_returns_none_when_attribute_missing() {
        // SubjectConfirmationData present but InResponseTo attribute
        // omitted — IdP-initiated flow shape. We reject these.
        let a = assertion_with_in_response_to(None);
        assert_eq!(extract_in_response_to(&a), None);
    }

    #[test]
    fn extract_returns_value_when_present() {
        let a = assertion_with_in_response_to(Some("_id-abc123"));
        assert_eq!(extract_in_response_to(&a), Some("_id-abc123".to_string()));
    }

    #[test]
    fn extract_walks_multiple_confirmations_for_first_match() {
        // Bearer confirmation might appear after another; we find
        // the first one with an InResponseTo set.
        let mut a = empty_assertion();
        a.subject = Some(Subject {
            name_id: None,
            subject_confirmations: Some(vec![
                SubjectConfirmation {
                    method: Some("holder-of-key".to_string()),
                    name_id: None,
                    subject_confirmation_data: None,
                },
                SubjectConfirmation {
                    method: Some("bearer".to_string()),
                    name_id: None,
                    subject_confirmation_data: Some(SubjectConfirmationData {
                        not_before: None,
                        not_on_or_after: None,
                        recipient: None,
                        in_response_to: Some("_id-found".to_string()),
                        address: None,
                        content: None,
                    }),
                },
            ]),
        });
        assert_eq!(extract_in_response_to(&a), Some("_id-found".to_string()));
    }

    #[test]
    fn case_sensitive_audience_match() {
        // SAML audience comparison is case-sensitive per the spec
        // (entity_ids are URIs, and URI scheme + host are usually
        // case-insensitive but path is not). The simpler
        // case-sensitive check matches what samael does internally
        // and matches our entity_id construction in build_authn_request_xml.
        let c = cond_with(vec![vec![
            "HTTPS://SP.EXAMPLE.COM/API/V1/AUTH/SAML/METADATA",
        ]]);
        assert!(!assertion_has_matching_audience(Some(&c), SP_AUD));
    }
}
