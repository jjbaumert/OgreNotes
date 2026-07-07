# SCIM token rotation

SCIM bearer tokens authenticate an IdP (Okta, Entra ID, …) to the
OgreNotes provisioning endpoints. Rotating them periodically
limits the blast radius of a leaked token. This runbook describes
the zero-downtime rotation pattern.

The companion document `scim-config.md` covers initial setup;
this one assumes you already have a working SCIM connector.

## When to rotate

- **Routine**: quarterly or annually, per your org's secret-
  rotation policy.
- **Out-of-band**: immediately if a token may have leaked (commit
  to a public repo, exposed in a screenshot, leaked via a
  compromised admin laptop).
- **After an offboarding**: if the admin who minted the token has
  left the org.

## Zero-downtime rotation pattern

The point of the dance: the IdP should never see a "no valid
token" gap. Mint the new token first, hand it to the IdP, confirm
the IdP is using the new one, THEN revoke the old.

### 1. Mint a new token

In the OgreNotes web UI:

1. Open the workspace admin page → **SCIM Tokens** section.
2. Click **Create token**.
3. Label it with the rotation date and the IdP name, e.g.
   `Okta-2026-05-rotation`. Labels are searchable in the list
   view and surface in audit-log entries.
4. The plaintext token is shown **once** in a modal — copy it
   immediately. The format is `<token_id>.<secret>`. After the
   modal closes, only `token_id` is visible in the list; the
   secret is bcrypt-hashed at rest.

The HTTP path under the hood: `POST /api/v1/workspaces/:ws/scim-tokens`.

### 2. Paste into the IdP

The IdP's SCIM connector has a field labeled something like
"API Token" or "Bearer Token". Replace the existing value with
the new token's plaintext. Save the configuration.

In Okta:
1. Applications → your OgreNotes app → **Provisioning** → **Integration**.
2. Edit, paste new token into **API Token**.
3. Click **Test API Credentials**. Expect "OK".
4. Save.

In Entra ID:
1. Enterprise applications → your app → **Provisioning**.
2. Edit, paste new token into **Secret Token**.
3. Click **Test Connection**. Expect green check.
4. Save.

### 3. Wait for the next sync

Most IdPs do incremental syncs on a 40-60 minute cadence. To
confirm the new token is being used, wait for one sync cycle and
check the audit log:

    aws logs filter-log-events \
        --log-group-name "/ecs/<prefix>ogrenote" \
        --filter-pattern "kind=scimTokenUsed" \
        --start-time $(date -u -d "1 hour ago" +%s)000

Look for entries with the NEW token's `token_id`. As long as
those are appearing, the IdP is authenticated against the new
token.

Or, force a sync via the IdP's UI — most have a "Provision on
demand" button that runs a one-shot sync immediately.

### 4. Confirm the old token is no longer in use

Filter logs for the OLD token_id over the last hour. There should
be ZERO entries. If you still see calls with the old token_id,
the IdP didn't fully roll over — check the IdP config and try a
forced sync again. Don't revoke yet.

### 5. Revoke the old token

In the OgreNotes admin UI:

1. Workspace admin → **SCIM Tokens**.
2. Find the old token by label or by `token_id`.
3. Click **Revoke**.

The token row's `disabled_at` field is set. Any subsequent
request bearing this token gets 401 from the SCIM extractor —
the auth check rejects any token whose `disabled_at != 0`.

HTTP path under the hood: `DELETE /api/v1/workspaces/:ws/scim-tokens/:token_id`.

### 6. Audit-verify the revocation

The revocation itself writes a row to `AdminAudit` keyed on the
workspace. Confirm:

    aws dynamodb query --table-name <table-name> \
        --key-condition-expression "PK = :pk AND begins_with(SK, :prefix)" \
        --expression-attribute-values '{":pk":{"S":"WORKSPACE#<ws-id>"},":prefix":{"S":"ADMIN_AUDIT#"}}' \
        --region <region>

The recent rows include a `scimTokenRevoked` entry naming the
revoking admin (the actor) and the token_id.

## What goes wrong, and how to detect it

- **IdP is still using the old token after step 3**: the IdP's
  cache hasn't picked up the new config. Force a sync, or in
  Okta's case, "Refresh" the provisioning integration.

- **You revoked the wrong token by mistake**: there's no
  "un-revoke". You'll need to mint a new token, paste into the
  IdP, and accept the gap. If the wrong token was actively in
  use, the IdP's next sync will 401, which is a high-signal
  alert through your IdP's monitoring.

- **The IdP keeps trying to use the old token after revocation**:
  the connector wasn't updated. Re-do step 2 and confirm the IdP
  actually saved the new value (some IdPs silently retain the
  old config if the save fails).

- **You can't tell which token is active**: each `ScimTokenUsed`
  audit row carries the `token_id`. The token_id is the first
  segment of the plaintext token (`<token_id>.<secret>`). If you
  still have the new token plaintext, the prefix-up-to-the-dot
  is the token_id you should be seeing in audit logs.

## Force-revoke without IdP coordination

For an emergency revocation (suspected leak), don't wait for the
IdP to roll over — revoke immediately. The IdP's next sync will
401; their monitoring will alert; you fix the IdP config when
you're back to a clean state. The cost is some hours of failed
provisioning ops, far less than the cost of a leaked-token
window.

Steps: skip 1-4 above; just go to the admin UI's SCIM Tokens
section and revoke. Then deal with the IdP.

## Rate-limit context

M-E8 gap-005 added a pre-bcrypt rate limit on SCIM endpoints
(`SCIM_REQUEST_RATE_LIMIT_PER_MIN`, default 100). Failed
authentication counts against the budget. If you find yourself
investigating a SCIM 429 during rotation, possibilities:
- the IdP is retrying aggressively with the stale token after
  revocation;
- the IdP is configured for sub-minute polling and is normally
  near the cap.

Bump the config if the cap is genuinely too low; otherwise wait
for the bucket to roll over (60s).

## v2 carry-forwards

- **Token expiry**. Tokens currently live until revoked. A future
  enhancement could set `expires_at` at mint time, and the
  extractor would treat post-expiry tokens like
  `disabled_at != 0`.
- **Automated rotation reminders**. No mechanism notifies admins
  when a token is N months old. An ops dashboard could surface
  this.
- **Per-token scopes**. Currently a SCIM token can hit any
  endpoint in the workspace's SCIM v2 surface. A future v2 could
  scope a token to (e.g.) read-only or to specific resource
  types.
