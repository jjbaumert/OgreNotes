// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

pub mod activity;
pub mod admin_audit;
pub mod document;
pub mod folder;
pub mod mfa_recovery;
pub mod notification;
pub mod security_audit;
pub mod session;
pub mod snapshot;
pub mod template_gallery;
pub mod thread;
pub mod user;
pub mod workspace;
pub mod workspace_saml_config;
pub mod workspace_scim_token;

/// Access levels for folder/document membership.
///
/// Serialization is UPPERCASE (e.g. `"VIEW"`, `"EDIT"`) — that's
/// the wire shape DynamoDB stores and existing clients send. The
/// `#[serde(alias)]` lines on each variant additionally accept the
/// lowercase form on deserialize so requests carrying `"view"` /
/// `"edit"` succeed (this was a real interop issue surfaced by
/// the M-P7 bulk-share integration test). The serialization output
/// is unchanged; aliases affect deserialization only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum AccessLevel {
    #[serde(alias = "own")]
    Own,
    #[serde(alias = "edit")]
    Edit,
    #[serde(alias = "comment")]
    Comment,
    #[serde(alias = "view")]
    View,
}

/// Document types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocType {
    Document,
    Spreadsheet,
    Chat,
}

impl DocType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocType::Document => "document",
            DocType::Spreadsheet => "spreadsheet",
            DocType::Chat => "chat",
        }
    }
}

/// Folder types.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FolderType {
    System,
    User,
}

impl FolderType {
    pub fn as_str(&self) -> &'static str {
        match self {
            FolderType::System => "system",
            FolderType::User => "user",
        }
    }
}

/// Child types for folder membership.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildType {
    Doc,
    Folder,
}

impl ChildType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChildType::Doc => "doc",
            ChildType::Folder => "folder",
        }
    }
}

/// Notification levels for per-thread preferences.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifLevel {
    /// Notify on all activity in the thread.
    All,
    /// Notify only on @mentions and direct replies (default).
    Direct,
    /// No notifications for this thread.
    Mute,
}

impl Default for NotifLevel {
    fn default() -> Self {
        Self::Direct
    }
}

/// Email notification preferences.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifEmailPref {
    All,
    MentionsOnly,
    Disabled,
}

impl Default for NotifEmailPref {
    fn default() -> Self {
        Self::MentionsOnly
    }
}

/// Link sharing modes for documents.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkSharingMode {
    Edit,
    View,
    None,
}

/// View-mode sub-options for a link-shared document. Each gates one
/// extra capability for members who reach the doc through a `View`-mode
/// link; all are ignored when the link mode is `Edit` (Edit already
/// implies them). Defaults to all-false — the owner opts each one in.
///
/// These are *capabilities checked at the feature endpoints* (comments,
/// history, conversation, request-access — Phase 2), deliberately not
/// folded into `AccessLevel`. `#[serde(default)]` on the struct lets a
/// stored row that predates a newly-added field decode with that field
/// false, and a row with no options attribute at all decode as the
/// all-false default.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewOptions {
    pub allow_comments: bool,
    pub show_history: bool,
    pub show_conversation: bool,
    pub allow_request_access: bool,
}

/// Folder permission inheritance mode.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InheritMode {
    Inherit,
    Restricted,
}

impl Default for InheritMode {
    fn default() -> Self {
        Self::Inherit
    }
}

/// Workspace membership roles.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Member,
}

