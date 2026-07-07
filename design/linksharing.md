# Link Sharing — Design

> Status: **draft for review** (2026-06-02, v2). This doc describes
> OgreNotes' link-sharing behavior and the deliberate scope
> decisions behind it.
>
> **v2 scope change.** v1 of this doc proposed a three-tier audience
> ladder (workspace → any signed-in user → public/anonymous) with
> rotatable capability tokens, an anonymous-access path, and a "join"
> discoverability model. After the §0 security review and a correction
> of what *workspace* means, that scope is **cut**: OgreNotes link
> sharing is **workspace-internal only**, and **no unauthenticated
> access is permitted**. The sections below reflect the reduced scope;
> §0 records what was dropped and why so the decision is auditable.

## 0. Scope decision & what changed from v1

Two constraints now bound this feature:

1. **No unauthenticated access.** A document is never reachable by a
   caller without a valid session. This kills the `public`/anonymous
   tier outright.
2. **Caller must be a member of the document's workspace.** Being any
   signed-in OgreNotes user is *not* sufficient — you must belong to the
   tenant that owns the doc. This kills the cross-tenant
   `authenticated` tier.

What remains is the single tier OgreNotes already half-implements:
**a link grants the chosen access level to members of the document's
workspace.** Consequently, "link sharing" here is not a secret,
shareable capability — it is a **per-document workspace-visibility
toggle**, and the "link" is simply the document's normal URL.

Dropped from v1 (and *why*):

| Dropped | Why it's unnecessary under the new scope |
|---------|------------------------------------------|
| Opaque rotatable **token** + `/d/{token}` URL + pointer item + resolve endpoint | Access is gated by *workspace membership*, not by possession of a secret URL. A token-capability adds nothing when holding the URL doesn't grant access. The doc URL suffices; "off" is the revocation. |
| `authenticated` and `public` **audiences** | Forbidden by the two constraints above. Only `workspace` remains, so `audience` is constant and the field is removed. |
| **Anonymous** principal, `OptionalAuth` extractor, anonymous WS handshake, synthetic guest identity | No unauthenticated access. The existing `AuthUser`/JWT boundary and the existing WS-token flow are unchanged. |
| **Join / leave** as a *hard search gate* (old §11) | The join-gate existed to fence off the *wide* audiences, which are gone. With only the workspace audience we keep docs **auto-discoverable** (a hard gate breaks the company-link discovery promise), and handle large-org signal-to-noise via **ranking demotion** instead — see §9. No join step is needed. |
| `public_links_enabled` site setting, admin public-doc list, external allowlist | Nothing public to gate or enumerate. |
| Security findings **H1–H4, M1–M4** from §0 review | All concerned the anonymous/token surface, which no longer exists. |

The feature is now small enough that its real value is: a clean
mode/sub-option model, a **copyable doc URL** in the UI, enforcement of
the previously-**inert** external flag's removal, and closing a real
**audit-logging bug**.

## 1. Terminology: what "workspace" means

