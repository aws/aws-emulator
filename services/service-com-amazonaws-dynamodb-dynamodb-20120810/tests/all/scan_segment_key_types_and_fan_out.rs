// SPDX-License-Identifier: CC0-1.0

//! Which share of a table one parallel `Scan` segment reads, over every TYPE a partition
//! key can have and over a LARGE fan-out.
//!
//! The rule pinned here, one level above the suite beside it (`scan_segment_partitions`
//! reads it over string keys and small fan-outs): a parallel `Scan` divides the key space
//! it walks into one block of PARTITIONS per segment, so for ANY `TotalSegments` inside
//! the modelled range (`smithy_query` `shape ScanTotalSegments`, range 1..1000000) the
//! per-segment result sets are pairwise disjoint, their union in segment order is the
//! unsegmented scan, every item sharing a partition key is read by ONE segment, and
//! reading a segment again answers the same items. Which block a partition falls in
//! follows its key VALUE alone, so it does not follow the attribute TYPE that key has
//! (`S`, `N` or `B`) and — for a number — not the spelling the client sent either:
//! "Numbers are sent across the network to DynamoDB as strings, to maximize compatibility
//! across languages and libraries. However, DynamoDB treats them as number type
//! attributes" (`AttributeValue$N`), so one numeric value is one key, in one segment,
//! however it was written.
//!
//! WHERE a given key lands is the service's own internal placement, published nowhere;
//! this suite asserts no key's segment number, only the partitioning above — which is
//! what a parallel-scan client depends on.
//!
//! A segment's share is also what its RESUME TOKEN rule admits: "In a parallel scan, a
//! Scan request that includes ExclusiveStartKey must specify the same segment whose
//! previous Scan returned the corresponding value of LastEvaluatedKey"
//! (`Scan$ExclusiveStartKey`), so a segment admits a token for exactly the items it reads
//! and refuses every other. That agreement is how the rule is read at the TOP of the
//! modelled range, where sweeping every segment is not a request any client makes: the
//! segments of any division are contiguous blocks of one ordered key space, so a narrow
//! division's block locates the one segment of a million that reads a key, and the
//! agreement is read there in both directions. The wording of the refusal itself belongs to
//! `read_and_statement_refusal_variants`.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's
//! front doors use, from `crate::emulator`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, describe_when_active, emulator, error_code, ok};
// The partition component of an item key, and the reader of the keys a page reports, are
// the ones the suite beside this one states for this test binary — the reader takes the
// member the table's partition key is carried in, which is the one thing that differs here.
use crate::scan_segment_partitions::{item_keys, partition_of};

/// One seeded table: the attribute TYPE its partition key is declared with, and the key
/// values it holds — one item each.
struct Keyed {
    case: &'static str,
    table: &'static str,
    /// The member a value of that type is carried in on the wire (`AttributeValue`).
    wire: &'static str,
    keys: &'static [&'static str],
}

/// The tables the rule is read over: one per type a key attribute may be declared with
/// ("the data type for the attribute ... S - the attribute is of type String. N - the
/// attribute is of type Number. B - the attribute is of type Binary",
/// `AttributeDefinition$AttributeType`), with key values spanning each type's own range —
/// for `N` a zero, a negative fraction, a plain integer, an exponent form and the 38
/// significant digits a number may carry; for `B` a zero byte, several bytes, a high byte
/// and text.
const KEYED: [Keyed; 3] = [
    Keyed {
        case: "string partition keys",
        table: "SegTypedString",
        wire: "S",
        keys: &["alpha", "bravo", "charlie", "delta", "echo"],
    },
    Keyed {
        case: "number partition keys",
        table: "SegTypedNumber",
        wire: "N",
        keys: &[
            "0",
            "-17.5",
            "4711",
            "1.5e9",
            "99999999999999999999999999999999999999",
        ],
    },
    Keyed {
        case: "binary partition keys",
        table: "SegTypedBinary",
        wire: "B",
        keys: &["AA==", "AQID", "/w==", "ZGF0YQ=="],
    },
];

