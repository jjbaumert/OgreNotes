// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::api::{search, users};
use crate::inserts::{full_catalog, CatalogItem, InsertSection};

/// Trigger mode — determines which producers fire and which
/// prefix character shows in the menu header.
///
/// - `At` (v1): full fanout (people + documents + inserts + AI).
///   The user gets everything the `@` prefix has always meant
///   in this codebase.
/// - `Slash` (v2 slice 2): commands only — the inserts + AI
///   catalog with no server-side people/document lookup. Matches
///   the Notion/Craft convention where `/` opens a command
///   menu, distinct from `@` (mention-shaped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtMenuMode {
    At,
    Slash,
}

impl AtMenuMode {
    fn prefix(&self) -> char {
        match self {
            AtMenuMode::At => '@',
            AtMenuMode::Slash => '/',
        }
    }
}

/// @ / slash menu (#148): typeahead-searches users AND documents,
/// plus a static Insert / AI catalog (via
/// `crate::inserts::INSERT_CATALOG` and
/// `crate::editor::blocks::BLOCK_INSERTS`). Selection is a
/// per-item enum so each match arm in the parent can dispatch
/// the right `ToolbarCommand`.
///
/// The menu is anchored at the caret via viewport coords passed
/// in (`left` / `top`); trigger detection and dismissal live in
/// `pages/document.rs`.
///
/// `mode` decides which prefix character is shown and whether
/// the server-side people/document fetches fire. It's a plain
/// enum (not a signal) because each mount is dedicated to one
/// trigger — the `@` and `/` menus are separate `AtMenu` mounts
/// with their own signals.
#[component]
pub fn AtMenu(
    /// Whether the menu is visible.
    visible: ReadSignal<bool>,
    /// The current search query (text after the prefix).
    query: ReadSignal<String>,
    /// Position: left pixels from viewport edge.
    left: ReadSignal<f64>,
    /// Position: top pixels from viewport edge.
    top: ReadSignal<f64>,
    /// Fires when the user activates an item (click or Enter).
    on_select: Callback<AtMenuItem>,
    /// Fires when the menu should close without a selection
    /// (Escape, click outside — the trigger-detection Effect in
    /// document.rs also closes on caret movement).
    on_close: Callback<()>,
    /// Highlighted flat index — parent tracks this so it can
    /// route Up/Down/Enter on the doc-level keydown handler
    /// while the menu is visible.
    highlighted: ReadSignal<usize>,
    /// Setter so the menu can bound the highlighted index when
    /// the result list length changes.
    set_highlighted: WriteSignal<usize>,
    /// Setter for the results signal so the parent's keydown
    /// handler can read the current list to route Enter/Tab.
    set_results: WriteSignal<Vec<AtMenuItem>>,
    /// Read-side of the results signal — same list the parent
    /// reads for Enter/Tab dispatch.
    results: ReadSignal<Vec<AtMenuItem>>,
    /// Trigger mode — `At` or `Slash`. See `AtMenuMode`.
    #[prop(default = AtMenuMode::At)]
    mode: AtMenuMode,
) -> impl IntoView {
    // Producer: refetch on visibility flip or query change.
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        let q = query.get();
        let set_results_local = set_results;
        let set_highlighted_local = set_highlighted;
        // #148 review finding #4: preserve the highlighted item
        // across refetches when it's still in the new list, so
        // arrow-nav intent survives keystroke-driven refetches
        // (typing "@a" → ↓↓↓ Andrea → "n" → Andrea keeps its
        // highlight even though the list narrowed). Capture the
        // current item's identity BEFORE the async resolve; look
        // it up in the new results afterwards.
        let prior_identity: Option<String> = {
            let idx = highlighted.get_untracked();
            let cur = results.get_untracked();
            cur.get(idx).map(item_identity)
        };
        leptos::task::spawn_local(async move {
            let items = resolve_at_items(&q, mode).await;
            let next_idx = match prior_identity.as_deref() {
                Some(key) => items
                    .iter()
                    .position(|it| item_identity(it) == key)
                    .unwrap_or(0),
                None => 0,
            };
            set_results_local.set(items);
            set_highlighted_local.set(next_idx);
        });
    });

    // Keep the highlighted item in view as the user arrow-keys
    // through results. The previous attempt (47f10a7) relied on
    // `scrollIntoView`, which:
    //   1. Requires a specific selector — the `data-at-menu-idx`
    //      hyphenated dynamic attribute wasn't reliably placed in
    //      the DOM by the Leptos macro, and
    //   2. The web-sys `_with_bool` overload aligns to
    //      top-or-bottom, not "nearest" — a jarring jump every
    //      keystroke even when the item was already visible.
    //
    // This version:
    //   - Anchors on the reliably-applied `.at-menu-item--
    //     highlighted` class, and
    //   - Computes the scroll adjustment manually against the
    //     scroll container's own scrollTop, so we only scroll
    //     when the item is genuinely outside the viewport.
    //
    // Effect defers to a microtask so the new class has landed
    // in the DOM before we query for it.
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        // Track the highlighted signal so the Effect re-fires
        // when it changes.
        let _idx = highlighted.get();
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(0).await;
            let Some(document) = web_sys::window().and_then(|w| w.document())
            else {
                return;
            };
            let Ok(Some(item)) =
                document.query_selector(".at-menu .at-menu-item--highlighted")
            else {
                return;
            };
            let Ok(Some(body)) = document.query_selector(".at-menu .at-menu-body")
            else {
                return;
            };
            let (Some(item_el), Some(body_el)) = (
                item.dyn_ref::<web_sys::HtmlElement>(),
                body.dyn_ref::<web_sys::HtmlElement>(),
            ) else {
                return;
            };
            // offset_top is measured against the nearest
            // positioned ancestor. In our DOM, .at-menu-body has
            // no explicit `position` — the item's offset_top
            // ends up relative to the .at-menu container (which
            // has `position: fixed`). Use the rects instead:
            // difference between item.top and body.top gives the
            // item's position within the body's viewport,
            // independent of what's `position: relative`.
            let body_rect = body_el.get_bounding_client_rect();
            let item_rect = item_el.get_bounding_client_rect();
            let item_top_in_body = item_rect.top() - body_rect.top();
            let item_bottom_in_body = item_top_in_body + item_rect.height();
            let scroll_top = body_el.scroll_top() as f64;
            let client_height = body_el.client_height() as f64;
            if item_top_in_body < 0.0 {
                // Item's above the visible slice — scroll up.
                let new_top = scroll_top + item_top_in_body;
                body_el.set_scroll_top(new_top as i32);
            } else if item_bottom_in_body > client_height {
                // Item's below the visible slice — scroll down.
                let new_top = scroll_top + (item_bottom_in_body - client_height);
                body_el.set_scroll_top(new_top as i32);
            }
        });
    });

    view! {
        <Show when=move || visible.get()>
            <div
                class="at-menu"
                style:left=move || format!("{}px", left.get())
                style:top=move || format!("{}px", top.get())
            >
                <div class="at-menu-header">
                    <span class="at-menu-query">
                        {mode.prefix().to_string()}
                        {move || query.get()}
                    </span>
                </div>
                <div class="at-menu-body">
                    {move || {
                        let items = results.get();
                        if items.is_empty() {
                            let _ = on_close; // suppress unused-if-empty warn.
                            view! {
                                <div class="at-menu-empty">
                                    {crate::t!("at-menu-empty")}
                                </div>
                            }.into_any()
                        } else {
                            render_sections(items, highlighted, on_select).into_any()
                        }
                    }}
                </div>
            </div>
        </Show>
    }
}

