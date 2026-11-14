// SPDX-License-Identifier: CC0-1.0

//! What the EBS direct APIs answer for a snapshot this service no longer holds.
//!
//! RULE: a snapshot id this service holds no snapshot under resolves to nothing, so every
//! operation that resolves one answers the modelled `ResourceNotFoundException` — HTTP 404
//! (`smithy_query(query="operation",operation="ListSnapshotBlocks")`), the
//! `x-amzn-errortype` header naming that shape, `Reason` `SNAPSHOT_NOT_FOUND` (a member of
//! `ResourceNotFoundExceptionReason`) and the sentence naming the snapshot the request asked
//! for — and that holds for a snapshot that WAS held and has since been removed exactly as it
//! does for an id nothing was ever minted for. A snapshot that no longer exists cannot be
//! answered with an empty HTTP 200 listing: that is the wire shape of a snapshot that exists
//! and holds no block, so a caller polling the read path for the removal to take effect could
//! not tell the two apart and would never stop.
//!
//! The removal takes with it only what was stored WITH that snapshot — its blocks, the tokens
//! they handed out and the `ClientToken` row that minted it — and nothing of any other
//! snapshot: every other snapshot of the store stays listable, readable and comparable, and a
//! lineage survives one of its generations being removed.
//!
//! The EBS direct APIs model no delete of their own (the service's operations are the write
//! loop and the two reads), so the removal is the one `ec2:DeleteSnapshot` performs on this
//! store, applied here through the store's own removal path — the one that operation reaches.
//! The two services driven over their own wires on one engine are covered beside the router,
//! in `core/router/tests/all/ebs_refused_block_write_then_ec2_snapshot_completion.rs`, which
//! no single service crate's test target can host.
//!
//! Each row is a distinct input along what the rule generalises over: how many blocks the
//! removed snapshot held and at which indexes, the volume size it was of, where in a lineage
//! it sat, which operation reads it afterwards, and which member of that operation its id
//! arrived in.

use serde_json::json;
use service_com_amazonaws_ebs_ebs::state::{encode_page_token, service_state};
use smithy_server_runtime::MultiServiceRouter;

use crate::changed_blocks_between_snapshots::sealed_snapshot;
use crate::read_path_requires_a_completed_snapshot::assert_not_found;
use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, get_block, list_blocks, listed_blocks, member,
    put_block, router_with_engine, send, start_snapshot, write_snapshot, Reply, BLOCK_SIZE,
};

/// The shape name the `x-amzn-errortype` header of the refusal carries.
const NOT_FOUND_ERROR_TYPE: &str = "ResourceNotFoundException";

/// A block token shaped like one of this service's but minted by nobody, for the probe that
/// shows the snapshot is resolved before the token is.
const UNMINTED_BLOCK_TOKEN: &str = "QUJD";

/// The removal `ec2:DeleteSnapshot` performs on this service's store: the record goes, and
/// with it everything stored only with it.
fn remove_snapshot(engine: &engine::InMemoryEngine, case: &str, snapshot_id: &str) {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: this service's store must be reachable"));
    let removed = state.lock().remove_snapshot(snapshot_id);
    assert!(
        removed.is_some(),
        "case {case:?}: expected {snapshot_id} to be stored before it is removed"
    );
}

/// Assert `reply` is the refusal for a snapshot that resolves to nothing, naming
/// `snapshot_id`: the status, the error shape, `Reason`, the sentence, and the
/// `x-amzn-errortype` header a client's error handling dispatches on.
fn assert_resolves_nowhere(case: &str, reply: &Reply, snapshot_id: &str) {
    assert_not_found(case, reply, snapshot_id);
    let error_type = reply.header("x-amzn-errortype");
    assert!(
        error_type.ends_with(NOT_FOUND_ERROR_TYPE),
        "case {case:?}: expected the x-amzn-errortype header to name {NOT_FOUND_ERROR_TYPE}, \
         got {error_type:?}"
    );
}

