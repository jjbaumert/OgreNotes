// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! SCIM 2.0 wire-shape DTOs (Phase 4 M-E5 piece C).
//!
//! Match RFC 7643 (Core Schema) and RFC 7644 (Protocol) exactly.
//! Names are camelCase per the spec; `schemas` is `Vec<String>` of
//! URNs and is required on every resource. Optional fields use
//! `#[serde(skip_serializing_if = "Option::is_none")]` so the
//! server never emits null-valued keys an IdP might choke on
//! (some IdPs are picky about absent vs explicit-null).
//!
//! Why one struct per resource (rather than separate request /
//! response types):
//!   - SCIM clients send partial bodies (POST with just userName,
//!     PATCH with single operations). Optional fields everywhere
//!     covers both shapes.
//!   - Round-tripping a resource through serde stays bit-identical
//!     to what the client sent — important for the `meta` echo
//!     pattern.

use serde::{Deserialize, Serialize};

// ─── URN constants ──────────────────────────────────────────────

/// Schema URN for the core User resource (RFC 7643 §4.1).
pub const SCHEMA_USER: &str = "urn:ietf:params:scim:schemas:core:2.0:User";

/// Schema URN for the core Group resource (RFC 7643 §4.2).
pub const SCHEMA_GROUP: &str = "urn:ietf:params:scim:schemas:core:2.0:Group";

/// Schema URN for a SCIM error response (RFC 7644 §3.12).
pub const SCHEMA_ERROR: &str = "urn:ietf:params:scim:api:messages:2.0:Error";

/// Schema URN for the ListResponse envelope (RFC 7644 §3.4.2).
pub const SCHEMA_LIST_RESPONSE: &str =
    "urn:ietf:params:scim:api:messages:2.0:ListResponse";

/// Schema URN for the PatchOp request (RFC 7644 §3.5.2).
pub const SCHEMA_PATCH_OP: &str = "urn:ietf:params:scim:api:messages:2.0:PatchOp";

/// Schema URN for ServiceProviderConfig (RFC 7643 §5).
pub const SCHEMA_SERVICE_PROVIDER_CONFIG: &str =
    "urn:ietf:params:scim:schemas:core:2.0:ServiceProviderConfig";

/// Schema URN for the ResourceType discovery resource (RFC 7643 §6).
pub const SCHEMA_RESOURCE_TYPE: &str =
    "urn:ietf:params:scim:schemas:core:2.0:ResourceType";

/// Schema URN for a Schema discovery resource (RFC 7643 §7).
pub const SCHEMA_SCHEMA: &str = "urn:ietf:params:scim:schemas:core:2.0:Schema";

// ─── Meta sub-resource ──────────────────────────────────────────

/// SCIM `meta` complex attribute. Returned on every resource the
/// server emits; clients MAY omit on request and the server fills
/// it in. RFC 7643 §3.1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// `User` or `Group`. The single-word resource type, NOT a URN.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_type: Option<String>,
    /// RFC 3339 timestamp. Server sets on POST.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    /// RFC 3339 timestamp. Server updates on every mutation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// Canonical URL for this resource. Server fills in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Opaque version tag. We don't support ETags in v1 but the
    /// field stays present so a future addition is a one-spot
    /// change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

// ─── User resource ──────────────────────────────────────────────

/// Complex `name` sub-attribute on User. RFC 7643 §4.1.1.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserName {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formatted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub given_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub middle_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub honorific_prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub honorific_suffix: Option<String>,
}

/// Multi-valued attribute: emails, phone numbers, etc. RFC 7643
/// §2.4. SCIM convention is `value` for the address, `type` for
/// the category ("work" / "home" / etc.), `primary` for the default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MultiValue {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// RFC 7643 reserves `type` as a SCIM keyword; serde rename
    /// keeps the wire name correct while letting Rust use a sane
    /// field identifier.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

/// SCIM User resource (RFC 7643 §4.1). The required core attribute
/// is `userName`; everything else is optional per spec. Server
/// always emits `schemas` and `id`; client may omit both on POST.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScimUser {
    /// Server-assigned. `Option` so POST request bodies (which omit
    /// id) deserialize cleanly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// IdP-side correlation key. The JIT path uses this as the
    /// `User.external_id` GSI key (same convention as the SAML
    /// NameID).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// REQUIRED. RFC 7643 §4.1.1 says "MUST be unique" — we enforce
    /// uniqueness in the JIT handler.
    pub user_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<UserName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub emails: Vec<MultiValue>,
    /// Provisioning toggle. `false` = deprovision. RFC 7643 §4.1.1.
    /// Defaults to true on POST when omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    /// REQUIRED per RFC 7644 §3.1. Server emits `[SCHEMA_USER]`.
    /// On request, the client MAY include but is not required to.
    #[serde(default)]
    pub schemas: Vec<String>,
}

