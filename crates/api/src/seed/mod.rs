// Copyright (c) 2026 Joel Baumert. All Rights Reserved.
//
// #142 Phase 3 — sample-template seeding.
//
// The `Sample Templates` gallery is a fixed set of docs seeded into a
// well-known workspace (`SAMPLES_WORKSPACE_ID`) owned by a well-known
// system user (`SAMPLES_SYSTEM_USER_ID`). Every user's template gallery
// pulls this workspace in unconditionally, so every user sees the same
// samples regardless of their own workspace membership.
//
// The seed is idempotent: each template has a stable `sample_id` and
// the seeded doc's DDB id is `sample-<sample_id>`. Re-running the seed
// looks up each doc by that id and skips existing ones. That way the
// binary can safely run as part of a deploy step, or by hand.
//
// Fixture bodies are HTML embedded at compile time via `include_str!`,
// converted into a Y.Doc snapshot via `ogrenotes_collab::import::from_html`
// (the same path the `/documents/import` endpoint uses), then written to
// S3 as v1 of the doc's snapshot.

use std::sync::Arc;

use ogrenotes_common::time::now_usec;
use ogrenotes_storage::models::document::DocumentMeta;
use ogrenotes_storage::models::security_audit::{SecurityAudit, SecurityAuditAction};
use ogrenotes_storage::models::user::{AuthProvider, User, UserRole};
use ogrenotes_storage::models::workspace::{Workspace, WorkspaceMember};
use ogrenotes_storage::models::folder::Folder;
use ogrenotes_storage::models::{DocType, FolderType, InheritMode, ViewOptions, WorkspaceRole};
use ogrenotes_storage::repo::doc_repo::DocRepo;
use ogrenotes_storage::repo::folder_repo::FolderRepo;
use ogrenotes_storage::repo::security_audit_repo::SecurityAuditRepo;
use ogrenotes_storage::repo::user_repo::UserRepo;
use ogrenotes_storage::repo::workspace_repo::WorkspaceRepo;

// ─── Well-known IDs ─────────────────────────────────────────────
//
// Constants used to be in `ogrenotes_common::samples`, but crates/common
// is meant for cross-crate primitives (id gen, time, config, metrics).
// Only crates/api reads these, so they live here next to the seed logic
// that owns them.

/// Owner of every sample-gallery document. Never signs in — no
/// `USER#<id>/SESSION` rows are created for it. Provisioned by the seed
/// binary if absent.
pub const SAMPLES_SYSTEM_USER_ID: &str = "samples-system-user";

/// Workspace that holds every sample-gallery document. Every user's
/// template gallery query pulls from here in addition to their own
/// workspace, so the samples are visible cluster-wide.
pub const SAMPLES_WORKSPACE_ID: &str = "samples-workspace";

/// One entry in the sample-template gallery. Fields are `&'static str`
/// so the whole array can live in the binary — no filesystem access at
/// runtime.
pub struct SampleTemplate {
    /// Stable identifier used to derive the DDB doc id (`sample-<sample_id>`)
    /// so repeat runs of the seed binary are idempotent.
    pub sample_id: &'static str,
    /// Display title. Shown in the picker gallery.
    pub title: &'static str,
    /// HTML fixture — parsed by `ogrenotes_collab::import::from_html` at
    /// seed time. Marks (bold/italic/links) survive; tables and images
    /// don't (the importer's v1 limitations).
    pub body_html: &'static str,
}

/// The complete set. Fixtures are drawn from the "Built-In / Default
/// Templates" list in `design/templates.md` — the five most broadly-useful
/// ones for a v1 gallery. Adding a sample = extending this array and
/// re-running the seed binary.
pub const SAMPLE_TEMPLATES: &[SampleTemplate] = &[
    SampleTemplate {
        sample_id: "meeting-notes",
        title: "Meeting Notes",
        body_html: include_str!("samples/meeting-notes.html"),
    },
    SampleTemplate {
        sample_id: "one-on-one",
        title: "1:1 Meeting Notes",
        body_html: include_str!("samples/one-on-one.html"),
    },
    SampleTemplate {
        sample_id: "project-hub",
        title: "Project Hub",
        body_html: include_str!("samples/project-hub.html"),
    },
    SampleTemplate {
        sample_id: "onboarding-checklist",
        title: "Onboarding Checklist",
        body_html: include_str!("samples/onboarding-checklist.html"),
    },
    SampleTemplate {
        sample_id: "sales-battlecard",
        title: "Sales Battlecard",
        body_html: include_str!("samples/sales-battlecard.html"),
    },
];

