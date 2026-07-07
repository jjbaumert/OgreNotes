# Import quotas + ops recipes

Operator-facing recipes for the Phase 5 M-P5 document-import
path. Covers the per-user quotas, the body-size caps, what
gets rejected vs sanitized, and how to investigate a complaint
that an import "didn't work."

## Endpoints in scope

| Route | Method | Purpose | Body limit | Rate limit (default) |
|---|---|---|---|---|
| `/api/v1/documents/import` | POST JSON | Create new doc from Markdown or HTML source | 1 MB | 10 / min / user |
| `/api/v1/documents/:id/import` | POST multipart | Overwrite existing doc with XLSX or CSV | 10 MB | shared `bulk_op` bucket: 20 / min / user |
| `/api/v1/documents/import-job` | POST multipart | Create new doc from DOCX or PDF — **async**, worker-driven | 10 MB | `import` bucket: 10 / min / user |
| `/api/v1/documents/bulk/export` | POST JSON | Zip up to 100 docs → archive | n/a (response is the payload) | 5 / min / user |

Every limit is configurable via env-var without a redeploy
(see "Tuning" below).

## Body-size caps

| Cap | Constant / value | Reject status |
|---|---|---|
| Markdown / HTML import body | `1 MB` (route-scoped `DefaultBodyLimit`) | 413 if the multipart layer rejects; 400 with reason from the handler otherwise |
| Existing-doc XLSX / CSV upload | `MAX_CONTENT_SIZE = 10 * 1024 * 1024` (10 MB) | 400 "File too large: N bytes (max M)" |
| Bulk export ids per request | `BULK_EXPORT_MAX = 100` | 400 "bulk export limit is 100 ids per request; got N" |

1 MB of Markdown source is hundreds of pages of prose; we have
not seen a legitimate request exceed it. 10 MB is more
generous for binary spreadsheet uploads — Excel files compress
the binary XML well, but pictures and embedded objects bloat
quickly.

## Rate limits

Defaults from `crates/common/src/config.rs`:

| Bucket | Default | Env var |
|---|---|---|
| `import` | 10 / min / user | `RATE_LIMIT_IMPORT_PER_MIN` |
| `bulk_op` (delete / restore / move / share) | 20 / min / user | `RATE_LIMIT_BULK_OP_PER_MIN` |
| `bulk_export` | 5 / min / user | `RATE_LIMIT_BULK_EXPORT_PER_MIN` |

The bucket scope is `user-id` (resolved from the JWT), not IP.
A user behind a shared NAT does not steal another user's
quota; an attacker who steals one user's JWT is throttled
against that user's quota, not yours.

Bucket implementation: Redis sorted-set with a 60-second
window. Trip → 429 with the standard `Retry-After` header.

## Sanitization (HTML path)

`crates/collab/src/import.rs` runs HTML imports through a
**two-stage pipeline**:

1. **`ammonia::Builder::default()`** strips:
   - `<script>`, `<iframe>`, `<form>`, `<style>`, `<object>`,
     `<embed>`, `<applet>`, `<meta>`, `<link>` — all dropped
     entirely.
   - Inline event handlers (`onclick`, `onerror`, etc.) — all
     stripped.
   - `javascript:` URLs in `href` and `src` — replaced with
     the empty string.
   - Style attributes (CSS injection risk).
2. **`html5ever` + `markup5ever_rcdom`** parses the sanitized
   tree into our internal `Doc` representation.

**Known v1 limitation, both formats:** inline marks (bold,
italic, links) are dropped. The text content survives; the
mark layer doesn't. The fix is a separate v1.1 piece that
walks the inline tree alongside block parsing.

This matters because users importing a Markdown / HTML article
get readable text but lose all inline formatting. Set
expectations accordingly when a user reports "my imported
document lost the link styling."

## Async import jobs — DOCX / PDF (Phase 6 M-6.5 / M-6.6)

`POST /api/v1/documents/import-job` is the **async** import path,
distinct from the two synchronous routes above: a 50-page DOCX or PDF
can take seconds to convert, so it runs on the `ogrenote-worker`
service (see `runbook/async-worker-ops.md`), not the request thread.

Flow:

