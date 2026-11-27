// SPDX-License-Identifier: CC0-1.0

//! The two preconditions `CreateChannel` takes on a general purpose bucket destination and on
//! the capacity mode of the stream it reads, end to end over the wire.
//!
//! RULE 1: the `OutputKeyTemplate` of a general purpose bucket destination "is used to
//! construct the Amazon S3 object key for delivered objects"
//! (`S3StorageConfiguration$OutputKeyTemplate`), so a value that renders no such key is
//! refused with the error this service answers for an input it validates before it provisions
//! anything (`ValidationException`, 400) and NO channel is created, each fault stating its own
//! sentence: the template "must not contain consecutive periods, period-only path segments, or
//! start with /", it "must not contain consecutive slashes (//)", it may carry no unclosed
//! variable placeholder, and a variable it names has to be one the service expands. A template
//! that renders a key is accepted and reported back as written.
//!
//! RULE 2: a channel is "only supported for data streams with the on-demand capacity mode"
//! (`CreateChannel`), so a create whose source stream is in any other mode is refused with
//! that same error, naming the stream by NAME under the account that holds it and the mode it
//! is in, and asking for the mode the feature needs.
//!
//! RULE 3: `DataFreshnessInSeconds` is ONE member
//! (`smithy_query(query="shape",shape="DataFreshnessInSeconds")` — a plain integer with no
//! `@range`) reached from every channel destination configuration and update, and the service
//! delivers only on the window that member documents, 300 to 900 seconds inclusive. So a
//! `CreateChannel` or an `UpdateChannel` naming a window outside it — whichever destination kind
//! carried the member — is refused with that same error naming the member and its two bounds,
//! creates no channel and moves no stored window, while a window on either bound or between
//! them is served and read back, and a create naming none takes the documented 300-second
//! default.
//!
//! The rows differ along what each rule generalises over: for rule 1 the position and the form
//! of the offending segment or expansion, and the forms a template may take inside the modelled
//! `@pattern`; for rule 2 the stream's name and the region the request is signed for; for rule 3
//! where the window sits relative to the documented range (under its lower end, on each
//! published bound, inside, over its upper end, zero, negative, absent), the destination kind
//! that carries the member, and whether the window is being set by a create or moved by an
//! update.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    capacity_mode_prose, channel_arn, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};

/// The sentences `CreateChannel` refuses a template that renders no object key with, written
/// out here rather than borrowed from the crate, so the wording is what this suite pins.
const INVALID_TEMPLATE_MESSAGE: &str =
    "OutputKeyTemplate must not contain consecutive periods, period-only path segments, or \
     start with /";
const CONSECUTIVE_SLASHES_MESSAGE: &str =
    "OutputKeyTemplate must not contain consecutive slashes (//)";
const UNCLOSED_VARIABLE_MESSAGE: &str =
    "OutputKeyTemplate contains an unclosed variable placeholder";

use crate::channel_lifecycle::{
    bucket_arn, dead_letter_queue, description, member, on_demand_stream,
    poll_until_channel_active, role_arn, schema_arn, storage_configuration, stream_configuration,
    table_bucket_arn,
};
use crate::stream_tags::{error_code, error_message, poll_until_active_in, stream_arn, Emulator, REGION};

/// The error this service answers for an input it validates before it provisions anything.
const VALIDATION: &str = "ValidationException";

/// A `CreateChannel` body delivering to a general purpose bucket, with `template` as the
/// destination's `OutputKeyTemplate` when a case names one.
fn create(name: &str, source: &str, template: Option<&str>) -> Value {
    let mut storage = storage_configuration("key-template-bucket");
    if let Some(template) = template {
        storage["OutputKeyTemplate"] = json!(template);
    }
    json!({
        "ChannelName": name,
        "ServiceExecutionRoleARN": role_arn("channel-delivery"),
        "StreamConfigurationList": [stream_configuration(source, "STRING", None)],
        "S3DestinationConfiguration": { "StorageConfiguration": storage },
    })
}

