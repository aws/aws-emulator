// SPDX-License-Identifier: CC0-1.0

//! The ORDER this service lists an enumerated type's admissible values in when it refuses
//! a value outside that type, over the real `awsJson1_1` wire — for EVERY enumerated type
//! a request member of this service targets, not only the one a report cited.
//!
//! The rule: a value outside a modelled enumerated type is refused at the request boundary
//! with `ValidationException` (HTTP 400) and the sentence `1 validation error detected:
//! Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value
//! set: [<values>]`, where `<values>` is that type's WHOLE membership listed in the order
//! the hash table those values are held in is walked: each value's bucket ascending — the
//! bucket being the 31-polynomial hash of the value's UTF-16 code units, taken modulo the
//! table's width — and two values that share a bucket read from the head each was pushed
//! onto, so the one pushed LATER is listed first.
//!
//! The order is therefore a property of the enumerated TYPE: it is computed from the
//! values themselves, so one type reads the same way under every member that carries it —
//! any operation, any destination, any depth, any list index — and it is neither the order
//! the model declares the values in nor a sort of them. The order the values were PUSHED in
//! is the service's own, which for all but one of this service's types is the order the
//! model declares them in; `CompressionFormat`'s set is built in the order the service's API
//! reference publishes that member's valid values instead, which the service publishes
//! itself (`src/service/enum_value_sets.rs`).
//!
//! The `ProcessorType` and `CompressionFormat` listings are the ones this service's own
//! refusals are recorded stating; every other row's listing is what the rule above derives
//! for that type.
//!
//! The rows differ along what the rule generalises over: which enumerated type failed
//! (seven of them), how many values it has (two to eleven), whether two of its values
//! COLLIDE in one bucket (three rows do, four do not), which operation carried the value
//! (`CreateDeliveryStream`, `UpdateDestination`), which destination member it sat under
//! (extended Amazon S3, Splunk, Elasticsearch, HTTP endpoint), how deep it sat (a
//! destination member, a nested configuration's member, a serializer five levels down, a
//! list element, a list element inside a list element) and which 1-based index of a list it
//! sat at.
//!
//! The last three tests take ONE type further — `CompressionFormat`, whose set this service
//! builds in an order this checkout's model does not carry — over every member that targets
//! it and every shape of value outside it the rows draw, and pin that a create refused for
//! one leaves no stream behind.
//!
//! Each row's declared values are the model's own
//! (`smithy_query(query="shape", shape="ProcessorType,ProcessorParameterName,
//! CompressionFormat,OrcCompression,ElasticsearchIndexRotationPeriod,HECEndpointType,
//! ContentEncoding")`) and are checked against the generated value lists in `ops::types`,
//! so neither the membership a row expects nor the listing it pins can drift from the type
//! the model declares.
//!
//! The sibling suite `enum_value_set_order` pins the clause's envelope, subject and member
//! path, and that a structural refusal keeps its own error code and leaves no stream
//! behind; this one pins the LISTING, type by type.
//!
//! The harness (`Emulator`, `s3_configuration`, …) is the lifecycle suite's.

use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::ops::types::shared_types::{
    COMPRESSION_FORMAT_VALUES, CONTENT_ENCODING_VALUES,
    ELASTICSEARCH_INDEX_ROTATION_PERIOD_VALUES, H_E_C_ENDPOINT_TYPE_VALUES, ORC_COMPRESSION_VALUES,
    PROCESSOR_PARAMETER_NAME_VALUES, PROCESSOR_TYPE_VALUES,
};

use super::delivery_stream_lifecycle::{
    error_code, error_message, s3_configuration, Emulator, BUCKET_ARN, ROLE_ARN,
};

/// The code a request refused at the model's boundary carries.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A value no enumerated type of this model declares, so every row below breaks exactly
/// one enum constraint and nothing else.
const OFF_ENUM: &str = "NotAModelledValue";

