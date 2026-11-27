// SPDX-License-Identifier: CC0-1.0

//! What `CompleteSnapshot`'s aggregate `Checksum` decides about the completion that
//! declares it.
//!
//! Every test drives real HTTP through the same `MultiServiceRouter` the production router
//! uses, over the harness `snapshot_block_loop` owns.
//!
//! RULE: `Checksum` is the producer's aggregate over the content it wrote ("An aggregated
//! Base-64 SHA256 checksum based on the checksums of each written block. To generate the
//! aggregated checksum using the linear aggregation method, arrange the checksums for each
//! written block in ascending order of their block index, concatenate them to form a single
//! string, and then generate the checksum on the entire string using the SHA256 algorithm",
//! `smithy_query(query="doc",shape="com.amazonaws.ebs#CompleteSnapshotRequest",member="Checksum")`),
//! so the service recomputes that value from the blocks the snapshot holds and serves only
//! the completion whose declared aggregate IS it: a completion declaring any other
//! well-formed value does not correspond to this snapshot's content, is refused through the
//! service's own modelled `ValidationException` (HTTP 400, `Reason`
//! `INVALID_PARAMETER_VALUE`) naming both checksums, and seals nothing. The member is
//! optional, so a completion that declares no aggregate is unaffected.
//!
//! Only the blocks a snapshot's own producer WROTE are aggregated — "each written block" —
//! so the blocks a snapshot inherits from its `ParentSnapshotId`, which were written to
//! that parent, are no part of its aggregate, exactly as they are no part of its
//! `ChangedBlocksCount`.
//!
//! Each half is asserted over a TABLE. The served rows differ along how many blocks were
//! written (none, one, two, three), where they sit (adjacent, sparse, the last index the
//! volume addresses), the order they were WRITTEN in (ascending, descending, interleaved —
//! the aggregation order is the block INDEX, not the write order), whether an index was
//! rewritten, and whether the snapshot inherited blocks from a parent. The refused rows
//! differ along HOW the declared value misses: the aggregate of no block at all, of the
//! same checksums in descending index order, of the raw digests instead of their Base64
//! text, of a subset or a superset of the blocks written, one block's own per-block
//! checksum, and a well-formed value that is the checksum of nothing.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::declared_block_inputs::assert_validation;
// The two checksums a mismatch names, worded once by the suite that owns that sentence for
// the `x-amz-Checksum` header: the value the service computed, then the one the request
// supplied.
use crate::payload_checksum_integrity::mismatch_message;
use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, list_blocks, listed_blocks, member, put_block,
    router, send, start_snapshot, write_snapshot, Reply, BLOCK_SIZE,
};

/// The reason a refused aggregate carries: the value of a parameter is not one the service
/// accepts for it (`ValidationExceptionReason`,
/// `smithy_query(query="shape",shape="com.amazonaws.ebs#ValidationExceptionReason")`).
const REFUSAL_REASON: &str = "INVALID_PARAMETER_VALUE";

/// The LINEAR aggregation of `block_checksums`, computed here independently of the service:
/// the per-block checksums — already in ascending block-index order — concatenated into a
/// single string, with the SHA256 of that whole string reported Base64.
///
/// `pub(crate)` so a sibling suite that completes a snapshot with the aggregate its content
/// really has names this composition rather than growing a second copy of it.
pub(crate) fn linear_aggregate(block_checksums: &[String]) -> String {
    checksum(block_checksums.concat().as_bytes())
}

/// `CompleteSnapshot` with `ChangedBlocksCount`, and — when the row declares one — the
/// aggregate `Checksum` beside the algorithm and the aggregation method that name how it
/// was computed.
async fn complete_with_aggregate(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    changed_blocks_count: i32,
    aggregate: Option<&str>,
) -> Reply {
    let mut headers = vec![(
        "x-amz-ChangedBlocksCount",
        changed_blocks_count.to_string(),
    )];
    if let Some(value) = aggregate {
        headers.push(("x-amz-Checksum", value.to_string()));
        headers.push(("x-amz-Checksum-Algorithm", "SHA256".to_string()));
        headers.push(("x-amz-Checksum-Aggregation-Method", "LINEAR".to_string()));
    }
    send(
        router,
        "POST",
        &format!("/snapshots/completion/{snapshot_id}"),
        &headers,
        Vec::new(),
    )
    .await
}

