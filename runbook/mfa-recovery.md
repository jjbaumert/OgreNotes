# MFA recovery

Phase 4 M-E3 ships TOTP-based MFA for OgreNotes. A user who enrolls
in MFA gets ten single-use recovery codes at enrollment time —
those are the lifeline when the authenticator app is lost.

This runbook covers two audiences: the **end user** who has lost
their second factor, and the **admin** helping a user whose recovery
codes are also gone.

## End-user path: I lost my authenticator app

You enrolled in MFA, you wrote your ten recovery codes somewhere
you can still reach, and your phone died / got reset / got stolen.
Use a recovery code to log back in.

### 1. Start a login

Hit `/login` and authenticate with the IdP (GitHub, Google,
whatever provider you enrolled with). The OAuth callback notices
your account has MFA enrolled and redirects you to the MFA
challenge page with an opaque handle in the URL.

### 2. Use a recovery code instead of a TOTP

On the MFA challenge page, click "Use a recovery code". The
frontend swaps the 6-digit TOTP input for a recovery-code input
(format `xxxxx-xxxxx`).

Type one of your ten codes verbatim. The hyphens are preserved.
On success, a session is minted exactly as if you'd typed a TOTP.

### 3. Re-enroll a new authenticator

Each recovery code is single-use. The instant one is consumed it's
deleted server-side; if you needed it once, you might need another
later. As soon as you're logged in:

1. Go to **Settings → Security → MFA**.
2. Click "Disarm MFA". You'll need to type one of your remaining
   recovery codes to disarm — the server requires a fresh
   authenticator factor, and since you don't have one, the
   recovery-code prompt accepts it instead.
3. Click "Enroll" and pair a new authenticator app.
4. Save the new set of ten recovery codes to somewhere safe.

### 4. What if I'm out of recovery codes too?

Skip to the admin path below. Self-service recovery ends when
both the authenticator and the recovery-code paper are lost.

## Admin path: helping a fully locked-out user

A user contacts support saying they're locked out of MFA AND their
recovery codes are gone. Verify identity OUT-OF-BAND first — the
whole point of MFA is to defend against a stolen primary factor,
so don't accept the lockout claim at face value.

### 1. Verify identity

Acceptable: video call where the user shows ID, ticket from the
user's verified email account, in-person verification, manager
attestation. **Not acceptable**: an inbound email from the
allegedly-locked-out address (account takeovers start there) or
a third party speaking on the user's behalf.

Document who verified, when, and how.

### 2. Disarm the user's MFA via the database

There is **no admin UI** for force-disarming MFA in v1. The reason
is deliberate: making this trivial via the dashboard turns the
admin role into an MFA bypass. The path is direct DDB writes,
which guarantees an audit log entry through `AdminAudit`.

Find the user's ID:

    aws dynamodb scan --table-name <table-name> \
        --filter-expression "email = :email" \
        --expression-attribute-values '{":email":{"S":"user@example.com"}}' \
        --projection-expression "user_id" \
        --region <region>

Clear the MFA fields. Three updates:

    # Remove the encrypted TOTP secret
    aws dynamodb update-item --table-name <table-name> \
        --key '{"PK":{"S":"USER#<user-id>"},"SK":{"S":"PROFILE"}}' \
        --update-expression "REMOVE mfa_secret, mfa_enrolled_at" \
        --region <region>

Delete all recovery codes for the user:

    # List existing recovery code rows
    aws dynamodb query --table-name <table-name> \
        --key-condition-expression "PK = :pk AND begins_with(SK, :prefix)" \
        --expression-attribute-values '{":pk":{"S":"USER#<user-id>"},":prefix":{"S":"MFA_RECOVERY#"}}' \
        --region <region>

    # Delete each one (loop in your shell or use batch-write-item)
    aws dynamodb delete-item --table-name <table-name> \
        --key '{"PK":{"S":"USER#<user-id>"},"SK":{"S":"MFA_RECOVERY#<n>"}}' \
        --region <region>

### 3. Tell the user

The user can now log in via OAuth alone. They should immediately
re-enroll a new authenticator (end-user step 3 above) and save
the new recovery codes.

### 4. Audit-log the manual intervention

`AdminAudit` rows are written automatically for routes that go
through the admin API. Manual DDB writes bypass that path —
record the intervention by hand in your incident ticket /
postmortem:

- Operator who performed the unlock
- User affected
- Out-of-band verification method
- Timestamp

This is the only forensic record of the manual unlock. The user's
`SecurityAudit` rows will show the next `LoginSuccess` (no MFA
gate hit) but won't carry the operator's identity.

## Workspace policy: MFA required for everyone

If your workspace requires MFA (`Workspace.mfa_required = true`),
every member must enroll on next login. A user without MFA enrolled
who tries to log in gets a 202 with an `mfaEnrollmentRequired` flag
in the response; the frontend redirects them to the enrollment
page.

A locked-out user in a required-MFA workspace cannot use OAuth
alone after admin force-disarm — they'll be sent back to enroll
on the very next login. That's the intended behavior.

To temporarily disable the workspace-wide requirement (e.g. while
recovering a critical user):

    aws dynamodb update-item --table-name <table-name> \
        --key '{"PK":{"S":"WORKSPACE#<ws-id>"},"SK":{"S":"META"}}' \
        --update-expression "SET mfa_required = :false" \
        --expression-attribute-values '{":false":{"BOOL":false}}' \
        --region <region>

Document this too — toggling workspace MFA is a sensitive change.

## Failure modes worth knowing about

- **User reports the recovery code "doesn't work"**: Common causes:
  (a) The code was already consumed — each is single-use; (b) The
  user is reading a code from a different enrollment (re-enrollment
  invalidates the prior set); (c) The user has a typo. Server logs
  show `MfaRecoveryFailed` audit rows on every failed attempt —
  check `aws logs filter-log-events --log-group <log-group>
  --filter-pattern "MfaRecoveryFailed"` for the user's ID.

- **Rate limit on recovery attempts**: M-E8 gap-002 added a
  budget of `MFA_CHALLENGE_MAX_FAILURES` (default 5) wrong codes
  per handle before the handle is invalidated and the user has
  to restart the login. If a user reports 429s on recovery,
  they've exhausted that budget — they need to restart the OAuth
  flow.

- **Re-enrollment overwrites the prior secret**: A user who
  enrolls again (without disarming first) overwrites the existing
  secret and recovery codes. The old authenticator app stops
  working immediately. This is intended — paired devices are
  always exactly one — but worth noting in support docs.

- **TOTP clock skew**: Authenticator apps assume the device clock
  matches the server within ~30s. A user whose phone clock has
  drifted >60s reports "the code doesn't work even though it's
  fresh". Server logs show `MfaVerify { ok: false }` despite the
  user being adamant the code is current. Fix: ask the user to
  re-sync their phone's time.

## v2 carry-forwards

- **Admin UI for force-disarm**. Currently DDB-only. A future
  admin-console addition could surface this with an explicit
  "I verified identity out-of-band" attestation that captures
  the AdminAudit row automatically.
- **WebAuthn / hardware keys**. Phase 4 ships TOTP + recovery
  codes only. WebAuthn is the right next-gen step but is its
  own milestone.
- **Backup-code regeneration without disarm**. Currently the only
  way to mint a new set of recovery codes is to disarm + re-enroll.
  Replacement-codes-only is a smaller flow that could ship later.