1. Client POSTs the file (multipart, `.docx` or `.pdf`, ≤ 10 MB). The
   destination folder is resolved + authorized in the request (Edit
   access, or the caller's home folder).
2. The bytes are staged to `imports/{user_id}/{id}.{docx,pdf}` in S3
   and an `ImportDocx` / `ImportPdf` job is enqueued. The route returns
   `202 Accepted` with `{ "jobId": "…" }`.
3. The worker fetches the blob, parses it, writes a new document, and
   the client's `GET /api/v1/jobs/{id}` poll flips to
   `{ "state": "succeeded", "resultJson": "{\"docId\":\"…\"}" }`
   (or `"failed"` with an error string). The staging blob is deleted
   on either terminal outcome.

**Lossy by design — set expectations.** Both formats are converted to
our block model; fidelity is intentionally low for v1:

- **DOCX:** inline marks (bold/italic/…) dropped; lists import as
  plain paragraphs; headings keep their level; tables map to table
  blocks; images, footnotes, and headers/footers are dropped; nested
  tables are flattened into the enclosing cell.
- **PDF:** **text only.** Tables collapse to their concatenated cell
  text (PDF has no table semantics to recover); images are dropped;
  inline marks are dropped; there is **no paragraph reconstruction** —
  each extracted line becomes one paragraph. A scanned / image-only
  PDF yields an **empty document** (no OCR in v1).

**Size guards.** The 10 MB body cap bounds the upload. The PDF parser
adds an input guard (32 MB) and an extracted-text cap (16 MB), but
`pdf-extract` exposes no ceiling on internal Flate-stream inflation —
the upload cap + per-user rate limit are the mitigation for a
compression-bomb PDF. A genuinely malformed PDF that would otherwise
panic `pdf-extract` is caught and dead-lettered rather than crashing
the worker.

## Failure modes and what to look for

### "I tried to import and got 400"

Most-likely causes, in order:

1. **Wrong format string.** The endpoint takes
   `format = "markdown" | "md" | "html"` for the JSON route;
   `.xlsx | .csv` filename suffix for the multipart route.
   Anything else → 400 with the message naming the supported
   formats.
2. **Body too large.** > 1 MB on the JSON route → either 413
   (multipart layer) or 400 (handler). > 10 MB on the existing-
   doc upload → 400 "File too large: N bytes (max M)".
3. **Markdown that pulldown-cmark refuses.** Rare — the parser
   is generous. If you see a parse panic in logs, file a bug
   with the source attached.
4. **CSV not UTF-8.** The CSV path rejects non-UTF-8 with 400
   "CSV file is not valid UTF-8". A Windows-encoded export
   needs re-saving as UTF-8 in the source application.

Check the server log for the user's request id (TraceLayer
emits `x-request-id` on every response; CloudWatch logs that
header). The handler logs `event_type = "doc_imported"` on
success — if you see the request but no success log, the
sanitizer or parser dropped it.

### "I tried to import and got 429"

The `import` rate-limit bucket trips. The response includes
`Retry-After`. If the user genuinely needs a higher quota,
raise `RATE_LIMIT_IMPORT_PER_MIN` for the deployment — there
is no per-user override surface in v1.

For a one-off elevated quota (e.g. migration from a competitor
where the user is uploading dozens of docs), set
`RATE_LIMIT_IMPORT_PER_MIN=100` in the ECS task definition,
roll the service, do the work, then revert. The next steady
state goes back to 10.

### "The import succeeded but my document is empty / mangled"

Almost always one of:

- **Inline marks dropped.** v1 limitation; see Sanitization
  above. The user's text content is there but bold / italic /
  link styling is gone.
- **`<script>` / `<iframe>` blocks dropped.** Working as
  intended; ammonia strips them.
- **A `<table>` with `<th>` or `<colgroup>`.** v1's html5ever
  walk handles the row/cell structure but does not preserve
  column-group metadata or alignment. Cells survive.
- **Markdown footnotes / definition lists / GFM extensions.**
  pulldown-cmark's default extensions cover task lists,
  tables, strikethrough, and footnotes; anything else is
  treated as a paragraph.
- **A PDF / DOCX import via `/import-job` looks blank or flat.**
  Expected — see "Async import jobs" above. A scanned (image-only)
  PDF has no extractable text and produces an empty doc (no OCR in
  v1); a DOCX/PDF with rich formatting keeps the text but drops marks
  and most structure. If the job never produced a doc at all, it
  likely dead-lettered — check the worker logs / DLQ per
  `runbook/async-worker-ops.md` (status will be `"failed"` on the
  `/jobs/{id}` poll).

Reproduce locally: feed the user's source into a test that
calls `ogrenotes_collab::import::from_markdown(src)` /
`from_html(src)` and inspects the resulting `Doc`. The
`crates/collab/tests/import_tests.rs` file is the template.

### "I tried bulk-export and got 207"

