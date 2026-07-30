// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Inventory rows for the Quip import manifest (Phase 1). All rows share
//! the import partition `PK = IMPORT#<import_id>`; none carries a token.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadState {
    Pending,
    ContentDone,
    CommentsDone,
    Skipped,
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

/// One cross-thread link discovered in `source_quip_thread_id` whose
/// target thread hadn't been imported yet at the time it was encountered.
/// SK = `UNRESOLVED#<source_quip_thread_id>`. Consumed by a later pass
/// (Phase 2b+) that revisits these once every thread has an `ogre_doc_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct UnresolvedRow {
    pub source_quip_thread_id: String,
    pub owner_id: String,
    pub links: Vec<PendingLinkItem>,
}

impl UnresolvedRow {
    pub fn sk(&self) -> String {
        format!("UNRESOLVED#{}", self.source_quip_thread_id)
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
            state: ThreadState::Pending, ogre_doc_id: None };
        assert_eq!(t.sk(), "THREAD#qt1");
    }
}
