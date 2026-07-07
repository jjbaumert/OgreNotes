// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use serde::{Deserialize, Serialize};

use super::NotifEmailPref;

/// AES-256-GCM ciphertext blob stored alongside the MFA secret.
/// The auth crate (`crates/auth/src/mfa.rs`) re-exports this type and
/// owns the encrypt/decrypt operations. The fields are publicly
/// visible base64 strings (URL-safe, no padding); the plaintext is
/// not reachable from this side of the L2↔L3 boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncryptedString {
    /// 12-byte AES-GCM nonce. Generated fresh at every encrypt;
    /// reusing one across two ciphertexts under the same key
    /// destroys GCM's confidentiality.
    pub nonce: String,
    /// Ciphertext + auth tag, concatenated as a single blob.
    pub ct: String,
}

/// OAuth provider that originally minted the credentials bound to a
/// particular `User` row. Stored so that a second login via a DIFFERENT
/// provider with the same email cannot silently take over the account.
///
/// `Unknown` represents legacy rows written before provider tracking
/// existed; the first successful login upgrades those rows to the live
/// provider (see `find_or_create_user`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthProvider {
    Github,
    Google,
    /// Phase 4 M-E4: user provisioned via a workspace's SAML IdP.
    /// The IdP NameID is stored in `provider_subject_id`; the
    /// stable external identity used for dedupe on subsequent logins
    /// is `external_id` (per the JIT branch in
    /// `crates/auth/src/user.rs::find_or_create_user`).
    Saml,
    /// Used by `POST /auth/dev-login` in test / dev environments.
    Dev,
    /// Legacy row, no provider recorded. Treated as "accept and upgrade".
    Unknown,
}

impl Default for AuthProvider {
    fn default() -> Self {
        AuthProvider::Unknown
    }
}

/// Global authorization tier for a User. Ordered: variants compare as
/// `User < Admin`, so `role >= UserRole::Admin` is the canonical
/// admin-check used by `User::is_admin()`.
///
/// Phase 4 ships `User` and `Admin` only. Custom admin profiles
/// (`SuperAdmin`, `AuditViewer`, etc.) are a v2 carry-forward; adding
/// a new variant slots in cleanly because storage is a string and the
/// derived `is_admin()` keeps every existing call site working.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    #[default]
    User,
    Admin,
}

/// User-level light/dark theme preference (Phase 5 M-P1 piece B).
/// `System` follows the browser's `prefers-color-scheme`; `Light` /
/// `Dark` force a specific theme regardless of system pref.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

/// #148 — AI-assistant access policy on the User row. Replaces the
/// prior `ask_enabled: bool` gate with a three-state model so the
/// operator can enable ask without also handing the user an escape
/// hatch out of the operator's cost caps.
///
/// - `Disabled`: `/api/v1/ask` returns 403. Admin (`role >= Admin`)
///   bypasses this state.
/// - `SystemOnly`: user can ask, but only via the operator's
///   Anthropic key. The `x-anthropic-key` (BYOK) header is
///   rejected with 400 so the frontend can hide/disable its BYOK
///   input under this policy. Admin bypasses the BYOK rejection.
/// - `SystemOrByok`: user can ask via the operator's key OR
///   provide their own BYOK key that bypasses operator quotas +
///   costs. Matches the pre-existing `ask_enabled = true`
///   behavior — the migration target for legacy rows with
///   `ask_enabled: true`.
///
/// Default = `Disabled` — matches the pre-existing default for
/// new production users (admin must opt them in).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AskPolicy {
    #[default]
    Disabled,
    SystemOnly,
    SystemOrByok,
}

