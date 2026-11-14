// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's SPLIT LINEAGE, end to end over the wire: `CreateStream` ->
//! `SplitShard`, once per generation -> `ListShards` / `DescribeStreamSummary`.
//!
//! RULE: an accepted `SplitShard` replaces exactly the one open shard it names with two open
//! children covering `[StartingHashKey, NewStartingHashKey - 1]` and `[NewStartingHashKey,
//! EndingHashKey]`, each naming that shard as the one it was produced from, and closes it
//! while leaving every other stored shard exactly as it was — so however many generations of
//! splits a stream has been through, its OPEN shards still tile the whole 128-bit hash key
//! space exactly once, as one contiguous run with no gap and no overlap, and every closed
//! shard keeps the range it was laid down with. The stream a split re-tiles is the one its
//! `StreamName`, its `StreamARN`, or both together name, inside the region the request was
//! signed for; a request naming neither, or naming two different streams with them, is
//! refused and re-tiles nothing, as is one naming a shard an earlier generation already
//! closed — and a refusal about a stream the request RESOLVED names that stream by its
//! `StreamName`, whichever identifier addressed it, which is the rule every refusal of this
//! service that names a resolved stream follows (`MergeShards`, the two retention-period
//! operations and `GetShardIterator` word it the same way).
//!
//! `manual_resharding` asserts ONE split of a freshly created stream; these rows take the
//! rule the generation deeper it generalises over: the depth of the lineage (one, two and
//! three splits of the same stream), which shard of the hash-ordered map the next split names
//! (the lowest, a middle one, the highest), where in the parent's range the key falls (both
//! keys the rule publishes as its bounds, the midpoint and both quarters), how many shards
//! the stream started with, and which identifier the request addresses the stream by.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    retention_period_not_longer_message, retention_period_not_shorter_message,
    shard_not_in_stream_message, stream_not_found_message, DEFAULT_RETENTION_PERIOD_HOURS,
    STREAM_ACTIVE, stream_identifier_mismatch_message, STREAM_IDENTIFIER_REQUIRED_MESSAGE,
    STREAM_MODE_PROVISIONED, STREAM_UPDATING,
};

use crate::shard_scaling::{
    hash_key_range, is_open, parents, past_reshard, shard_id, shard_map, summary,
};
use crate::stream_tags::{
    error_code, error_message, past_activation, poll_until_active, stream_arn, Emulator, REGION,
};

/// The lowest hash key a stream's shard map covers.
const FIRST_HASH_KEY: u128 = 0;

/// The first shard a create lays down, which every row here splits to get a closed shard: a
/// shard id is minted from the shard's position in the stream's map.
const CLOSED_SHARD_ID: &str = "shardId-000000000000";

/// A shard id no stream of these cases holds, well formed under `ShardId`'s modelled pattern
/// `^[a-zA-Z0-9_.-]+$`.
const ABSENT_SHARD_ID: &str = "shardId-000000000099";

/// The highest hash key a stream's shard map covers: the partition key hash is a "128-bit
/// integer value" ("an MD5 hash function is used to map partition keys to 128-bit integer
/// values", `smithy_query(query="doc",shape="PutRecordInput",member="PartitionKey")`), so the
/// space the open shards tile is `0 ..= 2^128 - 1`.
const LAST_HASH_KEY: u128 = u128::MAX;

/// Where inside the parent's range a step puts its `NewStartingHashKey`. The keys the rule
/// admits are the ones its refusal publishes as the bounds: strictly greater than one plus
/// the parent's `StartingHashKey`, and strictly less than its `EndingHashKey`.
#[derive(Debug, Clone, Copy)]
enum SplitAt {
    /// The midpoint of the parent's range, which halves it.
    Midpoint,
    /// A quarter of the way in, so the lower child is the smaller one.
    LowerQuarter,
    /// Three quarters of the way in, so the upper child is the smaller one.
    UpperQuarter,
    /// The lowest key the rule admits: two plus the parent's `StartingHashKey`.
    LowestAdmitted,
    /// The highest key the rule admits: one less than the parent's `EndingHashKey`.
    HighestAdmitted,
}