A **workspace** is the **tenant / organization** that owns identity and
enterprise policy. It is
**not** a folder or a folder subtree (folders are a separate
organization axis rooted at the user's `home_folder_id`).

- Model: `Workspace { workspace_id, name, owner_id, mfa_required, … }`,
  `crates/storage/src/models/workspace.rs`.
- It carries org-level controls: **MFA enforcement** (`mfa_required`),
  **SAML SSO**, and **SCIM** provisioning all hang off the workspace
  (see `crates/api/src/routes/workspaces.rs`).
- Membership: `WorkspaceMember { workspace_id, user_id, role }`, with
  `role ∈ {Owner, Admin, Member}`.
- **Every user has a `default_workspace_id`** (new users at creation;
  legacy users were backfilled with a *"Personal Workspace"* by
  `crates/api/src/bin/backfill_workspaces.rs`).
- **Every document inherits its owner's default workspace** at creation
  and is stored as `DocumentMeta.workspace_id`.

**Reach caveat.** Because a doc's workspace defaults to its owner's
default workspace, the reach of a workspace link depends on whether that
workspace is a real multi-person org or a personal one:

- **Enterprise tenant** (SAML/SCIM, many members) → a link = "everyone
  in the company can open this." Meaningful.
- **Solo user's "Personal Workspace"** → a link reaches only the owner
  and anyone explicitly added to that personal workspace. It behaves
  almost like "private."

## 2. Product decisions (settled)

| Decision | Resolution |
|----------|------------|
| **Who can a link reach?** | **Members of the document's workspace only.** No external (cross-tenant) users, no anonymous/public access. |
| **What is the "link"?** | The document's existing URL. There is **no secret token**; access is membership-gated, so the URL need not be unguessable. |
| **Revocation** | Setting the mode to off (link sharing disabled) immediately removes link-derived access for all workspace members. There is no per-URL revocation because there is no URL-capability. |
| **Permission levels a link grants** | `view` or `edit` (or off). Never `own`. `comment` is expressed as `view` + the `allow_comments` sub-option. |
| **View sub-options** | All four ship: allow comments, show history/diffs, show conversation, request edit access. |
| **Search discoverability** | Link-shared docs remain auto-discoverable in workspace members' search — unchanged from today (§9). |

## 3. Model

A link is a per-document setting with a **mode** and, for read-only
links, a set of **view sub-options**.

### 3.1 Mode

| Mode | Effective `AccessLevel` for workspace members | Notes |
|------|-----------------------------------------------|-------|
| off (`None`) | none from the link | Disabled. Explicit members/folder access still apply. |
| `view` | `View` | Read-only, refined by view sub-options. |
| `edit` | `Edit` | Full editing. Sub-options are irrelevant (Edit ⊃ them). |

### 3.2 View sub-options (apply only when mode = `view`)

These layer *capabilities* on top of the base `View` the link grants, so
a read-only link can selectively permit interaction without granting
full Edit. They are enforced at the relevant feature endpoints
(§5.3), not folded into `AccessLevel`.

| Option | Effect (for a workspace member reaching the doc via a `view` link) | Backed by |
|--------|--------------------------------------------------------------------|-----------|
| `allow_comments` | May add inline/pane comments. | `comments` route |
| `show_history` | May see edit history / diffs. | `history.rs` |
| `show_conversation` | May see the activity/conversation pane. | `chat.rs` / activity |
| `allow_request_access` | Sees a "Request edit access" affordance that notifies the owner. | new flow (§5.4) |

Defaults on a new `view` link: all **off** — the owner opts each in.

## 4. Capability decisions

| Capability | OgreNotes decision |
|------------|--------------------|
| Company link mode `edit`/`view`/`none` | ✅ Implemented as the workspace-visibility mode. |
| "Anyone in the company with the link" | ✅ Exactly the workspace-member audience. |
| Shareable link URL `thread["link"]` | The plain doc URL (no separate token — access is membership-gated). |
| `allow_access_outside_domain` / external sharing | ❌ **Deliberately not supported.** Policy: no cross-tenant or external access via links. |
| "Public web page" / anonymous access | ❌ **Deliberately not supported.** Policy: no unauthenticated access. |
| View sub-options (conversation, diffs, comments, request-to-edit; "allow new messages") | ✅ `show_conversation` (read pane), `show_history`, `allow_comments` (post), `allow_request_access`. "Allow new messages" maps to `allow_comments` — posting to a doc's conversation is the same single message stream as commenting (§12 #1). |
| `edit-share-link-settings` endpoint | ✅ `PATCH /documents/{id}/link-settings` (extended, §5.2). |
| Admin override of link settings | ✅ `PATCH /admin/documents/{id}/link-settings` (§5.5). |
| Disable public sharing site-wide / external allowlist / per-company revocation | ❌ N/A — there is no external/public sharing to govern. |

## 5. Data model & endpoints

### 5.1 `DocumentMeta` change (deliberate schema change — flagged)

```rust
// crates/storage/src/models/document.rs
pub struct DocumentMeta {
    // …
    /// Link-sharing mode (None = disabled). Unchanged from today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_sharing_mode: Option<LinkSharingMode>,   // Edit | View | None
    /// View-mode sub-options. Ignored unless mode = View.
    #[serde(default)]
    pub link_view_options: ViewOptions,
    // REMOVED: link_sharing_allow_external
}

// crates/storage/src/models/mod.rs
#[derive(Default, Serialize, Deserialize, …)]
#[serde(rename_all = "camelCase")]
pub struct ViewOptions {
    #[serde(default)] pub allow_comments: bool,
    #[serde(default)] pub show_history: bool,
    #[serde(default)] pub show_conversation: bool,
    #[serde(default)] pub allow_request_access: bool,
}
```

> **Breaking-change note (per CLAUDE.md).** Removing
> `link_sharing_allow_external` changes the serialized `DocumentMeta`
> wire shape and the DynamoDB item attributes. It is removed
> deliberately — it has always been inert (read by no access decision)
> and the new scope forbids what it purported to enable. No value is
> migrated. Adding `link_view_options` is backward-compatible via serde
> defaults. (`LinkSharingMode::None` is now redundant with
> `Option::None`; collapsing the enum to `{Edit, View}` is an optional
> follow-up cleanup, not required.)

### 5.1.1 Migration / backfill edge

Link sharing requires the doc to have a workspace: the access check only
applies the link when `meta.workspace_id` is `Some` *and* the caller is
a member of that workspace (`evaluate_doc_access`,
`crates/api/src/routes/documents.rs`). Therefore:

- Docs with `workspace_id = None` (legacy/test rows exist —
  documents.rs creation paths and tests) have an **inert** link: turning
  it on grants nobody anything.
- `backfill_workspaces.rs` already assigns every doc its owner's default
  workspace; it must run before/with this feature so workspace links are
  effective. Any doc still left with `workspace_id = None` (e.g. an
  "orphaned" doc whose owner has no default workspace) must surface in
  the UI as **"link sharing unavailable until this document has a
  workspace,"** rather than silently appearing on but granting nothing.

### 5.2 Link-settings endpoints (extend existing)

All require an authenticated `AuthUser`; no new auth surface.

| Method | Path | Change |
|--------|------|--------|
| `GET` | `/documents/{id}/link-settings` | Return `mode` + `viewOptions`. Caller needs `View`. |
| `PATCH` | `/documents/{id}/link-settings` | Set `mode` + `viewOptions`. Caller needs `Own`. **Emit `SecurityAudit`** (closes the existing gap). |

There is no rotate endpoint and no `DELETE` (mode = off / `None`
disables).

### 5.3 Enforcing view sub-options

Sub-options are checked at the feature edges, layered on the base `View`
the link grants. The capability predicate for, e.g., commenting:

```
can_comment(user, doc) =
    user has Comment+ via owner / direct member / folder         // existing
 || ( link active
      && (mode == Edit
          || (mode == View && link_view_options.allow_comments))
      && user is a member of doc.workspace_id )                  // new
```

The same shape gates history reads (`show_history`), the activity feed
(`show_conversation`), and the request-access affordance
(`allow_request_access`). **Comment-thread reads** (`list_threads` /
`list_messages`) are gated on **`show_conversation || allow_comments`**
for link-only viewers — you may read the conversation if you can see it
*or* participate in it (post-audit `gap-001`; posting stays gated on
`allow_comments`). `AccessLevel` itself is **not** widened — the
sub-option is an additional capability check at the specific endpoint,
applied only to link-only viewers (durable members and edit-link viewers
are unaffected).

### 5.4 Request edit access

`POST /documents/{id}/request-access` — a workspace member who reaches a
doc via a `view` link (with `allow_request_access` on) requests Edit.
**Notifies the owner only** via a new append-only
`NotifType::RequestAccess` (email-rendered). The owner is the sole
recipient because `Own` is non-transferable (`add_doc_member` rejects
granting `Own`), so a doc has exactly one Own-level principal — this
closes open question #2.

**No `SecurityAudit` row is emitted:** a request grants nothing and
changes no access state, so it is a notification, not a security-audit
event (unlike `LinkSharingChanged` / `ShareGranted`). Authorization runs
**before** rate limiting (so an unauthorized caller can't probe doc
existence via bucket state, and the cap counts only authorized
requests). Two-tier rate limit, both keyed under the `sharing` bucket:
the existing per-user limiter (`enforce(.., "sharing", user_id, ..)`) to
bound cross-doc fan-out, plus a per-(doc, requester) cap
(`reqaccess:{id}:{user_id}`) to bound repeatedly pinging one owner.

### 5.5 Admin override

`PATCH /admin/documents/{id}/link-settings` — admin override for any doc.
**Global-admin only** (reuses the live-row `is_admin`
check, like the rest of the `/admin` router). Shares the
`apply_link_settings` helper with the owner endpoint, so it emits the
same `LinkSharingChanged` `SecurityAudit` row — `actor = admin`,
`subject = doc owner` — keeping every link-change in **one trail**; it is
deliberately *not* recorded in `AdminAudit` (a link change is a sharing
event, and splitting the trail by who-made-it would scatter it).

### 5.6 Audit (closes the existing bug)

Originally `update_link_settings` wrote **no** `SecurityAudit` row despite
CLAUDE.md requiring sharing write-paths to emit one. The variant records
the **resulting state**:

```rust
SecurityAuditAction::LinkSharingChanged {
    doc_id: String,
    mode: Option<LinkSharingMode>,   // resulting mode; None = disabled
    view_options: ViewOptions,       // resulting sub-options
}
```

Emitted on **any** change — a mode change (enable / switch / disable)
**and** a sub-option-only change (enabling `allow_comments` /
`show_history` etc. is a permission expansion and must leave a trail) —
by the owner or admin path. A true no-op PATCH (neither field present)
logs nothing. Each row captures the full post-change state; a reader
diffs against the prior row to see what moved. (Earlier this deferred
sub-option auditing; the post-merge security audit flagged that as a
`security-concerns.md` contradiction, so the variant was enriched with
`view_options` and now logs sub-option changes too.)

## 6. Token confidentiality / DTO

There is no secret token, so the v1 concern is moot. The only care:
`GET /documents/{id}/link-settings` returns the mode + sub-options; this
is fine for any reader. No field needs to be hidden from the general doc
DTO.

## 7. Collaboration

**Unchanged.** Editing still flows through the existing authenticated
WS-token path (`POST /documents/{id}/ws-token` → Origin-checked `/ws`).
A workspace member with an `edit` link mints a ws-token exactly as a
direct member does — `check_doc_access(.., Edit)` already honors the
link branch. No anonymous handshake, no synthetic identity.

> **One pre-existing gap to track (not introduced here):**
> `POST /documents/{id}/ws-token` requires `Edit`
> (`crates/api/src/routes/ws.rs`). A `view`-link member who should see
> **live** updates has no read-only WS subscription path today. This is
> orthogonal to link audience and can be handled as a separate
> read-only-WS work item; flagged so it isn't mistaken for a link-share
> requirement.

## 8. Frontend

`frontend/src/components/share_dialog.rs` gains a **Link sharing**
section above the existing people list:

- Master toggle (off ⇒ mode `None`).
- Mode segmented control: **Can view** / **Can edit** (managers only,
  i.e. `Own`).
- View sub-option checkboxes (shown only when mode = view).
- **Copy link** — copies the document's URL. (Plain convenience; the URL
  is not a secret.)
