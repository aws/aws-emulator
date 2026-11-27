// SPDX-License-Identifier: CC0-1.0

//! The ORDER an unfiltered `Scan` answers its items in, and what that order does NOT follow.
//!
//! `Scan` answers an `ItemList` the model declares no ordering for
//! (`smithy_query(query="operation", operation="Scan")`), but the order one table is walked
//! in is observable: a client that reads `Items[0]`, or that compares a page of an unfiltered
//! scan with the page it read before, sees it. What that order IS is the layout of the KEY
//! SPACE — "DynamoDB uses the partition key's value as input to an internal hash function.
//! The output value from the hash function determines the partition" (the Developer Guide's
//! "Partitions and data distribution") — so the rule a client can hold the walk to, which is
//! what this suite reads:
//!
//! 1. the order is a function of the KEY VALUES the table holds and of nothing else, so the
//!    same keys are walked in the same order every time, whatever the table's write history
//!    is — items rewritten, deleted and written again, other partitions added and removed —
//!    and whatever order they were written in;
//! 2. it is a PLACEMENT order and not a value order: the walk is neither the ascending nor
//!    the descending order of the key values it holds, for a key of any type;
//! 3. it loses and repeats nothing: one page of it carries every item exactly once, and
//!    paging it with any `Limit` carries along that same order
//!    ("the primary key of the item where the operation stopped", `Scan$LastEvaluatedKey`);
//! 4. the rows of ONE partition sit together in it, in the order the store holds them — the
//!    sort key's own — and a walk of a secondary index is laid out by that index's key the
//!    same way.
//!
//! WHERE a given key lands is the service's own internal placement, which the model does not
//! state and which no caller can derive; this suite therefore asserts no key's position, only
//! the rule above. The rows span what the rule generalises over: the attribute type the
//! partition key is declared with ([`WALKS`]), the page size the walk is read with, the write
//! history behind it, and whether the walk is the table's own key space or a secondary
//! index's.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! doors use, from `crate::emulator`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{describe_when_active, emulator, ok};
// The typed value a row's key is written as, and the reader of the partition keys a page
// reports in the order it reported them, are the ones the suite beside this one states for
// this test binary.
use crate::scan_segment_key_types_and_fan_out::{partition_keys, typed};

/// One seeded table of one item per partition key: the attribute type the key is declared
/// with, the key values the table holds, and whether those values have an order of their OWN
/// that rule 2 can be read against.
struct Walk {
    case: &'static str,
    table: &'static str,
    /// The member a value of that type is carried in on the wire (`AttributeValue`).
    wire: &'static str,
    keys: &'static [&'static str],
    /// Whether this row's key values are compared by value in [`value_order`] — a number
    /// numerically and a string by its own bytes. A binary key's wire form is base64 text,
    /// whose order is not the order of the bytes it stands for, so that row is read for
    /// rules 1 and 3 and not for rule 2.
    value_ordered: bool,
}

/// The tables the walk is read over: one per type a key attribute may be declared with
/// ("S - the attribute is of type String. N - the attribute is of type Number. B - the
/// attribute is of type Binary", `AttributeDefinition$AttributeType`), each holding enough
/// distinct keys that a walk agreeing with their own order would not be chance.
const WALKS: [Walk; 3] = [
    Walk {
        case: "ten string partition keys",
        table: "WalkOrderString",
        wire: "S",
        keys: &[
            "ada", "boole", "cantor", "dedekind", "euler", "fermat", "gauss", "hilbert",
            "jacobi", "kepler",
        ],
        value_ordered: true,
    },
    Walk {
        case: "ten number partition keys",
        table: "WalkOrderNumber",
        wire: "N",
        keys: &[
            "-5", "0.5", "7", "13", "64", "128", "900", "1024", "20000", "123456789",
        ],
        value_ordered: true,
    },
    Walk {
        case: "eight binary partition keys",
        table: "WalkOrderBinary",
        wire: "B",
        keys: &[
            "AA==", "AAAA", "AQ==", "AQID", "Ag==", "ZA==", "ZGF0YQ==", "/w==",
        ],
        value_ordered: false,
    },
];

