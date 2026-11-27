// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListChangedBlocks`, driven over the same wire the sibling suites
//! use (the shared harness in [`super::snapshot_block_loop`]).
//!
//! The rules under test are:
//!
//! 1. THE COMPARISON IS OF THE CONTENT THE SECOND SNAPSHOT HOLDS: the blocks reported for
//!    two snapshots of one lineage are exactly the indexes the SECOND snapshot holds a block
//!    at whose bytes the first does not hold there — one the first holds no block at, or one
//!    it holds other bytes at — ascending by index, each carrying that second snapshot's own
//!    `BlockToken` and the first snapshot's only where that side holds the block too. An
//!    index only the FIRST snapshot holds is not a block of the second and is never
//!    reported. A snapshot compared against one it inherited every block from reports no
//!    changed block.
//!    (See [`super::a_changed_block_is_a_block_of_the_second_snapshot`] for that rule
//!    asserted on its own.)
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
//! 4. A COMPARISON IS OF TWO SNAPSHOTS: "The FirstSnapshotID parameter must be specified
//!    with a SecondSnapshotId parameter; otherwise, an error occurs" (`FirstSnapshotId`), so
//!    a request that names no first snapshot is refused with the modelled
//!    `ValidationException` carrying the `INVALID_SNAPSHOT_ID` reason and a sentence naming
//!    that member's role. A first snapshot that holds no block is a BASELINE holding
//!    nothing, which is a comparison: every block the second snapshot holds is reported,
//!    each with no `FirstBlockToken` — "this value is absent if the first snapshot does not
//!    have the changed block that is on the second snapshot" (`ChangedBlock`).
//! 5. THE SNAPSHOT THE URI NAMES MUST HOLD CONTENT: one that holds no block at all has
//!    nothing for the comparison to report, so it is refused with the same
//!    `INVALID_SNAPSHOT_ID` validation, worded with the snapshot's own id.
//! 6. EACH OUTPUT MEMBER IS REPORTED FROM WHAT IT DESCRIBES: an answer that reports a
//!    changed block reports the `VolumeSize` and `BlockSize` of the snapshots under
//!    comparison and the `ExpiryTime` of the tokens it handed out ("the time when the
//!    `BlockToken` expires"), while a comparison that reports NO changed block reports the
//!    empty list alone — it states that the two snapshots hold the same content and nothing
//!    about the geometry of either.
//! 7. A REPORTED TOKEN IS THE ONE ITS OWN SIDE HANDS OUT: the token reported for a side at a
//!    changed index is a token of THAT snapshot, so `GetSnapshotBlock` on that snapshot at
//!    that index answers with exactly the bytes that side holds there, and the token of the
//!    other side names no block of it.
//! 8. A COMPARISON IS A READ: it changes nothing a later caller sees — repeating it answers
//!    the same thing, and each side still lists and serves the same blocks under the same
//!    tokens afterwards.
//!
//! Each table row is a distinct input along what the rules generalise over — how the two
//! snapshots are related (parent/child, two generations, two siblings, a snapshot against
//! itself, the comparison reversed), which blocks were written and with which bytes, the
//! declared volume size, the page size and where the listing starts — so no row is the
//! specification.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, get_block, list_blocks, listed_blocks, member,
    put_block, router, send, start_snapshot, Reply, BLOCK_SIZE,
};

/// The HTTP status the modelled `ValidationException` carries (`@httpError 400`).
const VALIDATION_STATUS: u16 = 400;

/// The HTTP status the modelled `ResourceNotFoundException` carries (`@httpError 404`).
const NOT_FOUND_STATUS: u16 = 404;

/// A well-formed snapshot id (the 17 hex digit form this service mints) that no snapshot of
/// the store is held under, for the not-found half of rule 2.
const UNKNOWN_SNAPSHOT_ID: &str = "snap-0123456789abcdef0";

/// The error type the modelled `ValidationException` is dispatched on, as the
/// `x-amzn-errortype` header carries it.
const VALIDATION_ERROR_TYPE: &str = "ValidationException";

/// The fixed sentence the refusal of a request that names no first snapshot carries: it
/// names that member's role in the comparison and no id (rule 4).
const MISSING_FIRST_SNAPSHOT_MESSAGE: &str = "First snapshot should not be empty or null";