- A small explanatory line: *"Anyone in **{workspace name}** can open
  this"* (or the "unavailable — no workspace" state from §5.1.1).
- Read-only members see the current mode but cannot change it.

No public viewer route, no `/d/{token}` page.

> **As built (Phase 4, `f439b0b`).** The dialog gained a `doc_id` prop;
> the section is keyed on the document while the existing people list
> stays folder-scoped. Read-only vs. editable is driven by a new
> `canManage` field on the `GET /link-settings` response (= caller is
> owner). **Deferred from the above:** the explanatory line is **generic**
> ("Anyone in your workspace with the link can open this") — the workspace
> *name* and the no-workspace (§5.1.1) state need data the link-settings
> response doesn't carry yet. The **request-access affordance is
> viewer-facing** (a "you're viewing via a link" element on the doc view),
> so it's out of this manager-facing dialog's scope; the backend endpoint
> exists (§5.4) and the button is a small follow-up. The `share-link-*`
> i18n keys are translated across all six locales (en-US, de, es, fr, it,
> ar).

## 9. Search & discoverability

**Decision: keep link-shared docs discoverable workspace-wide — no join
gate — but _demote them in ranking_ so a member's own / engaged / recent
docs rank above company-link docs they have no relationship to.** This
preserves the company-link discovery promise while bounding the
signal-to-noise hit in large orgs (100K+ docs).

