// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Security audit-log model.
//!
//! Records authentication, MFA, SAML, SCIM, share-revoke, and
//! deletion events for forensics and compliance. The storage row
//! shape mirrors `AdminAudit` column-for-column so a future
//! consolidation into a single unified audit table is a rename of
//! the SK prefix (`SEC_AUDIT#` → `AUDIT#`) plus a kind-namespace
//! merge — no schema migration on the column names.
//!
//! DynamoDB key pattern:
//!   PK = `USER#<user_id>`            — the user this event is about
//!   SK = `SEC_AUDIT#<created_at:020>#<audit_id>`
//!
//! The 20-digit zero-padded timestamp keeps SK ordering chronological
//! under string comparison when listed via `scan_index_forward(false)`.
//!
//! API surface vs storage shape:
//!
//! - Call sites construct a `SecurityAuditAction` with typed inline
//!   payloads (`LoginFailure { reason }`, `ShareRevoked { doc_id,
//!   target }`, …). This makes the writer ergonomically type-safe and
//!   forces every event-emitting site to supply every field the
//!   variant requires — there's no "forgot to populate detail" path.
//! - The repo decomposes the typed variant into the `action` (tag
//!   string) and `detail` (JSON-as-string) columns at write time and
//!   reassembles it on read. Future readers that only care about the
//!   tag (e.g. dashboards, retention sweeps) can query the column
//!   without parsing the detail blob.

use serde::{Deserialize, Serialize};