/// One attribute value of the type carried in `wire`, holding `text` — the one place this
/// suite spells a typed value, so a row's type is written down once.
///
/// Reachable from the sibling suites of this test binary, so a suite whose rows also span the
/// TYPE a key is declared with spells a typed value the same way
/// (`tests/all/scan_unfiltered_walk_order.rs`).
pub(crate) fn typed(wire: &str, text: &str) -> Value {
    let mut value = json!({});
    value[wire] = json!(text);
    value
}

/// Create `table` keyed on a partition key of attribute type `wire`, with a numeric sort
/// key when `sorted`, and wait out the clock-owed `CREATING` -> `ACTIVE` transition
/// through the harness's own bounded poll.
async fn create_keyed_table(router: &MultiServiceRouter, table: &str, wire: &str, sorted: bool) {
    let mut schema = vec![json!({"AttributeName": "pk", "KeyType": "HASH"})];
    let mut definitions = vec![json!({"AttributeName": "pk", "AttributeType": wire})];
    if sorted {
        schema.push(json!({"AttributeName": "sk", "KeyType": "RANGE"}));
        definitions.push(json!({"AttributeName": "sk", "AttributeType": "N"}));
    }
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": table,
            "KeySchema": schema,
            "AttributeDefinitions": definitions,
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, table, table).await;
}

