// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! `EmailService` is the façade every handler hits after creating a
//! `Notification`. `spawn_for_notification` spawns a `tokio` task that
//! walks the full policy pipeline (config flag → email addr → prefs →
//! active-in-app → daily cap → send) and logs each outcome without
//! blocking the calling request.

use std::collections::HashMap;
use std::sync::Arc;

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::notification::Notification;
use ogrenotes_storage::repo::user_repo::UserRepo;

use crate::cap::EmailCapRepo;
use crate::policy::{is_recently_active, should_email_for_prefs};
use crate::sender::{EmailSender, SendError};
use crate::templates;

/// Outcome of a single send attempt. Captured as an enum (rather than
/// bool) so tests can assert *why* a send was skipped, and so ops can pick
/// the right metric label from the same signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// Email delivered to the sender.
    Sent,
    /// `email_enabled` was false. Default in every environment until flipped.
    SkippedDisabled,
    /// Recipient had no email address on file (should be impossible for
    /// OAuth users; defensive).
    SkippedNoAddress,
    /// Recipient's `NotifEmailPref` did not allow this event.
    SkippedPrefs,
    /// Recipient was active in-app within the last 5 minutes.
    SkippedActive,
    /// Recipient is already at today's 25-email cap.
    SkippedCap,
}

pub struct EmailService {
    sender: Arc<dyn EmailSender>,
    user_repo: Arc<UserRepo>,
    cap_repo: Arc<EmailCapRepo>,
    from_addr: String,
    frontend_origin: String,
    /// Keying material for the email-link HMAC (#40). Sourced from
    /// the same `jwt_secret` the auth layer uses; the HMAC input
    /// carries a `notif:v1:` domain separator so the two surfaces
    /// can't be cross-confused even though they share a key.
    notif_secret: Vec<u8>,
    enabled: bool,
}

impl EmailService {
    pub fn new(
        sender: Arc<dyn EmailSender>,
        user_repo: Arc<UserRepo>,
        cap_repo: Arc<EmailCapRepo>,
        from_addr: String,
        frontend_origin: String,
        notif_secret: Vec<u8>,
        enabled: bool,
    ) -> Self {
        Self {
            sender,
            user_repo,
            cap_repo,
            from_addr,
            frontend_origin,
            notif_secret,
            enabled,
        }
    }

    /// Fire-and-forget entry point for route handlers. Spawns a tokio task
    /// that drives `try_send`; errors are traced but never surfaced to the
    /// caller (so a failed SMTP hop never 500s a user-facing request).
    pub fn spawn_for_notification(
        self: &Arc<Self>,
        notif: Notification,
        is_direct: bool,
    ) {
        let svc = self.clone();
        tokio::spawn(async move {
            match svc.try_send(&notif, is_direct).await {
                Ok(outcome) => {
                    tracing::debug!(
                        user_id = notif.user_id.as_str(),
                        notif_id = notif.notif_id.as_str(),
                        outcome = ?outcome,
                        "email send outcome",
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        user_id = notif.user_id.as_str(),
                        notif_id = notif.notif_id.as_str(),
                        error = %e,
                        "email send failed",
                    );
                }
            }
        });
    }

    /// Run the full policy pipeline and — if every gate passes — deliver
    /// the email. The pipeline is ordered cheapest → costliest so the
    /// common "skip" paths exit before hitting DynamoDB / SMTP.
    pub async fn try_send(
        &self,
        notif: &Notification,
        is_direct: bool,
    ) -> Result<SendOutcome, SendError> {
        if !self.enabled {
            return Ok(SendOutcome::SkippedDisabled);
        }

        let user = match self.user_repo.get_by_id(&notif.user_id).await {
            Ok(Some(u)) => u,
            _ => return Ok(SendOutcome::SkippedNoAddress),
        };
        if user.email.is_empty() {
            return Ok(SendOutcome::SkippedNoAddress);
        }
        if !should_email_for_prefs(user.email_notifications, &notif.notif_type, is_direct) {
            return Ok(SendOutcome::SkippedPrefs);
        }
        if is_recently_active(user.last_active_at, now_usec()) {
            return Ok(SendOutcome::SkippedActive);
        }

        // Reserve a slot in today's cap *before* we render or dial out, so
        // a cap-exhausted user costs nothing beyond one DynamoDB update.
        let cap_reserved = self
            .cap_repo
            .increment_if_under_cap(&user.user_id)
            .await
            .map_err(|e| SendError::Cap(e.to_string()))?;
        if !cap_reserved {
            return Ok(SendOutcome::SkippedCap);
        }

        // Actor display name is best-effort; fall back to the id so a
        // deleted actor row doesn't block delivery.
        let actor_name = match self.user_repo.get_by_id(&notif.actor_id).await {
            Ok(Some(u)) => u.name,
            _ => notif.actor_id.clone(),
        };

        let exp_unix = (now_usec() / 1_000_000) + templates::NOTIF_LINK_TTL_SECS;
        let rendered = templates::render(
            notif,
            &actor_name,
            &self.frontend_origin,
            &self.notif_secret,
            exp_unix,
        );
        self.sender
            .send(
                &self.from_addr,
                &user.email,
                &rendered.subject,
                &rendered.html,
                &rendered.text,
            )
            .await?;
        Ok(SendOutcome::Sent)
    }

