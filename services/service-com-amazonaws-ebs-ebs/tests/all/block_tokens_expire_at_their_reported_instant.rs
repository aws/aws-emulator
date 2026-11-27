// SPDX-License-Identifier: CC0-1.0

//! When a `BlockToken` this service handed out stops naming a block.
//!
//! RULE: a `BlockToken` names a block only until the instant the service reports as the
//! token's expiry — `ListSnapshotBlocksResponse$ExpiryTime` is "The time when the
//! `BlockToken` expires"
//! (`smithy_query(query="doc",shape="com.amazonaws.ebs#ListSnapshotBlocksResponse",member="ExpiryTime")`,
//! the same sentence on `ListChangedBlocksResponse$ExpiryTime`) — so a `GetSnapshotBlock`
//! presenting the token at or after that instant resolves no block and is refused exactly
//! as a token this service never minted is: the modelled `ValidationException` (HTTP 400,
//! `smithy_query(query="shape",shape="com.amazonaws.ebs#ValidationException")`) with
//! `Reason` `INVALID_BLOCK_TOKEN`. Before that instant the same token returns the bytes
//! that were written.
//!
//! The rule is about the TOKEN, not the content: a completed snapshot keeps every block it
//! holds past the instant its tokens expire, so it is still listed with all of its indexes
//! — what a caller loses is the tokens, which is why `ExpiryTime` is reported at all.
//!
//! Rows differ along the window the snapshot's `Timeout` gives its tokens (the modelled
//! minimum, a value inside the range, the documented default a request names no `Timeout`
//! for, the modelled maximum), along how many blocks the snapshot holds and at which
//! indexes, and along whether those blocks were written into the snapshot or inherited
//! from the parent it was started from. The clock table crosses every row with the moment
//! the block is fetched at: inside the window, the reported instant having just passed, and
//! long past it.

use std::collections::BTreeMap;
use std::time::Duration;

use smithy_server_runtime::MultiServiceRouter;

use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, elapse, get_block, list_blocks, listed_blocks,
    member, put_block, router_with_engine, start_snapshot, timestamp_member, write_snapshot,
    BLOCK_SIZE,
};

/// The window a `StartSnapshot` naming no `Timeout` gives: its documentation states "If no
/// value is specified, the timeout defaults to `60` minutes"
/// (`smithy_query(query="doc",shape="com.amazonaws.ebs#StartSnapshotRequest",member="Timeout")`).
const DEFAULT_TIMEOUT_MINUTES: i32 = 60;

/// One snapshot under test: the `Timeout` its `StartSnapshot` carries and the blocks it ends
/// up holding. The window is DERIVED from that `Timeout` rather than restated, so no row can
/// claim a window its request does not ask for.
struct Row {
    case: &'static str,
    /// The `Timeout` the request carries, or `None` for a request that names none and takes
    /// the documented default. The modelled `range` of `StartSnapshotRequest$Timeout` is
    /// 10..4320 minutes
    /// (`smithy_query(query="shape",shape="com.amazonaws.ebs#StartSnapshotRequest")`).
    timeout: Option<i32>,
    volume_size: i64,
    /// `(index, byte seed)` blocks written into a sealed PARENT this snapshot is then
    /// started from, so it holds them without having written them.
    inherited: &'static [(i32, u8)],
    /// `(index, byte seed)` writes this snapshot makes, in order.
    writes: &'static [(i32, u8)],
}

const ROWS: &[Row] = &[
    Row {
        case: "the modelled minimum Timeout, one block at index 0",
        timeout: Some(10),
        volume_size: 1,
        inherited: &[],
        writes: &[(0, 31)],
    },
    Row {
        case: "a Timeout inside the modelled range, two sparse blocks",
        timeout: Some(90),
        volume_size: 1,
        inherited: &[],
        writes: &[(0, 32), (3, 33)],
    },
    Row {
        case: "the documented default Timeout, one block at a non-zero index",
        timeout: None,
        volume_size: 1,
        inherited: &[],
        writes: &[(1, 34)],
    },
    Row {
        case: "the modelled maximum Timeout, two adjacent blocks",
        timeout: Some(4320),
        volume_size: 1,
        inherited: &[],
        writes: &[(0, 35), (1, 36)],
    },
    Row {
        case: "blocks inherited from a parent and nothing written",
        timeout: Some(10),
        volume_size: 2,
        inherited: &[(0, 37), (2, 38)],
        writes: &[],
    },
];

/// One moment of the clock table: how much of the token's life has passed when the block is
/// fetched, and whether the token still names a block then.
struct Moment {
    case: &'static str,
    /// The clock that has passed since the snapshot was last touched, from the window its
    /// `Timeout` gives.
    elapsed: fn(Duration) -> Duration,
    /// Whether the token resolves at that moment.
    resolves: bool,
}

