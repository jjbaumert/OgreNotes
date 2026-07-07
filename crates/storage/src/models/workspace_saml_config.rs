// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Workspace-scoped SAML 2.0 IdP configuration (Phase 4 M-E4).
//!
//! One IdP per workspace in v1. Stored as a single DynamoDB row:
//!
//!   PK = `WORKSPACE#<workspace_id>`
//!   SK = `SAML_IDP`
//!
//! Storing the raw IdP metadata XML (rather than a decomposed set of
//! fields) means an IdP-side change (cert rotation, endpoint URL
//! update) only requires the admin to re-upload one document —
//! everything else parses out of the XML at SAML-handler time. The
//! attribute-name mappings stay separate because they're our
//! product's choice of which SAML attribute statement to read for
//! email/name, not something that lives in the IdP metadata.

use serde::{Deserialize, Serialize};

/// Maximum bytes the admin can upload as `idp_metadata_xml`.
/// 64 KB is well above any realistic SAML metadata document (5-20 KB
/// typical) and well below DynamoDB's 400 KB per-item limit, leaving
/// headroom for the rest of the row.
pub const MAX_METADATA_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSamlConfig {
    pub workspace_id: String,
    /// The IdP's SAML 2.0 entity ID (usually a URI). The SAML
    /// handler validates the `Issuer` element of incoming responses
    /// against this exact string.
    pub idp_entity_id: String,
    /// Full IdP metadata XML as the admin uploaded it. The SAML
    /// handler parses this at request time to extract the signing
    /// certificate(s) and SSO endpoint URL. Stored as-is so future
    /// cert rotation on the IdP side surfaces by the admin
    /// re-uploading exactly one document.
    pub idp_metadata_xml: String,
    /// Name of the SAML AttributeStatement attribute that carries
    /// the user's email. Usually `"email"` for IdPs that publish a
    /// simple schema or
    /// `"http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress"`
    /// for AD FS / Azure AD.
    pub attribute_email: String,
    /// Name of the SAML AttributeStatement attribute that carries
    /// the user's display name. Default is `"name"`.
    pub attribute_name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl WorkspaceSamlConfig {
    pub fn pk(&self) -> String {
        format!("WORKSPACE#{}", self.workspace_id)
    }

    pub fn sk() -> &'static str {
        "SAML_IDP"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> WorkspaceSamlConfig {
        WorkspaceSamlConfig {
            workspace_id: "ws-1".to_string(),
            idp_entity_id: "https://idp.example.com/metadata".to_string(),
            idp_metadata_xml: "<EntityDescriptor/>".to_string(),
            attribute_email: "email".to_string(),
            attribute_name: "name".to_string(),
            created_at: 1_700_000_000_000_000,
            updated_at: 1_700_000_000_000_000,
        }
    }

    #[test]
    fn pk_sk_format() {
        let c = fixture();
        assert_eq!(c.pk(), "WORKSPACE#ws-1");
        assert_eq!(WorkspaceSamlConfig::sk(), "SAML_IDP");
    }

    #[test]
    fn json_roundtrip() {
        let c = fixture();
        let json = serde_json::to_string(&c).unwrap();
        let back: WorkspaceSamlConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn max_metadata_bytes_is_at_least_typical_size() {
        // Typical SAML metadata is 5-20 KB; our cap is 64 KB. If
        // somebody bumps the cap down this test fires before a real
        // IdP's metadata gets rejected at upload time.
        assert!(MAX_METADATA_BYTES >= 20 * 1024);
    }
}