impl ScimUser {
    /// Construct an empty user with only `schemas` and `user_name`
    /// set — used by the JIT handler to fill in from a User row.
    pub fn new_with_user_name(user_name: impl Into<String>) -> Self {
        Self {
            id: None,
            external_id: None,
            user_name: user_name.into(),
            name: None,
            display_name: None,
            emails: Vec::new(),
            active: None,
            meta: None,
            schemas: vec![SCHEMA_USER.to_string()],
        }
    }
}

// ─── Group resource ─────────────────────────────────────────────

/// SCIM Group member (RFC 7643 §4.2). One entry per workspace
/// member; `value` is the User `id`, `display` is the user-friendly
/// label for log output.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMember {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    /// `User` always in our case (we don't support nested groups).
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    /// Canonical URL of the member User. Server fills in.
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub ref_: Option<String>,
}

/// SCIM Group resource (RFC 7643 §4.2). We map a workspace to a
/// Group — the workspace_id is the group `id`, and members are the
/// workspace's member list. PATCH on a Group adds/removes
/// workspace members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScimGroup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    /// REQUIRED. RFC 7643 §4.2.
    pub display_name: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub members: Vec<GroupMember>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
    #[serde(default)]
    pub schemas: Vec<String>,
}

// ─── ListResponse envelope ──────────────────────────────────────

/// RFC 7644 §3.4.2 — paginated list envelope for `GET /Users`,
/// `GET /Groups`, and any filtered query. Generic over the
/// `Resources` element type so the same struct serves both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResponse<T: Serialize> {
    /// Always `[SCHEMA_LIST_RESPONSE]`. RFC required.
    pub schemas: Vec<String>,
    /// Total matching resources across all pages, not just this
    /// page's count.
    pub total_results: usize,
    /// 1-based offset of the first item in `resources`. SCIM is
    /// 1-based; serde stays bit-identical.
    pub start_index: usize,
    /// Number of items actually returned. May be < requested count
    /// if fewer remain.
    pub items_per_page: usize,
    #[serde(rename = "Resources")]
    pub resources: Vec<T>,
}

impl<T: Serialize> ListResponse<T> {
    /// Build a list envelope. `start_index` is 1-based; SCIM's
    /// convention not the developer's.
    pub fn new(resources: Vec<T>, total_results: usize, start_index: usize) -> Self {
        let items_per_page = resources.len();
        Self {
            schemas: vec![SCHEMA_LIST_RESPONSE.to_string()],
            total_results,
            start_index,
            items_per_page,
            resources,
        }
    }
}

// ─── PatchOp (RFC 7644 §3.5.2) ─────────────────────────────────

/// The three legal `op` values in a SCIM PATCH operation. RFC
/// 7644 §3.5.2 says the wire value is case-insensitive — Okta
/// emits `"replace"`, Entra emits `"Replace"`, JumpCloud emits
/// `"REPLACE"`. Normalizing at deserialize time means the route
/// handler in piece D matches on a typed variant instead of
/// remembering to call `.eq_ignore_ascii_case()` at every site.
/// Closes the stringly-typed footgun before piece D is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchVerb {
    Add,
    Replace,
    Remove,
}

impl<'de> serde::Deserialize<'de> for PatchVerb {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.to_ascii_lowercase().as_str() {
            "add" => Ok(PatchVerb::Add),
            "replace" => Ok(PatchVerb::Replace),
            "remove" => Ok(PatchVerb::Remove),
            other => Err(serde::de::Error::custom(format!(
                "unknown SCIM patch op {other:?}; expected add / replace / remove"
            ))),
        }
    }
}

/// One operation inside a PatchOp request. RFC 7644 §3.5.2. `path`
/// uses SCIM's attribute-path syntax (`"emails[type eq \"work\"].value"`,
/// `"members"`, etc.); v1 implements the small subset Okta and
/// Entra emit: `userName`, `active`, `name.givenName`,
/// `name.familyName`, `displayName`, `emails`, `members`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchOperation {
    /// Case-insensitive per RFC; normalized to a typed variant at
    /// deserialize time.
    pub op: PatchVerb,
    /// Optional for `add` / `replace` when the value is a complete
    /// resource; required for `remove`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Untyped per RFC — can be a string, bool, object, or array.
    /// `serde_json::Value` is the right escape hatch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// RFC 7644 §3.5.2 — request body of `PATCH /Users/{id}` or
