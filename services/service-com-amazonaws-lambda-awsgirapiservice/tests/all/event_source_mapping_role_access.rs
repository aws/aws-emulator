// SPDX-License-Identifier: CC0-1.0

//! What the EXECUTION ROLE of the invoked function has to be allowed before Lambda will
//! wire that function to an event source.
//!
//! RULE: a call that wires a function to an event source — `CreateEventSourceMapping`, and
//! `UpdateEventSourceMapping` when it re-targets a mapping onto another function — is
//! refused with the modelled `InvalidParameterValueException` (HTTP 400), persisting
//! nothing, when the identity-based policies in force on that function's execution role do
//! not allow every read Lambda's poller of that KIND of event source makes on it; the
//! refusal is worded as AWS words it for the source's own error class. Lambda reads the
//! source acting as that role, so a mapping its policies do not cover is a trigger that
//! could never fire.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with IAM
//! mounted beside Lambda on ONE engine — each role is created through the real `CreateRole`,
//! `PutRolePolicy` and `AttachRolePolicy`, so what the gate reads is the document IAM
//! persisted and not a copy — and reads the outcome back through `GetEventSourceMapping`
//! and `ListEventSourceMappings`, so a mapping that was refused but still stored fails here.
//!
//! The rows differ along what the rule generalises over: WHICH event source the mapping
//! polls (each of the two shard sources, a queue, and a source this gate states no reads
//! for), WHICH of that source's reads the role is granted (all, all but one, none, one of
//! them explicitly denied beside the grant), HOW the grant names the action and the resource
//! (exactly, through a namespace wildcard, through an ARN wildcard, on another resource of
//! the same kind), and which call states the pair. The second and third tests pin the
//! complements: the ceiling a permissions boundary puts on those grants, and the cases where
//! nothing is KNOWN about the role and so nothing is refused.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::EVENT_SOURCE_MAPPING_SETTLE_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, create_mapping, get_mapping, list_mappings, router_on, Answer, DYNAMODB_SOURCE,
    KINESIS_SOURCE, MAPPINGS_URI, MSK_SOURCE, QUEUE_SOURCE,
};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The wire code of the modelled 400 every refusal here is carried by.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The service principal a function's execution role has to let assume it, so that
/// `CreateFunction` accepts the role at all.
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// How long past a due instant this suite waits before re-targeting a mapping: the settle
/// out of `Creating` is owed by the CLOCK, and an update inside that window is refused for
/// the mapping's availability rather than for its role.
const SLACK: std::time::Duration = std::time::Duration::from_millis(100);

/// Another stream of the same account, for the rows whose grant names the wrong resource.
const OTHER_KINESIS_STREAM: &str = "arn:aws:kinesis:us-east-1:000000000000:stream/other-events";

/// A stream CONSUMER of that same stream, the other form `EventSourceArn` admits for
/// "Amazon Kinesis — the ARN of the data stream or a stream consumer".
const KINESIS_CONSUMER: &str =
    "arn:aws:kinesis:us-east-1:000000000000:stream/order-events/consumer/fanout:1700000000";

/// An AWS-managed policy ARN this emulator's IAM does not carry a document for, which is
/// what a role carrying an unreadable grant is attached below.
const UNCARRIED_MANAGED_POLICY: &str =
    "arn:aws:iam::aws:policy/service-role/AWSLambdaKinesisExecutionRole";

/// The actions the refusal of an Amazon Kinesis data stream lists, in its order. Source:
/// the actions AWS's own managed execution-role policy for that event source
/// (`AWSLambdaKinesisExecutionRole`) grants.
pub(crate) const KINESIS_READS: [&str; 6] = [
    "GetRecords",
    "GetShardIterator",
    "DescribeStream",
    "DescribeStreamSummary",
    "ListShards",
    "ListStreams",
];

/// The same, for an Amazon DynamoDB stream. Source: `AWSLambdaDynamoDBExecutionRole`.
pub(crate) const DYNAMODB_READS: [&str; 4] = [
    "GetRecords",
    "GetShardIterator",
    "DescribeStream",
    "ListStreams",
];