/// Section groupings shown in the menu. Order = display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtMenuSection {
    People,
    Documents,
    Insert,
    Ai,
}

impl AtMenuSection {
    fn label_key(&self) -> &'static str {
        match self {
            AtMenuSection::People => "at-menu-section-people",
            AtMenuSection::Documents => "at-menu-section-documents",
            AtMenuSection::Insert => "at-menu-section-insert",
            AtMenuSection::Ai => "at-menu-section-ai",
        }
    }
}

/// One item shown in the @-menu.
#[derive(Debug, Clone)]
pub struct AtMenuItem {
    pub label: String,
    pub icon: String,
    pub section: AtMenuSection,
    pub kind: AtMenuItemKind,
}

/// Discriminated payload — every `on_at_select` match arm has the
/// data it needs to dispatch without a second lookup.
#[derive(Debug, Clone)]
pub enum AtMenuItemKind {
    /// Insert a user @-mention (text + `MarkType::Mention`).
    UserMention { user_id: String, display: String },
    /// Insert an inline link to another document (text +
    /// `MarkType::Link` with `href = /d/<id>`).
    DocumentLink { doc_id: String, title: String },
    /// Insert a live-app block from `BLOCK_INSERTS`
    /// (`Kanban`, `Calendar`, ...).
    InsertLiveApp { id: &'static str },
    /// Insert a plain table (defaults to 3x3).
    InsertTable,
    /// Open the image upload picker.
    InsertImage,
    /// Insert a horizontal rule / divider.
    InsertHorizontalRule,
    /// Turn the current block into a code block.
    SetCodeBlock,
    /// Open the AskDialog pre-filled with the query for the
    /// @-ask flow. Selection captures the trigger range from
    /// `pages/document.rs` (the caller knows the `@ask <query>`
    /// span) and pipes text back via `ToolbarCommand::InsertAiText`.
    AskAi { prompt: String },
    /// #148 v2 — AI directive wrapper. Composes a fixed system-prompt
    /// prefix with the user's input and (optionally) the current
    /// selection or doc content, then opens the AskDialog. The
    /// composition happens at `on_at_select` time so it has access to
    /// editor state; the menu just carries the directive + query.
    AskWithDirective {
        directive: AiDirective,
        user_input: String,
    },
    /// #148 v2 slice 3 — insert today's date as plain text at the
    /// trigger range. `style` picks the format (short / medium /
    /// long / iso); the actual formatting happens at select-time
    /// against `js_sys::Date::now()` so the inserted text reflects
    /// the user's clock, not the menu-render time.
    InsertDate { style: DateInsertStyle },
    /// #148 v2 slice 5 — insert a single emoji character at the
    /// trigger range. The producer surfaces entries from a small
    /// curated list matched on name (`@joy`, `@fire`, etc.).
    InsertEmoji { emoji: &'static str },
}

/// Format for the `@date` / `/date` insertable. Names are chosen
/// to match the entries the user types after the trigger
/// (`@date short` / `@date iso` / `@date long`) — the query
/// producer matches them keyword-fuzzy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateInsertStyle {
    /// e.g. "5/19/26" — matches `i18n::DateStyle::Short`.
    Short,
    /// e.g. "May 19, 2026" — matches `i18n::DateStyle::Medium`.
    /// This is the default when the user types just `@date`.
    Medium,
    /// e.g. "May 19, 2026, 2:35 PM" — includes clock. Matches
    /// `i18n::DateStyle::Long`.
    Long,
    /// ISO 8601 date only, always en-US-agnostic — `2026-05-19`.
    /// Doesn't go through the Intl formatter (locale-independent
    /// by design).
    Iso,
}

/// #148 v2 — the four AI wrappers over the base @-ask flow. Each
/// composes a specific system prompt around the caller's input +
/// selection/doc content. See `compose_directive_prompt` for the
/// exact wording.
#[derive(Debug, Clone)]
pub enum AiDirective {
    /// Summarize the current doc (or selection). No target
    /// parameter; the directive is self-contained.
    Summarize,
    /// Translate the current selection (or doc) to the target
    /// language. `target` = "Spanish" / "French" / etc.
    Translate { target: String },
    /// Rewrite the current selection (or doc) to a target tone.
    /// `tone` = "concise" / "formal" / "casual" / etc.
    Rewrite { tone: String },
    /// Brainstorm five bullet points on the user's topic.
    Brainstorm,
}

/// #148 v2 — compose the final prompt for an `AskWithDirective`
/// item. Called from `on_at_select` in `pages/document.rs` with the
/// current editor state's selection/doc text extracted via
/// `commands::plain_text_from_state`.
///
/// `user_input` is whatever the user typed after the @-directive
/// (e.g. `@translate spanish` → `user_input = "spanish"`; `@summarize`
/// → `user_input = ""`). Each directive interprets it differently.
///
/// `source_text` and `scope` come from `plain_text_from_state`.
pub fn compose_directive_prompt(
    directive: &AiDirective,
    user_input: &str,
    source_text: &str,
    scope: crate::editor::commands::TextScope,
) -> String {
    // Retained for tests that pin the full-wire behavior;
    // production callers use `compose_directive_parts` so the
    // AskDialog's input shows the short instruction and the
    // (scope-guard + source text) rides invisibly.
    let (instruction, suffix) =
        compose_directive_parts(directive, user_input, source_text, scope);
    match suffix {
        Some(s) => format!("{instruction}\n\n{s}"),
        None => instruction,
    }
}

/// #148 v2 — split the AI directive into (visible instruction,
/// hidden suffix). The AskDialog shows `instruction` in its
/// input for the user to review and edit; on submit, the
/// dialog concatenates the (possibly-edited) instruction with
/// the hidden suffix to produce the final wire prompt.
///
/// This keeps the input readable ("Summarize this document
/// concisely.") instead of dumping the whole document into the
/// user's face, while still delivering the source text +
/// scope guard to the assistant.
///
/// `None` for the suffix means "no hidden context" — the
/// Brainstorm directive uses this: its prompt is self-contained
/// (the topic goes into the visible instruction) and there is
/// no source text to append.
pub fn compose_directive_parts(
    directive: &AiDirective,
    user_input: &str,
    source_text: &str,
    scope: crate::editor::commands::TextScope,
) -> (String, Option<String>) {
    let scope_label = match scope {
        crate::editor::commands::TextScope::Selection => "the following selected text",
        crate::editor::commands::TextScope::WholeDoc => "the following document",
    };
    // Every directive runs under `AskMode::Direct` — no RAG,
    // no cross-doc pull-ins. But without an explicit scope
    // restriction Claude can still lean on training-data
    // context to pad its answer. This preamble locks the
    // assistant to the source text delivered inline. Lives in
    // the hidden half so it doesn't pollute the input the user
    // reads.
    let scope_guard = "\
Use ONLY the source text provided below. Do NOT use information from any \
other document, from the web, or from your training data. If the source \
text does not contain enough information to answer, say so plainly rather \
than filling gaps from outside knowledge.";
    match directive {
        AiDirective::Summarize => {
            let topic_hint = if user_input.trim().is_empty() {
                String::new()
            } else {
                format!(" Focus on: {}.", user_input.trim())
            };
            let instruction = format!("Summarize {scope_label} concisely.{topic_hint}");
            let suffix = format!("{scope_guard}\n\n---\n\n{source_text}");
            (instruction, Some(suffix))
        }
        AiDirective::Translate { target } => {
            let target = if target.trim().is_empty() {
                // Default to Spanish if the user didn't specify —
                // matches the most-common OgreNotes translation
                // request from telemetry.
                "Spanish".to_string()
            } else {
                target.trim().to_string()
            };
            let instruction = format!(
                "Translate {scope_label} to {target}. Preserve formatting where possible."
            );
            let suffix = format!("{scope_guard}\n\n---\n\n{source_text}");
            (instruction, Some(suffix))
        }
        AiDirective::Rewrite { tone } => {
            let tone = if tone.trim().is_empty() {
                "concise and clear".to_string()
            } else {
                tone.trim().to_string()
            };
            let instruction = format!(
                "Rewrite {scope_label} to be {tone}. Return only the rewritten text."
            );
            let suffix = format!("{scope_guard}\n\n---\n\n{source_text}");
            (instruction, Some(suffix))
        }
        AiDirective::Brainstorm => {
            // Brainstorm is different: it's meant to GENERATE new
            // ideas, not stay within a source. The scope guard
            // would suppress that behavior, so it's omitted here
            // by design. `user_input` narrows the topic;
            // `source_text` acts as the seed when no input.
            let topic = if user_input.trim().is_empty() {
                source_text.trim()
            } else {
                user_input.trim()
            };
            let instruction = format!(
                "Brainstorm 5 concise, distinct bullet points about: {topic}\n\nReturn only the bullet list."
            );
            (instruction, None)
        }
    }
}

/// #148 v2 slice 3 — format the current wall-clock time as text
/// for the `@date` / `/date` insertable. Reads
/// `js_sys::Date::now()` at call time so the inserted string
/// reflects the user's clock; `Iso` bypasses the locale
/// formatter (always `YYYY-MM-DD`), the other styles route
/// through `i18n::format_date` so they match the rest of the
/// app's date rendering.
pub fn format_date_now(style: DateInsertStyle) -> String {
    let now_ms = js_sys::Date::now();
    match style {
        DateInsertStyle::Iso => {
            // YYYY-MM-DD from the JS Date object. Uses UTC to
            // stay locale-independent (the whole point of
            // choosing Iso).
            let date =
                js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(now_ms));
            let y = date.get_utc_full_year();
            let m = date.get_utc_month() + 1;
            let d = date.get_utc_date();
            format!("{y:04}-{m:02}-{d:02}")
        }
        DateInsertStyle::Short => crate::i18n::format_date(
            (now_ms * 1000.0) as i64,
            crate::i18n::DateStyle::Short,
        ),
        DateInsertStyle::Medium => crate::i18n::format_date(
            (now_ms * 1000.0) as i64,
            crate::i18n::DateStyle::Medium,
        ),
        DateInsertStyle::Long => crate::i18n::format_date(
            (now_ms * 1000.0) as i64,
            crate::i18n::DateStyle::Long,
        ),
    }
}

