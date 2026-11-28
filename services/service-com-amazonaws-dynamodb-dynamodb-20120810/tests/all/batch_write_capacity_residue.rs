// SPDX-License-Identifier: CC0-1.0

//! What a batch write does with the requests its table's capacity cannot cover right now.
//!
//! One rule, over a table of inputs rather than one request: how much capacity a table
//! has left decides HOW MANY of a batch's requests land, not WHETHER the call gets
//! anything done — `BatchWriteItem` stores the first request it can, serving it from
//! capacity borrowed against its budget's own refill when the budget is short, and hands
//! every request it did not store back in `UnprocessedItems` with HTTP 200, a value "you
//! can provide ... directly to a subsequent BatchWriteItem operation", so the caller's
//! retry-the-residue loop drains and no requested key is lost. It refuses the whole call
//! with `ProvisionedThroughputExceededException` when it got no work done: when its
//! requests each ask for more than the ceilings behind the table can ever hold, or when
//! the borrowing that would have served the first of them is past the allowance a table
//! lends against its provisioning.
//!
//! The rows differ along what the rule generalises over: the capacity mode, the write
//! units the table is provisioned for, the size of each item (and so the units one write
//! costs), how much of the budget was already spent, how many requests the batch carries,
//! which of the two ceilings is the binding one, and whether a batch mixes a request that
//! is only waiting for capacity with one that can never be admitted.
//!
//! The second suite walks the documented loop end to end: a stream of batches against a
//! table provisioned for one write unit, then `UpdateTable` to a capacity that covers the
//! residue, then the residue resent — with the landed keys and the reported ones
//! partitioning the requested keys exactly at every step, and every call answering inside
//! a bound. The third holds that progress to the table's provisioning: the capacity a
//! batch may borrow against its budget's refill is finite, so a caller issuing
//! one-request batches past what its table is provisioned for is refused rather than
//! served forever. The fourth holds the rule to the other request form a batch carries: a
//! `DeleteRequest` is admitted from the same capacity and handed back in the form it was
//! sent in.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::emulator::{call, describe_when_active, error_code, message, ok, REGION};

/// How long ONE call of this suite may take before it is called a hang: a batch write is
/// answered from stored state, so it answers whether or not capacity covered it. Generous
/// slack over the microseconds it takes, because a loaded machine is not a defect.
const CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a drain loop waits between attempts at a residue, and how many attempts it
/// makes — the "exponential backoff" the operation's doc recommends, flattened, because
/// what is under test is that the loop TERMINATES.
const DRAIN_PAUSE: std::time::Duration = std::time::Duration::from_millis(250);
const DRAIN_ATTEMPTS: usize = 20;

/// The write units a drained table is given: enough that the residue is only waiting for
/// the pause above, whichever ceiling held it back.
const DRAIN_WRITE_UNITS: i64 = 4000;

/// The read units a drained table is given beside them: the figure [`give_capacity`] both
/// compares the table against and asks for, so the two cannot drift apart.
const DRAIN_READ_UNITS: i64 = 2000;

/// How many requests one `BatchWriteItem` carries at most
/// (`BatchWriteItem$RequestItems` is `@length(1..25)`), which is what the loops here send
/// per call.
const MAX_BATCH_REQUESTS: usize = 25;

/// How many calls [`spend_write_budget`] makes before it reports a table that never runs
/// short: a provisioned budget holds at most 300 seconds of its rate (Developer Guide,
/// "Using burst capacity effectively"), so a loop of one-unit writes empties the budget of
/// the one- and two-unit tables below inside a fraction of this many calls, whatever credit
/// had accumulated behind them.
const SPEND_CALLS: usize = 200;

/// The fixed sentence a refused write carries, as the Developer Guide states it for a
/// table that has spent the throughput it is provisioned for.
const THROUGHPUT_EXCEEDED: &str = "The level of configured provisioned throughput for the \
                                   table was exceeded. Consider increasing your \
                                   provisioning level with the UpdateTable API.";

/// One item's primary key, as the assertions compare keys.
type Key = (String, i64);