/// The same, for an Amazon SQS queue. Source: `AWSLambdaSQSQueueExecutionRole`.
pub(crate) const QUEUE_READS: [&str; 3] = ["ReceiveMessage", "DeleteMessage", "GetQueueAttributes"];

/// Percent-encode one component of an `application/x-www-form-urlencoded` body, which is
/// how IAM's `awsQuery` parameters travel.
pub(crate) fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// Lambda over the SAME engine as IAM, so the gate resolves the execution role in the
/// service that owns it — exactly what `core/router` assembles.
pub(crate) fn router_with_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// One IAM call: IAM speaks `awsQuery`, so its parameters travel as a form body and its
/// answer comes back as XML.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (name, value) in params {
        body.push('&');
        body.push_str(&form_encode(name));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// The text of the first `<tag>` element of an XML body.
///
/// `pub(crate)` because every suite that reads a value back out of an IAM answer reads it
/// this one way (`tests/all/invoke_authorization.rs`).
pub(crate) fn element(body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("<{tag}> is absent from {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("<{tag}> is never closed in {body}"))
        + start;
    body[start..end].to_string()
}

/// A trust policy letting this service assume the role, which is what makes the role one
/// `CreateFunction` accepts.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": LAMBDA_PRINCIPAL },
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// Create the role `name`, with the statements `statements` as its one inline policy when
/// the row states any, and answer the ARN IAM persisted.
pub(crate) async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    statements: Option<Value>,
) -> String {
    let (status, created) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name),
            ("AssumeRolePolicyDocument", &trust_policy()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRole {name:?} to answer 200, got {status}: {created}"
    );
    if let Some(statements) = statements {
        let document = json!({ "Version": "2012-10-17", "Statement": statements }).to_string();
        let (status, written) = iam(
            router,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "event-source-reads"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRolePolicy on {name:?} to answer 200, got {status}: \
             {written}"
        );
    }
    element(&created, "Arn")
}

/// Create a function of `name` running as `role_arn`.
pub(crate) async fn create_function_as(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    role_arn: &str,
) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let answer = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: expected CreateFunction {name:?} to answer 201, got {} {}",
        answer.status,
        answer.text()
    );
}

/// `UpdateEventSourceMapping` on the mapping `uuid` names.
async fn update_mapping(router: &MultiServiceRouter, uuid: &str, body: &Value) -> Answer {
    call(router, "PUT", &format!("{MAPPINGS_URI}/{uuid}"), body).await
}

/// The action names as the shard-source sentence lists them: comma separated, with `and`
/// before the last.
fn listed(actions: &[&str]) -> String {
    let last = actions.len() - 1;
    actions
        .iter()
        .enumerate()
        .map(|(index, name)| match index {
            0 => (*name).to_string(),
            index if index == last => format!(", and {name}"),
            _ => format!(", {name}"),
        })
        .collect()
}

/// How AWS ends the Kinesis wording of this error class, after the noun of its last clause.
pub(crate) const KINESIS_TAIL: &str = ".";

/// How it ends the DynamoDB Streams wording of the same class: the tail is the source's own.
pub(crate) const DYNAMODB_TAIL: &str = " in IAM.";

/// How AWS words the refusal of a role that may not read a SHARD source: the source, every
/// read a reader of it makes, and the tail that source's wording ends on.
pub(crate) fn stream_refusal(event_source_arn: &str, actions: &[&str], tail: &str) -> String {
    format!(
        "Cannot access stream {event_source_arn}. Please ensure the role can perform the {} \
         Actions on your stream{tail}",
        listed(actions)
    )
}

/// How it words the refusal of a role that may not make one call on a QUEUE.
fn queue_refusal(action: &str) -> String {
    call_refusal(action, "SQS")
}

/// How AWS words the refusal of a role that may not make one CALL on the named source:
/// the call it may not make, and the service it would have been made to.
///
/// `pub(crate)` because this sentence has ONE owner here: the suite over its wording across
/// the source services reads it from this function too
/// (`tests/all/event_source_read_refusal_sentence.rs`), so the two tables cannot drift apart.
pub(crate) fn call_refusal(action: &str, service: &str) -> String {
    format!("The function execution role does not have permissions to call {action} on {service}")
}

