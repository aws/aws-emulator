// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the import surface: `ImportTable` and the reader it round-trips
//! through, `DescribeImport` (with `ListImports` as the second reader).
//!
//! The rules pinned here:
//!
//! 1. an accepted import CREATES the table its `TableCreationParameters` declare and
//!    writes into it the items the objects its `S3BucketSource` selects carry, decoded in
//!    the `InputFormat` and `InputCompressionType` the request names — so the items the
//!    new table serves are the source's own, an object outside the named key prefix is not
//!    read, and a record the source carries in a form the table cannot hold is counted as
//!    an error instead of stored, which leaves the import FAILED over the items that did
//!    pass;
//! 2. the import is RECORDED, and every member of the description is read from that
//!    record: it reports `IN_PROGRESS` with no end instant and no counters for the
//!    transition span and the status the task settled into afterwards, together with the
//!    instant it ended and what it processed — and the table it created becomes `ACTIVE`
//!    on the same transition, because its data is the import's;
//! 3. a `ClientToken` makes the call idempotent: the same token with the same parameters
//!    answers the same import rather than starting a second one, and the same token with
//!    any other parameter is refused with `ImportConflictException`;
//! 4. an import creates a table, so what its target already IS decides the answer: a
//!    target that is already a table is refused with `ResourceInUseException`, while a
//!    target another import is still filling is not yet a table — that call is accepted
//!    and its own task fails with `TableAlreadyExists`, leaving the table untouched — and
//!    a declaration `CreateTable` would refuse is refused up front, in the same words;
//! 5. a source the object store holds no bucket for leaves the task `FAILED`, with the
//!    reason it failed;
//! 6. the readers answer from that one record: `DescribeImport` reports it by its
//!    `ImportArn`, refuses an ARN no import of this endpoint is held under with
//!    `ImportNotFoundException` and a value that is no import ARN at all with a
//!    `ValidationException`, and `ListImports` lists it.
//!
//! Every DynamoDB call is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, with Amazon S3 mounted beside it
//! on ONE engine and the source objects written through its OWN client, because an import
//! can only read objects that service really holds. Each rule is asserted over a SET of
//! inputs — the format, the compression, the key schema, the prefix, the member the
//! replayed token changed — rather than one example.

use std::io::Write;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state;

use crate::emulator::{
    call, call_in, describe_when_active, emulator_with_s3, error_code, message, ok, put_object,
    REGION,
};

/// A second region, for the rule that an import record is addressable only from the
/// endpoint whose region it was started in.
const OTHER_REGION: &str = "eu-west-1";

/// The bucket a case that is not about the bucket imports from.
const BUCKET: &str = "table-imports";

/// Wait out one transition span, so the `IN_PROGRESS` -> terminal transition the clock
/// owes a started import is due.
async fn settle() {
    tokio::time::sleep(state::STATUS_TRANSITION + Duration::from_millis(250)).await;
}

/// `bytes` GZIP-compressed, which is what a `GZIP` source object holds.
fn gzipped(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).expect("the encoder takes the bytes");
    encoder.finish().expect("the encoder yields the frame")
}

/// `bytes` in a Zstandard frame carrying them as ONE raw block, which is what a `ZSTD`
/// source object may hold.
///
/// Built here rather than compressed, because the rule under test is that the import
/// READS a Zstandard source: the frame's own layout is the published one — the magic
/// number `0xFD2FB528`, a frame header descriptor with no content size, dictionary id or
/// checksum, a window descriptor, and a last block whose 3-byte header carries
/// `(size << 3) | (type << 1) | last` with type 0 for a raw block (RFC 8878 §3.1.1).
fn zstd_raw_frame(bytes: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x28];
    let header = (u32::try_from(bytes.len()).expect("the block fits a frame") << 3) | 1;
    frame.extend_from_slice(&header.to_le_bytes()[..3]);
    frame.extend_from_slice(bytes);
    frame
}

/// One source object's bytes in the DynamoDB JSON form a table export writes: one item
/// per line, each under the `Item` member.
fn dynamodb_json_lines(items: &[Value]) -> Vec<u8> {
    items
        .iter()
        .map(|item| format!("{}\n", json!({"Item": item})))
        .collect::<String>()
        .into_bytes()
}

/// The `ImportTableDescription` of a response.
fn described(body: &Value) -> Value {
    body["ImportTableDescription"].clone()
}

