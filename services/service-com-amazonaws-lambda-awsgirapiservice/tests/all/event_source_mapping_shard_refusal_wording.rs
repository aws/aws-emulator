// SPDX-License-Identifier: CC0-1.0

//! How Lambda WORDS the refusal of a wiring call whose function's execution role may not
//! read the SHARD event source it names.
//!
//! RULE: when a mapping onto a shard event source is refused because the invoked function's
//! execution role is short one of the reads Lambda's poller makes on that source, the
//! modelled `InvalidParameterValueException` (HTTP 400) carries the sentence the SOURCE
//! SERVICE's own error class is worded with — `Cannot access stream <the EventSourceArn the
//! request named>. Please ensure the role can perform the <that source's read actions, comma
//! separated with `and` before the last> Actions on your stream` closed by the tail that
//! source's wording ends on, which is ` in IAM.` for an Amazon DynamoDB stream and `.` for
//! an Amazon Kinesis data stream. Only the ARN varies with the request; everything else is
//! fixed text of the source's own wording, so the tail is not shared between the two
//! sources.
//!
//! The first test is a table over what the rule generalises over: WHICH shard source is
//! named, WHICH ARN of that source (two different DynamoDB streams, of two tables and two
//! stream labels), and WHY the role falls short (nothing about the source granted at all,
//! the other shard service's reads granted instead, the reads granted on another stream of
//! the same service, one of the polled reads missing, every read granted with one explicitly
//! denied) — beside the served controls that prove the refusal is not unconditional, each
//! read back through `GetEventSourceMapping`.
//!
//! The second test drives the same rule on a stream this emulator MINTED: a table created
//! with `StreamSpecification` through DynamoDB's own API, mounted beside Lambda and IAM on
//! one engine, so the ARN in the sentence is the `LatestStreamArn` that table reports rather
//! than a constant written here — and the same pair is accepted once the role carries the
//! four reads on it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{
    allow, assert_refused, create_body, create_function_as, given_role, logs_only, router_with_iam,
    stream_refusal, DYNAMODB_READS, DYNAMODB_TAIL, KINESIS_READS, KINESIS_TAIL,
};
use crate::event_source_mappings::{
    create_mapping, get_mapping, list_mappings, router_on, DYNAMODB_SOURCE, KINESIS_SOURCE,
};

/// The region every ARN below carries and every call below signs for.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix of the DynamoDB service (its `OPERATIONS.md`, "Wire protocol").
const DYNAMODB_TARGET_PREFIX: &str = "DynamoDB_20120810";

/// A SECOND DynamoDB stream, of another table and another stream label: the rule is about
/// the source's wording, so no row is the only ARN its service is tested on.
const OTHER_DYNAMODB_STREAM: &str =
    "arn:aws:dynamodb:us-east-1:000000000000:table/shipments/stream/2026-11-02T08:15:00.000";

/// Another Kinesis data stream of the same account, for the row whose grant names the wrong
/// resource.
const OTHER_KINESIS_STREAM: &str = "arn:aws:kinesis:us-east-1:000000000000:stream/other-events";

/// How long the table of the second test is given to reach `ACTIVE` before the test is a
/// failure.
const BUDGET: Duration = Duration::from_secs(45);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The partition key the table of the second test is keyed on.
const KEY: &str = "pk";

/// One row: the source a mapping is asked for, the statements of the role's one inline
/// policy, and the sentence the create must be refused with (`None` when it must be served).
struct Row {
    case: &'static str,
    event_source: String,
    statements: Value,
    refusal: Option<String>,
}

/// The sentence a DynamoDB stream's refusal carries, for the stream `arn`.
fn dynamodb_refusal(arn: &str) -> Option<String> {
    Some(stream_refusal(arn, &DYNAMODB_READS, DYNAMODB_TAIL))
}

/// The sentence a Kinesis data stream's refusal carries, for the stream `arn`.
fn kinesis_refusal(arn: &str) -> Option<String> {
    Some(stream_refusal(arn, &KINESIS_READS, KINESIS_TAIL))
}

/// A statement denying `action` on `resource`, which is what IAM's deny precedence is read
/// through.
fn deny(action: &str, resource: &str) -> Value {
    json!({ "Effect": "Deny", "Action": action, "Resource": resource })
}

