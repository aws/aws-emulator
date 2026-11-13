// SPDX-License-Identifier: CC0-1.0

//! A stream's MAXIMUM RECORD SIZE, end to end over the wire: `CreateStream` ->
//! `UpdateMaxRecordSize` -> `DescribeStreamSummary` / `PutRecord`.
//!
//! RULE: `UpdateMaxRecordSize` STORES the `MaxRecordSizeInKiB` its request names on the
//! stream its `StreamARN` addresses, replacing whatever that stream carried — the value its
//! create named, or the documented default it fell back to — and answers the modelled empty
//! body. From then on that stored value is what a read of the stream reports and what every
//! write to it is measured against: a DATA BLOB of exactly the stored maximum is stored
//! whatever partition key travels with it, and one byte more fails that member's length the
//! way a modelled `@length` does. The change is scoped to the one stream the request names,
//! and setting the same value again changes nothing.
//!
//! The refusals are the ones that rule implies and the operation models: a request naming
//! no `StreamARN` names no stream, which is `InvalidArgumentException`; an ARN this account
//! and region hold no stream under is `ResourceNotFoundException`; a stream that is not
//! `ACTIVE` is not available for the change, which is `ResourceInUseException`; and a value
//! outside the published range is refused as a `ValidationException` with nothing stored.
//!
//! The recorded flows this was derived from update ONE provisioned stream and then write to
//! it, which is one row of the table below. The rows differ along what the rule generalises
//! over: the value the update names (both published bounds and a value between them), the
//! direction of the change (up from the minimum, down from the ceiling), whether the create
//! had already named a maximum, and the capacity mode of the stream.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.
//!
//! The wire harness and the ARN builder are [`crate::stream_tags`]', reused rather than
//! copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    data_too_long_message, put_record_data_path, put_records_data_path, stream_not_found_message,
    DEFAULT_MAX_RECORD_SIZE_KI_B, STREAM_ARN_REQUIRED_MESSAGE, STREAM_CREATING, STREAM_DELETING,
    STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};

use crate::stream_tags::{
    error_code, error_message, past_activation, poll_until_active, stream_arn, Emulator, REGION,
};

/// The published bounds of `MaxRecordSizeInKiB`, in KiB
/// (`smithy_query(query="operation",operation="UpdateMaxRecordSize")` -> `MaxRecordSizeInKiB`
/// range 1024..10240).
const MAX_RECORD_SIZE_BOUNDS: (i64, i64) = (1024, 10240);

/// How many bytes one KiB is, the unit `MaxRecordSizeInKiB` is expressed in.
const BYTES_PER_KI_B: usize = 1024;

/// The partition key every write of this suite carries. It is bounded by its OWN modelled
/// `@length` of 1..256 (`PutRecordInput$PartitionKey`) and is not charged against the
/// record-size ceiling, which bounds the data blob alone.
const PARTITION_KEY: &str = "pk";

/// One row of the update table: the stream the update lands on, and the value it names.
struct Row {
    label: &'static str,
    /// `StreamModeDetails$StreamMode` the create names.
    mode: &'static str,
    /// `MaxRecordSizeInKiB` the create names, `None` on a row whose create names none.
    created_with: Option<i64>,
    /// The `MaxRecordSizeInKiB` the update names.
    updated_to: i64,
}

/// The table the update rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape: a provisioned stream created without a maximum, updated to
        // the smallest value the member admits.
        Row {
            label: "max-record-size-minimum-onto-a-stream-that-named-none",
            mode: STREAM_MODE_PROVISIONED,
            created_with: None,
            updated_to: MAX_RECORD_SIZE_BOUNDS.0,
        },
        // Upwards, off a maximum the create had already named.
        Row {
            label: "max-record-size-doubled-from-the-minimum",
            mode: STREAM_MODE_PROVISIONED,
            created_with: Some(MAX_RECORD_SIZE_BOUNDS.0),
            updated_to: 2 * MAX_RECORD_SIZE_BOUNDS.0,
        },
        // A value strictly between the two published bounds.
        Row {
            label: "max-record-size-between-the-published-bounds",
            mode: STREAM_MODE_PROVISIONED,
            created_with: Some(2 * MAX_RECORD_SIZE_BOUNDS.0),
            updated_to: 4 * MAX_RECORD_SIZE_BOUNDS.0,
        },
        // The ceiling, on a stream whose shard count the service manages.
        Row {
            label: "max-record-size-ceiling-on-an-on-demand-stream",
            mode: STREAM_MODE_ON_DEMAND,
            created_with: None,
            updated_to: MAX_RECORD_SIZE_BOUNDS.1,
        },
        // Downwards: the new, smaller maximum is what a later write is measured against.
        Row {
            label: "max-record-size-down-to-the-minimum-from-the-ceiling",
            mode: STREAM_MODE_ON_DEMAND,
            created_with: Some(MAX_RECORD_SIZE_BOUNDS.1),
            updated_to: MAX_RECORD_SIZE_BOUNDS.0,
        },
    ]
}