/// One enumerated type under test: how it is reached, what the model declares, and the
/// listing a refusal of a value outside it must state.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// The operation whose request carries the off-enum value.
    operation: &'static str,
    /// That request.
    body: Value,
    /// The member path the refused value sat at.
    path: &'static str,
    /// The enumerated type the member targets, named for the assertion messages.
    type_name: &'static str,
    /// Its values in the order the MODEL declares them.
    declared: &'static [&'static str],
    /// The same values as the generated catalogue holds them, so a row cannot expect a
    /// membership the model does not declare.
    modelled: &'static [&'static str],
    /// The listing the refusal must state.
    listing: &'static str,
    /// Two values of this type that share a bucket, as `(pushed earlier, pushed later)` —
    /// the order being the one the service's own set is built in, which is the model's
    /// declaration order for every type but `CompressionFormat`. The later one is stated
    /// first, so a row naming a pair pins the direction a shared bucket is read in.
    collision: Option<(&'static str, &'static str)>,
}

/// `ProcessorType` as the model declares it.
const PROCESSOR_TYPE_DECLARED: &[&str] = &[
    "RecordDeAggregation",
    "Decompression",
    "CloudWatchLogProcessing",
    "Lambda",
    "MetadataExtraction",
    "AppendDelimiterToRecord",
];

/// `ProcessorType`'s six values as this service's refusal is recorded stating them.
const PROCESSOR_TYPE_LISTING: &str = "Decompression, Lambda, RecordDeAggregation, \
                                      MetadataExtraction, AppendDelimiterToRecord, \
                                      CloudWatchLogProcessing";

/// `ProcessorParameterName` as the model declares it.
const PROCESSOR_PARAMETER_NAME_DECLARED: &[&str] = &[
    "LambdaArn",
    "NumberOfRetries",
    "MetadataExtractionQuery",
    "JsonParsingEngine",
    "RoleArn",
    "BufferSizeInMBs",
    "BufferIntervalInSeconds",
    "SubRecordType",
    "Delimiter",
    "CompressionFormat",
    "DataMessageExtraction",
];

/// `ProcessorParameterName`'s eleven values in the order the rule derives, `RoleArn` and
/// `Delimiter` sharing one bucket.
const PROCESSOR_PARAMETER_NAME_LISTING: &str = "DataMessageExtraction, NumberOfRetries, \
                                                Delimiter, RoleArn, JsonParsingEngine, \
                                                SubRecordType, MetadataExtractionQuery, \
                                                BufferIntervalInSeconds, LambdaArn, \
                                                BufferSizeInMBs, CompressionFormat";

/// `CompressionFormat` as the model declares it.
const COMPRESSION_FORMAT_DECLARED: &[&str] =
    &["UNCOMPRESSED", "GZIP", "ZIP", "Snappy", "HADOOP_SNAPPY"];

/// `CompressionFormat`'s five values in the order the rule derives over the order this
/// service's own set is built in (`UNCOMPRESSED`, `GZIP`, `ZIP`, `Snappy`,
/// `HADOOP_SNAPPY`), `GZIP` and `UNCOMPRESSED` sharing one bucket and `GZIP` having been
/// pushed later. The listing this service's refusal is recorded stating.
const COMPRESSION_FORMAT_LISTING: &str = "ZIP, HADOOP_SNAPPY, Snappy, GZIP, UNCOMPRESSED";

/// `OrcCompression` as the model declares it.
const ORC_COMPRESSION_DECLARED: &[&str] = &["NONE", "ZLIB", "SNAPPY"];

/// `OrcCompression`'s three values in the order the rule derives.
const ORC_COMPRESSION_LISTING: &str = "ZLIB, SNAPPY, NONE";

/// `ElasticsearchIndexRotationPeriod` as the model declares it.
const INDEX_ROTATION_PERIOD_DECLARED: &[&str] =
    &["NoRotation", "OneHour", "OneDay", "OneWeek", "OneMonth"];

/// `ElasticsearchIndexRotationPeriod`'s five values in the order the rule derives,
/// `OneWeek` and `OneMonth` sharing one bucket.
const INDEX_ROTATION_PERIOD_LISTING: &str = "OneHour, OneDay, OneMonth, OneWeek, NoRotation";

/// `HECEndpointType` as the model declares it.
const HEC_ENDPOINT_TYPE_DECLARED: &[&str] = &["Raw", "Event"];

/// `HECEndpointType`'s two values in the order the rule derives.
const HEC_ENDPOINT_TYPE_LISTING: &str = "Raw, Event";