impl SplitAt {
    /// The key this step splits `[starting, ending]` at.
    fn key(self, starting: u128, ending: u128) -> u128 {
        let width = ending - starting;
        match self {
            Self::Midpoint => starting + width / 2,
            Self::LowerQuarter => starting + width / 4,
            Self::UpperQuarter => starting + width / 4 * 3,
            Self::LowestAdmitted => starting + 2,
            Self::HighestAdmitted => ending - 1,
        }
    }
}

/// One generation of a lineage: which shard of the hash-ordered open map it splits, and where.
type Step = (usize, SplitAt);

/// Create a provisioned stream of `shard_count` shards.
async fn create_stream(emulator: &Emulator, case: &str, name: &str, shard_count: u32) {
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({
                "StreamName": name,
                "ShardCount": shard_count,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
            }),
        )
        .await;
}

/// The stream's open shards in hash key order, which is the order the tiling is read in —
/// `ListShards` reports the map in its own order, and a split appends its children to it.
fn open_in_hash_order(case: &str, map: &[Value]) -> Vec<Value> {
    let mut open: Vec<Value> = map
        .iter()
        .filter(|shard| is_open(case, shard))
        .cloned()
        .collect();
    open.sort_by_key(|shard| hash_key_range(case, shard).0);
    open
}

/// Every stored shard that is no longer open, as `(id, range)` pairs in map order.
fn closed_ranges(case: &str, map: &[Value]) -> Vec<(String, (u128, u128))> {
    map.iter()
        .filter(|shard| !is_open(case, shard))
        .map(|shard| (shard_id(case, shard), hash_key_range(case, shard)))
        .collect()
}

/// Assert that the stream's open shards tile the whole hash key space exactly once: one
/// contiguous run from the first key to the last, with no gap and no overlap between
/// neighbours.
fn assert_tiles_the_key_space(case: &str, open: &[Value]) {
    let ranges: Vec<(u128, u128)> = open
        .iter()
        .map(|shard| hash_key_range(case, shard))
        .collect();
    let first = ranges
        .first()
        .unwrap_or_else(|| panic!("case {case}: the stream must hold an open shard"));
    let last = ranges
        .last()
        .unwrap_or_else(|| panic!("case {case}: the stream must hold an open shard"));
    assert_eq!(
        first.0, FIRST_HASH_KEY,
        "case {case}: expected the lowest open shard to start at {FIRST_HASH_KEY}, got \
         {ranges:?}"
    );
    assert_eq!(
        last.1, LAST_HASH_KEY,
        "case {case}: expected the highest open shard to end at {LAST_HASH_KEY}, got {ranges:?}"
    );
    for pair in ranges.windows(2) {
        let (below, above) = (pair[0], pair[1]);
        assert!(
            below.1 < above.0 && above.0 - below.1 == 1,
            "case {case}: expected the open shards to meet with no gap and no overlap, got \
             {below:?} then {above:?} in {ranges:?}"
        );
        assert!(
            below.0 <= below.1 && above.0 <= above.1,
            "case {case}: expected every open shard to cover at least one key, got {below:?} \
             then {above:?} in {ranges:?}"
        );
    }
}

