use super::commands;
use super::model::{MarkType, NodeType};
use super::state::{EditorState, Transaction};

/// A keyboard shortcut specification.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySpec {
    /// The key name (lowercase), e.g., "b", "1", "enter".
    pub key: String,
    /// Whether Ctrl (or Cmd on Mac) is required.
    pub ctrl_or_meta: bool,
    /// Whether Shift is required.
    pub shift: bool,
    /// Whether Alt is required.
    pub alt: bool,
}

impl KeySpec {
    /// Parse a key specification string like "Mod-b", "Mod-Shift-7", "Enter".
    /// "Mod" means Ctrl on non-Mac, Cmd on Mac.
    pub fn parse(spec: &str) -> Self {
        let mut ctrl_or_meta = false;
        let mut shift = false;
        let mut alt = false;
        let mut key = String::new();

        for part in spec.split('-') {
            match part {
                "Mod" | "Ctrl" | "Meta" => ctrl_or_meta = true,
                "Shift" => shift = true,
                "Alt" => alt = true,
                k => key = k.to_lowercase(),
            }
        }

        Self {
            key,
            ctrl_or_meta,
            shift,
            alt,
        }
    }

    /// Check if a keyboard event matches this spec.
    pub fn matches_event(&self, key: &str, ctrl: bool, meta: bool, shift: bool, alt: bool) -> bool {
        let key_lower = key.to_lowercase();
        self.key == key_lower
            && self.ctrl_or_meta == (ctrl || meta)
            && self.shift == shift
            && self.alt == alt
    }
}

/// A command that can be triggered by a keyboard shortcut.
pub type KeyCommand = Box<dyn Fn(&EditorState, Option<&dyn Fn(Transaction)>) -> bool>;

/// A keymap maps keyboard shortcuts to commands.
pub struct Keymap {
    bindings: Vec<(KeySpec, KeyCommand)>,
}

