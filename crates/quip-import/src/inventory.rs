//! Pure BFS over a selected Quip folder tree. Decoupled from the network
//! via a `fetch_folders` closure so it is unit-testable with an in-memory
//! fixture. Discovers folders + thread IDs with multi-folder membership;
//! thread *metadata* is fetched separately by the caller.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;

use crate::client::{QuipError, QuipFolder};

#[derive(Debug, Clone, PartialEq)]
pub struct InvFolder {
    pub quip_folder_id: String,
    pub title: String,
    pub parent_quip_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvThread {
    pub quip_thread_id: String,
    pub member_folders: Vec<String>,
    pub first_folder: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inventory {
    pub folders: Vec<InvFolder>,
    pub threads: Vec<InvThread>,
}

/// BFS from `roots`, fetching folders in batches via `fetch_folders`.
/// `visited` guards cycles; threads are deduped, accumulating
/// `member_folders` (first encounter sets `first_folder`).
///
/// # Parentage
///
/// `parent_quip_id` is recorded here because **here is the only place it can
/// be**. Quip's `/1/folders/` reply describes a folder as `id` + `title` +
/// `children` and says nothing about who points *at* it (see
/// [`crate::client::QuipFolder`]), so a folder's parent is knowable only as
/// the edge the walk crossed to reach it.
///
/// The rule is *first edge wins*, mirroring `first_folder` for a shared
/// thread: a parent is recorded only on a folder's first discovery, and a
/// selected root is discovered before anything can point at it. The recorded
/// edges are therefore the **BFS tree** — acyclic by construction, even when
/// Quip's own graph contains a cycle or a folder sits under two parents. A
/// consumer that mirrors the tree gets to rely on that; one that recorded the
/// *last* edge instead would not.
pub async fn walk_inventory<F, Fut>(roots: &[String], fetch_folders: F) -> Result<Inventory, QuipError>
where
    F: Fn(Vec<String>) -> Fut,
    Fut: Future<Output = Result<Vec<QuipFolder>, QuipError>>,
{
    let mut queue: VecDeque<String> = roots.iter().cloned().collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut folders: Vec<InvFolder> = Vec::new();
    // child folder id -> the folder it was first discovered through. A
    // selected root never gets an entry, which is what makes it a root.
    let mut parents: HashMap<String, String> = HashMap::new();
    // thread id -> (member_folders in insertion order, first_folder)
    let mut threads: HashMap<String, InvThread> = HashMap::new();
    let mut thread_order: Vec<String> = Vec::new();

    while !queue.is_empty() {
        // Batch this BFS level (Quip /1/folders/ takes multiple ids).
        let batch: Vec<String> = queue.drain(..).filter(|id| visited.insert(id.clone())).collect();
        if batch.is_empty() {
            continue;
        }
        for folder in fetch_folders(batch).await? {
            folders.push(InvFolder {
                quip_folder_id: folder.id.clone(),
                title: folder.title.clone(),
                parent_quip_id: parents.get(&folder.id).cloned(),
            });
            for child in &folder.children {
                if let Some(sub) = &child.folder_id {
                    if !visited.contains(sub) {
                        // `or_insert` is the first-edge-wins rule: two
                        // parents in one BFS level both offer `sub`, and the
                        // queue's drain-time dedup means only one fetch
                        // happens, so the edge has to be settled here.
                        parents.entry(sub.clone()).or_insert_with(|| folder.id.clone());
                        queue.push_back(sub.clone());
                    }
                }
                if let Some(tid) = &child.thread_id {
                    let entry = threads.entry(tid.clone()).or_insert_with(|| {
                        thread_order.push(tid.clone());
                        InvThread {
                            quip_thread_id: tid.clone(),
                            member_folders: Vec::new(),
                            first_folder: folder.id.clone(),
                        }
                    });
                    if !entry.member_folders.contains(&folder.id) {
                        entry.member_folders.push(folder.id.clone());
                    }
                }
            }
        }
    }
    let threads = thread_order.into_iter().map(|id| threads.remove(&id).unwrap()).collect();
    Ok(Inventory { folders, threads })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{QuipFolder, QuipFolderChild};
    use std::collections::HashMap;

    fn child_thread(id: &str) -> QuipFolderChild {
        QuipFolderChild {
            thread_id: Some(id.into()),
            folder_id: None,
        }
    }
    fn child_folder(id: &str) -> QuipFolderChild {
        QuipFolderChild {
            thread_id: None,
            folder_id: Some(id.into()),
        }
    }

    // Fixture: root -> [thread t1, subfolder f2]; f2 -> [thread t1 (shared), t2].
    fn fixture() -> HashMap<String, QuipFolder> {
        HashMap::from([
            (
                "root".into(),
                QuipFolder {
                    id: "root".into(),
                    title: "Root".into(),
                    children: vec![child_thread("t1"), child_folder("f2")],
                },
            ),
            (
                "f2".into(),
                QuipFolder {
                    id: "f2".into(),
                    title: "Sub".into(),
                    children: vec![child_thread("t1"), child_thread("t2")],
                },
            ),
        ])
    }

    async fn fetch(
        ids: Vec<String>,
        fx: &HashMap<String, QuipFolder>,
    ) -> Result<Vec<QuipFolder>, crate::client::QuipError> {
        Ok(ids.iter().filter_map(|id| fx.get(id).cloned()).collect())
    }

    #[tokio::test]
    async fn bfs_discovers_all_and_dedups_shared_thread() {
        let fx = fixture();
        let inv = walk_inventory(&["root".into()], |ids| fetch(ids, &fx))
            .await
            .unwrap();
        assert_eq!(inv.folders.len(), 2, "root + f2");
        let t1 = inv.threads.iter().find(|t| t.quip_thread_id == "t1").unwrap();
        assert_eq!(t1.first_folder, "root");
        let mut mf = t1.member_folders.clone();
        mf.sort();
        assert_eq!(
            mf,
            vec!["f2".to_string(), "root".to_string()],
            "shared thread lists both folders once"
        );
        assert_eq!(
            inv.threads.iter().filter(|t| t.quip_thread_id == "t1").count(),
            1,
            "no duplicate rows"
        );
    }

    /// Parentage is only ever observable from the *edge the BFS crossed* —
    /// Quip's `/1/folders/` reply carries `id`, `title` and `children`, and
    /// nothing pointing upward (see `client::FolderMeta`). So the walk is the
    /// one place that can record it, and this pins that it does: a selected
    /// root has no parent inside the scope, and a sub-folder names the folder
    /// it was reached through.
    #[tokio::test]
    async fn bfs_records_the_edge_each_subfolder_was_discovered_through() {
        let fx = fixture();
        let inv = walk_inventory(&["root".into()], |ids| fetch(ids, &fx))
            .await
            .unwrap();
        let folder = |id: &str| inv.folders.iter().find(|f| f.quip_folder_id == id).unwrap();
        assert_eq!(
            folder("root").parent_quip_id,
            None,
            "a selected root has no parent inside the selected scope",
        );
        assert_eq!(folder("f2").parent_quip_id.as_deref(), Some("root"));
    }

    /// BFS order is already parent-before-child, so a consumer that walks
    /// `folders` in order never meets a child before its parent. Pinned
    /// because the folder-mirroring pass relies on parentage being a DAG.
    #[tokio::test]
    async fn folders_come_back_parent_before_child() {
        let fx = fixture();
        let inv = walk_inventory(&["root".into()], |ids| fetch(ids, &fx))
            .await
            .unwrap();
        let mut seen: HashSet<&str> = HashSet::new();
        for f in &inv.folders {
            if let Some(p) = &f.parent_quip_id {
                assert!(seen.contains(p.as_str()), "{} precedes its parent {p}", f.quip_folder_id);
            }
            seen.insert(&f.quip_folder_id);
        }
    }

    /// A folder reachable through two parents is still one row, and it keeps
    /// the **first** edge the BFS crossed — the same first-encounter rule
    /// `first_folder` already uses for a shared thread. Recording the second
    /// edge instead would let the recorded parentage contain a cycle.
    #[tokio::test]
    async fn a_folder_reachable_two_ways_keeps_the_first_edge() {
        // root -> [x, y]; x -> shared; y -> shared. x and y land in the same
        // BFS level, so `shared` is offered twice before it is ever fetched.
        let fx = HashMap::from([
            (
                "root".to_string(),
                QuipFolder {
                    id: "root".into(),
                    title: "Root".into(),
                    children: vec![child_folder("x"), child_folder("y")],
                },
            ),
            (
                "x".to_string(),
                QuipFolder {
                    id: "x".into(),
                    title: "X".into(),
                    children: vec![child_folder("shared")],
                },
            ),
            (
                "y".to_string(),
                QuipFolder {
                    id: "y".into(),
                    title: "Y".into(),
                    children: vec![child_folder("shared")],
                },
            ),
            (
                "shared".to_string(),
                QuipFolder {
                    id: "shared".into(),
                    title: "Shared".into(),
                    children: vec![],
                },
            ),
        ]);
        let inv = walk_inventory(&["root".into()], |ids| fetch(ids, &fx))
            .await
            .unwrap();
        assert_eq!(
            inv.folders.iter().filter(|f| f.quip_folder_id == "shared").count(),
            1,
            "one row per folder, however many parents offered it",
        );
        let shared = inv.folders.iter().find(|f| f.quip_folder_id == "shared").unwrap();
        assert_eq!(shared.parent_quip_id.as_deref(), Some("x"), "first edge wins");
    }

    #[tokio::test]
    async fn bfs_terminates_on_cycle() {
        // f_a -> f_b -> f_a (cycle). Must not infinite-loop.
        let fx = HashMap::from([
            (
                "a".to_string(),
                QuipFolder {
                    id: "a".into(),
                    title: "A".into(),
                    children: vec![child_folder("b")],
                },
            ),
            (
                "b".to_string(),
                QuipFolder {
                    id: "b".into(),
                    title: "B".into(),
                    children: vec![child_folder("a")],
                },
            ),
        ]);
        let inv = walk_inventory(&["a".into()], |ids| fetch(ids, &fx))
            .await
            .unwrap();
        assert_eq!(inv.folders.len(), 2);
    }

    /// A cycle in Quip's own graph must not become a cycle in the recorded
    /// parentage. It cannot, because a parent is recorded only on a folder's
    /// **first** discovery and a selected root is discovered before anything
    /// can point at it — so the recorded edges are the BFS tree, which is
    /// acyclic by construction.
    #[tokio::test]
    async fn a_cycle_still_records_an_acyclic_parent_chain() {
        let fx = HashMap::from([
            (
                "a".to_string(),
                QuipFolder {
                    id: "a".into(),
                    title: "A".into(),
                    children: vec![child_folder("b")],
                },
            ),
            (
                "b".to_string(),
                QuipFolder {
                    id: "b".into(),
                    title: "B".into(),
                    children: vec![child_folder("a")],
                },
            ),
        ]);
        let inv = walk_inventory(&["a".into()], |ids| fetch(ids, &fx))
            .await
            .unwrap();
        let folder = |id: &str| inv.folders.iter().find(|f| f.quip_folder_id == id).unwrap();
        assert_eq!(folder("a").parent_quip_id, None, "the selected root stays a root");
        assert_eq!(folder("b").parent_quip_id.as_deref(), Some("a"));
    }
}