/// `ListChangedBlocks` of `second`, with `first` as the first side where a row names one.
async fn list_changed_blocks(
    router: &MultiServiceRouter,
    second: &str,
    first: Option<&str>,
) -> Reply {
    let uri = match first {
        Some(first) => format!("/snapshots/{second}/changedblocks?firstSnapshotId={first}"),
        None => format!("/snapshots/{second}/changedblocks"),
    };
    send(router, "GET", &uri, &[], Vec::new()).await
}

/// Assert `snapshot_id` is readable: its listing reports exactly `blocks`, and every token it
/// hands out fetches the bytes that block was written with.
async fn assert_still_readable(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_id: &str,
    blocks: &[(i32, u8)],
) {
    let listing = list_blocks(router, snapshot_id, "").await;
    assert_eq!(
        listing.status,
        200,
        "case {case:?}: {snapshot_id} must still be listable, got {} with body {}",
        listing.status,
        String::from_utf8_lossy(&listing.body)
    );
    let listed = listed_blocks(&listing);
    let reported: Vec<i32> = listed.iter().map(|(index, _)| *index).collect();
    let mut expected: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
    expected.sort_unstable();
    assert_eq!(
        reported, expected,
        "case {case:?}: {snapshot_id} must still report the blocks written into it, got \
         {reported:?}"
    );
    for (block_index, token) in &listed {
        let seed = blocks
            .iter()
            .find(|(index, _)| index == block_index)
            .map(|(_, seed)| *seed)
            .unwrap_or_else(|| panic!("case {case:?}: index {block_index} is not a written one"));
        let fetched = get_block(router, snapshot_id, *block_index, token).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: block {block_index} of {snapshot_id} must still be fetchable, got \
             {} with body {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            checksum(&fetched.body),
            checksum(&block_bytes(seed)),
            "case {case:?}: block {block_index} of {snapshot_id} must still hold the bytes it \
             was written with, got {} byte(s)",
            fetched.body.len()
        );
    }
}

/// One row of the removal table: the case, the volume size the snapshot is of, and the
/// `(block index, byte seed)` pairs written into it before it is sealed and removed.
type RemovalRow = (&'static str, i64, &'static [(i32, u8)]);

/// Rows differing along what the rule generalises over: a snapshot with no block at all, one
/// with a single block, one with several at non-adjacent indexes, and one over a larger volume
/// whose only block sits at a high index.
const REMOVAL_ROWS: &[RemovalRow] = &[
    ("a removed snapshot with no block written", 1, &[]),
    ("a removed snapshot with one block", 1, &[(0, 11)]),
    (
        "a removed snapshot with blocks at non-adjacent indexes",
        2,
        &[(0, 12), (3, 13), (7, 14)],
    ),
    (
        "a removed snapshot over a larger volume with a high index",
        8,
        &[(4096, 15)],
    ),
];