const MOMENTS: &[Moment] = &[
    Moment {
        case: "half the window gone, well inside the reported expiry",
        elapsed: |window| window / 2,
        resolves: true,
    },
    Moment {
        case: "the whole window gone, so the reported expiry has just passed",
        elapsed: |window| window,
        resolves: false,
    },
    Moment {
        case: "a day past the reported expiry",
        elapsed: |window| window + Duration::from_secs(24 * 60 * 60),
        resolves: false,
    },
];

impl Row {
    /// The byte seed each index this snapshot holds ends up filled with: the inherited
    /// blocks, then this snapshot's own writes over them, so a rewritten index carries the
    /// last write ("If the specified block contains data, the existing data is
    /// overwritten", `smithy_query(query="operation",operation="PutSnapshotBlock")`).
    fn held_blocks(&self) -> BTreeMap<i32, u8> {
        let mut held: BTreeMap<i32, u8> = self.inherited.iter().copied().collect();
        for (block_index, seed) in self.writes {
            held.insert(*block_index, *seed);
        }
        held
    }

    /// The number of DISTINCT indexes this snapshot's own producer wrote, which is the
    /// `ChangedBlocksCount` its completion must declare to complete it.
    fn changed_blocks_count(&self) -> i32 {
        let mut indexes: Vec<i32> = self.writes.iter().map(|(index, _)| *index).collect();
        indexes.sort_unstable();
        indexes.dedup();
        i32::try_from(indexes.len()).expect("a handful of written indexes")
    }

    /// The window this row's request asks for: the `Timeout` it carries, or the documented
    /// default when it names none.
    fn window(&self) -> Duration {
        let minutes = self.timeout.unwrap_or(DEFAULT_TIMEOUT_MINUTES);
        let minutes = u64::try_from(minutes).expect("a Timeout inside the modelled range");
        Duration::from_secs(minutes * 60)
    }
}

