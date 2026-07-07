// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

//! In-page mobile keyboard for spreadsheet cell editing.
//!
//! Mobile browsers don't let JS replace the soft keyboard, so when a
//! mode that suppresses the OS keyboard is active (Numeric or
//! Formula) we set `inputmode="none"` on the cell input — keeping the
//! input focused but hiding the OS keys — and surface this component
//! pinned to the bottom of the visible viewport.
//!
//! Three modes per `design/mobile.md:68-78` (Phase 5 M-P3 piece C):
//!
//!   • Standard — defers to the device's built-in keyboard for text
//!     entry. Our component renders only the mode-switcher strip
//!     plus commit / cancel; the OS keyboard handles characters.
//!   • Numeric — dedicated number pad (digits, decimal, sign,
//!     backspace) for plain-number cells.
//!   • Formula — operators, parens, function-name chips, function
//!     autocomplete. Forced when the cell value starts with `=`.
//!
//! The component is wired through a caller-owned `mode` signal so
//! `spreadsheet_view` can derive the cell input's `inputmode`
//! attribute from the same source of truth. Auto-Formula override:
//! whenever the value starts with `=`, [`KeyboardMode::auto_for`]
//! returns `Formula`, which the component honors regardless of the
//! user's stored pick. As soon as the user backspaces the `=`, the
//! pick takes over again.
//!
//! Every key dispatches through the same `set_edit_value` setter
//! that the cell input's `on:input` uses, so the yrs edit path is
//! identical to a desktop keystroke.

use leptos::prelude::*;

use crate::a11y;

/// Static list of supported function names. Pulled from
/// `spreadsheet_view::FUNCTION_LIST` to keep the keyboard self-contained;
/// the spreadsheet view's autocomplete consumes the same names from its
/// own copy. Both lists must stay in sync — see `inputmode_tests` for a
/// regression test.
const FUNCTION_NAMES: &[&str] = &[
    "ABS", "AND", "AVERAGE", "CEILING", "CHAR", "CHOOSE", "CODE", "COLUMN",
    "COLUMNS", "CONCAT", "CONCATENATE", "COUNT", "COUNTA", "COUNTBLANK",
    "COUNTIF", "DATE", "DAY", "EXP", "FALSE", "FIND", "FLOOR", "HLOOKUP",
    "IF", "IFERROR", "IFNA", "IFS", "INDEX", "INT", "ISBLANK", "ISERROR",
    "ISNA", "ISNUMBER", "ISTEXT", "LEFT", "LEN", "LN", "LOG", "LOG10",
    "LOWER", "MATCH", "MAX", "MID", "MIN", "MOD", "MONTH", "NOT", "NOW",
    "OR", "PI", "POWER", "PRODUCT", "RAND", "RANDBETWEEN", "REPT", "RIGHT",
    "ROUND", "ROUNDDOWN", "ROUNDUP", "ROW", "ROWS", "SEARCH", "SIGN",
    "SQRT", "SUBSTITUTE", "SUM", "SUMIF", "SWITCH", "TEXT", "TODAY",
    "TRIM", "TRUE", "TRUNC", "TYPE", "UPPER", "VALUE", "VLOOKUP", "XOR",
    "YEAR",
];

/// Functions that should rank above peers with the same prefix in
/// autocomplete. Pure alphabetical ordering on the full FUNCTION_NAMES
/// list ranks SUBSTITUTE and SUBTOTAL above SUM for partial `SU` —
/// surprising for the user since SUM is the canonical first choice.
/// Mirror this list in spreadsheet_view.rs's identical const. Doubles
/// as the empty-partial seed (the keyboard shows these chips before
/// the user types anything).
pub(crate) const COMMON_FUNCTIONS: &[&str] = &[
    "SUM", "AVERAGE", "IF", "COUNT", "MIN", "MAX", "VLOOKUP", "SUMIF",
];