/// One row: what the snapshot inherits from a parent, and the writes it then makes.
struct Row {
    case: &'static str,
    volume_size: i64,
    /// `(index, byte seed)` blocks written into a PARENT the snapshot is then started from,
    /// or empty for a snapshot with no parent.
    inherited: &'static [(i32, u8)],
    /// `(index, byte seed)` writes this snapshot makes, in the order it makes them. An
    /// index may repeat: "If the specified block contains data, the existing data is
    /// overwritten" (`PutSnapshotBlock`).
    writes: &'static [(i32, u8)],
}

impl Row {
    /// The per-block checksums of the blocks WRITTEN into this snapshot, ascending by block
    /// index: the strings the linear aggregation concatenates.
    ///
    /// One entry per index whatever the write order and however often the index was
    /// rewritten — the last write is the block the snapshot holds — and no entry for a
    /// block inherited from a parent, which was written to that parent.
    fn written_block_checksums(&self) -> Vec<String> {
        let mut by_index: BTreeMap<i32, u8> = BTreeMap::new();
        for (index, seed) in self.writes {
            by_index.insert(*index, *seed);
        }
        by_index
            .values()
            .map(|seed| checksum(&block_bytes(*seed)))
            .collect()
    }

    /// The aggregate a correct producer of this snapshot declares.
    fn aggregate(&self) -> String {
        linear_aggregate(&self.written_block_checksums())
    }

    /// Every block index this snapshot holds once its writes are in, ascending: what a
    /// listing of the sealed snapshot must report.
    fn held_indexes(&self) -> Vec<i32> {
        let mut indexes: Vec<i32> = self
            .inherited
            .iter()
            .chain(self.writes.iter())
            .map(|(index, _)| *index)
            .collect();
        indexes.sort_unstable();
        indexes.dedup();
        indexes
    }

    /// The count a completion of this snapshot declares to complete it: the number of
    /// DISTINCT indexes its own producer wrote.
    fn written_count(&self) -> i32 {
        let mut indexes: Vec<i32> = self.writes.iter().map(|(index, _)| *index).collect();
        indexes.sort_unstable();
        indexes.dedup();
        i32::try_from(indexes.len()).expect("a row writes few blocks")
    }
}

/// Rows differ along the number, placement, write order and rewriting of the blocks
/// written, and along whether the snapshot inherited any block. The last index of a 1 GiB
/// volume is `2047` (`BlockIndex` counts 524288-byte blocks, so one GiB addresses 2048 of
/// them).
const ROWS: &[Row] = &[
    Row {
        case: "a snapshot nothing was written into",
        volume_size: 1,
        inherited: &[],
        writes: &[],
    },
    Row {
        case: "one block",
        volume_size: 1,
        inherited: &[],
        writes: &[(0, 111)],
    },
    Row {
        case: "two adjacent blocks written in ascending index order",
        volume_size: 1,
        inherited: &[],
        writes: &[(0, 112), (1, 113)],
    },
    Row {
        case: "two adjacent blocks written in descending index order",
        volume_size: 1,
        inherited: &[],
        writes: &[(1, 114), (0, 115)],
    },
    Row {
        case: "three sparse blocks written out of index order",
        volume_size: 1,
        inherited: &[],
        writes: &[(100, 116), (2, 117), (37, 118)],
    },
    Row {
        case: "the last index the volume addresses, written before a lower one",
        volume_size: 1,
        inherited: &[],
        writes: &[(2047, 119), (3, 120)],
    },
    Row {
        case: "an index rewritten, aggregated once with the bytes it ended up holding",
        volume_size: 1,
        inherited: &[],
        writes: &[(4, 121), (4, 122), (9, 123)],
    },
    Row {
        case: "a snapshot that only inherited blocks",
        volume_size: 2,
        inherited: &[(0, 124), (1, 125)],
        writes: &[],
    },
    Row {
        case: "a snapshot that wrote beside the block it inherited",
        volume_size: 2,
        inherited: &[(0, 126)],
        writes: &[(5, 127)],
    },
    Row {
        case: "a snapshot that overwrote the block it inherited",
        volume_size: 2,
        inherited: &[(0, 128)],
        writes: &[(0, 129)],
    },
];

