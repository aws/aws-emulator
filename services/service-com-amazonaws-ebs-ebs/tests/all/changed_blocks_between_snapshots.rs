// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListChangedBlocks`, driven over the same wire the sibling suites
//! use (the shared harness in [`super::snapshot_block_loop`]).
//!
//! The rules under test are:
//!
//! 1. THE COMPARISON IS OF CONTENT: the blocks reported for two snapshots of one lineage
//!    are exactly the indexes at which the two do not hold the same bytes — an index only
//!    one of them holds, or one both hold different bytes at — ascending by index, with
//!    each side's own `BlockToken` reported for the side that holds the block and no token
//!    at all for the side that does not. A snapshot compared against one it inherited
//!    every block from reports no changed block.
//! 2. "Returns information about the blocks that are different between two Amazon Elastic
//!    Block Store snapshots OF THE SAME VOLUME/SNAPSHOT LINEAGE" (`ListChangedBlocks`):
//!    two snapshots that share no ancestor cannot be compared, which is the modelled
//!    `ValidationException` carrying the `UNRELATED_SNAPSHOTS` reason; an id no snapshot of
//!    the service resolves is the not-found 404, and a value that is not shaped like a
//!    snapshot id at all is the malformed-parameter `ValidationException`.
//! 3. PAGING WALKS THAT ONE SET: `MaxResults` bounds the page, `NextToken` resumes exactly
//!    where the page stopped, `StartingBlockIndex` starts the listing at an index (and is
//!    ignored when a `NextToken` is given), and a token this service did not issue is
//!    refused.
//! 4. `FirstSnapshotId` is optional: a comparison against no snapshot has nothing to leave
//!    out, so every block the named snapshot holds is reported, each with no
//!    `FirstBlockToken` — "this value is absent if the first snapshot does not have the
//!    changed block that is on the second snapshot" (`ChangedBlock`).
//!
//! Each table row is a distinct input along what the rules generalise over — how the two
//! snapshots are related (parent/child, two generations, two siblings, a snapshot against
//! itself, the comparison reversed), which blocks were written and with which bytes, the
//! declared volume size, the page size and where the listing starts — so no row is the
//! specification.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, member, put_block, router, send, start_snapshot,
    Reply, BLOCK_SIZE,
};

/// The HTTP status the modelled `ValidationException` carries (`@httpError 400`).
const VALIDATION_STATUS: u16 = 400;

/// The HTTP status the modelled `ResourceNotFoundException` carries (`@httpError 404`).
const NOT_FOUND_STATUS: u16 = 404;

/// A well-formed snapshot id (the 17 hex digit form this service mints) that no snapshot of
/// the store is held under, for the not-found half of rule 2.
const UNKNOWN_SNAPSHOT_ID: &str = "snap-0123456789abcdef0";

/// `ListChangedBlocks` of `second_snapshot_id` with `query` appended to the URI.
async fn list_changed_blocks(
    router: &MultiServiceRouter,
    second_snapshot_id: &str,
    query: &str,
) -> Reply {
    let uri = if query.is_empty() {
        format!("/snapshots/{second_snapshot_id}/changedblocks")
    } else {
        format!("/snapshots/{second_snapshot_id}/changedblocks?{query}")
    };
    send(router, "GET", &uri, &[], Vec::new()).await
}

/// The comparison of `first` against `second`, asserted to have answered 200.
async fn compared(
    router: &MultiServiceRouter,
    case: &str,
    first: Option<&str>,
    second: &str,
    extra: &str,
) -> Reply {
    let mut query = match first {
        Some(first) => format!("firstSnapshotId={first}"),
        None => String::new(),
    };
    if !extra.is_empty() {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(extra);
    }
    let reply = list_changed_blocks(router, second, &query).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: ListChangedBlocks must answer 200, got {} with body {}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply
}

/// One changed block a listing reported: its index and each side's token, with `None` for a
/// side the report carried no token for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Changed {
    block_index: i32,
    first_block_token: Option<String>,
    second_block_token: Option<String>,
}