/// A statement allowing `actions` of `namespace` on `resource`.
pub(crate) fn allow(namespace: &str, actions: &[&str], resource: &str) -> Value {
    json!({
        "Effect": "Allow",
        "Action": actions
            .iter()
            .map(|action| format!("{namespace}:{action}"))
            .collect::<Vec<_>>(),
        "Resource": resource,
    })
}

/// The statement every one of these roles carries whatever else it does: the log writes a
/// function makes, which say nothing about any event source.
pub(crate) fn logs_only() -> Value {
    json!({
        "Effect": "Allow",
        "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
        "Resource": "*",
    })
}

/// One row: an event source, the statements of the role's one inline policy, and the
/// sentence the create must be refused with (`None` when it must be served).
struct Row {
    case: &'static str,
    event_source: &'static str,
    statements: Value,
    refusal: Option<String>,
}

/// The catalog: for each kind of event source, which of its reads the role is granted and
/// how the grant names them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The recorded case: a role carrying only the log writes.
            case: "a kinesis stream, with only logs granted",
            event_source: KINESIS_SOURCE,
            statements: json!([logs_only()]),
            refusal: Some(stream_refusal(KINESIS_SOURCE, &KINESIS_READS, KINESIS_TAIL)),
        },
        Row {
            // The positive control: exactly the six reads, on the stream itself.
            case: "a kinesis stream, with every read granted on it",
            event_source: KINESIS_SOURCE,
            statements: json!([logs_only(), allow("kinesis", &KINESIS_READS, KINESIS_SOURCE)]),
            refusal: None,
        },
        Row {
            case: "a kinesis stream, granted through a namespace wildcard",
            event_source: KINESIS_SOURCE,
            statements: json!([allow("kinesis", &["*"], "*")]),
            refusal: None,
        },
        Row {
            case: "a kinesis stream, granted through an ARN wildcard",
            event_source: KINESIS_SOURCE,
            statements: json!([allow(
                "kinesis",
                &KINESIS_READS,
                "arn:aws:kinesis:us-east-1:000000000000:stream/*"
            )]),
            refusal: None,
        },
        Row {
            case: "a kinesis stream, with every read granted on another stream",
            event_source: KINESIS_SOURCE,
            statements: json!([allow("kinesis", &KINESIS_READS, OTHER_KINESIS_STREAM)]),
            refusal: Some(stream_refusal(KINESIS_SOURCE, &KINESIS_READS, KINESIS_TAIL)),
        },
        Row {
            // One of the three calls consuming the stream takes is absent, and those are
            // what the mapping is decided on.
            case: "a kinesis stream, with one read short of the set",
            event_source: KINESIS_SOURCE,
            statements: json!([allow(
                "kinesis",
                &[
                    "GetRecords",
                    "GetShardIterator",
                    "DescribeStreamSummary",
                    "ListShards",
                    "ListStreams"
                ],
                KINESIS_SOURCE
            )]),
            refusal: Some(stream_refusal(KINESIS_SOURCE, &KINESIS_READS, KINESIS_TAIL)),
        },
        Row {
            // IAM's deny precedence: an explicit `Deny` wins over the grant beside it.
            case: "a kinesis stream, with every read granted and one denied",
            event_source: KINESIS_SOURCE,
            statements: json!([
                allow("kinesis", &KINESIS_READS, KINESIS_SOURCE),
                {
                    "Effect": "Deny",
                    "Action": "kinesis:GetRecords",
                    "Resource": KINESIS_SOURCE,
                },
            ]),
            refusal: Some(stream_refusal(KINESIS_SOURCE, &KINESIS_READS, KINESIS_TAIL)),
        },
        Row {
            // The reads are the source service's own, so another service's are not them.
            case: "a dynamodb stream, with the kinesis reads granted instead",
            event_source: DYNAMODB_SOURCE,
            statements: json!([allow("kinesis", &KINESIS_READS, "*")]),
            refusal: Some(stream_refusal(
                DYNAMODB_SOURCE,
                &DYNAMODB_READS,
                DYNAMODB_TAIL
            )),
        },
        Row {
            case: "a dynamodb stream, with every read granted on it",
            event_source: DYNAMODB_SOURCE,
            statements: json!([allow("dynamodb", &DYNAMODB_READS, DYNAMODB_SOURCE)]),
            refusal: None,
        },
        Row {
            case: "a queue, with only logs granted",
            event_source: QUEUE_SOURCE,
            statements: json!([logs_only()]),
            refusal: Some(queue_refusal(QUEUE_READS[0])),
        },
        Row {
            case: "a queue, with one call short of the set",
            event_source: QUEUE_SOURCE,
            statements: json!([allow(
                "sqs",
                &["ReceiveMessage", "DeleteMessage"],
                QUEUE_SOURCE
            )]),
            refusal: Some(queue_refusal("GetQueueAttributes")),
        },
        Row {
            case: "a queue, with every call granted on it",
            event_source: QUEUE_SOURCE,
            statements: json!([allow("sqs", &QUEUE_READS, QUEUE_SOURCE)]),
            refusal: None,
        },
        Row {
            // A cluster is reached through a DESCRIBE of the named resource first, so the
            // role that may not make it is refused by the call that names the cluster.
            case: "an msk cluster, with only logs granted",
            event_source: MSK_SOURCE,
            statements: json!([logs_only()]),
            refusal: Some(call_refusal("DescribeClusterV2", "MSK")),
        },
        Row {
            case: "an msk cluster, with the cluster describe granted on it",
            event_source: MSK_SOURCE,
            statements: json!([
                logs_only(),
                allow("kafka", &["DescribeClusterV2"], MSK_SOURCE)
            ]),
            refusal: None,
        },
        Row {
            // A stream consumer is read on the stream AND on the consumer, two resources
            // rather than one, so this gate states no reads for it either.
            case: "a kinesis stream consumer, with only logs granted",
            event_source: KINESIS_CONSUMER,
            statements: json!([logs_only()]),
            refusal: None,
        },
    ]
}