/// Create every [`KEYED`] table and seed one item under each of its keys. The creates are
/// issued together and awaited afterwards, so the suite waits out ONE transition span
/// rather than one per table.
async fn seed_keyed(router: &MultiServiceRouter) {
    for row in &KEYED {
        ok(
            router,
            "CreateTable",
            json!({
                "TableName": row.table,
                "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": row.wire}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
        )
        .await;
    }
    for row in &KEYED {
        describe_when_active(router, row.case, row.table).await;
        for key in row.keys {
            let mut item = json!({"probe": {"S": *key}});
            item["pk"] = typed(row.wire, key);
            ok(
                router,
                "PutItem",
                json!({"TableName": row.table, "Item": item}),
            )
            .await;
        }
    }
}

/// One `Scan` of `segment` of `total` over `table`, which must be served.
async fn scan_segment(
    router: &MultiServiceRouter,
    table: &str,
    segment: usize,
    total: usize,
) -> Value {
    ok(
        router,
        "Scan",
        json!({"TableName": table, "Segment": segment, "TotalSegments": total}),
    )
    .await
}

/// The partition-key values a response reports, in the order it reported them, read off
/// the member a key of that type is carried in. A response with no `Items` reports none,
/// which is what a segment with nothing to read answers.
///
/// Reachable from the sibling suites of this test binary, beside the composite-key reader
/// `scan_segment_partitions::item_keys`, so the suite that reads the ORDER of a walk off a
/// page reads it through this one (`tests/all/scan_unfiltered_walk_order.rs`).
pub(crate) fn partition_keys(page: &Value, wire: &str) -> Vec<String> {
    page.get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item["pk"][wire].as_str().unwrap_or("<no pk>").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `segment` of `total` ADMITS `key` as a resume token: a segment resumes a token
/// that sits inside its own block of the key space and refuses one that does not, which is
/// how a worker learns a token is another segment's. A refusal must be the modelled
/// 400 `ValidationException`.
async fn admits_token(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    segment: usize,
    total: usize,
    key: &Value,
) -> bool {
    let request = json!({
        "TableName": table,
        "Segment": segment,
        "TotalSegments": total,
        "ExclusiveStartKey": key.clone(),
    });
    let (status, body) = call(router, "Scan", request).await;
    if status == 200 {
        return true;
    }
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case {case}: a resume token of another segment is refused as a 400 \
         ValidationException, got {status} with {body}"
    );
    false
}

#[tokio::test]
async fn a_partition_key_of_any_type_is_read_by_one_segment_of_any_fan_out() {
    let router = emulator();
    seed_keyed(&router).await;

    for row in &KEYED {
        let whole = partition_keys(
            &ok(&router, "Scan", json!({"TableName": row.table})).await,
            row.wire,
        );
        assert_eq!(
            whole.len(),
            row.keys.len(),
            "case {:?}: the unsegmented scan reads every item, got {whole:?}",
            row.case
        );
        // Rows: how many segments the scan divides the key space into — the sequential
        // scan, fewer segments than the table has partitions, as many, and far more, which
        // leaves most segments with nothing of their own to read.
        for total in [1usize, 2, 3, 5, 8, 97] {
            let case = format!("{:?} over {total} segment(s)", row.case);
            let mut concatenated: Vec<String> = Vec::new();
            let mut busy = 0usize;
            for segment in 0..total {
                let page = scan_segment(&router, row.table, segment, total).await;
                let keys = partition_keys(&page, row.wire);
                assert_eq!(
                    (page["Count"].clone(), page["ScannedCount"].clone()),
                    (json!(keys.len()), json!(keys.len())),
                    "case {case} segment {segment}: with no filter both counts are this \
                     segment's own share, got {page}"
                );
                let again = partition_keys(
                    &scan_segment(&router, row.table, segment, total).await,
                    row.wire,
                );
                assert_eq!(
                    again, keys,
                    "case {case} segment {segment}: reading the same segment again answers \
                     the same items, got {again:?} against {keys:?}"
                );
                if !keys.is_empty() {
                    busy += 1;
                }
                concatenated.extend(keys);
            }
            // A division of the key space into more blocks than the table has partitions puts
            // those partitions in blocks of their own: the whole table answered by ONE segment
            // — the symptom of a fan-out that is not applied at all — is not this.
            if total > row.keys.len() {
                assert!(
                    busy > 1,
                    "case {case}: a fan-out wider than the table's {} partitions divides them \
                     over more than one segment, got {busy} segment(s) with items",
                    row.keys.len()
                );
            }
            let distinct: BTreeSet<&String> = concatenated.iter().collect();
            assert_eq!(
                distinct.len(),
                concatenated.len(),
                "case {case}: no item is read by two segments, got {concatenated:?}"
            );
            assert_eq!(
                concatenated, whole,
                "case {case}: the segments in segment order read exactly the unsegmented \
                 scan, got {concatenated:?} against {whole:?}"
            );
        }
    }
}

/// The table the number-spelling rule is read over.
const SPELLED: &str = "SegSpelledNumber";

/// The spellings one numeric partition-key VALUE is written in: the integer itself, the
/// same value in exponent form, the same value with trailing zeroes after a point, and the
/// same value as a fraction of a larger power of ten.
const SPELLINGS: [&str; 4] = ["7500", "7.5e3", "7500.00", "0.75e4"];

/// The fan-out that rule is read over — wide enough that a key placed by anything but its
/// numeric value would almost certainly land in another segment.
const SPELLED_TOTAL: usize = 16;

#[tokio::test]
async fn a_number_partition_key_is_placed_by_its_value_and_not_by_its_spelling() {
    let router = emulator();
    create_keyed_table(&router, SPELLED, "N", false).await;

    // Each spelling: the item it addresses, the key the store holds it under, and the one
    // segment of [`SPELLED_TOTAL`] that reads it.
    let mut placed: Vec<(&str, String, usize)> = Vec::new();
    for spelling in SPELLINGS {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": SPELLED,
                "Item": {"pk": {"N": spelling}, "probe": {"S": spelling}},
            }),
        )
        .await;
        let whole = ok(&router, "Scan", json!({"TableName": SPELLED})).await;
        let stored = partition_keys(&whole, "N");
        assert_eq!(
            stored.len(),
            1,
            "case {spelling:?}: every spelling of one number addresses ONE item, got {whole}"
        );
        let mut owner: Option<usize> = None;
        for segment in 0..SPELLED_TOTAL {
            let read = partition_keys(
                &scan_segment(&router, SPELLED, segment, SPELLED_TOTAL).await,
                "N",
            );
            if !read.is_empty() {
                assert!(
                    owner.is_none(),
                    "case {spelling:?}: the item is read by ONE segment, but segment \
                     {segment} read it again"
                );
                owner = Some(segment);
            }
        }
        let owner = owner.unwrap_or_else(|| {
            panic!("case {spelling:?}: no segment of {SPELLED_TOTAL} read the item")
        });
        placed.push((
            spelling,
            stored.first().cloned().unwrap_or_default(),
            owner,
        ));
    }

    let (first_spelling, first_key, first_owner) = placed
        .first()
        .cloned()
        .expect("the rows above placed every spelling");
    for (spelling, key, owner) in &placed {
        assert_eq!(
            (key.as_str(), *owner),
            (first_key.as_str(), first_owner),
            "case {spelling:?}: the same number is the same key in the same segment as \
             {first_spelling:?}, got key {key:?} in segment {owner}"
        );
    }
}