/// The `ImportArn` of a response that carries an import description.
fn import_arn(body: &Value) -> String {
    described(body)["ImportArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A counter of a description, which the wire leaves out while it still stands at its
/// default of zero.
fn counter(description: &Value, member: &str) -> i64 {
    description[member].as_i64().unwrap_or_default()
}

/// One `TableCreationParameters` with a single `HASH` key of type `S`, for a case that is
/// not about the table's shape.
fn simple_parameters(name: &str) -> Value {
    json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    })
}

/// One import request over [`BUCKET`], in the DynamoDB JSON format, into `name`.
fn simple_request(name: &str) -> Value {
    json!({
        "S3BucketSource": {"S3Bucket": BUCKET},
        "InputFormat": "DYNAMODB_JSON",
        "TableCreationParameters": simple_parameters(name),
    })
}

/// The emulator with [`BUCKET`] already created through Amazon S3's own API, which an
/// import needs in order to have somewhere to read from.
async fn emulator_with_source() -> MultiServiceRouter {
    let router = emulator_with_s3();
    crate::emulator::create_bucket(&router, BUCKET).await;
    router
}

/// One source case: the format and compression the request names, the objects written
/// under the bucket, and the items the import must end up holding.
struct SourceCase {
    case: &'static str,
    input_format: &'static str,
    compression: Option<&'static str>,
    format_options: Option<Value>,
    parameters: Value,
    /// The objects written into the bucket: key and bytes.
    objects: Vec<(String, Vec<u8>)>,
    /// The key prefix the request narrows the source to, where it names one.
    prefix: Option<&'static str>,
    /// The keys the imported table must serve, and the item each must read back as.
    expected: Vec<(Value, Value)>,
    /// What the task must report having processed: items read, items written, errors.
    processed: i64,
    imported: i64,
    errors: i64,
}