/// Stable identity key for a single @-menu item — used by the
/// producer effect to preserve the highlighted position across
/// keystroke-driven refetches (finding #4 of the #148 review).
///
/// The key composes the section, a kind discriminant tag, and
/// the most stable per-item field: `user_id` for people, `doc_id`
/// for documents, the entry `id` for live-app blocks and static
/// inserts. `AskAi` uses its section tag alone since there's
/// only one such entry at a time.
///
/// Two rows in different sections never collide because the key
/// leads with the section tag; two rows in the same section with
/// the same underlying id would also share a highlight, which
/// is fine (they'd be duplicates anyway).
fn item_identity(item: &AtMenuItem) -> String {
    let section_tag = match item.section {
        AtMenuSection::People => "p",
        AtMenuSection::Documents => "d",
        AtMenuSection::Insert => "i",
        AtMenuSection::Ai => "a",
    };
    let (kind_tag, extra) = match &item.kind {
        AtMenuItemKind::UserMention { user_id, .. } => ("user", user_id.as_str()),
        AtMenuItemKind::DocumentLink { doc_id, .. } => ("doc", doc_id.as_str()),
        AtMenuItemKind::InsertLiveApp { id } => ("live", *id),
        AtMenuItemKind::InsertTable => ("table", ""),
        AtMenuItemKind::InsertImage => ("image", ""),
        AtMenuItemKind::InsertHorizontalRule => ("hr", ""),
        AtMenuItemKind::SetCodeBlock => ("code", ""),
        AtMenuItemKind::AskAi { .. } => ("ask", ""),
        AtMenuItemKind::AskWithDirective { directive, .. } => match directive {
            AiDirective::Summarize => ("summarize", ""),
            AiDirective::Translate { .. } => ("translate", ""),
            AiDirective::Rewrite { .. } => ("rewrite", ""),
            AiDirective::Brainstorm => ("brainstorm", ""),
        },
        AtMenuItemKind::InsertDate { style } => match style {
            DateInsertStyle::Short => ("date", "short"),
            DateInsertStyle::Medium => ("date", "medium"),
            DateInsertStyle::Long => ("date", "long"),
            DateInsertStyle::Iso => ("date", "iso"),
        },
        AtMenuItemKind::InsertEmoji { emoji } => ("emoji", *emoji),
    };
    format!("{section_tag}:{kind_tag}:{extra}")
}