/// Start the snapshot of `row` — from a sealed parent holding its inherited blocks, where it
/// has any — write its writes, and answer its id. Nothing is completed here: the completion
/// under test is the caller's.
async fn pending_snapshot(router: &MultiServiceRouter, row: &Row) -> String {
    let mut body = serde_json::json!({ "VolumeSize": row.volume_size });
    if !row.inherited.is_empty() {
        let parent = write_snapshot(router, row.case, row.volume_size, row.inherited).await;
        body["ParentSnapshotId"] = serde_json::json!(parent);
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
        let digest = checksum(&data);
        let written = put_block(router, &snapshot_id, *block_index, data, BLOCK_SIZE, &digest).await;
        assert_eq!(
            written.status, 201,
            "case {:?}: PutSnapshotBlock at index {block_index} must answer 201, got {} with \
             body {}",
            row.case,
            written.status,
            String::from_utf8_lossy(&written.body)
        );
    }
    snapshot_id
}

/// RULE, served half: a completion whose declared aggregate IS the linear aggregation of
/// the per-block checksums of the blocks written into the snapshot is accepted — the
/// operation's 202 reporting the status the snapshot was in — and seals the snapshot, which
/// every later call reads back as `completed` and the read path then resolves.
#[tokio::test]
async fn a_completion_declaring_the_aggregate_of_its_written_blocks_seals_the_snapshot() {
    for row in ROWS {
        let router = router();
        let snapshot_id = pending_snapshot(&router, row).await;

        let aggregate = row.aggregate();
        let sealed = complete_with_aggregate(
            &router,
            &snapshot_id,
            row.written_count(),
            Some(&aggregate),
        )
        .await;
        assert_eq!(
            sealed.status, 202,
            "case {:?}: a completion declaring the aggregate {aggregate} of the blocks written \
             must answer 202, got {} with body {}",
            row.case,
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );
        assert_eq!(
            member(&sealed, "Status"),
            "pending",
            "case {:?}: the 202 must report the status the snapshot was in when the completion \
             was accepted, got {}",
            row.case,
            sealed.json()
        );

        // The seal the completion applied is what every later call reads off the record.
        let repeated = complete_snapshot(&router, &snapshot_id, row.written_count()).await;
        assert_eq!(
            member(&repeated, "Status"),
            "completed",
            "case {:?}: the snapshot the accepted completion sealed must read back completed, \
             got {}",
            row.case,
            repeated.json()
        );

        // And the sealed snapshot holds exactly the blocks the aggregate was taken over,
        // plus the ones it inherited.
        let listing = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listing.status, 200,
            "case {:?}: the sealed snapshot must be listable, got {} with body {}",
            row.case,
            listing.status,
            String::from_utf8_lossy(&listing.body)
        );
        let listed: Vec<i32> = listed_blocks(&listing)
            .iter()
            .map(|(index, _)| *index)
            .collect();
        assert_eq!(
            listed,
            row.held_indexes(),
            "case {:?}: the sealed snapshot must hold the blocks written into it, got {listed:?}",
            row.case
        );
    }
}

/// How a refused row misses the aggregate, computed from the per-block checksums of the
/// blocks the snapshot really holds.
struct Miss {
    case: &'static str,
    declared: fn(&[String]) -> String,
}

