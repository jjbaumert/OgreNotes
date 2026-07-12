// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! Phase 5 M-P4 piece A — command palette action registry.
//!
//! A flat thread-local list of `PaletteCommand`s, queried in two
//! ways:
//!
//!   - `matching(query, scope)` returns a metadata snapshot for
//!     rendering (label + shortcut + scope), filtering by
//!     substring match and scope visibility.
//!   - `run(id)` executes the registered action by id.
//!
//! Actions stay in the registry (they're `Box<dyn Fn()>` and not
//! cheaply cloneable); only their metadata leaves via
//! `CommandView`. The split lets the SearchDialog reactively
//! render command lists without dragging closures through Leptos
//! signals.
//!
//! v1 scope: Global commands only. Editor / Spreadsheet / Home-
//! scoped commands need access to the active page's dispatch
//! signals (e.g. the editor's `set_toolbar_command`), which the
//! v1 registration site doesn't have visibility into — that
//! threading lands in M-P4 piece B together with the fuzzy
//! ranker.
//!
//! Substring match is intentionally simple here. Piece B switches
//! to `fuzzy-matcher` for proper Sublime-style ranking and
//! recently-used MRU bias.

use std::cell::RefCell;

pub mod ask_bridge;
pub mod editor_bridge;
pub mod nav_bridge;

/// Local-storage key for the MRU list. Versioned via the schema so
/// a future change to the serialization shape doesn't try to read
/// the old format. localStorage entries are per-origin per-device,
/// so the MRU is a same-device "I usually run these" memory; a
/// future piece can grow a server-side ui_prefs.recent_commands
/// for cross-device sync.
const MRU_LOCALSTORAGE_KEY: &str = "ogrenotes.recent_commands.v1";

/// Cap the MRU at 20 entries. The palette only ever surfaces a
/// 20-row result list, and ranking signal beyond that is noise.
const MRU_MAX: usize = 20;

/// Scope determines when a command is visible in the palette.
/// `Global` is always offered; the others narrow to a specific
/// page context. Scope-restricted commands surface only when the
/// hosting page passes its scope to `matching(..., scope)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandScope {
    Global,
    Editor,
    Spreadsheet,
    Home,
}

impl CommandScope {
    fn css_class(self) -> &'static str {
        match self {
            CommandScope::Global => "is-global",
            CommandScope::Editor => "is-editor",
            CommandScope::Spreadsheet => "is-spreadsheet",
            CommandScope::Home => "is-home",
        }
    }
}

/// Boxed action — runs when the user selects the command from the
/// palette. Returns nothing; side effects are the whole point.
pub type ActionFn = Box<dyn Fn()>;

/// One entry in the registry. The `label_key` is the i18n
/// translation key — resolved at render-time so a runtime locale
/// switch propagates without re-registering every command.
pub struct PaletteCommand {
    pub id: &'static str,
    pub scope: CommandScope,
    pub label_key: &'static str,
    pub shortcut: Option<&'static str>,
    pub action: ActionFn,
}

/// Rendering snapshot — what the palette UI consumes. The action
/// itself stays in the registry; the UI dispatches via `run(id)`.
#[derive(Clone, Debug)]
pub struct CommandView {
    pub id: &'static str,
    pub scope: CommandScope,
    pub label: String,
    pub shortcut: Option<&'static str>,
}

impl CommandView {
    pub fn scope_class(&self) -> &'static str {
        self.scope.css_class()
    }
}