/// Create a table keyed `pk` (HASH, string) / `sk` (RANGE, number) with the given billing
/// members and wait out its `CREATING` span.
async fn table_with(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    billing: Value,
) {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"},
                      {"AttributeName": "sk", "KeyType": "RANGE"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"},
                                 {"AttributeName": "sk", "AttributeType": "N"}],
    });
    for (member, value) in billing.as_object().expect("the billing members are a map") {
        body[member.clone()] = value.clone();
    }
    ok(router, "CreateTable", body).await;
    describe_when_active(router, name, name).await;
}

/// The `PutRequest` of an item of `payload` bytes at `key`.
fn put_of(key: &Key, payload: usize) -> Value {
    json!({"PutRequest": {"Item": {
        "pk": {"S": key.0}, "sk": {"N": key.1.to_string()},
        "payload": {"S": "x".repeat(payload)}}}})
}

/// The primary key of an item, as the assertions compare keys.
fn key_of(item: &Value) -> Key {
    (
        item["pk"]["S"].as_str().unwrap_or_default().to_string(),
        item["sk"]["N"]
            .as_str()
            .unwrap_or_default()
            .parse()
            .unwrap_or_default(),
    )
}

/// Send one `BatchWriteItem` of `requests` against `table` and answer its status, its
/// body and how long it took.
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

/// The keys of those requests.
fn handed_back(body: &Value, table: &str) -> Vec<Key> {
    residue_of(body, table)
        .iter()
        .map(|request| key_of(&request["PutRequest"]["Item"]))
        .collect()
}

/// Walk the documented retry loop over `residue` until nothing is held back, answering
/// what is still held back when the attempts run out. `case` names the row.
///
/// An attempt is answered either way the operation documents: 200 with the requests it did
/// not store, or `ProvisionedThroughputExceededException` when it "cannot process any
/// items due to throttling", which the doc answers with the same instruction — "you should
/// retry the batch operation on those items" — so the chunk stays in the residue and the
/// loop goes round again. What is under test is that the loop TERMINATES, and that the
/// capacity behind the table is what drains it: a residue the table has the capacity for
/// must be SERVED, so a stream of attempts every one of which was throttled is a failure
/// however the loop ended.
async fn drain(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    table: &str,
    mut residue: Vec<Value>,
) -> Vec<Value> {
    let asked = residue.len();
    let mut served = 0usize;
    for attempt in 0..DRAIN_ATTEMPTS {
        if residue.is_empty() {
            break;
        }
        tokio::time::sleep(DRAIN_PAUSE).await;
        let mut still_held: Vec<Value> = Vec::new();
        for chunk in residue.chunks(25) {
            let (status, body, elapsed) = batch_write(router, table, chunk.to_vec()).await;
            assert!(
                (status == 200 || error_code(&body) == "ProvisionedThroughputExceededException")
                    && elapsed < CALL_DEADLINE,
                "case {case:?}, drain attempt {attempt}: the residue is answered promptly, got \
                 {status} after {elapsed:?} with {body}"
            );
            if status == 200 {
                served += 1;
                still_held.extend(residue_of(&body, table));
            } else {
                still_held.extend(chunk.to_vec());
            }
        }
        residue = still_held;
    }
    assert!(
        asked == 0 || served > 0,
        "case {case:?}: a residue of {asked} request(s) the table has the capacity for is \
         served, not throttled on every one of {DRAIN_ATTEMPTS} attempts"
    );
    residue
}

/// Spend `table`'s write budget down to nothing, so the batch under test meets the capacity
/// the table is PROVISIONED for rather than the credit that had accumulated behind it.
///
/// What a table has banked at any instant is how much of its provisioning went unused —
/// "DynamoDB currently retains up to 300 seconds (5 minutes) of unused read and write
/// capacity" (Developer Guide, "Using burst capacity effectively") — a figure no caller can
/// read. So it is EMPTIED rather than computed: batches of one-unit writes are sent until
/// one of them is handed part of its work back, which is the answer of a table with nothing
/// left to admit them from.
async fn spend_write_budget(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    table: &str,
) {
    for round in 0..SPEND_CALLS {
        let requests: Vec<Value> = (0..MAX_BATCH_REQUESTS)
            .map(|slot| put_of(&(format!("spend{round}-{slot}"), slot as i64), 0))
            .collect();
        let (status, body, _) = batch_write(router, table, requests).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a batch of one-unit writes is served while the budget is spent \
             down, got {status} with {body}"
        );
        if !handed_back(&body, table).is_empty() {
            return;
        }
    }
    panic!(
        "case {case:?}: a provisioned table holds at most 300 seconds of its rate, so \
         {SPEND_CALLS} batches of one-unit writes must run its budget out"
    )
}

