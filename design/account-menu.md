# Account Menu & Settings

> **Status: implemented** (branch `feat/account-menu-settings`, commit
> `1bc7293`). All six build-sequence steps shipped. This doc is kept as
> the design of record; the **Build sequence** and **Notification prefs**
> sections below carry as-built notes where the implementation
> deliberately diverged from the original plan.

Phase 5 polish design doc. OgreNotes has no unified **account
menu** — the per-user controls (profile, status, notification
prefs, appearance, sign-out) are today scattered across the
sidebar footer and the command palette, and one of the entry
points (`/profile`) is a dead link. This doc specifies a single avatar-anchored menu plus
a `/settings` page that consolidates them.

It is a parity-and-cleanup item, not a new product surface: the
persistence plumbing (`GET /users/me`, `PUT /users/me/prefs`)
already exists; most of the work is frontend consolidation plus
a few small new backend endpoints (profile-field edit, status,
notification prefs).

## Motivation

A single avatar-anchored menu in the lower-left of the sidebar
should be the hub for everything personal. Mapping that target
against what OgreNotes ships today:

| Capability | OgreNotes today | Status |
| --- | --- | --- |
| Profile and Status | read-only name in doc header; 👤 → `/profile` | **broken route**, no editor, no status |
| Notifications (prefs) | — | missing |
| Customize Sidebar | — | missing |
| Settings (panel) | prefs inline in sidebar footer | no page |
| ↳ Profile (name/photo/email/lang) | locale selector only | partial |
| ↳ Appearance (theme, font, …) | theme selector only | partial |
| ↳ Notifications | — | missing |
| ↳ Help & Support | `help.about-palette` → `console.log` | placeholder |
| Account Switcher | — | missing |
| Keyboard shortcuts | placeholder command | placeholder |
| Log Out | sidebar + palette → `client::logout()` | **done** |

Three navigation references in the current UI point at routes
that do not exist in `frontend/src/app.rs`: `/profile`
(`sidebar.rs`), `/tasks` (`sidebar.rs`), `/trash`
(`commands/mod.rs`). The first is in scope here; the other two
are tracked separately as dead-route bugs.

## Goals

- **One avatar-anchored menu.** Clicking the avatar opens a
  dropdown that is the single entry point for profile, status,
  settings, help, and sign-out — replacing the scattered footer
  controls.
- **A real `/settings` page** with stable sections (Profile,
  Appearance, Notifications, Accessibility, Help) that absorb the
  theme/locale selectors currently inline in the sidebar footer.
- **Editable profile.** Display name and avatar become editable
  (today they are OAuth-sourced and read-only). Email stays
  read-only (identity-bound; changing it is an auth concern out
  of scope here).
- **Status (presence).** A lightweight status — short text +
  optional emoji + optional expiry — surfaced next to the avatar.
- **No dead links.** `/profile` resolves (redirect to
  `/settings#profile`), and every menu item lands on a real
  surface.
- **i18n-native & a11y-native.** Every label resolves through
  Fluent; the menu is keyboard-navigable and the
  already-modelled `dyslexic_font` / `reduce_motion` prefs get a
  UI under Accessibility.

## Out of scope (v1)

- **Account Switcher / multi-account.** OgreNotes has no
  linked-account model; deferred until one exists.
- **Customize Sidebar.** The sidebar sections are fixed today;
  revisit after the menu lands.
- **Email change.** Identity-bound; an auth flow, not a setting.
- **Notification *delivery* changes.** This doc adds a prefs
  surface; the worker-side honoring of those prefs is a
  follow-up.
- **Billing / subscription.** No billing model exists.

## Architecture

```
┌──────────────────────────────┐
│ components/account_menu.rs   │  NEW — avatar button + dropdown
│  - avatar (img | initials)   │
│  - status pill               │
│  - menu items → routes/acts  │
└───────────────┬──────────────┘
                │ navigates / fires
                ▼
┌──────────────────────────────┐     ┌───────────────────────────┐
│ pages/settings.rs            │     │ api/client.rs             │
│  NEW — tabbed settings page  │◄───►│  get /users/me            │
│   #profile #appearance       │     │  put /users/me/prefs      │
│   #notifications #a11y #help │     │  put /users/me (NEW)      │
└──────────────────────────────┘     │  get/put /users/me/       │
                                      │     notification-prefs(NEW)│
                                      └───────────────────────────┘
```

The `ThemeSelector` and `LocaleSelector` components are **moved**
(not duplicated) out of the sidebar footer into the Appearance
section of `pages/settings.rs`; the sidebar footer keeps only the
build stamp. Sign-out moves into the account menu (the palette
`cmd-sign-out` stays).

### Menu tree (target)

```
[Avatar + name + status pill]
├─ Profile & Status      → /settings#profile  (edit name/avatar; set status)
├─ Settings              → /settings#appearance
│    ├─ Profile          name, avatar, email (read-only), language
│    ├─ Appearance       theme (system/light/dark), doc theme
│    ├─ Notifications    per-event toggles, email digest        (NEW backend)
│    ├─ Accessibility    dyslexic font, reduce motion           (prefs exist)
│    └─ Help & Support   shortcuts (⌘/Ctrl-K), docs link, version
├─ Keyboard shortcuts    → opens palette help (replaces console.log stub)
└─ Sign out              → client::logout() → /login
```

