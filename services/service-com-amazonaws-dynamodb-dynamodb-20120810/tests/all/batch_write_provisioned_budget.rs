// SPDX-License-Identifier: CC0-1.0

//! What the write capacity a table is PROVISIONED for does to a STREAM of
//! `BatchWriteItem` calls.
//!
//! One rule over a table of inputs: every request of a batch is admitted on its own
//! against the write capacity its table has left, so a table provisioned for a write rate
//! cannot absorb a stream of writes past that rate plus the burst credit it holds. What a
//! call does not admit comes back in `UnprocessedItems` in the form it was sent ("you can
//! provide this value directly to a subsequent BatchWriteItem operation") with HTTP 200
//! and stores nothing, and a call that could store nothing at all is refused with
//! `ProvisionedThroughputExceededException`. The keys that landed and the keys handed back
//! therefore partition the requested keys exactly: no key is both, and none is neither. A
//! table in on-demand mode is provisioned for no capacity to exceed, so it absorbs the
//! same stream whole.
//!
//! The rows differ along what the rule generalises over: the capacity mode, the write
//! units the table is provisioned for, the bytes of each item (and so the units one write
//! costs), the requests per batch, and the number of batches in the stream — from a single
//! batch the table's provisioning covers whole, through streams of ten to 320 batches, to a
//! batch whose every request asks for more than its table's credit could ever hold.
//!
//! The table here is keyed by a partition key alone and every write is aimed at a key of
//! its own, so the ceiling under test is the table's own provisioning rather than the
//! per-partition-key one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, error_code, message, ok, REGION};

/// How long ONE call of this suite may take before it is called a hang: a batch write is
/// decided from stored state, so it answers whether or not capacity covered it. Generous
/// slack over the milliseconds it takes, because a loaded machine is not a defect.
const CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// One write capacity unit covers a write of up to 1 KB (DynamoDB Developer Guide,
/// "Read/write capacity mode"), so an item of this many bytes costs one unit.
const WRITE_UNIT_BYTES: usize = 1024;

/// How many seconds of unused write capacity a provisioned table keeps for a burst:
/// "DynamoDB currently retains up to 300 seconds (5 minutes) of unused read and write
/// capacity" (DynamoDB Developer Guide, "Using burst capacity effectively"). It is the
/// most credit a budget can hold, and so part of the ceiling on what a stream of batches
/// can land.
const BURST_SECONDS: u64 = 300;

/// Seconds of refill credited to a table beyond the measured wall clock of its stream, so
/// a slow machine cannot make the bound below fail: the budget also refills while the
/// table is being created and while its stream is being scanned back.
const REFILL_SLACK_SECONDS: u64 = 5;

/// The read units every table here is given — ample, since what is under test is the
/// WRITE budget and the scan that reads the landed keys back must not be the thing that
/// runs short.
const READ_UNITS: u64 = 2000;

/// The fixed sentence a write past the capacity its table is provisioned for is refused
/// with, as the Developer Guide states it for that quota: the whole sentence pair, with no
/// value from the request or from state in it.
const THROUGHPUT_EXCEEDED: &str = "The level of configured provisioned throughput for the \
                                   table was exceeded. Consider increasing your \
                                   provisioning level with the UpdateTable API.";

/// Create a table keyed by `pk` (HASH, string) alone in the given capacity mode and wait
/// out its `CREATING` span. `write_units` is `None` for an on-demand table.
async fn table_for(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    write_units: Option<u64>,
) {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
    });
    match write_units {
        Some(units) => {
            body["BillingMode"] = json!("PROVISIONED");
            body["ProvisionedThroughput"] =
                json!({"ReadCapacityUnits": READ_UNITS, "WriteCapacityUnits": units});
        }
        None => body["BillingMode"] = json!("PAY_PER_REQUEST"),
    }
    ok(router, "CreateTable", body).await;
    describe_when_active(router, name, name).await;
}

