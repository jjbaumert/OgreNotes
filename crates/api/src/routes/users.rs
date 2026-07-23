// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use axum::extract::{Query, State};
use axum::routing::{get, put};
use axum::{Json, Router};
use ogrenotes_storage::models::security_audit::SecurityAuditAction;
use ogrenotes_storage::models::user::{UiPrefs, User, UserStatus};
use ogrenotes_storage::models::NotifEmailPref;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// Caps on the self-set status. Generous — the point is bounding the
/// stored blob, not enforcing product copy rules.
const MAX_STATUS_TEXT_LEN: usize = 100;
const MAX_STATUS_EMOJI_LEN: usize = 16;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(get_me).put(put_profile))
        .route("/me/prefs", put(put_ui_prefs))
        .route("/me/status", put(put_status))
        .route("/me/notification-prefs", put(put_notification_prefs))
        .route("/search", get(search_users))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UserResponse {
    user_id: String,
    name: String,
    email: String,
    avatar_url: Option<String>,
    home_folder_id: String,
    private_folder_id: String,
    trash_folder_id: String,
    default_workspace_id: Option<String>,
    /// Whether this user has the admin role. Drives the
    /// `pages/admin/*` route gate on the frontend. The server still
    /// re-enforces `require_admin` on every `/admin/*` request —
    /// this is UX only, never authoritative.
    is_admin: bool,
    /// Phase 4 M-E3 piece D: `Some(true)` when the user's default
    /// workspace has `mfa_required = true` and the user hasn't yet
    /// enrolled. Lets the frontend re-check on every page hydration
    /// (not just at login) and force the enrollment redirect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mfa_enrollment_required: Option<bool>,
    /// Phase 5 M-P1 piece B: UI preferences (theme, locale, etc.).
    /// `None` ⇒ user hasn't customized anything; the frontend uses
    /// inferred defaults (system theme, navigator.language).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ui_prefs: Option<UiPrefs>,
    /// Self-set status (account-menu step 5). Expired statuses are
    /// dropped read-side, so a present value here is always live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<UserStatus>,
    /// Email-notification preference (account-menu step 6). The
    /// notify worker already honors this; the Notifications tab now
    /// surfaces it. Serializes as its lowercase tag ("all" /
    /// "mentionsonly" / "disabled").
    email_notifications: NotifEmailPref,
    created_at: i64,
}

/// GET /users/me -- current user profile.
async fn get_me(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
) -> Result<Json<UserResponse>, ApiError> {
    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    Ok(Json(build_user_response(&state, user).await))
}

/// Build the `/users/me` response DTO from a loaded `User`. Shared by
/// `get_me` and `put_profile` so the two stay wire-identical — both
/// compute `is_admin` and `mfa_enrollment_required` the same way.
async fn build_user_response(state: &AppState, user: User) -> UserResponse {
    let is_admin = user.is_admin();
    let mfa_enrollment_required =
        crate::auth_policy::mfa_enrollment_required_for(state, &user).await;
    // Honor status expiry read-side (no sweeper): a status past its
    // `expires_at` reads as absent.
    let now = ogrenotes_common::time::now_usec();
    let status = user.status.filter(|s| !s.is_expired(now));
    UserResponse {
        user_id: user.user_id,
        name: user.name,
        email: user.email,
        avatar_url: user.avatar_url,
        home_folder_id: user.home_folder_id,
        private_folder_id: user.private_folder_id,
        trash_folder_id: user.trash_folder_id,
        default_workspace_id: user.default_workspace_id,
        is_admin,
        mfa_enrollment_required,
        ui_prefs: user.ui_prefs,
        status,
        email_notifications: user.email_notifications,
        created_at: user.created_at,
    }
}

/// Body for `PUT /users/me`. Both fields optional with partial-merge
/// semantics: a field absent from the body leaves the stored value
/// unchanged. `avatarUrl: ""` clears the avatar; `name` cannot be
/// blanked.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProfileRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    avatar_url: Option<String>,
}