## Data model

Reuse what exists; add the minimum.

**Already present** (`crates/storage/src/models/user.rs::UiPrefs`,
served by `GET /users/me` / merged by `PUT /users/me/prefs`):
`theme`, `doc_theme`, `dyslexic_font`, `reduce_motion`, `locale`.
The Accessibility and Appearance sections need **no** new
backend — they are UI over fields the merge-PUT already handles.

**New — profile fields.** `name` and `avatar_url` live on `User`
but have no mutation path (they are set only by the OAuth profile
sync). Add:

```
PUT /users/me            { name?: String, avatarUrl?: String }
```

Same partial-merge contract as `put_ui_prefs` (absent field ⇒
unchanged). Apply the same length caps as `sanitize_profile` in
`routes/auth.rs` (`MAX_NAME_LEN`, `MAX_AVATAR_URL_LEN`). Editing
identity-adjacent fields should emit a `SecurityAudit` row via
`routes::mfa::record_security_event` per the project audit
convention.

**New — status.** Add to `User` (or a sibling record):

```
status: Option<UserStatus>
struct UserStatus { text: String, emoji: Option<String>, expires_at: Option<i64> }
```

served on `GET /users/me` and set by `PUT /users/me/status`.
Expiry is honored read-side (a status past `expires_at` reads as
absent) to avoid a sweeper. Presence broadcast to collaborators
is **out of scope** for v1 — status is self-visible and shown on
the user's own avatar only until a presence channel exists.

**Notification prefs — as built (deviation).** The original plan
assumed no notification-pref existed and proposed a new
`NotificationPrefs` record (per-event booleans + digest cadence).
That was wrong: `User.email_notifications: NotifEmailPref`
(`All` / `MentionsOnly` / `Disabled`) **already exists and is
honored by the notify worker** (`crates/notify/src/service.rs`) — it
simply had no endpoint or UI. Building a parallel per-event record
would have been dead code the worker ignores, so step 6 instead
**exposes the existing field**:

```
PUT /users/me/notification-prefs   { emailNotifications }
```

and surfaces it on `GET /users/me`. A change takes effect for the
next email immediately — no worker rewiring. Finer-grained per-event
toggles / in-app channels remain a future item gated on the worker
growing richer preferences.

## Build sequence (as built — all shipped)

1. ✅ **`/profile` redirect + `/settings` route shell.** `/settings`
   tabbed shell + `/profile` → `/settings#profile`. Closed the dead
   sidebar link (issue #102).
2. ✅ **Appearance + Accessibility.** Moved `ThemeSelector` /
   `LocaleSelector` into Settings; added `dyslexic_font` /
   `reduce_motion` toggles. **Deviation:** because those selectors
   applied stored prefs on mount and lived in the always-present
   sidebar, moving them would have stopped stored theme/locale from
   applying app-wide — so the stored-prefs bootstrap (theme + a11y +
   first-login locale sync) was **lifted into `main.rs`** and the
   selectors became pure controls. Language ended up under **Profile**
   (matching the section tree), not Appearance.
3. ✅ **`account_menu.rs`.** Avatar (image/initials) + identity +
   Profile / Settings / Keyboard-shortcuts / Sign-out. Sign-out moved
   here.
4. ✅ **Profile editing.** `PUT /users/me` (name/avatar, capped,
   http(s)-only avatar) + `ProfileUpdated` SecurityAudit row (records
   which fields changed, never values) + Profile tab form. Pure
   `validate_profile_patch` is unit-tested.
5. ✅ **Status.** `UserStatus` model, `PUT /users/me/status`,
   read-side expiry, status editor + trigger pill. Pure `build_status`
   is unit-tested. No audit row (transient presence).
6. ✅ **Notification prefs.** Exposed the existing worker-honored
   `NotifEmailPref` (see the Notification-prefs deviation above) +
   Notifications tab. Help & Support section also landed here
   (shortcuts + version) and the deferred "Keyboard shortcuts" menu
   item / `help.about-palette` command were wired to `/settings#help`.

Steps 1–3 were pure frontend consolidation; steps 4–6 each added one
small endpoint and landed independently.

## Risks / notes

- **Tests are contracts.** Moving `ThemeSelector` /
  `LocaleSelector` must not change their persistence behavior; any
  existing test asserting their footer placement is a behavior
  contract — surface a finding rather than editing it.
- **Wire-shape additions are deliberate.** New fields on the
  `GET /users/me` DTO (`status`) and the new endpoints are public
  API surface; treat as additive (all-optional) so older clients
  keep decoding.
- **Avatar upload vs. URL.** v1 takes an `avatarUrl` string
  (paste/keep OAuth URL). Native upload to S3 is a larger piece;
  defer unless prioritized.
- **`reduce_motion` / `dyslexic_font` already merge-safe.** The
  `Option<bool>` shape in `UiPrefs` exists specifically so a
  partial PUT from this UI won't clobber the other a11y pref — no
  new merge logic needed.