/// `PATCH /Groups/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchOp {
    pub schemas: Vec<String>,
    #[serde(rename = "Operations")]
    pub operations: Vec<PatchOperation>,
}

// ─── Error response ────────────────────────────────────────────

/// RFC 7644 §3.12 — error response body. Status code is the HTTP
/// status (also stringified here per the spec — yes, SCIM uses a
/// string `status` field in the body that mirrors the response
/// status line). `scimType` is the structured SCIM error code; the
/// closed set is in §3.12 — we use a `&'static str` constant set
/// rather than an enum to keep the wire shape obvious.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScimError {
    pub schemas: Vec<String>,
    /// Stringified HTTP status: `"400"`, `"404"`, etc.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scim_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ScimError {
    /// Build a SCIM error response from a status, optional
    /// scim_type code, and a detail message. The schemas vec is
    /// filled in for the caller.
    pub fn new(
        status: u16,
        scim_type: Option<&str>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            schemas: vec![SCHEMA_ERROR.to_string()],
            status: status.to_string(),
            scim_type: scim_type.map(str::to_string),
            detail: Some(detail.into()),
        }
    }
}

/// RFC 7644 §3.12 `scimType` values — the closed set of structured
/// error codes a SCIM server can return. We expose them as
/// constants because piece D's route handlers need to surface a few
/// of them precisely (e.g., `uniqueness` for duplicate userName).
pub mod scim_type {
    pub const INVALID_FILTER: &str = "invalidFilter";
    pub const TOO_MANY: &str = "tooMany";
    pub const UNIQUENESS: &str = "uniqueness";
    pub const MUTABILITY: &str = "mutability";
    pub const INVALID_SYNTAX: &str = "invalidSyntax";
    pub const INVALID_PATH: &str = "invalidPath";
    pub const NO_TARGET: &str = "noTarget";
    pub const INVALID_VALUE: &str = "invalidValue";
    pub const INVALID_VERS: &str = "invalidVers";
    pub const SENSITIVE: &str = "sensitive";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_serializes_with_required_schemas() {
        // SCIM clients reject responses missing the `schemas` field.
        // Regression guard against accidentally dropping the
        // serde-default.
        let user = ScimUser::new_with_user_name("alice@example.com");
        let json = serde_json::to_value(&user).unwrap();
        assert_eq!(
            json["schemas"][0].as_str().unwrap(),
            SCHEMA_USER,
        );
        assert_eq!(json["userName"].as_str().unwrap(), "alice@example.com");
        // Optional fields with None must NOT serialize as null —
        // some IdPs (notably Entra) fail validation on null values.
        assert!(json.get("name").is_none());
        assert!(json.get("displayName").is_none());
    }