/// Every key the table holds, read back through a paginated strongly consistent `Scan`.
async fn stored_keys(router: &smithy_server_runtime::MultiServiceRouter, table: &str) -> Vec<Key> {
    let mut keys: Vec<Key> = Vec::new();
    let mut start_key = Value::Null;
    for _ in 0..1000 {
        let mut body = json!({"TableName": table, "ConsistentRead": true,
                              "ProjectionExpression": "pk, sk"});
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
                .map(key_of),
        );
        match page.get("LastEvaluatedKey") {
            Some(key) if !key.is_null() => start_key = key.clone(),
            _ => return keys,
        }
    }
    panic!("case {table}: the scan of the table never reached its last page");
}

/// Raise `table` to a capacity that covers anything this suite asks of it.
///
/// A table already provisioned for that pair has the capacity, and asking an `UpdateTable`
/// for the pair already in force is refused as the no-op it is, so the raise is only sent
/// when it moves something.
async fn give_capacity(router: &smithy_server_runtime::MultiServiceRouter, table: &str) {
    let described = ok(router, "DescribeTable", json!({"TableName": table})).await;
    let in_force = &described["Table"]["ProvisionedThroughput"];
    if in_force["ReadCapacityUnits"] == json!(DRAIN_READ_UNITS)
        && in_force["WriteCapacityUnits"] == json!(DRAIN_WRITE_UNITS)
    {
        return;
    }
    ok(
        router,
        "UpdateTable",
        json!({"TableName": table, "BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": DRAIN_READ_UNITS,
                                         "WriteCapacityUnits": DRAIN_WRITE_UNITS}}),
    )
    .await;
}

// ===========================================================================
// 1. What a batch does with the requests capacity does not cover
// ===========================================================================

/// What the rule says a row's measured batch is answered with.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// HTTP 200, with NO request handed back: capacity covered the whole batch.
    ServedWhole,
    /// HTTP 200, with SOME but not all of its requests handed back.
    ServedPartly,
    /// HTTP 200 with exactly ONE request stored and every other handed back: the call
    /// made the progress a batch always makes, and no more.
    ServedOne,
    /// HTTP 400 `ProvisionedThroughputExceededException`: nothing was processed, because
    /// no request of the batch is one any retry would admit.
    Refused,
}

/// One row: a table, whether its budget is spent before the call, and the batch whose
/// answer the rule decides.
struct Row {
    case: &'static str,
    /// The billing members `CreateTable` is given.
    billing: Value,
    /// Whether the table's write budget is emptied before the measured batch
    /// ([`spend_write_budget`]), so the batch meets the rate the table is provisioned for
    /// rather than the credit that accumulated behind it.
    spend_budget: bool,
    /// The payload bytes of each request of the measured batch, one entry per request.
    batch: Vec<usize>,
    /// Whether the measured batch aims every one of its requests at ONE partition key
    /// value, which is the ceiling beneath the table's own.
    one_key: bool,
    answer: Answer,
}

