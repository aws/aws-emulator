// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's MANUAL RESHARDING, end to end over the wire: `CreateStream` ->
//! `SplitShard` / `MergeShards` -> `ListShards` / `DescribeStreamSummary`.
//!
//! RULE: a manual reshard re-tiles the slice of the hash key space the shards it names
//! cover, and nothing else. `SplitShard` closes the named open shard and lays down two open
//! children covering `[StartingHashKey, NewStartingHashKey - 1]` and `[NewStartingHashKey,
//! EndingHashKey]`; `MergeShards` closes the two named open shards and lays down one open
//! child covering their joined range; each child names the shard or shards it was produced
//! from, and the stream is left `UPDATING` until the transition back to `ACTIVE` falls due.
//! Either call is served only for a stream in the provisioned capacity mode
//! (`ValidationException` otherwise), only while that stream is `ACTIVE`
//! (`ResourceInUseException` otherwise), only for shards the stream holds
//! (`ResourceNotFoundException` otherwise) and holds OPEN, and only for parameters the rule
//! admits — a `NewStartingHashKey` strictly inside the parent's range, and two shards whose
//! ranges meet (`InvalidArgumentException` otherwise). Every refusal leaves the stored shard
//! map exactly as it was.
//!
//! The recorded flows this was derived from split one shard at an invalid hash key and merge
//! two shards of an on-demand stream; each is one row of the tables below. The rows differ
//! along what the rule generalises over: how many shards the stream holds, which shard of
//! the map the call names, where in the parent's range the split key falls (both published
//! bounds and inside), the order the two merged shards are named in, the capacity mode and
//! status of the stream, and whether the shards it names are open, closed or absent.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    shard_not_in_stream_message, shard_not_open_message,
    split_hash_key_outside_the_parent_message, stream_not_found_message,
    stream_on_demand_message, STREAM_ACTIVE, STREAM_IDENTIFIER_REQUIRED_MESSAGE,
    STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED, STREAM_UPDATING,
};

// The wording an ineligible pair of shards is refused with is written out in
// `merge_adjacency`, not read from the implementation, so these rows assert the text a
// client sees rather than whatever the handler happens to say.
use crate::merge_adjacency::not_an_adjacent_pair;
use crate::shard_scaling::{
    hash_key_range, is_open, parents, past_reshard, shard_id, shard_map, summary,
};
use crate::stream_tags::{error_code, error_message, past_activation, poll_until_active, Emulator};

/// The open shards of `name`, lowest hash key first, as `ListShards` reports them.
async fn open_shards(emulator: &Emulator, case: &str, name: &str) -> Vec<Value> {
    shard_map(emulator, case, name)
        .await
        .into_iter()
        .filter(|shard| is_open(case, shard))
        .collect()
}

/// Create a stream of `shard_count` shards in `mode`. The caller waits the transition the
/// create armed out ONCE for every stream it set up, so the reshard under test arrives at an
/// `ACTIVE` stream.
async fn create_stream(emulator: &Emulator, name: &str, shard_count: u32, mode: &str) {
    let mut body = json!({
        "StreamName": name,
        "StreamModeDetails": { "StreamMode": mode },
    });
    if mode == STREAM_MODE_PROVISIONED {
        body["ShardCount"] = json!(shard_count);
    }
    emulator.ok(name, "CreateStream", &body).await;
}

/// Where inside the parent shard's range a split row puts its `NewStartingHashKey`. The
/// admitted keys are the ones the refusal message names: strictly greater than one plus the
/// parent's `StartingHashKey`, and strictly less than its `EndingHashKey`.
#[derive(Debug, Clone, Copy)]
enum SplitAt {
    /// The midpoint of the parent's range, which halves it.
    Midpoint,
    /// A quarter of the way in, so the children differ in size.
    Quarter,
    /// The lowest key the rule admits: two plus the parent's `StartingHashKey`.
    LowestAdmitted,
    /// The highest key the rule admits: one less than the parent's `EndingHashKey`.
    HighestAdmitted,
}

impl SplitAt {
    /// The key this row splits `[starting, ending]` at.
    fn key(self, starting: u128, ending: u128) -> u128 {
        match self {
            Self::Midpoint => starting + (ending - starting) / 2,
            Self::Quarter => starting + (ending - starting) / 4,
            Self::LowestAdmitted => starting + 2,
            Self::HighestAdmitted => ending - 1,
        }
    }
}

