// SPDX-License-Identifier: CC0-1.0

//! How an import task ENDS when its source carried an item the table could not hold, and
//! how an import nothing was ever started for is refused.
//!
//! The rules pinned here:
//!
//! 1. an import that could not write every item it read ends in the terminal
//!    `ImportStatus` `FAILED`, reporting one error per item it could not write
//!    (`ErrorCount`), the `FailureCode` those items failed — `ItemValidationError` — and
//!    the fixed `FailureMessage` DynamoDB states for it, while every item that DID pass is
//!    in the table and readable; an import that wrote every item it read ends `COMPLETED`
//!    with no failure at all;
//! 2. an operation addressed at an import ARN the catalogue holds no import for answers
//!    `ImportNotFoundException` ("The specified import was not found") with the sentence
//!    DynamoDB states for that class: fixed text around the one variable part, the
//!    `ImportArn` the request addressed.
//!
//! Every call is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, with Amazon S3 mounted beside it
//! on ONE engine: an import reads objects that service really holds, so the source files
//! below are written through its own client.
//!
//! Rule 1 is asserted over a TABLE of sources differing in HOW an item fails validation
//! (a value that is no number where the item types one `N`, a blob that is not base64, a
//! member of a number set that is no number, a value naming two datatypes, a line that is
//! no JSON at all, an item the key schema cannot key), in how many of them do (none, one,
//! two) and in where they sit among the sound items (first, middle, last). Rule 2 is
//! asserted over a table of well-formed ARNs no import was started for, differing in the
//! table they name, the shape of the import id and whether the ARN was derived from a real
//! import's by perturbing it.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state;

use crate::emulator::{
    call, describe_when_active, emulator_with_s3, error_code, message, ok, put_object, REGION,
};

/// The bucket every source below is written into.
const BUCKET: &str = "import-item-validation";

/// The `FailureCode` an import whose items failed validation reports.
const ITEM_VALIDATION_CODE: &str = "ItemValidationError";

/// The sentence that failure carries: fixed text with no variable part.
const ITEM_VALIDATION_MESSAGE: &str = "Some of the items failed validation checks and were not \
                                       imported. Please check CloudWatch error logs for more \
                                       details.";

/// The emulator with [`BUCKET`] created through Amazon S3's own API — the source an import
/// reads from.
async fn emulator_with_source() -> MultiServiceRouter {
    let router = emulator_with_s3();
    crate::emulator::create_bucket(&router, BUCKET).await;
    router
}

/// How long a poll for the import's terminal status may take before this suite calls it a
/// hang: generous slack over the emulator's transition span, because the point of the bound
/// is that the wait ENDS rather than how fast it does.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(10);

