// SPDX-License-Identifier: CC0-1.0

//! The RESPONSE BUDGET of `BatchGetItem`: what one call may carry, and what it defers.
//!
//! The rule: a batched read answers 200 with as many WHOLE items as fit its documented
//! response ceiling and hands every key it did not read back under `UnprocessedKeys` in
//! the form `RequestItems` carried them, so re-submitting that map drains the request to
//! exactly the set of keys it asked for, each served once. "A single operation can
//! retrieve up to 16 MB of data, which can contain as many as 100 items. BatchGetItem
//! returns a partial result if the response size limit is exceeded [...] If a partial
//! result is returned, the operation returns a value for UnprocessedKeys", and the
//! deferral is about the RESPONSE and nothing else: the items behind the deferred keys
//! are untouched in storage, and no item is ever truncated to fit.
//!
//! The ceiling itself is the one the store publishes
//! ([`MAX_BATCH_GET_RESPONSE_BYTES`]), so the rows below bound the item count they
//! expect by arithmetic over it rather than by a figure written out here.
//!
//! Driven as a TABLE spanning what the rule generalises over: the per-item size, the
//! number of keys (up to the documented 100), the consistency each entry asked for,
//! whether the batch addresses one table or several, and whether a projection shrinks
//! what the response carries.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_BATCH_GET_RESPONSE_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{emulator, ok};
use crate::table_items::create_active_table;

/// The most an item's attribute NAMES and its key value can add to the padding this
/// suite sizes its items by: the stored size of an item is every attribute's name plus
/// its value ("Item sizes and formats" in the Amazon DynamoDB Developer Guide), and the
/// two names and the short key string below are far inside this. It only widens the
/// interval the expected item count is asserted in.
const NAME_OVERHEAD: usize = 64;

/// Store `count` items of about `pad` bytes each under keys `k0..k<count>`, and answer
/// the keys in the order they were written.
async fn seed_items(
    router: &MultiServiceRouter,
    table: &str,
    count: usize,
    pad: usize,
) -> Vec<Value> {
    let mut keys = Vec::with_capacity(count);
    for index in 0..count {
        let key = format!("k{index}");
        ok(
            router,
            "PutItem",
            json!({"TableName": table, "Item": {
                "pk": {"S": key},
                "pad": {"S": "x".repeat(pad)},
            }}),
        )
        .await;
        keys.push(json!({"pk": {"S": key}}));
    }
    keys
}