/// Drive ONE generation of `case`'s lineage and assert what the rule says the call did.
async fn split_once(emulator: &Emulator, case: &str, name: &str, step: Step) {
    let (position, at) = step;
    let before = shard_map(emulator, case, name).await;
    let open_before = open_in_hash_order(case, &before);
    let closed_before = closed_ranges(case, &before);
    let parent = open_before.get(position).unwrap_or_else(|| {
        panic!(
            "case {case}: the map must hold an open shard at {position}, got {} open",
            open_before.len()
        )
    });
    let parent_id = shard_id(case, parent);
    let (starting, ending) = hash_key_range(case, parent);
    let key = at.key(starting, ending);

    emulator
        .ok(
            case,
            "SplitShard",
            &json!({
                "StreamName": name,
                "ShardToSplit": parent_id,
                "NewStartingHashKey": key.to_string(),
            }),
        )
        .await;

    // "Upon receiving a SplitShard request, Kinesis Data Streams immediately returns a
    // response and sets the stream status to UPDATING."
    let (status, open_count) = summary(emulator, case, name).await;
    assert_eq!(
        status, STREAM_UPDATING,
        "case {case}: expected the stream to be {STREAM_UPDATING} straight after the split at \
         {key}, got {status}"
    );
    assert_eq!(
        usize::try_from(open_count).unwrap_or(usize::MAX),
        open_before.len() + 1,
        "case {case}: expected the split at {key} to leave one open shard more than the {} \
         there were, got {open_count}",
        open_before.len()
    );

    let after = shard_map(emulator, case, name).await;

    // The two children are the shard the call named, re-tiled: the lower one every key below
    // `NewStartingHashKey`, the upper one that key and every key above it.
    let children: Vec<&Value> = after
        .iter()
        .filter(|shard| is_open(case, shard) && parents(shard) == vec![parent_id.clone()])
        .collect();
    assert_eq!(
        children.len(),
        2,
        "case {case}: expected the split at {key} to lay down two children of {parent_id}, \
         got {children:?}"
    );
    assert_eq!(
        hash_key_range(case, children[0]),
        (starting, key - 1),
        "case {case}: expected the lower child of {parent_id} to cover {starting}..={}, got \
         {:?}",
        key - 1,
        hash_key_range(case, children[0])
    );
    assert_eq!(
        hash_key_range(case, children[1]),
        (key, ending),
        "case {case}: expected the upper child of {parent_id} to cover {key}..={ending}, got \
         {:?}",
        hash_key_range(case, children[1])
    );

    // Exactly the shard the call named was closed, and it kept the range it covered; every
    // shard an earlier generation closed is still closed, still covering its own range.
    let mut expected_closed = closed_before.clone();
    expected_closed.push((parent_id.clone(), (starting, ending)));
    let closed_after = closed_ranges(case, &after);
    assert_eq!(
        closed_after, expected_closed,
        "case {case}: expected the split at {key} to close exactly {parent_id} and leave \
         every earlier closed shard as it was, got {closed_after:?}"
    );

    // Every shard the call did not name is untouched, open and covering the same range.
    for shard in &open_before {
        let held = shard_id(case, shard);
        if held == parent_id {
            continue;
        }
        let still = after
            .iter()
            .find(|candidate| shard_id(case, candidate) == held)
            .unwrap_or_else(|| panic!("case {case}: {held} must still be in the map"));
        assert!(
            is_open(case, still) && hash_key_range(case, still) == hash_key_range(case, shard),
            "case {case}: expected {held} to be left as it was, got {still}"
        );
    }

    // Whatever the lineage's shape, the open shards still tile the whole key space, and each
    // child's parent is a shard the map holds and holds closed.
    let open_after = open_in_hash_order(case, &after);
    assert_tiles_the_key_space(case, &open_after);
    for shard in &open_after {
        for named in parents(shard) {
            let ancestor = after
                .iter()
                .find(|candidate| shard_id(case, candidate) == named)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: {} names parent {named}, which the map must hold",
                        shard_id(case, shard)
                    )
                });
            assert!(
                !is_open(case, ancestor),
                "case {case}: expected the parent {named} to be closed, got {ancestor}"
            );
        }
    }
}

