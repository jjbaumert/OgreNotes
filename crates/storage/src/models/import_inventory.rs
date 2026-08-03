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
///
/// #194 F-10's comment anchors do not move this bound. The chunker slices on
/// entry *count*, and an anchor entry is the same shape as a section entry —
/// two opaque `temp:C:` ids — so it consumes exactly one slot and the
/// per-entry size the 2 000 was chosen against is unchanged. All it can do
/// is push a thread over a chunk boundary one entry sooner, and the measured
/// density is four anchors across the whole 56-thread staged corpus against
/// a densest-thread section count of 528.
pub const SECMAP_CHUNK_ENTRIES: usize = 2_000;

/// One chunk of a thread's section-id → block-id map, built during the
/// Phase-2 content pass so later comment/link resolution can translate a
/// Quip anchor (`#section-id`) into the Ogre block it landed on. SK =
/// `SECMAP#<quip_thread_id>#<chunk>`. Chunked because a thread with many
/// sections could otherwise blow the per-item size cap.
///
/// Since #194 F-10 an entry's key is either a Quip **section id** (the block
/// *is* that element) or a Quip **comment-anchor id** — the `annotationid`
/// of a commented inline range, whose block merely *contains* it. Both are
/// ids from the same Quip namespace and both answer the same question, so
/// they share one map and a reader keys on the id without caring which it
/// was. What that buys is that Phase 4 needs no row kind of its own:
/// `ImportRepo::get_secmap` already returns the anchor.
#[derive(Debug, Clone, PartialEq)]
pub struct SecMapRow {
    pub quip_thread_id: String,
    pub chunk: u32,
    pub owner_id: String,
    /// `(quip_anchor_id, ogre_block_id)` pairs, in the order encountered.
    /// The attribute names on the wire still read `quip_section_id` — the
    /// stored shape is unchanged, only what may key it has widened.
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
///
/// This is the hard item-size backstop. What actually binds first is the
/// per-kind budget below — see [`REPORT_MAX_NOTES_PER_KIND`].
pub const REPORT_MAX_NOTES: usize = 200;

/// How many distinct [`ReportNote::kind`]s may claim a budget on one
/// report. `kind` is a free-form `String`, so without this a caller bug
/// (a thread id leaking into the kind) would mint unbounded budgets and
/// walk the row straight back into the 400 KB failure this module exists
/// to prevent. Notes whose kind arrives after the limit is reached are
/// dropped and counted, like any other over-budget note.
pub const REPORT_MAX_NOTE_KINDS: usize = 8;

/// How many notes each distinct `kind` may occupy.
///
/// **The per-kind budget is what stops one noisy kind from starving the
/// rest.** A flat first-come cap is monopolizable: an import that drops
/// 500 images before it reaches its first inaccessible thread would spend
/// every slot on `image_dropped` notes, and the artifact whose job is to
/// name the lost *documents* would name none.
///
/// The budget is **fixed and non-transferable — an unused budget stays
/// unused.** That is deliberate, not an oversight: lending spare capacity
/// to whichever kind shows up first is exactly what would starve a kind
/// that appears late, and lateness is the normal case (image drops happen
/// throughout; a thread-level failure may not happen until the last
/// thread). Guaranteeing a floor for a kind you have not seen yet is only
/// possible if you refuse to give its slots away. The cost is that a
/// single-kind import shows 25 examples rather than 200; the counters,
/// which are uncapped, still carry the true totals.
pub const REPORT_MAX_NOTES_PER_KIND: usize = 25;

// The per-kind budgets must fit inside the item-size cap; otherwise the
// budgets would be the thing that blows the 400 KB item. Compile-time so
// that retuning any of the three constants can't silently break it.
const _: () = assert!(
    REPORT_MAX_NOTE_KINDS * REPORT_MAX_NOTES_PER_KIND <= REPORT_MAX_NOTES,
    "per-kind note budgets must fit within REPORT_MAX_NOTES"
);

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

    /// Record one note, honoring the budgets. A note is kept only if all
    /// three hold; otherwise it is discarded and `notes_dropped` advances:
    ///
    /// 1. its kind is under [`REPORT_MAX_NOTES_PER_KIND`] — no kind may
    ///    starve another (this is the one that normally binds),
    /// 2. its kind is already present, or there is room for a new one under
    ///    [`REPORT_MAX_NOTE_KINDS`],
    /// 3. the row is under [`REPORT_MAX_NOTES`] overall — the item-size
    ///    backstop, unreachable while (1) and (2) hold given the current
    ///    constants, and kept precisely so that retuning them can never
    ///    make the 400 KB bound the thing that gives way.
    ///
    /// Keeping this decision in Rust (rather than in a DynamoDB condition
    /// expression) is what makes the bounds testable without live
    /// infrastructure.
    pub fn push_note(&mut self, note: ReportNote) {
        let same_kind = self.notes.iter().filter(|n| n.kind == note.kind).count();
        let over_budget = same_kind >= REPORT_MAX_NOTES_PER_KIND
            || (same_kind == 0 && self.distinct_kinds() >= REPORT_MAX_NOTE_KINDS)
            || self.notes.len() >= REPORT_MAX_NOTES;
        if over_budget {
            self.notes_dropped = self.notes_dropped.saturating_add(1);
            return;
        }
        self.notes.push(note);
    }