/// The table a LARGE fan-out is read over: numeric partition keys with several rows each,
/// so a segment's share is whole partitions rather than single rows.
const FANNED: &str = "SegLargeFanOut";

/// Its partition keys — a zero, a negative fraction and a plain integer.
const FANNED_KEYS: [&str; 3] = ["0", "-4.5", "110600"];

/// How many rows sit in each of them.
const FANNED_ROWS: usize = 2;

/// The fan-out itself: far above the number of partitions the table holds, and well inside
/// `ScanTotalSegments` (range 1..1000000).
const LARGE_TOTAL: usize = 4096;

#[tokio::test]
async fn a_large_fan_out_partitions_the_table_the_way_a_small_one_does() {
    let router = emulator();
    create_keyed_table(&router, FANNED, "N", true).await;
    for key in FANNED_KEYS {
        for row in 0..FANNED_ROWS {
            ok(
                &router,
                "PutItem",
                json!({
                    "TableName": FANNED,
                    "Item": {
                        "pk": {"N": key},
                        "sk": {"N": row.to_string()},
                        "probe": {"S": format!("{key}-{row}")},
                    },
                }),
            )
            .await;
        }
    }
    let whole = item_keys(&ok(&router, "Scan", json!({"TableName": FANNED})).await, "N");
    assert_eq!(
        whole.len(),
        FANNED_KEYS.len() * FANNED_ROWS,
        "case a large fan-out: the unsegmented scan reads every row, got {whole:?}"
    );

    // Every segment of the fan-out, so what the segments together read — and which of them
    // read each partition — is the whole division and not a sample of it.
    let mut concatenated: Vec<String> = Vec::new();
    let mut owners: BTreeMap<String, usize> = BTreeMap::new();
    for segment in 0..LARGE_TOTAL {
        let page = scan_segment(&router, FANNED, segment, LARGE_TOTAL).await;
        let keys = item_keys(&page, "N");
        assert_eq!(
            (page["Count"].clone(), page["ScannedCount"].clone()),
            (json!(keys.len()), json!(keys.len())),
            "case a large fan-out, segment {segment}: both counts are this segment's own \
             share, got {page}"
        );
        for key in &keys {
            let partition = partition_of(key);
            let owner = *owners.entry(partition.clone()).or_insert(segment);
            assert_eq!(
                owner, segment,
                "case a large fan-out: every row of partition {partition:?} is read by ONE \
                 segment, but {owner} and {segment} both read it"
            );
        }
        concatenated.extend(keys);
    }

    let distinct: BTreeSet<&String> = concatenated.iter().collect();
    assert_eq!(
        distinct.len(),
        concatenated.len(),
        "case a large fan-out: no row is read by two segments, got {concatenated:?}"
    );
    assert_eq!(
        concatenated, whole,
        "case a large fan-out: the segments in segment order read exactly the unsegmented \
         scan, got {concatenated:?} against {whole:?}"
    );
    assert_eq!(
        owners.len(),
        FANNED_KEYS.len(),
        "case a large fan-out: every partition is read by some segment, got {owners:?}"
    );
    // And this fan-out has a block for every partition, so the partitions are read by
    // segments of their own: one segment answering the whole table is the symptom of a
    // fan-out that was never applied.
    let reading: BTreeSet<usize> = owners.values().copied().collect();
    assert!(
        reading.len() > 1,
        "case a large fan-out: {LARGE_TOTAL} segments divide {} partitions over more than \
         one of them, got {owners:?}",
        FANNED_KEYS.len()
    );

    // And a segment admits the resume tokens of exactly the rows it reads: the segment that
    // owns a partition resumes inside it, and the next segment of the same fan-out refuses
    // the same token.
    for (partition, owner) in &owners {
        let key = json!({"pk": {"N": partition}, "sk": {"N": "0"}});
        let case = format!("a large fan-out, the token of partition {partition:?}");
        assert!(
            admits_token(&router, &case, FANNED, *owner, LARGE_TOTAL, &key).await,
            "case {case}: segment {owner} reads that partition, so it resumes its token"
        );
        let other = (owner + 1) % LARGE_TOTAL;
        assert!(
            !admits_token(&router, &case, FANNED, other, LARGE_TOTAL, &key).await,
            "case {case}: segment {other} does not read that partition, so the token is not \
             its own to resume"
        );
    }
}

