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
pub async fn walk_inventory<F, Fut>(roots: &[String], fetch_folders: F) -> Result<Inventory, QuipError>
where
    F: Fn(Vec<String>) -> Fut,
    Fut: Future<Output = Result<Vec<QuipFolder>, QuipError>>,
{
    let mut queue: VecDeque<String> = roots.iter().cloned().collect();
    let mut visited: HashSet<String> = HashSet::new();
    let mut folders: Vec<InvFolder> = Vec::new();
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
                parent_quip_id: None,
            });
            for child in &folder.children {
                if let Some(sub) = &child.folder_id {
                    if !visited.contains(sub) {
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
}