/// RULE: every operation that resolves a `SnapshotId` refuses the id of a snapshot the store
/// no longer holds with the modelled not-found refusal, whatever that snapshot held and
/// whichever member its id arrives in — and the snapshot that was NOT removed is untouched.
#[tokio::test]
async fn every_operation_refuses_the_id_of_a_removed_snapshot() {
    let (router, engine) = router_with_engine();

    // The snapshot no row removes: every probe below must leave it exactly as readable as it
    // was, so a removal that took more than the id it was given cannot pass.
    let sentinel_blocks: &[(i32, u8)] = &[(0, 71), (2, 72)];
    let sentinel = write_snapshot(&router, "the sentinel snapshot", 1, sentinel_blocks).await;

    for (case, volume_size, blocks) in REMOVAL_ROWS {
        let snapshot_id = write_snapshot(&router, case, *volume_size, blocks).await;

        // While it is held, the read path serves it: the tokens and the page token gathered
        // here are ones this service really issued for this snapshot.
        let listing = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listing.status,
            200,
            "case {case:?}: the sealed snapshot must be listable before it is removed, got {} \
             with body {}",
            listing.status,
            String::from_utf8_lossy(&listing.body)
        );
        let issued_tokens = listed_blocks(&listing);

        remove_snapshot(&engine, case, &snapshot_id);

        // `ListSnapshotBlocks`, with each paging member the model carries — including a page
        // token this service issued for a block the snapshot held, whose own validation the
        // snapshot gate precedes. `MaxResults` stays inside its modelled range 100..10000.
        let mut queries: Vec<String> = vec![
            String::new(),
            "startingBlockIndex=0".to_string(),
            "maxResults=100".to_string(),
            "startingBlockIndex=1&maxResults=10000".to_string(),
        ];
        if let Some((block_index, _)) = issued_tokens.first() {
            queries.push(format!(
                "pageToken={}",
                encode_page_token(&snapshot_id, *block_index)
            ));
        }
        for query in &queries {
            let reply = list_blocks(&router, &snapshot_id, query).await;
            assert_resolves_nowhere(
                &format!("{case}: ListSnapshotBlocks?{query}"),
                &reply,
                &snapshot_id,
            );
        }

        // `GetSnapshotBlock`, with each token the snapshot handed out while it was held — a
        // token of a snapshot that is gone resolves nothing — and with a token this service
        // never minted, which answers the same refusal because the snapshot is resolved
        // first.
        for (block_index, token) in &issued_tokens {
            let reply = get_block(&router, &snapshot_id, *block_index, token).await;
            assert_resolves_nowhere(
                &format!("{case}: GetSnapshotBlock of block {block_index} with its own token"),
                &reply,
                &snapshot_id,
            );
        }
        let reply = get_block(&router, &snapshot_id, 0, UNMINTED_BLOCK_TOKEN).await;
        assert_resolves_nowhere(
            &format!("{case}: GetSnapshotBlock with a token this service never minted"),
            &reply,
            &snapshot_id,
        );

        // `ListChangedBlocks`, with the removed id in EITHER member it can arrive in: the
        // refusal names the id the request asked for, not the one in the URI.
        for (probe, second, first) in [
            (
                "ListChangedBlocks of the removed snapshot",
                snapshot_id.as_str(),
                None,
            ),
            (
                "ListChangedBlocks of the removed snapshot against a held one",
                snapshot_id.as_str(),
                Some(sentinel.as_str()),
            ),
            (
                "ListChangedBlocks of a held snapshot against the removed one",
                sentinel.as_str(),
                Some(snapshot_id.as_str()),
            ),
        ] {
            let reply = list_changed_blocks(&router, second, first).await;
            assert_resolves_nowhere(&format!("{case}: {probe}"), &reply, &snapshot_id);
        }

        // The write loop resolves the same id the same way: the snapshot a block would be
        // written into, the snapshot a seal would be applied to, and the parent a new
        // snapshot would be taken from are all snapshots this store no longer holds.
        let data = block_bytes(99);
        let expected = checksum(&data);
        let written = put_block(&router, &snapshot_id, 0, data, BLOCK_SIZE, &expected).await;
        assert_resolves_nowhere(
            &format!("{case}: PutSnapshotBlock into the removed snapshot"),
            &written,
            &snapshot_id,
        );

        let sealed = complete_snapshot(&router, &snapshot_id, 1).await;
        assert_resolves_nowhere(
            &format!("{case}: CompleteSnapshot of the removed snapshot"),
            &sealed,
            &snapshot_id,
        );

        let started = start_snapshot(
            &router,
            &json!({ "VolumeSize": volume_size, "ParentSnapshotId": snapshot_id.clone() }),
        )
        .await;
        assert_resolves_nowhere(
            &format!("{case}: StartSnapshot from the removed snapshot"),
            &started,
            &snapshot_id,
        );

        // And the snapshot the row did not remove is exactly as it was.
        assert_still_readable(&router, case, &sentinel, sentinel_blocks).await;
    }
}