/// An accepted split closes the parent and covers its range with two children, lowest keys
/// to one and the rest to the other.
#[tokio::test]
async fn split_shard_tiles_the_parents_range() {
    // (shard count of the stream, which shard of the map is split, where it is split)
    let cases: Vec<(u32, usize, SplitAt)> = vec![
        (1, 0, SplitAt::Midpoint),
        (1, 0, SplitAt::LowestAdmitted),
        (1, 0, SplitAt::HighestAdmitted),
        (2, 0, SplitAt::Quarter),
        (2, 1, SplitAt::Midpoint),
        (3, 1, SplitAt::HighestAdmitted),
        (4, 3, SplitAt::LowestAdmitted),
    ];

    let emulator = Emulator::new();
    // Every stream is created first and waited out ONCE: the rule is about the call, not
    // about how long a create takes.
    for (index, (shard_count, _, _)) in cases.iter().enumerate() {
        let case = format!("split-{index}-of-{shard_count}");
        create_stream(&emulator, &case, *shard_count, STREAM_MODE_PROVISIONED).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (index, (shard_count, position, at)) in cases.iter().enumerate() {
        let case = format!("split-{index}-of-{shard_count}");

        let before = open_shards(&emulator, &case, &case).await;
        let parent = before
            .get(*position)
            .unwrap_or_else(|| panic!("case {case}: the map must hold shard {position}"));
        let parent_id = shard_id(&case, parent);
        let (starting, ending) = hash_key_range(&case, parent);
        let key = at.key(starting, ending);

        emulator
            .ok(
                &case,
                "SplitShard",
                &json!({
                    "StreamName": case,
                    "ShardToSplit": parent_id,
                    "NewStartingHashKey": key.to_string(),
                }),
            )
            .await;

        // "Upon receiving a SplitShard request, Kinesis Data Streams immediately returns a
        // response and sets the stream status to UPDATING."
        let (status, _) = summary(&emulator, &case, &case).await;
        assert_eq!(
            status, STREAM_UPDATING,
            "case {case}: expected the stream to be {STREAM_UPDATING} straight after the \
             split, got {status}"
        );

        let after = shard_map(&emulator, &case, &case).await;
        let closed: Vec<String> = after
            .iter()
            .filter(|shard| !is_open(&case, shard))
            .map(|shard| shard_id(&case, shard))
            .collect();
        assert_eq!(
            closed,
            vec![parent_id.clone()],
            "case {case}: expected exactly the parent to be closed, got {closed:?}"
        );

        let children: Vec<&Value> = after
            .iter()
            .filter(|shard| is_open(&case, shard) && parents(shard) == vec![parent_id.clone()])
            .collect();
        assert_eq!(
            children.len(),
            2,
            "case {case}: expected two children of {parent_id}, got {children:?}"
        );
        assert_eq!(
            hash_key_range(&case, children[0]),
            (starting, key - 1),
            "case {case}: expected the lower child to cover {starting}..{}",
            key - 1
        );
        assert_eq!(
            hash_key_range(&case, children[1]),
            (key, ending),
            "case {case}: expected the upper child to cover {key}..{ending}"
        );

        // Every shard the call did not name is left exactly as it was, open and covering
        // the same range.
        for (position_of, shard) in before.iter().enumerate() {
            if position_of == *position {
                continue;
            }
            let untouched = shard_id(&case, shard);
            let still = after
                .iter()
                .find(|held| shard_id(&case, held) == untouched)
                .unwrap_or_else(|| panic!("case {case}: {untouched} must still be in the map"));
            assert!(
                is_open(&case, still) && hash_key_range(&case, still) == hash_key_range(&case, shard),
                "case {case}: expected {untouched} to be left as it was, got {still}"
            );
        }
    }

    // "After the operation is completed, Kinesis Data Streams sets the StreamStatus to
    // ACTIVE", with one more shard open than before the split.
    tokio::time::sleep(past_reshard()).await;
    for (index, (shard_count, _, _)) in cases.iter().enumerate() {
        let case = format!("split-{index}-of-{shard_count}");
        let (status, open) = summary(&emulator, &case, &case).await;
        assert_eq!(
            status, STREAM_ACTIVE,
            "case {case}: expected the stream to be {STREAM_ACTIVE} once the split fell \
             due, got {status}"
        );
        assert_eq!(
            open,
            u64::from(*shard_count) + 1,
            "case {case}: expected {} open shards once the split fell due, got {open}",
            shard_count + 1
        );
    }
}

/// An accepted merge closes both parents and covers their joined range with one child.
#[tokio::test]
async fn merge_shards_joins_two_adjacent_shards() {
    // (shard count, the lower shard of the pair, whether the request names them in the
    // adjacent-first order)
    let cases: Vec<(u32, usize, bool)> = vec![
        (2, 0, false),
        (2, 0, true),
        (3, 1, false),
        (4, 0, false),
        (4, 2, true),
    ];

    let emulator = Emulator::new();
    for (index, (shard_count, _, _)) in cases.iter().enumerate() {
        let case = format!("merge-{index}-of-{shard_count}");
        create_stream(&emulator, &case, *shard_count, STREAM_MODE_PROVISIONED).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (index, (shard_count, lower, reversed)) in cases.iter().enumerate() {
        let case = format!("merge-{index}-of-{shard_count}");

        let before = open_shards(&emulator, &case, &case).await;
        let pair: Vec<&Value> = before
            .iter()
            .skip(*lower)
            .take(2)
            .collect();
        assert_eq!(
            pair.len(),
            2,
            "case {case}: the map must hold an adjacent pair at {lower}"
        );
        let lower_id = shard_id(&case, pair[0]);
        let upper_id = shard_id(&case, pair[1]);
        let (starting, _) = hash_key_range(&case, pair[0]);
        let (_, ending) = hash_key_range(&case, pair[1]);
        let (named, adjacent) = if *reversed {
            (upper_id.clone(), lower_id.clone())
        } else {
            (lower_id.clone(), upper_id.clone())
        };

        emulator
            .ok(
                &case,
                "MergeShards",
                &json!({
                    "StreamName": case,
                    "ShardToMerge": named,
                    "AdjacentShardToMerge": adjacent,
                }),
            )
            .await;

        let (status, _) = summary(&emulator, &case, &case).await;
        assert_eq!(
            status, STREAM_UPDATING,
            "case {case}: expected the stream to be {STREAM_UPDATING} straight after the \
             merge, got {status}"
        );

        let after = shard_map(&emulator, &case, &case).await;
        let closed: Vec<String> = after
            .iter()
            .filter(|shard| !is_open(&case, shard))
            .map(|shard| shard_id(&case, shard))
            .collect();
        assert_eq!(
            closed,
            vec![lower_id.clone(), upper_id.clone()],
            "case {case}: expected exactly the two parents to be closed, got {closed:?}"
        );

        // "After the merge, the single child shard receives data for all hash key values
        // covered by the two parent shards", and it names both of them.
        let children: Vec<&Value> = after
            .iter()
            .filter(|shard| {
                is_open(&case, shard) && parents(shard) == vec![lower_id.clone(), upper_id.clone()]
            })
            .collect();
        assert_eq!(
            children.len(),
            1,
            "case {case}: expected one child of {lower_id} and {upper_id}, got {children:?}"
        );
        assert_eq!(
            hash_key_range(&case, children[0]),
            (starting, ending),
            "case {case}: expected the child to cover {starting}..{ending}"
        );
    }

    tokio::time::sleep(past_reshard()).await;
    for (index, (shard_count, _, _)) in cases.iter().enumerate() {
        let case = format!("merge-{index}-of-{shard_count}");
        let (status, open) = summary(&emulator, &case, &case).await;
        assert_eq!(
            status, STREAM_ACTIVE,
            "case {case}: expected the stream to be {STREAM_ACTIVE} once the merge fell \
             due, got {status}"
        );
        assert_eq!(
            open,
            u64::from(*shard_count) - 1,
            "case {case}: expected {} open shards once the merge fell due, got {open}",
            shard_count - 1
        );
    }
}

/// What the stream a refusal row addresses looks like when the call arrives.
#[derive(Debug, Clone, Copy)]
enum Subject {
    /// A provisioned stream of this many shards, `ACTIVE`.
    Active(u32),
    /// An on-demand stream, whose shard map is the service's to re-tile.
    OnDemand,
    /// A provisioned stream of this many shards whose first shard has been split, so that
    /// shard is closed when the call under test arrives.
    Split(u32),
    /// A stream whose create has not fallen due yet.
    Creating,
    /// No stream at all.
    Absent,
}

/// One refused reshard: the operation, the stream it addresses, the request it makes and the
/// modelled refusal it must answer. The request is built from the stream's own shard map, so
/// a row names ids and hash keys the emulator minted rather than literals.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    subject: Subject,
    /// The request body, built from the stream's name and its open shards.
    body: fn(&str, &[Value]) -> Value,
    code: &'static str,
    /// The message the refusal must carry, built the same way.
    says: fn(&str, &[Value]) -> String,
}

/// A refused reshard answers the modelled client error and leaves the shard map as it was.
#[tokio::test]
async fn a_refused_reshard_changes_nothing() {
    /// A key past the 128-bit hash key space, which the `HashKey` pattern
    /// `^(0|([1-9]\d{0,38}))$` admits and no shard's range holds: 2^128.
    const PAST_THE_KEY_SPACE: &str = "340282366920938463463374607431768211456";

    let cases: Vec<Refusal> = vec![
        Refusal {
            case: "split-at-the-parents-own-start",
            operation: "SplitShard",
            subject: Subject::Active(1),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": shard_id(name, &open[0]),
                    "NewStartingHashKey": hash_key_range(name, &open[0]).0.to_string(),
                })
            },
            code: "InvalidArgumentException",
            says: |name, open| {
                let (starting, ending) = hash_key_range(name, &open[0]);
                split_hash_key_outside_the_parent_message(
                    &starting.to_string(),
                    &shard_id(name, &open[0]),
                    name,
                    &starting.to_string(),
                    &ending.to_string(),
                )
            },
        },
        Refusal {
            case: "split-at-one-plus-the-start",
            operation: "SplitShard",
            subject: Subject::Active(2),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": shard_id(name, &open[1]),
                    "NewStartingHashKey": (hash_key_range(name, &open[1]).0 + 1).to_string(),
                })
            },
            code: "InvalidArgumentException",
            says: |name, open| {
                let (starting, ending) = hash_key_range(name, &open[1]);
                split_hash_key_outside_the_parent_message(
                    &(starting + 1).to_string(),
                    &shard_id(name, &open[1]),
                    name,
                    &starting.to_string(),
                    &ending.to_string(),
                )
            },
        },
        Refusal {
            case: "split-at-the-parents-own-end",
            operation: "SplitShard",
            subject: Subject::Active(2),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": shard_id(name, &open[0]),
                    "NewStartingHashKey": hash_key_range(name, &open[0]).1.to_string(),
                })
            },
            code: "InvalidArgumentException",
            says: |name, open| {
                let (starting, ending) = hash_key_range(name, &open[0]);
                split_hash_key_outside_the_parent_message(
                    &ending.to_string(),
                    &shard_id(name, &open[0]),
                    name,
                    &starting.to_string(),
                    &ending.to_string(),
                )
            },
        },
        Refusal {
            case: "split-in-another-shards-range",
            operation: "SplitShard",
            subject: Subject::Active(2),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": shard_id(name, &open[1]),
                    "NewStartingHashKey": hash_key_range(name, &open[0]).0.to_string(),
                })
            },
            code: "InvalidArgumentException",
            says: |name, open| {
                let (starting, ending) = hash_key_range(name, &open[1]);
                split_hash_key_outside_the_parent_message(
                    &hash_key_range(name, &open[0]).0.to_string(),
                    &shard_id(name, &open[1]),
                    name,
                    &starting.to_string(),
                    &ending.to_string(),
                )
            },
        },
        Refusal {
            case: "split-past-the-key-space",
            operation: "SplitShard",
            subject: Subject::Active(1),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": shard_id(name, &open[0]),
                    "NewStartingHashKey": PAST_THE_KEY_SPACE,
                })
            },
            code: "InvalidArgumentException",
            says: |name, open| {
                let (starting, ending) = hash_key_range(name, &open[0]);
                split_hash_key_outside_the_parent_message(
                    PAST_THE_KEY_SPACE,
                    &shard_id(name, &open[0]),
                    name,
                    &starting.to_string(),
                    &ending.to_string(),
                )
            },
        },
        Refusal {
            case: "split-an-on-demand-stream",
            operation: "SplitShard",
            subject: Subject::OnDemand,
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": shard_id(name, &open[0]),
                    "NewStartingHashKey": hash_key_range(name, &open[0]).1.to_string(),
                })
            },
            code: "ValidationException",
            says: |name, _| stream_on_demand_message(name),
        },
        Refusal {
            case: "split-a-shard-nothing-holds",
            operation: "SplitShard",
            subject: Subject::Active(1),
            body: |name, _| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": "shardId-000000000099",
                    "NewStartingHashKey": "1024",
                })
            },
            code: "ResourceNotFoundException",
            says: |name, _| shard_not_in_stream_message("shardId-000000000099", name),
        },
        Refusal {
            case: "split-a-closed-shard",
            operation: "SplitShard",
            subject: Subject::Split(2),
            body: |name, open| {
                // The first shard of the map was split while the stream was set up, so it
                // is closed; the open listing no longer holds it.
                let _ = open;
                json!({
                    "StreamName": name,
                    "ShardToSplit": "shardId-000000000000",
                    "NewStartingHashKey": "1024",
                })
            },
            code: "InvalidArgumentException",
            says: |name, _| shard_not_open_message("shardId-000000000000", name),
        },
        Refusal {
            case: "split-while-still-creating",
            operation: "SplitShard",
            subject: Subject::Creating,
            body: |name, _| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": "shardId-000000000000",
                    "NewStartingHashKey": "1024",
                })
            },
            code: "ResourceInUseException",
            says: |_, _| "CREATING".to_string(),
        },
        Refusal {
            case: "split-a-stream-nothing-holds",
            operation: "SplitShard",
            subject: Subject::Absent,
            body: |name, _| {
                json!({
                    "StreamName": name,
                    "ShardToSplit": "shardId-000000000000",
                    "NewStartingHashKey": "1024",
                })
            },
            code: "ResourceNotFoundException",
            says: |name, _| stream_not_found_message(name),
        },
        Refusal {
            case: "split-without-an-identifier",
            operation: "SplitShard",
            subject: Subject::Absent,
            body: |_, _| {
                json!({
                    "ShardToSplit": "shardId-000000000000",
                    "NewStartingHashKey": "1024",
                })
            },
            code: "InvalidArgumentException",
            says: |_, _| STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        },
        Refusal {
            case: "merge-an-on-demand-stream",
            operation: "MergeShards",
            subject: Subject::OnDemand,
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToMerge": shard_id(name, &open[0]),
                    "AdjacentShardToMerge": shard_id(name, &open[1]),
                })
            },
            code: "ValidationException",
            says: |name, _| stream_on_demand_message(name),
        },
        Refusal {
            case: "merge-shards-that-do-not-meet",
            operation: "MergeShards",
            subject: Subject::Active(3),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToMerge": shard_id(name, &open[0]),
                    "AdjacentShardToMerge": shard_id(name, &open[2]),
                })
            },
            code: "InvalidArgumentException",
            says: |name, open| {
                not_an_adjacent_pair(
                    &shard_id(name, &open[0]),
                    &shard_id(name, &open[2]),
                    name,
                )
            },
        },
        Refusal {
            case: "merge-one-shard-with-itself",
            operation: "MergeShards",
            subject: Subject::Active(2),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToMerge": shard_id(name, &open[0]),
                    "AdjacentShardToMerge": shard_id(name, &open[0]),
                })
            },
            code: "InvalidArgumentException",
            says: |name, open| {
                not_an_adjacent_pair(
                    &shard_id(name, &open[0]),
                    &shard_id(name, &open[0]),
                    name,
                )
            },
        },
        Refusal {
            case: "merge-a-shard-nothing-holds",
            operation: "MergeShards",
            subject: Subject::Active(2),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToMerge": shard_id(name, &open[0]),
                    "AdjacentShardToMerge": "shardId-000000000099",
                })
            },
            code: "ResourceNotFoundException",
            says: |name, _| shard_not_in_stream_message("shardId-000000000099", name),
        },
        Refusal {
            case: "merge-a-closed-shard",
            operation: "MergeShards",
            subject: Subject::Split(2),
            body: |name, open| {
                json!({
                    "StreamName": name,
                    "ShardToMerge": "shardId-000000000000",
                    "AdjacentShardToMerge": shard_id(name, &open[0]),
                })
            },
            code: "InvalidArgumentException",
            says: |name, _| shard_not_open_message("shardId-000000000000", name),
        },
        Refusal {
            case: "merge-while-still-creating",
            operation: "MergeShards",
            subject: Subject::Creating,
            body: |name, _| {
                json!({
                    "StreamName": name,
                    "ShardToMerge": "shardId-000000000000",
                    "AdjacentShardToMerge": "shardId-000000000001",
                })
            },
            code: "ResourceInUseException",
            says: |_, _| "CREATING".to_string(),
        },
        Refusal {
            case: "merge-in-a-stream-nothing-holds",
            operation: "MergeShards",
            subject: Subject::Absent,
            body: |name, _| {
                json!({
                    "StreamName": name,
                    "ShardToMerge": "shardId-000000000000",
                    "AdjacentShardToMerge": "shardId-000000000001",
                })
            },
            code: "ResourceNotFoundException",
            says: |name, _| stream_not_found_message(name),
        },
    ];

    let emulator = Emulator::new();
    for refusal in &cases {
        let case = refusal.case;
        match refusal.subject {
            Subject::Active(shards) | Subject::Split(shards) => {
                create_stream(&emulator, case, shards, STREAM_MODE_PROVISIONED).await;
            }
            Subject::OnDemand => {
                create_stream(&emulator, case, 0, STREAM_MODE_ON_DEMAND).await;
            }
            // A stream whose create is still due is armed inside the loop below, so the
            // call arrives while that transition is still in flight; an absent one is never
            // created at all.
            Subject::Creating | Subject::Absent => {}
        }
    }
    // The CONDITION each set-up stream reaches on the emulator's own clock, looked for again
    // until it holds — never a duration this case measures.
    for refusal in &cases {
        match refusal.subject {
            Subject::Active(_) | Subject::Split(_) | Subject::OnDemand => {
                poll_until_active(&emulator, refusal.case, refusal.case).await;
            }
            Subject::Creating | Subject::Absent => {}
        }
    }

    for refusal in &cases {
        let case = refusal.case;
        if let Subject::Split(_) = refusal.subject {
            // Closing a shard takes a split, so the row that needs a closed shard splits
            // the first one and waits the change out: the call under test then arrives at an
            // ACTIVE stream whose shardId-000000000000 is closed.
            let open = open_shards(&emulator, case, case).await;
            let (starting, ending) = hash_key_range(case, &open[0]);
            emulator
                .ok(
                    case,
                    "SplitShard",
                    &json!({
                        "StreamName": case,
                        "ShardToSplit": shard_id(case, &open[0]),
                        "NewStartingHashKey": (starting + (ending - starting) / 2).to_string(),
                    }),
                )
                .await;
            // The split closes the parent and leaves the stream `UPDATING`: what this row
            // waits for is the stream reporting `ACTIVE` again with that parent closed.
            poll_until_active(&emulator, case, case).await;
        }
        if let Subject::Creating = refusal.subject {
            emulator
                .ok(
                    case,
                    "CreateStream",
                    &json!({ "StreamName": case, "ShardCount": 2 }),
                )
                .await;
        }

        // A stream nothing holds, and one whose create is still due, serve no shard map to
        // build the request from or to compare against.
        let before = match refusal.subject {
            Subject::Absent | Subject::Creating => None,
            _ => Some(shard_map(&emulator, case, case).await),
        };
        let open: Vec<Value> = before
            .iter()
            .flatten()
            .filter(|shard| is_open(case, shard))
            .cloned()
            .collect();

        let (status, response) = emulator
            .call(refusal.operation, &(refusal.body)(case, &open))
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
        let says = (refusal.says)(case, &open);
        let message = error_message(&response);
        assert!(
            message.contains(&says),
            "case {case}: expected the message to name {says:?}, got {message:?}"
        );

        if let Some(before) = before {
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
                "case {case}: a refused reshard must leave the shard map as it was"
            );
        }
    }
}