#[tokio::test]
async fn an_import_creates_the_declared_table_and_writes_what_the_source_carried() {
    // Rows differ along what the rule generalises over: the source format, the
    // compression it is stored with, the shape of the key the table declares, whether the
    // request narrows the source with a prefix, and whether a record is one the table can
    // hold at all. The two format catalogues are the model's own (`InputFormat`:
    // CSV | DYNAMODB_JSON | ION, `InputCompressionType`: GZIP | NONE | ZSTD;
    // `smithy_query(query="shape",shape="InputFormat,InputCompressionType")`), and ION is
    // the one value no source is read in yet.
    let json_items = [
        json!({"Part": {"S": "alpha"}, "Size": {"N": "1"}}),
        json!({"Part": {"S": "beta"}, "Tags": {"SS": ["x", "y"]}}),
    ];
    let composite_parameters = json!({
        "TableName": "Composite",
        "KeySchema": [
            {"AttributeName": "Part", "KeyType": "HASH"},
            {"AttributeName": "Sort", "KeyType": "RANGE"},
        ],
        "AttributeDefinitions": [
            {"AttributeName": "Part", "AttributeType": "S"},
            {"AttributeName": "Sort", "AttributeType": "N"},
        ],
        "BillingMode": "PAY_PER_REQUEST",
    });
    let numeric_parameters = json!({
        "TableName": "Numbered",
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "N"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    let cases: Vec<SourceCase> = vec![
        SourceCase {
            case: "DynamoDB JSON, uncompressed, one HASH key",
            input_format: "DYNAMODB_JSON",
            compression: None,
            format_options: None,
            parameters: simple_parameters("Plain"),
            objects: vec![("plain/data.json".to_string(), dynamodb_json_lines(&json_items))],
            prefix: None,
            expected: vec![
                (json!({"Part": {"S": "alpha"}}), json_items[0].clone()),
                (json!({"Part": {"S": "beta"}}), json_items[1].clone()),
            ],
            processed: 2,
            imported: 2,
            errors: 0,
        },
        SourceCase {
            case: "DynamoDB JSON, GZIP, composite key, items without the Item envelope",
            input_format: "DYNAMODB_JSON",
            compression: Some("GZIP"),
            format_options: None,
            parameters: composite_parameters.clone(),
            objects: vec![(
                "gz/data.json.gz".to_string(),
                gzipped(
                    b"{\"Part\": {\"S\": \"alpha\"}, \"Sort\": {\"N\": \"1\"}}\n\
                      {\"Part\": {\"S\": \"alpha\"}, \"Sort\": {\"N\": \"2\"}}\n",
                ),
            )],
            prefix: None,
            expected: vec![
                (
                    json!({"Part": {"S": "alpha"}, "Sort": {"N": "1"}}),
                    json!({"Part": {"S": "alpha"}, "Sort": {"N": "1"}}),
                ),
                (
                    json!({"Part": {"S": "alpha"}, "Sort": {"N": "2"}}),
                    json!({"Part": {"S": "alpha"}, "Sort": {"N": "2"}}),
                ),
            ],
            processed: 2,
            imported: 2,
            errors: 0,
        },
        SourceCase {
            case: "DynamoDB JSON, ZSTD, one HASH key",
            input_format: "DYNAMODB_JSON",
            compression: Some("ZSTD"),
            format_options: None,
            parameters: simple_parameters("Zstandard"),
            objects: vec![(
                "zstd/data.json.zst".to_string(),
                zstd_raw_frame(&dynamodb_json_lines(&json_items[..1])),
            )],
            prefix: None,
            expected: vec![(json!({"Part": {"S": "alpha"}}), json_items[0].clone())],
            processed: 1,
            imported: 1,
            errors: 0,
        },
        SourceCase {
            case: "CSV with the file's own header row, a numeric key",
            input_format: "CSV",
            compression: None,
            format_options: None,
            parameters: numeric_parameters.clone(),
            objects: vec![(
                "csv/data.csv".to_string(),
                b"Id,Label,Note\n7,seven,\n8,\"eight, really\",kept\n".to_vec(),
            )],
            prefix: None,
            expected: vec![
                // Every column but the declared key is a string, and an empty cell is no
                // attribute at all.
                (
                    json!({"Id": {"N": "7"}}),
                    json!({"Id": {"N": "7"}, "Label": {"S": "seven"}}),
                ),
                (
                    json!({"Id": {"N": "8"}}),
                    json!({
                        "Id": {"N": "8"},
                        "Label": {"S": "eight, really"},
                        "Note": {"S": "kept"},
                    }),
                ),
            ],
            processed: 2,
            imported: 2,
            errors: 0,
        },
        SourceCase {
            case: "CSV with a named header list and a semicolon delimiter",
            input_format: "CSV",
            compression: Some("GZIP"),
            format_options: Some(json!({"Csv": {
                "Delimiter": ";",
                "HeaderList": ["Part", "Label"],
            }})),
            parameters: simple_parameters("Headered"),
            objects: vec![(
                "named/data.csv.gz".to_string(),
                gzipped(b"first;one\nsecond;two\n"),
            )],
            prefix: None,
            expected: vec![
                (
                    json!({"Part": {"S": "first"}}),
                    json!({"Part": {"S": "first"}, "Label": {"S": "one"}}),
                ),
                (
                    json!({"Part": {"S": "second"}}),
                    json!({"Part": {"S": "second"}, "Label": {"S": "two"}}),
                ),
            ],
            processed: 2,
            imported: 2,
            errors: 0,
        },
        SourceCase {
            case: "a prefix that selects one of two objects",
            input_format: "DYNAMODB_JSON",
            compression: None,
            format_options: None,
            parameters: simple_parameters("Prefixed"),
            objects: vec![
                (
                    "wanted/data.json".to_string(),
                    dynamodb_json_lines(&json_items[..1]),
                ),
                (
                    "other/data.json".to_string(),
                    dynamodb_json_lines(&json_items[1..]),
                ),
            ],
            prefix: Some("wanted/"),
            expected: vec![(json!({"Part": {"S": "alpha"}}), json_items[0].clone())],
            processed: 1,
            imported: 1,
            errors: 0,
        },
        SourceCase {
            case: "records the table cannot hold are counted as errors",
            input_format: "DYNAMODB_JSON",
            compression: None,
            format_options: None,
            parameters: simple_parameters("Partial"),
            objects: vec![(
                "errors/data.json".to_string(),
                [
                    // One item the table can hold, one line that is not JSON at all, one
                    // item that declares no key attribute, one carrying a value that is
                    // no attribute value, and one past the documented maximum item size
                    // (400 KB — "Service, Account, and Table Quotas").
                    "{\"Item\": {\"Part\": {\"S\": \"alpha\"}}}".to_string(),
                    "not json at all".to_string(),
                    "{\"Item\": {\"Other\": {\"S\": \"no key\"}}}".to_string(),
                    "{\"Item\": {\"Part\": {\"S\": \"beta\"}, \"Bad\": {\"N\": \"wat\"}}}"
                        .to_string(),
                    json!({"Item": {
                        "Part": {"S": "huge"},
                        "Blob": {"S": "x".repeat(400 * 1024 + 1)},
                    }})
                    .to_string(),
                ]
                .join("\n")
                .into_bytes(),
            )],
            prefix: None,
            expected: vec![(
                json!({"Part": {"S": "alpha"}}),
                json!({"Part": {"S": "alpha"}}),
            )],
            processed: 5,
            imported: 1,
            errors: 4,
        },
    ];

    for source in cases {
        let case = source.case;
        let router = emulator_with_source().await;
        let mut source_bytes = 0i64;
        for (key, body) in &source.objects {
            source_bytes += i64::try_from(body.len()).expect("the object is sized");
            put_object(&router, BUCKET, key, body.clone()).await;
        }
        // The bytes the task reports processed are only the objects the prefix selects.
        let selected_bytes: i64 = source
            .objects
            .iter()
            .filter(|(key, _)| source.prefix.is_none_or(|prefix| key.starts_with(prefix)))
            .map(|(_, body)| i64::try_from(body.len()).expect("the object is sized"))
            .sum();
        assert!(
            selected_bytes <= source_bytes,
            "case {case:?}: the selected bytes cannot exceed what was written"
        );

        let name = source.parameters["TableName"]
            .as_str()
            .expect("every row names its table")
            .to_string();
        let mut request = json!({
            "S3BucketSource": {"S3Bucket": BUCKET},
            "InputFormat": source.input_format,
            "TableCreationParameters": source.parameters.clone(),
        });
        if let Some(compression) = source.compression {
            request["InputCompressionType"] = json!(compression);
        }
        if let Some(options) = source.format_options.clone() {
            request["InputFormatOptions"] = options;
        }
        if let Some(prefix) = source.prefix {
            request["S3BucketSource"]["S3KeyPrefix"] = json!(prefix);
        }

        let started = ok(&router, "ImportTable", request.clone()).await;
        let description = described(&started);
        assert_eq!(
            description["ImportStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a started import is IN_PROGRESS, got {started}"
        );
        assert_eq!(
            description["TableArn"],
            json!(state::table_arn(REGION, &name)),
            "case {case:?}: the description names the table it creates, got {started}"
        );
        assert_eq!(
            description["InputFormat"],
            json!(source.input_format),
            "case {case:?}: the description reports the format asked for, got {started}"
        );
        assert_eq!(
            description["InputCompressionType"],
            json!(source.compression.unwrap_or("NONE")),
            "case {case:?}: the compression is reported as resolved, got {started}"
        );
        assert_eq!(
            description["TableCreationParameters"]["TableName"],
            json!(name),
            "case {case:?}: the declaration is reported back, got {started}"
        );
        assert_eq!(
            description["EndTime"],
            Value::Null,
            "case {case:?}: a running import reports no EndTime, got {started}"
        );
        assert_eq!(
            counter(&description, "ProcessedItemCount"),
            0,
            "case {case:?}: a running import has counted nothing yet, got {started}"
        );

        // The table the import creates settles with it, and only then does it serve the
        // items the source carried.
        let table = describe_when_active(&router, case, &name).await;
        assert_eq!(
            table["ItemCount"].as_i64().unwrap_or_default(),
            i64::try_from(source.expected.len()).expect("the row is sized"),
            "case {case:?}: the imported table holds the items the source carried, got {table}"
        );
        for (key, item) in &source.expected {
            let read = ok(
                &router,
                "GetItem",
                json!({"TableName": name, "Key": key, "ConsistentRead": true}),
            )
            .await;
            assert_eq!(
                read["Item"], *item,
                "case {case:?}: the imported table serves {key} as the source carried it, \
                 got {read}"
            );
        }

        // And the record reports what the task made of the source: an import that could
        // not write every item it read ends FAILED with the validation those items failed,
        // while the items that passed are in the table either way.
        let arn = import_arn(&started);
        let reported = described(&ok(&router, "DescribeImport", json!({"ImportArn": arn})).await);
        let failed = source.errors > 0;
        assert_eq!(
            reported["ImportStatus"],
            json!(if failed { "FAILED" } else { "COMPLETED" }),
            "case {case:?}: an import of {} items with {} errors must report that status, got \
             {reported}",
            source.imported,
            source.errors
        );
        assert_eq!(
            counter(&reported, "ProcessedItemCount"),
            source.processed,
            "case {case:?}: expected {} records processed, got {reported}",
            source.processed
        );
        assert_eq!(
            counter(&reported, "ImportedItemCount"),
            source.imported,
            "case {case:?}: expected {} items imported, got {reported}",
            source.imported
        );
        assert_eq!(
            counter(&reported, "ErrorCount"),
            source.errors,
            "case {case:?}: expected {} errors, got {reported}",
            source.errors
        );
        assert_eq!(
            reported["ProcessedSizeBytes"].as_i64(),
            Some(selected_bytes),
            "case {case:?}: expected the selected source bytes processed, got {reported}"
        );
        assert_eq!(
            reported["FailureCode"],
            if failed {
                json!("ItemValidationError")
            } else {
                Value::Null
            },
            "case {case:?}: the failure reported follows whether every item could be \
             written, got {reported}"
        );
    }
}

#[tokio::test]
async fn an_import_in_the_amazon_ion_format_is_refused_as_unimplemented() {
    let router = emulator_with_source().await;
    put_object(&router, BUCKET, "ion/data.ion", b"{}".to_vec()).await;
    let mut request = simple_request("Ionic");
    request["InputFormat"] = json!("ION");
    let (status, answered) = call(&router, "ImportTable", request).await;
    assert_eq!(
        status, 501,
        "an import no reader decodes is refused as not written yet, got {status} with {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "NotImplemented",
        "the refusal is the scaffold channel's, got {answered}"
    );
}

#[tokio::test]
async fn the_record_reports_the_import_and_both_readers_answer_from_it() {
    let router = emulator_with_source().await;
    put_object(
        &router,
        BUCKET,
        "one/data.json",
        dynamodb_json_lines(&[json!({"Part": {"S": "alpha"}})]),
    )
    .await;
    let started = ok(
        &router,
        "ImportTable",
        json!({
            "S3BucketSource": {"S3Bucket": BUCKET, "S3KeyPrefix": "one/"},
            "InputFormat": "DYNAMODB_JSON",
            "ClientToken": "token-for-one",
            "TableCreationParameters": simple_parameters("Recorded"),
        }),
    )
    .await;
    let arn = import_arn(&started);
    let start_time = described(&started)["StartTime"]
        .as_f64()
        .expect("a started import reports its StartTime");
    assert_eq!(
        described(&started)["ClientToken"],
        json!("token-for-one"),
        "the record carries the token the request was made under, got {started}"
    );

    // While it runs: the same answer from the single reader and the listing.
    let running = described(&ok(&router, "DescribeImport", json!({"ImportArn": arn})).await);
    assert_eq!(
        running["ImportStatus"],
        json!("IN_PROGRESS"),
        "the reader reports the import as running, got {running}"
    );
    let listed = ok(&router, "ListImports", json!({})).await;
    assert_eq!(
        listed["ImportSummaryList"][0]["ImportArn"],
        json!(arn),
        "the listing reports the import the call started, got {listed}"
    );
    assert_eq!(
        listed["ImportSummaryList"][0]["ImportStatus"],
        json!("IN_PROGRESS"),
        "the two readers report one status, got {listed}"
    );
    assert_eq!(
        listed["ImportSummaryList"][0]["EndTime"],
        Value::Null,
        "a running import reports no EndTime to either reader, got {listed}"
    );

    settle().await;
    let finished = described(&ok(&router, "DescribeImport", json!({"ImportArn": arn})).await);
    assert_eq!(
        finished["ImportStatus"],
        json!("COMPLETED"),
        "the import settles into the status its task reached, got {finished}"
    );
    let end_time = finished["EndTime"]
        .as_f64()
        .expect("a finished import reports its EndTime");
    assert!(
        (end_time - start_time - state::STATUS_TRANSITION.as_secs_f64()).abs() < 0.5,
        "the import ends one transition span after it began: started {start_time}, ended \
         {end_time}"
    );
    let listed = ok(&router, "ListImports", json!({})).await;
    assert_eq!(
        listed["ImportSummaryList"][0]["ImportStatus"],
        json!("COMPLETED"),
        "the listing settles with the single reader, got {listed}"
    );
    assert_eq!(
        listed["ImportSummaryList"][0]["EndTime"].as_f64(),
        Some(end_time),
        "both readers report one end instant, got {listed}"
    );
}

#[tokio::test]
async fn a_client_token_makes_the_call_idempotent() {
    let router = emulator_with_source().await;
    put_object(
        &router,
        BUCKET,
        "idem/data.json",
        dynamodb_json_lines(&[json!({"Part": {"S": "alpha"}})]),
    )
    .await;
    let mut request = simple_request("Idempotent");
    request["ClientToken"] = json!("replayed-token");
    request["S3BucketSource"]["S3KeyPrefix"] = json!("idem/");
    let first = ok(&router, "ImportTable", request.clone()).await;

    // An identical replay answers the import the token already started.
    let replayed = ok(&router, "ImportTable", request.clone()).await;
    assert_eq!(
        import_arn(&replayed),
        import_arn(&first),
        "an identical replay answers the same import, got {replayed}"
    );
    let listed = ok(&router, "ListImports", json!({})).await;
    assert_eq!(
        listed["ImportSummaryList"].as_array().map(Vec::len),
        Some(1),
        "an identical replay starts no second import, got {listed}"
    );

    // Rows differ in WHICH other member the replay changed.
    let mut other_table = request.clone();
    other_table["TableCreationParameters"] = simple_parameters("Different");
    let mut other_prefix = request.clone();
    other_prefix["S3BucketSource"]["S3KeyPrefix"] = json!("elsewhere/");
    let mut other_format = request.clone();
    other_format["InputFormat"] = json!("CSV");
    let mut other_compression = request.clone();
    other_compression["InputCompressionType"] = json!("GZIP");
    let mut other_options = request.clone();
    other_options["InputFormatOptions"] = json!({"Csv": {"Delimiter": ";"}});
    let mut other_key = request.clone();
    other_key["TableCreationParameters"]["KeySchema"] =
        json!([{"AttributeName": "Other", "KeyType": "HASH"}]);
    other_key["TableCreationParameters"]["AttributeDefinitions"] =
        json!([{"AttributeName": "Other", "AttributeType": "S"}]);
    let cases: [(&str, Value); 6] = [
        ("another table", other_table),
        ("another key prefix", other_prefix),
        ("another input format", other_format),
        ("another compression", other_compression),
        ("other format options", other_options),
        ("another key schema", other_key),
    ];

    for (case, changed) in cases {
        let (status, answered) = call(&router, "ImportTable", changed).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ImportConflictException",
            "case {case:?}: a changed replay is the modelled conflict, got {answered}"
        );
        assert_eq!(
            message(&answered),
            "Import conflict: Duplicate request detected with conflicting parameters",
            "case {case:?}: the refusal states the conflict it is, got {answered}"
        );
    }

    // And the conflict started nothing.
    let listed = ok(&router, "ListImports", json!({})).await;
    assert_eq!(
        listed["ImportSummaryList"].as_array().map(Vec::len),
        Some(1),
        "a refused replay records no import, got {listed}"
    );
}

#[tokio::test]
async fn a_target_that_is_already_a_table_is_refused_and_one_still_being_imported_is_not() {
    let router = emulator_with_source().await;
    put_object(
        &router,
        BUCKET,
        "busy/data.json",
        dynamodb_json_lines(&[json!({"Part": {"S": "imported"}})]),
    )
    .await;

    // Rows differ in HOW the target came to be a table: created outright, created and
    // settled, and created by an import that has finished. Each is a table, so each is
    // refused — whatever else the request asks for, since what collides is the target.
    ok(&router, "CreateTable", simple_parameters("Created")).await;
    ok(&router, "CreateTable", simple_parameters("Settled")).await;
    describe_when_active(&router, "a settled table", "Settled").await;
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Settled", "Item": {"Part": {"S": "its own"}}}),
    )
    .await;
    let mut imported = simple_request("Imported");
    imported["S3BucketSource"]["S3KeyPrefix"] = json!("busy/");
    ok(&router, "ImportTable", imported.clone()).await;
    settle().await;
    describe_when_active(&router, "a table an import created", "Imported").await;

    let mut other_format = simple_request("Settled");
    other_format["InputFormat"] = json!("CSV");
    let cases: [(&str, &str, Value); 4] = [
        ("a table still being created", "Created", simple_request("Created")),
        ("a settled table", "Settled", simple_request("Settled")),
        ("a settled table, another format", "Settled", other_format),
        (
            "a table a finished import created",
            "Imported",
            simple_request("Imported"),
        ),
    ];
    for (case, name, request) in cases {
        let (status, answered) = call(&router, "ImportTable", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ResourceInUseException",
            "case {case:?}: a target that is already a table is in use, got {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Table already exists: {name}"),
            "case {case:?}: the refusal states the name is taken, got {answered}"
        );
    }
    let listed = ok(&router, "ListImports", json!({})).await;
    assert_eq!(
        listed["ImportSummaryList"].as_array().map(Vec::len),
        Some(1),
        "a refused import records nothing, got {listed}"
    );

    // A target another import is STILL FILLING is not yet a table: that call is accepted,
    // and its own task is what fails on the name.
    let first = ok(&router, "ImportTable", simple_request("Racing")).await;
    let second = ok(&router, "ImportTable", simple_request("Racing")).await;
    assert_ne!(
        import_arn(&second),
        import_arn(&first),
        "the second import of a target still being filled is one of its own, got {second}"
    );
    assert_eq!(
        described(&second)["ImportStatus"],
        json!("IN_PROGRESS"),
        "that import is accepted and runs, got {second}"
    );
    assert_eq!(
        described(&second)["TableId"],
        Value::Null,
        "an import that created no table reports no TableId, got {second}"
    );

    settle().await;
    let winner = described(
        &ok(
            &router,
            "DescribeImport",
            json!({"ImportArn": import_arn(&first)}),
        )
        .await,
    );
    assert_eq!(
        winner["ImportStatus"],
        json!("COMPLETED"),
        "the import that took the name completes, got {winner}"
    );
    let loser = described(
        &ok(
            &router,
            "DescribeImport",
            json!({"ImportArn": import_arn(&second)}),
        )
        .await,
    );
    assert_eq!(
        loser["ImportStatus"],
        json!("FAILED"),
        "the import that did not take the name fails, got {loser}"
    );
    assert_eq!(
        loser["FailureCode"],
        json!("TableAlreadyExists"),
        "the record names the condition that failed it, got {loser}"
    );
    assert_eq!(
        loser["FailureMessage"],
        json!("Table already exists: Racing"),
        "the record states the name was taken, got {loser}"
    );
    assert_eq!(
        counter(&loser, "ProcessedItemCount"),
        0,
        "a task with nowhere to write reads nothing, got {loser}"
    );

    // And the table that held the name is exactly as it was: its own item is still served
    // and nothing of the source's was written into it.
    let read = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "Settled",
            "Key": {"Part": {"S": "its own"}},
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        read["Item"],
        json!({"Part": {"S": "its own"}}),
        "the table that held the name keeps its own item, got {read}"
    );
    let read = ok(
        &router,
        "GetItem",
        json!({
            "TableName": "Settled",
            "Key": {"Part": {"S": "imported"}},
            "ConsistentRead": true,
        }),
    )
    .await;
    assert_eq!(
        read.get("Item"),
        None,
        "a refused import writes nothing into the table that held the name, got {read}"
    );
}