/// The `PutRequest` of an item of `payload` bytes at `key`.
fn put_of(key: &str, payload: usize) -> Value {
    json!({"PutRequest": {"Item": {
        "pk": {"S": key},
        "payload": {"S": "x".repeat(payload)}}}})
}

/// The partition key a put request addresses.
fn key_of_request(request: &Value) -> String {
    request["PutRequest"]["Item"]["pk"]["S"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Send one `BatchWriteItem` of `requests` against `table`, answering its status, its body
/// and how long it took.
async fn batch_write(
    router: &smithy_server_runtime::MultiServiceRouter,
    table: &str,
    requests: Vec<Value>,
) -> (u16, Value, std::time::Duration) {
    let started = std::time::Instant::now();
    let (status, body) = call(
        router,
        "BatchWriteItem",
        json!({"RequestItems": {table: requests}}),
    )
    .await;
    (status, body, started.elapsed())
}

/// The requests a response handed back for `table`, "in the same form as RequestItems".
fn residue_of(body: &Value, table: &str) -> Vec<Value> {
    body["UnprocessedItems"][table]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every key the table holds, read back through a paginated strongly consistent `Scan`.
async fn stored_keys(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    table: &str,
) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut start_key = Value::Null;
    for _ in 0..1000 {
        let mut body = json!({"TableName": table, "ConsistentRead": true,
                              "ProjectionExpression": "pk"});
        if !start_key.is_null() {
            body["ExclusiveStartKey"] = start_key.clone();
        }
        let page = ok(router, "Scan", body).await;
        keys.extend(
            page["Items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|item| item["pk"]["S"].as_str().unwrap_or_default().to_string()),
        );
        match page.get("LastEvaluatedKey") {
            Some(key) if !key.is_null() => start_key = key.clone(),
            _ => return keys,
        }
    }
    panic!("case {case:?}: the scan of the table never reached its last page");
}

/// What the rule says a row's stream of batches does to the keys it requested.
#[derive(Debug, PartialEq, Eq)]
enum Absorption {
    /// Every requested key lands and nothing is handed back: the capacity behind the
    /// table covers the whole stream.
    Whole,
    /// Some keys land and the rest come back in `UnprocessedItems`: the budget bounds how
    /// many of the stream's writes are admitted, and the two sets partition the request.
    Bounded,
    /// Every call is refused `ProvisionedThroughputExceededException` and stores nothing:
    /// no request of the batch is one any retry could get through.
    Refused,
}

/// One row: a capacity mode, the shape of the stream sent against it, and what the rule
/// says the stream is answered with.
struct Row {
    case: &'static str,
    /// The write units the table is provisioned for, or `None` for on-demand.
    write_units: Option<u64>,
    batches: usize,
    per_batch: usize,
    /// The bytes of the one non-key attribute of each item, which is what decides how many
    /// write units a single write of it costs.
    payload: usize,
    absorption: Absorption,
}

/// The rows: the capacity mode, the provisioned units, the item size, the requests per
/// batch and the length of the stream.
///
/// A write costs one unit per 1 KB of the item and a budget holds at most
/// [`BURST_SECONDS`] of its rate — credit that is capacity which WENT UNUSED, so a table
/// just created holds only what went unused since — which is why 320 batches of 25 ~1 KB
/// writes are far past what a one-unit table can absorb, while one such batch against a
/// table provisioned for fifty units is well inside it. A batch of 399 KB writes against a
/// one-unit table lands exactly the one request the table had capacity in hand for and
/// hands the rest back: the capacity a table is provisioned for is a sustained RATE, so the
/// request that arrives with capacity left is charged what it consumed and it is the ones
/// after it that meet the deficit.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "on-demand, one full batch of ~1 KB writes",
            write_units: None,
            batches: 1,
            per_batch: 25,
            payload: 900,
            absorption: Absorption::Whole,
        },
        Row {
            case: "on-demand, ten full batches of 4 KB writes",
            write_units: None,
            batches: 10,
            per_batch: 25,
            payload: 4 * 1024,
            absorption: Absorption::Whole,
        },
        Row {
            case: "fifty write units, one full batch of ~1 KB writes",
            write_units: Some(50),
            batches: 1,
            per_batch: 25,
            payload: 900,
            absorption: Absorption::Whole,
        },
        Row {
            case: "one write unit, 320 full batches of ~1 KB writes",
            write_units: Some(1),
            batches: 320,
            per_batch: 25,
            payload: 900,
            absorption: Absorption::Bounded,
        },
        Row {
            case: "five write units, forty full batches of 4 KB writes",
            write_units: Some(5),
            batches: 40,
            per_batch: 25,
            payload: 4 * 1024,
            absorption: Absorption::Bounded,
        },
        Row {
            case: "two write units, twenty batches of ten 8 KB writes",
            write_units: Some(2),
            batches: 20,
            per_batch: 10,
            payload: 8 * 1024,
            absorption: Absorption::Bounded,
        },
        Row {
            case: "one write unit, one batch of writes past the whole burst",
            write_units: Some(1),
            batches: 1,
            per_batch: 3,
            payload: 399 * 1024,
            absorption: Absorption::Bounded,
        },
    ]
}