thread_local! {
    /// The registry. RefCell because registration happens once at
    /// boot from a sync context; queries are read-only borrows. We
    /// don't expect runtime additions, but mod-init reentry from a
    /// later piece is supported.
    static REGISTRY: RefCell<Vec<PaletteCommand>> = const { RefCell::new(Vec::new()) };

    /// Most-recently-used command ids, most-recent-first. Mirrors
    /// the localStorage entry under `MRU_LOCALSTORAGE_KEY`. Loaded
    /// once on bootstrap via [`load_recent_from_storage`]; mutated
    /// on every [`run`] call so the next palette open ranks
    /// familiar commands first.
    static RECENT: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Add a command to the registry. Idempotent on `id`: a second
/// registration with the same id replaces the prior entry. Lets
/// piece-B's expanded set override the v1 defaults without
/// fighting over ordering.
pub fn register(cmd: PaletteCommand) {
    REGISTRY.with(|cell| {
        let mut v = cell.borrow_mut();
        if let Some(slot) = v.iter_mut().find(|c| c.id == cmd.id) {
            *slot = cmd;
        } else {
            v.push(cmd);
        }
    });
}

/// Return up to 20 commands ranked against `query`. Three ranking
/// inputs combine:
///
///   1. Fuzzy score (SkimMatcherV2 — subsequence ranking, so "bld"
///      matches "Bold" but ranks below "ld" → "Bold").
///   2. Scope bonus (+50 when the command's scope equals the
///      hosting page's scope, never Global vs Global) so editor
///      cmds outrank generic Global ones at near-equal label
///      match.
///   3. MRU bonus (+100/-N per slot in `RECENT`) so familiar
///      commands the user just ran rank first at near-equal
///      label match. M-P4 piece C addition.
///
/// Empty query: returns MRU-first then registration order (filling
/// up to 20). The "I just opened the palette" state shows what the
/// user usually does.
///
/// `Global`-scoped commands are always visible; scope-specific
/// commands surface only when `scope` matches.
pub fn matching(query: &str, scope: CommandScope) -> Vec<CommandView> {
    use fuzzy_matcher::FuzzyMatcher;
    use fuzzy_matcher::skim::SkimMatcherV2;

    let q = query.trim();
    REGISTRY.with(|cell| {
        let registry = cell.borrow();
        let recent: Vec<String> = RECENT.with(|c| c.borrow().clone());

        // Helper: position of `id` in the MRU list (most-recent = 0).
        let mru_idx = |id: &str| recent.iter().position(|x| x == id);

        // Empty query: order is MRU-first, then unfamiliar cmds in
        // registration order, scoped accordingly.
        if q.is_empty() {
            let mut visible: Vec<&PaletteCommand> = registry
                .iter()
                .filter(|cmd| cmd.scope == CommandScope::Global || cmd.scope == scope)
                .collect();
            visible.sort_by_key(|cmd| {
                // Sort ascending: MRU hits get a low index; un-used
                // get a sentinel that sorts after all hits but keeps
                // their relative registration order.
                mru_idx(cmd.id).map(|i| i as i64).unwrap_or(i64::MAX)
            });
            return visible
                .into_iter()
                .map(|cmd| CommandView {
                    id: cmd.id,
                    scope: cmd.scope,
                    label: crate::i18n::translate(cmd.label_key, None),
                    shortcut: cmd.shortcut,
                })
                .take(20)
                .collect();
        }

        let matcher = SkimMatcherV2::default();
        let mut scored: Vec<(i64, CommandView)> = registry
            .iter()
            .filter(|cmd| cmd.scope == CommandScope::Global || cmd.scope == scope)
            .filter_map(|cmd| {
                let label = crate::i18n::translate(cmd.label_key, None);
                let score = matcher.fuzzy_match(&label, q)?;
                let scope_bonus =
                    if cmd.scope == scope && cmd.scope != CommandScope::Global {
                        50
                    } else {
                        0
                    };
                // MRU bonus: +100 for the most-recent, scaled down
                // 5 points per slot so familiarity decays before
                // capping at +0 around slot 20. Order of magnitude
                // matches scope_bonus so neither one dominates a
                // genuine label-match difference (typically 100s).
                let mru_bonus = mru_idx(cmd.id)
                    .map(|i| (100i64 - (i as i64 * 5)).max(0))
                    .unwrap_or(0);
                Some((
                    score + scope_bonus + mru_bonus,
                    CommandView {
                        id: cmd.id,
                        scope: cmd.scope,
                        label,
                        shortcut: cmd.shortcut,
                    },
                ))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, v)| v).take(20).collect()
    })
}

/// Return only commands that bind a keyboard shortcut. Drives the
/// `?`-prefix "shortcut help" view in the palette — same render
/// path as Action mode, but the row's right-side shortcut chip is
/// the point of the display rather than incidental. Filtered by
/// `scope` like `matching`.
pub fn matching_with_shortcuts(query: &str, scope: CommandScope) -> Vec<CommandView> {
    matching(query, scope)
        .into_iter()
        .filter(|v| v.shortcut.is_some())
        .collect()
}

/// Execute the command identified by `id`. Silent no-op when the
/// id isn't in the registry — defends against a stale render
/// referring to a command that was un-registered between snapshot
/// and click.
///
/// Records the invocation in the MRU list and persists to
/// localStorage so the next palette open ranks it first. We
/// record before the action fires — the user's *intent* is what
/// the MRU captures; whether the action actually mutated state is
/// irrelevant to ranking the next palette open.
pub fn run(id: &str) {
    record_use(id);
    REGISTRY.with(|cell| {
        if let Some(cmd) = cell.borrow().iter().find(|c| c.id == id) {
            (cmd.action)();
        }
    });
}

/// Hydrate the MRU list from `localStorage` on bootstrap. Called
/// once from `main.rs` after `register_defaults`. Failure modes
/// (storage disabled, malformed JSON, missing key) all fall
/// through to "no MRU" — the palette degrades cleanly to
/// registration-order on empty query.
///
/// No-op outside the wasm target so non-wasm unit tests don't
/// panic on `web_sys::window()` static access. The MRU then lives
/// in the in-memory RefCell only.
#[cfg(target_arch = "wasm32")]
pub fn load_recent_from_storage() {
    let Some(window) = web_sys::window() else { return };
    let Ok(Some(storage)) = window.local_storage() else { return };
    let Ok(Some(json)) = storage.get_item(MRU_LOCALSTORAGE_KEY) else {
        return;
    };
    if let Ok(v) = serde_json::from_str::<Vec<String>>(&json) {
        RECENT.with(|cell| *cell.borrow_mut() = v);
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn load_recent_from_storage() {}

/// Bump `id` to the front of the MRU and (on wasm) persist to
/// localStorage. Called from `run`. Mirrors localStorage behavior
/// of the locale switcher — best-effort; private-browsing /
/// quota-full errors drop the write silently and the in-memory
/// list still updates for the current session.
fn record_use(id: &str) {
    RECENT.with(|cell| {
        let mut v = cell.borrow_mut();
        v.retain(|x| x != id);
        v.insert(0, id.to_string());
        v.truncate(MRU_MAX);
        persist_recent(&v);
    });
}

#[cfg(target_arch = "wasm32")]
fn persist_recent(v: &[String]) {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            if let Ok(s) = serde_json::to_string(v) {
                let _ = storage.set_item(MRU_LOCALSTORAGE_KEY, &s);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn persist_recent(_v: &[String]) {}

/// Install the v1 baseline of Global-scope commands. Called once
/// from `main.rs`, synchronously, *before* the i18n harness is
/// initialized — there's no ordering dependency, since each
/// label_key is just a `&'static str` stored on the command struct.
/// Resolution against the active fluent bundle happens later, only
/// when the palette is queried (see `matching()` below), by which
/// point `i18n::init` has long since resolved.
///
/// Piece B extends this with the full ~40-command set, including
/// Editor- and Spreadsheet-scoped commands. The current 5 are
/// the safe, page-independent ones a v1 user always has access
/// to.
pub fn register_defaults() {
    register(PaletteCommand {
        id: "navigation.home",
        scope: CommandScope::Global,
        label_key: "cmd-go-home",
        shortcut: None,
        // #152: client-side via the shell-installed bridge (no full reload).
        action: Box::new(|| nav_bridge::go("/")),
    });
    register(PaletteCommand {
        id: "theme.toggle-dark",
        scope: CommandScope::Global,
        label_key: "cmd-toggle-dark-mode",
        shortcut: None,
        action: Box::new(|| {
            // Read current state from <html data-theme=…>; flip it.
            let Some(document) = web_sys::window().and_then(|w| w.document()) else {
                return;
            };
            let Some(root) = document.document_element() else {
                return;
            };
            let current = root
                .get_attribute("data-theme")
                .unwrap_or_default();
            let next = if current == "dark" {
                Some(crate::theme::ExplicitTheme::Light)
            } else {
                Some(crate::theme::ExplicitTheme::Dark)
            };
            leptos::task::spawn_local(async move {
                // change_theme also persists via /users/me/prefs;
                // failure is best-effort — local apply still wins.
                let _ = crate::theme::change_theme(next).await;
            });
        }),
    });
    register(PaletteCommand {
        id: "navigation.trash",
        scope: CommandScope::Global,
        label_key: "cmd-open-trash",
        shortcut: None,
        action: Box::new(|| nav_bridge::go("/trash")),
    });
    // Phase 6 M-6.2 piece C: open the agentic Ask dialog. Routes
    // through ask_bridge so the per-page visibility signal flips;
    // silent no-op on pages that don't mount AskDialog (admin/MFA).
    register(PaletteCommand {
        id: "ask.open",
        scope: CommandScope::Global,
        label_key: "cmd-ask",
        shortcut: None,
        action: Box::new(ask_bridge::open),
    });
    register(PaletteCommand {
        id: "auth.sign-out",
        scope: CommandScope::Global,
        label_key: "cmd-sign-out",
        shortcut: None,
        action: Box::new(|| {
            leptos::task::spawn_local(async {
                crate::api::client::logout().await;
                if let Some(window) = web_sys::window() {
                    let _ = window.location().set_href("/login");
                }
            });
        }),
    });
    register(PaletteCommand {
        id: "help.about-palette",
        scope: CommandScope::Global,
        label_key: "cmd-about-palette",
        shortcut: None,
        // Opens the Help & Support section of /settings (keyboard shortcuts
        // + build version). #152: client-side via the shell bridge; the
        // settings page reads the hash reactively so the right tab opens.
        action: Box::new(|| nav_bridge::go("/settings#help")),
    });

    // ─── Editor scope (M-P4 piece B) ────────────────────────────
    //
    // Each editor command wraps a ToolbarCommand variant and
    // dispatches through `editor_bridge::dispatch_editor`. When no
    // editor is mounted (e.g. user invokes from the home page) the
    // dispatch is a no-op — `matching()` already filters these out
    // by scope, but the bridge stays safe under any future scope
    // flow that doesn't pre-filter as tightly.
    use crate::components::toolbar::ToolbarCommand;
    let editor_cmd = |id: &'static str, label_key: &'static str, shortcut: Option<&'static str>, cmd: ToolbarCommand| {
        register(PaletteCommand {
            id,
            scope: CommandScope::Editor,
            label_key,
            shortcut,
            action: Box::new(move || {
                editor_bridge::dispatch_editor(cmd.clone());
            }),
        });
    };

    editor_cmd("editor.bold", "cmd-bold", Some("Ctrl+B"), ToolbarCommand::ToggleBold);
    editor_cmd("editor.italic", "cmd-italic", Some("Ctrl+I"), ToolbarCommand::ToggleItalic);
    editor_cmd("editor.underline", "cmd-underline", Some("Ctrl+U"), ToolbarCommand::ToggleUnderline);
    editor_cmd("editor.strike", "cmd-strike", None, ToolbarCommand::ToggleStrike);
    editor_cmd("editor.code", "cmd-code", None, ToolbarCommand::ToggleCode);
    editor_cmd("editor.heading-1", "cmd-heading-1", None, ToolbarCommand::SetHeading(1));
    editor_cmd("editor.heading-2", "cmd-heading-2", None, ToolbarCommand::SetHeading(2));
    editor_cmd("editor.heading-3", "cmd-heading-3", None, ToolbarCommand::SetHeading(3));
    editor_cmd("editor.paragraph", "cmd-paragraph", None, ToolbarCommand::SetParagraph);
    editor_cmd("editor.bullet-list", "cmd-bullet-list", None, ToolbarCommand::ToggleBulletList);
    editor_cmd("editor.ordered-list", "cmd-ordered-list", None, ToolbarCommand::ToggleOrderedList);
    editor_cmd("editor.task-list", "cmd-task-list", None, ToolbarCommand::ToggleTaskList);
    editor_cmd("editor.blockquote", "cmd-blockquote", None, ToolbarCommand::ToggleBlockquote);
    editor_cmd("editor.code-block", "cmd-code-block", None, ToolbarCommand::SetCodeBlock);
    editor_cmd("editor.divider", "cmd-divider", None, ToolbarCommand::InsertHorizontalRule);
    editor_cmd("editor.insert-table", "cmd-insert-table", None, ToolbarCommand::InsertTable);
    editor_cmd("editor.undo", "cmd-undo", Some("Ctrl+Z"), ToolbarCommand::Undo);
    editor_cmd("editor.redo", "cmd-redo", Some("Ctrl+Y"), ToolbarCommand::Redo);

    // #136 — auto-register a palette entry per registered live-app
    // block. A new block appears in ⌘K as soon as it's added to
    // `editor::blocks::BLOCK_INSERTS`.
    for entry in crate::editor::blocks::BLOCK_INSERTS.iter() {
        let id_owned: &'static str = Box::leak(format!("editor.{}", entry.id()).into_boxed_str());
        let cmd = ToolbarCommand::InsertLiveApp(entry.id());
        register(PaletteCommand {
            id: id_owned,
            scope: CommandScope::Editor,
            label_key: entry.label_key(),
            shortcut: None,
            action: Box::new(move || {
                editor_bridge::dispatch_editor(cmd.clone());
            }),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry is thread-local; tests are sequential per
    /// thread but parallel across threads. Each test clears the
    /// registry to a known state via this helper before adding
    /// fixtures.
    fn clear_registry() {
        REGISTRY.with(|cell| cell.borrow_mut().clear());
    }

    #[test]
    fn register_then_match_returns_view() {
        clear_registry();
        register(PaletteCommand {
            id: "test.foo",
            scope: CommandScope::Global,
            label_key: "this-key-does-not-exist",
            shortcut: None,
            action: Box::new(|| {}),
        });
        // `translate` returns the key itself when the i18n harness
        // is uninitialized in the test runner, so substring-match
        // against the key still works.
        let hits = matching("this-key", CommandScope::Global);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "test.foo");
    }

    #[test]
    fn duplicate_id_replaces_in_place() {
        clear_registry();
        register(PaletteCommand {
            id: "test.dup",
            scope: CommandScope::Global,
            label_key: "first",
            shortcut: None,
            action: Box::new(|| {}),
        });
        register(PaletteCommand {
            id: "test.dup",
            scope: CommandScope::Global,
            label_key: "second",
            shortcut: None,
            action: Box::new(|| {}),
        });
        let hits = matching("", CommandScope::Global);
        let dups: Vec<_> = hits.iter().filter(|v| v.id == "test.dup").collect();
        assert_eq!(dups.len(), 1, "expected one entry after replace");
        assert_eq!(dups[0].label, "second");
    }

    #[test]
    fn scope_filter_hides_editor_command_in_home_scope() {
        clear_registry();
        register(PaletteCommand {
            id: "test.editor-only",
            scope: CommandScope::Editor,
            label_key: "editor-bold",
            shortcut: None,
            action: Box::new(|| {}),
        });
        let home_hits = matching("editor", CommandScope::Home);
        assert!(home_hits.is_empty());
        let editor_hits = matching("editor", CommandScope::Editor);
        assert_eq!(editor_hits.len(), 1);
    }

    #[test]
    fn global_command_visible_in_every_scope() {
        clear_registry();
        register(PaletteCommand {
            id: "test.global-thing",
            scope: CommandScope::Global,
            label_key: "global-thing",
            shortcut: None,
            action: Box::new(|| {}),
        });
        for scope in [
            CommandScope::Home,
            CommandScope::Editor,
            CommandScope::Spreadsheet,
        ] {
            assert_eq!(
                matching("global-thing", scope).len(),
                1,
                "scope {:?} should see Global command",
                scope,
            );
        }
    }

    #[test]
    fn run_dispatches_to_registered_action() {
        clear_registry();
        // Use a Cell to capture mutation from inside the action.
        // Rc<Cell<bool>> is fine on a single-threaded test runner.
        use std::cell::Cell;
        use std::rc::Rc;
        let flag = Rc::new(Cell::new(false));
        let flag_for_action = Rc::clone(&flag);
        register(PaletteCommand {
            id: "test.runme",
            scope: CommandScope::Global,
            label_key: "x",
            shortcut: None,
            action: Box::new(move || flag_for_action.set(true)),
        });
        run("test.runme");
        assert!(flag.get());
    }

    #[test]
    fn run_unknown_id_is_noop() {
        clear_registry();
        // Should not panic; no observable effect.
        run("never-registered");
    }

    #[test]
    fn mru_orders_empty_query_results() {
        clear_registry();
        // Clear MRU so other tests in this module can't poison this one.
        RECENT.with(|c| c.borrow_mut().clear());
        register(PaletteCommand {
            id: "test.alpha",
            scope: CommandScope::Global,
            label_key: "alpha",
            shortcut: None,
            action: Box::new(|| {}),
        });
        register(PaletteCommand {
            id: "test.beta",
            scope: CommandScope::Global,
            label_key: "beta",
            shortcut: None,
            action: Box::new(|| {}),
        });
        register(PaletteCommand {
            id: "test.gamma",
            scope: CommandScope::Global,
            label_key: "gamma",
            shortcut: None,
            action: Box::new(|| {}),
        });
        // Use beta then alpha — MRU = [alpha (newest), beta].
        run("test.beta");
        run("test.alpha");
        // Empty query: MRU-first, then unfamiliar in registration order.
        let hits: Vec<&str> = matching("", CommandScope::Global)
            .iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(hits, vec!["test.alpha", "test.beta", "test.gamma"]);
    }

    #[test]
    fn matching_with_shortcuts_filters_to_bound() {
        clear_registry();
        RECENT.with(|c| c.borrow_mut().clear());
        register(PaletteCommand {
            id: "test.has-shortcut",
            scope: CommandScope::Global,
            label_key: "one",
            shortcut: Some("Ctrl+1"),
            action: Box::new(|| {}),
        });
        register(PaletteCommand {
            id: "test.no-shortcut",
            scope: CommandScope::Global,
            label_key: "two",
            shortcut: None,
            action: Box::new(|| {}),
        });
        let hits = matching_with_shortcuts("", CommandScope::Global);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "test.has-shortcut");
    }
}