/// However deep the lineage of splits, the open shards tile the whole hash key space exactly
/// once and every closed generation keeps the range it covered.
#[tokio::test]
async fn repeated_splits_keep_the_open_shards_tiling_the_key_space() {
    // (the case, how many shards the stream is created with, one step per generation)
    let cases: Vec<(&str, u32, Vec<Step>)> = vec![
        (
            "halve-one-shard-three-generations-deep",
            1,
            vec![
                (0, SplitAt::Midpoint),
                (0, SplitAt::Midpoint),
                (2, SplitAt::Midpoint),
            ],
        ),
        (
            "skew-a-child-of-a-child",
            1,
            vec![
                (0, SplitAt::LowerQuarter),
                (1, SplitAt::UpperQuarter),
                (1, SplitAt::Midpoint),
            ],
        ),
        (
            "split-at-both-published-bounds",
            1,
            vec![
                (0, SplitAt::LowestAdmitted),
                (1, SplitAt::HighestAdmitted),
            ],
        ),
        (
            "split-the-lowest-then-the-highest-of-four",
            4,
            vec![(0, SplitAt::Midpoint), (4, SplitAt::LowerQuarter)],
        ),
        (
            "split-a-middle-shard-of-three",
            3,
            vec![(1, SplitAt::UpperQuarter)],
        ),
        (
            "split-the-highest-shard-of-two",
            2,
            vec![(1, SplitAt::HighestAdmitted)],
        ),
    ];

    let emulator = Emulator::new();
    // Every stream is created and waited out ONCE: the rule is about the calls, not about how
    // long a create takes.
    for (case, shard_count, _) in &cases {
        create_stream(&emulator, case, case, *shard_count).await;
    }
    tokio::time::sleep(past_activation()).await;
    for (case, shard_count, _) in &cases {
        let open = open_in_hash_order(case, &shard_map(&emulator, case, case).await);
        assert_eq!(
            open.len(),
            usize::try_from(*shard_count).unwrap_or(usize::MAX),
            "case {case}: expected the create to lay down {shard_count} shards, got {}",
            open.len()
        );
        assert_tiles_the_key_space(case, &open);
    }

    // A split is served only for an `ACTIVE` stream, so the generations are driven one round
    // at a time: every case takes its next step, then the transition they armed is waited
    // out once for all of them.
    let generations = cases
        .iter()
        .map(|(_, _, steps)| steps.len())
        .max()
        .unwrap_or_default();
    for generation in 0..generations {
        for (case, _, steps) in &cases {
            if let Some(step) = steps.get(generation) {
                split_once(&emulator, case, case, *step).await;
            }
        }
        tokio::time::sleep(past_reshard()).await;
        for (case, _, steps) in &cases {
            if steps.get(generation).is_none() {
                continue;
            }
            let (status, _) = summary(&emulator, case, case).await;
            assert_eq!(
                status, STREAM_ACTIVE,
                "case {case}: expected the stream to be {STREAM_ACTIVE} once generation \
                 {generation} fell due, got {status}"
            );
        }
    }

    // "After the operation is completed, Kinesis Data Streams sets the stream status to
    // ACTIVE", with one open shard more per generation than the create laid down.
    for (case, shard_count, steps) in &cases {
        let (status, open_count) = summary(&emulator, case, case).await;
        let expected = u64::from(*shard_count)
            + u64::try_from(steps.len()).unwrap_or_else(|error| {
                panic!("case {case}: the step count must fit a count: {error}")
            });
        assert_eq!(
            (status.as_str(), open_count),
            (STREAM_ACTIVE, expected),
            "case {case}: expected {STREAM_ACTIVE} with {expected} open shards once every \
             generation fell due, got {status} with {open_count}"
        );
        let map = shard_map(&emulator, case, case).await;
        assert_eq!(
            closed_ranges(case, &map).len(),
            steps.len(),
            "case {case}: expected one closed shard per generation, got {map:?}"
        );
        assert_tiles_the_key_space(case, &open_in_hash_order(case, &map));
    }
}

