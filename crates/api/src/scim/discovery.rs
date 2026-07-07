// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! SCIM 2.0 discovery responses (Phase 4 M-E5 piece E):
//!
//!   GET /ServiceProviderConfig  — RFC 7643 §5
//!   GET /ResourceTypes          — RFC 7643 §6
//!   GET /Schemas                — RFC 7643 §7
//!
//! The shapes are mostly static — they advertise what SCIM features
//! the SP supports — so the bodies are built as `serde_json::Value`
//! literals rather than full DTO structs. That keeps the file
//! review-able: every advertised capability is one place to read.
//!
//! Each function takes a `base_url` so the embedded `meta.location`
//! and `endpoint` fields can resolve to absolute URLs. The base URL
//! is `"{frontend_origin}/api/v1/scim/v2/workspaces/{ws_id}"`.

use serde_json::{json, Value};

use crate::scim::dtos::{
    SCHEMA_GROUP, SCHEMA_RESOURCE_TYPE, SCHEMA_SCHEMA, SCHEMA_SERVICE_PROVIDER_CONFIG,
    SCHEMA_USER,
};

/// RFC 7643 §5 — describes what SCIM features this SP supports.
/// Every flag is the boolean we want IdPs to see, NOT what SCIM
/// optionally supports. Real consequences:
///   - `patch.supported=true`   — Okta + Entra will use PATCH for
///     deprovision and member-sync (the small subset piece D
///     supports).
///   - `bulk.supported=false`   — provisioners fall back to one
///     request per resource. Honest signal; SCIM bulk is rarely
///     used and we don't implement it.
///   - `filter.supported=true, maxResults=200` — piece D's filter
///     parser supports `userName eq` / `externalId eq`; maxResults
///     matches the MAX_COUNT cap in routes::scim.
///   - `changePassword.supported=false` — we never store
///     passwords (SAML SSO is the auth path).
///   - `sort.supported=false`   — piece D doesn't sort. Listed as
///     false so IdPs don't send sortBy and silently get unsorted
///     pages.
///   - `etag.supported=false`   — no version field in piece D's
///     User mapping.
pub fn service_provider_config(_base_url: &str) -> Value {
    json!({
        "schemas": [SCHEMA_SERVICE_PROVIDER_CONFIG],
        "documentationUri": "https://github.com/jjbaumert/OgreNotes",
        "patch": { "supported": true },
        "bulk": {
            "supported": false,
            "maxOperations": 0,
            "maxPayloadSize": 0,
        },
        "filter": { "supported": true, "maxResults": 200 },
        "changePassword": { "supported": false },
        "sort": { "supported": false },
        "etag": { "supported": false },
        // RFC requires this even though we never accept anything
        // but Bearer — IdPs that strictly parse the schema reject
        // an empty array.
        "authenticationSchemes": [{
            "type": "oauthbearertoken",
            "name": "OAuth Bearer Token",
            "description":
                "Authentication via a workspace-scoped SCIM bearer token \
                 (format: `<token_id>.<secret>`).",
            "specUri": "https://www.rfc-editor.org/info/rfc6750",
            "primary": true,
        }],
    })
}

/// RFC 7643 §6 — describes the resource types this SP exposes.
/// Returned as a `ListResponse` envelope per the spec's "the
/// `/ResourceTypes` endpoint returns a list" semantics.
pub fn resource_types(base_url: &str) -> Value {
    let user = json!({
        "schemas": [SCHEMA_RESOURCE_TYPE],
        "id": "User",
        "name": "User",
        "endpoint": "/Users",
        "description": "Workspace member account",
        "schema": SCHEMA_USER,
        "meta": {
            "resourceType": "ResourceType",
            "location": format!("{base_url}/ResourceTypes/User"),
        },
    });
    let group = json!({
        "schemas": [SCHEMA_RESOURCE_TYPE],
        "id": "Group",
        "name": "Group",
        "endpoint": "/Groups",
        "description": "Workspace (one Group per workspace in v1)",
        "schema": SCHEMA_GROUP,
        "meta": {
            "resourceType": "ResourceType",
            "location": format!("{base_url}/ResourceTypes/Group"),
        },
    });
    json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:ListResponse"],
        "totalResults": 2,
        "startIndex": 1,
        "itemsPerPage": 2,
        "Resources": [user, group],
    })
}

