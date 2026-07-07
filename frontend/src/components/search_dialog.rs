// Copyright (c) 2026 Joel Baumert. All Rights Reserved.

use leptos::prelude::*;

use crate::a11y;
use crate::api::search;
use crate::commands::{self, CommandScope, CommandView};
use crate::i18n::format_relative;

/// Strip all HTML tags except `<b>` and `</b>` from a snippet.
/// Tantivy's `to_html()` already escapes document text via `encode_minimal`,
/// but this provides defense-in-depth against any future changes.
///
/// Operates on `&str` slices throughout — `<` and `>` are ASCII, so every
/// slice boundary lands on a char boundary and multi-byte UTF-8 text is
/// copied through verbatim. (The previous byte-loop pushed `bytes[i] as
/// char`, which reinterpreted each UTF-8 byte as a Latin-1 scalar and
/// mangled non-ASCII text into mojibake — #6.)
fn sanitize_snippet(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    // `<` is ASCII and can never appear inside a multi-byte UTF-8 sequence,
    // so `find('<')` always returns a char boundary.
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt..];
        // Preserve only the bold-tag allowlist (case-sensitive, matching
        // Tantivy's output); drop every other tag.
        if let Some(tail) = after.strip_prefix("<b>") {
            out.push_str("<b>");
            rest = tail;
        } else if let Some(tail) = after.strip_prefix("</b>") {
            out.push_str("</b>");
            rest = tail;
        } else if let Some(gt) = after.find('>') {
            // Skip the unknown tag, including its closing `>`.
            rest = &after[gt + 1..];
        } else {
            // Unterminated tag — drop the remainder.
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn doc_type_icon(doc_type: &str) -> &'static str {
    match doc_type {
        "spreadsheet" => "\u{1F4CA}", // 📊
        "chat" => "\u{1F4AC}",        // 💬
        _ => "\u{1F4C4}",             // 📄
    }
}

/// Phase 5 M-P4 piece A — palette mode. `Search` is the existing
/// document-search path; `Action` resolves the query against the
/// command registry. Switching is driven by a `>` prefix on the
/// input — typing `>` enters Action mode, deleting back through
/// it returns to Search.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Search,
    Action,
}

