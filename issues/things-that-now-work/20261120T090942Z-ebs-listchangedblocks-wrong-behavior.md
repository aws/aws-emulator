Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c5b9149d70e2a9aad6a1bceb8aa9795c91a9cab7

# ListChangedBlocks reports changed blocks that are not on the second snapshot, emitting a ChangedBlock with no SecondBlockToken

_Filed: 2026-11-20T09:09:42Z_
_Operation: ListChangedBlocks | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c5b9149d70e2a9aad6a1bceb8aa9795c91a9cab7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`changed_indexes` takes the UNION of the two snapshots' block maps and keeps every index only one side holds (`_ => true`). When the index is held only by the FIRST snapshot, the handler still emits a `ChangedBlock` for it, with `FirstBlockToken` set and `SecondBlockToken` absent.

The model documents the opposite asymmetry, and only that one: `ChangedBlock$FirstBlockToken` is the member that carries an absence clause — "This value is absent if the first snapshot does not have the changed block that is on the second snapshot" — a sentence that presupposes a changed block IS on the second snapshot. `ChangedBlock$SecondBlockToken` carries no absence clause at all, so a `ChangedBlock` whose `SecondBlockToken` is omitted is a response shape the model documents no case for.

The handler's own comment quotes exactly that `FirstBlockToken` sentence and then generalises it to BOTH sides ("the side that holds no block at the index reports no token for it"), which is the step the quoted sentence does not license: it licenses only the first side being absent.

The condition is reachable with ordinary inputs in this crate's own semantics: a child inherits every block of its parent, so comparing with `firstSnapshotId` = child and `secondSnapshotId` = parent (or two siblings of one parent where only one wrote an index) gives an index the first side holds and the second does not. Both snapshots are sealed and share a lineage, and the second holds blocks, so none of the handler's refusals fire — the index is reported as changed with no second-side token.

## Evidence

MODEL — `smithy_query(query="doc", shape="com.amazonaws.ebs#ChangedBlock", member="FirstBlockToken")`:
"The block token for the block index of the FirstSnapshotId specified in the ListChangedBlocks operation. This value is absent if the first snapshot does not have the changed block that is on the second snapshot."

MODEL — `smithy_query(query="doc", shape="com.amazonaws.ebs#ChangedBlock", member="SecondBlockToken")` (no absence clause at all):
"The block token for the block index of the SecondSnapshotId specified in the ListChangedBlocks operation."

CODE — services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs:389-403 keeps an index only one side holds:
"fn changed_indexes(first: &Snapshot, second: &Snapshot) -> Vec<i32> {
    let mut indexes: BTreeSet<i32> = second.blocks.keys().copied().collect();
    indexes.extend(first.blocks.keys().copied());
    indexes
        .into_iter()
        .filter(|block_index| {
            match (first.blocks.get(block_index), second.blocks.get(block_index)) {
                (Some(first_block), Some(second_block)) => {
                    first_block.checksum != second_block.checksum
                }
                // An index only one of them holds is a block the other does not have.
                _ => true,
            }
        })"

CODE — services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs:268-281 then renders the second side as absent for such an index:
"            .map(|block_index| ChangedBlock {
                block_index: Some(block_index),
                first_block_token: first
                    .blocks
                    .get(&block_index)
                    .map(|entry| entry.block_token.clone()),
                second_block_token: second
                    .blocks
                    .get(&block_index)
                    .map(|entry| entry.block_token.clone()),
            })"

CODE — the comment above it, services/service-com-amazonaws-ebs-ebs/src/ops/list_changed_blocks.rs:261-267, quotes the FIRST-side sentence and applies it to both sides:
"        // The comparison itself: a block index is CHANGED when the two snapshots do not
        // hold the same content at it — one holds a block the other does not, or both
        // hold one and the stored checksums of their bytes differ. Each side's token is
        // the one that side hands out for that index, so a caller can read either
        // version back through `GetSnapshotBlock`; the side that holds no block at the
        // index reports no token for it — \"this value is absent if the first snapshot does
        // not have the changed block that is on the second snapshot\" (`ChangedBlock`)."

TEST COVERAGE — the current behaviour is pinned, with no model citation for the second-side-absent direction: tests/all/changed_blocks_between_snapshots.rs:1154-1160 adds the row "the same pair with the two sides swapped" (first = child holding {0,5,9}, second = parent holding {0,5}) and tests/all/changed_blocks_between_snapshots.rs:1205-1210 asserts "block {index} must carry a SecondBlockToken exactly when the second snapshot holds that block". The suite's own rule header cites the model only for the FIRST side (tests/all/changed_blocks_between_snapshots.rs:28-31: "each with no `FirstBlockToken` — \"this value is absent if the first snapshot does not have the changed block that is on the second snapshot\" (`ChangedBlock`)").

## Steps to Reproduce

1. POST /snapshots {"VolumeSize": 2} to mint a parent; PUT 524288-byte blocks at indexes 0 and 5 (with x-amz-Data-Length: 524288 and the matching x-amz-Checksum); POST /snapshots/completion/{parent} with x-amz-ChangedBlocksCount: 2 so the parent seals `completed`.
2. POST /snapshots {"VolumeSize": 2, "ParentSnapshotId": "<parent>"} to mint a child (it inherits indexes 0 and 5); PUT a block at index 9; POST /snapshots/completion/{child} with x-amz-ChangedBlocksCount: 1 so the child seals `completed`.
3. GET /snapshots/{parent}/changedblocks?firstSnapshotId=<child> — the comparison reversed, so the FIRST side holds an index the SECOND does not.
4. Observe the 200 body: ChangedBlocks contains {"BlockIndex": 9, "FirstBlockToken": "..."} with NO SecondBlockToken — a changed block that is not on the second snapshot, while the model gives an absence clause only for FirstBlockToken ("absent if the first snapshot does not have the changed block that is on the second snapshot") and none for SecondBlockToken.
