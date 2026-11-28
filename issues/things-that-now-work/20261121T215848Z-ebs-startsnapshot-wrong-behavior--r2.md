Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba9169dcb3a6b6883b8d60843b0b5f765354b232

# A ClientToken retry re-renders the snapshot's CURRENT status instead of replaying the original answer

_Filed: 2026-11-21T21:58:48Z_
_Operation: StartSnapshot | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba9169dcb3a6b6883b8d60843b0b5f765354b232`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartSnapshot` stores nothing of the answer it gave: `StartSnapshotIdempotencyRecord` keeps only the snapshot id and a request fingerprint. On a retry carrying the same `ClientToken` (and a matching fingerprint), the handler looks the snapshot up and re-renders a FRESH response from the live record — `response_for(snapshot)` reads `snapshot.status` at the instant of the retry.

Every member of that response is immutable except `Status`, and `Status` is exactly the one the snapshot's life changes: `CompleteSnapshot` moves the record to `completed` (or to `error` on a mismatched `ChangedBlocksCount`), and `apply_due_transitions` moves a pending snapshot to `error` when its `Timeout` falls due. So a retry issued after any of those returns a 201 `StartSnapshot` body whose `Status` is `completed` or `error` — not the result the original call produced, which by the handler's own construction was always `pending`.

That is the opposite of what the member is documented to do ("return the result from the original successful request"), and it makes a retried create report a snapshot state no `StartSnapshot` answer can legitimately carry: the operation's own doc fixes the status of a freshly created snapshot ("The new snapshot enters the `pending` state after the request completes"), which is why the create path hard-codes `SnapshotStatus::Pending`. A client that retries a timed-out create therefore cannot distinguish "my create was replayed" from "a different, already-finished snapshot", and anything that polls the create's answer for `pending` sees a value the first answer never had.

The existing idempotency test retries only immediately, while the snapshot is still `pending`, so the divergence is unpinned.

## Evidence

MODEL (the rule) — `smithy_query(query="doc",shape="com.amazonaws.ebs#StartSnapshotRequest",member="ClientToken")`:
"A unique, case-sensitive identifier that you provide to ensure the idempotency of the request. Idempotency ensures that an API request completes only once. With an idempotent request, if the original request completes successfully. The subsequent retries with the same client token return the result from the original successful request and they have no additional effect."

MODEL (what a create's answer reports) — `smithy_query(query="doc",shape="com.amazonaws.ebs#StartSnapshot")`:
"Creates a new Amazon EBS snapshot. The new snapshot enters the pending state after the request completes."

CODE — the replay path re-renders from the live record, services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:310-316:
"                let snapshot_id = record.snapshot_id.clone();
                let snapshot = store.snapshots.get(&snapshot_id).ok_or_else(|| {
                    no_state_message(format!(
                        \"ClientToken {token} names snapshot {snapshot_id}, which is no longer stored\"
                    ))
                })?;
                return Ok(response_for(snapshot));"

CODE — `response_for` reads the status AS IT IS NOW, services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:554-559:
"fn response_for(snapshot: &Snapshot) -> StartSnapshotOutput {
    StartSnapshotOutput {
        description: snapshot.description.clone(),
        snapshot_id: Some(snapshot.snapshot_id.clone()),
        owner_id: Some(snapshot.owner_id.clone()),
        status: Some(snapshot.status.as_wire().to_string()),"

CODE — the status the ORIGINAL answer always carried, with the model sentence quoted beside it, services/service-com-amazonaws-ebs-ebs/src/ops/start_snapshot.rs:421-422:
"            // \"The new snapshot enters the pending state after the request completes.\"
            status: SnapshotStatus::Pending,"

CODE — nothing of the original answer is stored, services/service-com-amazonaws-ebs-ebs/src/state.rs:314-318:
"#[derive(Debug, Clone)]
pub struct StartSnapshotIdempotencyRecord {
    pub snapshot_id: String,
    pub request_fingerprint: String,
}"

CODE — the status the record moves to after the create, services/service-com-amazonaws-ebs-ebs/src/state.rs:272-282 (`apply_completion` sets `Completed`/`Error`) and services/service-com-amazonaws-ebs-ebs/src/state.rs:401-404:
"            if now >= snapshot.expiry_time {
                snapshot.status = SnapshotStatus::Error;
                snapshot.blocks.clear();
            }"

TEST COVERAGE — the retry is only ever issued while the snapshot is still pending, so the divergence is unpinned, services/service-com-amazonaws-ebs-ebs/tests/all/snapshot_block_loop.rs:1090-1091 and 1123-1143:
"/// RULE 5: a repeated `ClientToken` replays the first snapshot rather than creating a
/// second one; the same token with a different request is a `ConflictException`."
"        let first = start_snapshot(&router, request).await;
        let second = start_snapshot(&router, request).await;"
"        assert_eq!(
            first.json(),
            second.json(),
            \"case {case:?}: the retry must replay the original answer\"
        );"
(no row completes or times out the snapshot between the two calls)

## Steps to Reproduce

1. POST /snapshots with body {"VolumeSize": 1, "ClientToken": "retry-token"} — 201, body carries "Status": "pending" and a SnapshotId S.
2. POST /snapshots/completion/S with header x-amz-ChangedBlocksCount: 0 — 202; the stored record is now `completed` (state.rs:272-279).
3. POST /snapshots again with the byte-identical body {"VolumeSize": 1, "ClientToken": "retry-token"} — the fingerprint matches, so the handler takes the replay branch at start_snapshot.rs:310-316.
4. Observe the 201 body: "Status": "completed", not the "pending" the original successful request answered with — while the member is documented to "return the result from the original successful request" and the operation is documented to answer for a snapshot that "enters the pending state after the request completes".
5. Variant without CompleteSnapshot: start the snapshot with {"VolumeSize": 1, "Timeout": 10, "ClientToken": "retry-token-2"}, let the Timeout elapse (apply_due_transitions, state.rs:401-404, sets the record to `error` and clears its blocks), then retry the identical request: the replay answers 201 with "Status": "error".