/// One security-relevant event. Variants carry inline typed payloads;
/// the repo decomposes into `action` + `detail` columns at the
/// DynamoDB boundary. New variants are append-only: a row written with
/// a variant the current binary doesn't recognize surfaces as a
/// `RepoError::MissingField("unknown security audit action: …")`,
/// not silently as a generic event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "detail", rename_all = "camelCase")]
pub enum SecurityAuditAction {
    /// Successful authentication. Actor = the user themselves.
    LoginSuccess,
    /// Failed authentication. `reason` is a short tag like
    /// `"bad_password"` or `"disabled"` — not a free-form message
    /// (avoid leaking which check failed via the audit log).
    LoginFailure { reason: String },
    /// User initiated TOTP enrollment (secret generated, QR scanned,
    /// not yet verified).
    MfaEnroll,
    /// User submitted a TOTP code at challenge time. `ok = false`
    /// rows surface in MFA-bypass-attempt rate-limit alerts.
    MfaVerify { ok: bool },
    /// User submitted a recovery code that didn't match any stored
    /// hash. Distinct from `MfaVerify { ok: false }` because the
    /// rate-limit alert that watches MFA-bypass attempts needs to
    /// distinguish 6-digit TOTP brute-force from 50-bit recovery-
    /// code brute-force — the cost and cadence differ enough that
    /// conflating them dilutes the signal.
    MfaRecoveryFailed,
    /// User consumed a single-use recovery code.
    MfaRecoveryUsed,
    /// User disarmed their MFA (clear secret + recovery codes).
    /// Fresh TOTP required before this event fires; logging it is
    /// the only forensic trail that an account's MFA was removed,
    /// because the SK-keyed enrollment rows are gone afterwards.
    MfaDisarm,
    /// SAML ACS endpoint verified an assertion and minted a session
    /// for this user. `workspace_id` is the workspace whose IdP
    /// config was used (Phase 4 v1 is one IdP per workspace);
    /// `name_id` is the IdP-assigned NameID. Both are required for
    /// forensic correlation in multi-workspace setups — a SAML event
    /// without workspace context is unactionable.
    SamlAssertionAccepted { workspace_id: String, name_id: String },
    /// A SCIM bearer token authenticated and performed an operation
    /// on this workspace. `token_id` is the opaque handle of the
    /// token row — not the secret. `op` is the dotted handler label
    /// (`users.list`, `users.create`, `users.replace`, `users.patch`,
    /// `users.delete`, `groups.list`, `groups.get`, `groups.patch`,
    /// `discovery.serviceProviderConfig`, `discovery.resourceTypes`,
    /// `discovery.schemas`) so retention scans can distinguish read
    /// traffic from mutation traffic without parsing the full
    /// request log. The closed set is defined in `routes::scim::ScimOp`.
    ScimTokenUsed { token_id: String, op: String },
    /// A member was removed from a document or folder. `target` is
    /// the removed member's user id or email; `doc_id` may be a
    /// folder id for folder revokes (the column doesn't distinguish
    /// — readers can correlate against the doc/folder repos).
    ShareRevoked { doc_id: String, target: String },
    /// A member was granted access to a document or folder. Mirror
    /// of `ShareRevoked` — `doc_id` may carry a folder id (the
    /// column doesn't distinguish). `level` is the access level the
    /// target was granted (`OWN` / `EDIT` / `COMMENT` / `VIEW`),
    /// serialized to match the wire-shape `AccessLevel` uses
    /// elsewhere.
    ShareGranted { doc_id: String, target: String, level: String },
    /// A member's access level was changed (PATCH on members
    /// endpoint). `level` is the new level — the previous level is
    /// not stored on the row because the audit chain is forward-
    /// reconstructable: walk the user's `SEC_AUDIT#` rows in order,
    /// and at any point the active level is the most recent
    /// non-revoked event's `level`.
    ShareUpdated { doc_id: String, target: String, level: String },
    /// A document's link-sharing settings were changed. Records the
    /// **resulting** state: `mode` is the new link mode (`View` / `Edit`)
    /// or `None` when disabled, and `view_options` is the full set of
    /// view-mode sub-options after the change. There is no `target` —
    /// the change affects every workspace member's access at once rather
    /// than a single member. Both mode changes and sub-option changes are
    /// audited (a sub-option toggle is a permission change); a reader
    /// diffs against the prior row to see exactly what moved. (`mode`
    /// serializes as `"view"`/`"edit"`/null; `view_options` as a camelCase
    /// object.)
    ///
    /// **Self-event (owner path, Phase 0):** `user_id` PK == `actor_id`
    /// == the doc owner; emitted by `PATCH /documents/{id}/link-settings`
    /// via `record_security_event`.
    ///
    /// **Cross-actor (admin override, Phase 3):** `user_id` PK == doc
    /// owner (subject), `actor_id` == admin; emitted by
    /// `PATCH /admin/documents/{id}/link-settings` via
    /// `record_security_event_by_actor` so forensics can answer "who
    /// changed it" independently of "whose document was changed."
    LinkSharingChanged {
        doc_id: String,
        mode: Option<super::LinkSharingMode>,
        view_options: super::ViewOptions,
    },
    /// A document was deleted. `hard = false` is a soft-delete (move
    /// to Trash); `hard = true` is a trash-cleanup purge after the
    /// retention window. The `user_id` PK is the document owner.
    DocDeleted { doc_id: String, hard: bool },
    /// An admin force-compacted a document (`POST /admin/documents/{id}
    /// /compact`) — snapshot the live Y.Doc to S3 and prune the op log,
    /// bypassing the normal no-active-clients gate. A privileged action on
    /// someone else's document, so it's cross-actor: the `user_id` PK is the
    /// document owner (subject), `actor_id` is the admin. Mirrors the
    /// `DocDeleted` keying so it surfaces in `GET /admin/audit`.
    DocCompacted { doc_id: String },
    /// A session was forcibly terminated. `reason` is a short tag —
    /// `"refresh_reuse_detected"`, `"admin_disable"`, etc.
    SessionRevoked { reason: String },
    /// A user edited their own profile (display name and/or avatar)
    /// via `PUT /users/me`. Actor == subject. Only *which* fields
    /// changed is recorded — never the values — so the audit log
    /// stays free of mutable PII while still answering "when did this
    /// account's identity surface change."
    ProfileUpdated { name_changed: bool, avatar_changed: bool },
    /// #140: a document's edit-lock was toggled (`PUT /documents/{id}/lock`).
    /// `locked` is the resulting state. A doc-wide freeze affecting every
    /// collaborator's write authority, so it's auditable. The `user_id` PK is
    /// the document owner (subject); `actor_id` is whoever toggled it (today
    /// always the owner, since the toggle is owner-only). Mirrors the
    /// `DocDeleted` keying.
    DocLockToggled { doc_id: String, locked: bool },
    /// A login via one OAuth provider that resolved to an existing account
    /// created via a *different* provider (GitHub<->Google account linking).
    /// Recorded for forensics — distinct from a same-provider `LoginSuccess`
    /// so cross-provider access to an account is not invisible in the log.
    AccountLinked { from_provider: String, to_provider: String },
    /// A synthetic system user was provisioned by a seed / migration binary
    /// (identity write). Subject = the created user; actor_id is a fixed
    /// label like `"seed"` since the operator identity isn't captured. Rare
    /// — fires once per stack prefix on the first seed run.
    SystemUserProvisioned,
    /// Phase 4 #142: a workspace admin created a company template gallery.
    /// Subject = actor (the admin who created it); the gallery's workspace
    /// and id land in the detail payload.
    TemplateGalleryCreated { workspace_id: String, gallery_id: String },
    /// Phase 4 #142: a workspace admin updated a company template gallery
    /// (renamed or changed the membership list).
    TemplateGalleryUpdated { workspace_id: String, gallery_id: String },
    /// Phase 4 #142: a workspace admin deleted a company template gallery.
    TemplateGalleryDeleted { workspace_id: String, gallery_id: String },
    /// An admin ran `POST /admin/documents/:id/repair-liveapp-attrs` to
    /// walk a document and canonicalize any LiveApp attributes that
    /// failed `validate_attrs`. `canonicalized_count` is the number
    /// of (node, attribute) pairs actually rewritten. Cross-actor:
    /// `user_id` PK is the document owner (subject), `actor_id` is
    /// the admin. Surfaces in `GET /admin/audit`.
    LiveAppAttrsRepaired { doc_id: String, canonicalized_count: usize },
    /// A LiveApp sub-node (Kanban card / column, Calendar event) was
    /// deleted from a document via an interactive write. gap-003 from
    /// the post-hardening security audit. Emitted per deleted
    /// LiveApp node so incident response can query "who deleted the
    /// assignee card on this doc?" without having to walk CRDT
    /// history. `block_id` may be empty when the deleted node had
    /// no `blockId` attribute stamped.
    LiveAppNodeDeleted {
        doc_id: String,
        node_type: String,
        block_id: String,
    },
}