/// The top of `ScanTotalSegments` (range 1..1000000).
const CEILING_TOTAL: usize = 1_000_000;

/// The segments of that fan-out this suite probes, spanning its ends: the first, its
/// neighbour, two inside it, and the largest `Segment` the model allows (`ScanSegment`,
/// range 0..999999) with the one below it.
const CEILING_SEGMENTS: [usize; 6] = [0, 1, 4_095, 500_000, 999_998, 999_999];

/// The NARROW fan-out a key's segment at the ceiling is located through: a division small
/// enough to sweep, whose own block then holds that segment ([`locate_at_the_ceiling`]).
const COARSE_TOTAL: usize = 1_024;

/// Which segment of `total` reads the item under the partition key `key`, found by reading
/// every segment of that division — the one segment of it the key belongs to.
async fn owning_segment(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    key: &str,
    total: usize,
) -> usize {
    let mut owner: Option<usize> = None;
    for segment in 0..total {
        let keys = partition_keys(&scan_segment(router, table, segment, total).await, "N");
        if keys.iter().any(|read| read == key) {
            assert!(
                owner.is_none(),
                "case {case}: {key:?} is read by ONE segment of {total}, but segment \
                 {segment} read it again"
            );
            owner = Some(segment);
        }
    }
    owner.unwrap_or_else(|| panic!("case {case}: no segment of {total} read {key:?}"))
}