#[component]
pub fn SearchDialog(
    visible: ReadSignal<bool>,
    on_close: Callback<()>,
    /// Scope of the hosting page — drives which scope-specific
    /// commands appear in Action mode. M-P4 piece B addition; the
    /// piece-A signature passed an implicit `Global`. Callers
    /// without a context (e.g. an embedded search widget) can
    /// pass `CommandScope::Global` to opt out.
    #[prop(default = CommandScope::Global)]
    scope: CommandScope,
    /// When the dialog opens, start in this mode. Reactive so the
    /// page can flip between Ctrl+K (Search) and Ctrl+Shift+P
    /// (Action) without re-mounting the dialog. The default is a
    /// stable Search-mode signal — callers without a binding can
    /// omit the prop.
    #[prop(into, default = Signal::derive(|| PaletteMode::Search))]
    initial_mode: Signal<PaletteMode>,
) -> impl IntoView {
    let (mode, set_mode) = signal(PaletteMode::Search);
    let (query, set_query) = signal(String::new());
    let (results, set_results) = signal::<Vec<search::SearchResultItem>>(Vec::new());
    let (actions, set_actions) = signal::<Vec<CommandView>>(Vec::new());
    let (loading, set_loading) = signal(false);
    let (has_searched, set_has_searched) = signal(false);
    // Generation counter for debounce — each keystroke increments it;
    // after the 300ms delay, the async task only fires the API call if
    // its generation still matches the latest value.
    let (search_seq, set_search_seq) = signal(0u32);

    let input_ref = NodeRef::<leptos::html::Input>::new();
    // M-P8 piece A: outer container ref for the focus-trap helper.
    // The input still owns the auto-focus on open; the trap kicks
    // in once the user starts tabbing.
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    a11y::install_focus_trap(dialog_ref, visible.into());

    // Auto-focus when dialog opens; honor `initial_mode` so a
    // Ctrl+Shift+P binding can open directly in Action mode. The
    // `>` prefill prepares the input so the existing on_input
    // path takes over without a special-case render branch.
    // Reset state when it closes.
    Effect::new(move |_| {
        if visible.get() {
            let start_mode = initial_mode.get_untracked();
            if start_mode == PaletteMode::Action {
                // Pre-fill so the on_input handler we'd fire on the
                // next keystroke is already in action territory.
                // matching() runs synchronously so the user sees
                // commands before they type anything.
                set_query.set(">".to_string());
                set_mode.set(PaletteMode::Action);
                set_actions.set(commands::matching("", scope));
            }
            let el = input_ref;
            leptos::task::spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(0).await;
                // `.get_untracked()` because tracking doesn't apply
                // inside an async block — the outer Effect already
                // tracks `visible` to fire at the right moment, and
                // accessing the NodeRef with tracking here just
                // produces a "reactive access outside context"
                // warning at runtime.
                if let Some(input) = el.get_untracked() {
                    let _ = input.focus();
                    // Move caret to end so the user can keep typing
                    // their filter without backspacing through the `>`.
                    let len = input.value().len() as u32;
                    let _ = input.set_selection_range(len, len);
                }
            });
        } else {
            set_query.set(String::new());
            set_results.set(Vec::new());
            set_actions.set(Vec::new());
            set_has_searched.set(false);
            set_loading.set(false);
            set_mode.set(PaletteMode::Search);
            set_search_seq.update(|g| *g = g.wrapping_add(1));
        }
    });

    let on_input = move |e: web_sys::Event| {
        let value = event_target_value(&e);
        set_query.set(value.clone());
        set_search_seq.update(|g| *g = g.wrapping_add(1));
        let seq = search_seq.get_untracked();

        // Mode switch: `>` flips into Action mode (run a command).
        // `?` flips into Action mode filtered to commands that bind
        // a keyboard shortcut — M-P4 piece C "shortcut help". Both
        // share the Action mode rendering; the difference is just
        // the filter and the lack of a `>` prefix.
        if let Some(rest) = value.strip_prefix('?') {
            set_mode.set(PaletteMode::Action);
            let action_query = rest.trim_start();
            set_actions.set(commands::matching_with_shortcuts(action_query, scope));
            set_loading.set(false);
            return;
        }
        if let Some(rest) = value.strip_prefix('>') {
            set_mode.set(PaletteMode::Action);
            // Hosting page's scope drives which commands surface —
            // Editor cmds rank above Global ones inside the editor,
            // Home page only sees Global. See commands::matching.
            let action_query = rest.trim_start();
            set_actions.set(commands::matching(action_query, scope));
            set_loading.set(false);
            return;
        }
        set_mode.set(PaletteMode::Search);
        set_actions.set(Vec::new());

        if value.trim().is_empty() {
            set_results.set(Vec::new());
            set_has_searched.set(false);
            set_loading.set(false);
            return;
        }

        set_loading.set(true);
        leptos::task::spawn_local(async move {
            // Debounce: wait 300ms, then check if we're still current
            gloo_timers::future::TimeoutFuture::new(300).await;
            if search_seq.get_untracked() != seq {
                return;
            }

            match search::search(&value, Some(20)).await {
                Ok(resp) => {
                    if search_seq.get_untracked() == seq {
                        set_results.set(resp.results);
                        set_has_searched.set(true);
                        set_loading.set(false);
                    }
                }
                Err(e) => {
                    if search_seq.get_untracked() == seq {
                        web_sys::console::error_1(
                            &format!("Search failed: {e}").into(),
                        );
                        set_results.set(Vec::new());
                        set_has_searched.set(true);
                        set_loading.set(false);
                    }
                }
            }
        });
    };

    view! {
        <Show when=move || visible.get()>
            <div class="search-backdrop" on:click=move |_| a11y::defer_close(on_close)>
                <div
                    node_ref=dialog_ref
                    class="search-dialog"
                    role="dialog"
                    aria-modal="true"
                    aria-label=crate::t!("search-dialog-label")
                    on:click=move |e: web_sys::MouseEvent| e.stop_propagation()
                    on:keydown=move |e: web_sys::KeyboardEvent| {
                        if let Some(node) = dialog_ref.get() {
                            a11y::handle_tab_trap(&e, node.as_ref());
                        }
                    }
                >
                    <div class="search-input-wrapper">
                        <span class="search-icon">{"\u{1F50D}"}</span>
                        <input
                            node_ref=input_ref
                            type="text"
                            class="search-input"
                            placeholder=crate::t!("search-placeholder")
                            prop:value=move || query.get()
                            on:input=on_input
                            on:keydown=move |e: web_sys::KeyboardEvent| {
                                if e.key() == "Escape" {
                                    // Defer the close one microtask so the
                                    // Escape keydown finishes bubbling
                                    // before Leptos tears down the dialog's
                                    // child closures — running on_close
                                    // synchronously here lets the bubble
                                    // reach a now-dropped wasm-bindgen
                                    // closure and panic. Symmetric with
                                    // the focus-restoration defer in
                                    // install_focus_trap.
                                    a11y::defer_close(on_close);
                                    return;
                                }
                                // Phase 5 M-P4 piece D: Enter runs the
                                // first item in Action mode and
                                // navigates to the first result in
                                // Search mode. The dialog stays open
                                // for empty result sets; pressing
                                // Enter when there's nothing to act on
                                // is a no-op rather than a close.
                                if e.key() == "Enter" {
                                    match mode.get_untracked() {
                                        PaletteMode::Action => {
                                            if let Some(first) =
                                                actions.get_untracked().first().cloned()
                                            {
                                                e.prevent_default();
                                                // Close the dialog first
                                                // (one microtask later for
                                                // the bubble-after-drop
                                                // reason as Escape), THEN
                                                // run the command. Running
                                                // the command before the
                                                // close leaves the palette
                                                // input still focused — for
                                                // editor commands that
                                                // means the editor's DOM
                                                // selection isn't observable
                                                // when toggle_mark resolves
                                                // and the command applies to
                                                // an empty selection. See
                                                // defer_close_then_run for
                                                // the full rationale.
                                                a11y::defer_close_then_run(on_close, move || {
                                                    commands::run(first.id);
                                                });
                                            }
                                        }
                                        PaletteMode::Search => {
                                            if let Some(first) =
                                                results.get_untracked().first().cloned()
                                            {
                                                e.prevent_default();
                                                // #152: client-side nav (shell stays mounted).
                                                crate::commands::nav_bridge::go(
                                                    &format!("/d/{}/doc", first.id),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        />
                        <span class="search-shortcut">"Esc"</span>
                    </div>

                    <div class="search-results" tabindex="0">
                        // ─── Action mode ─────────────────────────
                        <Show when=move || mode.get() == PaletteMode::Action>
                            <Show when=move || actions.get().is_empty()>
                                <div class="search-empty">
                                    {crate::t!("palette-no-actions")}
                                </div>
                            </Show>
                            <Show when=move || !actions.get().is_empty()>
                                {move || {
                                    actions
                                        .get()
                                        .into_iter()
                                        .map(|cmd| {
                                            let id = cmd.id;
                                            let label = cmd.label.clone();
                                            let scope_class = cmd.scope_class();
                                            let shortcut = cmd.shortcut;
                                            let on_close_local = on_close;
                                            view! {
                                                <div
                                                    class=format!("command-item {scope_class}")
                                                    on:click=move |_| {
                                                        commands::run(id);
                                                        on_close_local.run(());
                                                    }
                                                >
                                                    <span class="command-item-icon">"\u{203A}"</span>
                                                    <div class="command-item-content">
                                                        <div class="command-item-label">{label}</div>
                                                    </div>
                                                    {shortcut.map(|s| view! {
                                                        <span class="command-item-shortcut">{s}</span>
                                                    })}
                                                </div>
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                }}
                            </Show>
                        </Show>

                        // ─── Search mode ─────────────────────────
                        <Show when=move || mode.get() == PaletteMode::Search && loading.get()>
                            <div class="search-loading">{crate::t!("search-searching")}</div>
                        </Show>

                        <Show when=move || {
                            mode.get() == PaletteMode::Search
                                && !loading.get()
                                && has_searched.get()
                                && results.get().is_empty()
                        }>
                            <div class="search-empty">{crate::t!("search-no-results")}</div>
                        </Show>

                        <Show when=move || {
                            mode.get() == PaletteMode::Search
                                && !loading.get()
                                && !results.get().is_empty()
                        }>
                            {move || {
                                results
                                    .get()
                                    .into_iter()
                                    .map(|item| {
                                        let id = item.id.clone();
                                        let icon = doc_type_icon(&item.doc_type);
                                        let snippet = sanitize_snippet(&item.snippet);
                                        let time = format_relative(item.updated_at);
                                        view! {
                                            <div
                                                class="search-result-item"
                                                on:click=move |_| {
                                                    // #152: client-side nav (shell stays mounted).
                                                    crate::commands::nav_bridge::go(
                                                        &format!("/d/{}/doc", id),
                                                    );
                                                }
                                            >
                                                <span class="search-result-icon">{icon}</span>
                                                <div class="search-result-content">
                                                    <div class="search-result-title">
                                                        {item.title}
                                                    </div>
                                                    <div
                                                        class="search-result-snippet"
                                                        inner_html=snippet
                                                    />
                                                </div>
                                                <span class="search-result-time">{time}</span>
                                            </div>
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            }}
                        </Show>
                    </div>
                </div>
            </div>
        </Show>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_preserves_bold_tags() {
        assert_eq!(
            sanitize_snippet("the <b>auth</b> system"),
            "the <b>auth</b> system"
        );
    }

    #[test]
    fn sanitize_strips_script_tags() {
        assert_eq!(
            sanitize_snippet("<script>alert(1)</script>hello"),
            "alert(1)hello"
        );
    }

    #[test]
    fn sanitize_strips_img_onerror() {
        assert_eq!(
            sanitize_snippet("before<img onerror=alert(1) src=x>after"),
            "beforeafter"
        );
    }

    #[test]
    fn sanitize_preserves_plain_text() {
        assert_eq!(sanitize_snippet("no tags here"), "no tags here");
    }

    #[test]
    fn sanitize_preserves_multibyte_utf8() {
        // #6: the old byte-loop turned each UTF-8 byte into a Latin-1
        // char, corrupting non-ASCII text. Multi-byte content (and the
        // bold allowlist around it) must survive verbatim.
        assert_eq!(sanitize_snippet("café — naïve 日本語 🎉"), "café — naïve 日本語 🎉");
        assert_eq!(
            sanitize_snippet("the <b>café</b> menu"),
            "the <b>café</b> menu"
        );
        // A stripped tag adjacent to multi-byte text must not split a
        // codepoint or leak mojibake.
        assert_eq!(
            sanitize_snippet("日本<script>x</script>語"),
            "日本x語"
        );
    }

    #[test]
    fn sanitize_handles_empty_string() {
        assert_eq!(sanitize_snippet(""), "");
    }

    #[test]
    fn sanitize_preserves_escaped_entities() {
        // Tantivy escapes < > & in text — those should pass through unchanged
        assert_eq!(
            sanitize_snippet("x &lt; y &amp; z"),
            "x &lt; y &amp; z"
        );
    }

    #[test]
    fn sanitize_strips_nested_dangerous_tags() {
        assert_eq!(
            sanitize_snippet("<div><b>good</b><iframe src=evil></iframe></div>"),
            "<b>good</b>"
        );
    }

    #[test]
    fn sanitize_handles_unclosed_tag() {
        assert_eq!(sanitize_snippet("text<br"), "text");
    }
}