/// One snapshot a removal left in place: its id, and the `(block index, byte seed)` pairs it
/// must still report.
type Survivor<'a> = (&'a str, &'a [(i32, u8)]);

/// Which generation of a three-snapshot lineage a row removes.
#[derive(Debug, Clone, Copy)]
enum Generation {
    Parent,
    Child,
    Grandchild,
}

/// RULE, applied to a lineage: removing one snapshot removes that snapshot alone — the
/// generations around it keep their own blocks and tokens, and only the removed id is refused,
/// including when a comparison names it as one of its two sides.
///
/// The rows differ along which generation is removed, which is what the rule generalises over
/// here: the oldest (whose blocks the others inherited copies of), the middle one (which both
/// others point through) and the newest.
#[tokio::test]
async fn removing_one_generation_of_a_lineage_leaves_the_others_readable() {
    for (case, removed) in [
        ("the parent of a lineage", Generation::Parent),
        ("the middle generation of a lineage", Generation::Child),
        ("the newest generation of a lineage", Generation::Grandchild),
    ] {
        let (router, engine) = router_with_engine();

        // A parent, a child that adds a block, and a grandchild that adds another. Each one
        // inherits a copy of what the generation before it held, so what a row removes is
        // never the only copy of a block.
        let parent_blocks: &[(i32, u8)] = &[(0, 21)];
        let child_blocks: &[(i32, u8)] = &[(0, 21), (1, 22)];
        let grandchild_blocks: &[(i32, u8)] = &[(0, 21), (1, 22), (2, 23)];
        let parent = sealed_snapshot(&router, case, 2, None, &[(0, 21)]).await;
        let child = sealed_snapshot(&router, case, 2, Some(&parent), &[(1, 22)]).await;
        let grandchild = sealed_snapshot(&router, case, 2, Some(&child), &[(2, 23)]).await;

        let (removed_id, survivors): (&str, Vec<Survivor<'_>>) = match removed {
            Generation::Parent => (
                parent.as_str(),
                vec![
                    (child.as_str(), child_blocks),
                    (grandchild.as_str(), grandchild_blocks),
                ],
            ),
            Generation::Child => (
                child.as_str(),
                vec![
                    (parent.as_str(), parent_blocks),
                    (grandchild.as_str(), grandchild_blocks),
                ],
            ),
            Generation::Grandchild => (
                grandchild.as_str(),
                vec![
                    (parent.as_str(), parent_blocks),
                    (child.as_str(), child_blocks),
                ],
            ),
        };

        remove_snapshot(&engine, case, removed_id);

        let listed = list_blocks(&router, removed_id, "").await;
        assert_resolves_nowhere(
            &format!("{case}: ListSnapshotBlocks of the removed generation"),
            &listed,
            removed_id,
        );

        // Every surviving generation still holds its own blocks and still hands out tokens
        // that read them back.
        for (survivor, blocks) in &survivors {
            assert_still_readable(&router, case, survivor, blocks).await;
        }

        // A comparison that names the removed generation on either side is refused for it,
        // while the comparison of the two survivors is still served.
        for (survivor, _) in &survivors {
            for (probe, second, first) in [
                (
                    "the removed generation as the compared snapshot",
                    removed_id,
                    Some(*survivor),
                ),
                (
                    "the removed generation as the first side",
                    *survivor,
                    Some(removed_id),
                ),
            ] {
                let reply = list_changed_blocks(&router, second, first).await;
                assert_resolves_nowhere(&format!("{case}: {probe}"), &reply, removed_id);
            }
        }
        // Asserted for the rows whose survivors are still ADJACENT generations — the one
        // whose `ParentSnapshotId` names the other. Whether two snapshots whose only link ran
        // THROUGH the removed generation are still of one lineage is a different question
        // from what this suite is about, so it asks nothing about that pair.
        let survivors_adjacent = !matches!(removed, Generation::Child);
        if let ([(first, _), (second, _)], true) = (survivors.as_slice(), survivors_adjacent) {
            let reply = list_changed_blocks(&router, second, Some(first)).await;
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: the two surviving generations must still be comparable, got {} \
                 with body {}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        }
    }
}