/// The rows: the capacity mode, the provisioned units, the item size, whether the budget was
/// already spent, the number of requests, which ceiling binds, and a batch that mixes a
/// request waiting for capacity with one that can never be admitted.
///
/// A write costs one unit per 1 KB of the item, and a budget never holds more than 300
/// seconds of its rate (Developer Guide, "Using burst capacity effectively"), so a table
/// provisioned for one write unit can hold at most 300 units — which is why a ~400 KB
/// item is a write no amount of waiting admits ONCE THE BUDGET IS SPENT, while an 8 KB
/// item is one the table is merely short of capacity for. The rows that measure either of
/// those spend the budget first, because what admits a request is whether the table has
/// capacity LEFT: a table with capacity in hand serves the request that arrives and
/// charges what it consumed, however large, and it is the next request that meets the
/// deficit.
fn rows() -> Vec<Row> {
    let on_demand = json!({"BillingMode": "PAY_PER_REQUEST"});
    let provisioned = |write: i64| {
        json!({"BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": DRAIN_READ_UNITS,
                                         "WriteCapacityUnits": write}})
    };
    vec![
        Row {
            case: "on-demand, a full batch of small writes",
            billing: on_demand.clone(),
            spend_budget: false,
            batch: vec![900; 25],
            one_key: false,
            answer: Answer::ServedWhole,
        },
        Row {
            case: "one write unit, budget spent, a full batch of 8 KB writes",
            billing: provisioned(1),
            spend_budget: true,
            batch: vec![8 * 1024; 25],
            one_key: false,
            answer: Answer::ServedOne,
        },
        Row {
            case: "one write unit, budget spent, a single 8 KB write",
            billing: provisioned(1),
            spend_budget: true,
            batch: vec![8 * 1024],
            one_key: false,
            answer: Answer::ServedWhole,
        },
        Row {
            case: "two write units, budget spent, two 100 KB writes",
            billing: provisioned(2),
            spend_budget: true,
            batch: vec![100 * 1024; 2],
            one_key: false,
            answer: Answer::ServedPartly,
        },
        Row {
            case: "fifty write units, nothing spent, a full batch of small writes",
            billing: provisioned(50),
            spend_budget: false,
            batch: vec![900; 25],
            one_key: false,
            answer: Answer::ServedWhole,
        },
        Row {
            case: "one write unit, budget spent, a single write past the whole burst",
            billing: provisioned(1),
            spend_budget: true,
            batch: vec![399 * 1024],
            one_key: false,
            answer: Answer::Refused,
        },
        Row {
            case: "one write unit, budget spent, two writes past the whole burst",
            billing: provisioned(1),
            spend_budget: true,
            batch: vec![399 * 1024; 2],
            one_key: false,
            answer: Answer::Refused,
        },
        Row {
            case: "one write unit, budget spent, one write past the burst beside one inside it",
            billing: provisioned(1),
            spend_budget: true,
            batch: vec![399 * 1024, 900],
            one_key: false,
            answer: Answer::ServedPartly,
        },
        Row {
            // The per-partition-key ceiling lends ONE call what one call can ask of a
            // partition (25 operations of up to 400 KB), so a batch of eight ~400-unit
            // writes aimed at one key is inside what it serves and lands whole; what the
            // ceiling bounds is the SUSTAINED rate, which the drain suite below walks.
            case: "capacity to spare, eight 400 KB writes at one partition key",
            billing: provisioned(DRAIN_WRITE_UNITS),
            spend_budget: false,
            batch: vec![399 * 1024; 8],
            one_key: true,
            answer: Answer::ServedWhole,
        },
    ]
}

