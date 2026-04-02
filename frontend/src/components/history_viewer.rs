use leptos::prelude::*;

use crate::api::history;
use crate::editor::yrs_bridge;

/// Edit history viewer panel.
/// Lists document versions and shows a text diff when a version is selected.
#[component]
pub fn HistoryViewer(
    /// Whether the panel is visible.
    visible: ReadSignal<bool>,
    /// Document ID.
    doc_id: ReadSignal<String>,
    /// Current document text (for diffing).
    current_text: ReadSignal<String>,
) -> impl IntoView {
    let (versions, set_versions) = signal::<Vec<history::VersionEntry>>(Vec::new());
    let (loading, set_loading) = signal(false);
    let (diff_lines, set_diff_lines) = signal::<Vec<DiffLine>>(Vec::new());
    let (selected_version, set_selected_version) = signal::<Option<u64>>(None);

    // Load versions when panel opens.
    Effect::new(move |_| {
        if !visible.get() {
            return;
        }
        let id = doc_id.get();
        if id.is_empty() {
            return;
        }
        set_loading.set(true);
        leptos::task::spawn_local(async move {
            match history::list_versions(&id).await {
                Ok(resp) => set_versions.set(resp.versions),
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load versions: {e}").into(),
                    );
                }
            }
            set_loading.set(false);
        });
    });

    // Load and diff a selected version.
    Effect::new(move |_| {
        let Some(version) = selected_version.get() else {
            set_diff_lines.set(Vec::new());
            return;
        };
        let id = doc_id.get();
        if id.is_empty() {
            return;
        }

        leptos::task::spawn_local(async move {
            match history::get_version_content(&id, version).await {
                Ok(bytes) => {
                    // Convert yrs bytes to text content.
                    let old_text = yrs_bridge::ydoc_bytes_to_doc(&bytes)
                        .map(|doc| doc.text_content())
                        .unwrap_or_default();
                    let current = current_text.get_untracked();
                    let lines = compute_diff(&old_text, &current);
                    set_diff_lines.set(lines);
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("Failed to load version content: {e}").into(),
                    );
                }
            }
        });
    });

    view! {
        <Show when=move || visible.get()>
            <div class="history-viewer">
                <div class="history-header">"Edit History"</div>

                <div class="history-versions">
                    <Show when=move || loading.get()>
                        <div class="history-loading">"Loading..."</div>
                    </Show>
                    {move || {
                        let items = versions.get();
                        if items.is_empty() && !loading.get() {
                            view! {
                                <div class="history-empty">"No version history yet"</div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="history-list">
                                    {items.into_iter().map(|v| {
                                        let ver = v.version;
                                        let is_selected = move || selected_version.get() == Some(ver);
                                        view! {
                                            <div
                                                class="history-version-item"
                                                class:selected=is_selected
                                                on:click=move |_| {
                                                    if selected_version.get_untracked() == Some(ver) {
                                                        set_selected_version.set(None);
                                                    } else {
                                                        set_selected_version.set(Some(ver));
                                                    }
                                                }
                                            >
                                                <span class="history-version-num">{format!("v{}", v.version)}</span>
                                                <span class="history-version-time">{format_time(v.created_at)}</span>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_any()
                        }
                    }}
                </div>

                <Show when=move || !diff_lines.get().is_empty()>
                    <div class="history-diff">
                        {move || {
                            diff_lines.get().into_iter().map(|line| {
                                let class = match line.kind {
                                    DiffKind::Added => "diff-line diff-added",
                                    DiffKind::Removed => "diff-line diff-removed",
                                    DiffKind::Same => "diff-line diff-same",
                                };
                                let prefix = match line.kind {
                                    DiffKind::Added => "+ ",
                                    DiffKind::Removed => "- ",
                                    DiffKind::Same => "  ",
                                };
                                view! {
                                    <div class=class>
                                        <span class="diff-prefix">{prefix}</span>
                                        <span class="diff-text">{line.text}</span>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                </Show>
            </div>
        </Show>
    }
}

#[derive(Clone)]
struct DiffLine {
    kind: DiffKind,
    text: String,
}

#[derive(Clone, Copy, PartialEq)]
enum DiffKind {
    Added,
    Removed,
    Same,
}

/// Simple line-level diff using longest common subsequence.
fn compute_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let m = old_lines.len();
    let n = new_lines.len();

    // LCS table (O(m*n) — acceptable for document-sized texts).
    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if old_lines[i - 1] == new_lines[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to produce diff.
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old_lines[i - 1] == new_lines[j - 1] {
            result.push(DiffLine {
                kind: DiffKind::Same,
                text: old_lines[i - 1].to_string(),
            });
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            result.push(DiffLine {
                kind: DiffKind::Added,
                text: new_lines[j - 1].to_string(),
            });
            j -= 1;
        } else {
            result.push(DiffLine {
                kind: DiffKind::Removed,
                text: old_lines[i - 1].to_string(),
            });
            i -= 1;
        }
    }

    result.reverse();
    result
}

fn format_time(timestamp_usec: i64) -> String {
    let now_ms = js_sys::Date::now() as i64;
    let ts_ms = timestamp_usec / 1000;
    let diff_secs = (now_ms - ts_ms) / 1000;

    if diff_secs < 60 {
        "just now".to_string()
    } else if diff_secs < 3600 {
        format!("{}m ago", diff_secs / 60)
    } else if diff_secs < 86400 {
        format!("{}h ago", diff_secs / 3600)
    } else {
        format!("{}d ago", diff_secs / 86400)
    }
}
