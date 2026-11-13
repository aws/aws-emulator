// SPDX-License-Identifier: CC0-1.0

//! The SENTENCES this service's control plane refuses with, over the real `awsJson1_1`
//! wire, and the one refusal whose RULE the wording gave away.
//!
//! The rules asserted here, each over a set of inputs rather than one request:
//!
//! 1. a destination type a stream can neither be switched into nor out of refuses the
//!    switch ITSELF — the answer names the types the switch may not cross and does not
//!    depend on which, or how many, parameters of the new type the update carried, so no
//!    refusal reads as an invitation to supply more;
//! 2. every control-plane refusal renders one fixed template from the request's own
//!    parameters and the stream's own state: a lookup that resolved nothing states the
//!    account, an encryption change and an update state the status that blocks them, a
//!    conditional update states both version ids, and an update naming an unknown
//!    destination states that id;
//! 3. a create of a stream type that is fed by a source configuration is refused, naming
//!    that configuration and the type, when the request carries none.
//!
//! The harness (`Emulator`, `s3_configuration`, `error_code`, `error_message`) is the
//! lifecycle suite's.

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{
    error_code, error_message, s3_configuration, Emulator, BUCKET_ARN, PAST_THE_TRANSITION,
    ROLE_ARN,
};
use crate::stream_not_found_message::NotFoundWording;

/// A Redshift destination with every parameter its type requires, for a switch that leaves
/// nothing out.
fn redshift_update() -> Value {
    json!({
        "RoleARN": ROLE_ARN,
        "ClusterJDBCURL":
            "jdbc:redshift://example-cluster.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
        "CopyCommand": { "DataTableName": "events" },
        "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN },
    })
}

/// An HTTP endpoint destination with every parameter its type requires.
fn http_endpoint_update() -> Value {
    json!({
        "EndpointConfiguration": { "Url": "https://example.com/firehose" },
        "RoleARN": ROLE_ARN,
        "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN },
    })
}

/// An Elasticsearch destination with every parameter its type requires.
fn elasticsearch_update() -> Value {
    json!({
        "RoleARN": ROLE_ARN,
        "DomainARN": "arn:aws:es:us-west-2:000000000000:domain/logs-domain",
        "IndexName": "logs",
        "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN },
    })
}

/// Create a stream whose only destination is `member`: `config`.
async fn create(emulator: &Emulator, stream: &str, member: &str, config: Value) {
    let mut request = json!({ "DeliveryStreamName": stream });
    request[member] = config;
    let (status, created) = emulator.call("CreateDeliveryStream", request).await;
    assert_eq!(status, 200, "create {stream}: got {created:?}");
}

/// The `DeliveryStreamDescription` of `stream`.
async fn describe(emulator: &Emulator, stream: &str) -> Value {
    let (status, described) = emulator
        .call(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": stream }),
        )
        .await;
    assert_eq!(status, 200, "describe {stream}: got {described:?}");
    described["DeliveryStreamDescription"].clone()
}

