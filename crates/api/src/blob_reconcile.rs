// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Orphaned-object sweep (#166).
//!
//! Several paths can leave S3 objects behind after the thing that owns
//! them is gone, and nothing else ever revisits them:
//!
//! - a purge whose S3 sweep fails after its DynamoDB rows are deleted
//!   (the document is then unreachable, so no retry can find it);
//! - a document copy whose blobs were copied but whose row was never
//!   written (the id never becomes a document);
//! - import staging left by a job that died between staging and cleanup.
//!
//! This walks the per-document prefixes (`blobs/{doc_id}/`,
//! `docs/{doc_id}/`) and the import staging root (`imports/{id}/`) a page
//! at a time, and deletes what provably has no owner.
//!
//! **Conservative by construction**, because a mistake here deletes a
//! user's content:
//!
//! - A document prefix is an orphan only when the `DOC#` row is *absent*
//!   (a clean `Ok(None)`, never an error) **and** nothing under the prefix
//!   was written in the last [`DOC_MIN_AGE_USEC`]. The age gate is what
//!   keeps a document mid-creation safe: a copy writes its blobs before
//!   its row, and those blobs are minutes old, not a day. A soft-deleted
//!   document still has its row and is left to `trash_cleanup`.
//! - Import staging is an orphan only when nothing under it was written in
//!   the last [`STAGING_MIN_AGE_USEC`], and, for a Quip import (one with an
//!   `IMPORT#` record), only once that import is terminal. A running or
//!   resumable import's staging is never touched.
//! - Off by default (`BLOB_RECONCILE_ENABLED`), and dry-run by default
//!   (`BLOB_RECONCILE_DRY_RUN`): the first rollout only logs.
//! - Bounded per tick ([`MAX_CHECKED_PER_TICK`], [`MAX_DELETED_PER_TICK`]),
//!   so a large backlog drains over many ticks instead of in one burst.
//!
//! The walk position is kept in memory. A restart starts the walk over,
//! which only costs re-checking prefixes that were already fine.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::import::ImportStatus;

use crate::state::AppState;

const TICK_INTERVAL: Duration = Duration::from_secs(60 * 60);

const USEC_PER_HOUR: i64 = 60 * 60 * 1_000_000;

/// How long a document-owned prefix must be untouched before its absence
/// of a document row counts as orphaned.
pub const DOC_MIN_AGE_USEC: i64 = 24 * USEC_PER_HOUR;

/// How long import staging must be untouched before it counts as
/// orphaned. DOCX/PDF staging is consumed within minutes and Quip staging
/// is only considered once its import is terminal, so a week is a wide
/// margin.
pub const STAGING_MIN_AGE_USEC: i64 = 7 * 24 * USEC_PER_HOUR;

/// Prefixes looked at per tick, per root.
const MAX_CHECKED_PER_TICK: usize = 500;

/// Orphans deleted (or, in dry-run, reported) per tick, across all roots.
const MAX_DELETED_PER_TICK: usize = 100;

/// Page size for listing a root's children.
const LIST_PAGE: i32 = 100;

/// The roots walked, and what owns each child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Root {
    /// `blobs/{doc_id}/` — uploaded images.
    Blobs,
    /// `docs/{doc_id}/` — snapshots and oversized update payloads.
    Docs,
    /// `imports/{import_id or user_id}/` — import staging.
    Imports,
}

impl Root {
    const ALL: [Root; 3] = [Root::Blobs, Root::Docs, Root::Imports];

    fn prefix(self) -> &'static str {
        match self {
            Root::Blobs => "blobs/",
            Root::Docs => "docs/",
            Root::Imports => "imports/",
        }
    }
}

/// What the sweep decided about one child prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Its owner exists (or may still need it). Left alone.
    Owned,
    /// Ownerless, but written too recently to be sure. Left alone.
    TooRecent,
    /// Ownerless and old enough: deleted, or in dry-run, reported.
    Orphan { deleted: bool },
    /// Something could not be read, so nothing was decided. Left alone.
    Undecided(String),
}

/// Walk position per root, kept across ticks.
#[derive(Default)]
pub struct Cursors(Mutex<HashMap<Root, String>>);

impl Cursors {
    fn get(&self, root: Root) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&root).cloned()
    }

    fn set(&self, root: Root, after: Option<String>) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match after {
            Some(a) => map.insert(root, a),
            None => map.remove(&root),
        };
    }
}

/// Spawn the hourly sweep. Safe to call unconditionally — with
/// `blob_reconcile_enabled` false every tick is a no-op.
pub fn spawn_scheduler(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let cursors = Cursors::default();
        let mut ticker = tokio::time::interval(TICK_INTERVAL);
        // Skip the immediate first tick; a just-booted process has no
        // reason to sweep before anything else has settled.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if state.config.blob_reconcile_enabled {
                sweep(&state, now_usec(), state.config.blob_reconcile_dry_run, &cursors).await;
            }
        }
    })
}

/// Tally of one sweep, for the log line and for tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    pub checked: usize,
    pub orphans: usize,
    pub deleted: usize,
    pub undecided: usize,
}