/// The `CreateEventSourceMapping` body for one row: a shard source also needs the
/// `StartingPosition` the model makes "Required for Amazon Kinesis and Amazon DynamoDB
/// Stream event sources".
pub(crate) fn create_body(function: &str, event_source: &str) -> Value {
    let mut body = json!({ "FunctionName": function, "EventSourceArn": event_source });
    if event_source.starts_with("arn:aws:kinesis:") || event_source.starts_with("arn:aws:dynamodb:")
    {
        body["StartingPosition"] = json!("TRIM_HORIZON");
    }
    body
}

/// Assert `answer` is the modelled refusal carrying `expected`.
pub(crate) fn assert_refused(case: &str, answer: &Answer, expected: &str) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected HTTP 400, got {} {}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.error_code().contains(INVALID_PARAMETER),
        "case {case:?}: expected {INVALID_PARAMETER}, got {} in {}",
        answer.error_code(),
        answer.text()
    );
    let message = answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?} in {}",
        answer.text()
    );
}

#[tokio::test]
async fn a_mapping_is_created_only_where_the_execution_role_may_read_the_event_source() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router_with_iam();
        let role = given_role(
            &router,
            case,
            &format!("esm-reader-{index}"),
            Some(row.statements.clone()),
        )
        .await;
        let function = format!("esm-role-probe-{index}");
        create_function_as(&router, case, &function, &role).await;

        let answer = create_mapping(&router, &create_body(&function, row.event_source)).await;

        if let Some(expected) = row.refusal.as_deref() {
            assert_refused(case, &answer, expected);
            // The refusal is instead of the mapping, not beside it: nothing is left on the
            // function for a poller to pick up.
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
        // Round trip: the accepted mapping is one this emulator holds, on the source it was
        // made for.
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("EventSourceArn")),
            (200, Some(row.event_source.to_string())),
            "case {case:?}: the mapping must read back on {}, got {} {}",
            row.event_source,
            reading.status,
            reading.text()
        );
    }
}