/// Fan out to People / Documents / Insert / AI producers and
/// merge results. Errors are logged but don't block the other
/// producers — a Redis outage on user-search shouldn't hide
/// doc results or the Insert catalog.
///
/// `Slash` mode skips the People and Documents fetches entirely;
/// the slash-menu is command-shaped, not mention-shaped. The
/// static Insert + AI catalogs still run.
async fn resolve_at_items(query: &str, mode: AtMenuMode) -> Vec<AtMenuItem> {
    let trimmed = query.trim();

    // Server-side calls only fire when the query has content
    // AND the trigger mode is `At`. Static catalogs always run.
    // Sequential await (not a parallel join) so the frontend
    // crate doesn't pull in futures-util as a regular dep —
    // each call is single-digit-millisecond in the common
    // case, and the menu is debounced upstream by the trigger
    // detection.
    let (users_out, docs_out) = match (mode, trimmed.is_empty()) {
        (AtMenuMode::At, false) => {
            let u = fetch_user_items(trimmed).await;
            let d = fetch_document_items(trimmed).await;
            (u, d)
        }
        _ => (Vec::new(), Vec::new()),
    };

    let mut items: Vec<AtMenuItem> = Vec::new();
    items.extend(users_out);
    items.extend(docs_out);
    items.extend(insert_items(trimmed));
    items.extend(date_items(trimmed));
    items.extend(emoji_items(trimmed));
    items.extend(ai_items(trimmed));
    items
}

/// #148 v2 slice 3 — `@date` / `/date` insertable. Renders one
/// entry per style (Short, Medium, Long, Iso) when the user's
/// query matches the "date" keyword or one of the style names.
/// The trigger with no query matches nothing (kept off the
/// noise floor) — the empty-query menu shows People / Docs /
/// Insert / AI, not Date until the user narrows.
fn date_items(query: &str) -> Vec<AtMenuItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    // Match "date" prefix OR the style name itself. Prefix-match
    // covers `d`, `da`, `dat`, `date`; style-name match covers
    // `short`, `iso`, etc. when the user knows what they want.
    let matches_date = "date".starts_with(&q) || q.starts_with("date");
    let styles: &[(DateInsertStyle, &str, &str)] = &[
        (DateInsertStyle::Medium, "medium", "at-menu-insert-date-medium"),
        (DateInsertStyle::Short, "short", "at-menu-insert-date-short"),
        (DateInsertStyle::Long, "long", "at-menu-insert-date-long"),
        (DateInsertStyle::Iso, "iso", "at-menu-insert-date-iso"),
    ];
    styles
        .iter()
        .filter(|(_, style_key, _)| {
            matches_date || style_key.starts_with(&q) || q.starts_with(*style_key)
        })
        .map(|(style, _, label_key)| AtMenuItem {
            label: crate::i18n::translate(label_key, None),
            icon: "\u{1F4C5}".to_string(), // 📅
            section: AtMenuSection::Insert,
            kind: AtMenuItemKind::InsertDate { style: *style },
        })
        .collect()
}

