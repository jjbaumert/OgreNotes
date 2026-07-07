# Email notifications — dev & ops

M4 ships real email delivery for document share invites, direct replies,
@mentions, and chat messages. Delivery uses SMTP — MailHog locally, SES
SMTP in prod.

## Local verification

1. Start MailHog (and the rest of the dev stack):
   ```
   docker compose up -d
   ```
2. Run the API with email turned on, pointing at MailHog:
   ```
   EMAIL_ENABLED=true \
   EMAIL_FROM_ADDRESS=dev@ogrenotes.local \
   SMTP_HOST=localhost \
   SMTP_PORT=1025 \
   SMTP_STARTTLS=false \
   cargo run -p ogrenotes-api
   ```
3. Log in as two users and have user A share a document with user B.
4. Open the MailHog UI at <http://localhost:8025>. The share email should
   appear within a second.

If no email appears, check the API logs for `email send failed` (send
errors) or `email send outcome` at `debug` (a `Skipped*` outcome means
the pipeline filtered the send — see the five possible reasons below).

## Production config (SES SMTP)

Set these env vars on the API deployment:

```
EMAIL_ENABLED=true
EMAIL_FROM_ADDRESS=<verified SES identity>
SMTP_HOST=email-smtp.<region>.amazonaws.com
SMTP_PORT=587
SMTP_USERNAME=<SES SMTP user>
SMTP_PASSWORD=<SES SMTP password>
SMTP_STARTTLS=true
EMAIL_DAILY_CAP=25            # defaults to 25 if unset
```

The SES SMTP credentials are NOT the same as IAM access-key secrets —
generate them in the SES console under "SMTP settings" → "Create SMTP
credentials".

## Send-or-skip decision pipeline

Each notification runs through these gates in order. The first one that
rejects returns the corresponding `SendOutcome` and no further work
happens:

1. **`SkippedDisabled`** — `EMAIL_ENABLED=false`. Nothing is looked up.
2. **`SkippedNoAddress`** — recipient has no email (should be impossible
   for OAuth users; defensive).
3. **`SkippedPrefs`** — recipient's `NotifEmailPref` filters out this
   `NotifType` / `is_direct` combination. Defaults:
   - `MentionsOnly` (default): emails on `Mentioned`, `Shared`, or any
     event with `is_direct=true`.
   - `All`: emails on every event.
   - `Disabled`: emails nothing.
4. **`SkippedActive`** — recipient authed against the API within the last
   5 minutes. The in-app notification is enough.
5. **`SkippedCap`** — recipient has already received `EMAIL_DAILY_CAP`
   emails today (UTC day). Resets at midnight UTC.
6. **`Sent`** — the email hit SMTP successfully.

## `last_active_at`

Written by `ActivityTracker` (in `crates/api/src/middleware/activity.rs`)
from every authenticated request, debounced to one `UpdateItem` per user
per 5 minutes. This matches the "active in-app" suppression window so the
two signals agree on what "active" means.

If you suspect the writer is misbehaving, read the field directly:

```
aws dynamodb get-item \
  --table-name ogrenote \
  --key '{"PK":{"S":"USER#<uid>"},"SK":{"S":"PROFILE"}}' \
  --projection-expression last_active_at
```

## Daily cap

Per-user counter rows under `PK=USER#<uid>`, `SK=EMAIL_CAP#<yyyy-mm-dd>`
(UTC). Incremented atomically inside `EmailCapRepo::increment_if_under_cap`
with a `count < :cap` condition. Hitting the cap is a soft failure — the
server just logs `SkippedCap` and the in-app notification still lands.

Reset today's counter for a user (debugging only):

```
aws dynamodb delete-item \
  --table-name ogrenote \
  --key '{"PK":{"S":"USER#<uid>"},"SK":{"S":"EMAIL_CAP#YYYY-MM-DD"}}'
```

## Daily digest

The hourly digest worker (M4.1) is implemented and ships behind
`EMAIL_DIGEST_ENABLED=false`. See [email-digest.md](./email-digest.md)
for the enable-and-verify recipe.