#[tokio::test]
async fn a_declaration_a_create_would_refuse_is_refused_in_the_same_words() {
    let router = emulator_with_source().await;

    // Rows differ in HOW the declaration fails to be one a table can be created from —
    // each the same defect `CreateTable`'s own members are refused for.
    let mut undeclared_key = simple_parameters("Undeclared");
    undeclared_key["AttributeDefinitions"] = json!([{"AttributeName": "Other", "AttributeType": "S"}]);
    let mut unused_definition = simple_parameters("Unused");
    unused_definition["AttributeDefinitions"] = json!([
        {"AttributeName": "Part", "AttributeType": "S"},
        {"AttributeName": "Spare", "AttributeType": "S"},
    ]);
    let mut provisioned_without_throughput = simple_parameters("Provisioned");
    provisioned_without_throughput["BillingMode"] = json!("PROVISIONED");
    let mut on_demand_with_throughput = simple_parameters("OnDemand");
    on_demand_with_throughput["ProvisionedThroughput"] =
        json!({"ReadCapacityUnits": 1, "WriteCapacityUnits": 1});
    let mut sort_key_first = simple_parameters("Sorted");
    sort_key_first["KeySchema"] = json!([
        {"AttributeName": "Part", "KeyType": "RANGE"},
        {"AttributeName": "Sort", "KeyType": "HASH"},
    ]);
    sort_key_first["AttributeDefinitions"] = json!([
        {"AttributeName": "Part", "AttributeType": "S"},
        {"AttributeName": "Sort", "AttributeType": "S"},
    ]);
    let cases: [(&str, Value); 5] = [
        ("a key attribute with no definition", undeclared_key),
        ("a definition no key schema uses", unused_definition),
        (
            "the provisioned mode with no throughput",
            provisioned_without_throughput,
        ),
        ("the on-demand mode with a throughput", on_demand_with_throughput),
        ("a key schema that leads with its sort key", sort_key_first),
    ];

    for (case, parameters) in cases {
        let name = parameters["TableName"].clone();
        let mut request = simple_request("Ignored");
        request["TableCreationParameters"] = parameters.clone();
        let (status, answered) = call(&router, "ImportTable", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: a declaration no table can be created from is a bad request, \
             got {answered}"
        );
        // And the same defect, carried by `CreateTable`'s own members, is refused with the
        // same sentence: one rule, stated once.
        let (create_status, created) = call(&router, "CreateTable", parameters).await;
        assert_eq!(
            create_status, 400,
            "case {case:?}: the create refuses the same declaration, got {create_status} \
             with {created}"
        );
        assert_eq!(
            message(&answered),
            message(&created),
            "case {case:?}: the import and the create state one rule in one sentence"
        );
        let (_, described_table) = call(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            error_code(&described_table),
            "ResourceNotFoundException",
            "case {case:?}: a refused declaration creates no table, got {described_table}"
        );
    }
}