/// The catalog: for each shard source, the grant the role carries and the sentence that
/// source's error class is worded with.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The reported case: a role granted nothing about the stream at all.
            case: "a dynamodb stream, with only the log writes granted",
            event_source: DYNAMODB_SOURCE.to_string(),
            statements: json!([logs_only()]),
            refusal: dynamodb_refusal(DYNAMODB_SOURCE),
        },
        Row {
            // The same, on another table's stream: the ARN is the request's, the rest is the
            // source's own fixed text.
            case: "another table's dynamodb stream, with only the log writes granted",
            event_source: OTHER_DYNAMODB_STREAM.to_string(),
            statements: json!([logs_only()]),
            refusal: dynamodb_refusal(OTHER_DYNAMODB_STREAM),
        },
        Row {
            case: "a dynamodb stream, with its reads granted on another table's stream",
            event_source: DYNAMODB_SOURCE.to_string(),
            statements: json!([allow("dynamodb", &DYNAMODB_READS, OTHER_DYNAMODB_STREAM)]),
            refusal: dynamodb_refusal(DYNAMODB_SOURCE),
        },
        Row {
            // One of the three calls consuming a shard source takes is absent, and those are
            // what the mapping is decided on; the sentence still advises the whole grant.
            case: "a dynamodb stream, with one of the polled reads short",
            event_source: DYNAMODB_SOURCE.to_string(),
            statements: json!([allow(
                "dynamodb",
                &["GetRecords", "DescribeStream", "ListStreams"],
                DYNAMODB_SOURCE
            )]),
            refusal: dynamodb_refusal(DYNAMODB_SOURCE),
        },
        Row {
            case: "a dynamodb stream, with every read granted and one denied",
            event_source: DYNAMODB_SOURCE.to_string(),
            statements: json!([
                allow("dynamodb", &DYNAMODB_READS, DYNAMODB_SOURCE),
                deny("dynamodb:GetShardIterator", DYNAMODB_SOURCE),
            ]),
            refusal: dynamodb_refusal(DYNAMODB_SOURCE),
        },
        Row {
            // The served control: the refusal above is about the grant, not about the source.
            case: "a dynamodb stream, with every read granted on it",
            event_source: DYNAMODB_SOURCE.to_string(),
            statements: json!([logs_only(), allow("dynamodb", &DYNAMODB_READS, DYNAMODB_SOURCE)]),
            refusal: None,
        },
        Row {
            case: "a dynamodb stream, granted through a namespace wildcard",
            event_source: OTHER_DYNAMODB_STREAM.to_string(),
            statements: json!([allow("dynamodb", &["*"], "*")]),
            refusal: None,
        },
        Row {
            // The contrast: the other shard service words the same error class without the
            // DynamoDB tail.
            case: "a kinesis stream, with only the log writes granted",
            event_source: KINESIS_SOURCE.to_string(),
            statements: json!([logs_only()]),
            refusal: kinesis_refusal(KINESIS_SOURCE),
        },
        Row {
            case: "a kinesis stream, with the dynamodb reads granted instead",
            event_source: KINESIS_SOURCE.to_string(),
            statements: json!([allow("dynamodb", &DYNAMODB_READS, "*")]),
            refusal: kinesis_refusal(KINESIS_SOURCE),
        },
        Row {
            case: "a kinesis stream, with its reads granted on another stream",
            event_source: KINESIS_SOURCE.to_string(),
            statements: json!([allow("kinesis", &KINESIS_READS, OTHER_KINESIS_STREAM)]),
            refusal: kinesis_refusal(KINESIS_SOURCE),
        },
        Row {
            case: "a kinesis stream, with every read granted on it",
            event_source: KINESIS_SOURCE.to_string(),
            statements: json!([allow("kinesis", &KINESIS_READS, KINESIS_SOURCE)]),
            refusal: None,
        },
    ]
}

#[tokio::test]
async fn a_withheld_shard_source_read_is_refused_in_that_source_services_own_wording() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router_with_iam();
        let role = given_role(
            &router,
            case,
            &format!("shard-refusal-reader-{index}"),
            Some(row.statements.clone()),
        )
        .await;
        let function = format!("shard-refusal-probe-{index}");
        create_function_as(&router, case, &function, &role).await;

        let answer = create_mapping(&router, &create_body(&function, &row.event_source)).await;

        if let Some(expected) = row.refusal.as_deref() {
            assert_refused(case, &answer, expected);
            // The refusal is instead of the mapping, not beside it.
            let listed = list_mappings(&router, &format!("FunctionName={function}")).await;
            assert_eq!(
                (listed.status, listed.listed_uuids()),
                (200, Vec::new()),
                "case {case:?}: a refused create must leave no mapping behind, got {} {}",
                listed.status,
                listed.text()
            );
            continue;
        }

        assert_eq!(
            answer.status,
            202,
            "case {case:?}: expected HTTP 202, got {} {}",
            answer.status,
            answer.text()
        );
        let uuid = answer
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", answer.text()));
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("EventSourceArn")),
            (200, Some(row.event_source.clone())),
            "case {case:?}: the mapping must read back on {}, got {} {}",
            row.event_source,
            reading.status,
            reading.text()
        );
    }
}

