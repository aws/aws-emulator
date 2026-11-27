// SPDX-License-Identifier: CC0-1.0

//! The one rule this suite asserts, end-to-end over `ListChangedBlocks` and driven over the
//! same wire the sibling suites use (the shared harness in [`super::snapshot_block_loop`]):
//!
//! A CHANGED BLOCK IS A BLOCK OF THE SNAPSHOT THE COMPARISON IS ADDRESSED AT. A
//! `ChangedBlock` is "a block of data in an Amazon Elastic Block Store snapshot that is
//! different from another snapshot of the same volume/snapshot lineage"
//! (`smithy_query(query="doc",shape="com.amazonaws.ebs#ChangedBlock")`), its
//! `SecondBlockToken` is "the block token for the block index of the `SecondSnapshotId`"
//! with no absence clause, and its `FirstBlockToken` "is absent if the first snapshot does
//! not have the changed block that is on the second snapshot"
//! (`smithy_query(query="doc",shape="com.amazonaws.ebs#ChangedBlock",member="FirstBlockToken")`).
//! So the reported set is the blocks the SECOND snapshot holds whose bytes the first does not
//! hold at that index — every reported block carries the second side's own token, which reads
//! that side's bytes back through `GetSnapshotBlock`, and carries the first side's token only
//! where that side holds the block too. An index only the FIRST snapshot holds is no block of
//! the second, so it is never reported.
//!
//! The rows differ along what each side holds (an index only the first side holds, several of
//! them, an index only the second holds, each side holding one the other does not, a first
//! side holding nothing, a side holding the same bytes everywhere the other one does), along
//! the block indexes and byte contents involved, and along the declared volume size — and the
//! second test drives the same rule over a parent and the child that inherited from it, in
//! both directions, so no row is the specification.

use smithy_server_runtime::MultiServiceRouter;

use super::changed_blocks_between_snapshots::{compared, reported, sealed_snapshot, Changed};
use super::snapshot_block_loop::{block_bytes, checksum, get_block, router, BLOCK_SIZE};

/// One row of the table: the case, the declared volume size, the `(block index, byte seed)`
/// pairs the FIRST side holds, the pairs the SECOND side holds, the changed indexes the
/// comparison must report, and the indexes only the first side holds — which are blocks of no
/// snapshot the comparison reports.
type Row = (
    &'static str,
    i64,
    Vec<(i32, u8)>,
    Vec<(i32, u8)>,
    Vec<i32>,
    Vec<i32>,
);

/// The byte seed a side holds at `block_index`, or `None` when it holds no block there.
fn seed_at(blocks: &[(i32, u8)], block_index: i32) -> Option<u8> {
    blocks
        .iter()
        .find(|(index, _)| *index == block_index)
        .map(|(_, seed)| *seed)
}

