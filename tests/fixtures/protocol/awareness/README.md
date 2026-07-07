# Awareness Wire-Format Fixtures

These JSON files are the **shared contract** for the awareness message
payload that moves over the WebSocket between the frontend and the backend.

Both sides consume these exact bytes via `include_str!` in their test
suites:

- Backend: `crates/collab/src/awareness.rs` tests decode each fixture into
  `AwarenessState`, re-encode, decode again, and assert field preservation
  — the exact pass-through path that the server uses when validating a
  client-submitted awareness message and rebroadcasting it.
- Frontend: `frontend/src/collab/ws_client.rs` tests decode each fixture
  into `AwarenessPayload`, re-encode, decode again, and assert the same
  invariant.

## Contract

Adding a new on-the-wire field to either side requires:

1. Adding it to **both** the backend `AwarenessState` and the frontend
   `AwarenessPayload`.
2. Adding or updating a fixture that populates it.

A pull request that adds a field to one side only will fail its own test
(or the sibling side's test) because a fixture will no longer round-trip
cleanly. This is the point.

## Why fixtures instead of a shared crate

The frontend is intentionally excluded from the cargo workspace so its
WASM target doesn't interfere with native builds. A shared wire-type
crate is the stronger long-term fix; these fixtures are the near-term
replacement.

## Current fixtures

| File | Scenario |
|---|---|
| `cursor-only.json` | User focused at a position, no active selection. Block-relative encoding. |
| `selection.json` | User has an active selection spanning two blocks. All block-relative fields populated. |
| `typing-indicator.json` | User is typing in a comment thread; no editor cursor. |
| `legacy-absolute.json` | Old-format payload with only absolute `cursor_pos` (no block fields). Exercises the backwards-compat path. |
| `no-presence.json` | Connected user with no focus / no cursor at all. Baseline identity-only shape. |