/// Derive the stable DDB doc id for a sample. Same shape everywhere so
/// the seed lookup and `list_templates` produce matching ids.
pub fn sample_doc_id(sample_id: &str) -> String {
    format!("sample-{sample_id}")
}

/// Placeholder keys per sample doc id, computed once from the compile-time
/// HTML fixtures and cached for the lifetime of the process.
///
/// `list_templates` short-circuits its placeholder scan for samples using
/// this cache: the sample docs are immutable between deploys, so re-reading
/// the S3 snapshot on every gallery load is pure waste — 5 GETs per user
/// per open that always return the same bytes.
///
/// Keyed by `sample_doc_id(sample_id)` (i.e. `"sample-<sample_id>"`) so
/// the lookup shape matches what falls out of `query_docs_by_workspace`.
pub fn sample_placeholders(doc_id: &str) -> Option<&'static Vec<String>> {
    static CACHE: std::sync::OnceLock<std::collections::HashMap<String, Vec<String>>> =
        std::sync::OnceLock::new();
    let map = CACHE.get_or_init(|| {
        SAMPLE_TEMPLATES
            .iter()
            .map(|t| {
                let ydoc = ogrenotes_collab::import::from_html(t.body_html);
                let keys = ogrenotes_collab::mail_merge::scan_ydoc(&ydoc);
                (sample_doc_id(t.sample_id), keys)
            })
            .collect()
    });
    map.get(doc_id)
}

/// Per-run stats — printed by the CLI wrapper.
#[derive(Debug, Default)]
pub struct SeedSamplesStats {
    pub user_created: bool,
    pub workspace_created: bool,
    pub templates_created: usize,
    pub templates_skipped_existing: usize,
    /// Under `--force`, an existing sample's snapshot was rewritten to
    /// reflect a fixture change. Without `--force` this stays zero.
    pub templates_refreshed: usize,
}