/// User-level UI preferences (Phase 5 M-P1 piece B). Stored as a
/// JSON-stringified blob on the User row (`ui_prefs` attribute);
/// surfaced verbatim through `GET /users/me` and updated via
/// `PUT /users/me/prefs`.
///
/// Fields are individually optional + default so a partial write
/// from the frontend (e.g. only the `theme` field) can be merged
/// without losing the other preferences server-side. The PUT
/// handler performs a server-side merge against the existing
/// stored prefs.
///
/// Phase 5 milestones consume these in order:
///   - M-P1 piece C: `theme` (light/dark toggle UI)
///   - M-P2:         `locale` (i18n locale switcher)
///   - M-P3:         `doc_theme` (typography theme selector)
///   - M-P8:         `dyslexic_font`, `reduce_motion` (a11y controls)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UiPrefs {
    /// Light / dark / follow-system. `None` ⇒ follow system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemePref>,
    /// Per-document typography theme id (e.g. "default", "editorial").
    /// Freeform string in v1 — promoted to an enum in M-P3 when the
    /// theme catalog ships.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_theme: Option<String>,
    /// Substitute the OpenDyslexic font face for `--font-doc-body`
    /// across the user's editor surfaces.
    ///
    /// `Option<bool>` (rather than `bool`) so the PUT-merge in
    /// `routes/users.rs::put_ui_prefs` can distinguish "leave
    /// unchanged" (field absent / null) from "set to false"
    /// (explicit `false`). The previous `bool` shape silently
    /// flipped this to `false` on any partial PUT that didn't
    /// echo the field — a real a11y-pref regression risk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dyslexic_font: Option<bool>,
    /// Force `prefers-reduced-motion: reduce` behavior even when the
    /// OS pref isn't set. Wired in M-P8. Same `Option<bool>`
    /// rationale as `dyslexic_font` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduce_motion: Option<bool>,
    /// BCP-47 locale tag (e.g. "en-US", "ar"). `None` ⇒ fall back
    /// to `navigator.language` then en-US. Wired in M-P2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
}

/// A user's self-set status (account-menu step 5) — e.g.
/// "🌴 on vacation". Stored as a JSON-stringified blob on the User
/// row (`status` attribute), surfaced through `GET /users/me`, and
/// set via `PUT /users/me/status`. Self-visible only in v1: there is
/// no presence broadcast to collaborators yet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserStatus {
    /// Short free-text label.
    pub text: String,
    /// Optional leading emoji.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// Epoch microseconds after which the status auto-expires. `None`
    /// ⇒ sticks until explicitly cleared. Honored read-side (a status
    /// past this reads as absent), so there is no expiry sweeper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

impl UserStatus {
    /// True when `expires_at` is set and at/before `now` (epoch
    /// microseconds). Read paths drop expired statuses rather than
    /// relying on a background sweep.
    pub fn is_expired(&self, now_usec: i64) -> bool {
        matches!(self.expires_at, Some(exp) if exp <= now_usec)
    }
}