/// Lambda, IAM and DynamoDB on ONE engine: the stream the second test names is one DynamoDB
/// itself minted for a table, and the role deciding it is one IAM holds.
///
/// `pub(crate)` because the suite over which values name one stream of one table mounts the
/// same three services (`tests/all/event_source_mapping_stream_arn_resolution.rs`), and two
/// harnesses for one assembly would be two things to keep in step.
pub(crate) fn router_with_iam_and_dynamodb() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One DynamoDB call, over its own `awsJson1_0` target.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{DYNAMODB_TARGET_PREFIX}.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A table keyed on [`KEY`] with a stream enabled, waited out to `ACTIVE`; answers the
/// `LatestStreamArn` that table reports.
///
/// `pub(crate)` for the reason [`router_with_iam_and_dynamodb`] is: a stream this emulator
/// MINTED is what both suites resolve an `EventSourceArn` against.
pub(crate) async fn given_streamed_table(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let (status, created) = dynamodb(
        router,
        "CreateTable",
        &json!({
            "TableName": name,
            "AttributeDefinitions": [{ "AttributeName": KEY, "AttributeType": "S" }],
            "KeySchema": [{ "AttributeName": KEY, "KeyType": "HASH" }],
            "BillingMode": "PAY_PER_REQUEST",
            "StreamSpecification": { "StreamEnabled": true, "StreamViewType": "NEW_AND_OLD_IMAGES" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateTable {name:?} to answer 200, got {status}: {created}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, read) = dynamodb(router, "DescribeTable", &json!({ "TableName": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeTable {name:?} to answer 200, got {status}: {read}"
        );
        let description = &read["Table"];
        if description["TableStatus"] == json!("ACTIVE") {
            let arn = description["LatestStreamArn"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !arn.is_empty(),
                "case {case:?}: the table reports no LatestStreamArn: {read}"
            );
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {read}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

#[tokio::test]
async fn a_minted_table_streams_refusal_names_the_stream_the_table_reports() {
    let case = "a mapping onto the stream a table of this emulator minted";
    let router = router_with_iam_and_dynamodb();
    let stream = given_streamed_table(&router, case, "shard-refusal-orders").await;

    // A role granted nothing about the table at all, which is what the reported run used.
    let blind = given_role(
        &router,
        case,
        "shard-refusal-blind",
        Some(json!([logs_only()])),
    )
    .await;
    create_function_as(&router, case, "shard-refusal-blind-fn", &blind).await;

    let refused = create_mapping(
        &router,
        &create_body("shard-refusal-blind-fn", &stream),
    )
    .await;
    assert_refused(
        case,
        &refused,
        &stream_refusal(&stream, &DYNAMODB_READS, DYNAMODB_TAIL),
    );
    let listed = list_mappings(&router, "FunctionName=shard-refusal-blind-fn").await;
    assert_eq!(
        (listed.status, listed.listed_uuids()),
        (200, Vec::new()),
        "case {case:?}: a refused create must leave no mapping behind, got {} {}",
        listed.status,
        listed.text()
    );

    // The same pair, with the four reads granted on that very stream: accepted, and held.
    let reader = given_role(
        &router,
        case,
        "shard-refusal-reader",
        Some(json!([
            logs_only(),
            allow("dynamodb", &DYNAMODB_READS, &stream)
        ])),
    )
    .await;
    create_function_as(&router, case, "shard-refusal-reader-fn", &reader).await;
    let created = create_mapping(
        &router,
        &create_body("shard-refusal-reader-fn", &stream),
    )
    .await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: expected HTTP 202, got {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    let reading = get_mapping(&router, &uuid).await;
    assert_eq!(
        (reading.status, reading.string("EventSourceArn")),
        (200, Some(stream.clone())),
        "case {case:?}: the mapping must read back on {stream}, got {} {}",
        reading.status,
        reading.text()
    );
}