/// `ContentEncoding` as the model declares it.
const CONTENT_ENCODING_DECLARED: &[&str] = &["NONE", "GZIP"];

/// `ContentEncoding`'s two values in the order the rule derives.
const CONTENT_ENCODING_LISTING: &str = "NONE, GZIP";

/// A Splunk destination carrying every member the model requires, so a row that breaks one
/// of its enumerated members breaks nothing else.
fn splunk_destination(hec_endpoint_type: &str, s3: Value) -> Value {
    json!({
        "HECEndpoint": "https://splunk.example.com:8088",
        "HECEndpointType": hec_endpoint_type,
        "HECToken": "hec-token",
        "S3Configuration": s3,
    })
}

/// Every row: seven enumerated types, two operations, four destination members, five
/// depths and two list indices.
fn rows() -> Vec<Row> {
    vec![
        // The type a report cited, at the first element of an extended Amazon S3
        // destination's processor list.
        Row {
            case: "a processor type outside its enum",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-processor-type",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{
                            "Type": OFF_ENUM,
                            "Parameters": [{
                                "ParameterName": "LambdaArn",
                                "ParameterValue": "x",
                            }],
                        }],
                    },
                },
            }),
            path: "extendedS3DestinationConfiguration.processingConfiguration.processors.1.member\
                   .type",
            type_name: "ProcessorType",
            declared: PROCESSOR_TYPE_DECLARED,
            modelled: PROCESSOR_TYPE_VALUES,
            listing: PROCESSOR_TYPE_LISTING,
            collision: None,
        },
        // A second type one level deeper, at the SECOND element of the parameter list of
        // the first element of the processor list.
        Row {
            case: "a processor parameter name outside its enum, at the second parameter",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-parameter-name",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "ProcessingConfiguration": {
                        "Enabled": true,
                        "Processors": [{
                            "Type": "Lambda",
                            "Parameters": [
                                {
                                    "ParameterName": "LambdaArn",
                                    "ParameterValue": "x",
                                },
                                {
                                    "ParameterName": OFF_ENUM,
                                    "ParameterValue": "y",
                                },
                            ],
                        }],
                    },
                },
            }),
            path: "extendedS3DestinationConfiguration.processingConfiguration.processors.1.member\
                   .parameters.2.member.parameterName",
            type_name: "ProcessorParameterName",
            declared: PROCESSOR_PARAMETER_NAME_DECLARED,
            modelled: PROCESSOR_PARAMETER_NAME_VALUES,
            listing: PROCESSOR_PARAMETER_NAME_LISTING,
            collision: Some(("RoleArn", "Delimiter")),
        },
        // A third type, on a plain destination member.
        Row {
            case: "a compression format outside its enum",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-compression-format",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "CompressionFormat": OFF_ENUM,
                },
            }),
            path: "extendedS3DestinationConfiguration.compressionFormat",
            type_name: "CompressionFormat",
            declared: COMPRESSION_FORMAT_DECLARED,
            modelled: COMPRESSION_FORMAT_VALUES,
            listing: COMPRESSION_FORMAT_LISTING,
            collision: Some(("UNCOMPRESSED", "GZIP")),
        },
        // … the SAME type under another destination, inside that destination's nested
        // Amazon S3 configuration rather than on the destination itself.
        Row {
            case: "a compression format outside its enum, nested in a backing S3 configuration",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-compression-format-nested",
                "SplunkDestinationConfiguration": splunk_destination(
                    "Raw",
                    json!({
                        "RoleARN": ROLE_ARN,
                        "BucketARN": BUCKET_ARN,
                        "CompressionFormat": OFF_ENUM,
                    }),
                ),
            }),
            path: "splunkDestinationConfiguration.s3Configuration.compressionFormat",
            type_name: "CompressionFormat",
            declared: COMPRESSION_FORMAT_DECLARED,
            modelled: COMPRESSION_FORMAT_VALUES,
            listing: COMPRESSION_FORMAT_LISTING,
            collision: Some(("UNCOMPRESSED", "GZIP")),
        },
        // … and on the other operation that writes a destination, whose member is an
        // update rather than a configuration.
        Row {
            case: "a compression format outside its enum on an update",
            operation: "UpdateDestination",
            body: json!({
                "DeliveryStreamName": "off-enum-compression-format-update",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "ExtendedS3DestinationUpdate": { "CompressionFormat": OFF_ENUM },
            }),
            path: "extendedS3DestinationUpdate.compressionFormat",
            type_name: "CompressionFormat",
            declared: COMPRESSION_FORMAT_DECLARED,
            modelled: COMPRESSION_FORMAT_VALUES,
            listing: COMPRESSION_FORMAT_LISTING,
            collision: Some(("UNCOMPRESSED", "GZIP")),
        },
        // A fourth type, on the serializer five structures below the destination member.
        Row {
            case: "an ORC compression outside its enum, on a serializer five levels down",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-orc-compression",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "DataFormatConversionConfiguration": {
                        "Enabled": true,
                        "OutputFormatConfiguration": {
                            "Serializer": { "OrcSerDe": { "Compression": OFF_ENUM } },
                        },
                    },
                },
            }),
            path: "extendedS3DestinationConfiguration.dataFormatConversionConfiguration\
                   .outputFormatConfiguration.serializer.orcSerDe.compression",
            type_name: "OrcCompression",
            declared: ORC_COMPRESSION_DECLARED,
            modelled: ORC_COMPRESSION_VALUES,
            listing: ORC_COMPRESSION_LISTING,
            collision: None,
        },
        // A fifth type, under a destination of its own.
        Row {
            case: "an index rotation period outside its enum",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-index-rotation-period",
                "ElasticsearchDestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "IndexName": "events",
                    "IndexRotationPeriod": OFF_ENUM,
                    "S3Configuration": s3_configuration(),
                },
            }),
            path: "elasticsearchDestinationConfiguration.indexRotationPeriod",
            type_name: "ElasticsearchIndexRotationPeriod",
            declared: INDEX_ROTATION_PERIOD_DECLARED,
            modelled: ELASTICSEARCH_INDEX_ROTATION_PERIOD_VALUES,
            listing: INDEX_ROTATION_PERIOD_LISTING,
            collision: Some(("OneWeek", "OneMonth")),
        },
        // A sixth type, of two values, on a member the model marks required.
        Row {
            case: "an HEC endpoint type outside its enum",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-hec-endpoint-type",
                "SplunkDestinationConfiguration": splunk_destination(
                    OFF_ENUM,
                    s3_configuration(),
                ),
            }),
            path: "splunkDestinationConfiguration.hECEndpointType",
            type_name: "HECEndpointType",
            declared: HEC_ENDPOINT_TYPE_DECLARED,
            modelled: H_E_C_ENDPOINT_TYPE_VALUES,
            listing: HEC_ENDPOINT_TYPE_LISTING,
            collision: None,
        },
        // A seventh type, on a request configuration of the HTTP endpoint destination.
        Row {
            case: "a content encoding outside its enum",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "off-enum-content-encoding",
                "HttpEndpointDestinationConfiguration": {
                    "EndpointConfiguration": { "Url": "https://endpoint.example.com/firehose" },
                    "RequestConfiguration": { "ContentEncoding": OFF_ENUM },
                    "S3Configuration": s3_configuration(),
                },
            }),
            path: "httpEndpointDestinationConfiguration.requestConfiguration.contentEncoding",
            type_name: "ContentEncoding",
            declared: CONTENT_ENCODING_DECLARED,
            modelled: CONTENT_ENCODING_VALUES,
            listing: CONTENT_ENCODING_LISTING,
            collision: None,
        },
    ]
}