/// Stable-sort key: COMMON_FUNCTIONS entries land first in priority
/// order, everything else preserves the FUNCTION_NAMES alphabetical
/// order (usize::MAX collapses to last; stable-sort keeps the rest).
fn common_priority(name: &str) -> usize {
    COMMON_FUNCTIONS
        .iter()
        .position(|&c| c == name)
        .unwrap_or(usize::MAX)
}

/// Rank FUNCTION_NAMES entries that start with `partial` (already
/// uppercase). Used by the on-screen formula keyboard's chip strip.
/// Exposed for the unit test that gates the SU → SUM ranking
/// regression.
fn rank_partial_matches(partial: &str) -> Vec<&'static str> {
    let mut matches: Vec<&'static str> = FUNCTION_NAMES
        .iter()
        .copied()
        .filter(|name| name.starts_with(partial))
        .collect();
    matches.sort_by_key(|name| common_priority(name));
    matches
}

/// User-selectable keyboard layout. `spreadsheet_view` owns a signal
/// of this type and passes it to [`FormulaKeyboard`] so the cell
/// input's `inputmode` and the keyboard's rendered layout stay in
/// lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardMode {
    /// Defer to the OS keyboard. Component renders the mode-switcher
    /// strip + commit/cancel only.
    Standard,
    /// Number pad — digits, decimal, sign, backspace.
    Numeric,
    /// Formula keys — operators, parens, function chips.
    Formula,
}

impl KeyboardMode {
    /// Forced mode for a given cell value, if any. Returns
    /// `Some(Formula)` when the value starts with `=`; the user's
    /// stored pick is overridden in that case. Returns `None`
    /// otherwise — caller's preference applies.
    pub fn auto_for(value: &str) -> Option<Self> {
        if value.starts_with('=') {
            Some(Self::Formula)
        } else {
            None
        }
    }

    /// Whether this mode requires the OS soft keyboard to be hidden.
    /// `spreadsheet_view` consults this to set `inputmode="none"` on
    /// the cell input when our component owns the entry surface, and
    /// to defer to the per-column heuristic otherwise.
    pub fn suppresses_os_keyboard(self) -> bool {
        matches!(self, Self::Numeric | Self::Formula)
    }

    fn css_class(self) -> &'static str {
        match self {
            Self::Standard => "is-standard",
            Self::Numeric => "is-numeric",
            Self::Formula => "is-formula",
        }
    }

    /// Localized label string for the mode-switcher tab. Translated
    /// at call time; `crate::t!` requires a literal key, so the
    /// match-on-self lives here rather than at the call site.
    fn tab_label(self) -> String {
        match self {
            Self::Standard => crate::t!("kbd-mode-standard"),
            Self::Numeric => crate::t!("kbd-mode-numeric"),
            Self::Formula => crate::t!("kbd-mode-formula"),
        }
    }
}

/// Read the input's selection range, falling back to (len, len).
fn read_caret(input: &web_sys::HtmlInputElement) -> (usize, usize) {
    let len = input.value().len();
    let start = input
        .selection_start()
        .ok()
        .flatten()
        .map(|n| n as usize)
        .unwrap_or(len);
    let end = input
        .selection_end()
        .ok()
        .flatten()
        .map(|n| n as usize)
        .unwrap_or(len);
    (start.min(len), end.min(len))
}

/// Walk back from `caret` to find the start of the current "token" — the
/// run of letters/digits/`.` since the last operator, paren, or comma.
/// Used to replace partial function names when the user taps an
/// autocomplete chip.
fn token_start(value: &str, caret: usize) -> usize {
    let bytes = value.as_bytes();
    let mut i = caret.min(bytes.len());
    while i > 0 {
        let b = bytes[i - 1];
        let stop = matches!(
            b,
            b'(' | b')' | b',' | b'+' | b'-' | b'*' | b'/' | b' '
                | b'^' | b'&' | b'<' | b'>' | b'=' | b':'
        );
        if stop {
            break;
        }
        i -= 1;
    }
    i
}