#[tokio::test]
async fn re_targeting_a_mapping_is_held_to_the_same_grant() {
    let case = "a re-target onto a function whose role cannot read the stream";
    let router = router_with_iam();

    // Two roles: one granted every read of the stream, one carrying only the log writes.
    let reader = given_role(
        &router,
        case,
        "esm-retarget-reader",
        Some(json!([allow("kinesis", &KINESIS_READS, KINESIS_SOURCE)])),
    )
    .await;
    let blind = given_role(
        &router,
        case,
        "esm-retarget-blind",
        Some(json!([logs_only()])),
    )
    .await;
    create_function_as(&router, case, "esm-retarget-from", &reader).await;
    create_function_as(&router, case, "esm-retarget-blind-fn", &blind).await;
    create_function_as(&router, case, "esm-retarget-to", &reader).await;

    let created = create_mapping(&router, &create_body("esm-retarget-from", KINESIS_SOURCE)).await;
    assert_eq!(
        created.status,
        202,
        "case {case:?}: expected the mapping to be created, got {} {}",
        created.status,
        created.text()
    );
    let uuid = created
        .string("UUID")
        .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
    let function_arn = created
        .string("FunctionArn")
        .unwrap_or_else(|| panic!("case {case:?}: no FunctionArn in {}", created.text()));

    // The mapping is not available to any update while it is still settling, so the wait is
    // past the instant the create made the move due.
    tokio::time::sleep(EVENT_SOURCE_MAPPING_SETTLE_DELAY + SLACK).await;

    let refused = update_mapping(
        &router,
        &uuid,
        &json!({ "FunctionName": "esm-retarget-blind-fn" }),
    )
    .await;
    assert_refused(
        case,
        &refused,
        &stream_refusal(KINESIS_SOURCE, &KINESIS_READS, KINESIS_TAIL),
    );

    // And the refusal changed nothing: the mapping still invokes the function it was
    // created on.
    let reading = get_mapping(&router, &uuid).await;
    assert_eq!(
        (reading.status, reading.string("FunctionArn")),
        (200, Some(function_arn)),
        "case {case:?}: a refused re-target must leave the mapping on its function, got {} {}",
        reading.status,
        reading.text()
    );

    // A re-target onto a function whose role may read the stream is accepted.
    let accepted = update_mapping(
        &router,
        &uuid,
        &json!({ "FunctionName": "esm-retarget-to" }),
    )
    .await;
    assert_eq!(
        accepted.status,
        202,
        "case \"a re-target onto a reader\": expected HTTP 202, got {} {}",
        accepted.status,
        accepted.text()
    );
    assert!(
        accepted
            .string("FunctionArn")
            .is_some_and(|arn| arn.ends_with("esm-retarget-to")),
        "case \"a re-target onto a reader\": expected the mapping to move onto it, got {}",
        accepted.text()
    );
}

