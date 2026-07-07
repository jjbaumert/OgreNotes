// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Mapping between the internal `User` storage model and the SCIM
//! `ScimUser` wire DTO (Phase 4 M-E5 piece D).
//!
//! Lives in the `scim` module — these are L4 Edge concerns: how
//! the protocol's wire shape relates to our domain. The domain
//! model (User) is unchanged.

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::user::User;

use crate::scim::dtos::{Meta, MultiValue, ScimUser, UserName, SCHEMA_USER};

/// Convert an internal `User` row into the SCIM wire shape returned
/// by GET/POST/PUT/PATCH /Users handlers.
///
/// Defaults applied at the wire boundary:
///   - `id` = `user.user_id`
///   - `userName` = `user.email` (SCIM's logical login identifier)
///   - `emails` = single primary work email
///   - `active` = !is_disabled
///   - `meta.location` = canonical resource URL (caller-supplied
///     because this module doesn't know the workspace_id or origin)
pub fn user_to_scim(user: &User, location: Option<String>) -> ScimUser {
    let name_field = parse_display_name(&user.name);
    let resource_type = "User".to_string();
    let now = format_rfc3339_now();
    ScimUser {
        id: Some(user.user_id.clone()),
        external_id: user.external_id.clone(),
        user_name: user.email.clone(),
        name: Some(name_field),
        display_name: Some(user.name.clone()),
        emails: vec![MultiValue {
            value: Some(user.email.clone()),
            type_: Some("work".to_string()),
            primary: Some(true),
            display: None,
        }],
        active: Some(!user.is_disabled),
        meta: Some(Meta {
            resource_type: Some(resource_type),
            created: Some(now.clone()),
            last_modified: Some(now),
            location,
            version: None,
        }),
        schemas: vec![SCHEMA_USER.to_string()],
    }
}

/// Split a single-string display name into SCIM's structured
/// UserName. Our domain stores the whole name; SCIM clients expect
/// `givenName` + `familyName`. Convention: first whitespace-
/// delimited token is `givenName`, remainder is `familyName`. Lossy
/// for names that don't fit this shape, but matches what Okta does
/// when round-tripping a single-field name.
fn parse_display_name(name: &str) -> UserName {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return UserName {
            formatted: None,
            ..Default::default()
        };
    }
    let (given, family) = match trimmed.split_once(char::is_whitespace) {
        Some((g, rest)) => {
            let f = rest.trim();
            (
                Some(g.to_string()),
                if f.is_empty() {
                    None
                } else {
                    Some(f.to_string())
                },
            )
        }
        None => (Some(trimmed.to_string()), None),
    };
    UserName {
        formatted: Some(trimmed.to_string()),
        family_name: family,
        given_name: given,
        ..Default::default()
    }
}

/// Format `now_usec()` as RFC 3339 for SCIM's `meta.created` /
/// `meta.lastModified`. SCIM expects a string like
/// `"2026-05-14T12:34:56Z"`.
fn format_rfc3339_now() -> String {
    let usec = now_usec();
    let secs = usec / 1_000_000;
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .unwrap_or_else(chrono::Utc::now);
    dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_storage::models::user::{AuthProvider, UserRole};

    fn fixture_user() -> User {
        User {
            user_id: "u-1".to_string(),
            name: "Alice Wonderland".to_string(),
            email: "alice@example.com".to_string(),
            avatar_url: None,
            provider: AuthProvider::Saml,
            provider_subject_id: None,
            home_folder_id: "f-home".to_string(),
            private_folder_id: "f-priv".to_string(),
            trash_folder_id: "f-trash".to_string(),
            archive_folder_id: None,
            pinned_folder_id: None,
            default_workspace_id: None,
            mfa_secret: None,
            mfa_enrolled_at: None,
            external_id: Some("idp-12345".to_string()),
            role: UserRole::User,
            is_disabled: false,
            ask_policy: None,
            legacy_ask_enabled: false,
            email_notifications: Default::default(),
            ui_prefs: None,
            status: None,
            created_at: 0,
            updated_at: 0,
            last_active_at: 0,
        }
    }

    #[test]
    fn user_to_scim_maps_core_fields() {
        let user = fixture_user();
        let scim = user_to_scim(
            &user,
            Some("https://x.test/scim/v2/workspaces/ws-1/Users/u-1".to_string()),
        );
        assert_eq!(scim.id.as_deref(), Some("u-1"));
        assert_eq!(scim.external_id.as_deref(), Some("idp-12345"));
        assert_eq!(scim.user_name, "alice@example.com");
        assert_eq!(scim.display_name.as_deref(), Some("Alice Wonderland"));
        assert_eq!(scim.active, Some(true));
        assert_eq!(scim.emails.len(), 1);
        assert_eq!(scim.emails[0].value.as_deref(), Some("alice@example.com"));
        assert_eq!(scim.emails[0].primary, Some(true));
        assert_eq!(
            scim.meta.as_ref().unwrap().location.as_deref(),
            Some("https://x.test/scim/v2/workspaces/ws-1/Users/u-1"),
        );
        assert_eq!(
            scim.meta.as_ref().unwrap().resource_type.as_deref(),
            Some("User"),
        );
    }

    #[test]
    fn user_to_scim_inverts_active_for_disabled_users() {
        let mut user = fixture_user();
        user.is_disabled = true;
        let scim = user_to_scim(&user, None);
        assert_eq!(
            scim.active,
            Some(false),
            "SCIM `active` is the inverse of our `is_disabled`"
        );
    }

    #[test]
    fn parse_display_name_single_word() {
        let n = parse_display_name("Madonna");
        assert_eq!(n.given_name.as_deref(), Some("Madonna"));
        assert_eq!(n.family_name, None);
        assert_eq!(n.formatted.as_deref(), Some("Madonna"));
    }

    #[test]
    fn parse_display_name_first_last() {
        let n = parse_display_name("Alice Smith");
        assert_eq!(n.given_name.as_deref(), Some("Alice"));
        assert_eq!(n.family_name.as_deref(), Some("Smith"));
    }

    #[test]
    fn parse_display_name_three_tokens_joins_remainder() {
        // "First Middle Last" — given=First, family="Middle Last".
        let n = parse_display_name("Mary Jane Watson");
        assert_eq!(n.given_name.as_deref(), Some("Mary"));
        assert_eq!(n.family_name.as_deref(), Some("Jane Watson"));
    }

    #[test]
    fn parse_display_name_handles_empty() {
        let n = parse_display_name("   ");
        assert!(n.given_name.is_none());
        assert!(n.family_name.is_none());
        assert!(n.formatted.is_none());
    }
}