/// Provision the system user + samples workspace + every sample template
/// that isn't already present. Idempotent — safe to re-run.
///
/// `dry_run = true` logs what would change without writing.
///
/// `force = true` rewrites the snapshot of every existing sample to
/// match the current HTML fixture. Without this, editing a fixture and
/// re-running the seed is a silent no-op (the doc-id check considers
/// the row "already provisioned" regardless of content).
pub async fn run_seed_samples(
    user_repo: &UserRepo,
    workspace_repo: &WorkspaceRepo,
    doc_repo: &Arc<DocRepo>,
    folder_repo: &FolderRepo,
    security_audit_repo: &SecurityAuditRepo,
    dry_run: bool,
    force: bool,
) -> Result<SeedSamplesStats, String> {
    let mut stats = SeedSamplesStats::default();
    // Prefix action lines with `[dry-run]` when nothing will actually be
    // written. Without this the log stream looks identical to a real
    // seed once the banner scrolls off; operators grepping "provisioning"
    // over a mixed session couldn't tell whether a write happened.
    let action_prefix = if dry_run { "[dry-run] " } else { "" };

    // ─── System user ──────────────────────────────────────────────
    if user_repo
        .get_by_id(SAMPLES_SYSTEM_USER_ID)
        .await
        .map_err(|e| format!("user lookup: {e}"))?
        .is_none()
    {
        println!("{action_prefix}+ provisioning system user {SAMPLES_SYSTEM_USER_ID}");
        if !dry_run {
            let now = now_usec();
            // Stub system-folder rows so `user.home/private/trash_folder_id`
            // resolve. Without these, any code that walks a user's system
            // folders (e.g. a folder-integrity migration, an admin
            // list-all-folders view, or a per-user cleanup job) would
            // 404 on the samples user. Provisioning them costs three DDB
            // puts on the first-run path only.
            for (folder_id, title) in [
                (format!("{SAMPLES_SYSTEM_USER_ID}-home"), "Home"),
                (format!("{SAMPLES_SYSTEM_USER_ID}-private"), "Private"),
                (format!("{SAMPLES_SYSTEM_USER_ID}-trash"), "Trash"),
            ] {
                folder_repo
                    .create(&Folder {
                        folder_id,
                        title: title.to_string(),
                        color: 0,
                        parent_id: None,
                        owner_id: SAMPLES_SYSTEM_USER_ID.to_string(),
                        folder_type: FolderType::System,
                        inherit_mode: InheritMode::Restricted,
                        created_at: now,
                        updated_at: now,
                    })
                    .await
                    .map_err(|e| format!("create system folder for samples user: {e}"))?;
            }
            let system_user = User {
                user_id: SAMPLES_SYSTEM_USER_ID.to_string(),
                name: "OgreNotes Samples".to_string(),
                email: "samples-system-user@ogrenotes.internal".to_string(),
                avatar_url: None,
                provider: AuthProvider::Google,
                provider_subject_id: None,
                // Home / Private / Trash: unused (this user never signs in)
                // but the repo expects strings, so we give it stable values.
                home_folder_id: format!("{SAMPLES_SYSTEM_USER_ID}-home"),
                private_folder_id: format!("{SAMPLES_SYSTEM_USER_ID}-private"),
                trash_folder_id: format!("{SAMPLES_SYSTEM_USER_ID}-trash"),
                archive_folder_id: None,
                pinned_folder_id: None,
                default_workspace_id: Some(SAMPLES_WORKSPACE_ID.to_string()),
                mfa_secret: None,
                mfa_enrolled_at: None,
                external_id: None,
                role: UserRole::User,
                is_disabled: true, // never signs in
                ask_policy: None,
                legacy_ask_enabled: false,
                email_notifications: Default::default(),
                ui_prefs: None,
                status: None,
                last_active_at: 0,
                created_at: now,
                updated_at: now,
            };
            user_repo
                .create(&system_user)
                .await
                .map_err(|e| format!("create system user: {e}"))?;
            // Identity write — CLAUDE.md audit rule requires a SecurityAudit
            // row. Actor label is a fixed "seed" (no operator identity is
            // captured at CLI level); subject is the newly-created user.
            let audit = SecurityAudit {
                audit_id: nanoid::nanoid!(16),
                user_id: SAMPLES_SYSTEM_USER_ID.to_string(),
                actor_id: "seed".to_string(),
                action: SecurityAuditAction::SystemUserProvisioned,
                created_at: now,
            };
            security_audit_repo
                .create(&audit)
                .await
                .map_err(|e| format!("audit system-user creation: {e}"))?;
        }
        stats.user_created = true;
    } else {
        println!("- system user already exists");
    }

    // ─── Samples workspace ───────────────────────────────────────
    if workspace_repo
        .get(SAMPLES_WORKSPACE_ID)
        .await
        .map_err(|e| format!("workspace lookup: {e}"))?
        .is_none()
    {
        println!("{action_prefix}+ provisioning samples workspace {SAMPLES_WORKSPACE_ID}");
        if !dry_run {
            let now = now_usec();
            let workspace = Workspace {
                workspace_id: SAMPLES_WORKSPACE_ID.to_string(),
                name: "Sample Templates".to_string(),
                owner_id: SAMPLES_SYSTEM_USER_ID.to_string(),
                mfa_required: false,
                created_at: now,
                updated_at: now,
            };
            workspace_repo
                .create(&workspace)
                .await
                .map_err(|e| format!("create workspace: {e}"))?;
        }
        stats.workspace_created = true;
    } else {
        println!("- samples workspace already exists");
    }
    // add_member is a plain put (idempotent), and running it always is
    // the fix for the "workspace created but add_member failed" orphan:
    // without this, a partial first-run leaves the workspace missing
    // its owner-member, and the `.is_none()` guard above hides the gap
    // on every re-run. Running unconditionally re-establishes the
    // invariant "workspace exists ⇒ owner is a member" every time.
    if !dry_run {
        workspace_repo
            .add_member(&WorkspaceMember {
                workspace_id: SAMPLES_WORKSPACE_ID.to_string(),
                user_id: SAMPLES_SYSTEM_USER_ID.to_string(),
                role: WorkspaceRole::Owner,
                joined_at: now_usec(),
            })
            .await
            .map_err(|e| format!("add system user to workspace: {e}"))?;
    }

    // ─── Each template ────────────────────────────────────────────
    for template in SAMPLE_TEMPLATES {
        let doc_id = sample_doc_id(template.sample_id);
        let existing = doc_repo
            .get(&doc_id)
            .await
            .map_err(|e| format!("doc lookup {doc_id}: {e}"))?;

        match (existing, force) {
            (Some(existing_meta), true) => {
                // Fixture edit path: bump snapshot to reflect current HTML.
                println!(
                    "{action_prefix}* refreshing template {} ({}) v{}→v{}",
                    template.sample_id,
                    doc_id,
                    existing_meta.snapshot_version,
                    existing_meta.snapshot_version + 1,
                );
                if !dry_run {
                    let ydoc = ogrenotes_collab::import::from_html(template.body_html);
                    let snapshot = ogrenotes_collab::snapshot::doc_to_bytes(&ydoc);
                    let new_version = existing_meta.snapshot_version + 1;
                    doc_repo
                        .save_snapshot(
                            &doc_id,
                            &snapshot,
                            new_version,
                            now_usec(),
                            SAMPLES_SYSTEM_USER_ID,
                        )
                        .await
                        .map_err(|e| format!("refresh doc {doc_id}: {e}"))?;
                }
                stats.templates_refreshed += 1;
            }
            (Some(_), false) => {
                println!("- template {} already exists ({})", template.sample_id, doc_id);
                stats.templates_skipped_existing += 1;
            }
            (None, _) => {
                println!("{action_prefix}+ seeding template {} ({})", template.sample_id, doc_id);
                if !dry_run {
                    let ydoc = ogrenotes_collab::import::from_html(template.body_html);
                    let snapshot = ogrenotes_collab::snapshot::doc_to_bytes(&ydoc);
                    let now = now_usec();
                    let meta = DocumentMeta {
                        doc_id: doc_id.clone(),
                        title: template.title.to_string(),
                        owner_id: SAMPLES_SYSTEM_USER_ID.to_string(),
                        folder_id: None,
                        additional_folder_ids: Vec::new(),
                        workspace_id: Some(SAMPLES_WORKSPACE_ID.to_string()),
                        doc_type: DocType::Document,
                        snapshot_version: 1,
                        snapshot_s3_key: Some(format!("docs/{doc_id}/snapshots/1.bin")),
                        is_deleted: false,
                        deleted_at: None,
                        // Sparse-write convention: every other creation site
                        // uses `None` for the null case. Using `Some(None)`
                        // instead would emit a `link_sharing_mode="none"` DDB
                        // attribute where every other row omits it, and any
                        // predicate like `.is_some()` would treat samples as
                        // link-shared.
                        link_sharing_mode: None,
                        link_view_options: ViewOptions::default(),
                        locked: false,
                        is_template: true,
                        created_at: now,
                        updated_at: now,
                    };
                    match doc_repo.create(&meta, &snapshot).await {
                        Ok(()) => {}
                        Err(e) => {
                            // Two-writer race: we won the get() but another seed
                            // process won the put_item_conditional. Treat as
                            // "already provisioned" so re-runs converge instead
                            // of aborting mid-list.
                            let msg = e.to_string();
                            if msg.contains("ConditionalCheckFailedException") {
                                println!(
                                    "- template {} already exists ({}) — raced concurrent seed",
                                    template.sample_id, doc_id,
                                );
                                stats.templates_skipped_existing += 1;
                                continue;
                            }
                            return Err(format!("create doc {doc_id}: {msg}"));
                        }
                    }
                }
                stats.templates_created += 1;
            }
        }
    }

    Ok(stats)
}