/// Validate + normalize a profile patch into the repo's update shape.
/// Pure (no store access) so the trust-boundary rules are unit-tested
/// in isolation. Returns `(name, avatar_url)` where:
///   - `name`: `Some(capped)` to set, `None` to leave unchanged.
///   - `avatar_url`: `None` leave, `Some(None)` clear, `Some(Some(capped))` set.
///
/// `Err` carries a short message the handler surfaces as a 400. Caps
/// + char-boundary truncation reuse the same helpers as the OAuth
/// profile-sync path so the two write-paths agree on limits.
fn validate_profile_patch(
    req: UpdateProfileRequest,
) -> Result<(Option<String>, Option<Option<String>>), &'static str> {
    // Name: trim, reject blank, cap. A user shouldn't be able to
    // erase their display name (it's their identity surface in the
    // editor header, presence, and share lists).
    let name = match req.name {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Err("name cannot be empty");
            }
            Some(super::auth::truncate_chars(
                trimmed.to_string(),
                super::auth::MAX_NAME_LEN,
            ))
        }
        None => None,
    };

    // Avatar tri-state mapped onto the repo's `Option<Option<&str>>`:
    //   None        ⇒ field absent      ⇒ leave unchanged
    //   Some("")    ⇒ explicit clear    ⇒ remove the attribute
    //   Some(url)   ⇒ set (http(s) only, capped)
    let avatar_url = match req.avatar_url {
        None => None,
        Some(s) if s.trim().is_empty() => Some(None),
        Some(s) => {
            let trimmed = s.trim();
            // Accept http:// as well as https:// — the goal is keeping
            // `javascript:` / `data:` schemes out of the value that
            // later renders as an avatar `src`, not enforcing TLS on
            // the image host (the OAuth avatar-sync path in auth.rs
            // likewise stores provider-supplied http:// URLs).
            if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
                return Err("avatarUrl must be an http(s) URL");
            }
            Some(Some(super::auth::truncate_chars(
                trimmed.to_string(),
                super::auth::MAX_AVATAR_URL_LEN,
            )))
        }
    };

    Ok((name, avatar_url))
}

/// PUT /users/me -- edit the caller's own profile (display name and
/// avatar). Validates + caps the patch, persists only the fields that
/// actually differ from the stored row, and emits a `ProfileUpdated`
/// SecurityAudit row recording *which* fields changed (never the
/// values — see the variant doc). Only the caller's own row is
/// touched: `user_id` comes from the authenticated token, never the
/// body.
async fn put_profile(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<UpdateProfileRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let (name, avatar_url) =
        validate_profile_patch(req).map_err(|m| ApiError::BadRequest(m.to_string()))?;

    let mut user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    // Compare against current values so the audit row reflects what
    // actually changed — not merely which fields the request carried.
    let name_changed = matches!(&name, Some(n) if *n != user.name);
    let avatar_changed = match &avatar_url {
        Some(Some(url)) => user.avatar_url.as_deref() != Some(url.as_str()),
        Some(None) => user.avatar_url.is_some(),
        None => false,
    };

    if name_changed || avatar_changed {
        let now = ogrenotes_common::time::now_usec();
        state
            .user_repo
            .update(
                &user_id,
                name.as_deref(),
                avatar_url.as_ref().map(|inner| inner.as_deref()),
                now,
            )
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        super::audit::record_security_event(
            &state,
            &user_id,
            SecurityAuditAction::ProfileUpdated { name_changed, avatar_changed },
        );

        // Apply the change onto the already-loaded row so the response
        // reflects the new state without a second GET (mirrors
        // `put_ui_prefs` returning the merged value). Safe because the
        // values were capped above to exactly what the repo stored.
        if let Some(n) = name {
            user.name = n;
        }
        match avatar_url {
            Some(Some(url)) => user.avatar_url = Some(url),
            Some(None) => user.avatar_url = None,
            None => {}
        }
    }

    Ok(Json(build_user_response(&state, user).await))
}

/// Body for `PUT /users/me/status`. An absent or blank `text` clears
/// the status; otherwise it sets one. `expiresAt` is epoch
/// microseconds (matching the storage convention); `null` ⇒ sticks
/// until cleared.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetStatusRequest {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
}

/// Normalize a status request into the stored shape. Pure, so the
/// clear-vs-set rule and the caps are unit-testable. Blank text (or
/// absent) ⇒ `None` (clear). Emoji is trimmed; an empty emoji becomes
/// `None`. Text + emoji are capped. `expires_at` passes through — an
/// already-past value simply reads as expired (effectively cleared),
/// which is harmless, so no future-check is enforced here.
fn build_status(req: SetStatusRequest) -> Option<UserStatus> {
    let text = req.text.map(|t| t.trim().to_string()).unwrap_or_default();
    if text.is_empty() {
        return None;
    }
    let text = super::auth::truncate_chars(text, MAX_STATUS_TEXT_LEN);
    let emoji = req
        .emoji
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .map(|e| super::auth::truncate_chars(e, MAX_STATUS_EMOJI_LEN));
    Some(UserStatus { text, emoji, expires_at: req.expires_at })
}