/// Extract the partial function-name being typed (after `=`/`(`/`,`/operator),
/// uppercased. Returns "" if the token contains anything that isn't a
/// letter (so cell refs like `A1` don't trigger function autocomplete).
fn current_partial(value: &str, caret: usize) -> String {
    let start = token_start(value, caret);
    let token = &value[start..caret.min(value.len())];
    if token.is_empty() || !token.chars().all(|c| c.is_ascii_alphabetic()) {
        return String::new();
    }
    token.to_ascii_uppercase()
}

/// In-page mobile cell keyboard. Renders only when `visible` is
/// true; the rendered layout flips based on the effective mode
/// (caller's pick overridden to `Formula` when the value starts
/// with `=`).
#[component]
pub fn FormulaKeyboard(
    /// Current edit-mode value (read).
    edit_value: ReadSignal<String>,
    /// Setter — every key dispatches through this so the yrs path matches
    /// the desktop keystroke flow.
    set_edit_value: WriteSignal<String>,
    /// Live handle to the cell `<input>`; we read `selectionStart` to
    /// splice at the caret and restore the selection after each write.
    cell_input_ref: NodeRef<leptos::html::Input>,
    /// Whether the keyboard is mounted/visible. Caller derives this
    /// — typically `touch_primary && editing`.
    visible: Signal<bool>,
    /// User-selected mode. Spreadsheet_view owns this signal so it
    /// can derive `inputmode` on the cell input from the same source
    /// of truth.
    mode: ReadSignal<KeyboardMode>,
    /// Setter for the mode signal — fires on tab click.
    set_mode: WriteSignal<KeyboardMode>,
    /// Commit (Enter) — defers to the cell input's commit path.
    on_commit: Callback<()>,
    /// Cancel (Escape) — defers to the cell input's cancel path.
    on_cancel: Callback<()>,
) -> impl IntoView {
    // Effective mode: caller's pick, overridden to Formula when the
    // value starts with `=`. The override is reactive — flips back
    // to the user's pick the moment they backspace the `=`.
    let effective_mode = Signal::derive(move || {
        KeyboardMode::auto_for(&edit_value.get()).unwrap_or_else(|| mode.get())
    });

    // Splice `text` into the current `edit_value` at the input's caret;
    // restore the caret to the position after the inserted text on the
    // next animation frame (after Leptos updates the DOM).
    let insert_at_caret = move |text: String| {
        let Some(input_handle) = cell_input_ref.get() else { return };
        let input: web_sys::HtmlInputElement = input_handle.into();
        let (start, end) = read_caret(&input);
        let val = edit_value.get_untracked();
        let head = val.get(..start).unwrap_or(&val);
        let tail = val.get(end..).unwrap_or("");
        let new_val = format!("{head}{text}{tail}");
        let new_caret = (head.len() + text.len()) as u32;
        set_edit_value.set(new_val);
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            if let Some(handle) = cell_input_ref.get() {
                let el: web_sys::HtmlInputElement = handle.into();
                let _ = el.set_selection_range(new_caret, new_caret);
                let _ = el.focus();
            }
        });
    };

    // Replace the current partial token (if any) with `name(`. If no
    // partial is active, just insert `name(` at the caret.
    let insert_function = move |name: &'static str| {
        let Some(input_handle) = cell_input_ref.get() else { return };
        let input: web_sys::HtmlInputElement = input_handle.into();
        let (caret_start, caret_end) = read_caret(&input);
        let val = edit_value.get_untracked();
        let token_begin = token_start(&val, caret_start);
        let head = val.get(..token_begin).unwrap_or(&val);
        let tail = val.get(caret_end..).unwrap_or("");
        let inserted = format!("{name}(");
        let new_val = format!("{head}{inserted}{tail}");
        let new_caret = (head.len() + inserted.len()) as u32;
        set_edit_value.set(new_val);
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            if let Some(handle) = cell_input_ref.get() {
                let el: web_sys::HtmlInputElement = handle.into();
                let _ = el.set_selection_range(new_caret, new_caret);
                let _ = el.focus();
            }
        });
    };

    // Backspace: delete the character before the caret (or the current
    // selection if non-empty).
    let backspace = move || {
        let Some(input_handle) = cell_input_ref.get() else { return };
        let input: web_sys::HtmlInputElement = input_handle.into();
        let (start, end) = read_caret(&input);
        let val = edit_value.get_untracked();
        let (delete_from, delete_to) = if start == end && start > 0 {
            // Step back one char boundary (UTF-8 safe).
            let mut idx = start - 1;
            while idx > 0 && !val.is_char_boundary(idx) {
                idx -= 1;
            }
            (idx, start)
        } else if start != end {
            (start, end)
        } else {
            return;
        };
        let head = val.get(..delete_from).unwrap_or(&val);
        let tail = val.get(delete_to..).unwrap_or("");
        let new_val = format!("{head}{tail}");
        let new_caret = head.len() as u32;
        set_edit_value.set(new_val);
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            if let Some(handle) = cell_input_ref.get() {
                let el: web_sys::HtmlInputElement = handle.into();
                let _ = el.set_selection_range(new_caret, new_caret);
                let _ = el.focus();
            }
        });
    };

    // Function chips: filter list to the current partial token.
    let chips = move || -> Vec<&'static str> {
        let Some(input_handle) = cell_input_ref.get() else {
            return Vec::new();
        };
        let input: web_sys::HtmlInputElement = input_handle.into();
        let (caret, _) = read_caret(&input);
        let partial = current_partial(&edit_value.get(), caret);
        if partial.is_empty() {
            // No partial active — show common starters.
            return COMMON_FUNCTIONS[..6].to_vec();
        }
        let mut matches = rank_partial_matches(&partial);
        matches.truncate(12);
        matches
    };

    // mousedown on every key prevents the cell input from losing focus
    // (which would commit the edit and tear down the keyboard).
    let prevent_blur = |ev: web_sys::MouseEvent| {
        ev.prevent_default();
    };

    // Build a key button. `label` shows in the UI; `text` is what gets
    // inserted. Most keys label == text, but some (e.g. backspace) differ.
    let key = move |label: &'static str, insert: &'static str, extra_class: &'static str| {
        let class = format!("formula-key {extra_class}");
        let mut insert_at_caret = insert_at_caret;
        view! {
            <button
                class=class
                on:mousedown=prevent_blur
                on:click=move |_| insert_at_caret(insert.to_string())
            >{label}</button>
        }
    };

    // Mode-switcher tab strip. While `auto_for(value)` returns Some,
    // the override is in effect and the other tabs are disabled —
    // the user must backspace the `=` to unlock text/number modes.
    let render_tab = move |target: KeyboardMode| {
        let auto_locked = move || KeyboardMode::auto_for(&edit_value.get()).is_some();
        let is_active = move || effective_mode.get() == target;
        view! {
            <button
                class=move || {
                    let mut classes = String::from("formula-keyboard-tab ");
                    classes.push_str(target.css_class());
                    if is_active() {
                        classes.push_str(" is-active");
                    }
                    if auto_locked() && target != KeyboardMode::Formula {
                        classes.push_str(" is-locked");
                    }
                    classes
                }
                disabled=move || auto_locked() && target != KeyboardMode::Formula
                on:mousedown=prevent_blur
                on:click=move |_| set_mode.set(target)
                aria-pressed=move || if is_active() { "true" } else { "false" }
            >{move || target.tab_label()}</button>
        }
    };

    let commit_button = move || {
        view! {
            <button
                class="formula-key formula-key-commit"
                title=crate::t!("formula-key-commit")
                on:mousedown=prevent_blur
                on:click=move |_| a11y::defer_close(on_commit)
            >"\u{21B5}"</button>
        }
    };

    let cancel_button = move || {
        view! {
            <button
                class="formula-key formula-key-cancel"
                title=crate::t!("formula-key-cancel")
                on:mousedown=prevent_blur
                on:click=move |_| a11y::defer_close(on_cancel)
            >"\u{2715}"</button>
        }
    };

    let backspace_button = move || {
        view! {
            <button
                class="formula-key formula-key-back"
                title=crate::t!("formula-key-backspace")
                on:mousedown=prevent_blur
                on:click=move |_| backspace()
            >"\u{232B}"</button>
        }
    };

    view! {
        <Show when=move || visible.get()>
            <div
                class=move || format!("formula-keyboard {}", effective_mode.get().css_class())
                on:mousedown=prevent_blur
            >
                <div class="formula-keyboard-tabs" role="tablist">
                    {render_tab(KeyboardMode::Standard)}
                    {render_tab(KeyboardMode::Numeric)}
                    {render_tab(KeyboardMode::Formula)}
                </div>

                {move || match effective_mode.get() {
                    KeyboardMode::Standard => view! {
                        <div class="formula-keyboard-row formula-keyboard-actions">
                            {cancel_button()}
                            <span class="formula-keyboard-hint">
                                {crate::t!("kbd-standard-hint")}
                            </span>
                            {commit_button()}
                        </div>
                    }.into_any(),

                    KeyboardMode::Numeric => view! {
                        <div>
                            <div class="formula-keyboard-row">
                                {key("7", "7", "")}
                                {key("8", "8", "")}
                                {key("9", "9", "")}
                                {backspace_button()}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("4", "4", "")}
                                {key("5", "5", "")}
                                {key("6", "6", "")}
                                {cancel_button()}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("1", "1", "")}
                                {key("2", "2", "")}
                                {key("3", "3", "")}
                                {commit_button()}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("0", "0", "")}
                                {key(".", ".", "")}
                                {key("-", "-", "formula-key-op")}
                                {key("%", "%", "formula-key-op")}
                            </div>
                        </div>
                    }.into_any(),

                    KeyboardMode::Formula => view! {
                        <div>
                            <div class="formula-keyboard-chips">
                                {move || chips().into_iter().map(|name| {
                                    let mut insert_function = insert_function;
                                    view! {
                                        <button
                                            class="formula-chip"
                                            on:mousedown=prevent_blur
                                            on:click=move |_| insert_function(name)
                                        >{name}</button>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("(", "(", "")}
                                {key(")", ")", "")}
                                {key(",", ",", "")}
                                {key(":", ":", "")}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("7", "7", "")}
                                {key("8", "8", "")}
                                {key("9", "9", "")}
                                {key("+", "+", "formula-key-op")}
                                {key("-", "-", "formula-key-op")}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("4", "4", "")}
                                {key("5", "5", "")}
                                {key("6", "6", "")}
                                {key("*", "*", "formula-key-op")}
                                {key("/", "/", "formula-key-op")}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("1", "1", "")}
                                {key("2", "2", "")}
                                {key("3", "3", "")}
                                {key("^", "^", "formula-key-op")}
                                {key("&", "&", "formula-key-op")}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("0", "0", "")}
                                {key(".", ".", "")}
                                {key("=", "=", "formula-key-op")}
                                {backspace_button()}
                                {cancel_button()}
                            </div>
                            <div class="formula-keyboard-row">
                                {key("<", "<", "formula-key-op")}
                                {key(">", ">", "formula-key-op")}
                                {key("<=", "<=", "formula-key-op")}
                                {key(">=", ">=", "formula-key-op")}
                                {key("<>", "<>", "formula-key-op")}
                                {commit_button()}
                            </div>
                        </div>
                    }.into_any(),
                }}
            </div>
        </Show>
    }
}