/// Start the row's snapshot — from a sealed parent holding its inherited blocks, where it
/// has any — write its writes, seal it, and answer `(its id, the (index, token) pairs the
/// listing handed out)`.
async fn completed_snapshot(router: &MultiServiceRouter, row: &Row) -> (String, Vec<(i32, String)>) {
    let mut body = serde_json::json!({ "VolumeSize": row.volume_size });
    if !row.inherited.is_empty() {
        let parent = write_snapshot(router, row.case, row.volume_size, row.inherited).await;
        body["ParentSnapshotId"] = serde_json::json!(parent);
    }
    if let Some(timeout) = row.timeout {
        body["Timeout"] = serde_json::json!(timeout);
    }

    let started = start_snapshot(router, &body).await;
    assert_eq!(
        started.status, 201,
        "case {:?}: StartSnapshot must answer 201, got {} with body {}",
        row.case,
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let snapshot_id = member(&started, "SnapshotId");

    for (block_index, seed) in row.writes {
        let data = block_bytes(*seed);
        let data_checksum = checksum(&data);
        let written = put_block(
            router,
            &snapshot_id,
            *block_index,
            data,
            BLOCK_SIZE,
            &data_checksum,
        )
        .await;
        assert_eq!(
            written.status, 201,
            "case {:?}: PutSnapshotBlock at index {block_index} must answer 201, got {} with \
             body {}",
            row.case,
            written.status,
            String::from_utf8_lossy(&written.body)
        );
    }

    let sealed = complete_snapshot(router, &snapshot_id, row.changed_blocks_count()).await;
    assert_eq!(
        sealed.status, 202,
        "case {:?}: CompleteSnapshot must answer 202, got {} with body {}",
        row.case,
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );

    let listing = list_blocks(router, &snapshot_id, "").await;
    assert_eq!(
        listing.status, 200,
        "case {:?}: ListSnapshotBlocks must answer 200, got {} with body {}",
        row.case,
        listing.status,
        String::from_utf8_lossy(&listing.body)
    );
    assert!(
        timestamp_member(&listing, "ExpiryTime").is_some(),
        "case {:?}: the listing handing out the tokens must report the instant they expire \
         at, got {}",
        row.case,
        listing.json()
    );
    let handed_out = listed_blocks(&listing);
    let expected_indexes: Vec<i32> = row.held_blocks().keys().copied().collect();
    let listed_indexes: Vec<i32> = handed_out.iter().map(|(index, _)| *index).collect();
    assert_eq!(
        listed_indexes, expected_indexes,
        "case {:?}: expected a token for every index the snapshot holds, got {listed_indexes:?}",
        row.case
    );

    (snapshot_id, handed_out)
}

/// RULE: a token resolves while the instant the service reported for it is still in the
/// future, and not at or after it.
#[tokio::test]
async fn a_block_token_resolves_only_before_the_instant_it_expires_at() {
    for row in ROWS {
        for moment in MOMENTS {
            // A fresh router per sub-case: no sub-case can pass on another's clock.
            let (router, engine) = router_with_engine();
            let (snapshot_id, handed_out) = completed_snapshot(&router, row).await;
            let case = format!("{} / {}", row.case, moment.case);
            let held = row.held_blocks();

            // `Timeout` is in MINUTES, with a modelled minimum of 10, so no test can wait a
            // window out: the stored instants are rewound instead and the next request
            // observes the clock that rewind created.
            elapse(&engine, &case, &snapshot_id, (moment.elapsed)(row.window()));

            for (block_index, block_token) in &handed_out {
                let fetched = get_block(&router, &snapshot_id, *block_index, block_token).await;
                if moment.resolves {
                    assert_eq!(
                        fetched.status, 200,
                        "case {case:?}: a token inside its expiry must answer 200 at index \
                         {block_index}, got {} with body {}",
                        fetched.status,
                        String::from_utf8_lossy(&fetched.body)
                    );
                    let seed = held
                        .get(block_index)
                        .copied()
                        .unwrap_or_else(|| panic!("case {case:?}: index {block_index} is held"));
                    let expected = block_bytes(seed);
                    assert!(
                        fetched.body == expected,
                        "case {case:?}: index {block_index} must answer the bytes written, got \
                         {} byte(s) with checksum {:?}",
                        fetched.body.len(),
                        fetched.header("x-amz-Checksum")
                    );
                    assert_eq!(
                        fetched.header("x-amz-Checksum"),
                        checksum(&expected),
                        "case {case:?}: index {block_index} must report the checksum of the \
                         bytes written, got {:?}",
                        fetched.header("x-amz-Checksum")
                    );
                } else {
                    assert_expired_token(&case, &fetched, block_token);
                }
            }
        }
    }
}

/// The other half of the same rule: what expires is the TOKEN, so the snapshot keeps every
/// block it holds past that instant — it is still resolved by a listing, which still reports
/// all of its indexes and still reports an `ExpiryTime` — while no token it hands out
/// resolves any more.
#[tokio::test]
async fn an_expired_token_leaves_the_snapshots_blocks_in_place() {
    for row in ROWS {
        let (router, engine) = router_with_engine();
        let (snapshot_id, _) = completed_snapshot(&router, row).await;
        elapse(&engine, row.case, &snapshot_id, row.window());

        let listing = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listing.status, 200,
            "case {:?}: a completed snapshot must still be listable past its token expiry, \
             got {} with body {}",
            row.case,
            listing.status,
            String::from_utf8_lossy(&listing.body)
        );
        assert!(
            timestamp_member(&listing, "ExpiryTime").is_some(),
            "case {:?}: the listing must still report the instant its tokens expire at, got {}",
            row.case,
            listing.json()
        );
        let relisted = listed_blocks(&listing);
        let listed_indexes: Vec<i32> = relisted.iter().map(|(index, _)| *index).collect();
        let expected_indexes: Vec<i32> = row.held_blocks().keys().copied().collect();
        assert_eq!(
            listed_indexes, expected_indexes,
            "case {:?}: the snapshot must still hold every index it held, got \
             {listed_indexes:?}",
            row.case
        );

        for (block_index, block_token) in &relisted {
            let fetched = get_block(&router, &snapshot_id, *block_index, block_token).await;
            assert_expired_token(row.case, &fetched, block_token);
        }
    }
}

/// Assert `reply` is the refusal a token that no longer names a block gets: the modelled
/// `ValidationException` (HTTP 400) with `Reason` `INVALID_BLOCK_TOKEN` and the sentence
/// naming the rejected token — the SAME refusal a token this service never minted gets
/// (`crate::read_path_requires_a_completed_snapshot`), because a token that resolves
/// nothing is one condition however it came to resolve nothing.
fn assert_expired_token(case: &str, reply: &crate::snapshot_block_loop::Reply, block_token: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: a token past its expiry must be refused 400, got {} with body {}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        reply.error_code().ends_with("ValidationException"),
        "case {case:?}: expected the ValidationException error code, got {:?}",
        reply.error_code()
    );
    assert_eq!(
        member(reply, "Reason"),
        "INVALID_BLOCK_TOKEN",
        "case {case:?}: an expired token is a rejected token, not a missing snapshot, got {}",
        reply.json()
    );
    assert_eq!(
        member(reply, "Message"),
        format!("The block token {block_token} is invalid."),
        "case {case:?}: the refusal must name the rejected token, got {}",
        reply.json()
    );
}
