// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Inventory rows for the Quip import manifest (Phase 1). All rows share
//! the import partition `PK = IMPORT#<import_id>`; none carries a token.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadState {
    Pending,
    ContentDone,
    CommentsDone,
    Skipped,
    /// A thread that failed deterministically enough times (Task 4's `N`
    /// retry budget) that the content pass gave up on it and moved on.
    /// Distinct from `Skipped` (a deliberate disposition, e.g. chat
    /// threads) — `Failed` means the pass *tried* and lost.
    Failed,
}

/// One folder discovered during inventory BFS. SK = `FOLDER#<quip_folder_id>`.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderRow {
    pub quip_folder_id: String,
    pub owner_id: String,
    pub title: String,
    pub parent_quip_id: Option<String>,
    pub ogre_folder_id: Option<String>,
}

impl FolderRow {
    pub fn sk(&self) -> String {
        format!("FOLDER#{}", self.quip_folder_id)
    }
}

/// One thread (doc/spreadsheet/chat) discovered during inventory. SK =
/// `THREAD#<quip_thread_id>`. This is the per-thread resumability unit.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub quip_thread_id: String,
    pub owner_id: String,
    pub title: String,
    pub thread_type: String,
    pub updated_usec: i64,
    /// Every folder the thread appears in (multi-folder membership).
    pub member_folders: Vec<String>,
    /// First folder it was encountered in during BFS (stable ordering).
    pub first_folder: String,
    pub state: ThreadState,
    pub ogre_doc_id: Option<String>,
    /// Why the thread is `Skipped` or `Failed` (e.g. `"chat thread"`,
    /// `"403 forbidden after 3 attempts"`). `None` for `Pending` /
    /// `ContentDone` / `CommentsDone`, and for rows written before this
    /// field existed — sparse-omitted on write, defaults to `None` on
    /// read so pre-existing rows decode unchanged.
    pub reason: Option<String>,
    /// Number of content-pass attempts made on this thread so far. Lives
    /// on the row (not in worker memory) so it survives a process
    /// restart; Task 4 reads it to decide when a thread has failed enough
    /// times to give up. Rows written before this field existed have no
    /// `attempts` attribute and decode as `0`.
    pub attempts: u32,
}

impl ThreadRow {
    pub fn sk(&self) -> String {
        format!("THREAD#{}", self.quip_thread_id)
    }
}

/// Max `entries` per `SecMapRow` chunk, chosen to stay well under
/// DynamoDB's 400 KB item cap. Task 6 (the content-pass caller) splits a
/// thread's full section→block map into chunks of this size before
/// calling `put_secmap` once per chunk.
pub const SECMAP_CHUNK_ENTRIES: usize = 2_000;

/// One chunk of a thread's section-id → block-id map, built during the
/// Phase-2 content pass so later comment/link resolution can translate a
/// Quip anchor (`#section-id`) into the Ogre block it landed on. SK =
/// `SECMAP#<quip_thread_id>#<chunk>`. Chunked because a thread with many
/// sections could otherwise blow the per-item size cap.
#[derive(Debug, Clone, PartialEq)]
pub struct SecMapRow {
    pub quip_thread_id: String,
    pub chunk: u32,
    pub owner_id: String,
    /// `(quip_section_id, ogre_block_id)` pairs, in the order encountered.
    pub entries: Vec<(String, String)>,
}

impl SecMapRow {
    pub fn sk(&self) -> String {
        format!("SECMAP#{}#{}", self.quip_thread_id, self.chunk)
    }
}

/// Max `links` per persisted `UNRESOLVED#` chunk, chosen for the same
/// reason as [`SECMAP_CHUNK_ENTRIES`]: one DynamoDB item may not exceed
/// 400 KB. A [`PendingLinkItem`] costs roughly 120 bytes on the wire
/// (three ids plus their attribute names), so an unbounded row tops out
/// near 3k links — and a Quip index/directory page is exactly that dense.
/// 1 000 keeps a ~3x margin. Unlike `SECMAP#`, the caller does **not**
/// chunk: `ImportRepo::put_unresolved` splits and
/// `ImportRepo::list_unresolved` concatenates, so a source thread is one
/// logical [`UnresolvedRow`] on both sides of the repo boundary.
pub const UNRESOLVED_CHUNK_LINKS: usize = 1_000;

