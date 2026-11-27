// SPDX-License-Identifier: CC0-1.0

//! `StartBulkPutItems` and the bulk-operation catalogue it writes, driven end to end.
//!
//! RULE under test: a bulk put reads the items staged under its `InputS3Configuration` and
//! WRITES them into the existing table it addresses, recording ONE bulk operation for the
//! call — which `ListBulkOperations` reports afterwards and whose status runs the span every
//! clock-owed transition of this emulator takes. A call that repeats the `ClientToken` of an
//! earlier call answers the operation that token already started and starts no second one,
//! so the source is written once however many times the call is retried; a call that reuses
//! the token and CHANGES any other member asks for something the token does not stand for
//! and is refused. A table the account does not hold is `ResourceNotFoundException` naming
//! it, and nothing is written.
//!
//! The rows span what the rule generalises over: HOW MANY records the source carries (one,
//! several), WHAT they hold (a key alone, a key beside other attributes, nested values), the
//! KEY SCHEMA they are keyed against (a partition key alone, a partition and a sort key),
//! the COMPRESSION the source is staged with (none, GZIP), whether the request narrows the
//! source with a key prefix, WHICH spelling the table is addressed by (its name and its
//! ARN), and whether the call carries a `ClientToken` at all.
//!
//! Every item a bulk put wrote is read back through `GetItem` — the reader a client verifies
//! a write with — so the assertions are about STORED state and not about what the call
//! echoed. Amazon S3 is mounted beside DynamoDB, exactly as `core/router` assembles them,
//! so the objects the rows stage are the ones the operation really reads.

use std::io::Write;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    absent_clause, call, create_bucket, emulator_with_s3, error_code, message, ok, put_object,
    validation_envelope, POLL_INTERVAL,
};
use crate::table_items::create_active_table;

/// The bucket every row stages its source objects in.
const BUCKET: &str = "bulk-put-items-source";

/// The table most rows write into: one string partition key.
const TABLE: &str = "BulkPutItems";

/// A second table, keyed by a partition AND a sort key, so the rule is driven against more
/// than one key schema.
const COMPOSITE_TABLE: &str = "BulkPutItemsComposite";

/// One source object's bytes in the DynamoDB JSON form a table export writes: one item per
/// line, each under the `Item` member.
fn dynamodb_json_lines(items: &[Value]) -> Vec<u8> {
    items
        .iter()
        .map(|item| format!("{}\n", json!({"Item": item})))
        .collect::<String>()
        .into_bytes()
}

/// The same bytes, GZIP-compressed — the one compression a row stages its source with.
fn gzipped(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(bytes)
        .expect("the encoder takes the source bytes");
    encoder.finish().expect("the encoder closes its frame")
}

/// One `StartBulkPutItems` body: the table, the source prefix, the compression the source
/// is staged with, and the token when the row carries one.
///
/// Every request names an `InputS3Configuration` stating the format its records are staged
/// in and an `OutputS3Configuration` stating where the operation reports its per-record
/// outcomes and which of them it reports: `InputFormat`, `OutputS3Configuration` and that
/// configuration's `OutputType` are members the input REQUIRES
/// (`smithy_query(query="closure", operation="StartBulkPutItems")`), so a body that omits
/// one is refused before the call is served (the rows below that pin it).
fn start_body(
    table: &str,
    prefix: &str,
    compression: Option<&str>,
    client_token: Option<&str>,
) -> Value {
    let mut body = json!({
        "TableName": table,
        "InputS3Configuration": {
            "S3Bucket": BUCKET,
            "S3KeyPrefix": prefix,
            "InputFormat": "DYNAMODB_JSON",
        },
        "OutputS3Configuration": {
            "S3Bucket": BUCKET,
            "S3KeyPrefix": "outcomes/",
            "OutputType": ["ERROR"],
        },
    });
    if let Some(compression) = compression {
        body["InputS3Configuration"]["InputCompressionType"] = json!(compression);
    }
    if let Some(token) = client_token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// The description a `StartBulkPutItems` answered.
fn described(started: &Value) -> Value {
    started["BulkOperationDescription"].clone()
}

/// `count` items for [`TABLE`], each a distinct key beside attributes that are not one.
fn items(prefix: &str, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            json!({
                "pk": {"S": format!("{prefix}-{index}")},
                "note": {"S": format!("item {index}")},
                "doc": {"M": {"inner": {"L": [{"S": "a"}, {"N": index.to_string()}]}}},
            })
        })
        .collect()
}