/// A template that renders an object key is taken as written; one that cannot is refused and
/// leaves no channel behind.
#[tokio::test]
async fn an_output_key_template_that_renders_no_object_key_is_refused() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "key-template-source").await;

    // (case, the template, the sentence it is refused with, or `None` when it renders a key).
    // Every accepted row is inside the modelled `@pattern` `^[0-9A-Za-z!\-_'.*()\/=:{}]+$`
    // (`smithy_query(query="shape", shape="com.amazonaws.kinesis#S3StorageConfiguration")`),
    // which is what the request walk already enforces.
    let rows: Vec<(&str, &str, Option<String>)> = vec![
        ("a bare name", "records", None),
        ("a variable this service expands", "records/!{yyyy}", None),
        (
            "the calendar variables and the record's own",
            "r/!{yyyy}/!{MM}/!{dd}/!{HH}/!{mm}/!{ss}/!{sequence-number}!{extension}",
            None,
        ),
        (
            "the channel's own name and id",
            "r/!{channel-name}/!{channel-id}",
            None,
        ),
        ("every other character the pattern admits", "a.b-c_d'e!f(g)=h:i", None),
        // A brace that opens no variable is text: a variable opens with `!{`.
        ("a brace that opens no variable", "records/{yyyy}", None),
        ("a trailing separator", "a/", None),
        (
            "consecutive periods in the middle",
            "a/../b",
            Some(INVALID_TEMPLATE_MESSAGE.to_string()),
        ),
        (
            "consecutive periods at the start",
            "../b",
            Some(INVALID_TEMPLATE_MESSAGE.to_string()),
        ),
        (
            "consecutive periods inside a segment",
            "a..b",
            Some(INVALID_TEMPLATE_MESSAGE.to_string()),
        ),
        (
            "a period-only segment",
            "a/./b",
            Some(INVALID_TEMPLATE_MESSAGE.to_string()),
        ),
        (
            "nothing but a period",
            ".",
            Some(INVALID_TEMPLATE_MESSAGE.to_string()),
        ),
        (
            "a leading separator",
            "/a",
            Some(INVALID_TEMPLATE_MESSAGE.to_string()),
        ),
        (
            "two separators in a row",
            "a//b",
            Some(CONSECUTIVE_SLASHES_MESSAGE.to_string()),
        ),
        (
            "two separators after a variable",
            "a/!{yyyy}//b",
            Some(CONSECUTIVE_SLASHES_MESSAGE.to_string()),
        ),
        (
            "a variable that is never closed",
            "a/!{yyyy",
            Some(UNCLOSED_VARIABLE_MESSAGE.to_string()),
        ),
        (
            "a variable this service does not expand",
            "records/!{partition-id}",
            Some("OutputKeyTemplate contains invalid variable: !{partition-id}".to_string()),
        ),
    ];

    for (index, (case, template, refusal)) in rows.iter().enumerate() {
        let name = format!("key-template-{index}");
        let (status, response) = emulator
            .call("CreateChannel", &create(&name, &source, Some(template)))
            .await;
        let Some(refusal) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {template:?} accepted, got {status} {response}"
            );
            let reported = response["ChannelDescription"]["S3DestinationConfiguration"]
                ["StorageConfiguration"]["OutputKeyTemplate"]
                .as_str();
            assert_eq!(
                reported,
                Some(*template),
                "case {case:?}: expected the template reported back as written, got {reported:?}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected {template:?} refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {response}"
        );
        assert_eq!(
            &error_message(&response),
            refusal,
            "case {case:?}: expected the refusal to state the fault at hand"
        );
        // A refused create provisions nothing, so the name it asked for is still free.
        let (status, read) = emulator
            .call("DescribeChannel", &json!({ "ChannelName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create must leave no channel, got {status} {read}"
        );
    }
}