/// RULE (the ceiling): "a permissions boundary sets the maximum permissions that an
/// identity-based policy can grant to an entity", so a read the boundary does not allow is
/// withheld however widely the role's own policies grant it.
#[tokio::test]
async fn a_permissions_boundary_caps_what_the_role_may_read() {
    // (case, the boundary's statements, whether the mapping is created)
    let rows: [(&str, Value, bool); 3] = [
        (
            "a boundary allowing every read of the stream",
            json!([allow("kinesis", &KINESIS_READS, "*")]),
            true,
        ),
        (
            // Short of `GetShardIterator`, one of the three calls the mapping is decided on.
            "a boundary allowing one read short of the set",
            json!([allow(
                "kinesis",
                &[
                    "GetRecords",
                    "DescribeStream",
                    "DescribeStreamSummary",
                    "ListShards",
                    "ListStreams"
                ],
                "*"
            )]),
            false,
        ),
        (
            "a boundary allowing none of them",
            json!([logs_only()]),
            false,
        ),
    ];

    for (index, (case, boundary, accepted)) in rows.iter().enumerate() {
        let router = router_with_iam();
        let role_name = format!("esm-bounded-{index}");
        let role = given_role(
            &router,
            case,
            &role_name,
            Some(json!([allow("kinesis", &KINESIS_READS, KINESIS_SOURCE)])),
        )
        .await;
        let document = json!({ "Version": "2012-10-17", "Statement": boundary }).to_string();
        let (status, created) = iam(
            &router,
            "CreatePolicy",
            &[
                ("PolicyName", &format!("esm-boundary-{index}")),
                ("PolicyDocument", &document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreatePolicy to answer 200, got {status}: {created}"
        );
        let boundary_arn = element(&created, "Arn");
        let (status, bounded) = iam(
            &router,
            "PutRolePermissionsBoundary",
            &[
                ("RoleName", &role_name),
                ("PermissionsBoundary", &boundary_arn),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutRolePermissionsBoundary to answer 200, got {status}: \
             {bounded}"
        );

        let function = format!("esm-bounded-fn-{index}");
        create_function_as(&router, case, &function, &role).await;
        let answer = create_mapping(&router, &create_body(&function, KINESIS_SOURCE)).await;

        if *accepted {
            assert_eq!(
                answer.status,
                202,
                "case {case:?}: expected HTTP 202, got {} {}",
                answer.status,
                answer.text()
            );
        } else {
            assert_refused(
                case,
                &answer,
                &stream_refusal(KINESIS_SOURCE, &KINESIS_READS, KINESIS_TAIL),
            );
        }
    }
}

/// RULE (the complement): the refusal is grounded in what IAM's own store SHOWS about the
/// role. Where it shows nothing — there is no IAM mounted to ask, the store holds no such
/// role, or the role's only grant is an attachment whose document this account does not hold
/// — the mapping is created.
#[tokio::test]
async fn nothing_is_refused_where_the_store_shows_nothing_about_the_role() {
    // 1. No IAM mounted at all: this target carries Lambda alone.
    let case = "an emulator with no IAM to ask";
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function_as(
        &router,
        case,
        "esm-no-iam",
        "arn:aws:iam::000000000000:role/service-role/lambda-exec",
    )
    .await;
    let answer = create_mapping(&router, &create_body("esm-no-iam", KINESIS_SOURCE)).await;
    assert_eq!(
        answer.status,
        202,
        "case {case:?}: expected HTTP 202, got {} {}",
        answer.status,
        answer.text()
    );

    // 2. A role this account's IAM no longer holds: the function was created while it did,
    //    and the role was deleted afterwards.
    let case = "a role the store no longer holds";
    let router = router_with_iam();
    let role = given_role(&router, case, "esm-vanishing", Some(json!([logs_only()]))).await;
    create_function_as(&router, case, "esm-vanished-role", &role).await;
    for (action, params) in [
        (
            "DeleteRolePolicy",
            vec![
                ("RoleName", "esm-vanishing"),
                ("PolicyName", "event-source-reads"),
            ],
        ),
        ("DeleteRole", vec![("RoleName", "esm-vanishing")]),
    ] {
        let (status, body) = iam(&router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {action} to answer 200, got {status}: {body}"
        );
    }
    let answer = create_mapping(&router, &create_body("esm-vanished-role", KINESIS_SOURCE)).await;
    assert_eq!(
        answer.status,
        202,
        "case {case:?}: expected HTTP 202, got {} {}",
        answer.status,
        answer.text()
    );

    // 3. A role whose only grant is an AWS-managed policy this emulator carries no document
    //    for: that attachment might be the very one that carries the reads.
    let case = "a role carrying a grant this emulator cannot read";
    let router = router_with_iam();
    let role = given_role(&router, case, "esm-managed", None).await;
    let (status, attached) = iam(
        &router,
        "AttachRolePolicy",
        &[
            ("RoleName", "esm-managed"),
            ("PolicyArn", UNCARRIED_MANAGED_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected AttachRolePolicy to answer 200, got {status}: {attached}"
    );
    create_function_as(&router, case, "esm-managed-fn", &role).await;
    let answer = create_mapping(&router, &create_body("esm-managed-fn", KINESIS_SOURCE)).await;
    assert_eq!(
        answer.status,
        202,
        "case {case:?}: expected HTTP 202, got {} {}",
        answer.status,
        answer.text()
    );
}
