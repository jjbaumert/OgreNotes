use super::state::{EditorState, Transaction};
use super::transform::Step;

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

    /// Record a transaction's steps for undo.
    pub fn record(&mut self, transaction: &Transaction, old_doc: &super::model::Node) {
        if !transaction.doc_changed {
            return;
        }

        // Skip if the transaction is marked as an undo/redo operation
        if transaction.meta.get("history") == Some(&"undo".to_string())
            || transaction.meta.get("history") == Some(&"redo".to_string())
        {
            return;
        }

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

        // Decide whether to group with the previous entry
        let now = current_time_ms();
        let should_group = self
            .last_change_time
            .map(|t| now - t < self.new_group_delay)
            .unwrap_or(false)
            && !self.undo_stack.is_empty();

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
}