/// The `CreateStream` body a row provisions its stream with.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "StreamName": row.label,
        "StreamModeDetails": { "StreamMode": row.mode },
    });
    // "For the data streams with a provisioned mode, you must specify the number of shards"
    // (`CreateStream`), while an on-demand stream's shard count is the service's to manage.
    if row.mode == STREAM_MODE_PROVISIONED {
        body["ShardCount"] = json!(1);
    }
    if let Some(created_with) = row.created_with {
        body["MaxRecordSizeInKiB"] = json!(created_with);
    }
    body
}

/// A `PutRecord` body whose DATA blob is `data_bytes` long, with a partition key on top.
fn write_of(stream_arn: &str, data_bytes: usize) -> Value {
    json!({
        "StreamARN": stream_arn,
        "PartitionKey": PARTITION_KEY,
        "Data": aws_smithy_types::base64::encode(vec![b'x'; data_bytes]),
    })
}

/// The `MaxRecordSizeInKiB` `DescribeStreamSummary` reports for the stream named `name`.
/// Every stream is configured for one, so a summary that carries no such member is a
/// failure rather than an absent value.
async fn reported_maximum(emulator: &Emulator, case: &str, name: &str) -> i64 {
    let summary = emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamName": name }),
        )
        .await;
    summary
        .get("StreamDescriptionSummary")
        .and_then(|summary| summary.get("MaxRecordSizeInKiB"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| {
            panic!("case {case}: the summary must report a MaxRecordSizeInKiB, got {summary}")
        })
}

/// The maximum the stream of `row` carries before anything updates it: what its create
/// named, else the default a create that names none falls back to.
fn created_maximum(row: &Row) -> i64 {
    row.created_with
        .unwrap_or(i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B))
}

/// The `StreamStatus` `DescribeStreamSummary` reports for the stream named `name`.
async fn reported_status(emulator: &Emulator, case: &str, name: &str) -> String {
    let summary = emulator
        .ok(
            case,
            "DescribeStreamSummary",
            &json!({ "StreamName": name }),
        )
        .await;
    summary
        .get("StreamDescriptionSummary")
        .and_then(|summary| summary.get("StreamStatus"))
        .and_then(Value::as_str)
        .unwrap_or("<no StreamStatus>")
        .to_string()
}

