# Quip content fidelity — remediation plan

**Status:** plan, not started. Written 2026-08-01 from
`.superpowers/sdd/phase2a-fidelity-audit.md` (systematic audit of 56 real staged
documents against the live parser).

**Audit verdict:** the content pass is **safe on prose, unsafe on structure**.
Nothing corrupts, nothing panics, text always survives — but outlines flatten,
numbered procedures restart at "1." on every step, a blank line appears under
every bullet, and some embedded content vanishes silently.

Tickets: #187 (nested lists) · #188 (numbered sequences) · #189 (trailing breaks)
· #190 (section anchors) · #191 (live apps) · #192 (spreadsheet formulas) ·
#193 (fixture regression net) · #194 (minor bundle).

---

## Do this first, or the rest doesn't stick

**Unit 0 — the fixture regression net (#193).**

Seven bugs so far share one root cause: **every fixture encodes a shape Quip never
emits.** `nested_lists_stay_inside_their_item()` passes against markup with *zero*
corpus occurrences while the 470-occurrence real shape is silently flattened. Every
one of these was found by a human opening a document; none by CI.

Check in five real staged documents and assert **structural counts** (headings by
level, list depth, ordered-list runs, hard breaks, section ids captured) rather than
exact strings. Promote the audit harness
(`crates/collab/tests/quip_fidelity_audit.rs`, currently throwaway).

Candidates, each chosen for what it exercises: `AeOAAAcV1hg` (nested lists) ·
`CVLAAAgSl7Q` (numbered sequences, indent vars) · `aLeAAAuK0hD` (code blocks) ·
`SSfAAALs7fy` (mentions, checklist, folder links) · `QGYAAAjicgG` (spreadsheet,
section-id density).

**Before committing:** review the content — these are real documents from a real
account. Scrub or swap anything sensitive; the citing documents look like design
notes and API docs, but that needs a deliberate look, not an assumption. Keep it to
five (the full corpus is ~2 MB). Each fixture gets a provenance comment naming its
source thread and the shapes it covers — and must not claim "verbatim" unless it is
byte-exact.

Doing this first means every unit below is verified against reality rather than
against someone's mental model of Quip's HTML.

---

## Unit 1 — List structure (#187 + #188)

**Fix together.** They are the same markup problem seen from two angles: Quip
expresses both nesting *and* numbered-sequence continuation through sibling
`data-section-style` sections, so a fix for one that ignores the other will fight it.

- **#187:** a `<ul>`/`<ol>` appearing as a direct child of a list must be re-parented
  into the **preceding `<li>`**, not hoisted. The `<li>` carrying the nested list is
  marked `class='parent'`; `style="--indent0: N"` on the section div is a second,
  independent signal. Prefer whichever the corpus supports consistently.
- **#188:** merge consecutive `'6'` sections into one ordered list, treating an
  interleaved `'5'` section as the preceding item's sub-content rather than a
  terminator.

**Biggest blast radius in the plan** — 470 nesting sites across 24 documents, and
565 bullet lists must keep working. The audit confirmed bullet lists, table
structure and marks round-trip correctly today; this unit must not disturb them.

**Verify:** source depth == output depth on every corpus document; a 7-step
procedure produces one list numbered 1–7; no bullet list gains or loses items.

---

## Unit 2 — Trailing hard breaks (#189)

Drop the `<br/>` that terminates every `<li>`/`<td>`/`<th>` — Quip's line
terminator, not authored content. **5,483 sites, 47 of 56 documents**; the most
visible defect in the audit.

**The trap:** a `<br/>` *mid*-cell is authored content and must survive. 6,657
non-`<pre>` hard breaks round-trip correctly today. Distinguish by **position**, not
presence.

Leave a comment tying this to #184's opposite rule — inside `<pre>`, a `<br>` had to
*become* a newline. Same element, opposite treatment, different container. Someone
will later try to unify them into one wrong rule.

Small and independent; can run in parallel with Unit 1.

---

## Unit 3 — Section anchors (#190)

Capture `section_id` on every block type that carries one — today only `Para` and
`Heading` do, so **2,202 of 14,439** ids are recorded.

**Sequence before Phase 2b.** The `SECMAP#` rows exist so the link back-patch can
resolve an anchor; building 2b first would build it against a map that is 85% empty,
and links targeting a list item, cell or image could never resolve.

Note a ~7× increase in captured ids is a **volume** change as much as a parser one —
`SECMAP#` is already chunked against DynamoDB's 400 KB item cap (`SECMAP_CHUNK_ENTRIES`),
so re-check that chunking rather than assuming it absorbs the increase.

---

## Unit 4 — Silent loss (#191 live apps, #192 spreadsheet formulas)

The only two findings where content **disappears with no signal**.

Both have their data present in the export — a Kanban board's cards ride in
`data-live-app-payload`, formulas in a `formula` attribute the sanitizer strips — so
both are recoverable, unlike genuine Quip omissions (dates are client-rendered and
simply absent).

**Establish the floor first, cheaply:** whatever the eventual representation, a
dropped board or formula must write a `REPORT` note so the loss is *discoverable*.
The report row and note mechanism already exist. Do this before the richer mapping —
it converts silent loss into known loss in an afternoon.

Then decide representation per ticket. Both have native OgreNotes targets (the Kanban
block; the spreadsheet formula engine), so high fidelity is possible — but #192 needs
a real check that Quip's formula dialect maps onto ours before committing to a
translation, with a documented fallback (import as literal text + a note) for
formulas that don't.

---

## Unit 5 — Minor bundle (#194)

Slides, `<details>`, column layouts, comment anchors, image dimensions. All
**content-preserving** — presentation or metadata only, nothing lost.

One worth pulling forward: **comment anchors (`annotationid`)**. Phase 4 comments
will have nothing to anchor to without them, and re-importing later to recover
anchors is expensive. Cheap to capture now even if unused until then.

---

## Order

1. **Unit 0** — the net. Everything else is verified against it.
2. **Unit 1** — largest structural win; do it while the corpus knowledge is fresh.
3. **Unit 2** — parallel with Unit 1 (different code path, no overlap).
4. **Unit 3** — before Phase 2b.
5. **Unit 4** — report notes immediately; representation deliberately.
6. **Unit 5** — opportunistic, except `annotationid` before Phase 4.

**Demo gate:** re-import the same account and diff against the audit's structural
census. Source depth equals output depth, numbered runs are contiguous, hard-break
count drops by ~5,483, captured section ids approach the count of **distinct** ids (materially lower than 14,439 — Quip repeats an item's id verbatim onto its inner `<span>`, so `source_id_count` counts elements, not anchors; see #190), and the Kanban and
spreadsheet documents either import their content or say plainly that they didn't.

---

## Deliberately not in this plan

- **#170 folder structure** — has its own plan (`2026-08-01-quip-folder-structure.md`).
- **#183 checked-state** — blocked on measurement, not code: the corpus has one
  checklist and every item is unchecked, so the checked markup is unknown. Needs a
  Quip document with a ticked box before it can be written.
- **#179 invisible people**, **#178 `get_by_email` scan**, **#155/#156** — importer
  robustness and identity, not content fidelity.
- **Phase 2b link back-patch** — the next feature phase. Unit 3 is its precondition.