Rejected alternatives: a **hard join-gate** (breaks discovery — you can
only join what you can already find: chicken-and-egg), and **flat
auto-search at equal ranking** (today's behavior; the noise floor scales
with org size).

### 9.1 Three distinct rights

| Right | Rule | Status |
|-------|------|--------|
| **Access** (open the doc) | workspace member + link on → View/Edit; membership re-checked live every request | exists today |
| **Discoverability** (appears in results *at all*) | same set — auto, no join. Post-filter through `check_doc_access(.., View)` in `search.rs` | exists today, unchanged |
| **Ranking** (*where* it appears) | tiered by the searcher's relationship to the doc — **new** | needs a ranking layer (§9.3) |

### 9.2 Ranking tiers (relevance demotion, not exclusion)

Applied as a boost/penalty on the fused RRF score — everything stays in
the result set; the floor tier just ranks last:

1. **Durable** — owner / direct doc-member / folder-member.
2. **Engaged** — the searcher has opened it before. Signal already
   exists: the `DocOpen` row (`SK = OPEN#<user_id>`, `first_opened_at`)
   is the "previously viewed" signal.
3. **Recency** — `updated_at`, applied orthogonally across tiers.
4. **Workspace-link, no relationship** — discoverable but **demoted**;
   the floor tier.