/// RULE, applied to what was stored WITH the snapshot: the `ClientToken` row that minted it
/// goes with it, so a repeat of the same `StartSnapshot` under that token starts a NEW
/// snapshot rather than replaying an answer about a snapshot no read resolves.
///
/// The rows differ along the request the token stands for — the volume size declared, whether
/// it carries a description, and how many blocks were written into the snapshot before it was
/// removed.
#[tokio::test]
async fn a_client_token_whose_snapshot_was_removed_starts_a_new_snapshot() {
    /// One row: the case, the `StartSnapshot` body the token stands for, and the blocks
    /// written into the snapshot it minted before that snapshot is removed.
    type ClientTokenRow<'a> = (&'a str, serde_json::Value, &'a [(i32, u8)]);

    let (router, engine) = router_with_engine();

    let rows: &[ClientTokenRow<'_>] = &[
        (
            "a bare request under a client token",
            json!({ "VolumeSize": 1, "ClientToken": "removal-suite-token-1" }),
            &[],
        ),
        (
            "a described request under a client token",
            json!({
                "VolumeSize": 2,
                "Description": "the snapshot the token minted",
                "ClientToken": "removal-suite-token-2",
            }),
            &[(0, 31)],
        ),
        (
            "a larger request under a client token",
            json!({ "VolumeSize": 4, "ClientToken": "removal-suite-token-3" }),
            &[(0, 32), (5, 33)],
        ),
    ];

    for (case, body, blocks) in rows {
        let first = start_snapshot(&router, body).await;
        assert_eq!(
            first.status,
            201,
            "case {case:?}: StartSnapshot must answer 201, got {} with body {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let minted = member(&first, "SnapshotId");

        // While the snapshot is held, the token replays the answer about it.
        let replayed = start_snapshot(&router, body).await;
        assert_eq!(
            replayed.status,
            201,
            "case {case:?}: the repeated request must be answered, got {} with body {}",
            replayed.status,
            String::from_utf8_lossy(&replayed.body)
        );
        assert_eq!(
            member(&replayed, "SnapshotId"),
            minted,
            "case {case:?}: the same token must replay the snapshot it minted, got {}",
            replayed.json()
        );

        for (block_index, seed) in *blocks {
            let data = block_bytes(*seed);
            let expected = checksum(&data);
            let written =
                put_block(&router, &minted, *block_index, data, BLOCK_SIZE, &expected).await;
            assert_eq!(
                written.status,
                201,
                "case {case:?}: PutSnapshotBlock at {block_index} must answer 201, got {} with \
                 body {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }

        remove_snapshot(&engine, case, &minted);

        // The token no longer names anything, so the same request starts a new snapshot: a
        // fresh id, pending, and never the removed one.
        let after = start_snapshot(&router, body).await;
        assert_eq!(
            after.status,
            201,
            "case {case:?}: the repeated request must start a new snapshot once the one its \
             token minted is gone, got {} with body {}",
            after.status,
            String::from_utf8_lossy(&after.body)
        );
        let fresh = member(&after, "SnapshotId");
        assert_ne!(
            fresh, minted,
            "case {case:?}: the new snapshot must not be the removed {minted}, got {}",
            after.json()
        );
        assert_eq!(
            member(&after, "Status"),
            "pending",
            "case {case:?}: a snapshot just started is pending, got {}",
            after.json()
        );

        // And the removed id is still an id this service holds no snapshot under.
        let listed = list_blocks(&router, &minted, "").await;
        assert_resolves_nowhere(
            &format!("{case}: ListSnapshotBlocks after the token minted another snapshot"),
            &listed,
            &minted,
        );
    }
}