/// A create whose source stream is not on-demand is refused by name, under the account that
/// holds the stream, in whichever region the request is signed for.
#[tokio::test]
async fn a_source_stream_outside_the_on_demand_mode_is_refused_by_name() {
    let emulator = Emulator::new();

    // (case, the region the request is signed for, the stream's name)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("the default region", REGION, "provisioned-source"),
        ("another region", "eu-west-1", "provisioned-source"),
        (
            "a name at the other end of the name class",
            REGION,
            "A.provisioned_stream-0",
        ),
    ];

    for (index, (case, region, stream)) in rows.iter().enumerate() {
        emulator
            .ok_in(
                case,
                region,
                "CreateStream",
                &json!({
                    "StreamName": stream,
                    "ShardCount": 1,
                    "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
                }),
            )
            .await;
        poll_until_active_in(&emulator, case, region, stream).await;
        let (status, response) = emulator
            .call_in(
                region,
                "CreateChannel",
                &create(
                    &format!("provisioned-{index}"),
                    &stream_arn(region, stream),
                    None,
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {response}"
        );
        let expected = format!(
            "Request is invalid. Stream {stream} under account {} is in {} mode. Please enable \
             {} mode to use this feature.",
            engine::EMULATOR_ACCOUNT_ID,
            capacity_mode_prose(STREAM_MODE_PROVISIONED),
            capacity_mode_prose(STREAM_MODE_ON_DEMAND),
        );
        assert_eq!(
            error_message(&response),
            expected,
            "case {case:?}: expected {expected:?}"
        );
    }
}

/// The two capacity modes the model declares, as a sentence spells each one.
#[tokio::test]
async fn a_capacity_mode_is_worded_as_a_sentence_spells_it() {
    // (the `StreamMode` value, the word a sentence uses). Source:
    // `smithy_query(query="shape", shape="com.amazonaws.kinesis#StreamMode")`.
    for (mode, word) in [
        (STREAM_MODE_PROVISIONED, "Provisioned"),
        (STREAM_MODE_ON_DEMAND, "On-demand"),
    ] {
        let spelled = capacity_mode_prose(mode);
        assert_eq!(
            spelled, word,
            "case {mode:?}: expected the mode worded {word:?}, got {spelled:?}"
        );
    }
}

/// The shortest data-freshness window a destination delivers on: the lower end of the range the
/// service documents for the member, which the model itself states no bound for.
const MIN_FRESHNESS_SECONDS: i64 = 300;

/// The longest window it delivers on: the upper end of that same documented range.
const MAX_FRESHNESS_SECONDS: i64 = 900;

/// The window a destination that names none gets: "The default value is 300 seconds"
/// (`S3DestinationConfiguration$DataFreshnessInSeconds`, worded identically on
/// `S3TablesDestinationConfiguration$DataFreshnessInSeconds`).
const DEFAULT_FRESHNESS_SECONDS: i64 = 300;

/// The status a channel reports once the transition its create or its last accepted change
/// armed has fallen due (`smithy_query(query="shape",shape="ChannelDescription")` ->
/// `ChannelStatus`).
const ACTIVE: &str = "ACTIVE";

/// The sentence the service refuses a window outside the range with. FIXED text around the
/// member's own two bounds, written out here rather than borrowed from the crate, so a drift
/// away from the wording fails rather than comparing the handler against itself.
const FRESHNESS_OUT_OF_RANGE: &str = "DataFreshnessInSeconds must be between 300 and 900.";

/// Which destination kind a case names, which decides the member the request and the
/// description carry the window under and the record format its source stream is read in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Delivery to a general purpose bucket.
    S3,
    /// Delivery to streaming tables on Apache Iceberg.
    S3Tables,
}

impl Kind {
    /// The request and description member this destination is carried under.
    fn member(self) -> &'static str {
        match self {
            Self::S3 => "S3DestinationConfiguration",
            Self::S3Tables => "S3TablesDestinationConfiguration",
        }
    }
}