/// The values a refusal listed, taken out of its `enum value set: [ … ]` half.
fn stated_set(message: &str) -> Option<&str> {
    let (_, tail) = message.split_once("Member must satisfy enum value set: [")?;
    tail.split_once(']').map(|(set, _)| set)
}

/// `values` sorted, for comparing one listing's MEMBERSHIP with another's.
fn membership(values: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
    sorted.sort_unstable();
    sorted
}

#[tokio::test]
async fn every_enumerated_type_states_its_whole_set_in_the_order_its_table_is_walked() {
    let emulator = Emulator::new();

    for row in rows() {
        // The row's hand-written declaration order holds exactly the model's values, so
        // neither the membership nor the listing below can drift from the type the model
        // declares.
        assert_eq!(
            membership(row.declared),
            membership(row.modelled),
            "case {:?}: {} is declared {:?}, the model's catalogue holds {:?}",
            row.case,
            row.type_name,
            row.declared,
            row.modelled
        );

        let (status, refused) = emulator.call(row.operation, row.body.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {refused:?}",
            row.case
        );
        let code = error_code(&refused);
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {:?}: expected {VALIDATION_EXCEPTION}, got {code:?} {refused:?}",
            row.case
        );

        // The whole sentence: the envelope, the subject, the member path, the requirement's
        // wording and the listing.
        let actual = error_message(&refused);
        let expected = format!(
            "1 validation error detected: Value at '{}' failed to satisfy constraint: \
             Member must satisfy enum value set: [{}]",
            row.path, row.listing
        );
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            row.case
        );

        // The listing holds every value once and nothing the model does not declare: it is
        // the modelled membership re-ordered, not a set of its own.
        let listed: Vec<&str> = row.listing.split(", ").collect();
        assert_eq!(
            membership(&listed),
            membership(row.modelled),
            "case {:?}: the listing {:?} is not {}'s membership {:?}",
            row.case,
            row.listing,
            row.type_name,
            row.modelled
        );
    }
}