### 9.3 The gap this exposes (honest)

OgreNotes search has **no personalization today**: `search.rs` fuses
BM25 + vector by RRF *score only*, with no per-searcher signal. The
demotion therefore requires a **ranking layer** that takes the searcher's
identity and consults membership + `DocOpen` (+ recency). That is
search-side infrastructure, largely **separable from the link-sharing
model itself**:

- **Link-sharing Phases 0–4 do not depend on it.** They ship on today's
  flat auto-search (equal ranking). **Until the ranking layer lands,
  large-org signal-to-noise is unchanged from today** — this is the
  explicit trade for shipping link sharing without blocking on search
  work.
- The demotion is a **separate follow-on deliverable**, owned by a
  search-ranking design, sequenced after
  the link-sharing phases.

### 9.4 Future direction (relevance tuning)

The tier boosts are the seed of a richer, tunable relevance model:

- **Team-aware.** Boost docs owned by or shared within the searcher's
  team(s). The workspace is the *company* tenant (per the
  one-workspace-per-company assumption in `high-level-design.md`);
  *teams* are the finer cohort **within** the workspace that relevance
  should key on. **Decision:** a team is a **many-to-many cohort signal
  sourced from the IdP** (SCIM groups + the SAML/SCIM `manager` chain),
  used **only for ranking, never for access**. Because it is a soft
  signal, ranking *blends* cohort signals rather than resolving a single
  canonical team — so "does a user belong to one team or many" need not
  be answered. Membership, if ever materialized, is stored as join rows
  (mirroring `WORKSPACE#/MEMBER#`), never a scalar `team_id` — the
  many-to-many shape subsumes single and avoids a breaking key/DTO
  migration later.