/// RULE, refused half: a well-formed aggregate that is not the one the snapshot's content
/// produces is refused with the modelled `ValidationException` naming both checksums, and
/// changes nothing — the snapshot is still pending afterwards, so the completion the
/// control at the end makes is the first one it accepts.
///
/// Rows differ along HOW the declared value misses, which is what pins the composition the
/// aggregation is: order (descending instead of ascending), form of the parts (raw digests
/// instead of their Base64 text), the set of blocks aggregated (a subset, a superset, none),
/// whether the aggregation hashes at all (one block's own checksum), and a value that is the
/// checksum of nothing.
#[tokio::test]
async fn a_completion_declaring_any_other_aggregate_is_refused_and_seals_nothing() {
    let row = Row {
        case: "three written blocks",
        volume_size: 1,
        inherited: &[],
        writes: &[(0, 131), (1, 132), (7, 133)],
    };
    let router = router();
    let snapshot_id = pending_snapshot(&router, &row).await;
    let block_checksums = row.written_block_checksums();

    let misses: &[Miss] = &[
        Miss {
            case: "the aggregate of no block at all",
            declared: |_| linear_aggregate(&[]),
        },
        Miss {
            case: "the same checksums aggregated in descending index order",
            declared: |held| {
                let mut reversed = held.to_vec();
                reversed.reverse();
                linear_aggregate(&reversed)
            },
        },
        Miss {
            case: "the raw digests aggregated instead of their Base64 text",
            declared: |held| {
                let raw: Vec<u8> = held
                    .iter()
                    .flat_map(|value| {
                        aws_smithy_types::base64::decode(value).expect("a Base64 checksum")
                    })
                    .collect();
                checksum(&raw)
            },
        },
        Miss {
            case: "the aggregate of a subset of the blocks written",
            declared: |held| linear_aggregate(&held[..held.len() - 1]),
        },
        Miss {
            case: "the aggregate of the blocks written plus one that never was",
            declared: |held| {
                let mut superset = held.to_vec();
                superset.push(checksum(&block_bytes(134)));
                linear_aggregate(&superset)
            },
        },
        Miss {
            case: "one block's own per-block checksum, never aggregated",
            declared: |held| held[0].clone(),
        },
        Miss {
            // 32 bytes of Base64, inside the modelled `^[A-Za-z0-9+/=]+$` and `@length`
            // 0..64, so the value is accepted as a checksum and only then compared.
            case: "a well-formed value that is the checksum of nothing",
            declared: |_| aws_smithy_types::base64::encode([9_u8; 32]),
        },
    ];

    let computed = row.aggregate();
    for Miss { case, declared } in misses {
        let declared = (declared)(&block_checksums);
        assert_ne!(
            declared, computed,
            "case {case:?}: the row must declare a value other than the aggregate the content \
             produces"
        );
        let reply = complete_with_aggregate(
            &router,
            &snapshot_id,
            row.written_count(),
            Some(&declared),
        )
        .await;
        assert_validation(
            case,
            &reply,
            REFUSAL_REASON,
            &mismatch_message(&computed, &declared),
        );
    }

    // The control that makes every row above mean something: the SAME request with the
    // aggregate the snapshot's content really produces is served, and it reports `pending` —
    // so not one of the refusals above sealed the snapshot on its way out.
    let accepted = complete_with_aggregate(
        &router,
        &snapshot_id,
        row.written_count(),
        Some(&computed),
    )
    .await;
    assert_eq!(
        accepted.status, 202,
        "case \"the aggregate the content produces\": CompleteSnapshot must answer 202, got {} \
         with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    assert_eq!(
        member(&accepted, "Status"),
        "pending",
        "case \"the aggregate the content produces\": the 202 must report the status the \
         snapshot was in when the completion was accepted, so every refusal above left it \
         pending, got {}",
        accepted.json()
    );
}