/// The message AWS answers `InvalidArgumentException` with when a reshard names a shard an
/// earlier split or merge has already closed, with the shard the request named, the stream it
/// addressed and the account that holds it filled in. Written out here so the wire wording is
/// pinned by the test and not only by the handler that builds it.
fn expected_closed_shard_refusal(shard: &str, stream: &str) -> String {
    format!(
        "Shard {shard} in stream {stream} under account {} has already been merged or split, \
         and thus is not eligible for merging or splitting.",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A shard an earlier generation closed is no longer eligible: it gave its slice of the hash
/// key space up to the children that split minted, so naming it is refused with the service's
/// wording for an already-merged-or-split shard, whichever generation closed it and whichever
/// identifier the request addresses the stream by.
#[tokio::test]
async fn splitting_a_shard_an_earlier_generation_closed_is_refused() {
    let emulator = Emulator::new();
    let case = "split-an-already-split-shard";
    create_stream(&emulator, case, case, 2).await;
    tokio::time::sleep(past_activation()).await;

    // Two generations, so the map holds a shard the create laid down and closed, and a child
    // of it that the next split closed in turn.
    let mut closed_by_generation: Vec<String> = Vec::new();
    for step in [(0, SplitAt::Midpoint), (1, SplitAt::LowerQuarter)] {
        let open = open_in_hash_order(case, &shard_map(&emulator, case, case).await);
        let parent = open
            .get(step.0)
            .unwrap_or_else(|| panic!("case {case}: the map must hold an open shard at {}", step.0));
        closed_by_generation.push(shard_id(case, parent));
        split_once(&emulator, case, case, step).await;
        tokio::time::sleep(past_reshard()).await;
    }

    // (the closed shard the request names, how the request addresses the stream)
    let rows: Vec<(&String, AddressedBy)> = vec![
        (&closed_by_generation[0], AddressedBy::Name),
        (&closed_by_generation[0], AddressedBy::Arn),
        (&closed_by_generation[1], AddressedBy::Both),
    ];

    for (closed, addressed_by) in rows {
        let label = format!("{case}/{closed}/{addressed_by:?}");
        let before = shard_map(&emulator, &label, case).await;
        let mut body = json!({
            "ShardToSplit": closed,
            "NewStartingHashKey": "1024",
        });
        match addressed_by {
            AddressedBy::Name => body["StreamName"] = json!(case),
            AddressedBy::Arn => body["StreamARN"] = json!(stream_arn(REGION, case)),
            AddressedBy::Both => {
                body["StreamName"] = json!(case);
                body["StreamARN"] = json!(stream_arn(REGION, case));
            }
        }

        let (status, response) = emulator.call("SplitShard", &body).await;
        assert_eq!(
            status, 400,
            "case {label}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {label}: expected the modelled closed-shard refusal, got {response}"
        );
        let says = expected_closed_shard_refusal(closed, case);
        let message = error_message(&response);
        assert_eq!(
            message, says,
            "case {label}: expected the refusal to say {says:?}, got {message:?}"
        );

        let after = shard_map(&emulator, &label, case).await;
        assert_eq!(
            closed_ranges(&label, &after),
            closed_ranges(&label, &before),
            "case {label}: a refused split must leave the shard map as it was"
        );
        assert_eq!(
            open_in_hash_order(&label, &after),
            open_in_hash_order(&label, &before),
            "case {label}: a refused split must leave the open shards as they were"
        );
    }
}

/// One refusal about a stream a request addressed by its `StreamARN` alone: the operation, the
/// request it makes and the message it must answer, built from the stream's own NAME.
struct ArnAddressed {
    case: &'static str,
    operation: &'static str,
    /// The request body, built from the stream's ARN, a shard an earlier split closed and a
    /// shard the stream still holds open.
    body: fn(&str, &str, &str) -> Value,
    code: &'static str,
    /// The message the refusal must carry, built from the stream's stored NAME.
    says: fn(&str) -> String,
}

/// EVERY refusal about a stream the request resolved names it by its `StreamName` — the
/// variable part the service's messages carry — even when the request addressed the stream by
/// its `StreamARN` alone and never named it.
#[tokio::test]
async fn a_refusal_names_the_stream_the_request_resolved_by_name() {
    let cases: Vec<ArnAddressed> = vec![
        ArnAddressed {
            case: "arn-addressed-split-of-a-closed-shard",
            operation: "SplitShard",
            body: |arn, closed, _| {
                json!({
                    "StreamARN": arn,
                    "ShardToSplit": closed,
                    "NewStartingHashKey": "1024",
                })
            },
            says: |name| expected_closed_shard_refusal(CLOSED_SHARD_ID, name),
            code: "InvalidArgumentException",
        },
        ArnAddressed {
            case: "arn-addressed-merge-of-a-closed-shard",
            operation: "MergeShards",
            body: |arn, closed, open| {
                json!({
                    "StreamARN": arn,
                    "ShardToMerge": closed,
                    "AdjacentShardToMerge": open,
                })
            },
            says: |name| expected_closed_shard_refusal(CLOSED_SHARD_ID, name),
            code: "InvalidArgumentException",
        },
        ArnAddressed {
            case: "arn-addressed-increase-that-lengthens-nothing",
            operation: "IncreaseStreamRetentionPeriod",
            body: |arn, _, _| {
                json!({
                    "StreamARN": arn,
                    "RetentionPeriodHours": DEFAULT_RETENTION_PERIOD_HOURS,
                })
            },
            says: |name| {
                retention_period_not_longer_message(
                    name,
                    DEFAULT_RETENTION_PERIOD_HOURS,
                    DEFAULT_RETENTION_PERIOD_HOURS,
                )
            },
            code: "InvalidArgumentException",
        },
        ArnAddressed {
            case: "arn-addressed-decrease-that-shortens-nothing",
            operation: "DecreaseStreamRetentionPeriod",
            body: |arn, _, _| {
                json!({
                    "StreamARN": arn,
                    "RetentionPeriodHours": DEFAULT_RETENTION_PERIOD_HOURS,
                })
            },
            says: |name| {
                retention_period_not_shorter_message(
                    name,
                    DEFAULT_RETENTION_PERIOD_HOURS,
                    DEFAULT_RETENTION_PERIOD_HOURS,
                )
            },
            code: "InvalidArgumentException",
        },
        ArnAddressed {
            case: "arn-addressed-iterator-of-a-shard-nothing-holds",
            operation: "GetShardIterator",
            body: |arn, _, _| {
                json!({
                    "StreamARN": arn,
                    "ShardId": ABSENT_SHARD_ID,
                    "ShardIteratorType": "TRIM_HORIZON",
                })
            },
            says: |name| shard_not_in_stream_message(ABSENT_SHARD_ID, name),
            code: "ResourceNotFoundException",
        },
    ];

    let emulator = Emulator::new();
    for refusal in &cases {
        create_stream(&emulator, refusal.case, refusal.case, 2).await;
    }
    // The CONDITION each stream reaches on the emulator's own clock, looked for again until
    // it holds — never a duration this case measures.
    for refusal in &cases {
        poll_until_active(&emulator, refusal.case, refusal.case).await;
    }

    // Every row's stream gets a closed shard, so the reshard rows have one to name; the
    // retention rows are indifferent to the shard map.
    for refusal in &cases {
        split_once(&emulator, refusal.case, refusal.case, (0, SplitAt::Midpoint)).await;
    }
    for refusal in &cases {
        poll_until_active(&emulator, refusal.case, refusal.case).await;
    }

    for refusal in &cases {
        let case = refusal.case;
        let map = shard_map(&emulator, case, case).await;
        let closed = closed_ranges(case, &map);
        assert_eq!(
            closed.len(),
            1,
            "case {case}: the split must have closed exactly one shard, got {closed:?}"
        );
        assert_eq!(
            closed[0].0, CLOSED_SHARD_ID,
            "case {case}: expected the create's first shard to be the closed one, got {closed:?}"
        );
        let open = open_in_hash_order(case, &map);
        let still_open = shard_id(
            case,
            open.last()
                .unwrap_or_else(|| panic!("case {case}: the stream must hold an open shard")),
        );

        let arn = stream_arn(REGION, case);
        let (status, response) = emulator
            .call(
                refusal.operation,
                &(refusal.body)(&arn, CLOSED_SHARD_ID, &still_open),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            refusal.code,
            "case {case}: expected {}, got {response}",
            refusal.code
        );
        let says = (refusal.says)(case);
        let message = error_message(&response);
        assert_eq!(
            message, says,
            "case {case}: expected the refusal to name the stream by name, saying {says:?}, \
             got {message:?}"
        );
        assert!(
            !message.contains(&arn),
            "case {case}: a refusal must not name the stream by its ARN, got {message:?}"
        );
    }
}

/// Which identifiers a row addresses the stream by.
#[derive(Debug, Clone, Copy)]
enum AddressedBy {
    /// `StreamName` alone.
    Name,
    /// `StreamARN` alone.
    Arn,
    /// Both, naming the same stream.
    Both,
}

/// A split resolves its stream by `StreamName`, by `StreamARN` or by both — the same rule
/// every operation of this service that addresses a stream follows.
#[tokio::test]
async fn a_split_resolves_its_stream_by_either_identifier() {
    let emulator = Emulator::new();
    let cases: Vec<(&str, AddressedBy)> = vec![
        ("address-by-name", AddressedBy::Name),
        ("address-by-arn", AddressedBy::Arn),
        ("address-by-both", AddressedBy::Both),
    ];

    for (case, _) in &cases {
        create_stream(&emulator, case, case, 1).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (case, addressed_by) in &cases {
        let open = open_in_hash_order(case, &shard_map(&emulator, case, case).await);
        let parent = open
            .first()
            .unwrap_or_else(|| panic!("case {case}: the stream must hold an open shard"));
        let parent_id = shard_id(case, parent);
        let (starting, ending) = hash_key_range(case, parent);
        let key = SplitAt::Midpoint.key(starting, ending);

        let mut body = json!({
            "ShardToSplit": parent_id,
            "NewStartingHashKey": key.to_string(),
        });
        match addressed_by {
            AddressedBy::Name => body["StreamName"] = json!(case),
            AddressedBy::Arn => body["StreamARN"] = json!(stream_arn(REGION, case)),
            AddressedBy::Both => {
                body["StreamName"] = json!(case);
                body["StreamARN"] = json!(stream_arn(REGION, case));
            }
        }
        emulator.ok(case, "SplitShard", &body).await;

        let after = shard_map(&emulator, case, case).await;
        let closed = closed_ranges(case, &after);
        assert_eq!(
            closed,
            vec![(parent_id.clone(), (starting, ending))],
            "case {case}: expected the split to close exactly {parent_id}, got {closed:?}"
        );
        let open_after = open_in_hash_order(case, &after);
        let ranges: Vec<(u128, u128)> = open_after
            .iter()
            .map(|shard| hash_key_range(case, shard))
            .collect();
        assert_eq!(
            ranges,
            vec![(starting, key - 1), (key, ending)],
            "case {case}: expected the children to tile the parent's range, got {ranges:?}"
        );
        assert_tiles_the_key_space(case, &open_after);
    }
}

/// One refused split: the identifiers it names, and the modelled refusal it must answer.
struct Refusal {
    case: &'static str,
    /// `StreamName`, if the request carries one.
    name: fn(&str) -> Option<String>,
    /// `StreamARN`, if the request carries one.
    arn: fn(&str) -> Option<String>,
    code: &'static str,
    says: fn(&str) -> String,
}

/// A split whose identifiers name no one stream is refused and re-tiles nothing.
#[tokio::test]
async fn a_split_naming_no_one_stream_is_refused() {
    let cases: Vec<Refusal> = vec![
        Refusal {
            case: "address-by-neither",
            name: |_| None,
            arn: |_| None,
            code: "InvalidArgumentException",
            says: |_| STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        },
        Refusal {
            case: "address-two-different-streams",
            name: |case| Some(format!("{case}-other")),
            arn: |case| Some(stream_arn(REGION, case)),
            code: "InvalidArgumentException",
            says: |case| {
                stream_identifier_mismatch_message(
                    &format!("{case}-other"),
                    &stream_arn(REGION, case),
                )
            },
        },
        Refusal {
            case: "address-an-arn-nothing-holds",
            name: |_| None,
            arn: |case| Some(stream_arn(REGION, &format!("{case}-absent"))),
            code: "ResourceNotFoundException",
            says: |case| stream_not_found_message(&format!("{case}-absent")),
        },
    ];

    let emulator = Emulator::new();
    for refusal in &cases {
        create_stream(&emulator, refusal.case, refusal.case, 2).await;
    }
    tokio::time::sleep(past_activation()).await;

    for refusal in &cases {
        let case = refusal.case;
        let before = shard_map(&emulator, case, case).await;
        let open = open_in_hash_order(case, &before);
        let parent = open
            .first()
            .unwrap_or_else(|| panic!("case {case}: the stream must hold an open shard"));
        let (starting, ending) = hash_key_range(case, parent);

        let mut body = json!({
            "ShardToSplit": shard_id(case, parent),
            "NewStartingHashKey": SplitAt::Midpoint.key(starting, ending).to_string(),
        });
        if let Some(name) = (refusal.name)(case) {
            body["StreamName"] = json!(name);
        }
        if let Some(arn) = (refusal.arn)(case) {
            body["StreamARN"] = json!(arn);
        }

        let (status, response) = emulator.call("SplitShard", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a client error, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            refusal.code,
            "case {case}: expected {}, got {response}",
            refusal.code
        );
        let says = (refusal.says)(case);
        let message = error_message(&response);
        assert_eq!(
            message, says,
            "case {case}: expected the refusal to say {says:?}, got {message:?}"
        );

        let after = shard_map(&emulator, case, case).await;
        let was: Vec<(String, bool)> = before
            .iter()
            .map(|shard| (shard_id(case, shard), is_open(case, shard)))
            .collect();
        let now: Vec<(String, bool)> = after
            .iter()
            .map(|shard| (shard_id(case, shard), is_open(case, shard)))
            .collect();
        assert_eq!(
            now, was,
            "case {case}: a refused split must leave the shard map as it was"
        );
    }
}