207 from `/bulk/export` means **zero** docs in the request
succeeded. Every entry in the JSON body's `results` array will
have a non-200 status:

- 403 — the caller lost access to the doc between the home-
  page render and the bulk-export click. Refresh the home page;
  the doc shouldn't appear in the next render.
- 404 — the doc was deleted / trashed by another session
  between selection and submit.

207 is **not** "the request was malformed." That's 400 with a
descriptive message.

### "I tried bulk-export with a big batch and got 400"

`BULK_EXPORT_MAX = 100`. The error message tells the user the
exact id count; if they need more, they need to chunk client-
side. Phase 6 async-worker subsystem will lift this.

## Tuning the quotas in production

All limits are env-var-driven. To raise the import rate limit:

```bash
# In scripts/aws-test-config.env or the prod equivalent:
RATE_LIMIT_IMPORT_PER_MIN=50

# Then either:
./scripts/aws-redeploy.sh    # full rolling deploy
# or update the task definition's env in the AWS console
# and force a new ECS task.
```

The new value takes effect once the new task replaces the old
one. No code change.

| Env var | Default | Notes |
|---|---|---|
| `RATE_LIMIT_IMPORT_PER_MIN` | 10 | Per-user. Increases here directly raise the worst-case CPU on the sanitizer path. |
| `RATE_LIMIT_BULK_OP_PER_MIN` | 20 | Shared across delete/restore/move/share. |
| `RATE_LIMIT_BULK_EXPORT_PER_MIN` | 5 | Each call can produce a 100-doc zip; lower than bulk_op because the CPU + memory cost is higher. |

Body-size caps live in code (`MAX_CONTENT_SIZE`,
`BULK_OP_MAX`, `BULK_EXPORT_MAX`). Changing them is a code
change + redeploy.

## Metrics + alarms

Every import emits:

```
counter: doc.imported_total{format="markdown"|"html"|"xlsx"|"csv"}
log:     event_type="doc_imported" doc_id=<id> format=<format>
```

Every bulk export emits:

```
counter: doc.bulk_export_total{requested=<N>, succeeded=<N>}
```

CloudWatch dashboards under `OgreNotes/Imports` track:

- Import success rate per format.
- Import latency p95 (sanitizer + parser).
- 429 / 413 rate (quota or size pressure).

The SLA alarms in `runbook/sla-alarms.md` include an "import
failure rate > 5% for 10 min" line. When it pages, start with
the most-recent `event_type="doc_imported"` log entries — if
they suddenly stop, the sanitizer is probably panicking on
some new input shape.

## Common operator tasks

### Bulk-promote a user past the rate limit

There is no per-user override surface in v1. Either:

1. Have the user space out the imports (10/min is one every
   6 seconds — slow for a click-fest, fine for a script).
2. Temporarily raise `RATE_LIMIT_IMPORT_PER_MIN` deployment-
   wide. Revert when done.

### Confirm a failed import did not leak partial state

If the sanitizer / parser failed after the doc-meta row was
written but before the snapshot was saved, you'd see an
orphaned `DocumentMeta` row with no S3 snapshot. The handler
writes meta + snapshot in one repository call
(`state.doc_repo.create(&meta, &snapshot).await?`) so this
should not happen — but if a user reports an "empty document"
that they didn't intentionally create, check the audit log.
The handler emits `event_type="doc_imported"` only on success;
absence of that log line for a doc-id that exists is the
signal.

### Investigate a "my import had a script tag and it ran"

It didn't. ammonia strips `<script>` before html5ever sees
it. If a user is convinced otherwise, look at the resulting
HTML export (`GET /api/v1/documents/:id/export/html`) — the
`<script>` tag will not be present. The most-common false
positive is **a `<code>` block containing literal `<script>`
text**, which renders as visible code in the document but is
not executed. Working as intended.

## v2 carry-forwards

- **Inline mark preservation.** Bold / italic / link inside
  imported Markdown + HTML, blocked by the inline-mark walk
  in `import.rs`.
- **DOCX / PDF import.** Both deferred to Phase 6; need the
  async-worker subsystem (parse is slow and memory-heavy).
- **Per-user quota overrides.** No surface today; the admin
  console could host this in Phase 6.
- **Streaming uploads.** Today's body limits are buffered in
  memory; streaming the body into the parser would let us
  lift the 10 MB cap on the existing-doc upload path.
- **Resumable bulk export.** Today's bulk export is a single
  HTTP response. The async-worker subsystem could surface a
  job-id + poll endpoint for > 100-doc batches.