// ─── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_start_at_string_start() {
        assert_eq!(token_start("SUM", 3), 0);
    }

    #[test]
    fn token_start_after_equals() {
        assert_eq!(token_start("=SUM", 4), 1);
    }

    #[test]
    fn token_start_after_paren() {
        assert_eq!(token_start("=SUM(AVE", 8), 5);
    }

    #[test]
    fn token_start_after_comma() {
        assert_eq!(token_start("=SUM(A1,COU", 11), 8);
    }

    #[test]
    fn token_start_caret_zero() {
        assert_eq!(token_start("anything", 0), 0);
    }

    #[test]
    fn current_partial_extracts_letters_only() {
        assert_eq!(current_partial("=SUM(AV", 7), "AV");
    }

    #[test]
    fn current_partial_rejects_with_digits() {
        // Cell refs like A1 should NOT match function-name autocomplete.
        assert_eq!(current_partial("=A1", 3), "");
    }

    #[test]
    fn current_partial_empty_token() {
        // Caret immediately after `(` — nothing to autocomplete yet.
        assert_eq!(current_partial("=SUM(", 5), "");
    }

    #[test]
    fn current_partial_uppercases() {
        assert_eq!(current_partial("=su", 3), "SU");
    }

    #[test]
    fn function_names_match_spreadsheet_list_size() {
        // Sanity: keep FUNCTION_NAMES roughly in step with the
        // spreadsheet_view FUNCTION_LIST. They must both contain SUM.
        assert!(FUNCTION_NAMES.contains(&"SUM"));
        assert!(FUNCTION_NAMES.contains(&"AVERAGE"));
        assert!(FUNCTION_NAMES.contains(&"VLOOKUP"));
    }

    #[test]
    fn rank_partial_su_ranks_sum_first() {
        // Regression for the mobile-spreadsheet-keyboards doctor
        // scenario. Pure alphabetical ordering returned SUBSTITUTE
        // before SUM (then SUBTOTAL in the spreadsheet_view list);
        // the common-function priority list lifts SUM to the top.
        let ranked = rank_partial_matches("SU");
        assert_eq!(ranked.first().copied(), Some("SUM"));
        // SUMIF is the next COMMON_FUNCTIONS entry that matches SU;
        // it should land before SUBSTITUTE.
        let sumif_idx = ranked.iter().position(|&n| n == "SUMIF").unwrap();
        let substitute_idx = ranked.iter().position(|&n| n == "SUBSTITUTE").unwrap();
        assert!(sumif_idx < substitute_idx,
            "expected SUMIF before SUBSTITUTE, got {ranked:?}");
    }

    #[test]
    fn rank_partial_preserves_alpha_for_non_common() {
        // Non-COMMON matches keep their FUNCTION_NAMES alphabetical
        // order. With partial COU, COUNT (COMMON) lifts to the top
        // then COUNTA, COUNTBLANK, COUNTIF follow in alpha order.
        let ranked = rank_partial_matches("COU");
        assert_eq!(ranked.first().copied(), Some("COUNT"));
        // The tail should be alphabetical: COUNTA < COUNTBLANK < COUNTIF.
        let positions: Vec<usize> = ["COUNTA", "COUNTBLANK", "COUNTIF"]
            .iter()
            .map(|name| ranked.iter().position(|&n| n == *name).unwrap())
            .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]),
            "expected COUNTA < COUNTBLANK < COUNTIF, got {ranked:?}");
    }

    #[test]
    fn auto_mode_forces_formula_on_equals_prefix() {
        assert_eq!(KeyboardMode::auto_for("=SUM("), Some(KeyboardMode::Formula));
        assert_eq!(KeyboardMode::auto_for("="), Some(KeyboardMode::Formula));
    }

    #[test]
    fn auto_mode_returns_none_for_text() {
        assert_eq!(KeyboardMode::auto_for("hello"), None);
        assert_eq!(KeyboardMode::auto_for("123"), None);
        assert_eq!(KeyboardMode::auto_for(""), None);
    }

    #[test]
    fn standard_mode_lets_os_keyboard_through() {
        assert!(!KeyboardMode::Standard.suppresses_os_keyboard());
        assert!(KeyboardMode::Numeric.suppresses_os_keyboard());
        assert!(KeyboardMode::Formula.suppresses_os_keyboard());
    }
}