/// The primary key of `item` for the tables below: the key attributes it carries.
fn key_of(item: &Value) -> Value {
    let members: serde_json::Map<String, Value> = item
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| name.as_str() == "pk" || name.as_str() == "sk")
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    Value::Object(members)
}

/// What `GetItem` reports for `key` of `table`: the stored item, or `Value::Null` where the
/// key holds none.
async fn stored(router: &MultiServiceRouter, table: &str, key: Value) -> Value {
    let read = ok(router, "GetItem", json!({"TableName": table, "Key": key})).await;
    read.get("Item").cloned().unwrap_or(Value::Null)
}

/// The summaries `ListBulkOperations` reports, in the order it reports them: every bulk
/// operation of the region the endpoint serves, since the request carries no member that
/// narrows the listing (`smithy_query(query="operation", operation="ListBulkOperations")`).
async fn listed(router: &MultiServiceRouter) -> Vec<Value> {
    let answered = ok(router, "ListBulkOperations", json!({})).await;
    answered["BulkOperationsSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The emulator with Amazon S3 beside it, the source bucket created, and the two tables
/// this suite writes into `ACTIVE`.
async fn seeded() -> MultiServiceRouter {
    let router = emulator_with_s3();
    create_bucket(&router, BUCKET).await;
    create_active_table(&router, TABLE, &[("pk", "HASH", "S")]).await;
    create_active_table(
        &router,
        COMPOSITE_TABLE,
        &[("pk", "HASH", "S"), ("sk", "RANGE", "N")],
    )
    .await;
    router
}

/// One served row of the suite below: what the call addresses, where its source is staged
/// and what that source carries.
struct Row {
    case: &'static str,
    /// The table the call ADDRESSES, by name or by ARN.
    addressed: &'static str,
    /// The name the stored items are read back under.
    named: &'static str,
    /// The key prefix the source objects are staged under.
    prefix: &'static str,
    /// The compression the source is staged with, where the row names one.
    compression: Option<&'static str>,
    /// The items the source carries.
    carried: Vec<Value>,
    /// The `ClientToken` the call carries, where the row carries one.
    token: Option<&'static str>,
}

#[tokio::test]
async fn a_bulk_put_writes_what_its_source_carried_and_records_one_operation() {
    let router = seeded().await;
    let composite: Vec<Value> = (0..4)
        .map(|index| {
            json!({
                "pk": {"S": "shared"},
                "sk": {"N": index.to_string()},
                "doc": {"M": {"inner": {"L": [{"S": "a"}, {"N": "1"}]}}},
            })
        })
        .collect();

    // Each row: the case, the table the call ADDRESSES, the name the stored items are read
    // back under (so a row can address the same table by its ARN), the prefix the source is
    // staged under, the compression it is staged with, the items it carries, and the token
    // the call carries.
    let rows: Vec<Row> = vec![
        Row {
            case: "one record, uncompressed",
            addressed: TABLE,
            named: TABLE,
            prefix: "one/",
            compression: None,
            carried: items("one", 1),
            token: None,
        },
        Row {
            case: "several records, uncompressed",
            addressed: TABLE,
            named: TABLE,
            prefix: "few/",
            compression: Some("NONE"),
            carried: items("few", 5),
            token: None,
        },
        Row {
            case: "several records under a token",
            addressed: TABLE,
            named: TABLE,
            prefix: "token/",
            compression: None,
            carried: items("token", 3),
            token: Some("bulk-token-rows"),
        },
        Row {
            case: "a GZIP source, keyed by a partition and a sort key",
            addressed: COMPOSITE_TABLE,
            named: COMPOSITE_TABLE,
            prefix: "gzip/",
            compression: Some("GZIP"),
            carried: composite.clone(),
            token: None,
        },
        Row {
            case: "a table addressed by its ARN",
            addressed: "arn:aws:dynamodb:us-east-1:000000000000:table/BulkPutItems",
            named: TABLE,
            prefix: "by-arn/",
            compression: None,
            carried: items("by-arn", 2),
            token: None,
        },
    ];

    for Row {
        case,
        addressed,
        named,
        prefix,
        compression,
        carried,
        token,
    } in rows
    {
        let lines = dynamodb_json_lines(&carried);
        let body = if compression == Some("GZIP") {
            gzipped(&lines)
        } else {
            lines
        };
        put_object(&router, BUCKET, &format!("{prefix}data.json"), body).await;

        let started = described(
            &ok(
                &router,
                "StartBulkPutItems",
                start_body(addressed, prefix, compression, token),
            )
            .await,
        );
        let arn = started["BulkOperationArn"].as_str().unwrap_or_default();
        assert!(
            arn.contains(":bulk-operation/") && !arn.contains(":table/"),
            "case {case:?}: the call answers the account-scoped ARN of the operation it \
             started, got {started}"
        );
        assert_eq!(
            started["BulkOperationStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a bulk operation is reported running when it is started, got \
             {started}"
        );

        // Every record the source carried is stored, read back through the reader a client
        // verifies a write with.
        for item in &carried {
            let key = key_of(item);
            let read = stored(&router, named, key.clone()).await;
            assert_eq!(
                &read, item,
                "case {case:?}: the item at {key} is stored whole, got {read}"
            );
        }
    }

    // ONE operation per call that started one, all of them reported by the region's listing.
    let summaries = listed(&router).await;
    assert_eq!(
        summaries.len(),
        5,
        "one summary per call that started an operation, got {summaries:?}"
    );
    for summary in &summaries {
        assert!(
            summary["BulkOperationArn"].is_string() && summary["StartTime"].is_number(),
            "every summary carries the operation's ARN and the instant it began, got {summary}"
        );
    }
}

#[tokio::test]
async fn a_repeated_client_token_answers_one_operation_and_writes_the_source_once() {
    let router = seeded().await;
    let first_items = items("replay", 2);
    let second_items = items("second", 3);
    put_object(
        &router,
        BUCKET,
        "replay/data.json",
        dynamodb_json_lines(&first_items),
    )
    .await;
    put_object(
        &router,
        BUCKET,
        "second/data.json",
        dynamodb_json_lines(&second_items),
    )
    .await;

    let token = "bulk-token-replayed";
    let first = described(
        &ok(
            &router,
            "StartBulkPutItems",
            start_body(TABLE, "replay/", None, Some(token)),
        )
        .await,
    );
    // An IDENTICAL call under the same token answers the operation it already started, and
    // writes nothing of its own: the items are the ones the first call wrote.
    let again = described(
        &ok(
            &router,
            "StartBulkPutItems",
            start_body(TABLE, "replay/", None, Some(token)),
        )
        .await,
    );
    assert_eq!(
        again["BulkOperationArn"], first["BulkOperationArn"],
        "a replayed token answers the operation it already started, got {again} after {first}"
    );
    for item in &first_items {
        let key = key_of(item);
        let read = stored(&router, TABLE, key.clone()).await;
        assert_eq!(
            &read, item,
            "the items the first call wrote are the ones a replay reports over, got {read} at \
             {key}"
        );
    }

    // A call that reuses the token and CHANGES any other member asks for something the
    // token does not stand for: it is refused, and nothing of it is written. The rows span
    // the members a request carries beside the token.
    let changed_source = start_body(TABLE, "second/", None, Some(token));
    let changed_rate = {
        let mut body = start_body(TABLE, "replay/", None, Some(token));
        body["MaxWriteRequestUnits"] = json!(1024);
        body
    };
    let changed_compression = start_body(TABLE, "replay/", Some("GZIP"), Some(token));
    let changed_tier = {
        let mut body = start_body(TABLE, "replay/", None, Some(token));
        body["PerformanceTier"] = json!("STANDARD");
        body
    };
    let changed_table = start_body(COMPOSITE_TABLE, "replay/", None, Some(token));
    for (case, body) in [
        ("another source", changed_source),
        ("another write rate", changed_rate),
        ("another compression", changed_compression),
        ("another performance tier", changed_tier),
        ("another table", changed_table),
    ] {
        let (status, answered) = call(&router, "StartBulkPutItems", body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "IdempotentParameterMismatchException"),
            "case {case:?}: a token reused with another parameter is refused, got {status} with \
             {answered}"
        );
    }
    // The source the refused replays named was never written.
    for item in &second_items {
        let key = key_of(item);
        let read = stored(&router, TABLE, key.clone()).await;
        assert_eq!(
            read,
            Value::Null,
            "a refused replay writes nothing, got {read} at {key}"
        );
    }

    // ONE operation in the catalogue, however many times the call was replayed.
    let summaries = listed(&router).await;
    assert_eq!(
        summaries.len(),
        1,
        "a replayed token leaves one operation in the catalogue, got {summaries:?}"
    );

    // A DIFFERENT token is a call of its own, so it starts a second operation and writes
    // the source it names.
    let other = described(
        &ok(
            &router,
            "StartBulkPutItems",
            start_body(TABLE, "second/", None, Some("bulk-token-other")),
        )
        .await,
    );
    assert_ne!(
        other["BulkOperationArn"], first["BulkOperationArn"],
        "another token starts an operation of its own, got {other} after {first}"
    );
    let read = stored(&router, TABLE, key_of(&second_items[0])).await;
    assert_eq!(
        read, second_items[0],
        "the operation another token started writes its own source, got {read}"
    );
    let summaries = listed(&router).await;
    assert_eq!(
        summaries.len(),
        2,
        "two tokens, two operations, got {summaries:?}"
    );
}

#[tokio::test]
async fn a_bulk_operation_settles_into_its_terminal_status_and_reports_its_end() {
    let router = seeded().await;
    put_object(
        &router,
        BUCKET,
        "settling/data.json",
        dynamodb_json_lines(&items("settling", 2)),
    )
    .await;
    ok(
        &router,
        "StartBulkPutItems",
        start_body(TABLE, "settling/", None, None),
    )
    .await;

    // The transition the clock owes is observed by POLLING the listing until it settles,
    // under a deadline generous enough that a loaded machine is not a defect.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let summary = loop {
        let page = listed(&router).await;
        let settled = page
            .first()
            .filter(|summary| summary["BulkOperationStatus"] == json!("COMPLETED"))
            .cloned();
        if let Some(settled) = settled {
            break settled;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the bulk operation never settled, last {page:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert!(
        summary["EndTime"].is_number(),
        "a settled operation reports the instant it ended, got {summary}"
    );
    assert!(
        summary["EndTime"].as_f64() >= summary["StartTime"].as_f64(),
        "an operation ends no earlier than it began, got {summary}"
    );
}

#[tokio::test]
async fn a_source_that_cannot_be_read_settles_the_operation_as_failed() {
    let router = seeded().await;
    let mut body = start_body(TABLE, "nowhere/", None, None);
    body["InputS3Configuration"]["S3Bucket"] = json!("bulk-put-items-absent");
    let started = described(&ok(&router, "StartBulkPutItems", body).await);
    assert_eq!(
        started["BulkOperationStatus"],
        json!("IN_PROGRESS"),
        "an accepted call reports the operation it started, got {started}"
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let settled = loop {
        let page = listed(&router).await;
        let reported = page
            .first()
            .map(|summary| summary["BulkOperationStatus"].clone())
            .unwrap_or(Value::Null);
        if reported == json!("FAILED") {
            break reported;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the bulk operation never settled, last {page:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        settled,
        json!("FAILED"),
        "a source the object store holds none of fails the operation, got {settled}"
    );
}

#[tokio::test]
async fn a_bulk_put_is_refused_before_it_reads_or_writes_anything() {
    let router = seeded().await;
    put_object(
        &router,
        BUCKET,
        "refused/data.json",
        dynamodb_json_lines(&items("refused", 1)),
    )
    .await;

    // Each row: the case, the body, the error code and the whole message it must carry —
    // `None` for a message this suite does not pin.
    let absent_table = {
        let mut body = start_body("BulkPutItemsAbsent", "refused/", None, None);
        body["ClientToken"] = json!("bulk-token-absent-table");
        body
    };
    let no_source = {
        let mut body = start_body(TABLE, "refused/", None, None);
        body["InputS3Configuration"] = json!({"S3KeyPrefix": "refused/"});
        body
    };
    let ion_source = {
        let mut body = start_body(TABLE, "refused/", None, None);
        body["InputS3Configuration"]["InputFormat"] = json!("ION");
        body
    };
    // A body that states no outcome location at all, and one that states it `null`: the
    // member is one the input REQUIRES, so both are the same absence and are refused ahead
    // of everything the request names — the table it addresses included, which is why this
    // row names one the account does not hold and is still answered for the member.
    let no_outcome_location = {
        let mut body = start_body("BulkPutItemsAbsent", "refused/", None, None);
        body.as_object_mut()
            .expect("the body is an object")
            .remove("OutputS3Configuration");
        body
    };
    let null_outcome_location = {
        let mut body = start_body("BulkPutItemsAbsent", "refused/", None, None);
        body["OutputS3Configuration"] = Value::Null;
        body
    };
    let absent_member = Some(validation_envelope(&[absent_clause(
        "outputS3Configuration",
    )]));
    let rows: Vec<(&str, Value, &str, Option<String>)> = vec![
        (
            "a table the account does not hold",
            absent_table,
            "ResourceNotFoundException",
            Some("Requested resource not found: Table: BulkPutItemsAbsent not found".to_string()),
        ),
        (
            "an input configuration naming no bucket",
            no_source,
            "ValidationException",
            None,
        ),
        (
            "a source format no bulk put reads yet",
            ion_source,
            "NotImplemented",
            None,
        ),
        (
            "no outcome location at all",
            no_outcome_location,
            "ValidationException",
            absent_member.clone(),
        ),
        (
            "an outcome location stated null",
            null_outcome_location,
            "ValidationException",
            absent_member,
        ),
    ];

    for (case, body, code, expected) in rows {
        let (status, answered) = call(&router, "StartBulkPutItems", body).await;
        assert_eq!(
            error_code(&answered).as_str(),
            code,
            "case {case:?}: expected {code}, got {status} with {answered}"
        );
        if let Some(expected) = expected {
            let stated = message(&answered);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected the message {expected:?}, got {stated:?}"
            );
        }
    }

    // A source format outside the set the member admits
    // (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BulkOperationInputFormat")`
    // gives DYNAMODB_JSON and ION) is refused by that set, ahead of everything else the
    // request names — the table it addresses included, which is why these rows name one the
    // account does not hold and are still answered for the member.
    let format_rows: Vec<(&str, &str)> = vec![
        ("a format the import surface reads and a bulk put does not", "CSV"),
        ("a format no surface reads", "PARQUET"),
        ("the right value in the wrong case", "dynamodb_json"),
    ];
    for (case, format) in format_rows {
        let mut body = start_body("BulkPutItemsAbsent", "refused/", None, None);
        body["InputS3Configuration"]["InputFormat"] = json!(format);
        let (status, answered) = call(&router, "StartBulkPutItems", body).await;
        let expected = validation_envelope(&[format!(
            "Value '{format}' at 'inputS3Configuration.inputFormat' failed to satisfy \
             constraint: Member must satisfy enum value set: [ION, DYNAMODB_JSON]"
        )]);
        let stated = message(&answered);
        assert_eq!(
            (
                status,
                error_code(&answered).as_str(),
                stated.as_str()
            ),
            (400, "ValidationException", expected.as_str()),
            "case {case:?}: expected the value-set refusal, got {status} with {answered}"
        );
    }

    // Nothing any refused call carried was written, and no operation was recorded for one.
    let read = stored(&router, TABLE, json!({"pk": {"S": "refused-0"}})).await;
    assert_eq!(
        read,
        Value::Null,
        "a refused bulk put writes nothing, got {read}"
    );
    let summaries = listed(&router).await;
    assert!(
        summaries.is_empty(),
        "a refused bulk put records no operation, got {summaries:?}"
    );
}

#[tokio::test]
async fn a_listing_pages_by_max_results_over_the_regions_operations() {
    let router = seeded().await;
    for index in 0..3 {
        let prefix = format!("page{index}/");
        put_object(
            &router,
            BUCKET,
            &format!("{prefix}data.json"),
            dynamodb_json_lines(&items(&format!("page{index}"), 1)),
        )
        .await;
        ok(
            &router,
            "StartBulkPutItems",
            start_body(TABLE, &prefix, None, None),
        )
        .await;
    }
    put_object(
        &router,
        BUCKET,
        "other/data.json",
        dynamodb_json_lines(&[json!({"pk": {"S": "other"}, "sk": {"N": "0"}})]),
    )
    .await;
    ok(
        &router,
        "StartBulkPutItems",
        start_body(COMPOSITE_TABLE, "other/", None, None),
    )
    .await;

    // The listing reports every operation of the region, whichever table each acted on: the
    // request carries no member that narrows it.
    let all = ok(&router, "ListBulkOperations", json!({})).await;
    assert_eq!(
        all["BulkOperationsSummaryList"].as_array().map(Vec::len),
        Some(4),
        "the listing reports every operation of the region, got {all}"
    );
    assert!(
        all.get("NextToken").is_none(),
        "a page that reported everything carries no cursor, got {all}"
    );

    // Each page size the rows span reports at most that many entries and a cursor the next
    // call continues from, until the last page reports no cursor.
    for size in [1usize, 2, 3] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({"MaxResults": size});
            if let Some(cursor) = token.as_deref() {
                body["NextToken"] = json!(cursor);
            }
            let page = ok(&router, "ListBulkOperations", body).await;
            let entries = page["BulkOperationsSummaryList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                entries.len() <= size,
                "case MaxResults {size}: a page carries at most what it asked for, got {page}"
            );
            seen.extend(
                entries
                    .iter()
                    .filter_map(|summary| summary["BulkOperationArn"].as_str())
                    .map(ToString::to_string),
            );
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen.len(),
            4,
            "case MaxResults {size}: paging reports the region's four operations once each, \
             got {seen:?}"
        );
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case MaxResults {size}: no operation is reported twice, got {seen:?}"
        );
    }

    // A `MaxResults` below the bound the model declares for the member is refused, and a
    // `NextToken` this operation did not mint stands for no page.
    for (case, body, code) in [
        (
            "a page size below the member's minimum",
            json!({"MaxResults": 0}),
            "ValidationException",
        ),
        (
            "a cursor this operation did not mint",
            json!({"NextToken": "not-a-cursor"}),
            "ValidationException",
        ),
    ] {
        let (status, answered) = call(&router, "ListBulkOperations", body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {status} with {answered}"
        );
    }
}