/// Assert, for one answered comparison, that every block it reported is a block of `second`
/// holding `second_blocks`: it carries that side's token, the token reads those bytes back,
/// the first side's token is carried exactly where `first_blocks` holds the index, and no
/// index of `first_only` was reported at all.
async fn assert_blocks_are_of_the_second_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    second: &str,
    first_blocks: &[(i32, u8)],
    second_blocks: &[(i32, u8)],
    first_only: &[i32],
    blocks: &[Changed],
) {
    for block in blocks {
        let index = block.block_index;
        assert!(
            !first_only.contains(&index),
            "case {case:?}: block {index} is held by the first snapshot alone, so it is no \
             changed block of the second and must not be reported, got {block:?}"
        );
        let second_seed = seed_at(second_blocks, index).unwrap_or_else(|| {
            panic!(
                "case {case:?}: block {index} must be one the second snapshot holds, got \
                 {block:?}"
            )
        });
        let token = block.second_block_token.as_deref().unwrap_or_else(|| {
            panic!(
                "case {case:?}: block {index} must carry a SecondBlockToken, got {block:?}"
            )
        });
        assert_eq!(
            block.first_block_token.is_some(),
            seed_at(first_blocks, index).is_some(),
            "case {case:?}: block {index} must carry a FirstBlockToken exactly when the first \
             snapshot holds that block, got {block:?}"
        );

        // The second side's token is a token OF the second snapshot: it reads exactly the
        // bytes that side holds at the index back.
        let expected = block_bytes(second_seed);
        let fetched = get_block(router, second, index, token).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: the SecondBlockToken of block {index} must read that block of the \
             second snapshot back, got {} with body {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert!(
            fetched.body == expected,
            "case {case:?}: the SecondBlockToken of block {index} must read the bytes the \
             second snapshot holds, got {} bytes with checksum {:?}",
            fetched.body.len(),
            checksum(&fetched.body)
        );
        assert_eq!(
            fetched.header("x-amz-Data-Length"),
            BLOCK_SIZE.to_string(),
            "case {case:?}: the read of block {index} must report the stored length, got {:?}",
            fetched.header("x-amz-Data-Length")
        );
    }
}

/// THE RULE over two snapshots of one lineage whose held sets the row states: the comparison
/// reports the blocks of the second snapshot alone.
///
/// Each row builds its own lineage — an empty base with no parent, and one child of it per
/// side writing exactly the blocks the row gives that side — so the two sides share a lineage
/// while holding whatever the row says, in either relation to each other.
#[tokio::test]
async fn only_the_blocks_the_second_snapshot_holds_are_reported_as_changed() {
    let router = router();
    let case = "the lineages under comparison";

    let rows: Vec<Row> = vec![
        (
            "an index only the first side holds",
            1,
            vec![(0, 1), (9, 2)],
            vec![(0, 1)],
            Vec::new(),
            vec![9],
        ),
        (
            "an index only the first side holds beside one both hold other bytes at",
            1,
            vec![(0, 1), (5, 2), (9, 3)],
            vec![(0, 1), (5, 4)],
            vec![5],
            vec![9],
        ),
        (
            "several indexes only the first side holds",
            1,
            vec![(0, 1), (1, 2), (2, 3), (3, 4)],
            vec![(0, 1)],
            Vec::new(),
            vec![1, 2, 3],
        ),
        (
            "an index only the second side holds",
            1,
            vec![(0, 1)],
            vec![(0, 1), (7, 5)],
            vec![7],
            Vec::new(),
        ),
        (
            "each side holding an index the other does not",
            1,
            vec![(0, 1), (4, 6)],
            vec![(0, 1), (8, 7)],
            vec![8],
            vec![4],
        ),
        (
            "a first side holding nothing",
            1,
            Vec::new(),
            vec![(0, 1), (3, 2)],
            vec![0, 3],
            Vec::new(),
        ),
        (
            "high indexes over a larger volume",
            4,
            vec![(2048, 9), (8191, 10)],
            vec![(2048, 11)],
            vec![2048],
            vec![8191],
        ),
        (
            "a second side holding the same bytes everywhere it holds a block",
            2,
            vec![(0, 21), (5, 22), (6, 23)],
            vec![(5, 22)],
            Vec::new(),
            vec![0, 6],
        ),
    ];

    for (case_row, volume_size, first_blocks, second_blocks, expected, first_only) in rows {
        let base = sealed_snapshot(&router, case, volume_size, None, &[]).await;
        let first = sealed_snapshot(&router, case, volume_size, Some(&base), &first_blocks).await;
        let second = sealed_snapshot(&router, case, volume_size, Some(&base), &second_blocks).await;

        let reply = compared(&router, case_row, &first, &second, "maxResults=10000").await;
        let blocks = reported(&reply);
        let indexes: Vec<i32> = blocks.iter().map(|block| block.block_index).collect();
        assert_eq!(
            indexes, expected,
            "case {case_row:?}: expected the changed indexes {expected:?}, got {}",
            reply.json()
        );
        assert_blocks_are_of_the_second_snapshot(
            &router,
            case_row,
            &second,
            &first_blocks,
            &second_blocks,
            &first_only,
            &blocks,
        )
        .await;
    }
}

/// One row of the direction table: the case, the first side and the `(block index, byte
/// seed)` pairs it holds, the second side and the pairs it holds, the changed indexes the
/// comparison must report, and the indexes only the first side holds.
type DirectionRow<'a> = (
    &'a str,
    &'a str,
    &'a [(i32, u8)],
    &'a str,
    &'a [(i32, u8)],
    Vec<i32>,
    Vec<i32>,
);

/// THE SAME RULE over a parent and the child that inherited from it, in BOTH directions: the
/// child's own block is reported when the child is the snapshot the comparison is addressed
/// at, and reported by nothing when the parent is — a child inherits every block of its
/// parent, so reversing the pair is what leaves the first side holding an index the second
/// does not.
#[tokio::test]
async fn a_block_the_child_alone_holds_is_reported_only_for_the_child() {
    let router = router();
    let case = "a parent and the child that added one block";

    // The parent holds two blocks; the child inherits both, rewrites one with other bytes
    // and adds one of its own.
    let parent_blocks = vec![(0, 31), (5, 32)];
    let child_writes = vec![(5, 33), (9, 34)];
    let child_blocks = vec![(0, 31), (5, 33), (9, 34)];

    let parent = sealed_snapshot(&router, case, 2, None, &parent_blocks).await;
    let child = sealed_snapshot(&router, case, 2, Some(&parent), &child_writes).await;

    let directions: Vec<DirectionRow<'_>> = vec![
        (
            "the child as the snapshot the comparison is addressed at",
            parent.as_str(),
            &parent_blocks,
            child.as_str(),
            &child_blocks,
            vec![5, 9],
            Vec::new(),
        ),
        (
            "the same pair reversed, so the first side holds the added block",
            child.as_str(),
            &child_blocks,
            parent.as_str(),
            &parent_blocks,
            vec![5],
            vec![9],
        ),    ];

    for (case, first, first_blocks, second, second_blocks, expected, first_only) in directions {
        let reply = compared(&router, case, first, second, "").await;
        let blocks = reported(&reply);
        let indexes: Vec<i32> = blocks.iter().map(|block| block.block_index).collect();
        assert_eq!(
            indexes, expected,
            "case {case:?}: expected the changed indexes {expected:?}, got {}",
            reply.json()
        );
        assert_blocks_are_of_the_second_snapshot(
            &router,
            case,
            second,
            first_blocks,
            second_blocks,
            &first_only,
            &blocks,
        )
        .await;
    }
}