- **Engagement-based.** Intra-team topic affinity and cross-team
  engagement signals — who and what you interact with reshape ranking
  over time.
- **Knowledge/RAG synergy.** This dovetails with what already shipped in
  Phase 6: vector embeddings (6.2) and the DynamoDB adjacency knowledge
  graph + agentic search (6.3). Context/semantic search *benefits* from
  a broad-but-demoted corpus — RAG can retrieve company-wide knowledge
  while ranking the searcher's own context first, which is precisely why
  demotion (option keep-broad) beats a join-gate (option go-narrow) for
  the knowledge-search roadmap.

### 9.5 Indexing & acceptance

- **Indexing untouched:** the index stays global; share / unshare / open
  take effect at the next search via the query-time post-filter and
  ranking — no reindex.
- **Acceptance checks:**
  - **A** — a link-shared doc the searcher has a durable or engaged
    relationship to ranks **above** an unrelated link-shared doc of
    comparable text relevance.
  - **G** — a link-shared doc still **appears** for any workspace member
    (discovery preserved); turning the link off removes it for members
    with no durable access, but **never** for an explicitly-shared member
    or the owner.
  - **G (interim)** — until the ranking layer ships, behavior matches
    today: flat auto-search, score-only ranking.

## 10. Security posture

The workspace-only, authenticated-only scope removes the entire
high-risk surface from the v1 review:

- **Eliminated:** anonymous attack surface, fail-open limiting on public
  endpoints, token-in-URL logging, public PII leakage via history/chat,
  anonymous-edit cost/abuse, anonymous repudiation (v1 findings
  **H1–H4, M1–M4**).
- **Residual surface is authenticated and workspace-internal:**
  - Sub-option capability checks must be enforced server-side at each
    feature endpoint (§5.3) — never trust the client to hide a button.
  - `request-access` notification spam — bounded by the existing sharing
    rate limiter + a per-(doc, requester) cap (§5.4).
  - Comments/edits are rendered to other **workspace** members only;
    the existing sanitizer + CSP already cover this (no new
    unauthenticated render path).
  - **Audit:** the new `LinkSharingChanged` row (§5.6) is itself a
    security improvement — it closes a real existing gap.
- **Defense in depth:** the access check must keep requiring *current*
  workspace membership on every request (it does — membership is fetched
  live in `check_doc_access_allow_deleted`), so removing a user from the
  workspace immediately revokes their link-derived access.

### 10.1 Post-merge security audit (hardening)

A deep `security-auditor` pass (12 controls verified, 0 critical) drove a
hardening round on top of Phases 0–4:

- **gap-001** — comment-thread reads (`list_threads`/`list_messages`) now
  enforce `show_conversation || allow_comments` for link-only viewers
  (previously ungated; §5.3).
- **gap-004** — sub-option changes are now audited (variant enriched with
  `view_options`; §5.6).
- **gap-002** — the owner link-settings PATCH gained the per-user sharing
  rate limit (parity with the admin override and member shares).
- **gap-003** — `update_link_settings` writes under
  `attribute_exists(PK)` so a hard-delete race can't resurrect a partial
  `METADATA` ghost row.
- **gap-005** — `request_access` checks owner-self *before* charging the
  rate-limit buckets.

Declined: a typed frontend `LinkMode` enum (the backend's `null`-vs-`none`
dual disabled-representation makes the `String` form simpler and it's
confined to one component) and rejecting empty no-op PATCHes (contrived).

## 11. Delivery phases