/// #148 v2 slice 5 — small curated emoji table. Kept intentionally
/// short (~65 entries) so it fits in-binary without a wasm-size
/// hit and doesn't overwhelm the menu; typing `@` shows nothing
/// from this producer, but `@sm` narrows to smile / smirk, `@fire`
/// hits :fire:, etc. Names roughly follow Slack-style shortcodes
/// so muscle-memory carries over.
///
/// This is deliberately not the full Unicode CLDR emoji set —
/// that's ~3800 entries. If a richer picker is needed later, this
/// list stays as the "recent / common" shortcut and a modal picker
/// covers the long tail.
const EMOJI_TABLE: &[(&str, &[&str])] = &[
    ("\u{1F600}", &["smile", "happy", "grin"]),
    ("\u{1F604}", &["smile", "joy"]),
    ("\u{1F606}", &["laugh", "lol", "haha"]),
    ("\u{1F602}", &["joy", "cry-laugh", "lol"]),
    ("\u{1F609}", &["wink"]),
    ("\u{1F60A}", &["blush", "smile"]),
    ("\u{1F60D}", &["heart-eyes", "love"]),
    ("\u{1F618}", &["kiss", "kissing"]),
    ("\u{1F60E}", &["cool", "sunglasses"]),
    ("\u{1F914}", &["thinking", "hmm"]),
    ("\u{1F62C}", &["grimace", "yikes"]),
    ("\u{1F644}", &["eyeroll", "meh"]),
    ("\u{1F615}", &["confused", "unsure"]),
    ("\u{1F625}", &["sad", "disappointed"]),
    ("\u{1F62D}", &["cry", "sob"]),
    ("\u{1F621}", &["angry", "mad", "rage"]),
    ("\u{1F92F}", &["mind-blown", "shocked"]),
    ("\u{1F480}", &["skull", "dead"]),
    ("\u{1F44D}", &["thumbsup", "yes", "ok"]),
    ("\u{1F44E}", &["thumbsdown", "no"]),
    ("\u{1F44F}", &["clap", "applause"]),
    ("\u{1F64C}", &["hooray", "celebrate", "raised-hands"]),
    ("\u{1F64F}", &["please", "thanks", "pray"]),
    ("\u{270B}", &["hand", "wave", "stop"]),
    ("\u{270C}", &["peace", "victory"]),
    ("\u{1F91D}", &["handshake", "deal"]),
    ("\u{1F4AA}", &["muscle", "strong", "flex"]),
    ("\u{1F4A9}", &["poop", "crap"]),
    ("\u{2764}", &["heart", "love"]),
    ("\u{1F494}", &["broken-heart"]),
    ("\u{1F4A5}", &["boom", "explosion"]),
    ("\u{1F525}", &["fire", "lit", "hot"]),
    ("\u{2728}", &["sparkles", "magic", "new"]),
    ("\u{1F4A1}", &["idea", "lightbulb"]),
    ("\u{2705}", &["check", "done", "yes"]),
    ("\u{274C}", &["x", "no", "cross"]),
    ("\u{26A0}", &["warning", "alert"]),
    ("\u{1F6A8}", &["siren", "alert", "urgent"]),
    ("\u{1F389}", &["party", "celebrate", "tada"]),
    ("\u{1F381}", &["gift", "present"]),
    ("\u{1F680}", &["rocket", "launch", "ship"]),
    ("\u{1F41B}", &["bug", "insect"]),
    ("\u{1F41E}", &["lady-bug", "small-bug"]),
    ("\u{1F527}", &["wrench", "fix"]),
    ("\u{1F528}", &["hammer", "build"]),
    ("\u{1F4CC}", &["pin", "pushpin"]),
    ("\u{1F4CD}", &["location", "pin"]),
    ("\u{1F4CE}", &["paperclip", "attach"]),
    ("\u{1F4D3}", &["notebook", "notes"]),
    ("\u{1F4D6}", &["book", "read"]),
    ("\u{1F4DD}", &["memo", "note", "write"]),
    ("\u{1F4C8}", &["chart-up", "growth"]),
    ("\u{1F4C9}", &["chart-down", "decline"]),
    ("\u{1F4CA}", &["chart", "bar-chart"]),
    ("\u{1F4C5}", &["calendar", "date"]),
    ("\u{1F55B}", &["clock", "time"]),
    ("\u{23F0}", &["alarm", "clock", "wake"]),
    ("\u{1F4B0}", &["money", "cash", "bag"]),
    ("\u{1F4B8}", &["money-fly", "spending"]),
    ("\u{1F4E7}", &["email", "mail"]),
    ("\u{1F4AC}", &["chat", "message", "comment"]),
    ("\u{1F4F1}", &["phone", "mobile"]),
    ("\u{1F5A5}", &["computer", "laptop"]),
    ("\u{1F511}", &["key", "auth", "unlock"]),
    ("\u{1F512}", &["lock", "locked", "secure"]),
    ("\u{1F513}", &["unlocked", "open"]),
    ("\u{2615}", &["coffee", "tea"]),
    ("\u{1F37A}", &["beer", "drink"]),
    ("\u{1F355}", &["pizza"]),
    ("\u{1F354}", &["burger"]),
    ("\u{2600}", &["sun", "sunny"]),
    ("\u{1F319}", &["moon", "night"]),
    ("\u{2601}", &["cloud", "cloudy"]),
    ("\u{2614}", &["rain", "umbrella"]),
    ("\u{2744}", &["snow", "snowflake", "cold"]),
    ("\u{1F308}", &["rainbow"]),
    ("\u{1F31F}", &["star", "sparkle"]),
    ("\u{1F31E}", &["sun-face", "smile-sun"]),
];

/// #148 v2 slice 5 — narrow the emoji table by the user's query.
/// Matches on any keyword prefix or the leading "emoji" trigger
/// itself. Empty query returns nothing (keeps the menu quiet).
///
/// Small heuristic bias: ranking prefers keyword-prefix matches
/// over substring matches, so `@fi` puts `:fire:` at the top
/// rather than `:coffee:` (which has no "fi" but wouldn't match
/// anyway — the point is the ORDER of results when multiple do
/// match).
fn emoji_items(query: &str) -> Vec<AtMenuItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let show_all = "emoji".starts_with(&q) || q.starts_with("emoji");
    let mut hits: Vec<(u8, &'static str, &'static str)> = Vec::new();
    for (emoji, keywords) in EMOJI_TABLE {
        if show_all {
            hits.push((2, *emoji, keywords[0]));
            continue;
        }
        let mut best: Option<u8> = None;
        for kw in *keywords {
            if *kw == q {
                best = Some(0);
                break;
            }
            if kw.starts_with(&q) {
                best = Some(best.map_or(1, |b| b.min(1)));
            } else if kw.contains(&q) {
                best = Some(best.map_or(2, |b| b.min(2)));
            }
        }
        if let Some(rank) = best {
            hits.push((rank, *emoji, keywords[0]));
        }
    }
    hits.sort_by_key(|(rank, _, name)| (*rank, *name));
    // Cap results so a broad query like `@a` doesn't flood the menu.
    hits.truncate(12);
    hits.into_iter()
        .map(|(_, emoji, name)| AtMenuItem {
            label: format!("{emoji} :{name}:"),
            icon: emoji.to_string(),
            section: AtMenuSection::Insert,
            kind: AtMenuItemKind::InsertEmoji { emoji },
        })
        .collect()
}

async fn fetch_user_items(query: &str) -> Vec<AtMenuItem> {
    match users::search_users(query).await {
        Ok(resp) => resp
            .users
            .into_iter()
            .map(|u| AtMenuItem {
                label: format!("{} ({})", u.name, u.email),
                icon: "\u{1F464}".to_string(), // 👤
                section: AtMenuSection::People,
                kind: AtMenuItemKind::UserMention {
                    user_id: u.user_id,
                    display: u.name,
                },
            })
            .collect(),
        Err(e) => {
            web_sys::console::warn_1(&format!("at-menu users: {e:?}").into());
            Vec::new()
        }
    }
}

async fn fetch_document_items(query: &str) -> Vec<AtMenuItem> {
    match search::search(query, Some(6)).await {
        Ok(resp) => resp
            .results
            .into_iter()
            .map(|r| AtMenuItem {
                icon: match r.doc_type.as_str() {
                    "spreadsheet" => "\u{1F4CA}".to_string(),
                    "chat" => "\u{1F4AC}".to_string(),
                    _ => "\u{1F4C4}".to_string(),
                },
                label: r.title.clone(),
                section: AtMenuSection::Documents,
                kind: AtMenuItemKind::DocumentLink {
                    doc_id: r.id,
                    title: r.title,
                },
            })
            .collect(),
        Err(e) => {
            web_sys::console::warn_1(&format!("at-menu docs: {e:?}").into());
            Vec::new()
        }
    }
}

fn insert_items(query: &str) -> Vec<AtMenuItem> {
    full_catalog()
        .into_iter()
        .filter(|item| item.matches_query(query))
        .filter_map(|item| catalog_item_to_at_menu_item(&item))
        .collect()
}