/// `(VersionId, DestinationId)` of a stream's sole destination.
async fn version_and_destination(emulator: &Emulator, stream: &str) -> (String, String) {
    let description = describe(emulator, stream).await;
    (
        description["VersionId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        description["Destinations"][0]["DestinationId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// Call `UpdateDestination` with `member`: `body`.
async fn update(
    emulator: &Emulator,
    stream: &str,
    version: &str,
    destination_id: &str,
    member: &str,
    body: Value,
) -> (u16, Value) {
    let mut request = json!({
        "DeliveryStreamName": stream,
        "CurrentDeliveryStreamVersionId": version,
        "DestinationId": destination_id,
    });
    request[member] = body;
    emulator.call("UpdateDestination", request).await
}

/// The sentence a switch into or out of an Amazon OpenSearch Service destination is refused
/// with, and the one for an HTTP endpoint destination. Held here as the sentences this
/// service states them (`crate::service`'s wording), so a case names which it expects.
const SEARCH_REFUSAL: &str = "Changing the destination type to/from Elasticsearch/\
     Amazonopensearchservice/AmazonOpenSearchServerless from/to other destinations is not \
     supported at this time.";
const HTTP_ENDPOINT_REFUSAL: &str =
    "Changing the destination type to or from HttpEndpoint is not supported at this time.";

/// One attempted destination-type switch.
struct SwitchCase {
    case: &'static str,
    /// The stream the switch is attempted on, created below.
    stream: &'static str,
    /// The `*DestinationUpdate` member the switch is attempted with.
    update_member: &'static str,
    update: Value,
    expected: &'static str,
}

/// The rows vary what the rule must NOT depend on: how complete the new type's
/// configuration is (nothing but a role, a role plus the nested Amazon S3 update the old
/// refusal asked for, and every required parameter), which direction the switch goes, and
/// which other destination type is on the far side.
fn switch_cases() -> Vec<SwitchCase> {
    vec![
        // The rows the report cites: an extended Amazon S3 stream updated with an HTTP
        // endpoint destination, with and without the nested Amazon S3 update.
        SwitchCase {
            case: "into an HTTP endpoint with only a role",
            stream: "switch-extended-s3",
            update_member: "HttpEndpointDestinationUpdate",
            update: json!({ "RoleARN": ROLE_ARN }),
            expected: HTTP_ENDPOINT_REFUSAL,
        },
        SwitchCase {
            case: "into an HTTP endpoint with the nested Amazon S3 update supplied",
            stream: "switch-extended-s3",
            update_member: "HttpEndpointDestinationUpdate",
            update: json!({
                "RoleARN": ROLE_ARN,
                "S3Update": { "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN },
            }),
            expected: HTTP_ENDPOINT_REFUSAL,
        },
        SwitchCase {
            case: "into an HTTP endpoint with every required parameter",
            stream: "switch-extended-s3",
            update_member: "HttpEndpointDestinationUpdate",
            update: http_endpoint_update(),
            expected: HTTP_ENDPOINT_REFUSAL,
        },
        SwitchCase {
            case: "into an HTTP endpoint from a plain Amazon S3 destination",
            stream: "switch-s3",
            update_member: "HttpEndpointDestinationUpdate",
            update: http_endpoint_update(),
            expected: HTTP_ENDPOINT_REFUSAL,
        },
        SwitchCase {
            case: "out of an HTTP endpoint to Amazon S3",
            stream: "switch-http-endpoint",
            update_member: "S3DestinationUpdate",
            update: json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
            expected: HTTP_ENDPOINT_REFUSAL,
        },
        SwitchCase {
            case: "out of an HTTP endpoint to Redshift",
            stream: "switch-http-endpoint",
            update_member: "RedshiftDestinationUpdate",
            update: redshift_update(),
            expected: HTTP_ENDPOINT_REFUSAL,
        },
        // The same rule for the other family that takes no part in a type change.
        SwitchCase {
            case: "into Amazon OpenSearch Service from Amazon S3",
            stream: "switch-s3",
            update_member: "ElasticsearchDestinationUpdate",
            update: elasticsearch_update(),
            expected: SEARCH_REFUSAL,
        },
        SwitchCase {
            case: "out of Amazon OpenSearch Service to Redshift",
            stream: "switch-elasticsearch",
            update_member: "RedshiftDestinationUpdate",
            update: redshift_update(),
            expected: SEARCH_REFUSAL,
        },
    ]
}

#[tokio::test]
async fn a_switch_across_a_type_that_takes_no_part_in_one_is_refused_as_unsupported() {
    let emulator = Emulator::new();

    create(
        &emulator,
        "switch-extended-s3",
        "ExtendedS3DestinationConfiguration",
        json!({ "RoleARN": ROLE_ARN, "BucketARN": BUCKET_ARN }),
    )
    .await;
    create(
        &emulator,
        "switch-s3",
        "S3DestinationConfiguration",
        s3_configuration(),
    )
    .await;
    create(
        &emulator,
        "switch-http-endpoint",
        "HttpEndpointDestinationConfiguration",
        json!({
            "EndpointConfiguration": { "Url": "https://example.com/firehose" },
            "S3Configuration": s3_configuration(),
        }),
    )
    .await;
    create(
        &emulator,
        "switch-elasticsearch",
        "ElasticsearchDestinationConfiguration",
        json!({
            "RoleARN": ROLE_ARN,
            "DomainARN": "arn:aws:es:us-west-2:000000000000:domain/logs-domain",
            "IndexName": "logs",
            "S3Configuration": s3_configuration(),
        }),
    )
    .await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    for case in switch_cases() {
        let (version, destination_id) = version_and_destination(&emulator, case.stream).await;
        let (status, refused) = update(
            &emulator,
            case.stream,
            &version,
            &destination_id,
            case.update_member,
            case.update.clone(),
        )
        .await;
        assert_eq!(status, 400, "case {:?}: got {refused:?}", case.case);
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {:?}: got {refused:?}",
            case.case
        );
        let message = error_message(&refused);
        assert_eq!(
            message, case.expected,
            "case {:?}: expected {:?}, got {message:?}",
            case.case, case.expected
        );
        // The refused switch changes nothing: the stream keeps the version it had.
        let (after, _) = version_and_destination(&emulator, case.stream).await;
        assert_eq!(
            after, version,
            "case {:?}: the refused switch left the version at {version}, got {after}",
            case.case
        );
    }
}

/// One refusal: the request that triggers it, and the code and sentence it must answer.
struct RefusalCase {
    case: String,
    operation: &'static str,
    request: Value,
    code: &'static str,
    message: String,
}

#[tokio::test]
async fn a_control_plane_refusal_renders_one_template_from_the_request_and_the_state() {
    let emulator = Emulator::new();
    let account = engine::emulator_account_id();
    // The sentences each path words a lookup that resolved nothing in, from the one
    // test-side source the sibling suites assert them from.
    let not_found = NotFoundWording::ControlPlaneRead.sentence(account);
    let encryption_not_found = NotFoundWording::EncryptionChange.sentence(account);

    // A stream the refusals below are asserted against in each state: one that stays
    // `ACTIVE`, one still being created, and one already being deleted. The two transient
    // ones are put into their state LAST, immediately before the loop, because the clock
    // owes each of them a transition out of it.
    emulator.create_stream("refusal-active", None).await;
    emulator.create_stream("refusal-deleting", None).await;
    emulator.await_active("refusal-active").await;
    emulator.await_active("refusal-deleting").await;

    let (version, destination_id) = version_and_destination(&emulator, "refusal-active").await;
    let mut cases = vec![
        // A lookup that resolved nothing: the account the lookup was scoped to, whichever
        // operation did the resolving and whatever name it was given.
        RefusalCase {
            case: "start encryption on a name no stream has".to_string(),
            operation: "StartDeliveryStreamEncryption",
            request: json!({ "DeliveryStreamName": "ghost" }),
            code: "ResourceNotFoundException",
            message: encryption_not_found.clone(),
        },
        RefusalCase {
            case: "stop encryption on a name no stream has".to_string(),
            operation: "StopDeliveryStreamEncryption",
            request: json!({ "DeliveryStreamName": "z".repeat(64) }),
            code: "ResourceNotFoundException",
            message: encryption_not_found.clone(),
        },
        RefusalCase {
            case: "update a destination of a name no stream has".to_string(),
            operation: "UpdateDestination",
            request: json!({
                "DeliveryStreamName": "ghost-2",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "S3DestinationUpdate": { "Prefix": "x/" },
            }),
            code: "ResourceNotFoundException",
            message: not_found.clone(),
        },
        // An encryption change the stream's own status blocks, in both directions and for
        // both of the statuses that block it.
        RefusalCase {
            case: "enable encryption while the stream is creating".to_string(),
            operation: "StartDeliveryStreamEncryption",
            request: json!({ "DeliveryStreamName": "refusal-creating" }),
            code: "ResourceInUseException",
            message: "Cannot enable encryption on firehose: refusal-creating when it is \
                      in CREATING status"
                .to_string(),
        },
        RefusalCase {
            case: "disable encryption while the stream is creating".to_string(),
            operation: "StopDeliveryStreamEncryption",
            request: json!({ "DeliveryStreamName": "refusal-creating" }),
            code: "ResourceInUseException",
            message: "Cannot disable encryption on firehose: refusal-creating when it is \
                      in CREATING status"
                .to_string(),
        },
        RefusalCase {
            case: "enable encryption while the stream is deleting".to_string(),
            operation: "StartDeliveryStreamEncryption",
            request: json!({ "DeliveryStreamName": "refusal-deleting" }),
            code: "ResourceInUseException",
            message: "Cannot enable encryption on firehose: refusal-deleting when it is in \
                      DELETING status"
                .to_string(),
        },
        // Nothing to disable: the stream's encryption is stated as the fact it already
        // is, rather than as a status the call is waiting on.
        RefusalCase {
            case: "disable encryption that was never enabled".to_string(),
            operation: "StopDeliveryStreamEncryption",
            request: json!({ "DeliveryStreamName": "refusal-active" }),
            code: "InvalidArgumentException",
            message: "Cannot disable encryption on firehose: refusal-active when \
                      encryption is already disabled"
                .to_string(),
        },
        // An update the stream's own status blocks.
        RefusalCase {
            case: "update a destination while the stream is creating".to_string(),
            operation: "UpdateDestination",
            request: json!({
                "DeliveryStreamName": "refusal-creating",
                "CurrentDeliveryStreamVersionId": "1",
                "DestinationId": "destinationId-000000000001",
                "S3DestinationUpdate": { "Prefix": "x/" },
            }),
            code: "ResourceInUseException",
            message: "Cannot update firehose: refusal-creating when it is in CREATING \
                      status"
                .to_string(),
        },
    ];

    // A conditional update whose version is not the stream's: both ids are stated,
    // whichever value the request carried.
    for supplied in ["2", "7", "99999999"] {
        cases.push(RefusalCase {
            case: format!("update at version {supplied} when the stream is at {version}"),
            operation: "UpdateDestination",
            request: json!({
                "DeliveryStreamName": "refusal-active",
                "CurrentDeliveryStreamVersionId": supplied,
                "DestinationId": destination_id,
                "S3DestinationUpdate": { "Prefix": "x/" },
            }),
            code: "ConcurrentModificationException",
            message: format!(
                "Cannot update firehose: refusal-active since the current version id: \
                 {version} and specified version id: {supplied} do not match"
            ),
        });
    }

    // An update naming a destination the stream does not have: the id the request named,
    // in whichever form a `DestinationId` may be written.
    for unknown in ["destinationId-000000000002", "abc-123", "a"] {
        cases.push(RefusalCase {
            case: format!("update the unknown destination {unknown}"),
            operation: "UpdateDestination",
            request: json!({
                "DeliveryStreamName": "refusal-active",
                "CurrentDeliveryStreamVersionId": version,
                "DestinationId": unknown,
                "S3DestinationUpdate": { "Prefix": "x/" },
            }),
            code: "InvalidArgumentException",
            message: format!("Destination Id {unknown} not found"),
        });
    }

    // Into the transient states, now: every row below runs within the span the clock owes.
    emulator.create_stream("refusal-creating", None).await;
    let (status, deleted) = emulator
        .call(
            "DeleteDeliveryStream",
            json!({ "DeliveryStreamName": "refusal-deleting" }),
        )
        .await;
    assert_eq!(status, 200, "the delete starts: got {deleted:?}");

    for case in &cases {
        let (status, refused) = emulator.call(case.operation, case.request.clone()).await;
        assert_eq!(status, 400, "case {:?}: got {refused:?}", case.case);
        assert_eq!(
            error_code(&refused),
            case.code,
            "case {:?}: got {refused:?}",
            case.case
        );
        let message = error_message(&refused);
        assert_eq!(
            message, case.message,
            "case {:?}: expected {:?}, got {message:?}",
            case.case, case.message
        );
    }
}

#[tokio::test]
async fn an_encryption_change_states_the_encryption_status_that_blocks_it() {
    let emulator = Emulator::new();
    emulator.create_stream("encryption-in-flight", None).await;
    tokio::time::sleep(PAST_THE_TRANSITION).await;

    // "Firehose first sets the encryption status of the stream to ENABLING, and then to
    // ENABLED": a second change while the first is in flight is refused, and the sentence
    // states the ENCRYPTION status rather than the stream's.
    let (status, started) = emulator
        .call(
            "StartDeliveryStreamEncryption",
            json!({ "DeliveryStreamName": "encryption-in-flight" }),
        )
        .await;
    assert_eq!(status, 200, "the enable starts: got {started:?}");

    for (case, operation, verb) in [
        (
            "a second enable while the first is in flight",
            "StartDeliveryStreamEncryption",
            "enable",
        ),
        (
            "a disable while the enable is in flight",
            "StopDeliveryStreamEncryption",
            "disable",
        ),
    ] {
        let (status, refused) = emulator
            .call(
                operation,
                json!({ "DeliveryStreamName": "encryption-in-flight" }),
            )
            .await;
        assert_eq!(status, 400, "case {case:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "ResourceInUseException",
            "case {case:?}: got {refused:?}"
        );
        let expected = format!(
            "Cannot {verb} encryption on firehose: encryption-in-flight when encryption \
             is in ENABLING status"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_create_of_a_sourced_stream_without_its_source_configuration_is_refused() {
    let emulator = Emulator::new();

    // One row per `DeliveryStreamType` that is fed by a source configuration
    // (`smithy_query(query="shape", shape="DeliveryStreamType")`), each named with the
    // configuration that type requires.
    let rows: Vec<(&str, &str)> = vec![
        ("KinesisStreamAsSource", "KinesisSourceStreamConfig"),
        ("MSKAsSource", "MSKSourceConfiguration"),
        ("DatabaseAsSource", "DatabaseSourceConfiguration"),
    ];

    for (stream_type, configuration) in rows {
        let (status, refused) = emulator
            .call(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": format!("sourceless-{stream_type}"),
                    "DeliveryStreamType": stream_type,
                    "S3DestinationConfiguration": s3_configuration(),
                }),
            )
            .await;
        assert_eq!(status, 400, "case {stream_type:?}: got {refused:?}");
        assert_eq!(
            error_code(&refused),
            "InvalidArgumentException",
            "case {stream_type:?}: got {refused:?}"
        );
        let expected = format!("{configuration} is mandatory for {stream_type} stream type.");
        let message = error_message(&refused);
        assert_eq!(
            message, expected,
            "case {stream_type:?}: expected {expected:?}, got {message:?}"
        );
    }
}