#[tokio::test]
async fn a_batch_hands_back_what_capacity_does_not_cover_and_refuses_only_the_impossible() {
    let router = crate::emulator::emulator();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let table = format!("Row{index}");
        table_with(&router, &table, row.billing.clone()).await;

        // The budget is emptied first where the row asks for it, at keys of its own so the
        // measured batch meets a fresh per-key ceiling.
        if row.spend_budget {
            spend_write_budget(&router, case, &table).await;
        }
        let primed: BTreeSet<Key> = stored_keys(&router, &table).await.into_iter().collect();

        let requested: Vec<Key> = (0..row.batch.len())
            .map(|slot| {
                let partition = if row.one_key {
                    "hot".to_string()
                } else {
                    format!("row{index}-{slot}")
                };
                (partition, slot as i64)
            })
            .collect();
        let requests: Vec<Value> = requested
            .iter()
            .zip(&row.batch)
            .map(|(key, payload)| put_of(key, *payload))
            .collect();
        let (status, body, elapsed) = batch_write(&router, &table, requests).await;
        assert!(
            elapsed < CALL_DEADLINE,
            "case {case:?}: the call must be answered promptly whether or not capacity \
             covered it, it took {elapsed:?}"
        );

        if row.answer == Answer::Refused {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ProvisionedThroughputExceededException"),
                "case {case:?}: a batch no retry could ever get through is refused, got \
                 {status} with {body}"
            );
            assert_eq!(
                message(&body),
                THROUGHPUT_EXCEEDED,
                "case {case:?}: the refusal carries the provisioning sentence, got {body}"
            );
            assert_eq!(
                body["ThrottlingReasons"],
                json!([{"reason": "TableWriteProvisionedThroughputExceeded",
                        "resource":
                            format!("arn:aws:dynamodb:{REGION}:000000000000:table/{table}")}]),
                "case {case:?}: the refusal names the ceiling that bound and the table's \
                 ARN, got {body}"
            );
            let after: BTreeSet<Key> = stored_keys(&router, &table).await.into_iter().collect();
            assert_eq!(
                after, primed,
                "case {case:?}: a refused batch stores nothing, it left {after:?} where \
                 {primed:?} stood"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: a batch whose residue a retry could drain is served, got {status} \
             with {body}"
        );
        let reported: BTreeSet<Key> = handed_back(&body, &table).into_iter().collect();
        let expected_residue = match row.answer {
            Answer::ServedWhole => 0..1,
            Answer::ServedPartly => 1..row.batch.len(),
            Answer::ServedOne => row.batch.len() - 1..row.batch.len(),
            Answer::Refused => unreachable!("the refusal arm returned above"),
        };
        assert!(
            expected_residue.contains(&reported.len()),
            "case {case:?}: expected {expected_residue:?} of the {} requests handed back, got \
             {} ({reported:?})",
            row.batch.len(),
            reported.len()
        );
        // The landed keys and the reported keys PARTITION the requested keys: every one of
        // them is either stored or handed back, and none is both.
        let stored: BTreeSet<Key> = stored_keys(&router, &table).await.into_iter().collect();
        let landed: BTreeSet<Key> = stored.difference(&primed).cloned().collect();
        let requested: BTreeSet<Key> = requested.into_iter().collect();
        assert!(
            landed.is_disjoint(&reported),
            "case {case:?}: no key is both stored and handed back, {landed:?} and {reported:?} \
             overlap"
        );
        let accounted: BTreeSet<Key> = landed.union(&reported).cloned().collect();
        assert_eq!(
            accounted, requested,
            "case {case:?}: the landed keys and the reported keys must partition the \
             requested keys, got {accounted:?} for {requested:?}"
        );

        // The residue is resent as it was handed back, against capacity that covers it:
        // "you can provide this value directly to a subsequent BatchWriteItem operation".
        if reported.is_empty() {
            continue;
        }
        give_capacity(&router, &table).await;
        let still_held = drain(&router, case, &table, residue_of(&body, &table)).await;
        assert!(
            still_held.is_empty(),
            "case {case:?}: the residue drains against capacity that covers it, {} requests \
             are still held back",
            still_held.len()
        );
        let stored: BTreeSet<Key> = stored_keys(&router, &table).await.into_iter().collect();
        let landed: BTreeSet<Key> = stored.difference(&primed).cloned().collect();
        assert_eq!(
            landed, requested,
            "case {case:?}: every requested key is stored once the residue has drained, got \
             {landed:?} for {requested:?}"
        );
    }
}

// ===========================================================================
// 2. The documented drain loop over a stream of batches
// ===========================================================================