/// A general purpose bucket destination, naming `freshness` as its window only where a case
/// states one.
fn s3_destination(bucket: &str, freshness: Option<i64>) -> Value {
    let mut destination = json!({ "StorageConfiguration": storage_configuration(bucket) });
    if let (Some(members), Some(freshness)) = (destination.as_object_mut(), freshness) {
        members.insert("DataFreshnessInSeconds".to_string(), json!(freshness));
    }
    destination
}

/// A streaming table destination delivering to one table, naming `freshness` as its window only
/// where a case states one.
fn tables_destination(table: &str, freshness: Option<i64>) -> Value {
    let mut destination = json!({
        "DeadLetterQueueS3Configuration": dead_letter_queue("freshness-dlq", "tables/"),
        "S3TablesConfigurationList": [{
            "TableBucketARN": table_bucket_arn("freshness-tables"),
            "Namespace": "events",
            "TableName": table,
            "CompressionType": "SNAPPY",
            "PartitionSpec": {
                "PartitionFields": [{ "Transform": "TIME_HOUR", "SourceName": "written_at" }],
            },
        }],
    });
    if let (Some(members), Some(freshness)) = (destination.as_object_mut(), freshness) {
        members.insert("DataFreshnessInSeconds".to_string(), json!(freshness));
    }
    destination
}

/// A `CreateChannel` body for a channel named `name` whose destination of `kind` names
/// `freshness` as its window.
fn create_with_freshness(kind: Kind, name: &str, source: &str, freshness: Option<i64>) -> Value {
    let (format, schema, destination) = match kind {
        Kind::S3 => (
            "JSON",
            None,
            s3_destination(&format!("bucket-{name}"), freshness),
        ),
        Kind::S3Tables => (
            "GSR_JSON",
            Some(schema_arn("records", "iceberg")),
            tables_destination(&name.replace('-', "_"), freshness),
        ),
    };
    let mut body = json!({
        "ChannelName": name,
        "ServiceExecutionRoleARN": role_arn("channel-delivery"),
        "StreamConfigurationList": [stream_configuration(source, format, schema)],
    });
    body[kind.member()] = destination;
    body
}

/// The window the `ChannelDescription` in `response` reports for a destination of `kind`.
fn reported_freshness(case: &str, kind: Kind, response: &Value) -> Option<i64> {
    description(case, response)[kind.member()]["DataFreshnessInSeconds"].as_i64()
}

/// The window the STORED channel `arn` reports for a destination of `kind`.
async fn stored_freshness(emulator: &Emulator, case: &str, kind: Kind, arn: &str) -> Option<i64> {
    let read = emulator
        .ok(case, "DescribeChannel", &json!({ "ChannelARN": arn }))
        .await;
    reported_freshness(case, kind, &read)
}

/// Assert that `response` is the refusal the member's range earns: HTTP 400, the error this
/// service answers for an input it validates before it provisions anything, and the sentence
/// naming the member and its two bounds.
fn assert_out_of_range(case: &str, status: u16, response: &Value) {
    assert_eq!(
        status, 400,
        "case {case}: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(response),
        VALIDATION,
        "case {case}: expected {VALIDATION}, got {response}"
    );
    let message = error_message(response);
    assert_eq!(
        message, FRESHNESS_OUT_OF_RANGE,
        "case {case}: expected the message {FRESHNESS_OUT_OF_RANGE:?}, got {message:?}"
    );
}

/// One row of the window table: the window a call names (absent where `None`) and the window the
/// channel must report, or `None` where the call must be refused.
struct Window {
    case: &'static str,
    freshness: Option<i64>,
    served: Option<i64>,
}