/// The `pk` of every item in one `Responses` entry.
fn returned_keys(answered: &Value, table: &str) -> Vec<String> {
    answered["Responses"][table]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["pk"]["S"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `pk` of every key one `UnprocessedKeys` entry defers.
fn deferred_keys(answered: &Value, table: &str) -> Vec<String> {
    answered["UnprocessedKeys"][table]["Keys"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|key| key["pk"]["S"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// How many items of `pad` bytes a response may carry: the published ceiling over the
/// item size, as an interval, because the stored size of an item is its padding plus its
/// attribute names ([`NAME_OVERHEAD`]).
fn fitting_items(pad: usize) -> (usize, usize) {
    let budget = usize::try_from(MAX_BATCH_GET_RESPONSE_BYTES).expect("the ceiling is positive");
    (budget / (pad + NAME_OVERHEAD), budget / pad)
}

/// Re-submit `UnprocessedKeys` until it is empty, answering every key the whole drain
/// reported, in order. Bounded by the number of keys it may still have to read, so a
/// drain that never shrinks fails naming the response it stalled on instead of looping.
async fn drain(router: &MultiServiceRouter, case: &str, mut request: Value) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for _ in 0..=128 {
        let answered = ok(router, "BatchGetItem", request.clone()).await;
        for items in answered["Responses"]
            .as_object()
            .cloned()
            .unwrap_or_default()
            .values()
        {
            for item in items.as_array().cloned().unwrap_or_default() {
                seen.push(item["pk"]["S"].as_str().unwrap_or_default().to_string());
            }
        }
        let residue = answered["UnprocessedKeys"].clone();
        if residue.as_object().is_none_or(serde_json::Map::is_empty) {
            return seen;
        }
        request = json!({"RequestItems": residue});
    }
    panic!("case {case:?}: the drain loop never emptied UnprocessedKeys")
}

/// One row of the size table: how many items of what size one batch asks for, and
/// whether that batch must come back partial.
struct Row {
    case: &'static str,
    table: &'static str,
    items: usize,
    pad: usize,
    consistent: bool,
    partial: bool,
}

#[tokio::test]
async fn a_batch_stops_at_the_response_ceiling_and_defers_the_rest() {
    // The rows differ in the per-item size and the key count — the two terms the
    // ceiling is measured against — and in the consistency the entry asked for, which
    // changes what the read COSTS and not what the response may carry. The last two
    // rows are inside the ceiling and must be answered whole, including one that asks
    // for the documented maximum of 100 keys.
    let rows = [
        Row {
            case: "60 keys of about 380 KB each",
            table: "Big",
            items: 60,
            pad: 389_120,
            consistent: true,
            partial: true,
        },
        Row {
            case: "100 keys of about 190 KB each, eventually consistent",
            table: "Many",
            items: 100,
            pad: 194_560,
            consistent: false,
            partial: true,
        },
        Row {
            case: "10 keys of about 380 KB each",
            table: "Few",
            items: 10,
            pad: 389_120,
            consistent: true,
            partial: false,
        },
        Row {
            case: "100 keys of about 10 KB each",
            table: "Small",
            items: 100,
            pad: 10_240,
            consistent: false,
            partial: false,
        },
    ];

    for row in rows {
        let router = emulator();
        create_active_table(&router, row.table, &[("pk", "HASH", "S")]).await;
        let keys = seed_items(&router, row.table, row.items, row.pad).await;
        let request = json!({"RequestItems": {row.table: {
            "Keys": keys.clone(),
            "ConsistentRead": row.consistent,
        }}});

        let answered = ok(&router, "BatchGetItem", request.clone()).await;
        let returned = returned_keys(&answered, row.table);
        let deferred = deferred_keys(&answered, row.table);
        assert_eq!(
            returned.len() + deferred.len(),
            row.items,
            "case {:?}: every key is either answered or deferred, got {} answered and {} \
             deferred",
            row.case,
            returned.len(),
            deferred.len()
        );

        if !row.partial {
            assert!(
                deferred.is_empty(),
                "case {:?}: a batch inside the ceiling defers nothing, got {deferred:?}",
                row.case
            );
            continue;
        }

        // The response stopped AT the ceiling: as many whole items as fit it, and not
        // one more.
        let (lowest, highest) = fitting_items(row.pad);
        assert!(
            (lowest..=highest).contains(&returned.len()),
            "case {:?}: expected between {lowest} and {highest} items in the response, got {}",
            row.case,
            returned.len()
        );
        assert!(
            !deferred.is_empty(),
            "case {:?}: a batch past the ceiling names the keys it did not read, got {answered}",
            row.case
        );

        // The two sets are disjoint and cover the request exactly once.
        let answered_set: BTreeSet<&String> = returned.iter().collect();
        let deferred_set: BTreeSet<&String> = deferred.iter().collect();
        assert!(
            answered_set.is_disjoint(&deferred_set),
            "case {:?}: a key is answered or deferred, never both: {returned:?} / {deferred:?}",
            row.case
        );
        assert_eq!(
            answered_set.len() + deferred_set.len(),
            row.items,
            "case {:?}: the two sets name each key once, got {returned:?} / {deferred:?}",
            row.case
        );

        // No item was truncated to fit: every item the response carried holds its whole
        // padding.
        let items = answered["Responses"][row.table]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for item in &items {
            let carried = item["pad"]["S"].as_str().unwrap_or_default().len();
            assert_eq!(
                carried, row.pad,
                "case {:?}: an answered item is whole, got {carried} bytes of padding for {}",
                row.case, item["pk"]["S"]
            );
        }

        // "The value can be provided directly to a subsequent BatchGetItem operation":
        // the deferred entry carries the read options it was sent with, so the residue
        // is a request.
        assert_eq!(
            answered["UnprocessedKeys"][row.table]["ConsistentRead"],
            json!(row.consistent),
            "case {:?}: the residue carries the entry's own ConsistentRead, got {answered}",
            row.case
        );

        // A deferred key is still in storage, whole: the deferral is about the response
        // budget and not about what the table holds.
        for key in &deferred {
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": row.table, "Key": {"pk": {"S": key}},
                       "ConsistentRead": true}),
            )
            .await;
            assert_eq!(
                read["Item"]["pad"]["S"].as_str().unwrap_or_default().len(),
                row.pad,
                "case {:?}: the deferred key {key:?} still reads its whole item, got {}",
                row.case,
                read["Item"]["pk"]
            );
        }

        // The drain loop terminates and yields exactly the keys the request asked for,
        // each once.
        let drained = drain(&router, row.case, request).await;
        let mut sorted = drained.clone();
        sorted.sort();
        let mut expected: Vec<String> = (0..row.items).map(|index| format!("k{index}")).collect();
        expected.sort();
        assert_eq!(
            sorted, expected,
            "case {:?}: the drain reports each requested key exactly once, got {drained:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_budget_is_what_the_response_carries_not_what_the_store_holds() {
    let router = emulator();
    create_active_table(&router, "Projected", &[("pk", "HASH", "S")]).await;
    let keys = seed_items(&router, "Projected", 60, 389_120).await;

    // The same 60 keys whose whole items cannot fit one response: projected down to the
    // key alone, what the response CARRIES is a few hundred bytes, so the batch is
    // answered whole and defers nothing.
    let answered = ok(
        &router,
        "BatchGetItem",
        json!({"RequestItems": {"Projected": {
            "Keys": keys.clone(),
            "ConsistentRead": true,
            "ProjectionExpression": "pk",
        }}}),
    )
    .await;
    assert_eq!(
        returned_keys(&answered, "Projected").len(),
        60,
        "case a projection to the key alone: every key fits one response, got {answered}"
    );
    assert!(
        answered["UnprocessedKeys"]
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "case a projection to the key alone: nothing is deferred, got {answered}"
    );

    // Without the projection the same request is partial, so the two answers differ only
    // in what the response would have had to carry.
    let whole = ok(
        &router,
        "BatchGetItem",
        json!({"RequestItems": {"Projected": {"Keys": keys, "ConsistentRead": true}}}),
    )
    .await;
    assert!(
        !deferred_keys(&whole, "Projected").is_empty(),
        "case the same keys unprojected: the response is partial, got {whole}"
    );
}

#[tokio::test]
async fn the_ceiling_is_on_the_whole_response_across_every_table() {
    let router = emulator();
    create_active_table(&router, "First", &[("pk", "HASH", "S")]).await;
    create_active_table(&router, "Second", &[("pk", "HASH", "S")]).await;
    let first = seed_items(&router, "First", 30, 389_120).await;
    let second = seed_items(&router, "Second", 30, 389_120).await;

    // Sixty items of 380 KB across TWO tables are the same 22 MB one table's worth is:
    // the ceiling is on the response, so the batch is partial and the residue names the
    // keys it did not read, whichever table they sat in.
    let request = json!({"RequestItems": {
        "First": {"Keys": first, "ConsistentRead": true},
        "Second": {"Keys": second, "ConsistentRead": true},
    }});
    let answered = ok(&router, "BatchGetItem", request.clone()).await;
    let returned =
        returned_keys(&answered, "First").len() + returned_keys(&answered, "Second").len();
    let deferred =
        deferred_keys(&answered, "First").len() + deferred_keys(&answered, "Second").len();
    let (lowest, highest) = fitting_items(389_120);
    assert_eq!(
        returned + deferred,
        60,
        "case two tables of 30 large items: every key is answered or deferred, got {returned} \
         answered and {deferred} deferred"
    );
    assert!(
        (lowest..=highest).contains(&returned),
        "case two tables of 30 large items: expected between {lowest} and {highest} items in \
         the response, got {returned}"
    );

    // The drain reports each of the sixty keys once — thirty per table, so the two
    // tables' identical key names are counted per table rather than mixed.
    let mut per_table: Vec<(String, &str)> = Vec::new();
    let mut request = request;
    let mut rounds = 0;
    for _ in 0..=128 {
        rounds += 1;
        let answered = ok(&router, "BatchGetItem", request.clone()).await;
        for table in ["First", "Second"] {
            for key in returned_keys(&answered, table) {
                per_table.push((key, table));
            }
        }
        let residue = answered["UnprocessedKeys"].clone();
        if residue.as_object().is_none_or(serde_json::Map::is_empty) {
            break;
        }
        request = json!({"RequestItems": residue});
    }
    assert!(
        rounds > 1,
        "case two tables of 30 large items: the drain took more than one call, got {rounds}"
    );
    assert_eq!(
        per_table.len(),
        60,
        "case two tables of 30 large items: the drain reports sixty items in all, got \
         {per_table:?}"
    );
    per_table.sort();
    per_table.dedup();
    assert_eq!(
        per_table.len(),
        60,
        "case two tables of 30 large items: each key of each table is reported once, got \
         {per_table:?}"
    );
}
