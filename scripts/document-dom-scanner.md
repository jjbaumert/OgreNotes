# Persisted document DOM scanner

`scan_document_dom` reads DynamoDB metadata/update rows and S3 snapshots to check persisted document-tree structure. It does not write remote data, open a collaboration room, repair documents, or validate rendered HTML. It checks document and presentation trees; spreadsheets and chats are reported as skipped. Attribute semantics and block-ID uniqueness are outside this structural scan.

Review any supplied reports before starting another scan. Record whether evidence is a real environment observation, a fixture, synthetic, or of unknown provenance. A fixture result does not establish production validity. Session prohibitions on builds, tests, or cloud calls still apply; existing artifacts may be sufficient to answer the question.

## Select the environment and documents

Run from the repository root, using the intended AWS credentials/profile. Choose either repeated `--doc-id ID` arguments or `--all`; there is no implicit all-document scan. Use `--all` only when an inventory scan of that environment is authorized.

```bash
mkdir -p .ship/dom-diagnosis
dom_report_dir="$(mktemp -d .ship/dom-diagnosis/run.XXXXXX)"
scan_status=0
cargo run --locked -p ogrenotes-collab --features replay --bin scan_document_dom -- \
  --doc-id "${DOM_DOC_ID:?Set an authorized document ID}" \
  --table "${DOM_TABLE:?Set the full DynamoDB table name}" \
  --bucket "${DOM_BUCKET:?Set the authorized S3 bucket}" \
  --region "${AWS_REGION:-us-east-1}" \
  --history \
  > "$dom_report_dir/scan.ndjson" \
  2> "$dom_report_dir/scan.stderr" || scan_status=$?
printf 'scanner exit: %s; evidence: %s\n' "$scan_status" "$dom_report_dir"
```

Set the three `DOM_*` variables before running. Omit `--history` for current snapshot/tail inspection only. `--include-deleted` includes soft-deleted documents; otherwise they are explicitly skipped.

| Setting | Resolution |
|---|---|
| Table | `--table` is the full name. Otherwise `DYNAMODB_TABLE_PREFIX` plus the canonical suffix `ogrenote`. |
| Bucket | `--bucket`, otherwise `S3_BUCKET`. |
| Region | `--region`, then `AWS_REGION`, then `us-east-1`. |
| Credentials | Normal AWS SDK credential/profile resolution. |
| Endpoints | Normal SDK overrides, including `AWS_ENDPOINT_URL_DYNAMODB`, `AWS_ENDPOINT_URL_S3`, and shared `AWS_ENDPOINT_URL`. |

A local stack requires local endpoints, local credentials, and its own initialized table/bucket. Production requires the intended account/profile and environment-specific table/bucket. Check endpoint overrides before running: switching credentials alone does not switch the target endpoint. The header records resolved region, table, bucket, and redacted endpoint overrides; it does not establish the authenticated AWS account. Do not fall back to another environment after access failure.

## Bounds

All numeric limits must be positive. Reaching a bound leaves verification incomplete, even if the inspected prefix contains valid states or known violations.

| Flag | Default | Scope |
|---|---:|---|
| `--max-bytes` | 33,554,432 (32 MiB) | Aggregate current snapshot plus pending-update payloads per document. History has a **separate aggregate allowance of the same size**, shared across historical snapshots, not per object. |
| `--max-rows` | 10,000 | Pending/history row cap per document. |
| `--max-nodes` | 1,000,000 | Cumulative node visits during a current replay; a separate allowance covers the history sweep. Repeated inspections consume visits. |
| `--max-depth` | 128 | Tree traversal depth. |
| `--max-documents` | 100,000 | Inventory document cap. |
| `--max-pages` | 10,000 | Inventory scan page cap. |
| `--max-memory-bytes` | 536,870,912 (512 MiB) | Isolated decode/replay child address-space allowance, enforced with Unix `RLIMIT_AS`; not a parent-process RSS ceiling. |
| `--timeout-seconds` | 45 | AWS operation deadline including retries; attempts are capped at the smaller of this value and 15 seconds. Separate deadlines cover each complete S3 body read and the isolated decode/replay worker; worker CPU time is also limited. |
| `--page-size` | 100 | DynamoDB evaluated rows per page; range 1 through 1,000. |