/// The window table, shared by the create cases and the update cases: the rows differ along
/// where the window sits relative to the documented range — under its lower end, on each
/// published bound, inside, over its upper end, zero, negative — and, for a create, absent
/// altogether.
fn windows() -> Vec<Window> {
    vec![
        Window {
            case: "one second under the minimum",
            freshness: Some(MIN_FRESHNESS_SECONDS - 1),
            served: None,
        },
        Window {
            case: "a zero window",
            freshness: Some(0),
            served: None,
        },
        Window {
            case: "a negative window",
            freshness: Some(-1),
            served: None,
        },
        Window {
            case: "one second over the maximum",
            freshness: Some(MAX_FRESHNESS_SECONDS + 1),
            served: None,
        },
        Window {
            case: "a day-long window",
            freshness: Some(86_400),
            served: None,
        },
        Window {
            case: "the minimum itself",
            freshness: Some(MIN_FRESHNESS_SECONDS),
            served: Some(MIN_FRESHNESS_SECONDS),
        },
        Window {
            case: "inside the range",
            freshness: Some(451),
            served: Some(451),
        },
        Window {
            case: "the maximum itself",
            freshness: Some(MAX_FRESHNESS_SECONDS),
            served: Some(MAX_FRESHNESS_SECONDS),
        },
        Window {
            case: "no window named, so the documented default",
            freshness: None,
            served: Some(DEFAULT_FRESHNESS_SECONDS),
        },
    ]
}

/// A create takes a destination's data-freshness window only inside the range the service
/// delivers on — whichever destination kind carries the member — and a refused create leaves
/// behind no channel at all.
#[tokio::test]
async fn a_create_serves_a_data_freshness_window_only_inside_the_documented_range() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "freshness-source").await;

    let mut served: Vec<(&'static str, Kind, String, i64)> = Vec::new();
    for kind in [Kind::S3, Kind::S3Tables] {
        for (index, row) in windows().iter().enumerate() {
            let case = format!("{kind:?}/{}", row.case);
            let name = match kind {
                Kind::S3 => format!("freshness-s3-{index}"),
                Kind::S3Tables => format!("freshness-tables-{index}"),
            };
            let (status, response) = emulator
                .call(
                    "CreateChannel",
                    &create_with_freshness(kind, &name, &source, row.freshness),
                )
                .await;
            let arn = channel_arn(REGION, &name);

            let Some(expected) = row.served else {
                assert_out_of_range(&case, status, &response);

                // The refusal is taken before anything is persisted, so the name it asked for
                // holds no channel.
                let (read_status, described) = emulator
                    .call("DescribeChannel", &json!({ "ChannelARN": arn }))
                    .await;
                assert_eq!(
                    read_status, 400,
                    "case {case}: expected the refused create to have stored no channel, got \
                     {read_status} with body {described}"
                );
                assert_eq!(
                    error_code(&described),
                    "ResourceNotFoundException",
                    "case {case}: expected the refused create to have stored no channel, got \
                     {described}"
                );
                continue;
            };

            assert_eq!(
                status, 200,
                "case {case}: expected the window inside the range to be served, got {status} \
                 with body {response}"
            );
            assert_eq!(
                reported_freshness(&case, kind, &response),
                Some(expected),
                "case {case}: expected the create to report the window {expected}, got {response}"
            );
            served.push((row.case, kind, name, expected));
        }
    }

    // What each create accepted is what the STORED channel reports, once the transition the
    // create armed has fallen due.
    for (row_case, kind, name, expected) in &served {
        let case = format!("{kind:?}/{row_case}");
        let arn = channel_arn(REGION, name);
        poll_until_channel_active(&emulator, &case, &arn).await;
        assert_eq!(
            stored_freshness(&emulator, &case, *kind, &arn).await,
            Some(*expected),
            "case {case}: expected the stored window {expected} to be read back"
        );
    }

    // Exactly the served creates left a channel behind: a refused one leaves no residue on the
    // listing either.
    let case = "the channel listing";
    let listed = emulator.ok(case, "ListChannels", &json!({})).await;
    let empty: Vec<Value> = Vec::new();
    let mut listed_names: Vec<String> = listed["ChannelSummaries"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|summary| summary["ChannelName"].as_str().map(str::to_string))
        .collect();
    listed_names.sort();
    let mut expected_names: Vec<String> =
        served.iter().map(|(_, _, name, _)| name.clone()).collect();
    expected_names.sort();
    assert_eq!(
        listed_names, expected_names,
        "case {case}: expected exactly the served creates to be listed, got {listed}"
    );
}