#[tokio::test]
async fn a_source_the_object_store_holds_no_bucket_for_fails_the_task() {
    // The bucket is NOT created here: an import of a source nothing holds is accepted and
    // its task is what fails, which is where the reason is reported.
    let router = emulator_with_s3();
    let started = ok(&router, "ImportTable", simple_request("Missing")).await;
    let arn = import_arn(&started);
    assert_eq!(
        described(&started)["ImportStatus"],
        json!("IN_PROGRESS"),
        "an import of a missing source is accepted and runs, got {started}"
    );

    settle().await;
    let reported = described(&ok(&router, "DescribeImport", json!({"ImportArn": arn})).await);
    assert_eq!(
        reported["ImportStatus"],
        json!("FAILED"),
        "a source nothing holds fails the task, got {reported}"
    );
    assert_eq!(
        reported["FailureCode"],
        json!("S3NoSuchBucket"),
        "the record names the condition that failed it, got {reported}"
    );
    assert_eq!(
        reported["FailureMessage"],
        json!("The specified bucket does not exist"),
        "the record states the reason, got {reported}"
    );
    assert_eq!(
        reported["ProcessedSizeBytes"],
        Value::Null,
        "a task that read nothing processed no bytes, got {reported}"
    );
    assert_eq!(
        counter(&reported, "ImportedItemCount"),
        0,
        "a task that read nothing imported nothing, got {reported}"
    );

    // The table the import declared was still created, and it holds nothing.
    let table = describe_when_active(&router, "a failed import", "Missing").await;
    assert_eq!(
        table["ItemCount"].as_i64().unwrap_or_default(),
        0,
        "the table an unreadable source created holds no item, got {table}"
    );
}