Narrow the scope or report the limit before increasing live scan cost. The S3 body deadline starts separately because the SDK GetObject operation completes at response headers; neither deadline is a whole-scan time limit. The header records `timeout_seconds`. A timeout is an operational/incomplete result, not a valid document. Decoding/replay runs in one isolated child at a time. Worker failure, out-of-memory termination, deadline expiry, or unavailable OS limit enforcement fails closed as error/incomplete, never valid. The byte budget controls parent-fetched payloads; it is not a parent RSS ceiling. Failed historical body reads conservatively exhaust the remaining aggregate history byte allowance, so partial failed transfers cannot repeatedly bypass it. Read-only scans still consume AWS requests and local decoding work.

## Read the report

Stdout is NDJSON with `header`, `document`, `history`, and `summary` records, plus inventory `error` records when needed. Stderr carries command/build failures. Records contain structural paths/rules and provenance, not raw document text. Treat document/user IDs and object keys as potentially sensitive when sharing evidence.

A document record separates the stored `snapshot` check from `current` (snapshot plus the retained pending tail). It includes applied/read update counts, stability checks, byte/object identities, and `first_invalid_update` for the first invalid replay observation after a verified valid state. That valid state may occur later in the replay when an initially incomplete snapshot's dependencies resolve; it need not be the starting snapshot. `--history` also examines retained historical S3 snapshots, including objects without indexed snapshot rows.

`invalid_replay_observations` counts observed invalid replay states independently of whether `first_invalid_update` can be attributed. The summary's `intermediate_invalid` counts documents with one or more such observations, not individual transitions. A missing transition record does not erase known invalid observations.

- **Current:** report final persisted validity only when the full tail was replayed and checked. An incomplete `current` check has status `inconclusive`; retained violation details describe observed evidence, not proof of the unverified final state. An unreplayed suffix makes the current result inconclusive. A resource-limit observation remains incomplete even if a later inspected state appears valid.
- **Historical/intermediate:** a retained snapshot or replay intermediate can be invalid while the fully replayed current state is valid. Keep these findings separate; a repaired final state does not erase a historical finding.
- **Errors/inconclusive:** inaccessible/missing objects, unresolved CRDT dependencies, unstable reads, or exhausted bounds do not establish validity. Read `status`, `complete`, `read_stable`, errors, and summary counts together. A skipped document was not validated.

| Exit | Meaning |
|---|---|
| `0` | Completed clean scope, subject to explicitly skipped types/deleted documents. |
| `1` | Completed scan detected invalid current, snapshot, historical, or intermediate structure. It does not necessarily mean the current document is invalid. |
| `2` | Operational error or incomplete verification. This takes precedence when invalidity and incomplete evidence coexist. |

A build failure, signal, truncated stream, or missing summary is incomplete execution regardless of any earlier valid record. Preserve the exact command, source revision, timestamps, stderr, exit status, and report. Unique output directories keep repeat scans from replacing earlier evidence.

## Evidence limits and diagnosis

The scan compares metadata, update identities, and S3 object identities to detect changes during reading. It is not an atomic DynamoDB/S3 snapshot, a backup, or an observation of unsaved live-room state. A stable result is scoped to the observations made.

Retain payload SHA-256 hashes and available S3 keys/version IDs/ETags. Historical numeric snapshot keys may have been overwritten; a snapshot version alone is not immutable byte identity. An orphan object might be an abandoned candidate that was never active. History inspection does not recover deleted S3 object versions.

Compaction prunes `UPDATE#` rows, so the first invalid retained observation is not necessarily the original corrupting edit. A snapshot's user may only have serialized or restored existing bytes. An update's user/client version identifies retained sender provenance, not proof of the original node author or a unique Git commit. Missing history can make original attribution impossible. If `first_invalid_update` is absent or null, report it as unavailable; snapshot authorship cannot substitute for the missing transition.

For a scoped mechanism investigation, use the repository skill [ogrenotes-dom-diagnosis](../.agents/skills/ogrenotes-dom-diagnosis/SKILL.md) and its [evidence/source map](../.agents/skills/ogrenotes-dom-diagnosis/references/evidence-map.md). Correlate retained operations, import manifests, client metadata, bounded logs, and writer source; reproduce a suspected mechanism locally before asserting causality only when session permissions allow that execution. Otherwise describe the proposed reproduction and retain the unverified classification. Diagnosis does not authorize repair, compaction, deletion, import retries, or broader production scans.

Implementation and authoritative CLI help: [args.rs](../crates/collab/src/bin/scan_document_dom/args.rs), [main.rs](../crates/collab/src/bin/scan_document_dom/main.rs), [cloud.rs](../crates/collab/src/bin/scan_document_dom/cloud.rs), and [inspect.rs](../crates/collab/src/bin/scan_document_dom/inspect.rs).