/// The MOST writes of `payload` bytes each a table provisioned for `write_units` can have
/// admitted over a stream of `batches` calls that took `elapsed`.
///
/// Two terms, one per way a write is funded: the capacity the budget could hold and refill
/// over the stream ([`BURST_SECONDS`] of the rate plus the rate over the elapsed time),
/// divided by what one write of this size costs; plus the progress a call makes while it
/// has stored nothing, which is one request per call, so at most one per batch. A table
/// that admitted more than this absorbed writes nothing paid for.
fn most_admitted(
    write_units: u64,
    payload: usize,
    batches: usize,
    elapsed: std::time::Duration,
) -> usize {
    let units_per_write = payload.div_ceil(WRITE_UNIT_BYTES).max(1);
    let seconds = BURST_SECONDS + elapsed.as_secs() + REFILL_SLACK_SECONDS;
    let funded = usize::try_from(write_units.saturating_mul(seconds)).unwrap_or(usize::MAX);
    batches.saturating_add(funded / units_per_write)
}

#[tokio::test]
async fn a_provisioned_write_budget_bounds_a_stream_of_batches_and_loses_no_key() {
    let router = crate::emulator::emulator();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let table = format!("Budget{index}");
        table_for(&router, &table, row.write_units).await;

        let requested: Vec<String> = (0..row.batches * row.per_batch)
            .map(|slot| format!("k{slot}"))
            .collect();
        // Every request is kept as it was sent, so a handed-back one can be compared with
        // it member for member.
        let sent: BTreeMap<String, Value> = requested
            .iter()
            .map(|key| (key.clone(), put_of(key, row.payload)))
            .collect();

        let mut reported: BTreeSet<String> = BTreeSet::new();
        let mut refusals = 0usize;
        let started = std::time::Instant::now();
        for chunk in requested.chunks(row.per_batch) {
            let requests: Vec<Value> = chunk
                .iter()
                .filter_map(|key| sent.get(key).cloned())
                .collect();
            let (status, body, call_elapsed) = batch_write(&router, &table, requests).await;
            assert!(
                call_elapsed < CALL_DEADLINE,
                "case {case:?}: every call is answered whether or not capacity covered it, \
                 one took {call_elapsed:?}"
            );

            if row.absorption == Absorption::Refused {
                assert_eq!(
                    (status, error_code(&body), message(&body)),
                    (
                        400,
                        "ProvisionedThroughputExceededException".to_string(),
                        THROUGHPUT_EXCEEDED.to_string()
                    ),
                    "case {case:?}: a batch no retry could get through is refused with the \
                     provisioning sentence, got {status} with {body}"
                );
                assert_eq!(
                    body["ThrottlingReasons"],
                    json!([{"reason": "TableWriteProvisionedThroughputExceeded",
                            "resource":
                                format!("arn:aws:dynamodb:{REGION}:000000000000:table/{table}")}]),
                    "case {case:?}: the refusal names the ceiling that bound and the table it \
                     bound on, got {body}"
                );
                refusals += 1;
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case:?}: a batch whose residue a retry can drain is served, got {status} \
                 with {body}"
            );
            for request in residue_of(&body, &table) {
                let key = key_of_request(&request);
                assert_eq!(
                    Some(&request),
                    sent.get(&key),
                    "case {case:?}, key {key:?}: a handed-back request keeps the form it was \
                     sent in, got {request}"
                );
                assert!(
                    reported.insert(key.clone()),
                    "case {case:?}, key {key:?}: a key is handed back at most once over the \
                     stream, this is the second time"
                );
            }
        }
        let elapsed = started.elapsed();

        let read_back = stored_keys(&router, case, &table).await;
        let landed: BTreeSet<String> = read_back.iter().cloned().collect();
        assert_eq!(
            landed.len(),
            read_back.len(),
            "case {case:?}: a key lands at most once, the scan read {} keys for {} distinct ones",
            read_back.len(),
            landed.len()
        );
        let requested: BTreeSet<String> = requested.into_iter().collect();

        if row.absorption == Absorption::Refused {
            assert_eq!(
                (refusals, landed.len(), reported.len()),
                (row.batches, 0, 0),
                "case {case:?}: every call of the stream is refused and stores nothing, {} of \
                 {} calls were refused with {} keys stored and {} handed back",
                refusals,
                row.batches,
                landed.len(),
                reported.len()
            );
            continue;
        }

        match row.absorption {
            Absorption::Whole => assert_eq!(
                (&landed, reported.len()),
                (&requested, 0),
                "case {case:?}: capacity that covers the stream absorbs it whole, {} of {} keys \
                 landed and {} were handed back",
                landed.len(),
                requested.len(),
                reported.len()
            ),
            Absorption::Bounded => {
                assert!(
                    landed.len() < requested.len(),
                    "case {case:?}: a table provisioned for {:?} write units cannot absorb all \
                     {} writes, {} landed",
                    row.write_units,
                    requested.len(),
                    landed.len()
                );
                assert!(
                    !landed.is_empty() && !reported.is_empty(),
                    "case {case:?}: each call stores what it can and hands the rest back, {} \
                     keys landed and {} were handed back",
                    landed.len(),
                    reported.len()
                );
                let ceiling = most_admitted(
                    row.write_units.unwrap_or_default(),
                    row.payload,
                    row.batches,
                    elapsed,
                );
                assert!(
                    landed.len() <= ceiling,
                    "case {case:?}: at most {ceiling} writes are funded by what the table is \
                     provisioned for plus the one request a call always makes progress on, \
                     {} landed over {elapsed:?}",
                    landed.len()
                );
            }
            Absorption::Refused => unreachable!("the refusal arm returned above"),
        }

        // The landed keys and the handed-back keys PARTITION the requested keys: every one
        // of them is either stored or reported, and none is both — which is what lets a
        // client retry exactly the residue and lose nothing.
        assert!(
            landed.is_disjoint(&reported),
            "case {case:?}: no key is both stored and handed back, {} landed and {} were \
             handed back",
            landed.len(),
            reported.len()
        );
        let accounted: BTreeSet<String> = landed.union(&reported).cloned().collect();
        assert_eq!(
            accounted.len(),
            requested.len(),
            "case {case:?}: the landed keys and the handed-back keys must partition the {} \
             requested keys, {} landed and {} were handed back",
            requested.len(),
            landed.len(),
            reported.len()
        );
        assert_eq!(
            accounted, requested,
            "case {case:?}: every requested key is accounted for exactly once, by landing or \
             by being handed back"
        );
    }
}
