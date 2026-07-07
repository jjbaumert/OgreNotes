// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Backfill helpers for Phase 4 schema migrations.
//!
//! Each public function is callable from a `crates/api/src/bin/`
//! binary (production path) and from an integration test (regression
//! coverage). Keeping the loop in the library and the env-wiring in
//! the binary lets the test exercise the *actual* migration logic
//! against a real DynamoDB table without simulating `main()`.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;

use ogrenotes_common::id::new_id;
use ogrenotes_common::time::now_usec;
use ogrenotes_storage::dynamo::DynamoClient;
use ogrenotes_storage::models::user::UserRole;
use ogrenotes_storage::models::snapshot::DocSnapshot;
use ogrenotes_storage::models::workspace::Workspace;
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::snapshot_repo::SnapshotRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;
use ogrenotes_storage::repo::workspace_repo::WorkspaceRepo;

/// Aggregate counts from one backfill pass. Re-running a completed
/// backfill should produce `set_admin = set_user = 0` and
/// `already_migrated = scanned`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackfillUserRoleStats {
    /// Total PROFILE rows seen.
    pub scanned: usize,
    /// Rows that already carried a `role` attribute (idempotent skip).
    pub already_migrated: usize,
    /// Rows where the legacy `is_admin` bool was true; written as
    /// `role = "admin"`.
    pub set_admin: usize,
    /// Rows where `is_admin` was absent or false; written as
    /// `role = "user"`.
    pub set_user: usize,
    /// Rows whose `set_role` write failed. Surfaces aggregate
    /// breakage without aborting the whole pass — re-running the
    /// backfill picks up retried rows.
    pub errors: usize,
}

/// Page size for the scan. Bounded-batch per the M-E1 spec; small
/// enough to stay under DynamoDB's 1 MB response cap with headroom.
const SCAN_PAGE_SIZE: i32 = 100;

/// Walk every `PROFILE` row, materialize `role = "user"` /
/// `role = "admin"` based on the legacy `is_admin` boolean, and
/// write back via `UserRepo::set_role`.
///
/// Idempotent: rows that already have a `role` attribute are skipped.
/// Pre-migration rows surface ONLY through this raw scan — the
/// `UserRepo::list_all` path silently drops rows that fail
/// `user_from_item`, which now requires `role`. Bypassing the typed
/// read path here is the entire point of this binary.
///
/// `--dry-run` callers pass `dry_run = true`: the function still
/// scans + classifies + logs every decision, but skips the writes.
/// Stats are the same as if the writes had succeeded.
pub async fn run_backfill_user_role(
    user_repo: &UserRepo,
    dynamo: &DynamoClient,
    dry_run: bool,
) -> Result<BackfillUserRoleStats, String> {
    let mut stats = BackfillUserRoleStats::default();
    let mut last_key: Option<HashMap<String, AttributeValue>> = None;

    loop {
        let mut builder = dynamo
            .inner()
            .scan()
            .table_name(dynamo.table_name())
            .filter_expression("SK = :sk")
            .expression_attribute_values(":sk", AttributeValue::S("PROFILE".to_string()))
            .limit(SCAN_PAGE_SIZE);
        if let Some(ref k) = last_key {
            builder = builder.set_exclusive_start_key(Some(k.clone()));
        }

        let result = builder
            .send()
            .await
            .map_err(|e| format!("scan: {}", e.into_service_error()))?;

        for item in result.items.unwrap_or_default() {
            stats.scanned += 1;

            if item.contains_key("role") {
                stats.already_migrated += 1;
                continue;
            }

            let user_id = match item.get("user_id").and_then(|v| v.as_s().ok()) {
                Some(id) => id.clone(),
                None => {
                    // No user_id on a PROFILE row is a schema invariant
                    // violation — log and skip; don't abort the whole
                    // pass, but do count it.
                    eprintln!("  skip: PROFILE row without user_id attribute");
                    stats.errors += 1;
                    continue;
                }
            };

            // Legacy boolean. Absent → false (the same default
            // `user_from_item` applied pre-migration).
            let was_admin = item
                .get("is_admin")
                .and_then(|v| v.as_bool().ok())
                .copied()
                .unwrap_or(false);
            let new_role = if was_admin { UserRole::Admin } else { UserRole::User };

            println!(
                "  user {user_id}: setting role={} (was is_admin={was_admin}){}",
                if was_admin { "admin" } else { "user" },
                if dry_run { " [dry-run]" } else { "" },
            );

            if dry_run {
                if was_admin {
                    stats.set_admin += 1;
                } else {
                    stats.set_user += 1;
                }
                continue;
            }

            match user_repo.set_role(&user_id, new_role).await {
                Ok(()) => {
                    if was_admin {
                        stats.set_admin += 1;
                    } else {
                        stats.set_user += 1;
                    }
                }
                Err(e) => {
                    eprintln!("  user {user_id}: set_role failed: {e}");
                    stats.errors += 1;
                }
            }
        }

        last_key = result.last_evaluated_key;
        if last_key.is_none() {
            break;
        }
    }

    Ok(stats)
}

