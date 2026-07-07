# Collaborative undo/anchor manual smoke test

A two-tab manual check that local **undo/redo** and **comment anchors**
survive a *concurrent remote edit* without corruption — the behavior
generalized from #151 (undo after an edit that shifts earlier positions).

## Why this is manual

The remap is **client-side only**: when a remote/concurrent edit swaps the
document under the editor, the browser recomputes a char-precise position
map (`step_map_for_doc_swap`) and carries the recorded undo/redo stack
(`HistoryPlugin::remap_through`) and comment anchors
(`remap_block_anchor`) into the new coordinate space. There is no
server-side path to exercise it, so it can't be checked via API calls.

It also can't be driven headlessly against a **deployed** stack: those run
with `DEV_MODE=false` (any stack with a `DOMAIN_NAME`), so the scriptable
`dev-login` returns 404 and a headless browser has no way past OAuth.

The *logic* is covered by automated tests in the build — run those first;
this checklist only confirms the browser wiring:

```bash
cd frontend
cargo test --lib swap_remap        # step_map_for_doc_swap (enumerated + property)
cargo test --lib editor::plugins   # undo_after_remote_edit_uses_synthesized_map
cargo test --lib remap_block_anchor
```

## When to run

Before a release, and after any change to: `editor/plugins.rs` (history),
`editor/transform.rs` (`step_map_for_doc_swap` / `StepMap`),
`editor/state.rs` (`remap_block_anchor` / selection remap),
`editor/yrs_bridge.rs`, or the remote-update sinks in
`components/editor_component.rs` / `pages/document.rs`.

## Setup

- Be logged in (real OAuth on a deployed stack, or a local `DEV_MODE` stack).
- Open the **same document in two browser tabs**, A and B. Same account is
  fine — each tab is its own collaborative session. Confirm they sync: type
  in A, see it appear in B.

## Test 1 — undo after a concurrent remote edit

1. **Tab A:** click at the **end** of the document, type ` ZZZ`. Confirm
   **Tab B** shows ` ZZZ` (proves they're syncing; records an undo entry in A).
2. **Tab B:** click near the **start** of the document, type ` XXX `.
   Confirm **Tab A** now shows ` XXX ` near the start **and** ` ZZZ` at the
   end. *(This is the remote edit shifting positions ahead of A's recorded
   undo — the #151-remote trigger.)*
3. **Tab A:** press **Ctrl/Cmd-Z** once.

| Result | Meaning |
|--------|---------|
| `ZZZ` gone, `XXX` and all original text intact, no stray characters | ✅ **Pass** — undo tracked `ZZZ` to its shifted offset and removed exactly it. |
| Mangled text near the end (`XX ZZZ`, `XXXZZ`), a leftover `Z`, the wrong characters removed, or undo does nothing / is stuck | ❌ **Fail** — undo applied at a stale offset (the #151 corruption). |

The clean result is the tell: pre-fix, the undo step landed at the
pre-shift offset and could not have produced an intact `XXX` + original.

## Test 2 — comment anchor follows a concurrent remote edit

1. **Tab A:** select a word or phrase mid-document and add a comment. A
   highlight appears over exactly that text.
2. **Tab B:** type some text **before** that phrase (earlier in the same
   block, or in an earlier block).
3. **Tab A:** watch the highlight.

| Result | Meaning |
|--------|---------|
| The highlight stays on the **same words** (it doesn't slide off onto neighboring text) | ✅ **Pass** — the anchor was remapped through the swap. |
| The highlight visibly slides left/right onto the wrong words and stays there | ❌ **Fail** — anchor not remapped. |

Note: anchors are block-relative, so an edit in *another* block must not
move the highlight at all. A same-block edit *before* the anchor shifts it;
the optimistic remap keeps it correct without waiting for the server
refetch.

## If it fails

Capture both tabs (screenshots), the document `doc_id`, and the exact key
sequence, then follow `reproduce-editor-failure.md` (replay the edit
history) to localize the failing update. The undo/anchor remap is pure and
unit-tested, so a live-only failure points at the *wiring* — the remote
sinks in `editor_component.rs` (undo) or `pages/document.rs` (anchors) — or
at the `step_map_for_doc_swap` precision fallback under many disjoint edits
in one sync window.
