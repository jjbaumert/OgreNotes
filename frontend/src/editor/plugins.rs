// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use super::state::{EditorState, Transaction};
use super::transform::{Step, StepMap};

/// A plugin that can observe and modify editor state transitions.
pub trait Plugin {
    /// Called when the editor state is first created.
    fn init(&mut self, _state: &EditorState) {}

    /// Called after a transaction is applied, with old and new state.
    fn apply(
        &mut self,
        _transaction: &Transaction,
        _old_state: &EditorState,
        _new_state: &EditorState,
    ) {
    }

    /// Optional: filter a transaction before it is applied.
    /// Return false to reject the transaction.
    fn filter_transaction(&self, _transaction: &Transaction) -> bool {
        true
    }
}

// ─── History Plugin ─────────────────────────────────────────────

/// Undo/redo history plugin.
/// Records steps from transactions and supports undo/redo operations.
pub struct HistoryPlugin {
    /// Stack of undoable step groups.
    undo_stack: Vec<UndoEntry>,
    /// Stack of redoable step groups.
    redo_stack: Vec<UndoEntry>,
    /// Maximum number of entries in the undo stack.
    max_depth: usize,
    /// Time of the last recorded change (for grouping).
    last_change_time: Option<f64>,
    /// Delay in ms before starting a new undo group.
    new_group_delay: f64,
}

/// An entry in the undo/redo stack.
#[derive(Debug, Clone)]
struct UndoEntry {
    /// The steps to undo this entry (inverted steps).
    steps: Vec<Step>,
}

impl HistoryPlugin {
    /// Create a new history plugin with default settings.
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_depth: 100,
            new_group_delay: 500.0,
            last_change_time: None,
        }
    }

    /// Create with custom depth and group delay.
    pub fn with_options(max_depth: usize, new_group_delay_ms: f64) -> Self {
        Self {
            max_depth,
            new_group_delay: new_group_delay_ms,
            ..Self::new()
        }
    }

    /// Remap all undo/redo entries through step maps from a concurrent change.
    /// This keeps stored inverted steps' positions aligned with the current
    /// document state, enabling correct undo after remote edits.
    pub fn remap_through(&mut self, maps: &[StepMap]) {
        if maps.is_empty() {
            return;
        }
        for entry in &mut self.undo_stack {
            for step in &mut entry.steps {
                for map in maps {
                    *step = step.map(map);
                }
            }
        }
        for entry in &mut self.redo_stack {
            for step in &mut entry.steps {
                for map in maps {
                    *step = step.map(map);
                }
            }
        }
    }

    /// Record a transaction's steps for undo.
    pub fn record(&mut self, transaction: &Transaction, old_doc: &super::model::Node) {
        if !transaction.doc_changed {
            return;
        }

        // Mentions spec §5 (Task 3): a "skip" transaction records nothing
        // at all — not even a new entry — and must NOT clear the redo
        // stack either, unlike a normal recorded change.
        if transaction.meta.get("history") == Some(&"skip".to_string()) {
            return;
        }

        // Skip if the transaction is marked as an undo/redo operation
        if transaction.meta.get("history") == Some(&"undo".to_string())
            || transaction.meta.get("history") == Some(&"redo".to_string())
        {
            return;
        }

        // NOTE: do NOT remap existing undo/redo entries through this
        // transaction's own maps. The undo stack is LIFO — an earlier entry is
        // unwound only after every later entry has been undone, which returns
        // the document to that earlier entry's original coordinate space. So
        // earlier entries must stay as recorded. Remapping them forward through
        // a later *local* edit corrupts them whenever that edit shifts
        // positions ahead of them (e.g. backspacing earlier in the document),
        // which is the #151 "undo gets stuck / stray heading character" bug.
        // remap_through is for genuinely concurrent (remote) edits, which are
        // applied separately and are not recorded here.

        // Compute inverted steps
        let mut inverted = Vec::new();
        let mut doc = old_doc.clone();
        for step in &transaction.steps {
            inverted.push(step.invert(&doc));
            if let Ok((new_doc, _)) = step.apply(&doc) {
                doc = new_doc;
            }
        }
        inverted.reverse(); // Undo steps must be applied in reverse order

        // Decide whether to group with the previous entry. A "merge"
        // transaction (mentions spec §5: paste-then-async-convert must be
        // ONE undo) groups with the previous entry regardless of elapsed
        // time — and regardless of whether `last_change_time` was ever
        // set — as long as there IS a previous entry to group into.
        let now = current_time_ms();
        let force_merge = transaction.meta.get("history") == Some(&"merge".to_string());
        let time_grouped = self
            .last_change_time
            .map(|t| now - t < self.new_group_delay)
            .unwrap_or(false);
        let should_group = (force_merge || time_grouped) && !self.undo_stack.is_empty();

        if should_group {
            // Merge with the last undo entry.
            // New inverted steps must come FIRST (LIFO: undo B before A).
            if let Some(last) = self.undo_stack.last_mut() {
                let mut merged = inverted;
                merged.extend(last.steps.drain(..));
                last.steps = merged;
            }
        } else {
            // New undo entry
            self.undo_stack.push(UndoEntry { steps: inverted });

            // Enforce max depth
            if self.undo_stack.len() > self.max_depth {
                self.undo_stack.remove(0);
            }
        }

        self.last_change_time = Some(now);

        // Any new change clears the redo stack
        self.redo_stack.clear();
    }

    /// Perform an undo operation.
    /// Returns a transaction that reverses the last change, or None if nothing to undo.
    /// If step application fails, the entry is preserved on the undo stack.
    pub fn undo(&mut self, state: &EditorState) -> Option<Transaction> {
        let entry = self.undo_stack.last()?.clone();

        // Try to apply the inverted steps
        let mut txn = state.transaction();
        for step in &entry.steps {
            match txn.step(step.clone()) {
                Ok(t) => txn = t,
                Err(_) => return None, // entry stays on undo stack
            }
        }
        txn = txn.set_meta("history", "undo");

        // Success -- remove from undo stack
        self.undo_stack.pop();

        // Push the forward steps to the redo stack
        let mut redo_steps = Vec::new();
        let mut doc = state.doc.clone();
        for step in &entry.steps {
            redo_steps.push(step.invert(&doc));
            if let Ok((new_doc, _)) = step.apply(&doc) {
                doc = new_doc;
            }
        }
        redo_steps.reverse();
        self.redo_stack.push(UndoEntry { steps: redo_steps });

        Some(txn)
    }

    /// Perform a redo operation.
    /// Returns a transaction that re-applies the last undone change, or None.
    /// If step application fails, the entry is preserved on the redo stack.
    pub fn redo(&mut self, state: &EditorState) -> Option<Transaction> {
        let entry = self.redo_stack.last()?.clone();

        let mut txn = state.transaction();
        for step in &entry.steps {
            match txn.step(step.clone()) {
                Ok(t) => txn = t,
                Err(_) => return None, // entry stays on redo stack
            }
        }
        txn = txn.set_meta("history", "redo");

        // Success -- remove from redo stack
        self.redo_stack.pop();

        // Push inverted steps back to the undo stack
        let mut undo_steps = Vec::new();
        let mut doc = state.doc.clone();
        for step in &entry.steps {
            undo_steps.push(step.invert(&doc));
            if let Ok((new_doc, _)) = step.apply(&doc) {
                doc = new_doc;
            }
        }
        undo_steps.reverse();
        self.undo_stack.push(UndoEntry { steps: undo_steps });

        Some(txn)
    }

    /// Whether undo is available.
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Whether redo is available.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Clear all history.
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_change_time = None;
    }
}