/// The sentence the refusal of an empty snapshot the URI NAMES carries: fixed text around
/// the one variable part, the snapshot id the request spelled (rule 5).
fn empty_snapshot_message(snapshot_id: &str) -> String {
    format!("Snapshot ({snapshot_id}) is empty")
}

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
///
/// `pub(crate)` so a sibling suite asserting a rule over BOTH listings of this service
/// drives the same wire path rather than growing a second copy of it.
pub(crate) async fn compared(
    router: &MultiServiceRouter,
    case: &str,
    first: &str,
    second: &str,
    extra: &str,
) -> Reply {
    let mut query = format!("firstSnapshotId={first}");
    if !extra.is_empty() {
        query.push('&');
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
///
/// `pub(crate)` so the sibling suite asserting the second-side rule reads a reported block
/// the same way rather than growing a second copy of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Changed {
    pub(crate) block_index: i32,
    pub(crate) first_block_token: Option<String>,
    pub(crate) second_block_token: Option<String>,
}

/// The changed blocks a listing reported, in the order it reported them.
pub(crate) fn reported(reply: &Reply) -> Vec<Changed> {
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

/// RULE 6, asserted on one answer: an answer that reports a changed block reports the
/// snapshot geometry (`VolumeSize`, `BlockSize`) and the expiry of the tokens it handed out,
/// while one that reports none reports `ChangedBlocks` alone.
///
/// `volume_size` is the size the snapshots under comparison were declared with, which is
/// what an answer that reports a changed block of them must report.
fn assert_token_members(case: &str, reply: &Reply, hands_out_tokens: bool, volume_size: i64) {
    let answer = reply.json();
    assert!(
        answer.get("ChangedBlocks").is_some(),
        "case {case:?}: every answer must report ChangedBlocks, got {answer}"
    );
    if hands_out_tokens {
        assert_eq!(
            answer.get("BlockSize").and_then(serde_json::Value::as_i64),
            Some(i64::from(BLOCK_SIZE)),
            "case {case:?}: an answer reporting a changed block must report the block size \
             of the snapshots compared, got {answer}"
        );
        assert_eq!(
            answer.get("VolumeSize").and_then(serde_json::Value::as_i64),
            Some(volume_size),
            "case {case:?}: an answer reporting a changed block must report the volume size \
             of the snapshots compared, got {answer}"
        );
        assert!(
            answer.get("ExpiryTime").is_some(),
            "case {case:?}: a page that handed out block tokens must report when they expire, \
             got {answer}"
        );
    } else {
        for member in ["BlockSize", "VolumeSize", "ExpiryTime"] {
            assert!(
                answer.get(member).is_none(),
                "case {case:?}: a page that reports no changed block must omit {member}, got \
                 {answer}"
            );
        }
    }
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
type ComparisonRow = (&'static str, Side, Side, Vec<(i32, bool, bool)>);

/// One row of the inheritance table: the case, the declared volume size, and the blocks the
/// parent holds as `(block index, byte seed)` pairs.
type InheritanceRow = (&'static str, i64, Vec<(i32, u8)>);

/// One row of the token-member table: the case, the first side, the second side, the query
/// the request carries, the volume size both sides were declared with, and the changed
/// indexes the page must report.
type TokenMemberRow<'a> = (&'a str, &'a str, &'a str, &'a str, i64, Vec<i32>);

/// Which snapshot of the lineage a row compares, so a row can name one the row before it
/// built.
#[derive(Debug, Clone, Copy)]
enum Side {
    Base,
    Parent,
    Child,
    Grandchild,
    Sibling,
}

/// RULE 1 and RULE 4: the reported blocks are exactly the blocks the second snapshot holds
/// whose bytes the first snapshot does not hold at that index, ascending, each carrying the
/// second side's token and the first side's only where that side holds the block — and a
/// comparison against a first snapshot that holds NOTHING reports every block the named
/// snapshot holds.
///
/// One lineage is built once — an empty base, a parent taken from it, a child of the parent,
/// a grandchild of the child and a second child (sibling) of the parent — and the rows
/// compare different pairs of it: a child that only added a block, a child that overwrote
/// one with other bytes, a child that changed nothing, two generations apart, two siblings,
/// a snapshot against itself, the same pair with the two sides swapped, and a snapshot
/// against the empty base.
#[tokio::test]
async fn the_reported_blocks_are_the_indexes_whose_content_differs() {
    let router = router();
    let case = "the lineage under comparison";

    // The base holds nothing. The parent, taken from it, holds blocks 0 and 1. The child
    // inherits both, overwrites 1 with other bytes and adds 2. The grandchild inherits all
    // three and adds 3. The sibling inherits the parent's two and adds 4.
    let base = sealed_snapshot(&router, case, 1, None, &[]).await;
    let parent = sealed_snapshot(&router, case, 1, Some(&base), &[(0, 10), (1, 11)]).await;
    let child = sealed_snapshot(&router, case, 1, Some(&parent), &[(1, 21), (2, 22)]).await;
    let grandchild = sealed_snapshot(&router, case, 1, Some(&child), &[(3, 33)]).await;
    let sibling = sealed_snapshot(&router, case, 1, Some(&parent), &[(4, 44)]).await;

    let id = |side: Side| match side {
        Side::Base => base.clone(),
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
            Side::Parent,
            Side::Child,
            vec![(1, true, true), (2, false, true)],
        ),
        (
            // Reversed, the child's added block 2 is an index the SECOND side holds no
            // block at, so it is no changed block OF the second snapshot: only index 1,
            // which both hold different bytes at, is reported.
            "the same pair with the two sides swapped",
            Side::Child,
            Side::Parent,
            vec![(1, true, true)],
        ),
        (
            "two generations apart",
            Side::Parent,
            Side::Grandchild,
            vec![(1, true, true), (2, false, true), (3, false, true)],
        ),
        (
            "a child that changed nothing its parent did not hold",
            Side::Child,
            Side::Grandchild,
            vec![(3, false, true)],
        ),
        (
            // The sibling's own block 4 is held by the FIRST side alone, so the comparison
            // of it against the child reports only what the child holds: 1 (other bytes)
            // and 2 (a block the sibling holds nothing at).
            "two siblings of one parent",
            Side::Sibling,
            Side::Child,
            vec![(1, true, true), (2, false, true)],
        ),
        ("a snapshot against itself", Side::Child, Side::Child, Vec::new()),
        (
            "an empty baseline: every block the named snapshot holds",
            Side::Base,
            Side::Child,
            vec![(0, false, true), (1, false, true), (2, false, true)],
        ),
        (
            "an empty baseline and a snapshot holding another block",
            Side::Base,
            Side::Sibling,
            vec![(0, false, true), (1, false, true), (4, false, true)],
        ),
    ];

    for (case, first, second, expected) in rows {
        let first_id = id(first);
        let second_id = id(second);
        let reply = compared(&router, case, &first_id, &second_id, "").await;
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
            assert!(
                *second_holds && block.second_block_token.is_some(),
                "case {case:?}: block {index} is a block of the second snapshot, so it must \
                 report a SecondBlockToken, got {block:?}"
            );
            // The two sides are distinct snapshots, so one index never reports one token
            // twice: each side hands out its own.
            if *first_holds && *second_holds && first_id != second_id {
                assert_ne!(
                    block.first_block_token, block.second_block_token,
                    "case {case:?}: block {index} must report each side's own token, got \
                     {block:?}"
                );
            }
        }

        // The members that describe the block tokens a page handed out are reported only
        // where the page handed any out (rule 6).
        let answer = reply.json();
        assert_token_members(case, &reply, !blocks.is_empty(), 1);
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

    // (case, volume size, the parent's blocks). Every parent holds at least one block, and
    // the empty BASE each lineage starts from is the baseline the second half compares
    // against: a snapshot the URI names that holds no block is refused (rule 5, covered by
    // `a_comparison_needs_a_first_snapshot_and_a_second_that_holds_content`).
    let rows: Vec<InheritanceRow> = vec![
        ("one block at index 0", 1, vec![(0, 1)]),
        (
            "three blocks spread across a larger volume",
            4,
            vec![(0, 2), (17, 3), (2048, 4)],
        ),
        ("one block at the last index of the volume", 1, vec![(2047, 5)]),
    ];

    for (case, volume_size, blocks) in rows {
        let base = sealed_snapshot(&router, case, volume_size, None, &[]).await;
        let parent = sealed_snapshot(&router, case, volume_size, Some(&base), &blocks).await;
        let child = sealed_snapshot(&router, case, volume_size, Some(&parent), &[]).await;

        let reply = compared(&router, case, &parent, &child, "").await;
        assert_eq!(
            reported(&reply),
            Vec::new(),
            "case {case:?}: a child that wrote nothing of its own must report no changed \
             block, got {}",
            reply.json()
        );

        // … while the same child compared against the EMPTY base reports every block it
        // inherited (rule 4), which is what tells "nothing changed" from "nothing is there".
        let listed = compared(&router, case, &base, &child, "maxResults=10000").await;
        let indexes: Vec<i32> = reported(&listed)
            .iter()
            .map(|block| block.block_index)
            .collect();
        let mut expected: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        expected.sort_unstable();
        assert_eq!(
            indexes, expected,
            "case {case:?}: compared against the empty base, every inherited block must be \
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
            let reply = compared(&router, case, &empty_parent, &filled_child, &query).await;
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
            &parent,
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

/// RULES 4 and 5: a comparison names TWO snapshots and reports the content of the one the
/// URI names, so a request that names no first snapshot, and one whose second snapshot holds
/// no block, are both refused with the `INVALID_SNAPSHOT_ID` validation — never answered as
/// a comparison that found nothing.
///
/// The rows differ along which requirement the request fails and how (the member absent,
/// against a second side holding blocks and against one holding none; a snapshot that was
/// started from no parent and written to never, and one taken from such a parent so it
/// inherited nothing), along the declared volume size and along what the other side holds —
/// so no row is the specification. The two refusals are told apart by their message: the
/// missing first side by that member's role, the snapshot the URI names by its id.
#[tokio::test]
async fn a_comparison_needs_a_first_snapshot_and_a_second_that_holds_content() {
    let router = router();
    let case = "the lineages under comparison";

    // A snapshot nothing was ever written into, a child of it that wrote nothing either (so
    // it inherited nothing and holds nothing), and a child of it that did write.
    let empty = sealed_snapshot(&router, case, 1, None, &[]).await;
    let empty_child = sealed_snapshot(&router, case, 1, Some(&empty), &[]).await;
    let filled_child = sealed_snapshot(&router, case, 1, Some(&empty), &[(0, 9), (3, 10)]).await;
    // The same shape over a larger volume, with the written blocks at high indexes.
    let bigger_empty = sealed_snapshot(&router, case, 4, None, &[]).await;
    let bigger_filled_child =
        sealed_snapshot(&router, case, 4, Some(&bigger_empty), &[(2048, 11), (8191, 12)]).await;

    // (case, the `firstSnapshotId` the request carries, the second side, the message the
    // refusal must carry).
    let rows: Vec<(&str, Option<&str>, &str, String)> = vec![
        (
            "no first snapshot is named at all",
            None,
            filled_child.as_str(),
            MISSING_FIRST_SNAPSHOT_MESSAGE.to_string(),
        ),
        (
            "no first snapshot is named and the second holds nothing either",
            None,
            empty.as_str(),
            MISSING_FIRST_SNAPSHOT_MESSAGE.to_string(),
        ),
        (
            "no first snapshot is named and the second is of a larger volume",
            None,
            bigger_filled_child.as_str(),
            MISSING_FIRST_SNAPSHOT_MESSAGE.to_string(),
        ),
        (
            "the snapshot the URI names holds no block",
            Some(filled_child.as_str()),
            empty.as_str(),
            empty_snapshot_message(&empty),
        ),
        (
            "the snapshot the URI names inherited nothing from an empty parent",
            Some(empty.as_str()),
            empty_child.as_str(),
            empty_snapshot_message(&empty_child),
        ),
        (
            "the empty snapshot the URI names is of a larger volume",
            Some(bigger_filled_child.as_str()),
            bigger_empty.as_str(),
            empty_snapshot_message(&bigger_empty),
        ),
    ];

    for (case, first, second, message) in rows {
        let query = match first {
            Some(first) => format!("firstSnapshotId={first}"),
            None => String::new(),
        };
        let reply = list_changed_blocks(&router, second, &query).await;
        assert_eq!(
            reply.status,
            VALIDATION_STATUS,
            "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.header("x-amzn-errortype").contains(VALIDATION_ERROR_TYPE),
            "case {case:?}: expected the x-amzn-errortype header to name \
             {VALIDATION_ERROR_TYPE}, got {:?}",
            reply.header("x-amzn-errortype")
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_SNAPSHOT_ID",
            "case {case:?}: expected the INVALID_SNAPSHOT_ID reason, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Message"),
            message,
            "case {case:?}: expected the refusal wording for this request, got {}",
            reply.json()
        );
        assert!(
            reply.json().get("ChangedBlocks").is_none(),
            "case {case:?}: a refused comparison must report no changed blocks, got {}",
            reply.json()
        );
        // The fixed missing-first-snapshot sentence names the member's ROLE, so that refusal
        // carries no snapshot id at all.
        if message.as_str() == MISSING_FIRST_SNAPSHOT_MESSAGE {
            assert!(
                !member(&reply, "Message").contains("snap-"),
                "case {case:?}: the missing-first-snapshot refusal must name no snapshot id, \
                 got {}",
                reply.json()
            );
        }
    }
}

/// RULE 6: each output member is reported from what it describes — a comparison that reports
/// no changed block reports the empty list alone, omitting `ExpiryTime`, `VolumeSize` and
/// `BlockSize`, while a page that reports changed blocks carries all three.
///
/// The rows differ along WHY the page reports nothing (a snapshot compared against itself, a
/// child that wrote nothing of its own, a listing starting past every changed index), along
/// the declared volume size and the indexes involved, and include pages that DO report
/// blocks — so the rule is asserted in both directions rather than on one shape of answer.
#[tokio::test]
async fn a_page_with_no_changed_block_reports_the_empty_list_alone() {
    let router = router();
    let case = "the compared lineage";

    // One lineage over a 1 GiB volume: the parent holds two blocks, the child inherits both,
    // overwrites one and adds one (so indexes 5 and 9 differ), and the grandchild writes
    // nothing of its own.
    let parent = sealed_snapshot(&router, case, 1, None, &[(0, 1), (5, 2)]).await;
    let child = sealed_snapshot(&router, case, 1, Some(&parent), &[(5, 3), (9, 4)]).await;
    let grandchild = sealed_snapshot(&router, case, 1, Some(&child), &[]).await;
    // A second lineage over a 4 GiB volume, with its one changed block at a high index.
    let wide_parent = sealed_snapshot(&router, case, 4, None, &[(2048, 5)]).await;
    let wide_child = sealed_snapshot(&router, case, 4, Some(&wide_parent), &[(6000, 6)]).await;

    // (case, the first side, the second side, the query, the volume size, the indexes the
    // page must report).
    let rows: Vec<TokenMemberRow<'_>> = vec![
        (
            "a snapshot compared against itself",
            child.as_str(),
            child.as_str(),
            "",
            1,
            Vec::new(),
        ),
        (
            "a child that wrote nothing of its own",
            child.as_str(),
            grandchild.as_str(),
            "",
            1,
            Vec::new(),
        ),
        (
            "a listing starting past every changed index",
            parent.as_str(),
            child.as_str(),
            "startingBlockIndex=10",
            1,
            Vec::new(),
        ),
        (
            "a listing starting past the one changed index of a larger volume",
            wide_parent.as_str(),
            wide_child.as_str(),
            "startingBlockIndex=6001",
            4,
            Vec::new(),
        ),
        (
            "a page that reports every changed block",
            parent.as_str(),
            child.as_str(),
            "",
            1,
            vec![5, 9],
        ),
        (
            "a page that reports the last changed block",
            parent.as_str(),
            child.as_str(),
            "startingBlockIndex=9",
            1,
            vec![9],
        ),
        (
            "a page of the larger volume that reports its changed block",
            wide_parent.as_str(),
            wide_child.as_str(),
            "startingBlockIndex=2048",
            4,
            vec![6000],
        ),
    ];

    for (case, first, second, query, volume_size, expected) in rows {
        let reply = compared(&router, case, first, second, query).await;
        let indexes: Vec<i32> = reported(&reply)
            .iter()
            .map(|block| block.block_index)
            .collect();
        assert_eq!(
            indexes, expected,
            "case {case:?}: expected the changed indexes {expected:?}, got {}",
            reply.json()
        );
        assert_token_members(case, &reply, !expected.is_empty(), volume_size);
    }
}

/// The reason the modelled `ValidationException` carries for a `blockToken` that names no
/// block of the snapshot the request addressed (`ValidationExceptionReason`).
const INVALID_BLOCK_TOKEN_REASON: &str = "INVALID_BLOCK_TOKEN";

/// One row of the token table: the case, the first side's id and the `(block index, byte
/// seed)` pairs it holds, then the second side's id and the pairs it holds.
///
/// The content each side holds is what a token reported for it must read back, so the row
/// states it rather than the answer being compared with itself.
type TokenRow<'a> = (&'a str, &'a str, Vec<(i32, u8)>, &'a str, Vec<(i32, u8)>);

/// The byte seed a side holds at `block_index`, or `None` when it holds no block there.
fn seed_at(blocks: &[(i32, u8)], block_index: i32) -> Option<u8> {
    blocks
        .iter()
        .find(|(index, _)| *index == block_index)
        .map(|(_, seed)| *seed)
}

/// Every changed block of `second`, ascending: the indexes `second` holds a block at whose
/// seed `first` does not hold at that index.
fn changed_indexes_of_second(first: &[(i32, u8)], second: &[(i32, u8)]) -> Vec<i32> {
    let mut indexes: Vec<i32> = second
        .iter()
        .filter(|(index, seed)| seed_at(first, *index) != Some(*seed))
        .map(|(index, _)| *index)
        .collect();
    indexes.sort_unstable();
    indexes.dedup();
    indexes
}

/// RULE 7, for one reported token: reading `block_index` of `snapshot_id` with it answers
/// with exactly the bytes `seed` names, and the geometry derived from those bytes.
async fn assert_token_reads_back(
    router: &MultiServiceRouter,
    case: &str,
    side: &str,
    snapshot_id: &str,
    block_index: i32,
    block_token: &str,
    seed: u8,
) {
    let expected = block_bytes(seed);
    let fetched = get_block(router, snapshot_id, block_index, block_token).await;
    assert_eq!(
        fetched.status,
        200,
        "case {case:?}: the {side} token for block {block_index} must read that block of \
         {snapshot_id} back, got {} with body {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
    assert!(
        fetched.body == expected,
        "case {case:?}: the {side} token for block {block_index} must read the bytes that side \
         holds, got {} bytes with checksum {:?}",
        fetched.body.len(),
        checksum(&fetched.body)
    );
    assert_eq!(
        fetched.header("x-amz-Checksum"),
        checksum(&expected),
        "case {case:?}: the {side} read of block {block_index} must report the checksum of \
         those bytes, got {:?}",
        fetched.header("x-amz-Checksum")
    );
    assert_eq!(
        fetched.header("x-amz-Data-Length"),
        BLOCK_SIZE.to_string(),
        "case {case:?}: the {side} read of block {block_index} must report the stored length, \
         got {:?}",
        fetched.header("x-amz-Data-Length")
    );
}

/// RULE 7, the other half: `block_token` names no block of `snapshot_id` at `block_index`,
/// so the read is refused as an invalid block token and hands out no bytes.
async fn assert_token_names_no_block(
    router: &MultiServiceRouter,
    case: &str,
    note: &str,
    snapshot_id: &str,
    block_index: i32,
    block_token: &str,
) {
    let fetched = get_block(router, snapshot_id, block_index, block_token).await;
    assert_eq!(
        fetched.status, VALIDATION_STATUS,
        "case {case:?}: {note} must be refused, got {} with body {}",
        fetched.status,
        String::from_utf8_lossy(&fetched.body)
    );
    assert_eq!(
        fetched.error_code(),
        VALIDATION_ERROR_TYPE,
        "case {case:?}: {note} must be refused as a validation error, got {:?}",
        fetched.error_code()
    );
    assert_eq!(
        member(&fetched, "Reason"),
        INVALID_BLOCK_TOKEN_REASON,
        "case {case:?}: {note} must be refused as an invalid block token, got {}",
        fetched.json()
    );
}

/// The `(block index, block token)` pairs `ListSnapshotBlocks` reports for `snapshot_id`.
async fn listing_of(
    router: &MultiServiceRouter,
    case: &str,
    snapshot_id: &str,
) -> Vec<(i32, String)> {
    let reply = list_blocks(router, snapshot_id, "").await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: ListSnapshotBlocks of {snapshot_id} must answer 200, got {} with body {}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    listed_blocks(&reply)
}

/// RULE 7: every token a comparison reports belongs to the side it was reported for — it
/// reads that side's own bytes back through `GetSnapshotBlock`, and it names no block of the
/// other side.
///
/// One lineage is built once — an empty base, a parent taken from it, a child of the parent
/// that rewrites one block and adds one, a grandchild that rewrites another, and a second
/// child of the parent — and the rows compare different pairs of it, so the rows differ along
/// how the two sides are related (parent/child, the same pair reversed, two generations, two
/// siblings, a snapshot against an ancestor that holds nothing), along which indexes each side
/// holds, and along whether the changed index is held by one side or by both with other bytes.
#[tokio::test]
async fn each_reported_token_reads_its_own_sides_bytes_back() {
    let router = router();
    let case = "the lineage whose tokens are read back";

    // The base holds nothing. The parent holds 0 and 5. The child inherits both, rewrites 5
    // with other bytes and adds 9. The grandchild inherits all three and rewrites 0. The
    // sibling inherits the parent's two and rewrites 5 with bytes of its own.
    let base = sealed_snapshot(&router, case, 2, None, &[]).await;
    let parent = sealed_snapshot(&router, case, 2, Some(&base), &[(0, 61), (5, 62)]).await;
    let child = sealed_snapshot(&router, case, 2, Some(&parent), &[(5, 63), (9, 64)]).await;
    let grandchild = sealed_snapshot(&router, case, 2, Some(&child), &[(0, 65)]).await;
    let sibling = sealed_snapshot(&router, case, 2, Some(&parent), &[(5, 66)]).await;

    // What each snapshot HOLDS, inherited blocks included: a snapshot taken from a parent
    // starts with the parent's blocks, and its own writes replace them at the indexes it
    // wrote.
    let parent_blocks = vec![(0, 61), (5, 62)];
    let child_blocks = vec![(0, 61), (5, 63), (9, 64)];
    let grandchild_blocks = vec![(0, 65), (5, 63), (9, 64)];
    let sibling_blocks = vec![(0, 61), (5, 66)];

    let rows: Vec<TokenRow<'_>> = vec![
        (
            "a child that rewrote one block and added one",
            parent.as_str(),
            parent_blocks.clone(),
            child.as_str(),
            child_blocks.clone(),
        ),
        (
            "the same pair with the two sides swapped",
            child.as_str(),
            child_blocks.clone(),
            parent.as_str(),
            parent_blocks.clone(),
        ),
        (
            "two generations apart",
            parent.as_str(),
            parent_blocks.clone(),
            grandchild.as_str(),
            grandchild_blocks.clone(),
        ),
        (
            "two siblings of one parent",
            sibling.as_str(),
            sibling_blocks.clone(),
            child.as_str(),
            child_blocks.clone(),
        ),
        (
            "an ancestor that holds nothing",
            base.as_str(),
            Vec::new(),
            child.as_str(),
            child_blocks.clone(),
        ),
    ];

    for (case, first, first_blocks, second, second_blocks) in rows {
        let reply = compared(&router, case, first, second, "").await;
        let blocks = reported(&reply);
        let expected = changed_indexes_of_second(&first_blocks, &second_blocks);
        let indexes: Vec<i32> = blocks.iter().map(|block| block.block_index).collect();
        assert_eq!(
            indexes, expected,
            "case {case:?}: expected the changed indexes {expected:?}, got {}",
            reply.json()
        );

        for block in &blocks {
            let index = block.block_index;
            let first_seed = seed_at(&first_blocks, index);
            let second_seed = seed_at(&second_blocks, index);
            assert_eq!(
                block.first_block_token.is_some(),
                first_seed.is_some(),
                "case {case:?}: block {index} must carry a FirstBlockToken exactly when the \
                 first snapshot holds that block, got {block:?}"
            );
            assert!(
                second_seed.is_some() && block.second_block_token.is_some(),
                "case {case:?}: block {index} is a block of the second snapshot, so it must \
                 carry a SecondBlockToken, got {block:?}"
            );

            if let (Some(token), Some(seed)) = (block.first_block_token.as_deref(), first_seed) {
                assert_token_reads_back(&router, case, "first", first, index, token, seed).await;
            }
            if let (Some(token), Some(seed)) = (block.second_block_token.as_deref(), second_seed) {
                assert_token_reads_back(&router, case, "second", second, index, token, seed).await;
            }

            // At an index BOTH sides hold, each side hands out its own token for its own
            // version of the block, so the two tokens differ and neither names a block of
            // the other snapshot.
            let both = (
                block.first_block_token.as_deref(),
                block.second_block_token.as_deref(),
            );
            if let (Some(first_token), Some(second_token)) = both {
                assert_ne!(
                    first_token, second_token,
                    "case {case:?}: the two sides of block {index} must hand out their own \
                     tokens, got {block:?}"
                );
                assert_token_names_no_block(
                    &router,
                    case,
                    "the first side's token read against the second snapshot",
                    second,
                    index,
                    first_token,
                )
                .await;
                assert_token_names_no_block(
                    &router,
                    case,
                    "the second side's token read against the first snapshot",
                    first,
                    index,
                    second_token,
                )
                .await;
            }
        }
    }
}

/// RULE 8: comparing two snapshots is a READ — it changes nothing a later caller sees.
///
/// Each row compares a pair twice and asserts the second answer is the first one again, then
/// asserts each side still lists exactly the blocks it listed before, under the same tokens,
/// and that those tokens still serve their bytes. The rows differ along which pair is
/// compared, along the direction of the comparison, along the query the comparison carries,
/// and along whether the answer reports changed blocks at all.
#[tokio::test]
async fn comparing_two_snapshots_leaves_both_sides_as_they_were() {
    let router = router();
    let case = "the lineage compared twice";

    let base = sealed_snapshot(&router, case, 3, None, &[]).await;
    let parent = sealed_snapshot(&router, case, 3, Some(&base), &[(1, 71), (2, 72)]).await;
    let child = sealed_snapshot(&router, case, 3, Some(&parent), &[(2, 73), (7, 74)]).await;

    // (case, the first side, the second side, the query the comparison carries).
    let rows: [(&str, &str, &str, &str); 5] = [
        (
            "a parent against its child",
            parent.as_str(),
            child.as_str(),
            "",
        ),
        ("the same pair reversed", child.as_str(), parent.as_str(), ""),
        (
            "a comparison that starts past the first changed index",
            parent.as_str(),
            child.as_str(),
            "startingBlockIndex=7",
        ),
        (
            "a comparison that reports nothing",
            child.as_str(),
            child.as_str(),
            "",
        ),
        (
            "an ancestor that holds nothing",
            base.as_str(),
            child.as_str(),
            "",
        ),
    ];

    for (case, first, second, query) in rows {
        let first_before = listing_of(&router, case, first).await;
        let second_before = listing_of(&router, case, second).await;

        let once = compared(&router, case, first, second, query).await;
        let twice = compared(&router, case, first, second, query).await;
        assert_eq!(
            reported(&once),
            reported(&twice),
            "case {case:?}: the comparison must report the same changed blocks every time, got \
             {} then {}",
            once.json(),
            twice.json()
        );
        assert_eq!(
            once.json(),
            twice.json(),
            "case {case:?}: the comparison must answer the same thing every time, got {} then \
             {}",
            once.json(),
            twice.json()
        );

        let first_after = listing_of(&router, case, first).await;
        let second_after = listing_of(&router, case, second).await;
        assert_eq!(
            first_before, first_after,
            "case {case:?}: the first snapshot must list the same blocks under the same tokens \
             after the comparison, got {first_after:?}"
        );
        assert_eq!(
            second_before, second_after,
            "case {case:?}: the second snapshot must list the same blocks under the same tokens \
             after the comparison, got {second_after:?}"
        );

        // The tokens the sides still hand out still serve their blocks: the comparison
        // neither consumed them nor released the bytes they name.
        for (snapshot_id, listed) in [(first, &first_after), (second, &second_after)] {
            for (block_index, block_token) in listed {
                let fetched = get_block(&router, snapshot_id, *block_index, block_token).await;
                assert_eq!(
                    fetched.status,
                    200,
                    "case {case:?}: block {block_index} of {snapshot_id} must still be served \
                     after the comparison, got {} with body {}",
                    fetched.status,
                    String::from_utf8_lossy(&fetched.body)
                );
            }
        }
    }
}
