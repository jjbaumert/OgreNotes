# Document & Anchor Mentions — design

**Date:** 2026-07-23
**Area:** frontend editor (`frontend/`, Leptos/WASM — outside the cargo
workspace) + backend (`crates/api`, `crates/collab`) + both schema files
**Status:** approved design, ready for implementation plan

## Provenance

Derived from a spec amendment ("Document Mentions on Paste — replaces
item 5, extends Stage 4") whose base document is not in the repository.
This design reconstructs the full feature — anchor mentions AND document
mentions — from that amendment plus the decisions recorded below. Where
the amendment says "as previously specified", the behavior is specified
here from scratch.

## Goal

When an OgreNotes URL is pasted into a document, resolve it to the most
specific entity it addresses and render a live mention element:

- **Anchor mention** — URL carries a block fragment that resolves:
  navy anchor icon + live snippet of the target block (cross-document
  targets also show the target document title).
- **Document mention** — URL has no fragment, or the fragment dangles
  but the document resolves: document icon + live document title.
- **Neither resolves / no access** — leave the paste as a plain
  markdown URL, untouched.

Both mention types are one inline atomic element class with a shared
navigation, refresh, and degradation contract.

## Decisions of record

| Decision | Choice | Rationale |
|---|---|---|
| Element architecture | One node type (`DocMention`) with attrs, not two node types | Amendment frames them as one family; degradation transitions become state changes, not CRDT structural edits; equivalence rule falls out for free |
| Block-URL producer | Ship "Copy Link to Block" in this feature | Nothing produces block URLs today; without a producer, anchor mentions can never fire |
| Activation target | Open at top (anchor mentions: scroll to block) | OgreNotes tracks no last-cursor/last-position anywhere (verified); adding per-user position persistence is a separate feature |
| Render state persistence | NOT persisted; per-session | Access is per-viewer — persisting one viewer's `missing` into the shared CRDT would be wrong and would leak access state across users |
| Refresh mechanism | Batch resolve on paste / document open; no live WS subscription | Amendment: "refreshed on render/open" |

## 1. Block-link URL convention (net-new)

- Block URL: **`/d/:id#b=<blockId>`**. Hash-based: no backend routing
  changes; composes with the `/d/:id/:slug` variant; the `b=` key
  namespaces the fragment (the settings page already uses bare hashes
  for tabs — different page, no collision).
- **Producer:** "Copy Link to Block" action in the editor context menu
  (`editor_context_menu.rs`), copying `<origin>/d/<docId>#b=<blockId>`
  for the right-clicked block. Blocks already carry stable `blockId`
  attrs (`model.rs` `needs_block_id`) and `data-block-id` in the DOM.
- **Fragment consumption on document load:** the document page parses
  `#b=`, scrolls via the existing `scroll_to_block(block_id,
  fallback_index)` (`components/dom_position.rs`), and briefly
  highlights the target block. If the block no longer exists, open at
  top and show a dismissible "linked section no longer exists" notice.
- i18n: the notice and the copy action label are Fluent keys in all six
  catalogs. (Amended 2026-07-23: copy is silent — no confirmation toast —
  matching the codebase's established silent-copy convention
  (`copy_doc_link`, share dialog). Decision recorded at S1 review.)

## 2. The `DocMention` element

One new node type in **both** schemas (`crates/collab/src/schema.rs`
canonical, `frontend/src/editor/schema.rs` mirror — the CI cross-schema
test must be extended). Modeled on the existing user `Mention` inline
leaf atom (`model.rs`: `is_leaf`, `is_atom`, `is_inline`,
`data-atom-size` on the DOM wrapper).

**Persisted attrs:**

| Attr | Type | Meaning |
|---|---|---|
| `url` | string | The original pasted URL. Never mutated; source of truth for "Copy Original URL" and serialization |
| `doc_id` | string | Target document id (parsed from the URL) |
| `block_id` | string? | Present ⇒ the mention was created as an anchor mention |
| `title` | string | Target document title as of the last successful resolve |
| `snippet` | string? | Target block snippet as of the last successful resolve (anchor only) |

**Ephemeral per-viewer render state** (component state, never written
to the document): `live` | `dangling-block` (doc resolves, block
doesn't) | `missing` (doc gone or access revoked — indistinguishable by
design). Computed from the latest resolve response each session.

**Cache refresh policy:** `title`/`snippet` attrs are updated via a
normal editing transaction when a resolve succeeds during an editing
session. Read-only viewers never write; they overlay the fresh resolve
result in memory over the cached attr. Offline/degraded renders fall
back to the cached attrs.

**Element behavior (shared contract):**

- Not directly editable; Delete/Backspace removes the whole element
  (Quip's convention for dynamic mentions). The full atom interaction
  sweep applies at implementation time: Enter, Delete, Backspace, Tab,
  input rules, caret affinity, paste-adjacent — same checklist the
  Embed/code-block atoms required.
- Activation (click):
  - anchor mention → navigate to `/d/<doc_id>#b=<block_id>` (in-app
    navigation; scroll + highlight per §1)
  - document mention → navigate to `/d/<doc_id>`, top
  - dangling-block → navigate to `/d/<doc_id>`, top, with the "linked
    section no longer exists" notice
  - missing → no navigation
- Element context menu: **"Copy Original URL"** (always, in every
  state) and **"Convert to Plain Link"** (replaces the element with a
  markdown link `[cached title](url)`).

## 3. Rendering by kind × state

| | `live` | `dangling-block` | `missing` |
|---|---|---|---|
| **anchor** (`block_id` set) | Navy anchor icon + snippet; cross-document targets append the target doc title | Renders as a document mention + subtle lost-block indicator (tooltip: "linked section no longer exists") | Grayed "missing document"; no navigation |
| **document** (no `block_id`) | Document icon + title | — (no fragment to dangle) | Grayed "missing document"; no navigation |

"Cross-document" = the mention's `doc_id` differs from the containing
document's id. Same-document anchor mentions show the snippet only.
All visible strings and tooltips are Fluent keys in all six catalogs.

## 4. Resolution service

**`POST /api/v1/mentions/resolve`** (authenticated, batch):

```jsonc
// request
{ "targets": [ { "docId": "…", "blockId": "…" /* optional */ } ] }
// response — same order as request
{ "results": [
  { "status": "ok", "title": "…", "blockFound": true,  "snippet": "…" },
  { "status": "ok", "title": "…", "blockFound": false },
  { "status": "notFound" }
] }
```

- **Access gating happens before any lookup.** A target the caller
  cannot read returns `notFound`, byte-identical to a nonexistent
  document — titles are never fetched, cached, or leaked for
  inaccessible targets, and existence itself is not disclosed.
  A test must assert response-body equality between the no-access and
  nonexistent cases.
- Snippet: first ~120 characters of the target block's plain-text
  content, server-extracted from the canonical document snapshot in
  storage. Known limitation: a resolve may trail in-flight WS edits by
  the persistence delay; acceptable for snippet display.
- Read-only endpoint: no `SecurityAudit` row (that pattern covers
  writes to identity/sharing/destructive state).
- Callers: paste-time (single target), document open (one batch for
  every mention in the doc). No per-element polling, no WS
  subscription. Repeated targets into the same document within one batch
  reuse a single per-request doc load (S2 implementation note).
- Contract note for the S3/S4 element: `blockFound` is the anchor's
  liveness signal — an existing-but-empty block yields `blockFound:
  true` with an empty-string snippet, which must render as a live
  anchor, never be conflated with "no snippet"/dangling.

## 5. Paste conversion

In the editor's `on_paste` (before generic HTML/markdown handling):
when the paste content is a single same-origin OgreNotes document URL
(`/d/:id`, optional `/:slug`, optional `#b=<blockId>`):

1. Insert the plain markdown URL immediately (no paste latency).
2. Resolve asynchronously, then apply the a/b/c ladder:
   - a. `ok` + `blockFound` (fragment present) → replace with anchor
     mention.
   - b. `ok` + no fragment, or fragment present but `blockFound:
     false` → replace with document mention (the dangling case keeps
     `block_id` in attrs so the lost-block indicator and notice work).
   - c. `notFound` / network error → leave the plain URL untouched.
     Silently — surfacing an error would distinguish no-access from
     nonexistent.
3. **Single-undo contract:** the mention replacement must coalesce
   with the paste in the undo manager (yrs `UndoManager`
   capture-window grouping) so one undo restores the raw URL. This is
   a hard requirement; if grouping cannot be made reliable, the
   feature must fall back to offering conversion instead of
   auto-converting.

Multi-URL pastes and URLs embedded in larger pasted content are NOT
converted (plain links) — conversion applies to a lone-URL paste only.
"Convert to Plain Link" (§2) is the permanent opt-out for any mention.

## 6. Equivalence rule and future @-mention path

The element is constructed from `{doc_id, block_id?, url, title,
snippet?}` with no paste-specific state. A future @-mention
autocomplete (the existing `at_menu.rs` is the natural host) inserts
the identical `DocMention` node from a picker-resolved
`doc_id`+`title`, synthesizing `url = /d/<doc_id>`. No new construct;
no schema change. Out of scope for this feature.

## 7. Serialization and clipboard

- **Copy-out HTML:** `<a class="doc-mention" data-doc-id="…"
  data-block-id="…" href="<url>"><title></a>` — round-trips back to a
  `DocMention` on re-paste into OgreNotes (`clipboard.rs`, same
  pattern as the user-mention span); renders as a plain link anywhere
  else.
- **Markdown / export (md, docx, pdf):**
  `[title-at-time-of-serialization](url)` — the layering rule shared
  with the rest of the atom family.

## 8. Staging

| Stage | Contents | Standalone value |
|---|---|---|
| **S1** | `#b=` convention; "Copy Link to Block"; fragment consumption on doc load (scroll + highlight + lost-block notice) | Shareable block links |
| **S2** | Resolve endpoint + access gating + server-side resolution-matrix tests (incl. no-access ≡ nonexistent byte-equality) | API ready; independently testable |
| **S3** | `DocMention` in both schemas; render states; atom interaction sweep; element context menu; clipboard/serialization round-trip; cross-schema CI test extension | Element insertable/renderable (no paste path yet) |
| **S4** | Paste conversion + undo coalescing; convert-to-plain-link; refresh-on-open; full resolution matrix tests | Feature complete |

## 9. Testing

- **Backend (S2):** resolve matrix — {doc ok, missing, no access} ×
  {no fragment, fragment resolves, fragment dangling}; the
  no-access/nonexistent byte-equality assertion; snippet extraction
  truncation/unicode safety.
- **Frontend native:** URL parse matrix (`/d/:id`, `/:slug`, `#b=`,
  foreign origins, malformed fragments); serialization round-trips
  (HTML out→in, markdown out); attr defaults.
- **Editor behavior (S3/S4):** atom interaction sweep; single-undo
  restores raw URL; convert-to-plain-link; degraded-state rendering
  from mocked resolve responses. (No Leptos component-mount harness
  exists — behavior tests ride the native model/serialization layers
  plus manual verification via the doctor scenario flow.)
- **Full matrix (S4):** {fragment present, absent, dangling} ×
  {doc resolves, missing, no access} — each cell's expected element
  (anchor mention / document mention / document mention + indicator /
  plain URL) per the a/b/c ladder.

## Out of scope

- @-mention autocomplete insertion (path designed for, not built).
- Per-user last-position tracking (activation always opens at top).
- Live WS-pushed snippet/title updates (refresh is render/open only).
- User @-mentions (existing, unrelated feature) — unchanged.
- Notifying target-document owners that they've been mentioned.