/// PUT /users/me/status -- set or clear the caller's own status. Not
/// a SecurityAudit event: status is transient presence, not an
/// identity/sharing/destructive change. Returns the refreshed
/// `/users/me` view (with read-side expiry applied).
async fn put_status(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<SetStatusRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let status = build_status(req);
    let now = ogrenotes_common::time::now_usec();
    state
        .user_repo
        .set_status(&user_id, status.as_ref(), now)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;
    // Reflect the just-written value locally so the response is
    // consistent even if the read raced a concurrent write; expiry is
    // still applied in build_user_response.
    user.status = status;
    Ok(Json(build_user_response(&state, user).await))
}

/// Body for `PUT /users/me/notification-prefs`. `emailNotifications`
/// is the lowercase `NotifEmailPref` tag ("all" / "mentionsonly" /
/// "disabled"); an invalid value is rejected by the JSON extractor.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetNotificationPrefsRequest {
    email_notifications: NotifEmailPref,
}

/// PUT /users/me/notification-prefs -- set the caller's email
/// notification preference. Not a SecurityAudit event (a delivery
/// preference, not identity/sharing/destructive). The notify worker
/// already reads this field, so the change takes effect for the next
/// notification without further wiring.
async fn put_notification_prefs(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(req): Json<SetNotificationPrefsRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let now = ogrenotes_common::time::now_usec();
    state
        .user_repo
        .set_email_notifications(&user_id, req.email_notifications.clone(), now)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;
    user.email_notifications = req.email_notifications;
    Ok(Json(build_user_response(&state, user).await))
}

/// PUT /users/me/prefs -- update UI preferences (Phase 5 M-P1
/// piece B).
///
/// Body shape is `UiPrefs` with every field optional. The handler
/// performs a server-side merge: any field present in the body
/// overrides the stored value; any field absent (or set to `null`)
/// preserves the stored value. This avoids the round-trip-clobber
/// race where two frontends would each overwrite the other's
/// concurrent change to a different field.
///
/// Returns the merged prefs so the client can confirm its local
/// view matches server state without a second GET.
async fn put_ui_prefs(
    State(state): State<AppState>,
    AuthUser { user_id, .. }: AuthUser,
    Json(patch): Json<UiPrefs>,
) -> Result<Json<UiPrefs>, ApiError> {
    let user = state
        .user_repo
        .get_by_id(&user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound("User not found".to_string()))?;

    // Merge: start from existing (or defaults if first-write); each
    // body field that is Some replaces. Every field is `Option<...>`
    // so absent-or-null in the body means "leave unchanged" — no
    // silent clobbers of a previously-set preference when the
    // frontend sends a partial PUT. (Pre-fix this lost a11y prefs
    // like `dyslexicFont` whenever the user changed theme.)
    let mut merged = user.ui_prefs.unwrap_or_default();
    if patch.theme.is_some() {
        merged.theme = patch.theme;
    }
    if patch.doc_theme.is_some() {
        merged.doc_theme = patch.doc_theme;
    }
    if patch.locale.is_some() {
        merged.locale = patch.locale;
    }
    if patch.dyslexic_font.is_some() {
        merged.dyslexic_font = patch.dyslexic_font;
    }
    if patch.reduce_motion.is_some() {
        merged.reduce_motion = patch.reduce_motion;
    }
    if patch.editor_width.is_some() {
        merged.editor_width = patch.editor_width;
    }

    let now = ogrenotes_common::time::now_usec();
    state
        .user_repo
        .set_ui_prefs(&user_id, &merged, now)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(merged))
}

#[derive(Deserialize)]
struct SearchQuery {
    /// Exact email lookup (used by ShareDialog).
    #[serde(default)]
    email: Option<String>,
    /// Substring search across email and name (used by @menu).
    #[serde(default)]
    q: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    user_id: String,
    name: String,
    email: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    users: Vec<SearchResult>,
}

