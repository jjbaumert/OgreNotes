# Daily digest email — dev & ops

The Phase 2 daily-digest worker (M4.1) is shipped: `crates/api/src/digest.rs`
defines an hourly tokio task that, at `EMAIL_DIGEST_HOUR_UTC`, scans every
user, finds those who haven't authenticated in the last 24 hours, and sends
each one a digest of their unread notifications via `EmailService::try_send_digest`.
The worker is spawned from `main.rs:174` regardless of flags, so flipping it
on is a config change — no redeploy of new code.

This runbook is the "how to flip it on and verify" recipe. Send-pipeline
internals (caps, prefs, `last_active_at`) live in [email.md](./email.md);
this page only covers what's specific to the digest path.

## Prerequisites

The digest piggybacks on the same SMTP transport as immediate emails. So
before enabling the digest, make sure single-event email already works
(see [email.md](./email.md) "Local verification"). If immediate emails
aren't reaching MailHog/SES, the digest won't either.

## Local verification

1. Start the dev stack (Mailhog included):
   ```
   docker compose up -d
   ```
2. Pick a current UTC hour you can wait for — the scheduler only fires
   at the top of `EMAIL_DIGEST_HOUR_UTC`. For instant feedback in dev,
   set it to the *next* upcoming hour (e.g. if it's 14:42 UTC right now,
   set `EMAIL_DIGEST_HOUR_UTC=15` and the worker fires within ~18 min)
   or call `digest::send_digests(&state).await` directly from a test
   binary if you don't want to wait.
3. Run the API with digest mode on:
   ```
   EMAIL_ENABLED=true \
   EMAIL_FROM_ADDRESS=dev@ogrenotes.local \
   SMTP_HOST=localhost \
   SMTP_PORT=1025 \
   SMTP_STARTTLS=false \
   EMAIL_DIGEST_ENABLED=true \
   EMAIL_DIGEST_HOUR_UTC=15 \
   cargo run -p ogrenotes-api
   ```
4. Seed an inactive user:
   - Create a user A.
   - As another user B, share a doc with A and post an @mention so A
     accumulates a couple of unread notifications.
   - Make sure A has *not* hit the API in the last 24 hours: easiest is
     to leave A alone after the seed (don't open the app as A) — or
     manually rewind `last_active_at`:
     ```
     aws dynamodb update-item \
       --table-name ogrenote \
       --key '{"PK":{"S":"USER#<a_uid>"},"SK":{"S":"PROFILE"}}' \
       --update-expression 'SET last_active_at = :z' \
       --expression-attribute-values '{":z":{"N":"0"}}'
     ```
5. Wait for the configured hour. The worker logs `digest pass complete
   scanned=… sent=…` at INFO. The MailHog UI at <http://localhost:8025>
   should show one digest email with bullet points per unread notification.

## Production rollout (SES SMTP)

Add to the API deployment env on top of the immediate-email config:

```
EMAIL_DIGEST_ENABLED=true
EMAIL_DIGEST_HOUR_UTC=15           # 15:00 UTC = 11am ET / 8am PT
```

Pick a hour that lines up with your user base's morning. The scheduler
sends one global pass — there is no per-user timezone routing yet.

### Once-per-day guarantee across restarts and replicas

`spawn_scheduler` keeps an in-memory "last sent date" packed as `YYYYMMDD`
and atomically swaps it before kicking off the per-user scan. That guards
against double-fires on a single instance, but **does not** guard against
multiple API replicas: each replica will run its own scan, and the
per-user `EMAIL_DAILY_CAP` is a rate limit on total volume, **not** a
deduplication primitive — N replicas scanning a user concurrently can
each pass their own cap check before any of them increments the counter,
so a user can receive multiple identical digests in the same day until
the cap actually catches up. **Run the scheduler on exactly one node:**
gate `EMAIL_DIGEST_ENABLED=true` on a single instance, or run a
dedicated worker pod with the rest of the fleet at `false`.

### Spot-checking after rollout

- API logs: every fired hour writes one `digest pass complete scanned=N
  sent=M` line at INFO. `scanned-sent` users were skipped (active,
  prefs disabled, no unread, or hit the daily cap).
- Per-skip detail at DEBUG (`digest skipped user_id=… outcome=…`) —
  enable temporarily if a user reports they should have received one.
- SES dashboard: confirm sending volume against expectations.

## What is NOT in this milestone

- **Per-user timezone routing.** Everyone gets the digest at the same
  UTC hour. `User.timezone` would unlock per-user dispatch later.
- **Cross-replica leader election.** Run the worker on one node only.
- **Per-user "last digest sent" timestamps.** The 24-hour inactivity
  filter plus the global once-per-day guard plus the per-user 25/day
  cap together provide the required safety margins.