#[tokio::test]
async fn a_reader_answers_only_for_an_import_this_endpoint_holds() {
    let router = emulator_with_source().await;
    put_object(
        &router,
        BUCKET,
        "held/data.json",
        dynamodb_json_lines(&[json!({"Part": {"S": "alpha"}})]),
    )
    .await;
    let mut request = simple_request("Held");
    request["S3BucketSource"]["S3KeyPrefix"] = json!("held/");
    let arn = import_arn(&ok(&router, "ImportTable", request).await);

    // An import of this region is addressable here and nowhere else.
    let (status, answered) = call_in(&router, OTHER_REGION, "DescribeImport", json!({"ImportArn": arn.clone()})).await;
    assert_eq!(
        status, 400,
        "case another region: expected HTTP 400, got {status} with {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ValidationException",
        "case another region: an ARN of this region addresses no import there, got {answered}"
    );

    // A well-formed ARN of this endpoint that names no import is the modelled miss.
    let absent = format!(
        "{}/import/01700000000000-deadbeef",
        state::table_arn(REGION, "Held")
    );
    let (status, answered) = call(&router, "DescribeImport", json!({"ImportArn": absent.clone()})).await;
    assert_eq!(
        status, 400,
        "case an import never started: expected HTTP 400, got {status} with {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ImportNotFoundException",
        "case an import never started: expected the modelled miss, got {answered}"
    );
    assert_eq!(
        message(&answered),
        format!("Import not found: Import with arn: {absent} doesn't exist"),
        "case an import never started: the refusal echoes the ARN addressed, got {answered}"
    );

    // Rows differ in HOW the value fails to be an import ARN of this endpoint.
    let cases: [(&str, String); 4] = [
        (
            "an ARN of another service",
            format!(
                "{}/import/01700000000000-deadbeef",
                engine::emulator_arn("s3", REGION, "table/Held")
            ),
        ),
        (
            "an export of the same table",
            format!(
                "{}/export/01700000000000-deadbeef",
                state::table_arn(REGION, "Held")
            ),
        ),
        (
            "a table of another region",
            format!(
                "{}/import/01700000000000-deadbeef",
                state::table_arn(OTHER_REGION, "Held")
            ),
        ),
        (
            "an import of no table at all",
            format!(
                "arn:aws:dynamodb:{REGION}:{}:import/01700000000000-deadbeef",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (case, value) in cases {
        let (status, answered) =
            call(&router, "DescribeImport", json!({"ImportArn": value.clone()})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: a value that is no import ARN is a bad request, got {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Invalid ImportArn: Invalid ImportArn provided as input {value}"),
            "case {case:?}: the refusal states the member and echoes the value sent"
        );
    }
}