    /// Number of distinct `kind`s currently represented in `notes`. Linear
    /// over a list bounded at [`REPORT_MAX_NOTES`], so the cost is a
    /// few-hundred-element scan per note — irrelevant next to the DynamoDB
    /// round trip that follows it.
    fn distinct_kinds(&self) -> usize {
        self.notes
            .iter()
            .map(|n| n.kind.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
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

    fn note(kind: &str, i: usize) -> ReportNote {
        ReportNote {
            quip_thread_id: format!("qt{i:04}"),
            kind: kind.to_string(),
            detail: "403 forbidden".into(),
        }
    }

    fn count_kind(row: &ReportRow, kind: &str) -> usize {
        row.notes.iter().filter(|n| n.kind == kind).count()
    }

    /// The load-bearing bound: past its budget a kind's note list stops
    /// growing (so the item can't outgrow DynamoDB's 400 KB cap and take
    /// the whole report down with it) while the counters keep the true
    /// total — that's what lets the report say "and N more".
    #[test]
    fn notes_truncate_at_the_cap_while_counters_keep_counting() {
        const OVERFLOW: usize = 50;
        let mut row = ReportRow::new("u1");
        for i in 0..REPORT_MAX_NOTES_PER_KIND + OVERFLOW {
            row.push_note(note("failed", i));
            row.bump_counter("threads_failed", 1);
        }

        assert_eq!(
            row.notes.len(),
            REPORT_MAX_NOTES_PER_KIND,
            "notes must stop at the kind's budget"
        );
        assert_eq!(
            row.counters.get("threads_failed"),
            Some(&((REPORT_MAX_NOTES_PER_KIND + OVERFLOW) as u64)),
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
        assert_eq!(row.notes[0].quip_thread_id, "qt0000");
        assert_eq!(
            row.notes[REPORT_MAX_NOTES_PER_KIND - 1].quip_thread_id,
            format!("qt{:04}", REPORT_MAX_NOTES_PER_KIND - 1)
        );
    }

    /// Below the budget, nothing is dropped and no truncation is claimed.
    #[test]
    fn notes_below_the_cap_are_all_kept_and_not_marked_truncated() {
        let mut row = ReportRow::new("u1");
        for i in 0..REPORT_MAX_NOTES_PER_KIND {
            row.push_note(note("skipped", i));
        }
        assert_eq!(row.notes.len(), REPORT_MAX_NOTES_PER_KIND);
        assert_eq!(row.notes_dropped, 0, "an exactly-full budget is not truncated");
    }

    /// The starvation case, and the reason the budget is per-kind rather
    /// than first-come: an import drops a pile of images long before it
    /// reaches its first inaccessible document. Under a flat cap the
    /// images take every slot and the report — whose whole job is to name
    /// the *documents* you lost — names none of them.
    #[test]
    fn a_noisy_kind_cannot_starve_the_notes_that_name_lost_documents() {
        let mut row = ReportRow::new("u1");
        // Far more image drops than the whole row could ever hold, all
        // arriving before the first thread-level failure.
        for i in 0..REPORT_MAX_NOTES * 3 {
            row.push_note(note("image_dropped", i));
        }
        row.push_note(note("skipped", 9_000));
        row.push_note(note("failed", 9_001));

        assert_eq!(
            count_kind(&row, "image_dropped"),
            REPORT_MAX_NOTES_PER_KIND,
            "a noisy kind is held to its own budget",
        );
        assert_eq!(
            count_kind(&row, "skipped"),
            1,
            "a thread-level note arriving after the flood must still land",
        );
        assert_eq!(count_kind(&row, "failed"), 1);
    }

    /// Budgets are non-transferable in the other direction too: a kind
    /// that never appears does not lend its slots to one that does.
    #[test]
    fn an_unused_budget_is_not_lent_to_a_noisy_kind() {
        let mut row = ReportRow::new("u1");
        for i in 0..REPORT_MAX_NOTES * 2 {
            row.push_note(note("image_dropped", i));
        }
        assert_eq!(
            row.notes.len(),
            REPORT_MAX_NOTES_PER_KIND,
            "one kind alone fills only its own budget, never the whole row",
        );
    }

    /// `kind` is a free-form String, so a caller bug that mints a new kind
    /// per note must not mint unbounded budgets with it. The row stays
    /// inside the item-size cap and the overflow is counted.
    #[test]
    fn unbounded_distinct_kinds_cannot_grow_the_row_past_the_item_cap() {
        let mut row = ReportRow::new("u1");
        const ATTEMPTS: usize = 5_000;
        for i in 0..ATTEMPTS {
            // The pathological shape: the thread id leaks into the kind.
            row.push_note(note(&format!("failed_qt{i}"), i));
        }
        assert_eq!(
            row.notes.len(),
            REPORT_MAX_NOTE_KINDS,
            "only the first budgeted kinds get a slot",
        );
        assert!(row.notes.len() <= REPORT_MAX_NOTES);
        assert_eq!(row.notes_dropped, (ATTEMPTS - REPORT_MAX_NOTE_KINDS) as u64);
    }

    /// The global item-size backstop is reachable when every budgeted kind
    /// spends its full allowance: the row holds exactly the cap, no more.
    #[test]
    fn every_kind_at_full_budget_fills_the_row_to_exactly_the_item_cap() {
        let mut row = ReportRow::new("u1");
        for k in 0..REPORT_MAX_NOTE_KINDS {
            for i in 0..REPORT_MAX_NOTES_PER_KIND {
                row.push_note(note(&format!("kind{k}"), i));
            }
        }
        assert_eq!(row.notes.len(), REPORT_MAX_NOTE_KINDS * REPORT_MAX_NOTES_PER_KIND);
        assert!(
            row.notes.len() <= REPORT_MAX_NOTES,
            "the budgets must never sum past the item-size cap",
        );
        assert_eq!(row.notes_dropped, 0);
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