impl Plugin for HistoryPlugin {
    fn apply(
        &mut self,
        transaction: &Transaction,
        old_state: &EditorState,
        _new_state: &EditorState,
    ) {
        self.record(transaction, &old_state.doc);
    }
}

impl Default for HistoryPlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// Get the current time in milliseconds.
/// Uses performance.now() in the browser, falls back to 0.0 in tests.
fn current_time_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.performance())
            .map(|p| p.now())
            .unwrap_or(0.0)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0 // In non-WASM tests, always return 0 (forces new group every time)
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::model::*;
    use crate::editor::selection::Selection;
    use crate::editor::state::EditorState;

    fn simple_doc() -> Node {
        Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("Hello world")]),
            )]),
        )
    }

    #[test]
    fn undo_single_insert() {
        let state = EditorState::create_default(simple_doc());
        let original_doc = state.doc.clone();

        // Insert text
        let txn = state.transaction().insert_text("X").unwrap();
        let new_state = state.apply(txn.clone());
        assert_ne!(new_state.doc, original_doc);

        // Record and undo
        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);
        assert!(history.can_undo());

        let undo_txn = history.undo(&new_state).unwrap();
        let restored = new_state.apply(undo_txn);
        assert_eq!(restored.doc, original_doc);
    }

    #[test]
    fn redo_after_undo() {
        let state = EditorState::create_default(simple_doc());

        let txn = state.transaction().insert_text("X").unwrap();
        let after_insert = state.apply(txn.clone());

        let mut history = HistoryPlugin::new();
        history.record(&txn, &state.doc);

        // Undo
        let undo_txn = history.undo(&after_insert).unwrap();
        let after_undo = after_insert.apply(undo_txn);
        assert_eq!(after_undo.doc, state.doc);
        assert!(history.can_redo());

        // Redo
        let redo_txn = history.redo(&after_undo).unwrap();
        let after_redo = after_undo.apply(redo_txn);
        assert_eq!(after_redo.doc, after_insert.doc);
    }

    #[test]
    fn new_change_clears_redo() {
        let state = EditorState::create_default(simple_doc());

        let txn1 = state.transaction().insert_text("A").unwrap();
        let state2 = state.apply(txn1.clone());

        let mut history = HistoryPlugin::new();
        history.record(&txn1, &state.doc);

        // Undo
        let undo_txn = history.undo(&state2).unwrap();
        let state3 = state2.apply(undo_txn);
        assert!(history.can_redo());

        // New change should clear redo
        let txn2 = state3.transaction().insert_text("B").unwrap();
        history.record(&txn2, &state3.doc);
        assert!(!history.can_redo());
    }

    #[test]
    fn undo_nothing_returns_none() {
        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::new();
        assert!(!history.can_undo());
        assert!(history.undo(&state).is_none());
    }

    #[test]
    fn redo_nothing_returns_none() {
        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::new();
        assert!(!history.can_redo());
        assert!(history.redo(&state).is_none());
    }

    #[test]
    fn max_depth_enforced() {
        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::with_options(3, 0.0);
        let mut current = state.clone();

        for i in 0..5 {
            let txn = current
                .transaction()
                .insert_text(&format!("{i}"))
                .unwrap();
            history.record(&txn, &current.doc);
            current = current.apply(txn);
        }

        // Only 3 undo entries should remain
        assert_eq!(history.undo_stack.len(), 3);
    }

    #[test]
    fn non_changing_transaction_not_recorded() {
        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::new();

        // A transaction with no steps
        let txn = state
            .transaction()
            .set_selection(Selection::cursor(3));
        history.record(&txn, &state.doc);

        assert!(!history.can_undo());
    }

    #[test]
    fn clear_history() {
        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::new();

        let txn = state.transaction().insert_text("X").unwrap();
        history.record(&txn, &state.doc);
        assert!(history.can_undo());

        history.clear();
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }

    // ── meta("history") == "merge" / "skip" (mentions spec §5: single-undo
    // paste-then-async-convert) ──

    #[test]
    fn merge_meta_groups_with_previous_entry_regardless_of_time() {
        // new_group_delay = 0.0 so the native `current_time_ms() == 0`
        // quirk can't accidentally satisfy this on its own: without the
        // merge meta, two back-to-back records here would NOT group
        // (0.0 - 0.0 < 0.0 is false). The merge meta must force grouping
        // anyway.
        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::with_options(100, 0.0);

        let txn_a = state.transaction().insert_text("A").unwrap();
        let state_a = state.apply(txn_a.clone());
        history.record(&txn_a, &state.doc);
        assert_eq!(history.undo_stack.len(), 1);

        let txn_b = state_a.transaction().insert_text("B").unwrap();
        let txn_b = txn_b.set_meta("history", "merge");
        let state_b = state_a.apply(txn_b.clone());
        history.record(&txn_b, &state_a.doc);

        // Grouped into the same entry, not a second one.
        assert_eq!(history.undo_stack.len(), 1);

        // A single undo reverts BOTH insertions.
        let undo_txn = history.undo(&state_b).unwrap();
        let restored = state_b.apply(undo_txn);
        assert_eq!(restored.doc, state.doc);
        assert!(!history.can_undo());
    }

    #[test]
    fn merge_meta_groups_even_when_last_change_time_is_none() {
        // Edge case named explicitly by the contract: force_merge must
        // group with a non-empty undo_stack even if last_change_time was
        // never set (e.g. history state constructed/seeded some other
        // way than through record()).
        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::new();
        history.undo_stack.push(UndoEntry { steps: Vec::new() });
        assert!(history.last_change_time.is_none());

        let txn = state
            .transaction()
            .insert_text("X")
            .unwrap()
            .set_meta("history", "merge");
        history.record(&txn, &state.doc);

        // Merged into the seeded entry, not pushed as a second one.
        assert_eq!(history.undo_stack.len(), 1);
    }

    #[test]
    fn skip_meta_records_nothing() {
        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::new();

        let txn_a = state.transaction().insert_text("A").unwrap();
        let state_a = state.apply(txn_a.clone());
        history.record(&txn_a, &state.doc);
        assert!(history.can_undo());

        // Build up a non-empty redo stack so we can prove skip doesn't
        // clear it either.
        let undo_txn = history.undo(&state_a).unwrap();
        let state_undo = state_a.apply(undo_txn);
        assert!(history.can_redo());

        let txn_b = state_undo
            .transaction()
            .insert_text("B")
            .unwrap()
            .set_meta("history", "skip");
        history.record(&txn_b, &state_undo.doc);

        assert_eq!(history.undo_stack.len(), 0); // nothing recorded
        assert!(history.can_redo()); // redo stack NOT cleared
    }

    // ── Undo after concurrent edit (known bug) ──

    #[test]
    fn undo_list_wrap_after_concurrent_text_edit() {
        // Reproduces: User A wraps paragraph in bullet list, User B edits text,
        // User A undoes. The undo should unwrap the list while preserving B's edit.
        //
        // This test documents the expected correct behavior. It will fail until
        // HistoryPlugin remaps undo step positions through concurrent changes.

        use crate::editor::commands::toggle_list;
        use std::cell::RefCell;

        let state = EditorState::create_default(simple_doc());
        let mut history = HistoryPlugin::new();

        // ── User A: wrap paragraph in bullet list ──
        let wrap_txn: RefCell<Option<Transaction>> = RefCell::new(None);
        toggle_list(
            NodeType::BulletList,
            NodeType::ListItem,
            &state,
            Some(&|txn| { *wrap_txn.borrow_mut() = Some(txn); }),
        );
        let wrap_txn = wrap_txn.into_inner().expect("toggle_list should dispatch");
        history.record(&wrap_txn, &state.doc);
        let after_wrap = state.apply(wrap_txn);

        // Verify: doc is now BulletList > ListItem > Paragraph("Hello world")
        let list = after_wrap.doc.child(0).unwrap();
        assert_eq!(list.node_type(), Some(NodeType::BulletList));
        assert_eq!(list.text_content(), "Hello world");

        // ── User B: concurrent text edit (simulate remote update) ──
        // Insert " beautiful" after "Hello" inside the nested paragraph.
        // Structure: Doc(0) > BulletList(1) > ListItem(2) > Paragraph(3) > "Hello world"
        // "Hello" ends at position 3+5=8, so we insert at position 8.
        let after_remote = {
            let insert_pos = 8; // after "Hello" inside the nested para
            let remote_state = EditorState {
                selection: Selection::cursor(insert_pos),
                ..after_wrap.clone()
            };
            let remote_txn = remote_state.transaction()
                .insert_text(" beautiful")
                .expect("insert should succeed");
            // Do NOT record in history (remote changes aren't recorded),
            // but DO remap existing undo entries through the remote change's maps.
            history.remap_through(&remote_txn.maps);
            after_wrap.apply(remote_txn)
        };

        // Verify: text is now "Hello beautiful world"
        assert_eq!(after_remote.doc.child(0).unwrap().text_content(), "Hello beautiful world");

        // ── User A: undo ──
        let undo_txn = history.undo(&after_remote);

        // The undo should succeed (not return None)
        assert!(undo_txn.is_some(),
            "Undo should succeed even after concurrent edit, but returned None \
            (stale positions in inverted step)");

        let after_undo = after_remote.apply(undo_txn.unwrap());

        // The paragraph should be unwrapped from the list
        let first = after_undo.doc.child(0).unwrap();
        assert_eq!(first.node_type(), Some(NodeType::Paragraph),
            "Undo should unwrap the list back to a paragraph, \
            but got {:?}. Doc: {:?}", first.node_type(), after_undo.doc);

        // User B's text edit should be preserved
        assert!(first.text_content().contains("beautiful"),
            "User B's concurrent edit should be preserved after undo, \
            but got: '{}'. Doc: {:?}", first.text_content(), after_undo.doc);
    }

    // ── Regression: undo after a *remote* edit, via the production map ──

    #[test]
    fn undo_after_remote_edit_uses_synthesized_map() {
        // #151 generalization (remote form): the user appends "!" to the
        // 2nd paragraph (recorded), a collaborator then inserts "XX" at the
        // START of the 1st paragraph — shifting every later position,
        // including the recorded undo step — and the undo must still cleanly
        // remove the append at its now-shifted offset, never corrupt the doc.
        //
        // Crucially this drives the PRODUCTION map source: the map is
        // synthesized purely from (old_doc, new_doc) via
        // step_map_for_doc_swap — exactly what the remote sink does — not
        // from a local transaction's own maps (as the older test above does).
        use crate::editor::transform::step_map_for_doc_swap;

        let doc = Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("Hello")]),
                ),
                Node::element_with_content(
                    NodeType::Paragraph,
                    Fragment::from(vec![Node::text("World")]),
                ),
            ]),
        );
        let state = EditorState::create_default(doc);
        let mut history = HistoryPlugin::new();

        // Local edit: append "!" at the end of P_b ("World").
        // P_a "Hello": Open@0 H@1..o@5 Close@6 ; P_b "World": Open@7 W@8..d@12 Close@13.
        let end_of_pb = 13;
        let local_txn = EditorState { selection: Selection::cursor(end_of_pb), ..state.clone() }
            .transaction()
            .insert_text("!")
            .unwrap();
        history.record(&local_txn, &state.doc);
        let state1 = state.apply(local_txn);
        assert_eq!(state1.doc.child(1).unwrap().text_content(), "World!");

        // Remote edit: insert "XX" at the start of P_a. Built as a
        // transaction only to PRODUCE the post-merge doc; its maps are
        // discarded — the map is re-derived from the doc pair below.
        let start_of_pa = 1;
        let remote_txn = EditorState { selection: Selection::cursor(start_of_pa), ..state1.clone() }
            .transaction()
            .insert_text("XX")
            .unwrap();
        let state2 = state1.apply(remote_txn);
        assert_eq!(state2.doc.child(0).unwrap().text_content(), "XXHello");
        assert_eq!(state2.doc.child(1).unwrap().text_content(), "World!");

        // Production path: synthesize the map from the swap, then remap.
        let map = step_map_for_doc_swap(&state1.doc, &state2.doc);
        history.remap_through(std::slice::from_ref(&map));

        // Undo: removes the "!" at its shifted offset; the remote XX stays.
        let undo_txn = history
            .undo(&state2)
            .expect("undo should succeed after the remote edit");
        let state3 = state2.apply(undo_txn);
        assert_eq!(
            state3.doc.child(0).unwrap().text_content(),
            "XXHello",
            "the remote insert must be preserved through undo",
        );
        assert_eq!(
            state3.doc.child(1).unwrap().text_content(),
            "World",
            "the local append must be cleanly undone at the shifted offset",
        );
    }

    // ── Regression: undo after a *local* edit earlier in the document (#151) ──

    #[test]
    fn undo_after_earlier_local_edit_restores_original() {
        // Regression for #151 ("Undo Broken"): the user built a document, then
        // backspaced a character earlier in it (inside "Some Text"), then undid
        // repeatedly. Undo got stuck, leaving a stray character that still
        // carried a later block's heading formatting.
        //
        // Root cause: the undo stack is LIFO — an earlier entry is unwound only
        // after every later entry, so it must stay in its *original* coordinate
        // space. record() must NOT remap earlier entries through a subsequent
        // *local* edit's maps (remap_through is only for genuinely concurrent /
        // remote edits, which are applied separately and not recorded here).
        //
        // new_group_delay = 0.0 forces each edit into its own undo entry,
        // matching a user who pauses between edits.
        let state = EditorState::create_default(simple_doc());
        let original_doc = state.doc.clone();
        let mut history = HistoryPlugin::with_options(100, 0.0);

        // Edit 1 (recorded first): append "Z" at the end of the paragraph.
        let end = 12; // just after "Hello world"
        let txn1 = EditorState { selection: Selection::cursor(end), ..state.clone() }
            .transaction()
            .insert_text("Z")
            .unwrap();
        history.record(&txn1, &state.doc);
        let state1 = state.apply(txn1);
        assert_eq!(state1.doc.child(0).unwrap().text_content(), "Hello worldZ");

        // Edit 2 (recorded second, more recent): delete the leading "H".
        // This shifts every position after it, including edit 1's recorded
        // delete — but LIFO undo must not depend on remapping edit 1.
        let txn2 = state1.transaction().delete(1, 2).unwrap();
        history.record(&txn2, &state1.doc);
        let state2 = state1.apply(txn2);
        assert_eq!(state2.doc.child(0).unwrap().text_content(), "ello worldZ");

        // Undo edit 2: restores the "H".
        let u2 = history.undo(&state2).expect("undo of edit 2 should succeed");
        let after_u2 = state2.apply(u2);
        assert_eq!(after_u2.doc.child(0).unwrap().text_content(), "Hello worldZ");

        // Undo edit 1: must remove the appended "Z" (not a misaligned char).
        let u1 = history.undo(&after_u2).expect("undo of edit 1 should succeed");
        let after_u1 = after_u2.apply(u1);
        assert_eq!(
            after_u1.doc, original_doc,
            "undo after an earlier local edit must restore the original doc; \
             got: '{}'",
            after_u1.doc.child(0).unwrap().text_content(),
        );
    }
}