/// Poll `DescribeImport` until the import reports a TERMINAL status, and answer the
/// description it reported then — the loop a client runs after an `ImportTable`.
async fn describe_when_terminal(router: &MultiServiceRouter, case: &str, arn: &str) -> Value {
    let started = std::time::Instant::now();
    let mut last = Value::Null;
    while started.elapsed() < TERMINAL_DEADLINE {
        let described = ok(router, "DescribeImport", json!({"ImportArn": arn})).await;
        let description = described["ImportTableDescription"].clone();
        let status = description["ImportStatus"].as_str().unwrap_or_default();
        if status != "IN_PROGRESS" && status != "CANCELLING" {
            return description;
        }
        last = description;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("case {case:?}: the import never reached a terminal status, last {last}")
}

/// One source file's lines, in the DynamoDB JSON form a table export writes: one item per
/// line under the `Item` member.
fn lines(items: &[Value]) -> Vec<u8> {
    items
        .iter()
        .map(|item| format!("{item}\n"))
        .collect::<String>()
        .into_bytes()
}

/// One row: the lines the source file holds, and what the import must make of them.
struct Row {
    case: &'static str,
    table: &'static str,
    /// The source file's lines, each a JSON document as the file carries it.
    source: Vec<Value>,
    /// The keys the imported table must serve, with the item each reads back as.
    imported: Vec<(Value, Value)>,
    /// How many records the task read, and how many of them it could not write.
    processed: i64,
    errors: i64,
}

/// One sound item, keyed by `Part`.
fn sound(part: &str) -> Value {
    json!({"Item": {"Part": {"S": part}, "Size": {"N": "1"}}})
}

/// The key of a sound item, as a client builds it from the key schema.
fn key(part: &str) -> Value {
    json!({"Part": {"S": part}})
}

/// The item [`sound`] stores, as `GetItem` reads it back.
fn stored(part: &str) -> Value {
    json!({"Part": {"S": part}, "Size": {"N": "1"}})
}

#[tokio::test]
async fn an_import_whose_items_fail_validation_ends_failed_over_the_items_that_passed() {
    // The rows differ along what the rule generalises over: WHICH validation an item fails
    // (the datatype catalogue is the model's own — `smithy_query(query="shape",
    // shape="com.amazonaws.dynamodb#AttributeValue")` gives S | N | B | SS | NS | BS | M |
    // L | NULL | BOOL — so a value under `N`, `B` or `NS` that carries something else is
    // no item of that type), HOW MANY items fail (none, one, two) and WHERE they sit among
    // the sound ones (first, middle, last).
    let rows: Vec<Row> = vec![
        Row {
            case: "every item sound",
            table: "ImportAllSound",
            source: vec![sound("row-1"), sound("row-2")],
            imported: vec![
                (key("row-1"), stored("row-1")),
                (key("row-2"), stored("row-2")),
            ],
            processed: 2,
            errors: 0,
        },
        Row {
            case: "an N value that is not a number, last",
            table: "ImportBadNumber",
            source: vec![
                sound("row-1"),
                json!({"Item": {"Part": {"S": "row-2"}, "value": {"N": "not-a-number"}}}),
            ],
            imported: vec![(key("row-1"), stored("row-1"))],
            processed: 2,
            errors: 1,
        },
        Row {
            case: "a B value that is not base64, first",
            table: "ImportBadBlob",
            source: vec![
                json!({"Item": {"Part": {"S": "row-1"}, "blob": {"B": "not base64!!"}}}),
                sound("row-2"),
            ],
            imported: vec![(key("row-2"), stored("row-2"))],
            processed: 2,
            errors: 1,
        },
        Row {
            case: "a number set holding a member that is no number, in the middle",
            table: "ImportBadNumberSet",
            source: vec![
                sound("row-1"),
                json!({"Item": {"Part": {"S": "row-2"}, "sizes": {"NS": ["1", "two"]}}}),
                sound("row-3"),
            ],
            imported: vec![
                (key("row-1"), stored("row-1")),
                (key("row-3"), stored("row-3")),
            ],
            processed: 3,
            errors: 1,
        },
        Row {
            case: "two items failing, one typed and one unreadable",
            table: "ImportTwoFailures",
            source: vec![
                json!({"Item": {"Part": {"S": "row-1"}, "both": {"S": "x", "N": "1"}}}),
                sound("row-2"),
                json!({"Item": {"Other": {"S": "no key at all"}}}),
            ],
            imported: vec![(key("row-2"), stored("row-2"))],
            processed: 3,
            errors: 2,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = emulator_with_source().await;
        let body = lines(&row.source);
        let bytes = i64::try_from(body.len()).expect("the object is sized");
        put_object(&router, BUCKET, "source/data.json", body).await;

        let started = ok(
            &router,
            "ImportTable",
            json!({
                "S3BucketSource": {"S3Bucket": BUCKET},
                "InputFormat": "DYNAMODB_JSON",
                "InputCompressionType": "NONE",
                "TableCreationParameters": {
                    "TableName": row.table,
                    "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                    "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                    "BillingMode": "PAY_PER_REQUEST",
                },
            }),
        )
        .await;
        let arn = started["ImportTableDescription"]["ImportArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !arn.is_empty(),
            "case {case:?}: the accepted import is recorded under an ARN, got {started}"
        );

        let reported = describe_when_terminal(&router, case, &arn).await;
        let failed = row.errors > 0;
        assert_eq!(
            reported["ImportStatus"],
            json!(if failed { "FAILED" } else { "COMPLETED" }),
            "case {case:?}: expected the terminal status of an import with {} error(s), got \
             {reported}",
            row.errors
        );
        assert_eq!(
            reported["ErrorCount"].as_i64().unwrap_or_default(),
            row.errors,
            "case {case:?}: expected {} item(s) counted as errors, got {reported}",
            row.errors
        );
        assert_eq!(
            reported["ProcessedItemCount"].as_i64().unwrap_or_default(),
            row.processed,
            "case {case:?}: expected {} record(s) processed, got {reported}",
            row.processed
        );
        assert_eq!(
            reported["ImportedItemCount"].as_i64().unwrap_or_default(),
            i64::try_from(row.imported.len()).expect("the row is sized"),
            "case {case:?}: expected the sound items imported, got {reported}"
        );
        assert_eq!(
            reported["ProcessedSizeBytes"].as_i64(),
            Some(bytes),
            "case {case:?}: the source was read, so its bytes are reported processed, got \
             {reported}"
        );
        assert_eq!(
            (
                reported["FailureCode"].clone(),
                reported["FailureMessage"].clone(),
            ),
            if failed {
                (json!(ITEM_VALIDATION_CODE), json!(ITEM_VALIDATION_MESSAGE))
            } else {
                (Value::Null, Value::Null)
            },
            "case {case:?}: expected the failure of an import with {} error(s), got {reported}",
            row.errors
        );

        // The items that DID pass validation are in the table and readable, and the ones
        // that did not are not there. The table an import creates is ready when the import
        // that fills it has finished, so it is polled for exactly as a client polls it.
        describe_when_active(&router, case, row.table).await;
        for (addressed, item) in &row.imported {
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": row.table, "Key": addressed, "ConsistentRead": true}),
            )
            .await;
            assert_eq!(
                read["Item"], *item,
                "case {case:?}: the imported table serves {addressed} as the source carried \
                 it, got {read}"
            );
        }
        let scanned = ok(
            &router,
            "Scan",
            json!({"TableName": row.table, "ConsistentRead": true}),
        )
        .await;
        assert_eq!(
            scanned["Count"].as_i64().unwrap_or_default(),
            i64::try_from(row.imported.len()).expect("the row is sized"),
            "case {case:?}: the table holds only the items that passed validation, got \
             {scanned}"
        );
    }
}

#[tokio::test]
async fn an_import_arn_the_catalogue_holds_nothing_for_is_refused_with_the_documented_sentence() {
    let router = emulator_with_source().await;
    put_object(&router, BUCKET, "source/data.json", lines(&[sound("row-1")])).await;
    let started = ok(
        &router,
        "ImportTable",
        json!({
            "S3BucketSource": {"S3Bucket": BUCKET},
            "InputFormat": "DYNAMODB_JSON",
            "TableCreationParameters": {
                "TableName": "ImportNotFoundSource",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            },
        }),
    )
    .await;
    let real = started["ImportTableDescription"]["ImportArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The ARN a real import was recorded under answers that import, so the refusals below
    // are about ARNs the catalogue holds nothing for and not about the form of an ARN.
    let served = ok(&router, "DescribeImport", json!({"ImportArn": real.clone()})).await;
    assert_eq!(
        served["ImportTableDescription"]["ImportArn"],
        json!(real.clone()),
        "the import the call started is addressable by the ARN it was recorded under, got \
         {served}"
    );

    // Rows differ along what the rule generalises over: the table the ARN names, the shape
    // of the import id, and whether the ARN came from a real import's by perturbing its
    // last character.
    let perturbed = {
        let mut characters: Vec<char> = real.chars().collect();
        let last = characters.len() - 1;
        characters[last] = if characters[last] == 'f' { '0' } else { 'f' };
        characters.into_iter().collect::<String>()
    };
    let cases: Vec<(&str, String)> = vec![
        (
            "a real import's ARN with its last character perturbed",
            perturbed,
        ),
        (
            "an import id of a table that holds imports",
            format!(
                "{}/import/01700000000000-aaaaaaaa",
                state::table_arn(REGION, "ImportNotFoundSource")
            ),
        ),
        (
            "an import of a table that does not exist",
            format!(
                "{}/import/01794572414669-4049c720",
                state::table_arn(REGION, "NeverCreated")
            ),
        ),
        (
            "an import id of one character",
            format!("{}/import/x", state::table_arn(REGION, "NeverCreated")),
        ),
        (
            "an import id of the longest shape this service mints",
            format!(
                "{}/import/{}-{}",
                state::table_arn(REGION, "NeverCreated"),
                "9".repeat(14),
                "f".repeat(8)
            ),
        ),
    ];

    for (case, addressed) in cases {
        let (status, answered) = call(
            &router,
            "DescribeImport",
            json!({"ImportArn": addressed.clone()}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {addressed}, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ImportNotFoundException",
            "case {case:?}: expected the modelled miss, got {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Import not found: Import with arn: {addressed} doesn't exist"),
            "case {case:?}: expected the sentence that class is stated in, got {answered}"
        );
    }
}