/// Create one table per [`WALKS`] row and seed one item under each of its keys, in the order
/// the row lists them. The creates are issued together and awaited afterwards, so the suite
/// waits out ONE clock-owed `CREATING` -> `ACTIVE` span rather than one per table.
///
/// `suffix` names a second copy of the same tables, and `reversed` writes their items in the
/// opposite order, which is what rule 1 is read over.
async fn seed_walks(router: &MultiServiceRouter, suffix: &str, reversed: bool) {
    for walk in &WALKS {
        ok(
            router,
            "CreateTable",
            json!({
                "TableName": format!("{}{suffix}", walk.table),
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": walk.wire}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
        )
        .await;
    }
    for walk in &WALKS {
        let table = format!("{}{suffix}", walk.table);
        describe_when_active(router, walk.case, &table).await;
        let mut keys: Vec<&str> = walk.keys.to_vec();
        if reversed {
            keys.reverse();
        }
        for key in keys {
            put_key(router, &table, walk.wire, key, "seeded").await;
        }
    }
}

/// Write one item keyed on `key`, carrying `probe` as its only other attribute — what an
/// overwrite changes without touching the key.
async fn put_key(router: &MultiServiceRouter, table: &str, wire: &str, key: &str, probe: &str) {
    let mut item = json!({"probe": {"S": probe}});
    item["pk"] = typed(wire, key);
    ok(router, "PutItem", json!({"TableName": table, "Item": item})).await;
}

/// The order one unfiltered `Scan` of `table` answers its partition keys in.
async fn walk_order(router: &MultiServiceRouter, table: &str, wire: &str) -> Vec<String> {
    let page = ok(router, "Scan", json!({"TableName": table})).await;
    partition_keys(&page, wire)
}

/// The seeded keys of one row in ascending order of the VALUE each holds: numerically for a
/// number key — "Numbers are sent across the network to DynamoDB as strings ... However,
/// DynamoDB treats them as number type attributes" (`AttributeValue$N`) — and by its own
/// bytes for a string key.
fn value_order(walk: &Walk) -> Vec<String> {
    let mut keys: Vec<String> = walk.keys.iter().map(|key| (*key).to_string()).collect();
    if walk.wire == "N" {
        keys.sort_by(|left, right| {
            left.parse::<f64>()
                .unwrap_or_default()
                .total_cmp(&right.parse::<f64>().unwrap_or_default())
        });
    } else {
        keys.sort();
    }
    keys
}

/// The same keys as a set, which is what a walk that loses and repeats nothing answers.
fn as_set(keys: &[String]) -> Vec<String> {
    let mut sorted = keys.to_vec();
    sorted.sort();
    sorted
}

/// The keys a row seeds, as that set.
fn seeded_set(walk: &Walk) -> Vec<String> {
    as_set(
        &walk
            .keys
            .iter()
            .map(|key| (*key).to_string())
            .collect::<Vec<String>>(),
    )
}

#[tokio::test]
async fn an_unfiltered_scan_answers_the_same_order_every_time() {
    let router = emulator();
    seed_walks(&router, "", false).await;

    for walk in &WALKS {
        let first = walk_order(&router, walk.table, walk.wire).await;
        assert_eq!(
            as_set(&first),
            seeded_set(walk),
            "case {:?}: one page of the walk carries every seeded key exactly once, got \
             {first:?}",
            walk.case
        );
        // Rows: the same walk asked for again, asked for strongly consistently, and asked
        // for with the capacity breakdown beside it — none of which is a fact about where a
        // partition sits.
        for (case, request) in [
            ("read again", json!({"TableName": walk.table})),
            (
                "read strongly consistently",
                json!({"TableName": walk.table, "ConsistentRead": true}),
            ),
            (
                "read with the capacity breakdown",
                json!({"TableName": walk.table, "ReturnConsumedCapacity": "INDEXES"}),
            ),
        ] {
            let page = ok(&router, "Scan", request).await;
            let again = partition_keys(&page, walk.wire);
            assert_eq!(
                again, first,
                "case {:?} {case}: the walk answers the same order every time, got {again:?} \
                 against {first:?}",
                walk.case
            );
            assert_eq!(
                (page["Count"].as_u64(), page["ScannedCount"].as_u64()),
                (Some(first.len() as u64), Some(first.len() as u64)),
                "case {:?} {case}: an unfiltered walk counts what it answered, got {page}",
                walk.case
            );
        }
    }
}

#[tokio::test]
async fn the_walk_order_survives_the_tables_write_history() {
    let router = emulator();
    seed_walks(&router, "", false).await;

    for walk in &WALKS {
        let baseline = walk_order(&router, walk.table, walk.wire).await;
        // Rewriting every item, and updating one of them, changes what the rows CARRY and
        // not which keys the table holds.
        for key in walk.keys {
            put_key(&router, walk.table, walk.wire, key, "rewritten").await;
        }
        let mut update = json!({
            "TableName": walk.table,
            "UpdateExpression": "SET probe = :probe",
            "ExpressionAttributeValues": {":probe": {"S": "updated"}},
        });
        update["Key"] = json!({"pk": typed(walk.wire, walk.keys[0])});
        ok(&router, "UpdateItem", update).await;
        let rewritten = walk_order(&router, walk.table, walk.wire).await;
        assert_eq!(
            rewritten, baseline,
            "case {:?}: rewriting the items does not move where their partitions sit, got \
             {rewritten:?} against {baseline:?}",
            walk.case
        );

        // Deleting a key and writing it again puts it back where its key value places it,
        // not at the end of the walk.
        let moved = walk.keys[walk.keys.len() / 2];
        let mut delete = json!({"TableName": walk.table});
        delete["Key"] = json!({"pk": typed(walk.wire, moved)});
        ok(&router, "DeleteItem", delete.clone()).await;
        let without = walk_order(&router, walk.table, walk.wire).await;
        assert_eq!(
            without,
            baseline
                .iter()
                .filter(|key| key.as_str() != moved)
                .cloned()
                .collect::<Vec<String>>(),
            "case {:?}: deleting {moved:?} leaves the other partitions where they were, got \
             {without:?} against {baseline:?}",
            walk.case
        );
        put_key(&router, walk.table, walk.wire, moved, "written again").await;
        let restored = walk_order(&router, walk.table, walk.wire).await;
        assert_eq!(
            restored, baseline,
            "case {:?}: {moved:?} written again sits where its key puts it, got {restored:?} \
             against {baseline:?}",
            walk.case
        );

        // A partition the table did not hold before is placed by its own key, and the
        // partitions already there keep their order both while it is held and after it goes.
        let extra = match walk.wire {
            "N" => "987654321",
            "B" => "AQIDBAU=",
            _ => "noether",
        };
        put_key(&router, walk.table, walk.wire, extra, "extra").await;
        let widened = walk_order(&router, walk.table, walk.wire).await;
        assert_eq!(
            widened
                .iter()
                .filter(|key| key.as_str() != extra)
                .cloned()
                .collect::<Vec<String>>(),
            baseline,
            "case {:?}: a new partition does not reorder the ones already held, got \
             {widened:?} against {baseline:?}",
            walk.case
        );
        assert!(
            widened.iter().any(|key| key.as_str() == extra),
            "case {:?}: the new partition {extra:?} is walked too, got {widened:?}",
            walk.case
        );
        let mut remove = json!({"TableName": walk.table});
        remove["Key"] = json!({"pk": typed(walk.wire, extra)});
        ok(&router, "DeleteItem", remove).await;
        let narrowed = walk_order(&router, walk.table, walk.wire).await;
        assert_eq!(
            narrowed, baseline,
            "case {:?}: the walk is back to the keys it holds, got {narrowed:?} against \
             {baseline:?}",
            walk.case
        );
    }
}

#[tokio::test]
async fn the_walk_order_follows_the_keys_and_not_the_order_they_were_written() {
    let router = emulator();
    seed_walks(&router, "", false).await;
    seed_walks(&router, "Reversed", true).await;

    for walk in &WALKS {
        let written_forwards = walk_order(&router, walk.table, walk.wire).await;
        let reversed_table = format!("{}Reversed", walk.table);
        let written_backwards = walk_order(&router, &reversed_table, walk.wire).await;
        assert_eq!(
            written_backwards, written_forwards,
            "case {:?}: the same keys are walked in the same order whichever order they \
             were written in, got {written_backwards:?} against {written_forwards:?}",
            walk.case
        );
    }
}

#[tokio::test]
async fn the_walk_order_is_not_the_order_the_key_values_sort_in() {
    let router = emulator();
    seed_walks(&router, "", false).await;

    for walk in WALKS.iter().filter(|walk| walk.value_ordered) {
        let walked = walk_order(&router, walk.table, walk.wire).await;
        let ascending = value_order(walk);
        let descending: Vec<String> = ascending.iter().rev().cloned().collect();
        assert_ne!(
            walked, ascending,
            "case {:?}: the walk is the key space's layout and not the ascending order of \
             the key values, got {walked:?}",
            walk.case
        );
        assert_ne!(
            walked, descending,
            "case {:?}: the walk is the key space's layout and not the descending order of \
             the key values, got {walked:?}",
            walk.case
        );
    }
}

#[tokio::test]
async fn paging_an_unfiltered_scan_carries_along_the_same_order() {
    let router = emulator();
    seed_walks(&router, "", false).await;

    for walk in &WALKS {
        let unpaged = walk_order(&router, walk.table, walk.wire).await;
        // Rows: the page size the caller asks for, from one key at a time up to more than
        // the table holds.
        for limit in [1usize, 3, 7, walk.keys.len() + 1] {
            let case = format!("{:?}, {limit} key(s) a page", walk.case);
            let mut paged: Vec<String> = Vec::new();
            let mut start: Option<Value> = None;
            for _ in 0..=walk.keys.len() {
                let mut request = json!({"TableName": walk.table, "Limit": limit});
                if let Some(key) = start.clone() {
                    request["ExclusiveStartKey"] = key;
                }
                let page = ok(&router, "Scan", request).await;
                let keys = partition_keys(&page, walk.wire);
                assert!(
                    keys.len() <= limit,
                    "case {case}: a page holds at most the Limit the request asked for, got \
                     {page}"
                );
                paged.extend(keys);
                match page.get("LastEvaluatedKey").filter(|key| !key.is_null()) {
                    Some(key) => start = Some(key.clone()),
                    None => break,
                }
            }
            assert_eq!(
                paged, unpaged,
                "case {case}: the pages carry along the walk's own order, got {paged:?} \
                 against {unpaged:?}"
            );
        }
    }
}

/// The table rule 4 is read over: three partitions of three rows each, with a global
/// secondary index whose own partition key two of those rows share.
const COMPOSITE: &str = "WalkOrderComposite";

/// That table's index, laid out by the index key the entries carry.
const COMPOSITE_INDEX: &str = "ByGroup";

/// Create [`COMPOSITE`] and seed its rows, written in an order that is neither the table's
/// key order nor the index's.
async fn seed_composite(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": COMPOSITE,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "N"},
                {"AttributeName": "gk", "AttributeType": "S"},
            ],
            "GlobalSecondaryIndexes": [{
                "IndexName": COMPOSITE_INDEX,
                "KeySchema": [{"AttributeName": "gk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, "three partitions of three rows", COMPOSITE).await;
    // Written sort key DESCENDING inside each partition, and the partitions themselves in
    // no particular order, so neither order below can be the order of the writes.
    for partition in ["w1", "w0", "w2"] {
        for row in [2, 1, 0] {
            ok(
                router,
                "PutItem",
                json!({
                    "TableName": COMPOSITE,
                    "Item": {
                        "pk": {"S": partition},
                        "sk": {"N": row.to_string()},
                        "gk": {"S": format!("g{}", row % 2)},
                    },
                }),
            )
            .await;
        }
    }
}

/// The `pk`/`sk` pair of each item a page reports, in the order it reported them.
fn rows_of(page: &Value) -> Vec<(String, String)> {
    page.get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["pk"]["S"].as_str().unwrap_or("<no pk>").to_string(),
                        item["sk"]["N"].as_str().unwrap_or("<no sk>").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether every run of equal `group` values in `rows` is one contiguous run, and the rows
/// inside each run ascend by the key they are ordered by.
fn grouped_and_ascending(rows: &[(String, String)]) -> bool {
    let mut seen: Vec<String> = Vec::new();
    let mut current: Option<&str> = None;
    for (group, _) in rows {
        if current != Some(group.as_str()) {
            if seen.iter().any(|earlier| earlier == group) {
                return false;
            }
            seen.push(group.clone());
            current = Some(group.as_str());
        }
    }
    rows.windows(2).all(|pair| {
        let ((left_group, left_row), (right_group, right_row)) = (&pair[0], &pair[1]);
        left_group != right_group
            || left_row.parse::<f64>().unwrap_or_default()
                <= right_row.parse::<f64>().unwrap_or_default()
    })
}

#[tokio::test]
async fn the_rows_of_one_partition_sit_together_in_the_walk() {
    let router = emulator();
    seed_composite(&router).await;

    let page = ok(&router, "Scan", json!({"TableName": COMPOSITE})).await;
    let rows = rows_of(&page);
    assert_eq!(
        rows.len(),
        9,
        "case \"the table's own key space\": the walk carries every row, got {rows:?}"
    );
    assert!(
        grouped_and_ascending(&rows),
        "case \"the table's own key space\": the rows of one partition sit together, in sort \
         key order, got {rows:?}"
    );
    let again = rows_of(&ok(&router, "Scan", json!({"TableName": COMPOSITE})).await);
    assert_eq!(
        again, rows,
        "case \"the table's own key space\": the walk answers the same order every time, got \
         {again:?} against {rows:?}"
    );

    // The index is a key space of its OWN: its entries are laid out by the index key, so the
    // entries sharing one index partition key sit together and the table's key breaks the
    // tie inside that run.
    let indexed = ok(
        &router,
        "Scan",
        json!({"TableName": COMPOSITE, "IndexName": COMPOSITE_INDEX}),
    )
    .await;
    let entries: Vec<(String, String)> = indexed
        .get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["gk"]["S"].as_str().unwrap_or("<no gk>").to_string(),
                        format!(
                            "{}/{}",
                            item["pk"]["S"].as_str().unwrap_or("<no pk>"),
                            item["sk"]["N"].as_str().unwrap_or("<no sk>")
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        9,
        "case \"the index's own key space\": the walk carries every entry, got {entries:?}"
    );
    let groups: Vec<&str> = entries.iter().map(|(group, _)| group.as_str()).collect();
    let mut runs: Vec<&str> = Vec::new();
    for group in &groups {
        if runs.last() != Some(group) {
            assert!(
                !runs.contains(group),
                "case \"the index's own key space\": the entries of {group:?} sit together, \
                 got {groups:?}"
            );
            runs.push(group);
        }
    }
    for run in &runs {
        let inside: Vec<&String> = entries
            .iter()
            .filter(|(group, _)| group == run)
            .map(|(_, key)| key)
            .collect();
        let mut ascending = inside.clone();
        ascending.sort();
        assert_eq!(
            inside, ascending,
            "case \"the index's own key space\": inside {run:?} the table's key breaks the \
             tie, got {inside:?}"
        );
    }

    // Paging the index walk carries along that same order, a resume token at a time.
    let mut paged: Vec<(String, String)> = Vec::new();
    let mut start: Option<Value> = None;
    for _ in 0..=entries.len() {
        let mut request = json!({
            "TableName": COMPOSITE,
            "IndexName": COMPOSITE_INDEX,
            "Limit": 2,
        });
        if let Some(key) = start.clone() {
            request["ExclusiveStartKey"] = key;
        }
        let page = ok(&router, "Scan", request).await;
        paged.extend(
            page.get("Items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            (
                                item["gk"]["S"].as_str().unwrap_or("<no gk>").to_string(),
                                format!(
                                    "{}/{}",
                                    item["pk"]["S"].as_str().unwrap_or("<no pk>"),
                                    item["sk"]["N"].as_str().unwrap_or("<no sk>")
                                ),
                            )
                        })
                        .collect::<Vec<(String, String)>>()
                })
                .unwrap_or_default(),
        );
        match page.get("LastEvaluatedKey").filter(|key| !key.is_null()) {
            Some(key) => start = Some(key.clone()),
            None => break,
        }
    }
    assert_eq!(
        paged, entries,
        "case \"the index's own key space, two entries a page\": the pages carry along the \
         index walk's order, got {paged:?} against {entries:?}"
    );
}