impl Keymap {
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }

    /// Add a binding.
    pub fn bind(mut self, spec: &str, command: KeyCommand) -> Self {
        self.bindings.push((KeySpec::parse(spec), command));
        self
    }

    /// Try to handle a keyboard event. Returns true if a binding matched.
    pub fn handle(
        &self,
        key: &str,
        ctrl: bool,
        meta: bool,
        shift: bool,
        alt: bool,
        state: &EditorState,
        dispatch: &dyn Fn(Transaction),
    ) -> bool {
        for (spec, command) in &self.bindings {
            if spec.matches_event(key, ctrl, meta, shift, alt) {
                if command(state, Some(dispatch)) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if any binding matches (without dispatching).
    pub fn has_binding(&self, key: &str, ctrl: bool, meta: bool, shift: bool, alt: bool) -> bool {
        self.bindings
            .iter()
            .any(|(spec, _)| spec.matches_event(key, ctrl, meta, shift, alt))
    }
}

/// Build the default keymap with all MVP shortcuts.
pub fn default_keymap() -> Keymap {
    Keymap::new()
        // Mark toggles
        .bind("Mod-b", Box::new(|s, d| commands::toggle_mark(MarkType::Bold, s, d)))
        .bind("Mod-i", Box::new(|s, d| commands::toggle_mark(MarkType::Italic, s, d)))
        .bind("Mod-u", Box::new(|s, d| commands::toggle_mark(MarkType::Underline, s, d)))
        .bind("Mod-Shift-s", Box::new(|s, d| commands::toggle_mark(MarkType::Strike, s, d)))
        .bind("Mod-Shift-x", Box::new(|s, d| commands::toggle_mark(MarkType::Strike, s, d)))
        .bind("Mod-e", Box::new(|s, d| commands::toggle_mark(MarkType::Code, s, d)))
        .bind("Mod-Shift-k", Box::new(|s, d| commands::toggle_mark(MarkType::Code, s, d)))
        // Headings
        .bind("Mod-Alt-1", Box::new(|s, d| commands::set_heading(1, s, d)))
        .bind("Mod-Alt-2", Box::new(|s, d| commands::set_heading(2, s, d)))
        .bind("Mod-Alt-3", Box::new(|s, d| commands::set_heading(3, s, d)))
        // Block commands
        .bind("Mod-Alt-0", Box::new(|s, d| commands::set_paragraph(s, d)))
        .bind("Mod-Shift-l", Box::new(|s, d| commands::toggle_list(NodeType::BulletList, NodeType::ListItem, s, d)))
        // Text commands
        .bind("Mod-a", Box::new(commands::select_all))
        // List indent / dedent
        .bind("Tab", Box::new(|s, d| commands::tab_command(s, d)))
        .bind("Shift-Tab", Box::new(|s, d| commands::shift_tab_command(s, d)))
        // Consume browser shortcuts to prevent default browser actions
        .bind("Mod-s", Box::new(|_, _| true)) // prevent browser save dialog
        .bind("Mod-p", Box::new(|_, _| true)) // prevent browser print dialog
        // Note: Shift-Enter is handled by beforeinput's insertLineBreak handler
        // in view.rs, not via keymap, to avoid double-dispatch.
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_key() {
        let spec = KeySpec::parse("Enter");
        assert_eq!(spec.key, "enter");
        assert!(!spec.ctrl_or_meta);
        assert!(!spec.shift);
        assert!(!spec.alt);
    }

    #[test]
    fn parse_mod_key() {
        let spec = KeySpec::parse("Mod-b");
        assert_eq!(spec.key, "b");
        assert!(spec.ctrl_or_meta);
        assert!(!spec.shift);
        assert!(!spec.alt);
    }

    #[test]
    fn parse_mod_shift_key() {
        let spec = KeySpec::parse("Mod-Shift-s");
        assert_eq!(spec.key, "s");
        assert!(spec.ctrl_or_meta);
        assert!(spec.shift);
        assert!(!spec.alt);
    }

    #[test]
    fn parse_mod_alt_key() {
        let spec = KeySpec::parse("Mod-Alt-1");
        assert_eq!(spec.key, "1");
        assert!(spec.ctrl_or_meta);
        assert!(!spec.shift);
        assert!(spec.alt);
    }

    #[test]
    fn parse_shift_enter() {
        let spec = KeySpec::parse("Shift-Enter");
        assert_eq!(spec.key, "enter");
        assert!(!spec.ctrl_or_meta);
        assert!(spec.shift);
        assert!(!spec.alt);
    }

    #[test]
    fn matches_ctrl_b() {
        let spec = KeySpec::parse("Mod-b");
        assert!(spec.matches_event("b", true, false, false, false));
        assert!(spec.matches_event("b", false, true, false, false)); // Cmd on Mac
        assert!(!spec.matches_event("b", false, false, false, false)); // no mod
        assert!(!spec.matches_event("i", true, false, false, false)); // wrong key
    }

    #[test]
    fn matches_shift_enter() {
        let spec = KeySpec::parse("Shift-Enter");
        assert!(spec.matches_event("Enter", false, false, true, false));
        assert!(!spec.matches_event("Enter", false, false, false, false)); // no shift
        assert!(!spec.matches_event("Enter", true, false, true, false)); // extra ctrl
    }

    #[test]
    fn default_keymap_has_bindings() {
        let km = default_keymap();
        assert!(km.has_binding("b", true, false, false, false)); // Ctrl+B
        assert!(km.has_binding("i", true, false, false, false)); // Ctrl+I
        assert!(km.has_binding("a", true, false, false, false)); // Ctrl+A
        // Shift+Enter is handled by beforeinput, not keymap
        assert!(!km.has_binding("Enter", false, false, true, false));
        assert!(!km.has_binding("x", true, false, false, false)); // not bound
    }

    #[test]
    fn keymap_handle_dispatches() {
        use crate::editor::model::{Fragment, Node, NodeType};
        use crate::editor::state::EditorState;
        use crate::editor::selection::Selection;
        use std::cell::RefCell;

        let state = EditorState::create_default(Node::element_with_content(
            NodeType::Doc,
            Fragment::from(vec![Node::element_with_content(
                NodeType::Paragraph,
                Fragment::from(vec![Node::text("test")]),
            )]),
        ));
        let state = EditorState {
            selection: Selection::text(1, 5),
            ..state
        };

        let km = default_keymap();
        let dispatched = RefCell::new(false);

        let handled = km.handle("b", true, false, false, false, &state, &|_txn| {
            *dispatched.borrow_mut() = true;
        });

        assert!(handled);
        assert!(*dispatched.borrow());
    }
}
