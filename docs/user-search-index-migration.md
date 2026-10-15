# User search index migration (#35)

Substring user search queries DynamoDB n-gram postings scoped to the caller's
workspaces. Profile creation, name edits, and workspace grants write postings
before changing the authoritative record. Name edits and grants advance the
same conditional profile revision, so a concurrent change retries and covers
the resulting name and workspace combination. Member grants and removals
atomically update the canonical membership and a reverse membership edge on
the user partition. Search reads that edge and validates both caller and
candidate membership, profile, postings, and readiness with strongly
consistent base-table reads. Old postings are retained and filtered against
current source records; they can be compacted by a future offline rebuild.

Existing tables need a writer-first rollout:

1. Deploy the new API and worker code with
   `USER_SEARCH_INDEX_READS_ENABLED=false` (the default) to every instance.
   This temporary mode keeps the old bounded scan and its known sparse and
   crowded-result limitations while all writers begin maintaining postings.
2. Once no old writer instances remain, run the new image's backfill binary
   against the live table. The backend image includes the binary:

```sh
DYNAMODB_TABLE_PREFIX=ogrenotes AWS_REGION=us-east-1 \
  backfill_user_search_index
```

Set the table prefix and region for the target environment. The command scans
existing `PROFILE` rows to write global postings, then scans canonical member
rows to write workspace postings and reverse membership edges. It marks the
index complete only after both passes succeed. It is idempotent and safe to
rerun after an interruption. New writers abort a source change if its posting
write fails, so concurrent writes cannot make a completed index incomplete.

3. Verify `META#USER_SEARCH_INDEX` / `COMPLETE` has `state=complete`. Set
   `USER_SEARCH_INDEX_READS_ENABLED=true` on every API instance. New writes
   can continue during backfill because they prewrite postings and transact
   membership edges. Old writers must already be gone because they do not
   participate.

When indexed reads are enabled, a missing or incomplete marker makes
`/users/search` return an error instead of an incomplete result. Rerun the
command if readiness fails. The request-time scan disappears once the read
flag is enabled.

The index writes distinct one-, two-, and three-character grams. At the
ASCII ingress caps of 320 email characters and 256 name characters, a new
user or workspace grant writes at most 1,722 postings per scope. A rename
writes only name grams, at most 765 per scope, plus the global scope.
Unicode lowercase expansion can increase these counts, but the work remains
linear in normalized field length. Typical names and emails have far fewer
distinct grams. Old name postings remain so
concurrent renames cannot delete a currently needed posting; source checks
ignore them. Track posting count and write capacity as the directory grows.
Bounded historical-posting compaction needs a separately reviewed
generation-aware design; unconditional deletes are unsafe when names cycle.