/// Every cross-thread link discovered in `source_quip_thread_id` whose
/// target thread hadn't been imported yet at the time it was encountered.
/// Persisted as one or more chunks at SK =
/// `UNRESOLVED#<source_quip_thread_id>#<chunk>`; the repo hides the
/// chunking, so this struct is always the *whole* thread's link set.
/// Consumed by a later pass (Phase 2b+) that revisits these once every
/// thread has an `ogre_doc_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedRow {
    pub source_quip_thread_id: String,
    pub owner_id: String,
    pub links: Vec<PendingLinkItem>,
}

impl UnresolvedRow {
    /// SK of one persisted chunk. Chunk order is *numeric*, not the SK's
    /// lexicographic order (`#10` sorts before `#2`), so readers sort on
    /// the parsed chunk number — same rule as `SECMAP#`.
    pub fn sk(&self, chunk: u32) -> String {
        format!("UNRESOLVED#{}#{}", self.source_quip_thread_id, chunk)
    }
}

/// One link within an `UnresolvedRow`, naming the source block and the
/// Quip thread (and optional in-thread section) it points at.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingLinkItem {
    pub source_block_id: String,
    pub target_quip_thread_id: String,
    pub target_quip_section_id: Option<String>,
}

/// Max [`ReportNote`]s kept on a `REPORT` row. Past this, notes are
/// dropped and only counted (see [`ReportRow::notes_dropped`]).
///
/// The bound is load-bearing for the same reason [`SECMAP_CHUNK_ENTRIES`]
/// and [`UNRESOLVED_CHUNK_LINKS`] are: one DynamoDB item may not exceed
/// 400 KB. A 10 000-thread import in which every thread is inaccessible
/// would otherwise grow this row past the cap, and the *write* would start
/// failing — losing the entire report, which is the one artifact that tells
/// the user what the import dropped. A note costs roughly 200 bytes on the
/// wire (thread id + kind + a sentence of detail + attribute names), so 200
/// notes is ~40 KB: a ~10x margin.
///
/// Unlike `SECMAP#`/`UNRESOLVED#` this row is deliberately **not** chunked.
/// A bounded list is the right shape for something a human reads: nobody
/// scrolls 10 000 failure lines, and the counters carry the true totals.
pub const REPORT_MAX_NOTES: usize = 200;

/// The import's accumulating outcome report: counters plus a bounded list
/// of named losses and fallbacks. SK = `REPORT`, one row per import.
///
/// This is the minimal storage shape the design calls for — not the Phase 5
/// report UI. It exists so a skipped thread is distinguishable from a thread
/// that never existed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReportRow {
    pub owner_id: String,
    /// Free-form outcome tallies, e.g. `threads_imported`,
    /// `threads_skipped`, `threads_failed`, `images_dropped`. A `BTreeMap`
    /// so the rendered order is stable across runs. These keep counting
    /// past [`REPORT_MAX_NOTES`] — they are the "and 9 800 more" numbers.
    pub counters: BTreeMap<String, u64>,
    /// Named losses/fallbacks, capped at [`REPORT_MAX_NOTES`].
    pub notes: Vec<ReportNote>,
    /// How many notes were discarded after `notes` filled up. Zero means
    /// the list is complete; **any non-zero value means `notes` is only a
    /// prefix** and a reader must say so ("… and N more"). This is the
    /// truncation marker — a count rather than a flag, because the count is
    /// exactly what the sentence needs.
    pub notes_dropped: u64,
}

/// One named loss or fallback: which Quip thread, what kind of outcome,
/// and a human-readable detail.
///
/// `detail` is rendered to the user. Callers build it from a `QuipError`'s
/// `Display` (which is asserted token-free in `quip-import`'s tests) or from
/// their own text — never from a raw response body, which could echo a
/// credential into a durable row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportNote {
    pub quip_thread_id: String,
    /// Coarse outcome class, e.g. `"skipped"`, `"failed"`,
    /// `"image_dropped"`, `"flat_folder_fallback"`.
    pub kind: String,
    pub detail: String,
}