/// One bounded pass over every root, resuming where the last pass left
/// off. `now` is injectable so tests can age objects without waiting.
pub async fn sweep(state: &AppState, now: i64, dry_run: bool, cursors: &Cursors) -> SweepReport {
    let mut report = SweepReport::default();
    'roots: for root in Root::ALL {
        let mut checked_here = 0usize;
        while checked_here < MAX_CHECKED_PER_TICK {
            let after = cursors.get(root);
            let (names, more) = match state
                .doc_repo
                .s3()
                .list_child_names(root.prefix(), after.as_deref(), LIST_PAGE)
                .await
            {
                Ok(page) => page,
                Err(e) => {
                    tracing::warn!(root = root.prefix(), error = %e, "blob_reconcile: list failed");
                    report.undecided += 1;
                    continue 'roots;
                }
            };
            for name in &names {
                if report.orphans >= MAX_DELETED_PER_TICK {
                    // Out of budget: stop *before* this name so the next
                    // tick picks it up.
                    break 'roots;
                }
                let verdict = check(state, root, name, now, dry_run).await;
                report.checked += 1;
                checked_here += 1;
                match &verdict {
                    Verdict::Orphan { deleted } => {
                        report.orphans += 1;
                        report.deleted += usize::from(*deleted);
                    }
                    Verdict::Undecided(_) => report.undecided += 1,
                    Verdict::Owned | Verdict::TooRecent => {}
                }
                cursors.set(root, Some(name.clone()));
            }
            if !more {
                // End of this root: the next tick starts it over.
                cursors.set(root, None);
                break;
            }
        }
    }
    tracing::info!(
        checked = report.checked,
        orphans = report.orphans,
        deleted = report.deleted,
        undecided = report.undecided,
        dry_run,
        "blob_reconcile pass complete"
    );
    report
}

/// Decide about, and if orphaned (and not dry-run) delete, one child
/// prefix `{root}{name}/`.
pub async fn check(state: &AppState, root: Root, name: &str, now: i64, dry_run: bool) -> Verdict {
    // Same guard as `DocRepo::hard_delete`: every delete below is a prefix
    // delete, so a degenerate name must never reach one.
    if name.is_empty() || name.contains('/') {
        return Verdict::Undecided(format!("refusing a degenerate child name {name:?}"));
    }
    let prefix = format!("{}{name}/", root.prefix());

    let min_age = match root {
        Root::Blobs | Root::Docs => match state.doc_repo.get(name).await {
            Ok(Some(_)) => return Verdict::Owned,
            Ok(None) => DOC_MIN_AGE_USEC,
            Err(e) => return Verdict::Undecided(format!("document lookup: {e}")),
        },
        // An `IMPORT#` record means a Quip import; its staging waits for
        // the import to finish. No record means DOCX/PDF staging, keyed by
        // user id, or a Quip import whose record is gone.
        Root::Imports => match state.import_repo.get(name).await {
            Ok(Some(record)) if !is_terminal(record.status) => return Verdict::Owned,
            Ok(_) => STAGING_MIN_AGE_USEC,
            Err(e) => return Verdict::Undecided(format!("import lookup: {e}")),
        },
    };

    let objects = match state.doc_repo.s3().list_objects_modified(&prefix).await {
        Ok(objects) => objects,
        Err(e) => return Verdict::Undecided(format!("list {prefix}: {e}")),
    };
    let newest = objects.iter().map(|(_, modified)| *modified).max().unwrap_or(0);
    if newest > now - min_age {
        return Verdict::TooRecent;
    }

    if dry_run {
        tracing::info!(prefix, objects = objects.len(), "blob_reconcile dry-run: would delete");
        return Verdict::Orphan { deleted: false };
    }
    match state.doc_repo.s3().delete_prefix(&prefix).await {
        Ok(()) => {
            tracing::info!(prefix, objects = objects.len(), "blob_reconcile: deleted orphaned objects");
            Verdict::Orphan { deleted: true }
        }
        Err(e) => Verdict::Undecided(format!("delete {prefix}: {e}")),
    }
}

/// An import that can no longer run, so its staging is no longer needed.
fn is_terminal(status: ImportStatus) -> bool {
    matches!(status, ImportStatus::Succeeded | ImportStatus::Failed | ImportStatus::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_finished_imports_are_terminal() {
        for s in [ImportStatus::Succeeded, ImportStatus::Failed, ImportStatus::Cancelled] {
            assert!(is_terminal(s), "{s:?}");
        }
        for s in [
            ImportStatus::Scoping,
            ImportStatus::Running,
            ImportStatus::AwaitingIdentityConfirm,
            ImportStatus::TokenRejected,
        ] {
            assert!(!is_terminal(s), "{s:?} can still run, so its staging is live");
        }
    }

    #[test]
    fn cursors_remember_and_reset_per_root() {
        let c = Cursors::default();
        assert_eq!(c.get(Root::Blobs), None);
        c.set(Root::Blobs, Some("abc".into()));
        assert_eq!(c.get(Root::Blobs).as_deref(), Some("abc"));
        assert_eq!(c.get(Root::Docs), None, "roots are independent");
        c.set(Root::Blobs, None);
        assert_eq!(c.get(Root::Blobs), None);
    }
}