fn catalog_item_to_at_menu_item(item: &CatalogItem) -> Option<AtMenuItem> {
    // Skip catalog entries whose section isn't menu-shaped.
    let section = match item.section() {
        InsertSection::Ai | InsertSection::Runtime => return None,
        _ => AtMenuSection::Insert,
    };
    let label = crate::i18n::translate(item.label_key(), None);
    let icon = item.icon().to_string();
    let kind = match item.command() {
        crate::components::toolbar::ToolbarCommand::InsertTable => {
            AtMenuItemKind::InsertTable
        }
        crate::components::toolbar::ToolbarCommand::UploadImage => {
            AtMenuItemKind::InsertImage
        }
        crate::components::toolbar::ToolbarCommand::InsertHorizontalRule => {
            AtMenuItemKind::InsertHorizontalRule
        }
        crate::components::toolbar::ToolbarCommand::SetCodeBlock => {
            AtMenuItemKind::SetCodeBlock
        }
        crate::components::toolbar::ToolbarCommand::InsertLiveApp(id) => {
            AtMenuItemKind::InsertLiveApp { id }
        }
        // A catalog entry with any other command shape can't be
        // routed through the enum yet. Log a warning so a future
        // added entry with an unmapped shape doesn't silently
        // vanish from the @-menu (`inserts::tests` covers the
        // catalog itself; this branch guards the @-menu-side
        // routing).
        other => {
            web_sys::console::warn_1(
                &format!(
                    "at-menu: catalog entry {:?} has no AtMenuItemKind mapping for {other:?}",
                    item.id(),
                )
                .into(),
            );
            return None;
        }
    };
    Some(AtMenuItem {
        label,
        icon,
        section,
        kind,
    })
}

fn ai_items(query: &str) -> Vec<AtMenuItem> {
    let trimmed = query.trim();
    let mut items = Vec::new();

    // Base `@ask <prompt>` — always shows so users can send a
    // free-form question without a directive.
    let base_label = if trimmed.is_empty() {
        crate::t!("at-menu-ask-ai-hint")
    } else {
        format!("Ask AI: {trimmed}")
    };
    items.push(AtMenuItem {
        label: base_label,
        icon: "\u{2728}".to_string(), // ✨
        section: AtMenuSection::Ai,
        kind: AtMenuItemKind::AskAi {
            prompt: trimmed.to_string(),
        },
    });

    // #148 v2 — four directive wrappers. Each carries whatever
    // the user typed as `user_input`; the on_at_select layer
    // composes the final prompt against selection/doc content.
    items.push(AtMenuItem {
        label: crate::t!("at-menu-ai-summarize"),
        icon: "\u{1F4DD}".to_string(), // 📝
        section: AtMenuSection::Ai,
        kind: AtMenuItemKind::AskWithDirective {
            directive: AiDirective::Summarize,
            user_input: trimmed.to_string(),
        },
    });
    items.push(AtMenuItem {
        label: crate::t!("at-menu-ai-translate"),
        icon: "\u{1F310}".to_string(), // 🌐
        section: AtMenuSection::Ai,
        kind: AtMenuItemKind::AskWithDirective {
            directive: AiDirective::Translate {
                target: trimmed.to_string(),
            },
            user_input: trimmed.to_string(),
        },
    });
    items.push(AtMenuItem {
        label: crate::t!("at-menu-ai-rewrite"),
        icon: "\u{270F}".to_string(), // ✏
        section: AtMenuSection::Ai,
        kind: AtMenuItemKind::AskWithDirective {
            directive: AiDirective::Rewrite {
                tone: trimmed.to_string(),
            },
            user_input: trimmed.to_string(),
        },
    });
    items.push(AtMenuItem {
        label: crate::t!("at-menu-ai-brainstorm"),
        icon: "\u{1F4A1}".to_string(), // 💡
        section: AtMenuSection::Ai,
        kind: AtMenuItemKind::AskWithDirective {
            directive: AiDirective::Brainstorm,
            user_input: trimmed.to_string(),
        },
    });

    items
}