/// Aggregate counts from one workspace-backfill pass. Re-running a completed
/// backfill should produce `workspaces_created == 0` and `docs_assigned == 0`
/// (every row already carries its workspace), proving idempotency.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackfillWorkspacesStats {
    pub users_seen: usize,
    pub workspaces_created: usize,
    pub docs_seen: usize,
    pub docs_assigned: usize,
    /// Docs whose owner had no default workspace (e.g. a signup that raced
    /// the two passes). Skipped this run; a re-run picks them up.
    pub docs_skipped_no_owner_ws: usize,
}

/// Pass 1: give every user without a `default_workspace_id` a fresh personal
/// workspace. Pass 2: assign every doc without a `workspace_id` to its owner's
/// default workspace (via an in-memory user→workspace map built in pass 1).
///
/// Idempotent: a user that already has a default workspace, and a doc that
/// already has a workspace, are skipped — so a re-run creates and assigns
/// nothing. `dry_run = true` scans + classifies but performs no writes; the
/// stats are the same as if the writes had landed.
///
/// Run against a quiesced instance: a signup completing between the two passes
/// writes its own `default_workspace_id` but won't be in this run's map, so its
/// docs are logged as orphaned and skipped (a re-run resolves them).
pub async fn run_backfill_workspaces(
    user_repo: &UserRepo,
    workspace_repo: &WorkspaceRepo,
    doc_repo: &DocRepo,
    dry_run: bool,
) -> Result<BackfillWorkspacesStats, String> {
    let mut stats = BackfillWorkspacesStats::default();

    // Pass 1: every user needs a default_workspace_id.
    let mut user_to_workspace: HashMap<String, String> = HashMap::new();
    let mut cursor: Option<String> = None;
    loop {
        let (users, next) = user_repo
            .list_all(100, cursor.as_deref())
            .await
            .map_err(|e| format!("list_all users: {e}"))?;
        for user in users {
            stats.users_seen += 1;
            if let Some(ws) = user.default_workspace_id.clone() {
                user_to_workspace.insert(user.user_id.clone(), ws);
                continue;
            }
            let workspace_id = new_id();
            let now = now_usec();
            let name = if user.name.trim().is_empty() {
                "Personal Workspace".to_string()
            } else {
                format!("{}'s Workspace", user.name.trim())
            };
            if !dry_run {
                let ws = Workspace {
                    workspace_id: workspace_id.clone(),
                    name,
                    owner_id: user.user_id.clone(),
                    mfa_required: false,
                    created_at: now,
                    updated_at: now,
                };
                workspace_repo
                    .create(&ws)
                    .await
                    .map_err(|e| format!("create workspace: {e}"))?;
                user_repo
                    .set_default_workspace(&user.user_id, &workspace_id)
                    .await
                    .map_err(|e| format!("set_default_workspace: {e}"))?;
            }
            user_to_workspace.insert(user.user_id, workspace_id);
            stats.workspaces_created += 1;
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    // Pass 2: every doc inherits its owner's default workspace.
    let mut cursor: Option<(String, String)> = None;
    loop {
        let (metas, next) = doc_repo
            .list_all_meta(100, cursor.clone())
            .await
            .map_err(|e| format!("list_all_meta: {e}"))?;
        for meta in metas {
            stats.docs_seen += 1;
            if meta.workspace_id.is_some() {
                continue;
            }
            let Some(ws) = user_to_workspace.get(&meta.owner_id) else {
                stats.docs_skipped_no_owner_ws += 1;
                continue;
            };
            if !dry_run {
                doc_repo
                    .set_workspace_id(&meta.doc_id, ws)
                    .await
                    .map_err(|e| format!("set_workspace_id: {e}"))?;
            }
            stats.docs_assigned += 1;
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    Ok(stats)
}

/// Aggregate counts from one snapshot-backfill pass. A re-run over the same
/// S3 objects should produce `inserted == 0` and `already_present == seen`,
/// proving idempotency.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackfillSnapshotsStats {
    pub seen: usize,
    pub already_present: usize,
    pub inserted: usize,
    pub skipped_bad_key: usize,
    pub errors: usize,
}

/// Parse `docs/{doc_id}/snapshots/{version}.bin` into `(doc_id, version)`.
/// Anything else returns `None` so the caller skips non-snapshot keys
/// (presigned blobs, etc.) without erroring.
pub fn parse_snapshot_key(key: &str) -> Option<(String, u64)> {
    let rest = key.strip_prefix("docs/")?;
    let (doc_id, rest) = rest.split_once('/')?;
    let version_str = rest.strip_prefix("snapshots/")?.strip_suffix(".bin")?;
    let version: u64 = version_str.parse().ok()?;
    Some((doc_id.to_string(), version))
}

/// Walk every `docs/{doc_id}/snapshots/*.bin` object in `bucket` and write the
/// missing `SNAPSHOT#` rows (attributed to `user_id = "system"`, `created_at`
/// from the object's LastModified). Idempotent: an object whose row already
/// exists is skipped. `dry_run = true` classifies but performs no writes.
///
/// Per-object `get`/`create` failures are counted in `errors` and the pass
/// continues; a `list_objects_v2` failure also increments `errors` and stops
/// the walk (returning the partial stats) — matching the original binary.
pub async fn run_backfill_snapshots(
    s3: &aws_sdk_s3::Client,
    bucket: &str,
    snapshot_repo: &SnapshotRepo,
    dry_run: bool,
) -> Result<BackfillSnapshotsStats, String> {
    let mut stats = BackfillSnapshotsStats::default();
    let mut continuation: Option<String> = None;
    loop {
        let mut req = s3.list_objects_v2().bucket(bucket).prefix("docs/");
        if let Some(tok) = continuation.as_ref() {
            req = req.continuation_token(tok);
        }
        let page = match req.send().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("list_objects_v2 failed: {e}");
                stats.errors += 1;
                break;
            }
        };

        for obj in page.contents.unwrap_or_default() {
            let Some(key) = obj.key else { continue };
            stats.seen += 1;
            let Some((doc_id, version)) = parse_snapshot_key(&key) else {
                stats.skipped_bad_key += 1;
                continue;
            };

            match snapshot_repo.get(&doc_id, version).await {
                Ok(Some(_)) => {
                    stats.already_present += 1;
                    continue;
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("  {key}: snapshot_repo.get failed: {e}");
                    stats.errors += 1;
                    continue;
                }
            }

            let size_bytes = obj.size.unwrap_or(0).max(0) as u64;
            let created_at = obj
                .last_modified
                .map(|d| d.as_secs_f64() * 1_000_000.0)
                .map(|us| us as i64)
                .unwrap_or(0);
            let snap = DocSnapshot {
                doc_id: doc_id.clone(),
                version,
                s3_key: key.clone(),
                size_bytes,
                user_id: "system".to_string(),
                created_at,
            };
            if !dry_run {
                if let Err(e) = snapshot_repo.create(&snap).await {
                    eprintln!("  {key}: snapshot_repo.create failed: {e}");
                    stats.errors += 1;
                    continue;
                }
            }
            stats.inserted += 1;
        }

        if page.is_truncated.unwrap_or(false) {
            continuation = page.next_continuation_token;
            if continuation.is_none() {
                break;
            }
        } else {
            break;
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canonical_snapshot_key() {
        assert_eq!(
            parse_snapshot_key("docs/abc123/snapshots/7.bin"),
            Some(("abc123".to_string(), 7))
        );
    }

    #[test]
    fn rejects_non_snapshot_keys() {
        assert!(parse_snapshot_key("docs/abc/blobs/xyz.png").is_none());
        assert!(parse_snapshot_key("tmp/foo.bin").is_none());
        assert!(parse_snapshot_key("docs/abc/snapshots/notanumber.bin").is_none());
        assert!(parse_snapshot_key("docs/abc/snapshots/7.txt").is_none());
        assert!(parse_snapshot_key("docs/abc").is_none());
    }

    #[test]
    fn stats_default_is_all_zero() {
        let s = BackfillUserRoleStats::default();
        assert_eq!(s.scanned, 0);
        assert_eq!(s.already_migrated, 0);
        assert_eq!(s.set_admin, 0);
        assert_eq!(s.set_user, 0);
        assert_eq!(s.errors, 0);
    }
}
