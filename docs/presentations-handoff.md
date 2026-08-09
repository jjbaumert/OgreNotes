# Presentations — handoff

**As of 2026-08-02.** Everything described as shipped is merged to `main` **and deployed** to
`https://test.ogrenote.com`. Nothing is in flight.

Design of record: `design/presentations.md`. Plans:
`docs/superpowers/plans/2026-08-01-presentations-p1-deck-foundation.md`,
`docs/superpowers/plans/2026-08-02-presentations-p2-presenting.md`,
spec `docs/superpowers/specs/2026-08-01-deck-frame-blocks-design.md`.

---

## 1. What exists today

| Area | State |
|---|---|
| Deck document type (`DocType::Presentation`), `Slide`/`Frame` schema, validators, degraded HTML/MD export | shipped (PR #182) |
| Canvas editor: slide strip, drag/resize/snap frames, keymap, in-frame editing, frame comments, 6 themes, 4 layout presets | shipped (PR #182) |
| Full editor block set inside frames — images, mermaid, calendar, kanban, tables, code, quotes, task lists, embeds | shipped (PR #207) |
| Present mode (`/d/:id/present`), presenter view (`?presenter=1`) with notes + next-slide + timer | shipped (PR #215) |
| Live follow-the-presenter over WS awareness | shipped (PR #215), hardened (PR #224) |
| PDF slide export (landscape, one page per slide, themed) + the export menu entry | shipped (PR #215) |
| Mobile view/present + swipe navigation | shipped (PR #215) |
| Cross-instance awareness, per-session presenter identity, present-mode reconnect | shipped (PR #224) |
| Slide-strip delete/duplicate buttons | fixed (PR #234) |

CI coverage: `deck-basics`, `deck-blocks`, `deck-present` doctor scenarios run in the nightly
`playwright.yml` sweep. `frontend/src/presentation/` is lib-visible so its ~38 unit tests gate
`cargo test --lib`.

---

## 2. Next work, in the agreed order

### 2.1 — #209 Present-after-edit loads a corrupted deck ⚠️ **needs a decision before starting**

Clicking **Present** immediately after an edit races the editor's WebSocket teardown; the present
page's REST `get_content` can observe a partially-flushed document. ~50% reproducible. The
`deck-present` doctor scenario works around it with a `page.reload()`; **the product path is still
affected**.

Pick one before implementing:

1. **Read the in-memory editor state when navigating from the editor** (`doc_to_ydoc_bytes`) —
   the pattern this repo already prefers for read-after-edit flows.
2. **Gate the present-mode fetch** on the editor's WS teardown/flush completing.
3. **Take content from present mode's own WS sync** instead of the REST fetch.

Related prior art: the "get_content WS race" note in project memory.

### 2.2 — #195 Typing over a Ctrl+A selection relocates the first character

`PROBE TEXT` → `ROBE TEXTP`. Deterministic, reproduces long after mount, in **every** editor surface
(document, deck frame, presumably table cells). Not presentations-specific. Suspected area:
caret/selection mapping after a replace-selection transaction — the new caret resolves to the
replacement's *start* rather than its end. Distinct from closed #92 (mount-time input race).

### 2.3 — P3: feedback-prompt block

The last planned phase. `FeedbackPrompt` (container: `question`, `visibility`) +
`FeedbackResponse` (leaf atom: author, text, created-at) as a live-app block via
`design/live-app-blocks.md` — usable in decks **and** documents, so it ships value independently.
Design section: `design/presentations.md` → "Feedback-prompt block". Note the documented
limitation: `author-only` visibility is render-time only, not access control.

---

## 3. Tracked follow-ups (filed, none blocking)

| Issue | What |
|---|---|
| #227 | Duplicate "Follow ⟨name⟩" pills when a presenter has two windows. Naive dedup-by-user is **wrong** — it re-collapses the presenter's own windows, defeating #211. Needs third-party-only dedup. |
| #228 | PDF frame text isn't clipped to its frame box; long content runs off the page. **Silent content loss** — the canvas clips, the PDF doesn't. |
| #229 | Swipe double-advance guard is code-verified only. Synthetic `TouchEvent`s bypass native touch→click synthesis, so it needs a real-device pass. |
| #235 | `duplicate_dialog.rs:78` "reactive value already disposed" panic during PDF export. Consistently reproducible; likely latent and newly *exposed* by P2's export menu entry, not created by it. Currently the only thing keeping `deck-present` from green. |

---

## 4. Deferred by design

In `design/presentations.md`'s stated revisit order: polls with live tallies → engagement analytics
(per-viewer, per-slide) → `.pptx` import/export → slide transitions and build animations → spatial
(x/y-pinned) comments → corporate/custom themes via the admin console → mobile deck *editing* →
public presenter-session API.

Two soft items: the six theme palettes are implementer-chosen and want a design eye; and
`design/presentations.md` has two stale details — it says `/doc/{id}/present` (real route is
`d/:id/present`) and describes the backend themes table differently than it landed.

---

## 5. Gotchas worth knowing before touching this code

These each cost real debugging time; several were only found by running a browser.

**Leptos 0.7**
- `attr:data-x=…` on a native element **silently renders nothing**. Use plain `data-x=…`. This made
  every frame lose its block-id and broke in-frame editing entirely.
- Signals have **no equality gate on `.set()`** — polling a value into a signal re-fires every
  dependent on every tick. This produced an unbounded awareness re-broadcast (~3 frames/sec/viewer).
- Effects created **inside render closures** are disposed with those transient owners. Imperative
  DOM mounting (NodeRef + Effect, NodeRef + rAF) silently never runs there; serialize to `inner_html`
  instead.
- `<Show>`/`<For>` closures require `Send` — `Rc` can't satisfy them, `StoredValue` can.
- `<For>` never re-invokes `children` on reorder, so any index baked into a row goes stale. Resolve
  positions by `block_id` at event time.

**DOM**
- A parent that calls `set_pointer_capture()` on `pointerdown` **retargets the follow-up click to
  itself** — child buttons need `on:pointerdown` → `stop_propagation()`, not just `on:click`. This
  is what made the slide delete/duplicate buttons inert.

**This repo**
- CI runs `cargo test --lib` for the frontend. `components/` and `pages/` are **binary-only** — tests
  there never gate CI. Put testable logic in a lib module (`frontend/src/presentation/` is one).
- **Restart the API after every `trunk build`.** CSP inline-script hashes are computed at server
  startup; a stale server serves a CSP that blocks the new bootstrap and nothing mounts.
- Server-side block validators are **accept-verbatim-or-reject; never rewrite a value** — the write
  gate's canonicalization diff treats any rewrite as a violation. Readers clamp instead.
- Never bake placeholder prose into document content; render it as a hint. Baked placeholders meant
  users had to delete "Click to add heading" before typing.
- CI's playwright job builds the API **without the `pdf` feature** by design. The `deck-present`
  scenario treats the backend's "not compiled into this build" 400 as a visibly-logged skip.
- Route ordering: `/d/:id/present` is declared **flat and before** the `ParentRoute`, or `present` is
  swallowed as a `:slug`. (leptos_router matches sequentially — this is a hard guarantee, not luck.)
- The deck canvas is capped `max-width: 960px` for the editor; present mode must override it.
- `#212`'s cross-instance fix is **unobservable on a single-task stack** (test1- runs
  `desiredCount: 1`). Not a failed fix — its proof is the cross-instance integration tests.

**Review lesson.** Three separate bugs this cycle lived *only* in the interaction between two
individually-correct pieces (drag capture vs. button click; reconnect vs. broadcast dedup; stale
connection vs. session reuse). Per-change review missed all three; a combined/whole-branch review
caught them. Review deck interaction code at the interaction level.

---

## 6. Local verification recipe

Follow `.claude/skills/verify/SKILL.md`. Short version: `docker compose up -d`, export the dev env
(AWS shims → DynamoDB Local + MinIO, `DEV_MODE=true`, a unique `DYNAMODB_TABLE_PREFIX`,
`API_PORT=3100`), `cargo run --bin setup_dev -p ogrenotes-api`, `cd frontend && trunk build`, then
serve with `FRONTEND_DIST=$PWD/frontend/dist ./target/debug/ogrenotes-api`.

Doctor scenarios: `cd scripts/frontend-doctor && node doctor.js --scenario deck-basics --base-url
http://127.0.0.1:3100 --out /tmp/out` (also `deck-blocks`, `deck-present`). Ad-hoc probes go in that
same directory (for its `node_modules`) and authenticate by forwarding the dev-login `Set-Cookie`
headers into the browser context — the app ignores localStorage tokens.

Note: the deployed test stack runs `DEV_MODE=false`, so the doctor **cannot** authenticate against it
without a redeploy. Verify against a local stack.