impl WorkspaceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            WorkspaceRole::Owner => "owner",
            WorkspaceRole::Admin => "admin",
            WorkspaceRole::Member => "member",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bare serialized token for an enum value (serde wraps it in quotes).
    fn token<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string(v)
            .unwrap()
            .trim_matches('"')
            .to_string()
    }

    // ── AccessLevel: UPPERCASE on the wire, case-insensitive in ──────
    // These pin the contract spelled out in the type's doc comment:
    // serialization stays UPPERCASE, deserialization additionally accepts
    // the lowercase form (the M-P7 bulk-share interop fix).

    #[test]
    fn access_level_serializes_uppercase() {
        assert_eq!(token(&AccessLevel::Own), "OWN");
        assert_eq!(token(&AccessLevel::Edit), "EDIT");
        assert_eq!(token(&AccessLevel::Comment), "COMMENT");
        assert_eq!(token(&AccessLevel::View), "VIEW");
    }

    #[test]
    fn access_level_deserializes_either_case() {
        // Canonical UPPERCASE form.
        assert_eq!(
            serde_json::from_str::<AccessLevel>("\"VIEW\"").unwrap(),
            AccessLevel::View
        );
        // Lowercase alias form (the interop fix).
        assert_eq!(
            serde_json::from_str::<AccessLevel>("\"view\"").unwrap(),
            AccessLevel::View
        );
        assert_eq!(
            serde_json::from_str::<AccessLevel>("\"edit\"").unwrap(),
            AccessLevel::Edit
        );
        // A bogus token is still rejected.
        assert!(serde_json::from_str::<AccessLevel>("\"sudo\"").is_err());
    }

    // ── as_str() must agree with serde serialization ─────────────────
    // These enums carry both a hand-written as_str() and a serde
    // rename_all. If the two drift, the same value serializes to two
    // different wire strings depending on the code path — a silent data
    // bug. This invariant was previously unguarded.

    #[test]
    fn as_str_agrees_with_serde() {
        assert_eq!(DocType::Document.as_str(), token(&DocType::Document));
        assert_eq!(DocType::Spreadsheet.as_str(), token(&DocType::Spreadsheet));
        assert_eq!(DocType::Chat.as_str(), token(&DocType::Chat));

        assert_eq!(FolderType::System.as_str(), token(&FolderType::System));
        assert_eq!(FolderType::User.as_str(), token(&FolderType::User));

        assert_eq!(ChildType::Doc.as_str(), token(&ChildType::Doc));
        assert_eq!(ChildType::Folder.as_str(), token(&ChildType::Folder));

        assert_eq!(WorkspaceRole::Owner.as_str(), token(&WorkspaceRole::Owner));
        assert_eq!(WorkspaceRole::Admin.as_str(), token(&WorkspaceRole::Admin));
        assert_eq!(WorkspaceRole::Member.as_str(), token(&WorkspaceRole::Member));
    }

    // ── Defaults are behavioral contracts ────────────────────────────

    #[test]
    fn enum_defaults_match_documented_values() {
        assert_eq!(NotifLevel::default(), NotifLevel::Direct);
        assert_eq!(NotifEmailPref::default(), NotifEmailPref::MentionsOnly);
        assert_eq!(InheritMode::default(), InheritMode::Inherit);
    }

    #[test]
    fn lowercase_enums_round_trip() {
        for v in [DocType::Document, DocType::Spreadsheet, DocType::Chat] {
            let s = serde_json::to_string(&v).unwrap();
            assert_eq!(serde_json::from_str::<DocType>(&s).unwrap(), v);
        }
        assert_eq!(token(&NotifEmailPref::MentionsOnly), "mentionsonly");
        assert_eq!(token(&LinkSharingMode::None), "none");
        assert_eq!(token(&InheritMode::Restricted), "restricted");
    }

    // ── ViewOptions forward-compat decode ────────────────────────────
    // `#[serde(default)]` lets a stored row that predates a field — or
    // has no options attribute at all — decode with the missing fields
    // false. This is what keeps old rows readable after a field is added.

    #[test]
    fn view_options_default_is_all_false() {
        let d = ViewOptions::default();
        assert!(!d.allow_comments);
        assert!(!d.show_history);
        assert!(!d.show_conversation);
        assert!(!d.allow_request_access);
    }

    #[test]
    fn view_options_empty_object_decodes_all_false() {
        let v: ViewOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(v, ViewOptions::default());
    }

    #[test]
    fn view_options_partial_fills_missing_with_false() {
        // A row written before `allowRequestAccess` existed: only the
        // older fields are present; the new one must decode as false.
        let v: ViewOptions =
            serde_json::from_str(r#"{"allowComments":true,"showHistory":true}"#).unwrap();
        assert!(v.allow_comments);
        assert!(v.show_history);
        assert!(!v.show_conversation);
        assert!(!v.allow_request_access);
    }

    #[test]
    fn view_options_uses_camel_case_on_the_wire() {
        let json = serde_json::to_string(&ViewOptions {
            allow_comments: true,
            ..Default::default()
        })
        .unwrap();
        assert!(json.contains("allowComments"), "got {json}");
        assert!(!json.contains("allow_comments"), "snake_case leaked: {json}");
    }
}