/// An update moves a destination's window only inside the same range — whichever destination
/// kind carries the member — and a refused update leaves the stored window exactly as it was
/// and arms no transition.
#[tokio::test]
async fn an_update_moves_a_data_freshness_window_only_inside_the_documented_range() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "update-freshness-source").await;

    // One channel per destination kind, each created on the documented default, so a window a
    // refused update carried would be visible as the stored window moving off it.
    let mut channels: Vec<(Kind, String, String)> = Vec::new();
    for (kind, name) in [
        (Kind::S3, "update-freshness-s3"),
        (Kind::S3Tables, "update-freshness-tables"),
    ] {
        emulator
            .ok(
                name,
                "CreateChannel",
                &create_with_freshness(kind, name, &source, None),
            )
            .await;
        channels.push((kind, name.to_string(), channel_arn(REGION, name)));
    }
    for (_, name, arn) in &channels {
        poll_until_channel_active(&emulator, name, arn).await;
    }

    for (kind, name, arn) in &channels {
        let mut held = DEFAULT_FRESHNESS_SECONDS;
        for row in windows() {
            // An update states the window it moves the destination to, so the row that names
            // none is not an update at all and is left to the create cases.
            let Some(freshness) = row.freshness else {
                continue;
            };
            let case = format!("{kind:?}/{name}/{}", row.case);
            let mut body = json!({ "ChannelARN": arn });
            body[kind.member()] = json!({ "DataFreshnessInSeconds": freshness });
            let (status, response) = emulator.call("UpdateChannel", &body).await;

            let Some(expected) = row.served else {
                assert_out_of_range(&case, status, &response);

                // Nothing was written and no transition was armed: the channel still carries
                // the window it held and is still ACTIVE.
                let read = emulator
                    .ok(&case, "DescribeChannel", &json!({ "ChannelARN": arn }))
                    .await;
                let described = description(&case, &read);
                assert_eq!(
                    member(&case, &described, "ChannelStatus"),
                    ACTIVE,
                    "case {case}: expected the refused update to arm no transition, got \
                     {described}"
                );
                assert_eq!(
                    described[kind.member()]["DataFreshnessInSeconds"].as_i64(),
                    Some(held),
                    "case {case}: expected the refused update to have written nothing, got \
                     {described}"
                );
                continue;
            };

            assert_eq!(
                status, 200,
                "case {case}: expected the window inside the range to be served, got {status} \
                 with body {response}"
            );
            assert_eq!(
                reported_freshness(&case, *kind, &response),
                Some(expected),
                "case {case}: expected the update to report the window {expected}, got {response}"
            );

            // An accepted change leaves the channel out of ACTIVE until the transition it armed
            // falls due, and the channel settles on the window it carried.
            poll_until_channel_active(&emulator, &case, arn).await;
            assert_eq!(
                stored_freshness(&emulator, &case, *kind, arn).await,
                Some(expected),
                "case {case}: expected the channel to settle on the window {expected}"
            );
            held = expected;
        }
    }
}

/// The bucket helper is shared with the lifecycle suite, and a destination naming one bucket
/// is what every row above delivers to — asserted here so the helper cannot drift away from
/// the ARN form the model states.
#[test]
fn a_destination_bucket_arn_is_inside_the_modelled_form() {
    let arn = bucket_arn("key-template-bucket");
    assert_eq!(
        arn, "arn:aws:s3:::key-template-bucket",
        "the destination bucket ARN is the modelled `BucketARN` form, got {arn:?}"
    );
}
