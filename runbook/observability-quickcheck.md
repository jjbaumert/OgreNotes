# Observability quick-check

Phase 1 of `design/observability.md`. Operator playbook for
pulling client + server metric pairs for a doc / time window
when a user reports a collab bug. Used until the Phase 3
discrepancy_map agent and dashboard tile land.

## When to use this

Reach for this runbook when a user reports any of:

- "I typed but other tabs / refresh don't show my edits."
- "Edits show up live in the other window but vanish on refresh."
- "The doc seems to revert on its own."
- Anything where the symptom requires telling client-sent from
  server-received from server-persisted apart.

The cheap signal-discrimination tool is the pair of
`client.*` and server-side counters wired up in Phase 1. The
discrepancy between them tells you which stage of the pipeline
silently dropped the keystroke.

## The metric pairs

| Question | Client counter | Server counter | Discriminator |
|---|---|---|---|
| Did the editor produce a CRDT diff for the keystroke? | `client.editor.transactions_total` | n/a | If this is 0 while user reports typing, the editor state isn't reaching `apply_and_notify` — UI/contenteditable bug, not collab |
| Did yrs see the diff? | `client.collab.observe_fired_total` | n/a | If editor txns > observe fired, the editor changed but yrs saw no diff (`sync_model_to_ydoc` produced no ops). Front-end bug. |
| Did the client try to send? | `client.ws.frames_sent_total` + `client.ws.send_errors_total` | `ws.messages_total{type=update}` | Sent (ok or err) much greater than server-received = frames are leaving the client but not arriving. Network / WS-state bug. |
| Did the server accept? | n/a | `ws.update_decode_failures_total` + `ws.update_apply_failures_total` | Non-zero = frame arrived but couldn't be applied. Distinct from "didn't arrive." |
| Did the server persist? | n/a | `ws.update_persist_failures_total` + `dynamo.write_failures_total{op=append_update}` | Non-zero = applied + broadcast + lost. The historical silent-failure-on-refresh case before #38. |
| Did the broadcast reach a peer? | `client.ws.remote_frames_received_total` (on the *other* tab) | `ws.send_errors_total{side=primary}` | Broadcast send errored = the recipient's WS task dropped its receiver between `broadcast` and the send. Peer-gone-mid-broadcast. |
| Did the client send something the server didn't recognize? | n/a | `ws.unknown_msg_type_total{direction=recv}` | Non-zero = version-skew. The byte value goes in the `tracing::warn` log (matched on `event_type = "ws_unknown_msg_type"`). |
| Old client / no `client_version` on WS-token POST? | n/a | `server.dto.field_absent_total{route=/documents/:id/ws-token,field=client_version}` and `ws.skew_client_version_mismatch_total{result=absent}` | High count = cached WASM bundles from a prior deploy still active. Expected for ~hours after a deploy. |

## CloudWatch CLI snippets

Adjust `LOG_GROUP` and `METRIC_NAMESPACE` to your stack. The
existing dashboard already groups under the `OgreNotes`
namespace.

### Recent skew at a glance

```bash
# Are any unknown message types arriving right now?
aws cloudwatch get-metric-statistics \
  --namespace OgreNotes \
  --metric-name ws.unknown_msg_type_total \
  --dimensions Name=direction,Value=recv \
  --start-time "$(date -u -d '15 minutes ago' +%FT%TZ)" \
  --end-time "$(date -u +%FT%TZ)" \
  --period 60 --statistics Sum
```

### Client vs server: did the keystrokes leave the client?

```bash
# Last 5 minutes
WINDOW_START="$(date -u -d '5 minutes ago' +%FT%TZ)"
WINDOW_END="$(date -u +%FT%TZ)"

for metric in client.ws.frames_sent_total client.ws.send_errors_total; do
  echo "--- $metric ---"
  aws cloudwatch get-metric-statistics \
    --namespace OgreNotes --metric-name "$metric" \
    --start-time "$WINDOW_START" --end-time "$WINDOW_END" \
    --period 60 --statistics Sum
done

aws cloudwatch get-metric-statistics \
  --namespace OgreNotes --metric-name ws.messages_total \
  --dimensions Name=type,Value=update \
  --start-time "$WINDOW_START" --end-time "$WINDOW_END" \
  --period 60 --statistics Sum
```

The discriminator: sum `client.ws.frames_sent_total` and compare
to sum `ws.messages_total{type=update}`. A meaningful gap means
frames are leaving the client but not arriving server-side — the
shape of the May 30 2026 edits-not-persisting bug.

### Forensics for an unknown-byte report

CloudWatch metrics drop the offending byte value (256-cardinality
dimensions are expensive). The byte value is in the `tracing::warn`
log line. Pull recent occurrences:

```bash
aws logs filter-log-events \
  --log-group-name "$LOG_GROUP" \
  --filter-pattern '{ $.event_type = "ws_unknown_msg_type" }' \
  --start-time "$(date -u -d '1 hour ago' +%s)000"
```

Each match carries `direction`, `byte` (as `0xNN`), and
`payload_len`.

## Client-side activation for a specific user

If the deployed metrics aren't enough and you need per-keystroke
log lines from one user's session, ask them to reload the doc
with the URL flag appended:

```
https://ogrenotes.example.com/d/<doc_id>/<slug>?debug=collab,ws&level=debug&capture=1
```

This activates the always-compiled-in client logger in the
release WASM bundle, scoped to the `collab` and `ws` categories
at `debug` level. The `capture=1` also fills a 1000-entry ring
buffer that a future support-bundle endpoint (Phase 2) will
upload. Today the ring buffer is local to the tab; if you need
it, ask the user to paste the output of `debug.dumpRingBuffer()`
from the JS console (the wasm-bindgen export will land in
Phase 2 alongside the operator-pushed config field on
`/users/me`).

`?debug=all` matches every category. Removing the flag (next
URL navigation) reverts to no emission.

## What's NOT in Phase 1

- A CloudWatch dashboard tile pairing each client + server
  counter on one axis — Phase 3.
- The `aws-diagnostic` agent's `skew-report` knowledge module
  — Phase 3.
- Operator-pushed `client_log_config` on `/users/me` so the
  URL flag isn't the only activation path — Phase 2.
- `correlation_id` propagation from a keystroke to the DDB
  row — Phase 2.

For those, the answer is "wait for the corresponding phase to
land." For everything else, the metric pairs in the table above
should answer the question with one or two `aws cloudwatch`
calls.

## When you're stuck

If the metric pairs all balance and the user still reports the
bug, escalate to either:

- The `aws-diagnostic` agent with the doc id and a tight time
  window — it can read the live DynamoDB rows and the snapshot
  state which the metrics can't.
- The `frontend-doctor` agent driving a headless browser through
  the reported reproduction, which captures the WS message
  payloads Firefox's HAR doesn't.