    /// Send a digest email covering `notifs` to `user_id`. Follows the
    /// same disabled → address → prefs → cap pipeline as per-event sends,
    /// but deliberately **skips the "active in-app" check** — digests
    /// are for inactive users by definition, and the scheduler already
    /// filters on `last_active_at`. An empty `notifs` slice returns
    /// `SendOutcome::SkippedPrefs` (no-op).
    pub async fn try_send_digest(
        &self,
        user_id: &str,
        notifs: &[Notification],
    ) -> Result<SendOutcome, SendError> {
        if !self.enabled {
            return Ok(SendOutcome::SkippedDisabled);
        }
        if notifs.is_empty() {
            return Ok(SendOutcome::SkippedPrefs);
        }

        let user = match self.user_repo.get_by_id(user_id).await {
            Ok(Some(u)) => u,
            _ => return Ok(SendOutcome::SkippedNoAddress),
        };
        if user.email.is_empty() {
            return Ok(SendOutcome::SkippedNoAddress);
        }
        // Digest honors the "Disabled" pref but ignores the
        // MentionsOnly/All distinction — if a user wants *any* email,
        // they want the digest too.
        if matches!(
            user.email_notifications,
            ogrenotes_storage::models::NotifEmailPref::Disabled
        ) {
            return Ok(SendOutcome::SkippedPrefs);
        }

        let cap_reserved = self
            .cap_repo
            .increment_if_under_cap(&user.user_id)
            .await
            .map_err(|e| SendError::Cap(e.to_string()))?;
        if !cap_reserved {
            return Ok(SendOutcome::SkippedCap);
        }

        // Resolve actor names once up front so the render loop doesn't
        // re-fetch the same user row N times.
        let mut actor_names: HashMap<String, String> = HashMap::new();
        for n in notifs {
            if actor_names.contains_key(&n.actor_id) {
                continue;
            }
            let name = match self.user_repo.get_by_id(&n.actor_id).await {
                Ok(Some(u)) => u.name,
                _ => n.actor_id.clone(),
            };
            actor_names.insert(n.actor_id.clone(), name);
        }

        let exp_unix = (now_usec() / 1_000_000) + templates::NOTIF_LINK_TTL_SECS;
        let Some(rendered) = templates::render_digest(
            notifs,
            &actor_names,
            &self.frontend_origin,
            &self.notif_secret,
            exp_unix,
        ) else {
            return Ok(SendOutcome::SkippedPrefs);
        };
        self.sender
            .send(
                &self.from_addr,
                &user.email,
                &rendered.subject,
                &rendered.html,
                &rendered.text,
            )
            .await?;
        Ok(SendOutcome::Sent)
    }
}

#[cfg(test)]
mod tests {
    // Only the pure policy branches are testable at the unit level —
    // `try_send` depends on `UserRepo` and `EmailCapRepo`, both of which
    // are DynamoDB-backed. The `SkippedDisabled` gate fires before any
    // repo access, so we can verify it without a DynamoDB harness. The
    // remaining `SendOutcome` branches are covered by the integration
    // tests under `crates/api/tests/`.

    use super::*;
    use async_trait::async_trait;
    use ogrenotes_storage::dynamo::DynamoClient;
    use ogrenotes_storage::models::notification::{NotifType, Notification};
    use std::sync::Mutex;