#[tokio::test]
async fn at_the_top_of_the_modelled_range_a_segment_reads_what_its_resume_rule_admits() {
    let router = emulator();
    create_keyed_table(&router, "SegCeilingFanOut", "N", false).await;
    for key in KEYED[1].keys {
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": "SegCeilingFanOut",
                "Item": {"pk": {"N": *key}, "probe": {"S": *key}},
            }),
        )
        .await;
    }
    let whole = partition_keys(
        &ok(&router, "Scan", json!({"TableName": "SegCeilingFanOut"})).await,
        "N",
    );
    let held: BTreeSet<&String> = whole.iter().collect();

    let mut read_by_a_probe: Vec<String> = Vec::new();
    for segment in CEILING_SEGMENTS {
        let case = format!("segment {segment} of {CEILING_TOTAL}");
        let page = scan_segment(&router, "SegCeilingFanOut", segment, CEILING_TOTAL).await;
        let keys = partition_keys(&page, "N");
        assert_eq!(
            (page["Count"].clone(), page["ScannedCount"].clone()),
            (json!(keys.len()), json!(keys.len())),
            "case {case}: both counts are this segment's own share, got {page}"
        );
        for key in &keys {
            assert!(
                held.contains(key),
                "case {case}: a segment reads only items the table holds, got {key:?} \
                 against {whole:?}"
            );
        }
        let again = partition_keys(
            &scan_segment(&router, "SegCeilingFanOut", segment, CEILING_TOTAL).await,
            "N",
        );
        assert_eq!(
            again, keys,
            "case {case}: reading the same segment again answers the same items, got \
             {again:?} against {keys:?}"
        );
        // The agreement that stands in for a sweep this wide: a segment reads an item
        // exactly when it admits that item's key as its own resume token.
        for key in &whole {
            let admitted = admits_token(
                &router,
                &case,
                "SegCeilingFanOut",
                segment,
                CEILING_TOTAL,
                &json!({"pk": {"N": key}}),
            )
            .await;
            assert_eq!(
                keys.contains(key),
                admitted,
                "case {case}: the segment reads {key:?} exactly when it admits its resume \
                 token, read {:?} against admitted {admitted}",
                keys.contains(key)
            );
        }
        read_by_a_probe.extend(keys);
    }

    let distinct: BTreeSet<&String> = read_by_a_probe.iter().collect();
    assert_eq!(
        distinct.len(),
        read_by_a_probe.len(),
        "case the probed segments of {CEILING_TOTAL}: no item is read by two of them, got \
         {read_by_a_probe:?}"
    );

    locate_at_the_ceiling(&router, &whole).await;
}

/// Read the agreement above in the POSITIVE direction at a fan-out too wide to sweep: find
/// the one segment of [`CEILING_TOTAL`] that reads a key, and read what it answers.
///
/// The segments of any division are contiguous blocks of ONE ordered key space, so a wider
/// division only SUBDIVIDES a narrower one's blocks: the block a key's segment of
/// [`COARSE_TOTAL`] covers is where its segment of [`CEILING_TOTAL`] lies, which turns a
/// million-segment search into a sweep of the few hundred segments that block spans.
async fn locate_at_the_ceiling(router: &MultiServiceRouter, held: &[String]) {
    let key = held.first().cloned().expect("the table holds its keys");
    let case = format!("locating {key:?} among {CEILING_TOTAL} segments");
    let coarse = owning_segment(router, &case, "SegCeilingFanOut", &key, COARSE_TOTAL).await;
    let first = coarse * CEILING_TOTAL / COARSE_TOTAL;
    let last = ((coarse + 1) * CEILING_TOTAL / COARSE_TOTAL).min(CEILING_TOTAL - 1);

    let mut owner: Option<usize> = None;
    for segment in first..=last {
        let page = scan_segment(router, "SegCeilingFanOut", segment, CEILING_TOTAL).await;
        let keys = partition_keys(&page, "N");
        if keys.iter().any(|read| read == &key) {
            assert!(
                owner.is_none(),
                "case {case}: one segment reads it, but {segment} read it again after {owner:?}"
            );
            owner = Some(segment);
        }
    }
    let owner = owner.unwrap_or_else(|| {
        panic!(
            "case {case}: segment {coarse} of {COARSE_TOTAL} reads it, so one segment of \
             {first}..={last} must read it too"
        )
    });
    // That segment reads it, and admits its resume token: the agreement holds where a
    // segment HAS something of its own at the top of the range too.
    assert!(
        admits_token(
            router,
            &case,
            "SegCeilingFanOut",
            owner,
            CEILING_TOTAL,
            &json!({"pk": {"N": key.clone()}}),
        )
        .await,
        "case {case}: segment {owner} reads it, so it resumes its token"
    );
    let elsewhere = if owner == first { last } else { first };
    if elsewhere != owner {
        assert!(
            !admits_token(
                router,
                &case,
                "SegCeilingFanOut",
                elsewhere,
                CEILING_TOTAL,
                &json!({"pk": {"N": key.clone()}}),
            )
            .await,
            "case {case}: segment {elsewhere} does not read it, so the token is not its own \
             to resume"
        );
    }
}