    #[test]
    fn user_round_trips_post_request_shape() {
        // Real Okta POST /Users body — minimal valid shape.
        let body = r#"{
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:User"],
            "userName": "alice@example.com",
            "name": { "givenName": "Alice", "familyName": "Smith" },
            "active": true,
            "emails": [{ "value": "alice@example.com", "type": "work", "primary": true }]
        }"#;
        let user: ScimUser = serde_json::from_str(body).unwrap();
        assert_eq!(user.user_name, "alice@example.com");
        assert_eq!(user.name.as_ref().unwrap().given_name.as_deref(), Some("Alice"));
        assert_eq!(user.active, Some(true));
        assert_eq!(user.emails[0].value.as_deref(), Some("alice@example.com"));
        assert_eq!(user.emails[0].type_.as_deref(), Some("work"));
    }

    #[test]
    fn user_omits_none_fields_to_avoid_idp_null_pickiness() {
        // Entra ID and some other IdPs fail validation if the
        // server emits null-valued attributes. skip_serializing_if
        // must remove None entirely, not produce null.
        let user = ScimUser::new_with_user_name("bob");
        let json = serde_json::to_string(&user).unwrap();
        assert!(!json.contains("null"), "no null values: {json}");
        assert!(!json.contains("\"id\""), "id absent when None");
    }

    #[test]
    fn group_round_trips_member_refs() {
        let body = r#"{
            "schemas": ["urn:ietf:params:scim:schemas:core:2.0:Group"],
            "displayName": "Engineering",
            "members": [
                { "value": "user-123", "display": "Alice", "type": "User" }
            ]
        }"#;
        let group: ScimGroup = serde_json::from_str(body).unwrap();
        assert_eq!(group.display_name, "Engineering");
        assert_eq!(group.members.len(), 1);
        assert_eq!(group.members[0].value, "user-123");
        assert_eq!(group.members[0].type_.as_deref(), Some("User"));
    }

    #[test]
    fn list_response_wire_shape_matches_rfc() {
        // RFC 7644 §3.4.2 — the response keys are exactly these,
        // with `Resources` capital-R. Many IdPs case-match.
        let envelope = ListResponse::new(
            vec![ScimUser::new_with_user_name("a")],
            42,
            1,
        );
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["schemas"][0].as_str().unwrap(), SCHEMA_LIST_RESPONSE);
        assert_eq!(json["totalResults"].as_u64().unwrap(), 42);
        assert_eq!(json["startIndex"].as_u64().unwrap(), 1);
        assert_eq!(json["itemsPerPage"].as_u64().unwrap(), 1);
        // The capital-R is the RFC-mandated key.
        assert!(json.get("Resources").is_some());
        assert!(json.get("resources").is_none());
    }

    #[test]
    fn patch_op_round_trips_okta_replace_active() {
        // Real Okta deprovision request — sets active=false.
        let body = r#"{
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [
                { "op": "replace", "value": { "active": false } }
            ]
        }"#;
        let patch: PatchOp = serde_json::from_str(body).unwrap();
        assert_eq!(patch.operations.len(), 1);
        assert_eq!(patch.operations[0].op, PatchVerb::Replace);
        assert_eq!(
            patch.operations[0].value.as_ref().unwrap()["active"],
            serde_json::Value::Bool(false),
        );
    }

    #[test]
    fn patch_op_round_trips_path_targeted_op() {
        // Real Entra ID PATCH — replace a single attribute by path.
        // Entra capitalizes the op (`"Replace"`); the case-
        // insensitive Deserialize normalizes to PatchVerb::Replace.
        let body = r#"{
            "schemas": ["urn:ietf:params:scim:api:messages:2.0:PatchOp"],
            "Operations": [
                { "op": "Replace", "path": "name.givenName", "value": "NewName" }
            ]
        }"#;
        let patch: PatchOp = serde_json::from_str(body).unwrap();
        assert_eq!(patch.operations[0].path.as_deref(), Some("name.givenName"));
        assert_eq!(patch.operations[0].op, PatchVerb::Replace);
    }

    #[test]
    fn patch_verb_accepts_all_case_variants() {
        // RFC 7644 §3.5.2 — `op` is case-insensitive on the wire.
        // Real IdPs disagree on capitalization. Normalize at parse.
        for raw in ["add", "Add", "ADD", "aDd"] {
            let v: PatchVerb =
                serde_json::from_str(&format!("\"{raw}\"")).unwrap();
            assert_eq!(v, PatchVerb::Add, "for raw={raw}");
        }
        for raw in ["replace", "Replace", "REPLACE"] {
            let v: PatchVerb = serde_json::from_str(&format!("\"{raw}\"")).unwrap();
            assert_eq!(v, PatchVerb::Replace);
        }
        for raw in ["remove", "Remove"] {
            let v: PatchVerb = serde_json::from_str(&format!("\"{raw}\"")).unwrap();
            assert_eq!(v, PatchVerb::Remove);
        }
    }

    #[test]
    fn patch_verb_rejects_unknown_op() {
        // A typo or made-up verb must fail deserialize loudly,
        // not coerce into one of the known variants.
        let err = serde_json::from_str::<PatchVerb>("\"upsert\"").unwrap_err();
        assert!(err.to_string().contains("unknown SCIM patch op"));
    }

    #[test]
    fn error_response_has_string_status_per_spec() {
        // RFC 7644 §3.12 — status is a STRING, not a number, in the
        // body. Surprising but normative.
        let err = ScimError::new(400, Some(scim_type::INVALID_FILTER), "bad filter");
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["status"].as_str().unwrap(), "400");
        assert!(json["status"].as_i64().is_none(), "status MUST NOT be a number");
        assert_eq!(json["scimType"].as_str().unwrap(), "invalidFilter");
        assert_eq!(json["schemas"][0].as_str().unwrap(), SCHEMA_ERROR);
    }
}