#[tokio::test]
async fn a_listing_is_neither_the_declaration_order_nor_a_sort_of_the_values() {
    let emulator = Emulator::new();

    // Which types this service's refusals state in an order other than the model's
    // declaration order, and so would be got wrong by a renderer that walked the schema.
    // (The two two-value types above are stated in their declaration order, so they
    // discriminate nothing and are not asked to. The sort assertion below covers every row,
    // and the `CompressionFormat` tests at the end of this suite pin its listing member by
    // member.)
    let not_the_declaration_order = [
        "ProcessorType",
        "ProcessorParameterName",
        "CompressionFormat",
        "OrcCompression",
        "ElasticsearchIndexRotationPeriod",
    ];

    for row in rows() {
        let (_, refused) = emulator.call(row.operation, row.body.clone()).await;
        let message = error_message(&refused);
        let Some(stated) = stated_set(&message) else {
            panic!(
                "case {:?}: the message states no enum value set, got {message:?}",
                row.case
            );
        };
        if not_the_declaration_order.contains(&row.type_name) {
            assert_ne!(
                stated,
                row.declared.join(", "),
                "case {:?}: {} must not be stated in the model's declaration order, got \
                 [{stated}]",
                row.case,
                row.type_name
            );
        }
        // No type of this service is stated as a sort of its values, whichever order its
        // own set was built in.
        assert_ne!(
            stated,
            membership(row.declared).join(", "),
            "case {:?}: {} must not be stated as a sort of its values, got [{stated}]",
            row.case,
            row.type_name
        );
    }
}

#[tokio::test]
async fn two_values_sharing_a_bucket_are_stated_the_later_pushed_first() {
    let emulator = Emulator::new();

    for row in rows() {
        let Some((earlier, later)) = row.collision else {
            continue;
        };
        let (_, refused) = emulator.call(row.operation, row.body.clone()).await;
        let message = error_message(&refused);
        let Some(stated) = stated_set(&message) else {
            panic!(
                "case {:?}: the message states no enum value set, got {message:?}",
                row.case
            );
        };
        let listed: Vec<&str> = stated.split(", ").collect();
        let at = |value: &str| {
            listed
                .iter()
                .position(|listed| *listed == value)
                .unwrap_or_else(|| {
                    panic!("case {:?}: {value} is not listed in [{stated}]", row.case)
                })
        };
        assert!(
            at(later) < at(earlier),
            "case {:?}: {later} shares {earlier}'s bucket and was pushed after it, so it \
             must be stated first, got [{stated}]",
            row.case
        );
    }
}