/// Whatever the update stored is what a later read reports and what a later write is
/// measured against, for every value the member admits and in both directions.
#[tokio::test]
async fn an_update_stores_the_maximum_every_later_read_and_write_uses() {
    let emulator = Emulator::new();
    for row in rows() {
        emulator
            .ok(row.label, "CreateStream", &create_body(&row))
            .await;
    }
    for row in rows() {
        poll_until_active(&emulator, row.label, row.label).await;
    }

    for row in rows() {
        let case = row.label;
        let arn = stream_arn(REGION, case);

        // What the create named is what the stream carries before the update, so the value
        // asserted after it is the one the UPDATE stored.
        assert_eq!(
            reported_maximum(&emulator, case, case).await,
            created_maximum(&row),
            "case {case}: before the update the stream carries what its create named, else \
             the default ({}), got {}",
            created_maximum(&row),
            reported_maximum(&emulator, case, case).await
        );

        let update = json!({ "StreamARN": arn, "MaxRecordSizeInKiB": row.updated_to });
        let (status, answered) = emulator.call("UpdateMaxRecordSize", &update).await;
        assert_eq!(
            (status, &answered),
            (200, &json!({})),
            "case {case}: an update of an ACTIVE stream answers 200 and the modelled empty \
             body, got {status} with body {answered}"
        );

        let read = reported_maximum(&emulator, case, case).await;
        assert_eq!(
            read,
            row.updated_to,
            "case {case}: expected the stored MaxRecordSizeInKiB {}, got {read:?}",
            row.updated_to
        );

        // A record whose data is exactly the stored maximum is stored, partition key on
        // top of it...
        let maximum = usize::try_from(row.updated_to).unwrap_or(0) * BYTES_PER_KI_B;
        let answer = emulator
            .ok(case, "PutRecord", &write_of(&arn, maximum))
            .await;
        assert!(
            answer
                .get("SequenceNumber")
                .and_then(Value::as_str)
                .is_some(),
            "case {case}: a record whose data is exactly {maximum} bytes is inside the stored \
             maximum, got {answer}"
        );

        // ...and a data blob one byte longer than the maximum is a validation failure of
        // that member's length. The row at the published ceiling has no room above it:
        // there the stream's maximum IS the modelled `@length` of `Data`, so a longer blob
        // is refused by the boundary validator rather than against the stored maximum,
        // which states the same code in the framework's own wording.
        let (status, response) = emulator
            .call("PutRecord", &write_of(&arn, maximum + 1))
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: a data blob of {} bytes is longer than the stored maximum, got \
             {status} with body {response}",
            maximum + 1
        );
        if row.updated_to < MAX_RECORD_SIZE_BOUNDS.1 {
            assert_eq!(
                error_message(&response),
                data_too_long_message(&[put_record_data_path()], maximum),
                "case {case}: expected the validation sentence a length bound is stated in, \
                 got {response}"
            );
        }

        // Setting the same value again leaves the stream exactly where it is.
        emulator.ok(case, "UpdateMaxRecordSize", &update).await;
        let read = reported_maximum(&emulator, case, case).await;
        assert_eq!(
            read,
            row.updated_to,
            "case {case}: re-applying {} changes nothing, got {read:?}",
            row.updated_to
        );
    }
}

/// The update is scoped to the ONE stream its ARN names: another stream of the same account
/// and region keeps the maximum it had, and its writes keep being measured against it.
#[tokio::test]
async fn an_update_changes_only_the_stream_it_names() {
    let emulator = Emulator::new();
    let case = "scoped";
    let updated = "max-record-size-scoped-updated";
    let untouched = "max-record-size-scoped-untouched";
    for name in [updated, untouched] {
        emulator
            .ok(
                case,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
    }
    for name in [updated, untouched] {
        poll_until_active(&emulator, case, name).await;
    }

    let enlarged = 2 * MAX_RECORD_SIZE_BOUNDS.0;
    emulator
        .ok(
            case,
            "UpdateMaxRecordSize",
            &json!({
                "StreamARN": stream_arn(REGION, updated),
                "MaxRecordSizeInKiB": enlarged,
            }),
        )
        .await;

    assert_eq!(
        reported_maximum(&emulator, case, untouched).await,
        i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B),
        "case {case}: the stream the update did not name keeps the default maximum its \
         create fell back to"
    );

    // The default maximum a stream that names none falls back to still bounds the untouched
    // stream, while the updated stream takes a data blob past it.
    let past_the_default =
        usize::try_from(DEFAULT_MAX_RECORD_SIZE_KI_B).unwrap_or(0) * BYTES_PER_KI_B + 1;
    let answer = emulator
        .ok(
            case,
            "PutRecord",
            &write_of(&stream_arn(REGION, updated), past_the_default),
        )
        .await;
    assert!(
        answer
            .get("SequenceNumber")
            .and_then(Value::as_str)
            .is_some(),
        "case {case}: the updated stream takes a record whose data is {past_the_default} \
         bytes, got {answer}"
    );

    let (status, response) = emulator
        .call(
            "PutRecord",
            &write_of(&stream_arn(REGION, untouched), past_the_default),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "ValidationException"),
        "case {case}: the untouched stream still refuses a data blob of {past_the_default} \
         bytes, got {status} with body {response}"
    );

    // The same bound holds for the other writer, which measures every entry of its batch
    // against the stream's maximum and names EVERY entry at fault by its position: one
    // clause for a batch with one oversize blob, two for a batch with two.
    let default_maximum = usize::try_from(DEFAULT_MAX_RECORD_SIZE_KI_B).unwrap_or(0) * BYTES_PER_KI_B;
    let oversize = json!({
        "PartitionKey": PARTITION_KEY,
        "Data": aws_smithy_types::base64::encode(vec![b'x'; past_the_default]),
    });
    let inside = json!({
        "PartitionKey": PARTITION_KEY,
        "Data": aws_smithy_types::base64::encode("x"),
    });
    // (case, the batch's entries, the positions whose Data is at fault)
    let batches = [
        ("one oversize entry", vec![inside.clone(), oversize.clone()], vec![2usize]),
        (
            "two oversize entries",
            vec![oversize.clone(), inside.clone(), oversize.clone()],
            vec![1usize, 3],
        ),
    ];
    for (batch_case, records, at_fault) in &batches {
        let (status, response) = emulator
            .call(
                "PutRecords",
                &json!({
                    "StreamARN": stream_arn(REGION, untouched),
                    "Records": records,
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}/{batch_case}: a batch entry past the stream's maximum is refused, \
             got {status} with body {response}"
        );
        let paths: Vec<String> = at_fault
            .iter()
            .map(|position| put_records_data_path(*position))
            .collect();
        assert_eq!(
            error_message(&response),
            data_too_long_message(&paths, default_maximum),
            "case {case}/{batch_case}: expected one clause per entry at fault, got {response}"
        );
    }
}

