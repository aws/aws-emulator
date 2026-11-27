Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 864dd26a02416cc990250070e65c7b703d06755e

# Block tokens keep resolving forever past the ExpiryTime the service reports for them

_Filed: 2026-11-19T10:21:54Z_
_Operation: GetSnapshotBlock | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `864dd26a02416cc990250070e65c7b703d06755e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every listing of a completed snapshot reports an `ExpiryTime` that the model and this crate's own state module both define as the instant the `BlockToken`s handed out stop being valid. No handler ever consults that instant: `GetSnapshotBlock` resolves a token by comparing it with the entry stored at `(SnapshotId, BlockIndex)` and never compares `now` with `snapshot.expiry_time`, and the only place blocks (and with them their tokens) are released — `ComAmazonawsEbsEbsState::apply_due_transitions` — skips every snapshot that is not `Pending`.

Since the read path only sees `Completed` snapshots (`Snapshot::is_readable`), the consequence is that for exactly the snapshots whose tokens are handed out, `expiry_time` is never acted on: `ListSnapshotBlocks` reports an instant (for a snapshot completed early in its Timeout window, an instant that is soon in the PAST) and `GetSnapshotBlock` then keeps answering 200 with the block bytes for that same token indefinitely. A caller that trusts `ExpiryTime` to decide when to re-list sees a token the service said had expired continue to work, and a caller testing the opposite (expecting the refusal the crate documents) never gets it.

This needs no new constant or window: the instant is already stored on the record and already put on the wire.

## Evidence

Smithy, `ListSnapshotBlocksResponse$ExpiryTime` (smithy_query doc): "The time when the `BlockToken` expires."

The crate states the same rule twice:
- services/service-com-amazonaws-ebs-ebs/src/state.rs:187-190 — "/// When the snapshot's `Timeout` elapses: also the instant the block tokens it\n    /// hands out stop being valid, which is what `ListSnapshotBlocks$ExpiryTime`\n    /// reports (\"The time when the `BlockToken` expires\").\n    pub expiry_time: SystemTime,"
- services/service-com-amazonaws-ebs-ebs/src/state.rs:357-362 — "The token is stored on the [`BlockEntry`] it names and NOWHERE else ... a superseded or expired token simply stops resolving, and no token row can outlive the block it named."

The code that would have to honour it does not:
- services/service-com-amazonaws-ebs-ebs/src/ops/get_snapshot_block.rs:243-252 — "let resolved = snapshot\n            .blocks\n            .get(&input.block_index)\n            .filter(|entry| entry.block_token == input.block_token);" — the only test applied to the token; `snapshot.expiry_time` is never read in this file.
- services/service-com-amazonaws-ebs-ebs/src/state.rs:392-399 — "for snapshot in self.snapshots.values_mut() {\n            if snapshot.status != SnapshotStatus::Pending {\n                continue;\n            }\n            if now >= snapshot.expiry_time {\n                snapshot.status = SnapshotStatus::Error;\n                snapshot.blocks.clear();" — blocks (and their tokens) are released only for a snapshot still `Pending`, i.e. never for one the read path can see (`Snapshot::is_readable`, state.rs:290-292: "self.status == SnapshotStatus::Completed").
- services/service-com-amazonaws-ebs-ebs/src/ops/list_snapshot_blocks.rs:283 — "expiry_time: Some(::aws_smithy_types::DateTime::from(snapshot.expiry_time))," — reported unconditionally.

Test coverage: tests/all/snapshot_block_loop.rs:535-536 only asserts `ExpiryTime` is present ("ListSnapshotBlocks must report the stored ExpiryTime"); no test in the crate exercises a token used after that instant (a grep for "expire" in tests/ finds only comments in changed_blocks_between_snapshots.rs), so nothing pins the current behaviour.

## Steps to Reproduce

1. StartSnapshot with `Timeout` at its minimum (VolumeSize 1, Timeout 10), PutSnapshotBlock one 524288-byte block, CompleteSnapshot with ChangedBlocksCount 1.
2. ListSnapshotBlocks on the snapshot and read `ExpiryTime` (= StartTime + 10 minutes) and the block's `BlockToken`.
3. Advance past that instant (or construct the stored `Snapshot` with an `expiry_time` already in the past) and call GetSnapshotBlock with the same `blockToken`.
4. Observe HTTP 200 with the block data: the token the service reported as expired still resolves, because get_snapshot_block.rs:243-252 never compares `now` with `snapshot.expiry_time` and apply_due_transitions (state.rs:392-399) releases nothing for a `Completed` snapshot.