#[tokio::test]
async fn one_type_is_stated_alike_under_every_member_that_carries_it() {
    let emulator = Emulator::new();

    // Group the rows by the type that failed: one type, one listing, whichever operation,
    // destination member, depth or list index carried the value.
    let mut seen: Vec<(&'static str, &'static str, String)> = Vec::new();
    for row in rows() {
        let (_, refused) = emulator.call(row.operation, row.body.clone()).await;
        let message = error_message(&refused);
        let stated = stated_set(&message)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the message states no enum value set, got {message:?}",
                    row.case
                )
            })
            .to_string();
        match seen
            .iter()
            .find(|(type_name, _, _)| *type_name == row.type_name)
        {
            Some((_, first_case, first)) => assert_eq!(
                &stated, first,
                "case {:?}: {} was stated as [{first}] by case {first_case:?}, got [{stated}]",
                row.case, row.type_name
            ),
            None => seen.push((row.type_name, row.case, stated)),
        }
    }

    // Every type the rows cover was actually refused, so no row passed by never being
    // reached.
    assert_eq!(
        seen.len(),
        7,
        "expected all seven enumerated types to have been refused, got {seen:?}"
    );
}

/// One position a `CompressionFormat` is submitted at: the request that carries it, the
/// member path the clause must name, and the value submitted there.
struct Position {
    /// What this position varies, for the assertion messages.
    case: &'static str,
    /// The operation whose request carries the value.
    operation: &'static str,
    /// That request.
    body: Value,
    /// The member path the refused value sat at.
    path: &'static str,
}

/// A Splunk destination's backing Amazon S3 configuration, carrying a compression format of
/// `compression`.
fn splunk_with_compression(compression: &str) -> Value {
    splunk_destination(
        "Raw",
        json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": BUCKET_ARN,
            "CompressionFormat": compression,
        }),
    )
}

/// Every member that carries a `CompressionFormat`, each submitted a value outside the type.
///
/// The rows differ along what the rule generalises over: the operation
/// (`CreateDeliveryStream`, `UpdateDestination`), the destination the member sits under
/// (extended Amazon S3, the deprecated Amazon S3 destination, a backup configuration,
/// Splunk, Elasticsearch), the depth it sits at (a destination member, a nested
/// configuration's member) and the SHAPE of the value submitted (a format name the service
/// does not take, the lower-cased spelling of a modelled one, a modelled one with a trailing
/// space, a value of another enumerated type, and the empty string).
fn compression_format_positions() -> Vec<Position> {
    vec![
        Position {
            case: "a format name outside the type, on an extended Amazon S3 destination",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "compression-outside-extended",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "CompressionFormat": "BOGUS_FORMAT",
                },
            }),
            path: "extendedS3DestinationConfiguration.compressionFormat",
        },
        Position {
            case: "the lower-cased spelling of a modelled value, on the deprecated \
                   Amazon S3 destination",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "compression-outside-legacy",
                "S3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "CompressionFormat": "gzip",
                },
            }),
            path: "s3DestinationConfiguration.compressionFormat",
        },
        Position {
            case: "the empty string, on a destination's backup configuration",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "compression-outside-backup",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "S3BackupMode": "Enabled",
                    "S3BackupConfiguration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": BUCKET_ARN,
                        "CompressionFormat": "",
                    },
                },
            }),
            path: "extendedS3DestinationConfiguration.s3BackupConfiguration.compressionFormat",
        },
        Position {
            case: "a modelled value with a trailing space, nested in a Splunk destination's \
                   Amazon S3 configuration",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "compression-outside-splunk",
                "SplunkDestinationConfiguration": splunk_with_compression("GZIP "),
            }),
            path: "splunkDestinationConfiguration.s3Configuration.compressionFormat",
        },
        Position {
            case: "a value of another enumerated type, nested in an Elasticsearch \
                   destination's Amazon S3 configuration",
            operation: "CreateDeliveryStream",
            body: json!({
                "DeliveryStreamName": "compression-outside-elasticsearch",
                "ElasticsearchDestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "IndexName": "events",
                    "S3Configuration": {
                        "RoleARN": ROLE_ARN,
                        "BucketARN": BUCKET_ARN,
                        "CompressionFormat": "ZLIB",
                    },
                },
            }),
            path: "elasticsearchDestinationConfiguration.s3Configuration.compressionFormat",
        },
        Position {
            case: "a format name outside the type, on an extended Amazon S3 update",
            operation: "UpdateDestination",
            body: json!({
                "DeliveryStreamName": "compression-outside-update",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "ExtendedS3DestinationUpdate": { "CompressionFormat": "LZ4" },
            }),
            path: "extendedS3DestinationUpdate.compressionFormat",
        },
        Position {
            case: "the lower-cased spelling of a modelled value, on the deprecated \
                   Amazon S3 update",
            operation: "UpdateDestination",
            body: json!({
                "DeliveryStreamName": "compression-outside-legacy-update",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "S3DestinationUpdate": { "CompressionFormat": "snappy" },
            }),
            path: "s3DestinationUpdate.compressionFormat",
        },
    ]
}