/// An ARN this account and region hold no stream under names nothing to update, and a
/// request naming no ARN at all names no stream.
#[tokio::test]
async fn a_request_naming_no_stream_of_this_account_and_region_is_refused() {
    let emulator = Emulator::new();
    let name = "max-record-size-refusals";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    // (case, the ARN the request names, the error code, the sentence it carries)
    let rows: Vec<(&str, Option<String>, &str, String)> = vec![
        (
            "a request naming no StreamARN",
            None,
            "InvalidArgumentException",
            STREAM_ARN_REQUIRED_MESSAGE.to_string(),
        ),
        (
            "a stream nothing created",
            Some(stream_arn(REGION, "max-record-size-never-created")),
            "ResourceNotFoundException",
            stream_not_found_message(&stream_arn(REGION, "max-record-size-never-created")),
        ),
        (
            "a stream of another region",
            Some(stream_arn("eu-west-1", name)),
            "ResourceNotFoundException",
            stream_not_found_message(&stream_arn("eu-west-1", name)),
        ),
        (
            // `StreamARN` is modelled `^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$`, so an ARN
            // naming another account's stream is well formed and names no stream of this
            // one.
            "a stream of another account",
            Some(format!(
                "arn:aws:kinesis:{REGION}:123456789012:stream/{name}"
            )),
            "ResourceNotFoundException",
            stream_not_found_message(&format!(
                "arn:aws:kinesis:{REGION}:123456789012:stream/{name}"
            )),
        ),
    ];

    for (case, arn, expected_code, expected_message) in &rows {
        let mut body = json!({ "MaxRecordSizeInKiB": 2 * MAX_RECORD_SIZE_BOUNDS.0 });
        if let Some(arn) = arn {
            body["StreamARN"] = json!(arn);
        }
        let (status, response) = emulator.call("UpdateMaxRecordSize", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            *expected_code,
            "case {case}: expected {expected_code}, got {response}"
        );
        assert_eq!(
            &error_message(&response),
            expected_message,
            "case {case}: expected the sentence this service states, got {response}"
        );
    }

    // The stream this account does hold is untouched by the refusals.
    assert_eq!(
        reported_maximum(&emulator, "after the refusals", name).await,
        i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B),
        "case after the refusals: a refused update stores nothing"
    );
}