| Phase | Scope | Size | Status |
|-------|-------|------|--------|
| **0 — Audit gap** | Emit `SecurityAudit` (`LinkSharingChanged`) on the existing link-settings change. Pure fix + regression tests. No model change. | S | ✅ **landed** (`2e9b73f`) |
| **1 — Model cleanup** | Remove inert `link_sharing_allow_external` (deliberate schema change); add `link_view_options: ViewOptions` (serde defaults, sparse JSON-string storage, no migration). Update `GET`/`PATCH /link-settings` DTOs (`viewOptions`). | S–M | ✅ **landed** (`b95235c`) |
| **2 — Sub-option enforcement** | `allow_comments` / `show_history` / `show_conversation` capability checks at the feature endpoints (link-only viewers only); `request-access` flow + `NotifType::RequestAccess`. | M | ✅ **landed** (`31f039b`) |
| **3 — Admin override** | `PATCH /admin/documents/{id}/link-settings` (global-admin) + audit via `record_security_event_by_actor` (actor = admin, subject = owner); shared `apply_link_settings` helper. | S | ✅ **landed** (`feat/link-sharing`) |
| **4 — Frontend** | Link-sharing section in the share dialog (doc_id prop): mode segmented control, view sub-option checkboxes, Copy link, owner-gated via a new `canManage` GET field. Generic workspace line (no name); no-workspace state deferred. | M | ✅ **landed** (`f439b0b`) |

> **Landed in Phase 3:** the audit variant was tightened from
> `mode: Option<String>` to `mode: Option<LinkSharingMode>` (§5.6).
> `from_storage` parses the stored `"view"`/`"edit"` strings back to the
> enum and errors on an unknown value; pre-existing rows stay readable
> because the wire values are identical.

**Separate follow-on (not a link-sharing phase):** the **search ranking
demotion** of unrelated workspace-link docs (§9.2–9.3) is search-side
infra owned by a search-ranking design. Phases
0–4 ship on today's flat auto-search; the demotion lands independently,
and large-org signal-to-noise is unchanged until it does.

(The pre-existing **read-only WS** gap from §7 is also tracked
separately and is not a prerequisite for any phase above.)

## 12. Open questions for review

1. **Guest/"allow new messages"** — **resolved (folded, intentionally).**
   In OgreNotes a document's conversation is one message stream:
   `show_conversation` gates *reading* the activity/conversation pane,
   and *posting* (inline comments and document-level threads alike) is
   the single `create_thread`/`add_message` path gated by
   `allow_comments`. There is no separate doc-chat write endpoint
   (`chat.rs` is standalone rooms), so a separate "allow new
   messages" toggle has nothing distinct to gate. Revisit only if a user
   needs "can post to the conversation but not comment inline" (or vice
   versa).
2. **Request-access target** — **resolved: owner only.** `Own` is
   non-transferable (`add_doc_member` rejects it), so a doc has exactly
   one Own-level principal; "all Own-level members" ≡ the owner (§5.4).
3. **`LinkSharingMode::None` cleanup** — collapse the enum to
   `{Edit, View}` and rely on `Option::None`, or leave as-is? (Cosmetic.)
4. **Read-only WS** (§7) — schedule it alongside this work so `view`
   links get live updates, or defer?
5. **Search ranking layer** (§9.3–9.4) — confirmed a **separate
   deliverable** owned by the search workstream, sequenced after
   Phases 0–4.
   **Decided:** a "team" is a many-to-many, IdP-sourced cohort signal
   (SCIM groups + `manager` chain), ranking-only, never access; stored
   as join rows if ever materialized (§9.4). **Left open (deliberately,
   not needed for a soft signal):** whether team membership is ever
   constrained to a single team as a *policy* — undecided until/unless a
   use case demands a canonical team. Remaining detail for the
   search-ranking design: exact signal sources (`DocOpen`,
   knowledge-graph adjacency, embeddings) and their weights.