#[tokio::test]
async fn a_compression_format_outside_the_type_is_refused_with_the_set_this_service_states() {
    let emulator = Emulator::new();

    // The one listing every position must state, so the set belongs to the TYPE and not to
    // a member, an operation or a depth.
    let mut stated_everywhere: Vec<(&str, String)> = Vec::new();

    for position in compression_format_positions() {
        let (status, refused) = emulator.call(position.operation, position.body).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} with {refused:?}",
            position.case
        );
        let code = error_code(&refused);
        assert_eq!(
            code, VALIDATION_EXCEPTION,
            "case {:?}: expected {VALIDATION_EXCEPTION}, got {code:?} with {refused:?}",
            position.case
        );

        let actual = error_message(&refused);
        let expected = format!(
            "1 validation error detected: Value at '{}' failed to satisfy constraint: \
             Member must satisfy enum value set: [{COMPRESSION_FORMAT_LISTING}]",
            position.path
        );
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected:?}, got {actual:?}",
            position.case
        );

        let stated = stated_set(&actual)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the message states no enum value set, got {actual:?}",
                    position.case
                )
            })
            .to_string();
        // The listing holds exactly the modelled membership, re-ordered — not a set of its
        // own.
        let listed: Vec<&str> = stated.split(", ").collect();
        assert_eq!(
            membership(&listed),
            membership(COMPRESSION_FORMAT_VALUES),
            "case {:?}: the listing [{stated}] is not CompressionFormat's membership {:?}",
            position.case,
            COMPRESSION_FORMAT_VALUES
        );
        if let Some((first_case, first)) = stated_everywhere.first() {
            assert_eq!(
                &stated, first,
                "case {:?}: the type was stated as [{first}] by case {first_case:?}, got \
                 [{stated}]",
                position.case
            );
        }
        stated_everywhere.push((position.case, stated));
    }

    assert_eq!(
        stated_everywhere.len(),
        7,
        "expected every position to have been refused, got {stated_everywhere:?}"
    );
}

/// RULE: a create refused for a value outside an enumerated type leaves no stream behind
/// under the name it carried.
///
/// The positive control — that every value the type DOES declare is taken and reported back
/// — is `destination_argument_rules`'s
/// `every_value_the_compression_format_set_admits_can_be_configured_and_is_reported`, which
/// reads each stored format back through `DescribeDeliveryStream`; this test does not repeat
/// it.
#[tokio::test]
async fn a_create_refused_for_a_compression_format_outside_the_type_leaves_no_stream() {
    let emulator = Emulator::new();

    let name = "compression-refused-leaves-nothing";
    let (status, refused) = emulator
        .call(
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": name,
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": ROLE_ARN,
                    "BucketARN": BUCKET_ARN,
                    "CompressionFormat": "BOGUS_FORMAT",
                },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {name:?}: expected HTTP 400, got {status} with {refused:?}"
    );
    let actual = error_message(&refused);
    let expected = format!(
        "1 validation error detected: Value at \
         'extendedS3DestinationConfiguration.compressionFormat' failed to satisfy constraint: \
         Member must satisfy enum value set: [{COMPRESSION_FORMAT_LISTING}]"
    );
    assert_eq!(
        actual, expected,
        "case {name:?}: expected {expected:?}, got {actual:?}"
    );

    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": name }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {name:?}: expected HTTP 400 for the refused name, got {status} with {described:?}"
    );
    let code = error_code(&described);
    assert_eq!(
        code, "ResourceNotFoundException",
        "case {name:?}: expected \"ResourceNotFoundException\", got {code:?} with {described:?}"
    );
}