/// RFC 7643 §7 — describes the attributes available on each
/// resource. Minimal: we declare only the fields piece D's User
/// mapper emits and piece E's Group mapper emits. A full schema
/// expansion is a future polish item; the IdPs we care about read
/// this for sanity-check only.
pub fn schemas(base_url: &str) -> Value {
    let user_schema = json!({
        "schemas": [SCHEMA_SCHEMA],
        "id": SCHEMA_USER,
        "name": "User",
        "description": "SCIM core User resource (RFC 7643 §4.1)",
        "attributes": [
            attr("userName",   "string", true,  "server"),
            attr("externalId", "string", false, "readWrite"),
            attr("displayName","string", false, "readWrite"),
            attr("active",     "boolean", false,"readWrite"),
            // Complex `name` attribute — describe via subAttributes.
            json!({
                "name": "name",
                "type": "complex",
                "multiValued": false,
                "required": false,
                "mutability": "readWrite",
                "subAttributes": [
                    attr("formatted",   "string", false, "readWrite"),
                    attr("familyName",  "string", false, "readWrite"),
                    attr("givenName",   "string", false, "readWrite"),
                ],
            }),
            // emails: multi-valued complex.
            json!({
                "name": "emails",
                "type": "complex",
                "multiValued": true,
                "required": false,
                "mutability": "readWrite",
                "subAttributes": [
                    attr("value",   "string",  false, "readWrite"),
                    attr("type",    "string",  false, "readWrite"),
                    attr("primary", "boolean", false, "readWrite"),
                ],
            }),
        ],
        "meta": {
            "resourceType": "Schema",
            "location": format!("{base_url}/Schemas/{SCHEMA_USER}"),
        },
    });
    let group_schema = json!({
        "schemas": [SCHEMA_SCHEMA],
        "id": SCHEMA_GROUP,
        "name": "Group",
        "description": "SCIM core Group resource — workspace in v1 (RFC 7643 §4.2)",
        "attributes": [
            attr("displayName", "string", true, "readOnly"),
            json!({
                "name": "members",
                "type": "complex",
                "multiValued": true,
                "required": false,
                "mutability": "readWrite",
                "subAttributes": [
                    attr("value",   "string", true,  "readWrite"),
                    attr("display", "string", false, "readOnly"),
                    attr("type",    "string", false, "readOnly"),
                ],
            }),
        ],
        "meta": {
            "resourceType": "Schema",
            "location": format!("{base_url}/Schemas/{SCHEMA_GROUP}"),
        },
    });
    json!({
        "schemas": ["urn:ietf:params:scim:api:messages:2.0:ListResponse"],
        "totalResults": 2,
        "startIndex": 1,
        "itemsPerPage": 2,
        "Resources": [user_schema, group_schema],
    })
}

/// Build a flat attribute descriptor. Lifted out because every
/// attribute except complex ones uses the same shape.
fn attr(name: &str, type_: &str, required: bool, mutability: &str) -> Value {
    json!({
        "name": name,
        "type": type_,
        "multiValued": false,
        "required": required,
        "mutability": mutability,
        "returned": "default",
        "caseExact": false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_provider_config_advertises_correct_features() {
        let v = service_provider_config("https://x.test/scim/v2/workspaces/ws-1");
        assert_eq!(
            v["schemas"][0].as_str().unwrap(),
            SCHEMA_SERVICE_PROVIDER_CONFIG
        );
        assert!(v["patch"]["supported"].as_bool().unwrap());
        assert!(!v["bulk"]["supported"].as_bool().unwrap());
        assert!(v["filter"]["supported"].as_bool().unwrap());
        assert!(!v["changePassword"]["supported"].as_bool().unwrap());
        assert!(!v["sort"]["supported"].as_bool().unwrap());
        // RFC requires a non-empty authenticationSchemes list. Some
        // IdPs reject the doc otherwise — regression guard.
        assert!(v["authenticationSchemes"][0]["type"].is_string());
    }

    #[test]
    fn resource_types_lists_user_and_group() {
        let v = resource_types("https://x.test/scim/v2/workspaces/ws-1");
        assert_eq!(v["totalResults"].as_u64().unwrap(), 2);
        let resources = v["Resources"].as_array().unwrap();
        assert_eq!(resources.len(), 2);
        let ids: Vec<&str> = resources
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"User"));
        assert!(ids.contains(&"Group"));
        // URN regression guard: a typo in a schemas constant would
        // silently serialize a malformed body that strict-validating
        // IdPs would reject. Anchor each resource's `schemas[0]` to
        // the canonical constant.
        for r in resources {
            assert_eq!(r["schemas"][0].as_str().unwrap(), SCHEMA_RESOURCE_TYPE);
        }
        // The `schema` field on a ResourceType points at the
        // RFC-mandated URN for User / Group.
        let by_id = |id: &str| {
            resources
                .iter()
                .find(|r| r["id"].as_str() == Some(id))
                .unwrap()
        };
        assert_eq!(by_id("User")["schema"].as_str().unwrap(), SCHEMA_USER);
        assert_eq!(by_id("Group")["schema"].as_str().unwrap(), SCHEMA_GROUP);
    }

    #[test]
    fn schemas_includes_user_required_username() {
        let v = schemas("https://x.test/scim/v2/workspaces/ws-1");
        let resources = v["Resources"].as_array().unwrap();
        let user_schema = resources
            .iter()
            .find(|r| r["id"].as_str().unwrap() == SCHEMA_USER)
            .unwrap();
        let user_name_attr = user_schema["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"].as_str() == Some("userName"))
            .unwrap();
        assert!(user_name_attr["required"].as_bool().unwrap());
        // URN regression guard — see resource_types_lists_user_and_group.
        for r in resources {
            assert_eq!(r["schemas"][0].as_str().unwrap(), SCHEMA_SCHEMA);
        }
    }
}
