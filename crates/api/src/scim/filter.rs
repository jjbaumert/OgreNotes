// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Minimal SCIM filter parser (Phase 4 M-E5 piece D).
//!
//! RFC 7644 §3.4.2.2 defines a rich filter language with `eq`,
//! `ne`, `co`, `sw`, `ew`, `gt`, `ge`, `lt`, `le`, `pr`, `and`,
//! `or`, `not`, plus grouped expressions. Implementing the whole
//! thing is a real grammar-parser job.
//!
//! In practice the SCIM provisioners we care about (Okta, Entra
//! ID, Google Workspace, JumpCloud) only send a *single* `eq`
//! comparison when looking up a user by handle, and they always
//! filter on either `userName` or `externalId`. v1 covers exactly
//! that subset and rejects anything else with `invalidFilter`. A
//! later expansion can layer the richer grammar without changing
//! the call site.

use crate::scim::dtos::{scim_type, ScimError};

/// The supported filter shape: `<attribute> eq "<value>"`.
/// Attributes are limited to `userName` and `externalId` for v1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupportedFilter {
    UserNameEq(String),
    ExternalIdEq(String),
}

/// Parse a SCIM filter query parameter. Returns:
///   - `Ok(None)` if no filter was supplied (caller returns all
///     workspace users).
///   - `Ok(Some(filter))` for a supported `eq` clause.
///   - `Err(ScimError)` for any unsupported attribute or operator,
///     with `scimType=invalidFilter` per RFC 7644 §3.12.
pub fn parse_filter(raw: Option<&str>) -> Result<Option<SupportedFilter>, ScimError> {
    let Some(raw) = raw else { return Ok(None) };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }

    // The grammar we accept: `<attr> <ws> eq <ws> "<value>"`. Real
    // IdPs always emit lowercase `eq`; RFC says case-insensitive
    // but it's safe to require lowercase for v1 and reject the
    // capitalized form as `invalidFilter`. If a real client trips
    // that, expanding is a one-line change.
    let lower = raw.to_ascii_lowercase();
    let Some((attr, rest)) = lower.split_once(' ') else {
        return Err(ScimError::new(
            400,
            Some(scim_type::INVALID_FILTER),
            format!("filter must have shape `<attr> eq \"<value>\"`: {raw}"),
        ));
    };
    let rest = rest.trim_start();

    // Confirm the operator. We don't capture the rest — the quote
    // boundaries are extracted directly from `lower` below.
    if rest.strip_prefix("eq ").is_none() {
        return Err(ScimError::new(
            400,
            Some(scim_type::INVALID_FILTER),
            format!("only `eq` operator is supported in v1: {raw}"),
        ));
    }

    // Extract the quoted value. SCIM 2.0 grammar requires double
    // quotes. We extract from the ORIGINAL raw string (case-
    // preserved) using the byte offset of the opening quote in the
    // lowercased form — string lengths match byte-for-byte because
    // ASCII-only operators precede the quote.
    let q_start = lower.find('"').ok_or_else(|| {
        ScimError::new(
            400,
            Some(scim_type::INVALID_FILTER),
            format!("filter value must be double-quoted: {raw}"),
        )
    })?;
    let q_end_rel = lower[q_start + 1..].find('"').ok_or_else(|| {
        ScimError::new(
            400,
            Some(scim_type::INVALID_FILTER),
            format!("filter value missing closing quote: {raw}"),
        )
    })?;
    let q_end = q_start + 1 + q_end_rel;
    let value = raw[q_start + 1..q_end].to_string();

    // Whatever followed the closing quote (other than whitespace)
    // is an unsupported AND/OR clause.
    let tail = raw[q_end + 1..].trim();
    if !tail.is_empty() {
        return Err(ScimError::new(
            400,
            Some(scim_type::INVALID_FILTER),
            format!("v1 supports a single `eq` clause only: {raw}"),
        ));
    }

    match attr {
        "username" => Ok(Some(SupportedFilter::UserNameEq(value))),
        "externalid" => Ok(Some(SupportedFilter::ExternalIdEq(value))),
        other => Err(ScimError::new(
            400,
            Some(scim_type::INVALID_FILTER),
            format!(
                "v1 supports filter on `userName` or `externalId` only; got `{other}`"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_filter_none_returns_none() {
        assert_eq!(parse_filter(None).unwrap(), None);
    }

    #[test]
    fn parse_filter_empty_returns_none() {
        // Some IdPs send `?filter=` literally; treat as no filter.
        assert_eq!(parse_filter(Some("")).unwrap(), None);
        assert_eq!(parse_filter(Some("   ")).unwrap(), None);
    }

    #[test]
    fn parse_filter_username_eq() {
        let f = parse_filter(Some(r#"userName eq "alice@example.com""#)).unwrap();
        assert_eq!(
            f,
            Some(SupportedFilter::UserNameEq("alice@example.com".to_string()))
        );
    }

    #[test]
    fn parse_filter_external_id_eq() {
        let f = parse_filter(Some(r#"externalId eq "okta-12345""#)).unwrap();
        assert_eq!(
            f,
            Some(SupportedFilter::ExternalIdEq("okta-12345".to_string()))
        );
    }

    #[test]
    fn parse_filter_preserves_value_case() {
        // Filter operators / attribute names are case-insensitive,
        // but the VALUE inside the quotes must round-trip verbatim.
        // A user whose externalId is `Okta-12345` must not be
        // looked up by `okta-12345`.
        let f = parse_filter(Some(r#"externalId eq "Okta-CaseSensitive""#)).unwrap();
        assert_eq!(
            f,
            Some(SupportedFilter::ExternalIdEq("Okta-CaseSensitive".to_string()))
        );
    }

    #[test]
    fn parse_filter_rejects_unknown_attr() {
        let err = parse_filter(Some(r#"displayName eq "Alice""#)).unwrap_err();
        assert_eq!(err.status, "400");
        assert_eq!(err.scim_type.as_deref(), Some("invalidFilter"));
    }

    #[test]
    fn parse_filter_rejects_ne_operator() {
        let err = parse_filter(Some(r#"userName ne "alice""#)).unwrap_err();
        assert_eq!(err.scim_type.as_deref(), Some("invalidFilter"));
    }

    #[test]
    fn parse_filter_rejects_compound_and() {
        let err = parse_filter(Some(
            r#"userName eq "alice" and externalId eq "x""#,
        ))
        .unwrap_err();
        assert_eq!(err.scim_type.as_deref(), Some("invalidFilter"));
    }

    #[test]
    fn parse_filter_rejects_missing_quotes() {
        let err = parse_filter(Some(r#"userName eq alice"#)).unwrap_err();
        assert_eq!(err.scim_type.as_deref(), Some("invalidFilter"));
    }

    #[test]
    fn parse_filter_rejects_unclosed_quote() {
        let err = parse_filter(Some(r#"userName eq "alice"#)).unwrap_err();
        assert_eq!(err.scim_type.as_deref(), Some("invalidFilter"));
    }
}