    struct CaptureSender(Mutex<Vec<(String, String, String)>>);

    #[async_trait]
    impl EmailSender for CaptureSender {
        async fn send(
            &self,
            _from: &str,
            to: &str,
            subject: &str,
            _html: &str,
            text: &str,
        ) -> Result<(), SendError> {
            self.0
                .lock()
                .unwrap()
                .push((to.to_string(), subject.to_string(), text.to_string()));
            Ok(())
        }
    }

    fn sample_notif() -> Notification {
        Notification {
            notif_id: "n1".into(),
            user_id: "u1".into(),
            notif_type: NotifType::Shared,
            doc_id: Some("d1".into()),
            thread_id: None,
            actor_id: "actor".into(),
            message: "shared".into(),
            preview: None,
            block_id: None,
            read: false,
            created_at: 0,
        }
    }

    /// Builds an EmailService whose repos point at an unreachable
    /// DynamoDB endpoint. Only safe for branches that return *before* any
    /// repo access — the `enabled` gate, and (for digests) the empty-slice
    /// gate. The full pipeline is exercised by the api integration tests.
    fn build_service(enabled: bool) -> Arc<EmailService> {
        // Local-only AWS config so the client is cheap to construct; it
        // is never called because the gates under test return first.
        let aws_config = aws_sdk_dynamodb::Config::builder()
            .region(aws_sdk_dynamodb::config::Region::new("us-east-1"))
            .endpoint_url("http://127.0.0.1:1")
            .credentials_provider(aws_sdk_dynamodb::config::Credentials::new(
                "fake", "fake", None, None, "test",
            ))
            .behavior_version(aws_sdk_dynamodb::config::BehaviorVersion::latest())
            .build();
        let client = aws_sdk_dynamodb::Client::from_conf(aws_config);
        let dyn_client = DynamoClient::new(client, "t".into());
        let sender = Arc::new(CaptureSender(Mutex::new(Vec::new())));
        let user_repo = Arc::new(UserRepo::new(dyn_client.clone()));
        let cap_repo = Arc::new(EmailCapRepo::new(dyn_client, 25));
        Arc::new(EmailService::new(
            sender,
            user_repo,
            cap_repo,
            "no-reply@test".into(),
            "https://app.test".into(),
            b"test-secret".to_vec(),
            enabled,
        ))
    }

    #[tokio::test]
    async fn disabled_short_circuits_before_any_repo_call() {
        let svc = build_service(false);
        let outcome = svc
            .try_send(&sample_notif(), true)
            .await
            .expect("disabled gate should never error");
        assert_eq!(outcome, SendOutcome::SkippedDisabled);
    }

    #[tokio::test]
    async fn digest_disabled_short_circuits_before_any_repo_call() {
        // try_send_digest must honor the same `enabled` gate as try_send,
        // returning before it ever touches the (unreachable) user_repo.
        let svc = build_service(false);
        let outcome = svc
            .try_send_digest("u1", &[sample_notif()])
            .await
            .expect("disabled gate should never error");
        assert_eq!(outcome, SendOutcome::SkippedDisabled);
    }

    #[tokio::test]
    async fn digest_empty_slice_skips_without_repo_access() {
        // Enabled service, but an empty digest must return SkippedPrefs at
        // the empty-slice gate — which sits before the first user_repo
        // lookup, so the unreachable DynamoDB endpoint is never dialed.
        let svc = build_service(true);
        let outcome = svc
            .try_send_digest("u1", &[])
            .await
            .expect("empty-digest gate should never error");
        assert_eq!(outcome, SendOutcome::SkippedPrefs);
    }

    #[tokio::test]
    async fn user_lookup_failure_degrades_to_skipped_no_address() {
        // One step past the pure gates: with the service enabled, the
        // recipient lookup actually dials the unreachable endpoint
        // (127.0.0.1:1 → immediate connection refused, still hermetic)
        // and errors. The `_` arm in try_send must swallow that repo
        // error into SkippedNoAddress — a DynamoDB outage degrades to
        // "no email" rather than bubbling an Err out of the pipeline.
        let svc = build_service(true);
        let outcome = svc
            .try_send(&sample_notif(), true)
            .await
            .expect("repo failure must not surface as SendError");
        assert_eq!(outcome, SendOutcome::SkippedNoAddress);
    }
}