/// User profile stored in DynamoDB.
/// PK: USER#<user_id>, SK: PROFILE
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct User {
    pub user_id: String,
    pub name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,

    /// OAuth provider that minted this user's credentials. Guards against
    /// cross-provider account hijack via shared email addresses.
    #[serde(default)]
    pub provider: AuthProvider,
    /// Provider-specific subject / user id. Opaque to us; only compared
    /// for equality on subsequent logins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_subject_id: Option<String>,

    // System folder IDs (created on first login)
    pub home_folder_id: String,
    pub private_folder_id: String,
    pub trash_folder_id: String,
    /// Archive folder (Phase 2): documents removed from active view but not deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_folder_id: Option<String>,
    /// Pinned folder (Phase 2): starred/favorited documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_folder_id: Option<String>,

    /// Default workspace for this user. Created on first login; scopes
    /// documents that do not specify a workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace_id: Option<String>,

    /// AES-256-GCM-encrypted TOTP secret. `None` until enroll. The
    /// plaintext (a Base32 20-byte secret) is only decrypted at TOTP
    /// verification time via `crates/auth/src/mfa::decrypt`; no other
    /// code path should reach into this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mfa_secret: Option<EncryptedString>,
    /// Timestamp at which the user verified their TOTP secret and
    /// finalized enrollment. `None` until a successful
    /// `POST /auth/mfa/verify`. Gates the "MFA required" branch in
    /// the login flow — pre-enrollment users skip the challenge step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mfa_enrolled_at: Option<i64>,

    /// External identity-provider id used to dedupe SCIM-provisioned
    /// users and SAML-JIT-created users. Opaque to us; SCIM clients
    /// supply it via the `externalId` field on the User resource, SAML
    /// stores the IdP NameID here. When `Some`, the row participates
    /// in the sparse `GSI6-external-id` index so SCIM
    /// `?filter=externalId eq "x"` lookups stay O(1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,

    /// Global authorization tier. `User` is the default for newly
    /// created rows; `Admin` is granted via `POST /admin/users/:id/promote`
    /// or by appearing in `AppConfig::admin_emails`. No `#[serde(default)]`
    /// — every row must carry an explicit `role` after the Phase 4 M-E1
    /// backfill; legacy rows missing the attribute deserialize as an
    /// error.
    pub role: UserRole,
    /// Soft-disabled: blocks login without deleting data.
    #[serde(default)]
    pub is_disabled: bool,
    /// #148 — AI-assistant access policy. Three states:
    /// `Disabled` / `SystemOnly` / `SystemOrByok` (see
    /// `AskPolicy` doc). Defaults to `Disabled` for newly-created
    /// production users; admin flips via
    /// `PUT /api/v1/admin/users/:id/ask-policy`. Admins bypass
    /// `Disabled` outright; `SystemOnly` also lets admins bring
    /// their own key.
    ///
    /// Read as `Option<...>` so DDB rows written before this
    /// field existed still deserialize; the `ask_policy()`
    /// getter fills in the effective value by deriving from the
    /// legacy `ask_enabled` attribute when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_policy: Option<AskPolicy>,

    /// Legacy: pre-`ask_policy` two-state gate. Kept in the
    /// struct so DDB rows written before the policy migration
    /// still deserialize into a valid User; the write path never
    /// emits this field again (see `set_ask_policy` on
    /// `UserRepo`, which explicitly REMOVEs the legacy attribute
    /// on the write it makes). Read via `ask_policy()` — do NOT
    /// consume this field directly outside of that getter.
    #[serde(default, rename = "ask_enabled")]
    pub legacy_ask_enabled: bool,

    /// Email notification preference.
    #[serde(default)]
    pub email_notifications: NotifEmailPref,
    /// UI preferences (theme, locale, accessibility). `None` ⇒ user
    /// has not customized any preference yet; the frontend uses the
    /// system / inferred defaults until they save a value. Phase 5
    /// M-P1 piece B introduces the field; subsequent M-P milestones
    /// wire individual fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_prefs: Option<UiPrefs>,
    /// Self-set presence status (account-menu step 5). `None` ⇒ no
    /// status. Stored as a JSON blob; read paths honor `expires_at`
    /// so an expired status reads as absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<UserStatus>,
    /// Last time the user was active in-app (for email suppression).
    #[serde(default)]
    pub last_active_at: i64,

    pub created_at: i64,
    pub updated_at: i64,
}

impl User {
    pub fn pk(&self) -> String {
        format!("USER#{}", self.user_id)
    }