impl SecurityAuditAction {
    /// Tag string written to the `action` storage column. Mirrors
    /// the `AdminAuditAction::as_str()` precedent — used as the
    /// indexable discriminator without parsing the detail JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LoginSuccess => "loginSuccess",
            Self::LoginFailure { .. } => "loginFailure",
            Self::MfaEnroll => "mfaEnroll",
            Self::MfaVerify { .. } => "mfaVerify",
            Self::MfaRecoveryFailed => "mfaRecoveryFailed",
            Self::MfaRecoveryUsed => "mfaRecoveryUsed",
            Self::MfaDisarm => "mfaDisarm",
            Self::SamlAssertionAccepted { .. } => "samlAssertionAccepted",
            Self::ScimTokenUsed { .. } => "scimTokenUsed",
            Self::ShareRevoked { .. } => "shareRevoked",
            Self::ShareGranted { .. } => "shareGranted",
            Self::ShareUpdated { .. } => "shareUpdated",
            Self::LinkSharingChanged { .. } => "linkSharingChanged",
            Self::DocDeleted { .. } => "docDeleted",
            Self::DocCompacted { .. } => "docCompacted",
            Self::SessionRevoked { .. } => "sessionRevoked",
            Self::ProfileUpdated { .. } => "profileUpdated",
            Self::DocLockToggled { .. } => "docLockToggled",
            Self::AccountLinked { .. } => "accountLinked",
            Self::SystemUserProvisioned => "systemUserProvisioned",
            Self::TemplateGalleryCreated { .. } => "templateGalleryCreated",
            Self::TemplateGalleryUpdated { .. } => "templateGalleryUpdated",
            Self::TemplateGalleryDeleted { .. } => "templateGalleryDeleted",
            Self::LiveAppAttrsRepaired { .. } => "liveAppAttrsRepaired",
            Self::LiveAppNodeDeleted { .. } => "liveAppNodeDeleted",
        }
    }

    /// Detail payload as JSON. Empty `{}` for unit variants. The repo
    /// writes this `.to_string()` into the `detail` column AND
    /// `routes/admin.rs` re-emits it verbatim in the
    /// `GET /admin/audit` response, so the keys here ARE part of
    /// the public API wire shape. Camel-case to match the
    /// `AuditEntry` outer fields (`auditId`, `actorId`,
    /// `targetUserId`) and the project-wide
    /// `#[serde(rename_all = "camelCase")]` convention.
    pub fn detail_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::LoginSuccess => json!({}),
            Self::LoginFailure { reason } => json!({ "reason": reason }),
            Self::MfaEnroll => json!({}),
            Self::MfaVerify { ok } => json!({ "ok": ok }),
            Self::MfaRecoveryFailed => json!({}),
            Self::MfaRecoveryUsed => json!({}),
            Self::MfaDisarm => json!({}),
            Self::SamlAssertionAccepted { workspace_id, name_id } => {
                json!({ "workspaceId": workspace_id, "nameId": name_id })
            }
            Self::ScimTokenUsed { token_id, op } => {
                json!({ "tokenId": token_id, "op": op })
            }
            Self::ShareRevoked { doc_id, target } => json!({ "docId": doc_id, "target": target }),
            Self::ShareGranted { doc_id, target, level } => {
                json!({ "docId": doc_id, "target": target, "level": level })
            }
            Self::ShareUpdated { doc_id, target, level } => {
                json!({ "docId": doc_id, "target": target, "level": level })
            }
            Self::LinkSharingChanged { doc_id, mode, view_options } => {
                json!({ "docId": doc_id, "mode": mode, "viewOptions": view_options })
            }
            Self::DocDeleted { doc_id, hard } => json!({ "docId": doc_id, "hard": hard }),
            Self::DocCompacted { doc_id } => json!({ "docId": doc_id }),
            Self::SessionRevoked { reason } => json!({ "reason": reason }),
            Self::ProfileUpdated { name_changed, avatar_changed } => {
                json!({ "nameChanged": name_changed, "avatarChanged": avatar_changed })
            }
            Self::DocLockToggled { doc_id, locked } => {
                json!({ "docId": doc_id, "locked": locked })
            }
            Self::AccountLinked { from_provider, to_provider } => {
                json!({ "fromProvider": from_provider, "toProvider": to_provider })
            }
            Self::SystemUserProvisioned => json!({}),
            Self::TemplateGalleryCreated { workspace_id, gallery_id }
            | Self::TemplateGalleryUpdated { workspace_id, gallery_id }
            | Self::TemplateGalleryDeleted { workspace_id, gallery_id } => {
                json!({ "workspaceId": workspace_id, "galleryId": gallery_id })
            }
            Self::LiveAppAttrsRepaired { doc_id, canonicalized_count } => {
                json!({ "docId": doc_id, "canonicalizedCount": canonicalized_count })
            }
            Self::LiveAppNodeDeleted { doc_id, node_type, block_id } => {
                json!({
                    "docId": doc_id,
                    "nodeType": node_type,
                    "blockId": block_id,
                })
            }
        }
    }

    /// Reconstruct the typed variant from the storage columns. Used
    /// by the repo's read path. Unknown `tag` values fail loud — the
    /// row was written by a newer binary or is corrupt; defaulting to
    /// a generic variant would hide the schema skew.
    pub fn from_storage(tag: &str, detail: &serde_json::Value) -> Result<Self, String> {
        // Accept either casing: pre-2026-05-18 rows were written
        // with snake_case keys (e.g. `doc_id`); post-2026-05-18 rows
        // are camelCase (`docId`) so the JSON shape matches the
        // outer AuditEntry's camelCase convention end-to-end. The
        // fallback keeps any rows already written under the old
        // shape readable forever — there's no migration step.
        let get_str = |primary: &str, fallback: Option<&str>| -> Result<String, String> {
            if let Some(v) = detail.get(primary).and_then(|v| v.as_str()) {
                return Ok(v.to_string());
            }
            if let Some(legacy) = fallback {
                if let Some(v) = detail.get(legacy).and_then(|v| v.as_str()) {
                    return Ok(v.to_string());
                }
            }
            Err(format!("{tag}: missing detail.{primary} (string)"))
        };
        let get_bool = |key: &str| -> Result<bool, String> {
            detail
                .get(key)
                .and_then(|v| v.as_bool())
                .ok_or_else(|| format!("{tag}: missing detail.{key} (bool)"))
        };
        match tag {
            "loginSuccess" => Ok(Self::LoginSuccess),
            "loginFailure" => Ok(Self::LoginFailure { reason: get_str("reason", None)? }),
            "mfaEnroll" => Ok(Self::MfaEnroll),
            "mfaVerify" => Ok(Self::MfaVerify { ok: get_bool("ok")? }),
            "mfaRecoveryFailed" => Ok(Self::MfaRecoveryFailed),
            "mfaRecoveryUsed" => Ok(Self::MfaRecoveryUsed),
            "mfaDisarm" => Ok(Self::MfaDisarm),
            "samlAssertionAccepted" => Ok(Self::SamlAssertionAccepted {
                workspace_id: get_str("workspaceId", Some("workspace_id"))?,
                name_id: get_str("nameId", Some("name_id"))?,
            }),
            "scimTokenUsed" => Ok(Self::ScimTokenUsed {
                token_id: get_str("tokenId", Some("token_id"))?,
                op: get_str("op", None)?,
            }),
            "shareRevoked" => Ok(Self::ShareRevoked {
                doc_id: get_str("docId", Some("doc_id"))?,
                target: get_str("target", None)?,
            }),
            "shareGranted" => Ok(Self::ShareGranted {
                doc_id: get_str("docId", Some("doc_id"))?,
                target: get_str("target", None)?,
                level: get_str("level", None)?,
            }),
            "shareUpdated" => Ok(Self::ShareUpdated {
                doc_id: get_str("docId", Some("doc_id"))?,
                target: get_str("target", None)?,
                level: get_str("level", None)?,
            }),
            "linkSharingChanged" => {
                // Absent / null / non-string `mode` => disabled (None);
                // a present string parses to the typed LinkSharingMode.
                let mode = match detail.get("mode").and_then(|v| v.as_str()) {
                    Some(s) => Some(
                        serde_json::from_value::<super::LinkSharingMode>(
                            serde_json::Value::String(s.to_string()),
                        )
                        .map_err(|e| format!("{tag}: bad mode '{s}': {e}"))?,
                    ),
                    None => None,
                };
                // Absent / null / non-object `viewOptions` => default (also the
                // read path for pre-enrichment rows that lack the field).
                let view_options = match detail.get("viewOptions") {
                    Some(v) if v.is_object() => {
                        serde_json::from_value(v.clone()).unwrap_or_default()
                    }
                    _ => super::ViewOptions::default(),
                };
                Ok(Self::LinkSharingChanged {
                    doc_id: get_str("docId", Some("doc_id"))?,
                    mode,
                    view_options,
                })
            }
            "docDeleted" => Ok(Self::DocDeleted {
                doc_id: get_str("docId", Some("doc_id"))?,
                hard: get_bool("hard")?,
            }),
            "docCompacted" => Ok(Self::DocCompacted {
                doc_id: get_str("docId", Some("doc_id"))?,
            }),
            "sessionRevoked" => Ok(Self::SessionRevoked { reason: get_str("reason", None)? }),
            "profileUpdated" => Ok(Self::ProfileUpdated {
                name_changed: get_bool("nameChanged")?,
                avatar_changed: get_bool("avatarChanged")?,
            }),
            "docLockToggled" => Ok(Self::DocLockToggled {
                doc_id: get_str("docId", Some("doc_id"))?,
                locked: get_bool("locked")?,
            }),
            "accountLinked" => Ok(Self::AccountLinked {
                from_provider: get_str("fromProvider", Some("from_provider"))?,
                to_provider: get_str("toProvider", Some("to_provider"))?,
            }),
            "systemUserProvisioned" => Ok(Self::SystemUserProvisioned),
            "templateGalleryCreated" => Ok(Self::TemplateGalleryCreated {
                workspace_id: get_str("workspaceId", Some("workspace_id"))?,
                gallery_id: get_str("galleryId", Some("gallery_id"))?,
            }),
            "templateGalleryUpdated" => Ok(Self::TemplateGalleryUpdated {
                workspace_id: get_str("workspaceId", Some("workspace_id"))?,
                gallery_id: get_str("galleryId", Some("gallery_id"))?,
            }),
            "templateGalleryDeleted" => Ok(Self::TemplateGalleryDeleted {
                workspace_id: get_str("workspaceId", Some("workspace_id"))?,
                gallery_id: get_str("galleryId", Some("gallery_id"))?,
            }),
            "liveAppAttrsRepaired" => Ok(Self::LiveAppAttrsRepaired {
                doc_id: get_str("docId", Some("doc_id"))?,
                canonicalized_count: detail
                    .get("canonicalizedCount")
                    .or_else(|| detail.get("canonicalized_count"))
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .ok_or_else(|| {
                        format!("{tag}: missing detail.canonicalizedCount (u64)")
                    })?,
            }),
            "liveAppNodeDeleted" => Ok(Self::LiveAppNodeDeleted {
                doc_id: get_str("docId", Some("doc_id"))?,
                node_type: get_str("nodeType", Some("node_type"))?,
                block_id: get_str("blockId", Some("block_id"))?,
            }),
            other => Err(format!("unknown security audit action: {other}")),
        }
    }
}