/// Section-grouped rendering. Groups results by their `section`
/// field, orders by canonical section order, hides empty
/// sections. Highlighted-index is the flat index across all
/// currently-visible items (matches the ↑/↓ dispatch in
/// pages/document.rs).
fn render_sections(
    items: Vec<AtMenuItem>,
    highlighted: ReadSignal<usize>,
    on_select: Callback<AtMenuItem>,
) -> impl IntoView {
    let ordered: Vec<AtMenuSection> = vec![
        AtMenuSection::People,
        AtMenuSection::Documents,
        AtMenuSection::Insert,
        AtMenuSection::Ai,
    ];

    // Flat index counter accumulated across sections.
    let mut flat: usize = 0;
    let mut sections_view = Vec::new();
    for section in ordered {
        let section_items: Vec<(usize, AtMenuItem)> = items
            .iter()
            .filter(|i| i.section == section)
            .cloned()
            .map(|i| {
                let idx = flat;
                flat += 1;
                (idx, i)
            })
            .collect();
        if section_items.is_empty() {
            continue;
        }
        let header_key = section.label_key();
        let rows = section_items
            .into_iter()
            .map(|(idx, item)| {
                let item_click = item.clone();
                view! {
                    <div
                        class="at-menu-item"
                        class:at-menu-item--highlighted=move || highlighted.get() == idx
                        data-at-menu-idx=idx.to_string()
                        on:click=move |_| on_select.run(item_click.clone())
                    >
                        <span class="at-menu-icon">{item.icon.clone()}</span>
                        <span class="at-menu-label">{item.label.clone()}</span>
                    </div>
                }
            })
            .collect::<Vec<_>>();
        sections_view.push(view! {
            <div class="at-menu-section">
                <div class="at-menu-section-header">
                    {crate::i18n::translate(header_key, None)}
                </div>
                <div class="at-menu-section-body">
                    {rows}
                </div>
            </div>
        });
    }

    view! {
        <div class="at-menu-results">
            {sections_view}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_item(section: AtMenuSection, kind: AtMenuItemKind) -> AtMenuItem {
        AtMenuItem {
            label: String::new(),
            icon: String::new(),
            section,
            kind,
        }
    }

    #[test]
    fn mode_prefix_selects_the_right_trigger_char() {
        assert_eq!(AtMenuMode::At.prefix(), '@');
        assert_eq!(AtMenuMode::Slash.prefix(), '/');
    }

    #[test]
    fn identity_distinguishes_kinds_within_a_section() {
        let a = mk_item(
            AtMenuSection::People,
            AtMenuItemKind::UserMention {
                user_id: "u-1".into(),
                display: "Alice".into(),
            },
        );
        let b = mk_item(
            AtMenuSection::People,
            AtMenuItemKind::UserMention {
                user_id: "u-2".into(),
                display: "Alice".into(),
            },
        );
        assert_ne!(
            item_identity(&a),
            item_identity(&b),
            "two Alices with different user_ids must not share an identity"
        );
    }

    #[test]
    fn identity_matches_same_underlying_item() {
        let a = mk_item(
            AtMenuSection::Documents,
            AtMenuItemKind::DocumentLink {
                doc_id: "doc-abc".into(),
                title: "Design".into(),
            },
        );
        let b = mk_item(
            AtMenuSection::Documents,
            AtMenuItemKind::DocumentLink {
                doc_id: "doc-abc".into(),
                title: "Design (renamed)".into(),
            },
        );
        assert_eq!(
            item_identity(&a),
            item_identity(&b),
            "same doc_id must share identity even if the display title changed"
        );
    }

    #[test]
    fn identity_distinguishes_sections() {
        // A user named exactly the same as a document — different
        // sections, so different identity.
        let user = mk_item(
            AtMenuSection::People,
            AtMenuItemKind::UserMention {
                user_id: "u-1".into(),
                display: "Roadmap".into(),
            },
        );
        let doc = mk_item(
            AtMenuSection::Documents,
            AtMenuItemKind::DocumentLink {
                doc_id: "u-1".into(),
                title: "Roadmap".into(),
            },
        );
        assert_ne!(item_identity(&user), item_identity(&doc));
    }

    #[test]
    fn identity_stable_for_static_inserts() {
        let a = mk_item(AtMenuSection::Insert, AtMenuItemKind::InsertTable);
        let b = mk_item(AtMenuSection::Insert, AtMenuItemKind::InsertTable);
        assert_eq!(item_identity(&a), item_identity(&b));
        let c = mk_item(AtMenuSection::Insert, AtMenuItemKind::InsertImage);
        assert_ne!(item_identity(&a), item_identity(&c));
    }

    #[test]
    fn compose_summarize_uses_scope_label_and_appends_source() {
        use crate::editor::commands::TextScope;
        let out = compose_directive_prompt(
            &AiDirective::Summarize,
            "",
            "Lorem ipsum body.",
            TextScope::WholeDoc,
        );
        assert!(out.contains("Summarize the following document"), "{out}");
        assert!(out.ends_with("Lorem ipsum body."), "{out}");

        let sel = compose_directive_prompt(
            &AiDirective::Summarize,
            "",
            "A paragraph.",
            TextScope::Selection,
        );
        assert!(sel.contains("selected text"), "{sel}");
    }

    #[test]
    fn compose_summarize_honors_user_focus_hint() {
        use crate::editor::commands::TextScope;
        let out = compose_directive_prompt(
            &AiDirective::Summarize,
            "the risks section",
            "body",
            TextScope::WholeDoc,
        );
        assert!(out.contains("Focus on: the risks section."), "{out}");
    }

    #[test]
    fn compose_translate_defaults_to_spanish_when_target_missing() {
        use crate::editor::commands::TextScope;
        let out = compose_directive_prompt(
            &AiDirective::Translate {
                target: String::new(),
            },
            "",
            "Hello world",
            TextScope::Selection,
        );
        assert!(out.contains("to Spanish"), "{out}");
    }

    #[test]
    fn compose_translate_uses_supplied_target() {
        use crate::editor::commands::TextScope;
        let out = compose_directive_prompt(
            &AiDirective::Translate {
                target: "French".into(),
            },
            "",
            "Hello",
            TextScope::Selection,
        );
        assert!(out.contains("to French"), "{out}");
        assert!(!out.contains("to Spanish"), "{out}");
    }

    #[test]
    fn compose_rewrite_defaults_when_tone_missing() {
        use crate::editor::commands::TextScope;
        let out = compose_directive_prompt(
            &AiDirective::Rewrite {
                tone: String::new(),
            },
            "",
            "This is a wordy paragraph.",
            TextScope::Selection,
        );
        assert!(out.contains("concise and clear"), "{out}");
    }

    #[test]
    fn compose_rewrite_uses_supplied_tone() {
        use crate::editor::commands::TextScope;
        let out = compose_directive_prompt(
            &AiDirective::Rewrite {
                tone: "formal".into(),
            },
            "",
            "hey there.",
            TextScope::Selection,
        );
        assert!(out.contains("to be formal"), "{out}");
    }

    #[test]
    fn compose_brainstorm_uses_user_input_over_source() {
        use crate::editor::commands::TextScope;
        let out = compose_directive_prompt(
            &AiDirective::Brainstorm,
            "reducing churn",
            "unrelated selection body",
            TextScope::Selection,
        );
        assert!(out.contains("about: reducing churn"), "{out}");
        assert!(!out.contains("unrelated selection body"), "{out}");
    }

    #[test]
    fn compose_brainstorm_falls_back_to_source_when_empty() {
        use crate::editor::commands::TextScope;
        let out = compose_directive_prompt(
            &AiDirective::Brainstorm,
            "",
            "onboarding pain points",
            TextScope::Selection,
        );
        assert!(out.contains("about: onboarding pain points"), "{out}");
    }

    #[test]
    fn identity_distinguishes_live_app_entries_by_id() {
        let kanban = mk_item(
            AtMenuSection::Insert,
            AtMenuItemKind::InsertLiveApp { id: "kanban" },
        );
        let calendar = mk_item(
            AtMenuSection::Insert,
            AtMenuItemKind::InsertLiveApp { id: "calendar" },
        );
        assert_ne!(item_identity(&kanban), item_identity(&calendar));
    }

    #[test]
    fn date_items_empty_query_returns_nothing() {
        assert!(date_items("").is_empty());
        assert!(date_items("   ").is_empty());
    }

    #[test]
    fn date_items_date_prefix_returns_all_styles() {
        for q in &["d", "da", "dat", "date"] {
            let items = date_items(q);
            assert_eq!(
                items.len(),
                4,
                "query {q:?} should surface all four styles"
            );
            let ids: Vec<_> = items.iter().map(item_identity).collect();
            assert!(ids.iter().any(|i| i == "i:date:medium"));
            assert!(ids.iter().any(|i| i == "i:date:short"));
            assert!(ids.iter().any(|i| i == "i:date:long"));
            assert!(ids.iter().any(|i| i == "i:date:iso"));
        }
    }

    #[test]
    fn date_items_style_keyword_narrows_to_that_style() {
        let items = date_items("iso");
        assert_eq!(items.len(), 1);
        assert_eq!(item_identity(&items[0]), "i:date:iso");

        let items = date_items("short");
        assert_eq!(items.len(), 1);
        assert_eq!(item_identity(&items[0]), "i:date:short");
    }

    #[test]
    fn date_items_ignores_unrelated_query() {
        assert!(date_items("kanban").is_empty());
        assert!(date_items("xyzzy").is_empty());
    }

    // ── #148 v2 slice 5 emoji items ──────────────────────────

    #[test]
    fn emoji_items_empty_query_returns_nothing() {
        assert!(emoji_items("").is_empty());
        assert!(emoji_items("   ").is_empty());
    }

    #[test]
    fn emoji_items_exact_keyword_finds_that_emoji() {
        let items = emoji_items("fire");
        assert!(!items.is_empty(), "'fire' should match");
        // First result should be the fire emoji.
        match items[0].kind {
            AtMenuItemKind::InsertEmoji { emoji } => {
                assert_eq!(emoji, "\u{1F525}", "🔥 expected first for 'fire'");
            }
            _ => panic!("expected InsertEmoji"),
        }
    }

    #[test]
    fn emoji_items_prefix_ranks_before_substring() {
        // "sm" prefix-matches "smile" / "smirk"; substring-matches
        // nothing else in the table. Verifies the rank/sort
        // orders prefix hits ahead of substring hits.
        let items = emoji_items("sm");
        assert!(!items.is_empty());
        // Every returned item should have a keyword starting
        // with "sm" (there are no non-prefix matches in the table
        // for this query).
        for item in &items {
            if let AtMenuItemKind::InsertEmoji { emoji } = item.kind {
                let (_, kws) = EMOJI_TABLE.iter().find(|(e, _)| *e == emoji).unwrap();
                assert!(
                    kws.iter().any(|k| k.starts_with("sm")),
                    "returned emoji {emoji:?} has no keyword starting with 'sm'; keywords: {kws:?}"
                );
            }
        }
    }

    #[test]
    fn emoji_items_caps_result_count() {
        // Broad query `a` matches lots of substrings; ensure the
        // producer caps to 12 so a wide query doesn't flood.
        let items = emoji_items("a");
        assert!(items.len() <= 12);
    }

    #[test]
    fn emoji_items_emoji_prefix_returns_broad_set() {
        // Typing just `@emoji` (or a prefix like `@em`) surfaces
        // a broad slice of the table rather than nothing, so a
        // user without a specific term in mind gets a picker.
        let items = emoji_items("em");
        assert!(items.len() >= 10);
    }

    #[test]
    fn identity_distinguishes_emoji_by_char() {
        let fire = mk_item(
            AtMenuSection::Insert,
            AtMenuItemKind::InsertEmoji {
                emoji: "\u{1F525}",
            },
        );
        let heart = mk_item(
            AtMenuSection::Insert,
            AtMenuItemKind::InsertEmoji {
                emoji: "\u{2764}",
            },
        );
        assert_ne!(item_identity(&fire), item_identity(&heart));
    }

    #[test]
    fn identity_distinguishes_date_styles() {
        let med = mk_item(
            AtMenuSection::Insert,
            AtMenuItemKind::InsertDate {
                style: DateInsertStyle::Medium,
            },
        );
        let iso = mk_item(
            AtMenuSection::Insert,
            AtMenuItemKind::InsertDate {
                style: DateInsertStyle::Iso,
            },
        );
        assert_ne!(item_identity(&med), item_identity(&iso));
    }

    // ── #148 v2 compose_directive_parts (visible + hidden split) ──

    #[test]
    fn compose_parts_summarize_keeps_source_out_of_instruction() {
        use crate::editor::commands::TextScope;
        let (instruction, suffix) = compose_directive_parts(
            &AiDirective::Summarize,
            "",
            "SECRET DOC CONTENT",
            TextScope::WholeDoc,
        );
        assert!(
            !instruction.contains("SECRET DOC CONTENT"),
            "instruction (visible in input) must not contain source text; got: {instruction}"
        );
        assert!(
            instruction.contains("Summarize"),
            "instruction must be a readable directive; got: {instruction}"
        );
        let suffix = suffix.expect("Summarize should produce a hidden suffix");
        assert!(
            suffix.contains("SECRET DOC CONTENT"),
            "hidden suffix must carry the source text"
        );
        assert!(
            suffix.contains("Use ONLY"),
            "hidden suffix must carry the scope guard"
        );
    }

    #[test]
    fn compose_parts_translate_carries_target_in_visible_instruction() {
        use crate::editor::commands::TextScope;
        let (instruction, suffix) = compose_directive_parts(
            &AiDirective::Translate {
                target: "French".into(),
            },
            "",
            "Bonjour",
            TextScope::Selection,
        );
        assert!(instruction.contains("French"));
        assert!(!instruction.contains("Bonjour"));
        assert!(suffix.unwrap().contains("Bonjour"));
    }

    #[test]
    fn compose_parts_brainstorm_returns_no_hidden_suffix() {
        use crate::editor::commands::TextScope;
        let (instruction, suffix) = compose_directive_parts(
            &AiDirective::Brainstorm,
            "onboarding pain points",
            "",
            TextScope::WholeDoc,
        );
        assert!(instruction.contains("onboarding pain points"));
        assert!(
            suffix.is_none(),
            "Brainstorm is self-contained; no hidden suffix (else the scope guard would suppress new-idea generation)"
        );
    }

    #[test]
    fn compose_full_prompt_matches_parts_reassembled() {
        // The legacy `compose_directive_prompt` should equal the
        // parts concatenated with `\n\n`. Pins the reassembly
        // contract used by the AskDialog's submit path.
        use crate::editor::commands::TextScope;
        for scope in [TextScope::WholeDoc, TextScope::Selection] {
            let full = compose_directive_prompt(
                &AiDirective::Summarize,
                "risks section",
                "the source",
                scope,
            );
            let (instr, suffix) = compose_directive_parts(
                &AiDirective::Summarize,
                "risks section",
                "the source",
                scope,
            );
            let reassembled = match suffix {
                Some(s) => format!("{instr}\n\n{s}"),
                None => instr,
            };
            assert_eq!(full, reassembled);
        }
    }
}