/// GET /users/search?email=...&q=... -- search for users.
/// Use `email` for exact email lookup, `q` for substring search.
///
/// Results are workspace-scoped: a non-admin caller only sees users
/// they share at least one workspace with. Cross-workspace enumeration
/// (which previously let any authenticated user walk the entire
/// PROFILE partition) is now filtered at the route layer.
///
/// Admin callers bypass the filter — the admin console still needs
/// an unscoped view for cross-workspace user management.
///
/// A non-shared hit returns an empty result rather than a
/// distinguishing "user exists but you can't see them" error —
/// the error itself would be an enumeration oracle.
async fn search_users(
    State(state): State<AppState>,
    AuthUser { user_id, is_admin, .. }: AuthUser,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResponse>, ApiError> {
    // gap-002 from the post-hardening audit — the picker's 250 ms
    // client-side debounce is a UX signal only; a scripted caller
    // bypasses it entirely, and each substring hit fans out one
    // GSI4 query for the workspace-scope check. Rate-limit the
    // whole endpoint (both email-exact and q substring paths) so
    // a compromised token can't drive unbounded query amplification
    // or fast directory enumeration within their workspace scope.
    crate::middleware::rate_limit::enforce(
        &state.redis,
        "user_search",
        &user_id,
        state.config.rate_limit_user_search_per_min,
        60,
    )
    .await?;

    // Resolve caller's workspace set once per request (admin
    // callers skip). Empty set = user in no workspace → all
    // non-admin searches return empty. That's the desired
    // security posture: a user with no workspace membership has
    // no legitimate reason to enumerate the directory.
    let caller_workspaces: Option<std::collections::HashSet<String>> = if is_admin {
        None
    } else {
        let ws = state
            .workspace_repo
            .list_workspaces_for_user(&user_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        Some(ws.into_iter().collect())
    };

    // Exact email lookup takes priority.
    if let Some(email) = query.email {
        let email = email.trim().to_lowercase();
        if email.is_empty() {
            return Ok(Json(SearchResponse { users: Vec::new() }));
        }
        return match state.user_repo.get_by_email(&email).await {
            Ok(Some(user)) => {
                if !shares_workspace_with_caller(
                    &state,
                    caller_workspaces.as_ref(),
                    &user.user_id,
                )
                .await?
                {
                    // Non-shared → indistinguishable from "not found"
                    // to keep the endpoint from doubling as an
                    // existence oracle.
                    return Ok(Json(SearchResponse { users: Vec::new() }));
                }
                Ok(Json(SearchResponse {
                    users: vec![SearchResult {
                        user_id: user.user_id,
                        name: user.name,
                        email: user.email,
                    }],
                }))
            }
            Ok(None) => Ok(Json(SearchResponse { users: Vec::new() })),
            Err(e) => Err(ApiError::Internal(e.to_string())),
        };
    }

    // Substring search across email and name.
    if let Some(q) = query.q {
        let q = q.trim().to_string();
        if q.is_empty() {
            return Ok(Json(SearchResponse { users: Vec::new() }));
        }
        return match state.user_repo.search_users(&q).await {
            Ok(users) => {
                let mut out = Vec::with_capacity(users.len());
                for u in users {
                    if shares_workspace_with_caller(
                        &state,
                        caller_workspaces.as_ref(),
                        &u.user_id,
                    )
                    .await?
                    {
                        out.push(SearchResult {
                            user_id: u.user_id,
                            name: u.name,
                            email: u.email,
                        });
                    }
                }
                Ok(Json(SearchResponse { users: out }))
            }
            Err(e) => Err(ApiError::Internal(e.to_string())),
        };
    }

    Ok(Json(SearchResponse { users: Vec::new() }))
}

/// True when the hit user shares at least one workspace with the
/// caller. `None` in `caller_workspaces` = admin bypass — return
/// true unconditionally.
async fn shares_workspace_with_caller(
    state: &AppState,
    caller_workspaces: Option<&std::collections::HashSet<String>>,
    hit_user_id: &str,
) -> Result<bool, ApiError> {
    let Some(caller_ws) = caller_workspaces else {
        return Ok(true);
    };
    // Short-circuit: caller has no workspaces → nothing to share.
    // Saves a DDB round-trip per hit for the "user in zero
    // workspaces" edge case (fresh signup pre-first-workspace).
    if caller_ws.is_empty() {
        return Ok(false);
    }
    let hit_ws = state
        .workspace_repo
        .list_workspaces_for_user(hit_user_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(sets_intersect(caller_ws, &hit_ws))
}

/// True when the two workspace sets share at least one id. Pure
/// helper split out for unit testing — the async wrapper adds
/// only the DDB fetch and admin bypass.
fn sets_intersect(
    caller_workspaces: &std::collections::HashSet<String>,
    hit_workspaces: &[String],
) -> bool {
    hit_workspaces.iter().any(|w| caller_workspaces.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(name: Option<&str>, avatar: Option<&str>) -> UpdateProfileRequest {
        UpdateProfileRequest {
            name: name.map(str::to_string),
            avatar_url: avatar.map(str::to_string),
        }
    }

    #[test]
    fn name_present_but_blank_is_rejected() {
        assert!(validate_profile_patch(patch(Some(""), None)).is_err());
        assert!(validate_profile_patch(patch(Some("   "), None)).is_err());
    }

    #[test]
    fn name_is_trimmed_and_set() {
        let (name, avatar) = validate_profile_patch(patch(Some("  Alice  "), None)).unwrap();
        assert_eq!(name.as_deref(), Some("Alice"));
        assert!(avatar.is_none(), "absent avatar leaves the field unchanged");
    }

    #[test]
    fn absent_fields_leave_both_unchanged() {
        let (name, avatar) = validate_profile_patch(patch(None, None)).unwrap();
        assert!(name.is_none());
        assert!(avatar.is_none());
    }

    #[test]
    fn name_is_capped_at_the_byte_limit() {
        let long = "a".repeat(crate::routes::auth::MAX_NAME_LEN + 50);
        let (name, _) = validate_profile_patch(patch(Some(&long), None)).unwrap();
        assert_eq!(name.unwrap().len(), crate::routes::auth::MAX_NAME_LEN);
    }

    #[test]
    fn empty_avatar_clears_the_field() {
        let (_, avatar) = validate_profile_patch(patch(None, Some("   "))).unwrap();
        assert_eq!(avatar, Some(None));
    }

    #[test]
    fn https_and_http_avatars_are_set() {
        let (_, https) = validate_profile_patch(patch(None, Some("https://x/a.png"))).unwrap();
        assert_eq!(https, Some(Some("https://x/a.png".to_string())));
        let (_, http) = validate_profile_patch(patch(None, Some("http://x/a.png"))).unwrap();
        assert_eq!(http, Some(Some("http://x/a.png".to_string())));
    }

    #[test]
    fn dangerous_avatar_schemes_are_rejected() {
        assert!(validate_profile_patch(patch(None, Some("javascript:alert(1)"))).is_err());
        assert!(validate_profile_patch(patch(None, Some("data:image/png;base64,AAAA"))).is_err());
        assert!(validate_profile_patch(patch(None, Some("ftp://x/a.png"))).is_err());
    }

    fn status_req(text: Option<&str>, emoji: Option<&str>, expires_at: Option<i64>) -> SetStatusRequest {
        SetStatusRequest {
            text: text.map(str::to_string),
            emoji: emoji.map(str::to_string),
            expires_at,
        }
    }

    #[test]
    fn blank_or_absent_status_text_clears() {
        assert!(build_status(status_req(None, Some("🌴"), None)).is_none());
        assert!(build_status(status_req(Some("   "), None, None)).is_none());
    }

    #[test]
    fn status_text_trimmed_and_emoji_kept() {
        let s = build_status(status_req(Some("  heads down  "), Some(" 🎧 "), Some(42))).unwrap();
        assert_eq!(s.text, "heads down");
        assert_eq!(s.emoji.as_deref(), Some("🎧"));
        assert_eq!(s.expires_at, Some(42));
    }

    #[test]
    fn empty_emoji_becomes_none() {
        let s = build_status(status_req(Some("busy"), Some("   "), None)).unwrap();
        assert!(s.emoji.is_none());
    }

    #[test]
    fn status_text_is_capped() {
        let long = "x".repeat(MAX_STATUS_TEXT_LEN + 50);
        let s = build_status(status_req(Some(&long), None, None)).unwrap();
        assert_eq!(s.text.len(), MAX_STATUS_TEXT_LEN);
    }

    // ── workspace-scoped search filter ─────────────────────

    fn ws_set(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn intersect_shared_workspace_returns_true() {
        let caller = ws_set(&["ws-1", "ws-2"]);
        let hit = vec!["ws-2".into(), "ws-9".into()];
        assert!(sets_intersect(&caller, &hit));
    }

    #[test]
    fn intersect_disjoint_workspaces_returns_false() {
        let caller = ws_set(&["ws-1"]);
        let hit = vec!["ws-2".into(), "ws-3".into()];
        assert!(!sets_intersect(&caller, &hit));
    }

    #[test]
    fn intersect_caller_alone_returns_false() {
        let caller = ws_set(&["ws-1"]);
        let hit: Vec<String> = Vec::new();
        assert!(!sets_intersect(&caller, &hit));
    }

    #[test]
    fn intersect_hit_membership_alone_returns_false() {
        let caller: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let hit = vec!["ws-2".into()];
        assert!(!sets_intersect(&caller, &hit));
    }

    #[test]
    fn intersect_multi_workspace_matches_any_overlap() {
        // Common real-world shape: user in 3 workspaces, hit in 2,
        // overlap on one. Filter must pass.
        let caller = ws_set(&["a", "b", "c"]);
        let hit = vec!["x".into(), "b".into()];
        assert!(sets_intersect(&caller, &hit));
    }
}
