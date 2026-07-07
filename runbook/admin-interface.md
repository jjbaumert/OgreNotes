# Admin interface

OgreNotes' administration surface has two layers: a **browser UI**
(three pages in the Leptos SPA) and the **REST API** those pages
call, nested under `/api/v1/admin`. A separate **SCIM v2** surface
handles automated user provisioning from an IdP. This runbook is
the URL reference for all three.

Every admin UI page and `/api/v1/admin` route requires an
authenticated user with `is_admin`. Mutations are additionally
rate-limited and written to the permanent `AdminAudit` log. SCIM
is the exception: it authenticates a per-workspace bearer token,
not an admin session (see `scim-config.md` and
`scim-token-rotation.md`).

## Hosts

There is no hardcoded production domain in the repo; the host is
environment-specific. Substitute yours for `<UI>` and `<API>`
below.

| Environment | UI origin (`<UI>`) | API origin (`<API>`) |
|-------------|--------------------|----------------------|
| Local dev   | `http://localhost:8080` (`FRONTEND_ORIGIN`) | `http://localhost:3000` |
| Production  | `https://<your-domain>` (set via `FRONTEND_ORIGIN`, served behind the ALB) | same origin, under `/api/v1` |

## Admin UI (browser pages)

Routed in `frontend/src/app.rs`. Visit these signed in as an
admin:

| Page            | URL                  | What it does |
|-----------------|----------------------|--------------|
| User management | `<UI>/admin/users`   | List, inspect, enable/disable, promote/demote, toggle AI-ask access |
| Metrics         | `<UI>/admin/metrics` | In-process metrics snapshot |
| Audit log       | `<UI>/admin/audit`   | Browse admin/security audit rows |

Related enterprise (workspace-scoped) UI pages:

| Page                   | URL |
|------------------------|-----|
| SAML SSO config        | `<UI>/workspaces/:id/saml` |
| SCIM config            | `<UI>/workspaces/:id/scim` |
| MFA enroll / challenge | `<UI>/auth/mfa-enroll`, `<UI>/auth/mfa-challenge` |

## Admin API (`/api/v1/admin`)

Defined in `crates/api/src/routes/admin.rs`. All require an admin
session (`require_admin`); the mutating routes are rate-limited
(`rate_limit_admin_mut_per_min`) and audit-logged via
`record_admin_action`.

| Method     | URL | Purpose |
|------------|-----|---------|
| GET        | `<API>/api/v1/admin/users` | List users (supports query filters) |
| GET        | `<API>/api/v1/admin/users/{id}` | Get one user |
| POST       | `<API>/api/v1/admin/users/{id}/disable` | Disable account |
| POST       | `<API>/api/v1/admin/users/{id}/enable` | Re-enable account |
| POST       | `<API>/api/v1/admin/users/{id}/promote` | Grant admin |
| POST       | `<API>/api/v1/admin/users/{id}/demote` | Revoke admin |
| GET / PUT  | `<API>/api/v1/admin/users/{id}/ask-enabled` | Read / set per-user AI-ask access |
| GET        | `<API>/api/v1/admin/metrics` | Metrics snapshot |
| GET        | `<API>/api/v1/admin/audit` | Audit log (query-filterable) |
| POST       | `<API>/api/v1/admin/documents/{id}/compact` | Force CRDT op-log compaction |

## SCIM v2 provisioning API (`/api/v1/scim/v2`)

Defined in `crates/api/src/routes/scim.rs`. **Token-authenticated**
(per-workspace SCIM bearer token via the `scim_auth` middleware),
not the admin session — meant for IdP-driven provisioning. All
routes are workspace-scoped.

| Method                     | URL |
|----------------------------|-----|
| GET / POST                 | `<API>/api/v1/scim/v2/workspaces/{ws_id}/Users` |
| GET / PUT / PATCH / DELETE | `<API>/api/v1/scim/v2/workspaces/{ws_id}/Users/{user_id}` |
| GET                        | `<API>/api/v1/scim/v2/workspaces/{ws_id}/Groups` |
| GET / PATCH                | `<API>/api/v1/scim/v2/workspaces/{ws_id}/Groups/{group_id}` |
| GET                        | `<API>/api/v1/scim/v2/workspaces/{ws_id}/ServiceProviderConfig` |
| GET                        | `<API>/api/v1/scim/v2/workspaces/{ws_id}/ResourceTypes` |
| GET                        | `<API>/api/v1/scim/v2/workspaces/{ws_id}/Schemas` |

## Access model at a glance

- **Admin UI + `/api/v1/admin`** — gated on `user.is_admin`
  (`require_admin`). Admin *mutations* are rate-limited and every
  privileged action funnels through `record_admin_action` →
  `AdminAudit` (retained permanently).
- **SCIM** — separate per-workspace bearer token via `scim_auth`.
  Setup in `scim-config.md`; rotation in `scim-token-rotation.md`.

## Related runbooks

- `scim-config.md`, `scim-token-rotation.md` — SCIM setup and token lifecycle
- `saml-config.md` — SAML SSO setup
- `mfa-recovery.md` — MFA recovery-code lifecycle
- `restore-from-backup.md`, `trash-purge.md` — destructive ops that emit audit rows