/// The changed blocks a listing reported, in the order it reported them.
fn reported(reply: &Reply) -> Vec<Changed> {
    let token = |block: &serde_json::Value, name: &str| {
        block
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    reply
        .json()
        .get("ChangedBlocks")
        .and_then(serde_json::Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .map(|block| Changed {
                    block_index: block
                        .get("BlockIndex")
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|index| i32::try_from(index).ok())
                        .unwrap_or(-1),
                    first_block_token: token(block, "FirstBlockToken"),
                    second_block_token: token(block, "SecondBlockToken"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Start a snapshot (of `parent`, where the row has one), write `blocks` into it as
/// `(index, byte seed)` pairs, seal it and answer its id.
///
/// Each seed names a distinct 512 KiB payload ([`block_bytes`]), so two blocks written from
/// the same seed hold the same bytes and two written from different seeds do not — which is
/// what rule 1's comparison is of.
pub(crate) async fn sealed_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    volume_size: i64,
    parent: Option<&str>,
    blocks: &[(i32, u8)],
) -> String {
    let mut body = json!({ "VolumeSize": volume_size });
    if let Some(parent) = parent {
        body["ParentSnapshotId"] = json!(parent);
    }
    let started = start_snapshot(router, &body).await;
    assert_eq!(
        started.status,
        201,
        "case {case:?}: StartSnapshot must answer 201, got {} with body {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let snapshot_id = member(&started, "SnapshotId");
    assert!(
        snapshot_id.starts_with("snap-"),
        "case {case:?}: StartSnapshot must mint a snapshot id, got {snapshot_id:?}"
    );

    for (block_index, seed) in blocks {
        let data = block_bytes(*seed);
        let expected = checksum(&data);
        let written = put_block(
            router,
            &snapshot_id,
            *block_index,
            data,
            BLOCK_SIZE,
            &expected,
        )
        .await;
        assert_eq!(
            written.status,
            201,
            "case {case:?}: PutSnapshotBlock at {block_index} must answer 201, got {} with \
             body {}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );
    }

    let count = i32::try_from(blocks.len()).expect("a small block count");
    let sealed = complete_snapshot(router, &snapshot_id, count).await;
    assert_eq!(
        sealed.status,
        202,
        "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );
    snapshot_id
}

/// One row of the comparison table: the case, which snapshot of the lineage is the first
/// side, which is the second, and the `(block index, the first side holds it, the second
/// side holds it)` rows the comparison must report.
type ComparisonRow = (&'static str, Option<Side>, Side, Vec<(i32, bool, bool)>);

/// One row of the inheritance table: the case, the declared volume size, and the blocks the
/// parent holds as `(block index, byte seed)` pairs.
type InheritanceRow = (&'static str, i64, Vec<(i32, u8)>);

/// Which snapshot of the lineage a row compares, so a row can name one the row before it
/// built.
#[derive(Debug, Clone, Copy)]
enum Side {
    Parent,
    Child,
    Grandchild,
    Sibling,
}

/// RULE 1 and RULE 4: the reported blocks are exactly the indexes the two snapshots do not
/// hold the same bytes at, ascending, each side's token present only where that side holds
/// the block — and a comparison against NO first snapshot reports every block the named
/// snapshot holds.
///
/// One lineage is built once — a parent, a child of it, a grandchild of the child and a
/// second child (sibling) of the parent — and the rows compare different pairs of it: a
/// child that only added a block, a child that overwrote one with other bytes, a child that
/// changed nothing, two generations apart, two siblings, a snapshot against itself, and the
/// same pair with the two sides swapped.
#[tokio::test]
async fn the_reported_blocks_are_the_indexes_whose_content_differs() {
    let router = router();
    let case = "the lineage under comparison";

    // The parent holds blocks 0 and 1. The child inherits both, overwrites 1 with other
    // bytes and adds 2. The grandchild inherits all three and adds 3. The sibling inherits
    // the parent's two and adds 4.
    let parent = sealed_snapshot(&router, case, 1, None, &[(0, 10), (1, 11)]).await;
    let child = sealed_snapshot(&router, case, 1, Some(&parent), &[(1, 21), (2, 22)]).await;
    let grandchild = sealed_snapshot(&router, case, 1, Some(&child), &[(3, 33)]).await;
    let sibling = sealed_snapshot(&router, case, 1, Some(&parent), &[(4, 44)]).await;

    let id = |side: Side| match side {
        Side::Parent => parent.clone(),
        Side::Child => child.clone(),
        Side::Grandchild => grandchild.clone(),
        Side::Sibling => sibling.clone(),
    };

    // (case, first side, second side, the expected (index, first holds it, second holds it)
    // rows).
    let rows: Vec<ComparisonRow> = vec![
        (
            "a child that overwrote one block and added another",
            Some(Side::Parent),
            Side::Child,
            vec![(1, true, true), (2, false, true)],
        ),
        (
            "the same pair with the two sides swapped",
            Some(Side::Child),
            Side::Parent,
            vec![(1, true, true), (2, true, false)],
        ),
        (
            "two generations apart",
            Some(Side::Parent),
            Side::Grandchild,
            vec![(1, true, true), (2, false, true), (3, false, true)],
        ),
        (
            "a child that changed nothing its parent did not hold",
            Some(Side::Child),
            Side::Grandchild,
            vec![(3, false, true)],
        ),
        (
            "two siblings of one parent",
            Some(Side::Sibling),
            Side::Child,
            vec![(1, true, true), (2, false, true), (4, true, false)],
        ),
        (
            "a snapshot against itself",
            Some(Side::Child),
            Side::Child,
            Vec::new(),
        ),
        (
            "no first snapshot at all: every block the named snapshot holds",
            None,
            Side::Child,
            vec![(0, false, true), (1, false, true), (2, false, true)],
        ),
        (
            "no first snapshot and a snapshot holding one block",
            None,
            Side::Sibling,
            vec![(0, false, true), (1, false, true), (4, false, true)],
        ),
    ];

    for (case, first, second, expected) in rows {
        let first_id = first.map(id);
        let second_id = id(second);
        let reply = compared(&router, case, first_id.as_deref(), &second_id, "").await;
        let blocks = reported(&reply);

        let indexes: Vec<i32> = blocks.iter().map(|block| block.block_index).collect();
        let expected_indexes: Vec<i32> = expected.iter().map(|(index, _, _)| *index).collect();
        assert_eq!(
            indexes, expected_indexes,
            "case {case:?}: expected the changed indexes {expected_indexes:?} ascending, got \
             {blocks:?}"
        );

        for (block, (index, first_holds, second_holds)) in blocks.iter().zip(&expected) {
            assert_eq!(
                block.first_block_token.is_some(),
                *first_holds,
                "case {case:?}: block {index} must report a FirstBlockToken only when the \
                 first snapshot holds it, got {block:?}"
            );
            assert_eq!(
                block.second_block_token.is_some(),
                *second_holds,
                "case {case:?}: block {index} must report a SecondBlockToken only when the \
                 second snapshot holds it, got {block:?}"
            );
            // The two sides are distinct snapshots, so one index never reports one token
            // twice: each side hands out its own.
            if *first_holds && *second_holds && first_id.as_deref() != Some(second_id.as_str()) {
                assert_ne!(
                    block.first_block_token, block.second_block_token,
                    "case {case:?}: block {index} must report each side's own token, got \
                     {block:?}"
                );
            }
        }

        // The members read off the snapshot the request named, every time.
        let answer = reply.json();
        assert_eq!(
            answer.get("BlockSize").and_then(serde_json::Value::as_i64),
            Some(i64::from(BLOCK_SIZE)),
            "case {case:?}: the answer must report the snapshot's block size, got {answer}"
        );
        assert_eq!(
            answer.get("VolumeSize").and_then(serde_json::Value::as_i64),
            Some(1),
            "case {case:?}: the answer must report the snapshot's volume size, got {answer}"
        );
        assert!(
            answer.get("ExpiryTime").is_some(),
            "case {case:?}: the answer must report when the block tokens expire, got {answer}"
        );
        assert!(
            answer.get("NextToken").is_none(),
            "case {case:?}: a page that reported every changed block must carry no NextToken, \
             got {answer}"
        );
    }
}

/// RULE 1, over the volume sizes and block counts the comparison is independent of: a child
/// that inherits every block of its parent and writes nothing reports NO changed block,
/// whatever the snapshot holds.
///
/// The rows differ along the declared volume size and how many blocks (and which indexes)
/// the parent holds.
#[tokio::test]
async fn a_snapshot_that_inherited_everything_reports_no_changed_block() {
    let router = router();

    // (case, volume size, the parent's blocks).
    let rows: Vec<InheritanceRow> = vec![
        ("an empty parent", 1, Vec::new()),
        ("one block at index 0", 1, vec![(0, 1)]),
        (
            "three blocks spread across a larger volume",
            4,
            vec![(0, 2), (17, 3), (2048, 4)],
        ),
        ("one block at the last index of the volume", 1, vec![(2047, 5)]),
    ];

    for (case, volume_size, blocks) in rows {
        let parent = sealed_snapshot(&router, case, volume_size, None, &blocks).await;
        let child = sealed_snapshot(&router, case, volume_size, Some(&parent), &[]).await;

        let reply = compared(&router, case, Some(&parent), &child, "").await;
        assert_eq!(
            reported(&reply),
            Vec::new(),
            "case {case:?}: a child that wrote nothing of its own must report no changed \
             block, got {}",
            reply.json()
        );

        // … while the same child compared against NO first snapshot reports every block it
        // inherited (rule 4), which is what tells "nothing changed" from "nothing is there".
        let listed = compared(&router, case, None, &child, "maxResults=10000").await;
        let indexes: Vec<i32> = reported(&listed)
            .iter()
            .map(|block| block.block_index)
            .collect();
        let mut expected: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        expected.sort_unstable();
        assert_eq!(
            indexes, expected,
            "case {case:?}: compared against no snapshot, every inherited block must be \
             reported, got {}",
            listed.json()
        );
    }
}

/// RULE 2: a pair that shares no ancestor is refused `UNRELATED_SNAPSHOTS`, and an id that
/// names no snapshot of the service is refused for the reason its FORM gives — nothing is
/// compared either way.
///
/// The rows differ along which side is unrelated or unresolvable and how the id fails: two
/// independent snapshots in each order, a snapshot against a child of another lineage, a
/// well-formed id no snapshot is held under in each position, an id of a snapshot that is
/// still being written, and values that are not snapshot ids at all.
#[tokio::test]
async fn an_unrelated_or_unresolvable_snapshot_is_refused() {
    let router = router();
    let case = "the lineages under comparison";

    let left = sealed_snapshot(&router, case, 1, None, &[(0, 1)]).await;
    let right = sealed_snapshot(&router, case, 1, None, &[(0, 2)]).await;
    let right_child = sealed_snapshot(&router, case, 1, Some(&right), &[(1, 3)]).await;
    // A snapshot nothing has sealed: the read path resolves only a snapshot whose content is
    // final, so it is reported absent rather than compared half-written.
    let pending = {
        let started = start_snapshot(&router, &json!({ "VolumeSize": 1 })).await;
        member(&started, "SnapshotId")
    };

    // (case, first snapshot id, second snapshot id, status, error code, reason, message).
    let rows: Vec<(&str, String, String, u16, &str, &str, String)> = vec![
        (
            "two snapshots started with no parent",
            left.clone(),
            right.clone(),
            VALIDATION_STATUS,
            "ValidationException",
            "UNRELATED_SNAPSHOTS",
            format!("The snapshot {left} and {right} are not related."),
        ),
        (
            "the same pair in the other order",
            right.clone(),
            left.clone(),
            VALIDATION_STATUS,
            "ValidationException",
            "UNRELATED_SNAPSHOTS",
            format!("The snapshot {right} and {left} are not related."),
        ),
        (
            "a snapshot against a child of another lineage",
            left.clone(),
            right_child.clone(),
            VALIDATION_STATUS,
            "ValidationException",
            "UNRELATED_SNAPSHOTS",
            format!("The snapshot {left} and {right_child} are not related."),
        ),
        (
            "a well-formed id no snapshot is held under as the second snapshot",
            left.clone(),
            UNKNOWN_SNAPSHOT_ID.to_string(),
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            "SNAPSHOT_NOT_FOUND",
            format!("The snapshot '{UNKNOWN_SNAPSHOT_ID}' does not exist."),
        ),
        (
            "a well-formed id no snapshot is held under as the first snapshot",
            UNKNOWN_SNAPSHOT_ID.to_string(),
            left.clone(),
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            "SNAPSHOT_NOT_FOUND",
            format!("The snapshot '{UNKNOWN_SNAPSHOT_ID}' does not exist."),
        ),
        (
            "a snapshot that is still being written",
            pending.clone(),
            left.clone(),
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
            "SNAPSHOT_NOT_FOUND",
            format!("The snapshot '{pending}' does not exist."),
        ),
        (
            "a second snapshot id of no width the service issues",
            left.clone(),
            "snap-0".to_string(),
            VALIDATION_STATUS,
            "ValidationException",
            "INVALID_SNAPSHOT_ID",
            "Value (snap-0) for parameter secondSnapshotId is invalid. Expected: 'snap-...'."
                .to_string(),
        ),
        (
            "a first snapshot id whose digits are all zero",
            "snap-00000000".to_string(),
            left.clone(),
            VALIDATION_STATUS,
            "ValidationException",
            "INVALID_SNAPSHOT_ID",
            "Value (snap-00000000) for parameter firstSnapshotId is invalid. Expected: \
             'snap-...'."
                .to_string(),
        ),
    ];

    for (case, first, second, status, code, reason, message) in rows {
        let reply = list_changed_blocks(&router, &second, &format!("firstSnapshotId={first}")).await;
        assert_eq!(
            reply.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case:?}: expected the {code} refusal, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Reason"),
            reason,
            "case {case:?}: expected the reason {reason}, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Message"),
            message,
            "case {case:?}: expected the refusal wording for {first}/{second}, got {}",
            reply.json()
        );
    }
}

/// RULE 3: paging walks the one changed set — `MaxResults` bounds the page, the `NextToken`
/// resumes where it stopped, `StartingBlockIndex` starts at an index, and a token this
/// service did not issue is refused.
///
/// The rows differ along the page size (the modelled minimum, which the set is larger than,
/// and a size above the whole set) and along the index the listing is asked to start at.
#[tokio::test]
async fn paging_walks_the_changed_blocks_in_index_order() {
    let router = router();
    let case = "the paged lineage";

    // A set LARGER than the modelled minimum page size (`MaxResults` is `range
    // 100..10000`), so the token path is the only way to see all of it: the parent holds
    // nothing and the child writes one block at each index below `CHANGED_BLOCKS`.
    const CHANGED_BLOCKS: i32 = 101;
    let empty_parent = sealed_snapshot(&router, case, 1, None, &[]).await;
    let written: Vec<(i32, u8)> = (0..CHANGED_BLOCKS)
        .map(|index| (index, u8::try_from(index % 251).expect("a seed below 251")))
        .collect();
    let filled_child = sealed_snapshot(&router, case, 1, Some(&empty_parent), &written).await;
    let every_changed_index: Vec<i32> = (0..CHANGED_BLOCKS).collect();

    // (case, the page size the request asks for, how many pages that takes).
    let mut tokens_seen: Vec<String> = Vec::new();
    for (case, page_size, expected_pages) in [
        ("the modelled minimum page size", 100, 2),
        ("a page size above the whole set", 10_000, 1),
    ] {
        let mut collected: Vec<i32> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= expected_pages,
                "case {case:?}: a page size of {page_size} must take {expected_pages} pages \
                 for {CHANGED_BLOCKS} changed blocks, got at least {pages}"
            );
            let mut query = format!("maxResults={page_size}");
            if let Some(token) = &token {
                query.push_str(&format!("&pageToken={token}"));
            }
            let reply = compared(&router, case, Some(&empty_parent), &filled_child, &query).await;
            let page = reported(&reply);
            assert!(
                page.len() <= usize::try_from(page_size).expect("a page size that fits a usize"),
                "case {case:?}: a page must carry at most {page_size} blocks, got {}",
                page.len()
            );
            collected.extend(page.iter().map(|block| block.block_index));
            token = reply
                .json()
                .get("NextToken")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            if let Some(token) = &token {
                tokens_seen.push(token.clone());
            } else {
                break;
            }
        }
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages for a page size of {page_size}, \
             got {pages}"
        );
        assert_eq!(
            collected, every_changed_index,
            "case {case:?}: paging must report every changed index once, ascending, got \
             {} indexes starting {:?}",
            collected.len(),
            collected.iter().take(4).collect::<Vec<_>>()
        );
    }

    // A second lineage, small enough to list in one page: the parent holds four blocks and
    // the child overwrites two of them and adds two, so four indexes differ — 1 and 3
    // (overwritten), 4 and 6 (added) — and 0 and 2 do not.
    let parent = sealed_snapshot(
        &router,
        case,
        1,
        None,
        &[(0, 60), (1, 61), (2, 62), (3, 63)],
    )
    .await;
    let child = sealed_snapshot(
        &router,
        case,
        1,
        Some(&parent),
        &[(1, 71), (3, 73), (4, 74), (6, 76)],
    )
    .await;

    // `StartingBlockIndex`: "The list in the response will start from this block index or
    // the next valid block index in the snapshots."
    for (case, starting_index, expected) in [
        ("from the first changed index", 0, vec![1, 3, 4, 6]),
        ("from an index no block is changed at", 2, vec![3, 4, 6]),
        ("from the last changed index", 6, vec![6]),
        ("from past every changed index", 7, Vec::new()),
    ] {
        let reply = compared(
            &router,
            case,
            Some(&parent),
            &child,
            &format!("startingBlockIndex={starting_index}"),
        )
        .await;
        let indexes: Vec<i32> = reported(&reply)
            .iter()
            .map(|block| block.block_index)
            .collect();
        assert_eq!(
            indexes, expected,
            "case {case:?}: expected the changed indexes from {starting_index} to be \
             {expected:?}, got {}",
            reply.json()
        );
        assert!(
            reply.json().get("NextToken").is_none(),
            "case {case:?}: a page that reported the rest of the set must carry no NextToken, \
             got {}",
            reply.json()
        );
    }

    // A page token this service did not issue for THIS comparison — one it never minted, and
    // one it minted for another snapshot — is refused rather than read as an index.
    let foreign_token = tokens_seen
        .first()
        .cloned()
        .expect("the paged walk minted a token");
    for (case, token) in [
        ("a token this service never minted", "ff00".to_string()),
        ("a token minted for another snapshot", foreign_token),
    ] {
        let reply = list_changed_blocks(
            &router,
            &child,
            &format!("firstSnapshotId={parent}&pageToken={token}"),
        )
        .await;
        assert_eq!(
            reply.status,
            VALIDATION_STATUS,
            "case {case:?}: expected HTTP {VALIDATION_STATUS} for the token {token:?}, got {} \
             with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_PAGE_TOKEN",
            "case {case:?}: expected the page-token refusal, got {}",
            reply.json()
        );
    }
}