#[tokio::test]
async fn a_stream_of_batches_against_one_write_unit_drains_without_losing_a_key() {
    let router = crate::emulator::emulator();
    let case = "a stream of batches against one write unit";
    table_with(
        &router,
        "Stream",
        json!({"BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 2000, "WriteCapacityUnits": 1}}),
    )
    .await;

    let total = 1000usize;
    let requested: Vec<Key> = (0..total).map(|slot| (format!("k{slot}"), 0)).collect();
    let mut residue: Vec<Value> = Vec::new();
    let mut reported: BTreeSet<Key> = BTreeSet::new();
    let started = std::time::Instant::now();
    for chunk in requested.chunks(25) {
        let requests: Vec<Value> = chunk.iter().map(|key| put_of(key, 900)).collect();
        let (status, body, elapsed) = batch_write(&router, "Stream", requests).await;
        assert_eq!(
            (status, elapsed < CALL_DEADLINE),
            (200, true),
            "case {case:?}: every call of the stream is answered promptly with the residue \
             it held back, got {status} after {elapsed:?} with {body}"
        );
        reported.extend(handed_back(&body, "Stream"));
        residue.extend(residue_of(&body, "Stream"));
    }
    let stream_elapsed = started.elapsed();
    assert!(
        stream_elapsed < CALL_DEADLINE,
        "case {case:?}: the whole stream of {} batches answers inside one call's bound, it \
         took {stream_elapsed:?}",
        total / 25
    );

    let requested: BTreeSet<Key> = requested.into_iter().collect();
    let landed: BTreeSet<Key> = stored_keys(&router, "Stream").await.into_iter().collect();
    assert!(
        landed.len() < total,
        "case {case:?}: a table provisioned for one write unit cannot absorb all {total} \
         writes, {} landed",
        landed.len()
    );
    assert!(
        !reported.is_empty(),
        "case {case:?}: the writes it did not absorb are reported back, none were"
    );
    assert!(
        landed.is_disjoint(&reported),
        "case {case:?}: no key is both stored and reported unprocessed"
    );
    let accounted: BTreeSet<Key> = landed.union(&reported).cloned().collect();
    assert_eq!(
        accounted, requested,
        "case {case:?}: the landed keys and the reported keys must partition the {total} \
         requested keys, {} landed and {} were reported",
        landed.len(),
        reported.len()
    );

    // The residue drains against capacity that covers it, and each key lands exactly once:
    // the store holds every requested key and nothing else.
    give_capacity(&router, "Stream").await;
    let still_held = drain(&router, case, "Stream", residue).await;
    assert!(
        still_held.is_empty(),
        "case {case:?}: the drain loop must terminate, {} requests are still held back",
        still_held.len()
    );
    let drained: Vec<Key> = stored_keys(&router, "Stream").await;
    let unique: BTreeSet<Key> = drained.iter().cloned().collect();
    assert_eq!(
        (drained.len(), unique == requested),
        (total, true),
        "case {case:?}: every requested key is stored exactly once after the drain, the store \
         holds {} keys",
        drained.len()
    );
}

// ===========================================================================
// 3. The progress a batch makes is bounded by the table's provisioning
// ===========================================================================

#[tokio::test]
async fn one_request_batches_run_the_progress_allowance_out_and_are_then_refused() {
    let router = crate::emulator::emulator();
    let case = "one-request batches against one write unit";
    table_with(
        &router,
        "Bounded",
        json!({"BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": 2000, "WriteCapacityUnits": 1}}),
    )
    .await;
    // ~101 units per write against a table that can hold 300 and refills one per second:
    // what a batch may borrow against that refill is finite, so a caller writing one
    // request per call cannot keep making progress past what the table is provisioned for.
    let payload = 100 * 1024;
    let mut landed = 0usize;
    let mut refused: Option<Value> = None;
    for attempt in 0..40i64 {
        let (status, body, elapsed) = batch_write(
            &router,
            "Bounded",
            vec![put_of(&(format!("b{attempt}"), attempt), payload)],
        )
        .await;
        assert!(
            elapsed < CALL_DEADLINE,
            "case {case:?}, attempt {attempt}: every call answers promptly, it took {elapsed:?}"
        );
        if status == 200 {
            if handed_back(&body, "Bounded").is_empty() {
                landed += 1;
            }
            continue;
        }
        refused = Some(body);
        break;
    }
    let refusal = refused.unwrap_or_else(|| {
        panic!(
            "case {case:?}: the progress a batch makes is bounded, so a caller writing past \
             the table's provisioning must be refused; {landed} writes landed instead"
        )
    });
    assert_eq!(
        (error_code(&refusal).as_str(), message(&refusal).as_str()),
        (
            "ProvisionedThroughputExceededException",
            THROUGHPUT_EXCEEDED
        ),
        "case {case:?}: the refusal is the modelled throttle with its fixed sentence, got \
         {refusal}"
    );
    // What got through is bounded by the capacity the table holds plus what one call may
    // borrow against its refill — far less than the 40 writes the caller asked for.
    assert!(
        landed < 20,
        "case {case:?}: a one-unit table admits what it is provisioned for plus a bounded \
         burst, {landed} writes of ~101 units landed"
    );
    let stored = stored_keys(&router, "Bounded").await.len();
    assert_eq!(
        stored, landed,
        "case {case:?}: every write that was not handed back or refused is stored, the table \
         holds {stored} items for {landed} served calls"
    );
}

// ===========================================================================
// 4. The same rule for the OTHER request form a batch carries
// ===========================================================================

#[tokio::test]
async fn a_batch_of_deletes_hands_back_what_capacity_does_not_cover_as_delete_requests() {
    let router = crate::emulator::emulator();
    let case = "a batch of deletes against a spent budget";
    // The items to be removed are seeded against capacity that covers them — each ~8 KB item
    // costs nine units to store and the same to remove — and the table is then held to one
    // write unit, with the credit behind it spent, so the deletes meet an empty budget.
    table_with(
        &router,
        "Deletes",
        json!({"BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": DRAIN_READ_UNITS,
                                         "WriteCapacityUnits": DRAIN_WRITE_UNITS}}),
    )
    .await;
    let doomed: Vec<Key> = (0..25).map(|slot| (format!("d{slot}"), slot)).collect();
    let (status, body, _) = batch_write(
        &router,
        "Deletes",
        doomed.iter().map(|key| put_of(key, 8 * 1024)).collect(),
    )
    .await;
    assert_eq!(
        (status, handed_back(&body, "Deletes").len()),
        (200, 0),
        "case {case:?}: the items to be removed are stored inside the table's credit, got \
         {status} with {} handed back",
        handed_back(&body, "Deletes").len()
    );
    ok(
        &router,
        "UpdateTable",
        json!({"TableName": "Deletes", "BillingMode": "PROVISIONED",
               "ProvisionedThroughput": {"ReadCapacityUnits": DRAIN_READ_UNITS,
                                         "WriteCapacityUnits": 1}}),
    )
    .await;
    // The lowered capacity is in force once the table is ACTIVE again: "while it's UPDATING,
    // you can't issue another UpdateTable request", and this suite raises the capacity again
    // when it drains the residue.
    describe_when_active(&router, case, "Deletes").await;
    spend_write_budget(&router, case, "Deletes").await;

    let requests: Vec<Value> = doomed
        .iter()
        .map(|key| json!({"DeleteRequest": {"Key": {"pk": {"S": key.0},
                                                    "sk": {"N": key.1.to_string()}}}}))
        .collect();
    let (status, body, elapsed) = batch_write(&router, "Deletes", requests).await;
    assert_eq!(
        (status, elapsed < CALL_DEADLINE),
        (200, true),
        "case {case:?}: a delete the budget is short for is handed back rather than refused, \
         got {status} after {elapsed:?}"
    );
    let residue = residue_of(&body, "Deletes");
    let held: BTreeSet<Key> = residue
        .iter()
        .map(|request| {
            assert!(
                request.get("DeleteRequest").is_some() && request.get("PutRequest").is_none(),
                "case {case:?}: a handed-back delete keeps the form it was sent in, got \
                 {request}"
            );
            key_of(&request["DeleteRequest"]["Key"])
        })
        .collect();
    assert_eq!(
        held.len(),
        doomed.len() - 1,
        "case {case:?}: the call removes the one item its progress covers and hands the rest \
         back, it handed back {} of {}",
        held.len(),
        doomed.len()
    );
    // Every delete handed back left its item in place, and the one that was not handed back
    // removed it.
    let stored: BTreeSet<Key> = stored_keys(&router, "Deletes").await.into_iter().collect();
    for key in &doomed {
        assert_eq!(
            stored.contains(key),
            held.contains(key),
            "case {case:?}, key {key:?}: an item is still there exactly when its delete was \
             handed back"
        );
    }

    // The residue of deletes drains against capacity that covers it, and then none of the
    // items it names is left.
    give_capacity(&router, "Deletes").await;
    let still_held = drain(&router, case, "Deletes", residue).await;
    assert!(
        still_held.is_empty(),
        "case {case:?}: the drain loop must terminate, {} deletes are still held back",
        still_held.len()
    );
    let stored: BTreeSet<Key> = stored_keys(&router, "Deletes").await.into_iter().collect();
    for key in &doomed {
        assert!(
            !stored.contains(key),
            "case {case:?}, key {key:?}: every delete of the residue has landed"
        );
    }
}
