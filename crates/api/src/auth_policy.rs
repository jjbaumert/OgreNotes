// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Cross-route authentication policy helpers. Logic that several
//! route modules need but that has no home in a domain crate (it
//! composes a repo call with a model check, which is an L4
//! concern). Lifted here so two sibling route modules don't
//! cross-import each other's `pub(crate)` helpers — the
//! maintenance hazard the M-E3 piece-D review flagged.

use ogrenotes_storage::models::user::{User, UserRole};

use crate::state::AppState;

/// Promote `user` to Admin if their email is on the `admin_emails`
/// allowlist and they aren't already an admin. Mutates `user.role` in
/// place so the caller's subsequent session-mint reflects the promotion,
/// and best-effort-persists the role change (a failed write is logged by
/// the repo, never aborts the login). Idempotent: a re-promote is a no-op
/// at the storage layer.
///
/// Called from every primary-auth path: the OAuth callback, dev-login,
/// and the SAML ACS handler — previously three verbatim copies that had
/// begun to drift (the SAML copy imported `UserRole` at the use-site).
pub(crate) async fn apply_admin_email_promotion(state: &AppState, user: &mut User) {
    if !user.is_admin() && state.config.admin_emails.contains(&user.email.to_lowercase()) {
        user.role = UserRole::Admin;
        let _ = state.user_repo.set_role(&user.user_id, UserRole::Admin).await;
    }
}

/// Phase 4 M-E3 piece D: check whether the user's default workspace
/// requires MFA AND the user hasn't yet enrolled. Returns
/// `Some(true)` to flag the TokenResponse / UserResponse; `None`
/// for every other path so the wire shape stays absent for the
/// common case (skip_serializing_if_none on the field).
///
/// Scope: only the user's `default_workspace_id` is checked. A user
/// who belongs to multiple workspaces, one of which requires MFA
/// but isn't their default, isn't caught here — that's the v2
/// "any workspace requires it" check, which would need a GSI4
/// walk of their memberships.
///
/// Three callers today: `routes::auth::issue_session_response`
/// (challenge / recovery paths), `routes::auth::dev_login`, and
/// `routes::users::get_me`. The OAuth callback redirects rather
/// than returning JSON, so it doesn't need the helper.
pub(crate) async fn mfa_enrollment_required_for(
    state: &AppState,
    user: &User,
) -> Option<bool> {
    if user.mfa_enrolled_at.is_some() {
        return None;
    }
    let ws_id = user.default_workspace_id.as_deref()?;
    match state.workspace_repo.get(ws_id).await {
        Ok(Some(ws)) if ws.mfa_required => Some(true),
        _ => None,
    }
}