/// "For successful operation, the resource must be in the ACTIVE state"
/// (`ResourceInUseException`): a stream still inside the transition its create armed, and
/// one a delete has been accepted for, are not available for the change.
#[tokio::test]
async fn an_update_of_a_stream_that_is_not_active_is_refused() {
    // (the stream's name, wait past the activation, delete first, the status the update
    // lands in)
    let cases = [
        ("max-record-size-creating", false, false, STREAM_CREATING),
        ("max-record-size-deleting", true, true, STREAM_DELETING),
    ];

    for (name, wait_for_activation, delete_first, expected_status) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                name,
                "CreateStream",
                &json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
        if wait_for_activation {
            tokio::time::sleep(past_activation()).await;
        }
        if delete_first {
            emulator
                .ok(name, "DeleteStream", &json!({ "StreamName": name }))
                .await;
        }

        let status_now = reported_status(&emulator, name, name).await;
        assert_eq!(
            status_now, expected_status,
            "case {name}: the stream is in {expected_status} when the update lands, got \
             {status_now}"
        );

        let arn = stream_arn(REGION, name);
        let (status, response) = emulator
            .call(
                "UpdateMaxRecordSize",
                &json!({ "StreamARN": arn, "MaxRecordSizeInKiB": MAX_RECORD_SIZE_BOUNDS.1 }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceInUseException"),
            "case {name}: an update of a {expected_status} stream is refused, got {status} \
             with body {response}"
        );
        assert_eq!(
            error_message(&response),
            // The sentence this service states for a stream that is not `ACTIVE`, written
            // out here rather than borrowed from the crate, so the wording is what this
            // case pins.
            format!(
                "Stream {name} under account {} not ACTIVE, instead in state \
                 {expected_status}",
                engine::EMULATOR_ACCOUNT_ID
            ),
            "case {name}: expected the not-active sentence this service states, got {response}"
        );
        assert_eq!(
            reported_maximum(&emulator, name, name).await,
            i64::from(DEFAULT_MAX_RECORD_SIZE_KI_B),
            "case {name}: a refused update stores nothing"
        );
    }
}

/// "Specify a value between 1024 and 10240 KiB (1 to 10 MiB). If you specify a value that is
/// out of this range, UpdateMaxRecordSize sends back an ValidationException message"
/// (`UpdateMaxRecordSizeInput$MaxRecordSizeInKiB`) — checked on each published bound, one
/// value past each of them, and a request that names the required member not at all.
#[tokio::test]
async fn a_value_outside_the_published_range_is_refused() {
    let emulator = Emulator::new();
    let name = "max-record-size-range";
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let arn = stream_arn(REGION, name);

    // Each published bound is accepted...
    for accepted in [MAX_RECORD_SIZE_BOUNDS.0, MAX_RECORD_SIZE_BOUNDS.1] {
        emulator
            .ok(
                name,
                "UpdateMaxRecordSize",
                &json!({ "StreamARN": arn, "MaxRecordSizeInKiB": accepted }),
            )
            .await;
        assert_eq!(
            reported_maximum(&emulator, name, name).await,
            accepted,
            "case {accepted}: a value on a published bound is stored"
        );
    }
    let stored = MAX_RECORD_SIZE_BOUNDS.1;

    // ...and one past either of them is not.
    let refused = [
        ("one below the published minimum", json!(MAX_RECORD_SIZE_BOUNDS.0 - 1)),
        ("one past the published maximum", json!(MAX_RECORD_SIZE_BOUNDS.1 + 1)),
        ("twice the published maximum", json!(2 * MAX_RECORD_SIZE_BOUNDS.1)),
        ("no size at all", json!(0)),
        ("a negative size", json!(-MAX_RECORD_SIZE_BOUNDS.0)),
    ];
    for (case, value) in &refused {
        let (status, response) = emulator
            .call(
                "UpdateMaxRecordSize",
                &json!({ "StreamARN": arn, "MaxRecordSizeInKiB": value }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {case}: a value outside the published range is refused, got {status} with \
             body {response}"
        );
    }

    // A request that names no value at all names nothing to store either.
    let (status, response) = emulator
        .call("UpdateMaxRecordSize", &json!({ "StreamARN": arn }))
        .await;
    assert_eq!(
        status, 400,
        "case no member: the required member must be named, got {status} with body {response}"
    );

    assert_eq!(
        reported_maximum(&emulator, name, name).await,
        stored,
        "case after the refusals: a refused update leaves the stored maximum as it was"
    );
}