/// RULE, the blocks the aggregate is over: "each written block" is the blocks this
/// snapshot's own producer wrote, so a snapshot that inherited blocks from a
/// `ParentSnapshotId` aggregates only its own writes — and a producer that aggregates the
/// inherited blocks too is refused, with the aggregate of its own writes named as the
/// computed value.
#[tokio::test]
async fn the_aggregate_is_over_the_blocks_written_not_the_blocks_inherited() {
    let row = Row {
        case: "a write beside two inherited blocks",
        volume_size: 2,
        inherited: &[(0, 141), (1, 142)],
        writes: &[(6, 143)],
    };
    let router = router();
    let snapshot_id = pending_snapshot(&router, &row).await;

    // What the producer wrote, and what the snapshot HOLDS: the inherited blocks are part of
    // the second and no part of the first.
    let computed = row.aggregate();
    let over_everything = linear_aggregate(&[
        checksum(&block_bytes(141)),
        checksum(&block_bytes(142)),
        checksum(&block_bytes(143)),
    ]);
    let refused =
        complete_with_aggregate(&router, &snapshot_id, 1, Some(&over_everything)).await;
    assert_validation(
        "the aggregate of the inherited blocks and the written one",
        &refused,
        REFUSAL_REASON,
        &mismatch_message(&computed, &over_everything),
    );

    let accepted = complete_with_aggregate(&router, &snapshot_id, 1, Some(&computed)).await;
    assert_eq!(
        accepted.status, 202,
        "case \"the aggregate of the one block written\": CompleteSnapshot must answer 202, got \
         {} with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    let repeated = complete_snapshot(&router, &snapshot_id, 1).await;
    assert_eq!(
        member(&repeated, "Status"),
        "completed",
        "case \"the aggregate of the one block written\": the sealed snapshot must read back \
         completed, got {}",
        repeated.json()
    );
}

/// RULE, the member's optionality: `Checksum` is not `@required`
/// (`smithy_query(query="operation",operation="CompleteSnapshot")`), so a completion that
/// declares no aggregate has nothing to disagree with and is served on the evidence the
/// service does have — and the aggregate decides the REQUEST while `ChangedBlocksCount`
/// decides the STATUS, so a correct aggregate beside a count the content disagrees with is
/// still accepted and still finalises the snapshot to `error`.
#[tokio::test]
async fn an_undeclared_aggregate_refuses_nothing_and_a_declared_one_decides_only_the_request() {
    let row = Row {
        case: "two written blocks",
        volume_size: 1,
        inherited: &[],
        writes: &[(0, 151), (1, 152)],
    };

    // No aggregate declared: the completion is served and seals the snapshot.
    let router = router();
    let undeclared_id = pending_snapshot(&router, &row).await;
    let undeclared = complete_with_aggregate(&router, &undeclared_id, 2, None).await;
    assert_eq!(
        undeclared.status, 202,
        "case \"no aggregate declared\": CompleteSnapshot must answer 202, got {} with body {}",
        undeclared.status,
        String::from_utf8_lossy(&undeclared.body)
    );
    let sealed = complete_snapshot(&router, &undeclared_id, 2).await;
    assert_eq!(
        member(&sealed, "Status"),
        "completed",
        "case \"no aggregate declared\": the snapshot must read back completed, got {}",
        sealed.json()
    );

    // The right aggregate beside a count the content disagrees with: accepted, and finalised
    // to the status that count decides.
    let mismatched_count_id = pending_snapshot(&router, &row).await;
    let accepted =
        complete_with_aggregate(&router, &mismatched_count_id, 1, Some(&row.aggregate())).await;
    assert_eq!(
        accepted.status, 202,
        "case \"the right aggregate beside a count one short\": CompleteSnapshot must answer \
         202, got {} with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
    let finalised = complete_snapshot(&router, &mismatched_count_id, 1).await;
    assert_eq!(
        member(&finalised, "Status"),
        "error",
        "case \"the right aggregate beside a count one short\": the count must still decide the \
         status the record finalises to, got {}",
        finalised.json()
    );
}