    pub fn sk() -> &'static str {
        "PROFILE"
    }

    /// Derived admin check. `true` when `role >= UserRole::Admin`.
    /// Preserved as a method (not a field) so every existing call site
    /// (`user.is_admin()`) keeps reading even though storage now carries
    /// the `role` enum.
    pub fn is_admin(&self) -> bool {
        self.role >= UserRole::Admin
    }

    /// #148 — effective AI-assistant policy. Returns the explicit
    /// `ask_policy` field when set; otherwise derives from the
    /// legacy `ask_enabled` bool: `true` → `SystemOrByok`
    /// (most-open, matches pre-migration behavior), `false` →
    /// `Disabled`. Once every DDB row has been re-written the
    /// legacy field will be dead and this fallback can be
    /// removed.
    pub fn ask_policy(&self) -> AskPolicy {
        self.ask_policy.unwrap_or_else(|| {
            if self.legacy_ask_enabled {
                AskPolicy::SystemOrByok
            } else {
                AskPolicy::Disabled
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogrenotes_common::id::new_id;
    use ogrenotes_common::time::now_usec;

    fn sample_user() -> User {
        let now = now_usec();
        User {
            user_id: new_id(),
            name: "Test User".to_string(),
            email: "test@example.com".to_string(),
            avatar_url: Some("https://example.com/avatar.png".to_string()),
            provider: AuthProvider::Unknown,
            provider_subject_id: None,
            home_folder_id: new_id(),
            private_folder_id: new_id(),
            trash_folder_id: new_id(),
            archive_folder_id: Some(new_id()),
            pinned_folder_id: Some(new_id()),
            default_workspace_id: Some(new_id()),
            mfa_secret: None,
            mfa_enrolled_at: None,
            external_id: None,
            role: UserRole::User,
            is_disabled: false,
            ask_policy: None,
            legacy_ask_enabled: false,
            email_notifications: NotifEmailPref::default(),
            ui_prefs: None,
            status: None,
            last_active_at: 0,
            created_at: now,
            updated_at: now,
        }
    }

    fn status_at(expires_at: Option<i64>) -> UserStatus {
        UserStatus { text: "ok".to_string(), emoji: None, expires_at }
    }

    #[test]
    fn status_without_expiry_never_expires() {
        assert!(!status_at(None).is_expired(i64::MAX));
    }

    #[test]
    fn status_not_expired_before_deadline() {
        assert!(!status_at(Some(1_000)).is_expired(999));
    }

    #[test]
    fn status_expired_at_exact_deadline() {
        // Boundary: the instant of expiry counts as expired (`<=`).
        assert!(status_at(Some(1_000)).is_expired(1_000));
    }

    #[test]
    fn status_expired_after_deadline() {
        assert!(status_at(Some(1_000)).is_expired(1_001));
    }

    #[test]
    fn user_pk_format() {
        let user = sample_user();
        assert!(user.pk().starts_with("USER#"));
        assert_eq!(user.pk(), format!("USER#{}", user.user_id));
    }

    #[test]
    fn user_sk_format() {
        assert_eq!(User::sk(), "PROFILE");
    }

    #[test]
    fn user_json_roundtrip() {
        let user = sample_user();
        let json = serde_json::to_string(&user).unwrap();
        let back: User = serde_json::from_str(&json).unwrap();
        assert_eq!(user, back);
    }

    #[test]
    fn auth_provider_saml_serializes_lowercase() {
        // The wire shape is the lowercase variant tag. SAML provider
        // rows land alongside github/google/dev under the same
        // `provider` column on the User row; admins grepping logs
        // for `provider="saml"` rely on this string staying stable.
        assert_eq!(
            serde_json::to_string(&AuthProvider::Saml).unwrap(),
            "\"saml\""
        );
        let back: AuthProvider = serde_json::from_str("\"saml\"").unwrap();
        assert_eq!(back, AuthProvider::Saml);
    }

    #[test]
    fn user_avatar_url_optional() {
        let mut user = sample_user();
        user.avatar_url = None;
        let json = serde_json::to_string(&user).unwrap();
        assert!(!json.contains("avatar_url"));
        let back: User = serde_json::from_str(&json).unwrap();
        assert_eq!(back.avatar_url, None);
    }

    #[test]
    fn user_role_serializes_lowercase() {
        // Stored as plain lowercase strings — "user" / "admin" — same
        // wire shape as the existing `provider` field.
        assert_eq!(
            serde_json::to_string(&UserRole::User).unwrap(),
            "\"user\""
        );
        assert_eq!(
            serde_json::to_string(&UserRole::Admin).unwrap(),
            "\"admin\""
        );
    }

    #[test]
    fn user_role_orders_admin_above_user() {
        // `is_admin()` derivation depends on this ordering; if a future
        // variant lands above `Admin` the derivation still holds for
        // the existing tier.
        assert!(UserRole::Admin > UserRole::User);
    }

    #[test]
    fn user_is_admin_derives_from_role() {
        let mut user = sample_user();
        user.role = UserRole::User;
        assert!(!user.is_admin());
        user.role = UserRole::Admin;
        assert!(user.is_admin());
    }

    #[test]
    fn user_external_id_round_trips_when_set() {
        let mut user = sample_user();
        user.external_id = Some("scim-user-42".to_string());
        let json = serde_json::to_string(&user).unwrap();
        assert!(json.contains("\"external_id\":\"scim-user-42\""));
        let back: User = serde_json::from_str(&json).unwrap();
        assert_eq!(back.external_id.as_deref(), Some("scim-user-42"));
    }

    #[test]
    fn user_external_id_omitted_when_none() {
        let user = sample_user();
        assert!(user.external_id.is_none());
        let json = serde_json::to_string(&user).unwrap();
        // Sparse GSI semantics: a missing external_id must not emit
        // the attribute at all, otherwise pre-SCIM users would land
        // in the index with a NULL key (DynamoDB would reject).
        assert!(!json.contains("external_id"), "external_id should be absent");
    }

    #[test]
    fn user_missing_role_field_fails_to_deserialize() {
        // Hard-migration guard: legacy JSON without `role` must NOT
        // silently default. Mirrors the `RepoError::MissingField("role")`
        // path on the DynamoDB read side.
        let mut value = serde_json::to_value(sample_user()).unwrap();
        value.as_object_mut().unwrap().remove("role");
        let res: Result<User, _> = serde_json::from_value(value);
        assert!(res.is_err(), "User without `role` must fail deserialize");
    }

    // ── #148 AskPolicy derivation + serde ─────────────────────

    #[test]
    fn ask_policy_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&AskPolicy::Disabled).unwrap(),
            "\"disabled\""
        );
        assert_eq!(
            serde_json::to_string(&AskPolicy::SystemOnly).unwrap(),
            "\"system_only\""
        );
        assert_eq!(
            serde_json::to_string(&AskPolicy::SystemOrByok).unwrap(),
            "\"system_or_byok\""
        );
    }

    #[test]
    fn ask_policy_default_is_disabled() {
        // New production users must default to Disabled — an admin
        // has to opt them in explicitly. This mirrors the
        // pre-migration `ask_enabled: false` default.
        assert_eq!(AskPolicy::default(), AskPolicy::Disabled);
    }

    #[test]
    fn user_ask_policy_returns_explicit_field_when_set() {
        let mut user = sample_user();
        user.ask_policy = Some(AskPolicy::SystemOnly);
        user.legacy_ask_enabled = true; // explicit field wins
        assert_eq!(user.ask_policy(), AskPolicy::SystemOnly);
    }

    #[test]
    fn user_ask_policy_derives_from_legacy_true() {
        // Pre-migration row with `ask_enabled: true` and no
        // `ask_policy` attribute → SystemOrByok (most-open;
        // preserves pre-migration behavior).
        let mut user = sample_user();
        user.ask_policy = None;
        user.legacy_ask_enabled = true;
        assert_eq!(user.ask_policy(), AskPolicy::SystemOrByok);
    }

    #[test]
    fn user_ask_policy_derives_from_legacy_false() {
        // Pre-migration row with `ask_enabled: false` and no
        // `ask_policy` → Disabled.
        let mut user = sample_user();
        user.ask_policy = None;
        user.legacy_ask_enabled = false;
        assert_eq!(user.ask_policy(), AskPolicy::Disabled);
    }

    #[test]
    fn user_ask_policy_field_absent_serializes_without_key() {
        // `Option<AskPolicy>::None` on the field with
        // `skip_serializing_if = "Option::is_none"` means new
        // writes for a legacy user don't accidentally stamp the
        // field with a default value that might not match the
        // derived legacy semantics.
        let user = sample_user();
        assert!(user.ask_policy.is_none());
        let json = serde_json::to_string(&user).unwrap();
        assert!(!json.contains("\"ask_policy\""), "ask_policy should be absent when None");
    }
}