/// One row in the security audit log.
///
/// `user_id` (PK) is the subject of the event. `actor_id` is who
/// caused it — equal to `user_id` for self-events (login, MFA),
/// different for admin-driven events (admin revoked a session,
/// trash-cleanup worker purged a doc). Matches the
/// `AdminAudit { target_user_id, actor_id }` precedent.
#[derive(Debug, Clone)]
pub struct SecurityAudit {
    pub audit_id: String,
    pub user_id: String,
    pub actor_id: String,
    pub action: SecurityAuditAction,
    pub created_at: i64,
}

impl SecurityAudit {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk(&self) -> String {
        format!("SEC_AUDIT#{:020}#{}", self.created_at, self.audit_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(action: SecurityAuditAction, created_at: i64) -> SecurityAudit {
        SecurityAudit {
            audit_id: "aud1".to_string(),
            user_id: "alice".to_string(),
            actor_id: "alice".to_string(),
            action,
            created_at,
        }
    }

    #[test]
    fn pk_targets_the_subject_user() {
        let row = fixture(SecurityAuditAction::LoginSuccess, 0);
        assert_eq!(row.pk(), "USER#alice");
    }

    #[test]
    fn sk_zero_pads_for_chronological_ordering() {
        // Same invariant as AdminAudit: lexicographic SK must match
        // numeric timestamp order so newest-first scans work without
        // any client-side re-sort.
        let a = fixture(SecurityAuditAction::LoginSuccess, 100);
        let b = fixture(SecurityAuditAction::LoginSuccess, 1_700_000_000_000_000);
        assert!(a.sk() < b.sk(), "SK ordering must be chronological under string compare");

        // The pair above happens to sort correctly even without
        // zero-padding (compare character 12: '0' < '7'). The pair
        // below would fail under a missing pad — `"…#9#"` >
        // `"…#10#"` lexicographically. This is what the format
        // specifier actually guards.
        let c = fixture(SecurityAuditAction::LoginSuccess, 9);
        let d = fixture(SecurityAuditAction::LoginSuccess, 10);
        assert!(c.sk() < d.sk(), "zero-pad required: single-digit must sort before double-digit");
    }

    #[test]
    fn sk_uses_sec_audit_prefix() {
        // The future-consolidation path renames this prefix to
        // `AUDIT#` and merges with ADMIN_AUDIT — until then it must
        // stay distinct so begins_with queries don't collide.
        let row = fixture(SecurityAuditAction::LoginSuccess, 1);
        assert!(row.sk().starts_with("SEC_AUDIT#"));
    }

    #[test]
    fn action_tag_round_trips_for_every_variant() {
        let cases = vec![
            SecurityAuditAction::LoginSuccess,
            SecurityAuditAction::LoginFailure { reason: "bad_password".to_string() },
            SecurityAuditAction::MfaEnroll,
            SecurityAuditAction::MfaVerify { ok: true },
            SecurityAuditAction::MfaVerify { ok: false },
            SecurityAuditAction::MfaRecoveryUsed,
            SecurityAuditAction::MfaRecoveryFailed,
            SecurityAuditAction::MfaDisarm,
            SecurityAuditAction::SamlAssertionAccepted {
                workspace_id: "ws1".to_string(),
                name_id: "alice@example.com".to_string(),
            },
            SecurityAuditAction::ScimTokenUsed {
                token_id: "tok42".to_string(),
                op: "users.create".to_string(),
            },
            SecurityAuditAction::ShareRevoked {
                doc_id: "doc1".to_string(),
                target: "bob@example.com".to_string(),
            },
            SecurityAuditAction::LinkSharingChanged {
                doc_id: "doc1".to_string(),
                mode: Some(crate::models::LinkSharingMode::View),
                view_options: crate::models::ViewOptions::default(),
            },
            SecurityAuditAction::LinkSharingChanged {
                doc_id: "doc1".to_string(),
                mode: Some(crate::models::LinkSharingMode::Edit),
                view_options: crate::models::ViewOptions {
                    allow_comments: true,
                    show_history: true,
                    ..Default::default()
                },
            },
            SecurityAuditAction::LinkSharingChanged {
                doc_id: "doc1".to_string(),
                mode: None,
                view_options: crate::models::ViewOptions::default(),
            },
            SecurityAuditAction::DocDeleted { doc_id: "doc1".to_string(), hard: true },
            SecurityAuditAction::DocDeleted { doc_id: "doc1".to_string(), hard: false },
            SecurityAuditAction::DocCompacted { doc_id: "doc1".to_string() },
            SecurityAuditAction::SessionRevoked { reason: "refresh_reuse_detected".to_string() },
            SecurityAuditAction::ProfileUpdated { name_changed: true, avatar_changed: false },
            SecurityAuditAction::ProfileUpdated { name_changed: false, avatar_changed: true },
            SecurityAuditAction::SystemUserProvisioned,
            SecurityAuditAction::TemplateGalleryCreated {
                workspace_id: "ws-1".to_string(),
                gallery_id: "g-1".to_string(),
            },
            SecurityAuditAction::TemplateGalleryUpdated {
                workspace_id: "ws-1".to_string(),
                gallery_id: "g-1".to_string(),
            },
            SecurityAuditAction::TemplateGalleryDeleted {
                workspace_id: "ws-1".to_string(),
                gallery_id: "g-1".to_string(),
            },
            SecurityAuditAction::LiveAppAttrsRepaired {
                doc_id: "doc-1".to_string(),
                canonicalized_count: 7,
            },
            SecurityAuditAction::LiveAppNodeDeleted {
                doc_id: "doc-1".to_string(),
                node_type: "kanban_card".to_string(),
                block_id: "blk-42".to_string(),
            },
        ];
        for original in cases {
            let tag = original.as_str();
            let detail = original.detail_json();
            let back = SecurityAuditAction::from_storage(tag, &detail)
                .unwrap_or_else(|e| panic!("roundtrip failed for {original:?}: {e}"));
            assert_eq!(back, original, "roundtrip mismatch on {original:?}");
        }
    }

    #[test]
    fn unknown_action_tag_surfaces_distinctly() {
        let err = SecurityAuditAction::from_storage("loginUltraSuccess", &serde_json::json!({}))
            .expect_err("unknown tag must fail");
        assert!(err.contains("loginUltraSuccess"), "error must name the unknown tag: {err}");
    }

    #[test]
    fn missing_detail_field_surfaces_distinctly() {
        // A row written with the right tag but a stripped detail
        // (data-loss event) must error rather than reconstruct a
        // default-valued variant.
        let err = SecurityAuditAction::from_storage("loginFailure", &serde_json::json!({}))
            .expect_err("missing detail.reason must fail");
        assert!(err.contains("reason"), "error must name the missing field: {err}");
    }

    #[test]
    fn detail_json_carries_inline_payload() {
        let action = SecurityAuditAction::ShareRevoked {
            doc_id: "doc1".to_string(),
            target: "bob@example.com".to_string(),
        };
        let detail = action.detail_json();
        // camelCase to match the outer AuditEntry shape — see
        // detail_json doc comment for the wire-shape rationale.
        assert_eq!(detail["docId"], "doc1");
        assert_eq!(detail["target"], "bob@example.com");
    }

    #[test]
    fn doc_lock_toggled_round_trips_through_storage() {
        // #140: the lock-toggle audit row survives the tag + detail round-trip.
        let action = SecurityAuditAction::DocLockToggled {
            doc_id: "doc1".to_string(),
            locked: true,
        };
        assert_eq!(action.as_str(), "docLockToggled");
        let detail = action.detail_json();
        assert_eq!(detail["docId"], "doc1");
        assert_eq!(detail["locked"], true);
        let back = SecurityAuditAction::from_storage(action.as_str(), &detail)
            .expect("round-trip");
        assert_eq!(back, action);
    }

    #[test]
    fn account_linked_round_trips_through_storage() {
        // gap-003: cross-provider link audit row survives tag + detail round-trip.
        let action = SecurityAuditAction::AccountLinked {
            from_provider: "github".to_string(),
            to_provider: "google".to_string(),
        };
        assert_eq!(action.as_str(), "accountLinked");
        let detail = action.detail_json();
        assert_eq!(detail["fromProvider"], "github");
        assert_eq!(detail["toProvider"], "google");
        let back = SecurityAuditAction::from_storage(action.as_str(), &detail)
            .expect("round-trip");
        assert_eq!(back, action);
    }

    #[test]
    fn from_storage_accepts_legacy_snake_case_detail() {
        // Defends the backward-compat fallback in `from_storage`:
        // rows written before 2026-05-18 carried snake_case detail
        // keys. They must remain readable forever — there's no
        // migration step.
        let detail = serde_json::json!({ "doc_id": "doc1", "target": "bob" });
        let action = SecurityAuditAction::from_storage("shareRevoked", &detail)
            .expect("legacy snake_case row must parse");
        match action {
            SecurityAuditAction::ShareRevoked { doc_id, target } => {
                assert_eq!(doc_id, "doc1");
                assert_eq!(target, "bob");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn detail_json_is_empty_for_unit_variants() {
        let action = SecurityAuditAction::LoginSuccess;
        let detail = action.detail_json();
        assert_eq!(detail, serde_json::json!({}));
    }
}
