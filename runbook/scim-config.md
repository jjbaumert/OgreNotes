# SCIM provisioning — configure and verify

Phase 4 M-E5 ships workspace-level SCIM 2.0 (RFC 7643/7644)
provisioning. A workspace admin mints a SCIM bearer token, the
admin pastes the token plus the workspace's SCIM base URL into
their IdP's SCIM connector, and the IdP then creates / updates /
deprovisions users + workspace memberships via the SCIM endpoints.

SCIM canonically pairs with SAML SSO — SCIM creates the user
records that SAML later signs them into. M-E5 doesn't require SAML
to be configured (SCIM-created users just can't log in until SAML
is), but the two are typically configured together.

## Prerequisites

- A workspace where the operator is owner or workspace-admin.
- An IdP tenant where the operator can configure SCIM (Okta admin,
  Entra ID app admin, JumpCloud admin, etc.).
- The OgreNotes deploy URL (e.g. `https://ogrenotes.example.com`).

## 1. Get the SCIM base URL + mint a token

In OgreNotes, open `/workspaces/<workspace-id>/scim`. The page
shows:

1. **SCIM Base URL** — paste this into the IdP's SCIM
   "Application URL" / "Base URL" / "Tenant URL" field. Format:
   ```
   https://ogrenotes.example.com/api/v1/scim/v2/workspaces/<workspace-id>
   ```
2. **Create a new token** — enter a label like "Okta connector for
   Acme" and click **Create**.

The page shows the plaintext bearer in an amber banner **ONCE**.
Format is `<token_id>.<secret>`. Copy it immediately — we cannot
recover it.

## 2. Configure the IdP

### Okta

1. **Applications → Your SAML app → Provisioning → Configure API
   Integration**.
2. **Enable API integration** → set:
   - **Base URL**: `https://ogrenotes.example.com/api/v1/scim/v2/workspaces/<workspace-id>`
   - **API Token**: paste the plaintext token from step 1.
3. **Test API Credentials**. Okta will hit
   `GET /ServiceProviderConfig` to discover capabilities. A green
   tick means the bearer auth works.
4. **To App** tab: enable:
   - **Create Users** — Okta will POST /Users on assignment.
   - **Update User Attributes** — Okta will PATCH /Users/{id}.
   - **Deactivate Users** — Okta sends PATCH with `active: false`.
5. **Attribute Mappings**: confirm `userName` maps to email,
   `externalId` is populated (Okta usually sets it automatically),
   and `displayName` is set.
6. Assign people / groups to the app to trigger initial sync.

### Entra ID (Azure AD)

1. **Enterprise applications → Your SAML app → Provisioning →
   Get started**.
2. **Provisioning Mode**: **Automatic**.
3. **Admin Credentials**:
   - **Tenant URL**: `https://ogrenotes.example.com/api/v1/scim/v2/workspaces/<workspace-id>`
   - **Secret Token**: paste the plaintext token.
4. **Test Connection** — Entra hits `GET /ServiceProviderConfig`.
5. **Mappings → Provision Microsoft Entra ID Users**: confirm
   `userPrincipalName → userName`, `objectId → externalId`,
   `displayName → displayName`.
6. Save, then assign users / groups to the app and turn
   **Provisioning Status** to **On**.

### JumpCloud

1. **User Authentication → SSO → Your SAML app → Configure SCIM**.
2. **Base URL** + **Bearer Token** fields — same values as above.
3. JumpCloud test query is `GET /Users?filter=userName eq "..."`;
   our filter parser supports this exact shape.

## 3. Verify provisioning

After triggering an initial sync from the IdP:

### Users land

Open the workspace's member list (regular admin UI) — the IdP-
provisioned users should appear with role = Member.

Or query SCIM directly to confirm what the IdP sees:

```
curl -H "Authorization: Bearer <token_id>.<secret>" \
     "https://ogrenotes.example.com/api/v1/scim/v2/workspaces/<id>/Users"
```

The response is a SCIM ListResponse envelope (capital-R
`Resources`). Each user has `id`, `userName`, `externalId`,
`active`.

### Audit log

Search the API log group in CloudWatch for:

```
kind=scimTokenUsed
```

You should see one structured event per SCIM request with
`token_id=<your token>` and `op=<users.list | users.create | ...>`.
Every authenticated SCIM request lands one of these.

### Deprovision works

In the IdP, unassign a user from the OgreNotes app. Within seconds
the IdP issues `PATCH /Users/{id}` with `active: false` (Okta) or
`DELETE /Users/{id}` (some IdPs). Verify:

- The user's `User.is_disabled` is `true` (admin UI shows the
  account greyed out).
- The user can no longer log in via SAML.

## Common errors → what they mean

The SCIM endpoints return SCIM-shaped error bodies (`schemas`,
`status` as string, `scimType`, `detail`) per RFC 7644 §3.12 — IdPs
expect this exact shape. The structured `tracing::warn!` fields in
the API log carry the forensic detail.

| Status / scimType | Cause |
|---|---|
| 401 (no body distinction) | Bearer missing, malformed, wrong secret, or token revoked. Check the token isn't disabled on the OgreNotes SCIM page. |
| 400 `invalidFilter` | The IdP sent a filter v1 doesn't support. v1 supports `userName eq "..."` and `externalId eq "..."` only. If a real IdP needs richer filters, file a follow-up. |
| 400 `invalidValue` (members) | A Group PATCH `members` entry was missing its `value` field. IdP misconfiguration; check the Attribute Mapping. |
| 400 `invalidValue` (member does not exist) | A Group PATCH `add` referenced a `user_id` that isn't in DDB. The IdP is sending a member uid it cached before a User was deleted. Trigger a re-sync. |
| 400 `mutability` | The IdP tried to change `userName` via PUT. v1 treats it as immutable; the IdP's "rename" feature isn't supported. |
| 400 `invalidPath` (Group) | A PATCH used the SCIM filter-path shape `members[value eq "x"]`. v1 supports `path="members"` with explicit value lists; the filter-path form is a follow-up. |
| 409 `uniqueness` | POST /Users hit a cross-provider hijack guard: the email already belongs to a User row with a non-SAML provider (GitHub, Google). Migrate the legacy account first (see runbook for that). |
| 500 (no body distinction) | Internal DDB / lookup failure. Check API logs. |

## Revocation + cleanup

Revoking a token:

1. `/workspaces/<id>/scim` → click **Revoke** on the row.
2. The token row stays in DDB (so audit-log references resolve)
   but `disabled_at` is set; subsequent SCIM requests via that
   bearer get 401.

Decommissioning SCIM for a workspace entirely:

1. Disable provisioning in the IdP first (so it stops sending
   requests).
2. Revoke every token in `/workspaces/<id>/scim`.
3. Optional: deprovisioned users keep their disabled User rows —
   delete them manually via the admin UI if you want a clean
   slate.

## Why no automated happy-path test against a real IdP

The SCIM test suite covers JIT round-trip, filter parsing,
deprovision via PATCH, member reconciliation via Group PATCH, and
the workspace-scope authorization gate — but those tests use a
fixture token minted directly via `mint_token()` and the SCIM
endpoints exercised in-process. End-to-end "real IdP pushes to
our deploy" verification is the runbook above; we don't have a
CI-hostable Okta or Entra instance.