impl ReportRow {
    pub fn sk() -> &'static str {
        "REPORT"
    }

    pub fn new(owner_id: &str) -> Self {
        Self {
            owner_id: owner_id.to_string(),
            ..Self::default()
        }
    }

    /// Record one note, honoring the cap: past [`REPORT_MAX_NOTES`] the
    /// note is discarded and only `notes_dropped` advances. Keeping this
    /// decision in Rust (rather than in a DynamoDB condition expression)
    /// is what makes the bound testable without live infrastructure.
    pub fn push_note(&mut self, note: ReportNote) {
        if self.notes.len() >= REPORT_MAX_NOTES {
            self.notes_dropped = self.notes_dropped.saturating_add(1);
            return;
        }
        self.notes.push(note);
    }

    /// Add `by` to `key`, creating it at zero first. Saturating: a counter
    /// that overflowed would be a nonsense number in a user-facing report,
    /// but it is never worth panicking a running import over.
    pub fn bump_counter(&mut self, key: &str, by: u64) {
        let slot = self.counters.entry(key.to_string()).or_insert(0);
        *slot = slot.saturating_add(by);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_state_serializes_lowercase_and_round_trips() {
        for (s, tok) in [
            (ThreadState::Pending, "pending"),
            (ThreadState::ContentDone, "contentdone"),
            (ThreadState::CommentsDone, "commentsdone"),
            (ThreadState::Skipped, "skipped"),
            (ThreadState::Failed, "failed"),
        ] {
            let j = serde_json::to_string(&s).unwrap();
            assert_eq!(j.trim_matches('"'), tok);
            let back: ThreadState = serde_json::from_str(&j).unwrap();
            assert_eq!(back, s);
        }
    }

    #[test]
    fn row_sk_formats() {
        let f = FolderRow { quip_folder_id: "qf1".into(), owner_id: "u1".into(),
            title: "Root".into(), parent_quip_id: None, ogre_folder_id: None };
        assert_eq!(f.sk(), "FOLDER#qf1");
        let t = ThreadRow { quip_thread_id: "qt1".into(), owner_id: "u1".into(),
            title: "Doc".into(), thread_type: "document".into(), updated_usec: 5,
            member_folders: vec!["qf1".into()], first_folder: "qf1".into(),
            state: ThreadState::Pending, ogre_doc_id: None, reason: None, attempts: 0 };
        assert_eq!(t.sk(), "THREAD#qt1");
    }

    #[test]
    fn report_row_sk_is_the_singleton_report_key() {
        assert_eq!(ReportRow::sk(), "REPORT");
    }

    /// The load-bearing bound: past [`REPORT_MAX_NOTES`] the note list
    /// stops growing (so the item can't outgrow DynamoDB's 400 KB cap and
    /// take the whole report down with it) while the counters keep the
    /// true total — that's what lets the report say "and N more".
    #[test]
    fn notes_truncate_at_the_cap_while_counters_keep_counting() {
        const OVERFLOW: usize = 50;
        let mut row = ReportRow::new("u1");
        for i in 0..REPORT_MAX_NOTES + OVERFLOW {
            row.push_note(ReportNote {
                quip_thread_id: format!("qt{i}"),
                kind: "failed".into(),
                detail: "403 forbidden".into(),
            });
            row.bump_counter("threads_failed", 1);
        }

        assert_eq!(row.notes.len(), REPORT_MAX_NOTES, "notes must stop at the cap");
        assert_eq!(
            row.counters.get("threads_failed"),
            Some(&((REPORT_MAX_NOTES + OVERFLOW) as u64)),
            "the counter must keep counting past the cap",
        );
        assert!(
            row.counters["threads_failed"] > row.notes.len() as u64,
            "the true total must exceed the retained list — otherwise a reader \
             would believe the list is complete",
        );
        assert_eq!(
            row.notes_dropped, OVERFLOW as u64,
            "the truncation marker must say exactly how many notes were lost",
        );
        // The retained notes are the FIRST ones, not the last: the earliest
        // failures are the ones a user debugs from.
        assert_eq!(row.notes[0].quip_thread_id, "qt0");
        assert_eq!(
            row.notes[REPORT_MAX_NOTES - 1].quip_thread_id,
            format!("qt{}", REPORT_MAX_NOTES - 1)
        );
    }

    /// Below the cap, nothing is dropped and no truncation is claimed.
    #[test]
    fn notes_below_the_cap_are_all_kept_and_not_marked_truncated() {
        let mut row = ReportRow::new("u1");
        for i in 0..REPORT_MAX_NOTES {
            row.push_note(ReportNote {
                quip_thread_id: format!("qt{i}"),
                kind: "skipped".into(),
                detail: "chat thread".into(),
            });
        }
        assert_eq!(row.notes.len(), REPORT_MAX_NOTES);
        assert_eq!(row.notes_dropped, 0, "an exactly-full list is not truncated");
    }

    #[test]
    fn counters_accumulate_per_key() {
        let mut row = ReportRow::new("u1");
        row.bump_counter("threads_imported", 1);
        row.bump_counter("threads_imported", 4);
        row.bump_counter("images_dropped", 2);
        assert_eq!(row.counters["threads_imported"], 5);
        assert_eq!(row.counters["images_dropped"], 2);
    }
}
