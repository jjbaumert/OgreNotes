// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Fire-and-forget SecurityAudit writers shared across route modules.
//!
//! These helpers were originally authored in `routes/mfa.rs` (the MFA
//! handlers were the first callers) but have no dependency on MFA state —
//! `auth`, `sharing`, `documents`, `scim`, `saml`, `users`, and the
//! `trash_cleanup` worker all emit security-audit rows through them. They
//! live here so every call site imports from a module named for what the
//! helpers do (audit) rather than where they happened to be written.

use ogrenotes_storage::models::security_audit::{SecurityAudit, SecurityAuditAction};

use crate::state::AppState;

/// Spawn a SecurityAudit write for a self-event (subject == actor).
/// Login, MFA enroll/verify/disarm, doc-delete-by-owner, etc. fit
/// here — the user the event is about is the same user who caused
/// it. Cross-actor events (admin revokes a share, SCIM token
/// disables a user) must use [`record_security_event_by_actor`].
pub(crate) fn record_security_event(
    state: &AppState,
    user_id: &str,
    action: SecurityAuditAction,
) {
    record_security_event_by_actor(state, user_id, user_id, action);
}

/// Spawn a SecurityAudit write where the actor differs from the
/// subject. The audit row's PK is keyed on `user_id` (the subject
/// — "this is what happened to your access") but `actor_id` is the
/// authenticated principal who triggered the event. Forensics need
/// both: PK-keyed queries answer "what happened to user X," and
/// the actor field answers "who did it." Mirrors the AdminAudit
/// fire-and-forget pattern in `routes/admin.rs::record_admin_action`
/// — the durable log row is best-effort; the structured tracing
/// event in the spawn body is the fallback if DDB is unhealthy.
pub(crate) fn record_security_event_by_actor(
    state: &AppState,
    user_id: &str,
    actor_id: &str,
    action: SecurityAuditAction,
) {
    let repo = state.security_audit_repo.clone();
    let user_id_owned = user_id.to_string();
    let actor_id_owned = actor_id.to_string();
    tracing::info!(
        event_type = "security_audit",
        kind = action.as_str(),
        user_id = %user_id,
        actor_id = %actor_id,
        "security_audit"
    );
    let action_for_log = action.as_str();
    tokio::spawn(async move {
        let row = SecurityAudit {
            audit_id: nanoid::nanoid!(16),
            user_id: user_id_owned.clone(),
            actor_id: actor_id_owned,
            action,
            created_at: ogrenotes_common::time::now_usec(),
        };
        if let Err(e) = repo.create(&row).await {
            tracing::error!(
                error = %e,
                user_id = %user_id_owned,
                kind = action_for_log,
                "security_audit DDB write failed; tracing event is the only record"
            );
        }
    });
}
