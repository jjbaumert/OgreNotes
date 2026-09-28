# Self-hosting OgreNotes

One home server runs the app. Your documents live in AWS: DynamoDB for
records and S3 for files. Pay-per-request pricing makes that about a dollar
or two a month at personal scale.

```
 browser ──https──▶ Caddy ──▶ api ─┬─▶ Redis (local: sessions, queue, rate limits)
            (TLS, dynamic DNS)     ├─▶ DynamoDB  ┐
                 worker ───────────┴─▶ S3        ├─ AWS, over the internet
                                       SSM (secrets) ┘
 search index: local disk, snapshotted to S3 hourly
```

Everything here runs from `deploy/selfhost/`.

## What you need

- A Linux machine with Docker (Compose v2), the AWS CLI v2 and `openssl`.
  Keep its clock in sync (NTP on), because AWS rejects requests signed
  with a skewed clock.
- An AWS account, and a domain whose DNS is a **Route 53 hosted zone**. Caddy
  keeps an A record there pointed at your home IP (dynamic DNS) and gets its
  TLS certificate through Route 53 too.
- Your router forwarding TCP **443** (and UDP 443 for HTTP/3) to the machine.
  Port 80 isn't needed.
- A GitHub OAuth app for sign-in (GitHub → Settings → Developer settings →
  OAuth Apps). Set its callback URL to
  `https://<your domain>/api/v1/auth/callback`.

## First-time setup

1. Copy the three example files and fill them in:
   - `.env` needs the domain.
   - `ogrenotes.env` needs the region, the table prefix, the bucket name,
     the GitHub OAuth client ID and your admin email. Leave the app's AWS
     keys blank for now.
   - `caddy.env` needs your ACME email and the DNS zone and record.

   Then run `chmod 600 ogrenotes.env caddy.env`.

   ```sh
   cp .env.example .env
   cp ogrenotes.env.example ogrenotes.env
   cp caddy.env.example caddy.env
   ```

2. Run the one-time AWS setup with your **admin** AWS profile:
   ```sh
   AWS_PROFILE=admin ./setup.sh
   ```
   It sets up:
   - **The S3 bucket:** private, encrypted, versioned (old versions kept
     30 days), with CORS for your domain.
   - **The DynamoDB table:** point-in-time recovery and deletion
     protection on.
   - **SSM secrets:** it generates `JWT_SECRET` and `MFA_ENCRYPTION_KEY`,
     and asks for the GitHub client secret.
   - **IAM:** it fills in the two IAM policies and prints the commands to
     create the two IAM users.

3. Create the two IAM users with the commands `setup.sh` printed:
   - `ogrenotes-app` can use only this table, this bucket and these SSM
     paths. It is also explicitly denied deleting the table or bucket.
   - `ogrenotes-dns` can only edit your Route 53 zone.

   Put each user's access key in `ogrenotes.env` and `caddy.env`
   respectively. **Only these keys live on the machine.** Every other
   secret is read from SSM at startup.

4. Start everything:
   ```sh
   docker compose up -d --build
   docker compose logs -f api caddy
   ```
   The first start creates a search index from scratch and does nothing else
   interesting. Caddy updates the DNS record and gets a certificate. Then
   open `https://<your domain>` and sign in with GitHub.

## Checking it works

- `https://<domain>/api/v1/status` returns `{"storage":"ok",…}`.
- You can sign in, create a document, type, and see the save badge say
  **Saved**. Reloading keeps the text, and images upload and display (that
  exercises S3 CORS).
- After an hour, the logs show `search index backed up`.
- After a few days, check the logs for `blob_reconcile dry-run: would delete`.
  If what it lists looks right (only orphans), set
  `BLOB_RECONCILE_DRY_RUN=false` in `ogrenotes.env` and run
  `docker compose up -d`.

## When the internet drops

This is expected, and the app handles it:

- **Server banner.** The server notices within about 15 seconds. The app
  shows *"Can't save right now"*, and API calls fail at once rather than
  hang.
- **Unsaved edits.** Edits made in open documents show **Not saved**. They
  are kept in the browser tab and on the server. The browser warns you
  before you close a tab holding unsaved edits.
- **Recovery.** When the connection returns, the banner clears, the edits
  are re-sent and saved, and the badge returns to **Saved**.
- **Pages opened during an outage.** They show a notice and wait, rather
  than sending you to the sign-in page.
- **Background jobs** (imports) wait out the outage without using up their
  retries.

If the server itself restarts while the connection is down, it waits for
SSM (about 30 seconds of retries), then exits and is restarted by Docker
until the connection returns.

## Upgrading

```sh
git pull
docker compose up -d --build
```

## Backups and restore

| What | Backup | Restore |
|---|---|---|
| Documents, users, sharing (DynamoDB) | Point-in-time recovery, 35 days | `aws dynamodb restore-table-to-point-in-time --source-table-name <prefix>ogrenote --target-table-name <newprefix>ogrenote --restore-date-time <time>`, then set `DYNAMODB_TABLE_PREFIX=<newprefix>` and `docker compose up -d` |
| Snapshots, images (S3) | Versioning, 30 days of old versions | Copy the wanted version back: `aws s3api list-object-versions`, then `aws s3api copy-object … ?versionId=…` |
| Search index | Snapshot to `search-index/snapshot.zip` every hour | Automatic. An empty index volume is restored from the snapshot on start, then brought up to date. An admin can also rebuild it from scratch: `POST /api/v1/admin/search/reindex` |
| Secrets (SSM) | Keep a copy of the generated values somewhere safe | `aws ssm put-parameter` |
| Redis | Not needed (its data volume survives restarts) | It holds only transient state: rate limits, one-time login handles, and the job queue. Losing the queue means re-starting any import that was in progress |

Try a restore once, on a scratch prefix, before you need it.

## Moving to a new machine

Copy `deploy/selfhost/*.env` and `.env`, then run `docker compose up -d
--build`. The data is already in AWS, and the search index restores itself
from S3.

## Costs and limits

- Set an **AWS Budget** alert (for example at $5/month) so a bug or runaway
  job can't surprise you.
- Around 100 GB/month of S3 downloads are free; past that, about $0.09/GB.
- Every database call crosses the internet, about 20–100 ms each depending
  on your region. Pick the AWS region nearest you.

## Security notes

- The image is built **without** the dev-login endpoint
  (`CARGO_FEATURES=xlsx,docx,pdf`), and `DEV_MODE` is forced to `false`.
- Rotating `JWT_SECRET` in SSM signs everyone out on the next restart.
- Rotate the two IAM access keys periodically:
  `aws iam create-access-key`, update the env file, run
  `docker compose up -d`, then delete the old key.
