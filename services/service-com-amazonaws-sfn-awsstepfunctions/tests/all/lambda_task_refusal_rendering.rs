// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the METADATA a refused `Task` state's cause closes with, driven
//! over both services' own wire protocols through the production `MultiServiceRouter` with
//! Step Functions and IAM mounted on ONE engine, the shape `core/router/src/lib.rs` mounts
//! them in, so the policies the call is judged against are the documents IAM's own
//! handlers wrote.
//!
//! RULE: a failed `Task`'s cause carries the invoked service's exception as the client that
//! made the call RENDERS it, and which client that is follows from the integration FLAVOUR
//! the `Resource` named. An OPTIMIZED `arn:aws:states:::<namespace>:<action>` integration is
//! served by the earlier generation of client, whose message closes with SEMICOLON-separated
//! fields: that client's own name for the service, the status, the code the answer named the
//! class by, the refused request and `Proxy: null`, with no attempt count. The AWS SDK family
//! and the LEGACY Lambda flavour — the function's own ARN as the `Resource` — are served by
//! the current AWS SDK, whose message closes with ONE group of comma-separated fields (the
//! service as that SDK names it, the status and the refused request) followed by a group
//! naming how many attempts the call took, and names neither an error code nor a proxy.
//!
//! The rows differ along what the rule generalises over: the FORM the `Resource` names the
//! call by (a Lambda function's ARN invoked directly, the optimized `lambda:invoke`
//! integration, and the same call through the AWS SDK family), and the SERVICE the call is
//! made to, one optimized integration of another service beside Lambda's.
//!
//! Only the DENIED side is here: the call is refused before it is made, so no target
//! service has to be mounted for the rendering to be observed.

use serde_json::{json, Value};

use crate::state_machine_lifecycle::REGION;
use crate::task_execution_role::{masked, Chain, ACCESS_DENIED_CODE, REQUEST_ID};

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The status a service whose front door answers an unauthorized call as a Forbidden
/// carries, which both services below do.
const FORBIDDEN: u16 = 403;

/// The code Amazon SQS's own front door names an authorization refusal by, which is not the
/// AWS-common one every other service here answers.
const SQS_REFUSAL_CODE: &str = "AccessDenied";

/// One row: the case, the `Resource` the `Task` names, the parameters it carries, the error
/// name the failed task reports, and the metadata its cause has to close with.
struct Row {
    case: &'static str,
    resource: String,
    parameters: Value,
    error: &'static str,
    metadata: String,
}

/// The metadata the CURRENT AWS SDK closes a refusal's message with, with the refused
/// request's id masked ([`masked`]).
///
/// `service` is the name that SDK's own client gives the invoked service.
fn current_sdk_metadata(service: &str, status: u16) -> String {
    format!(
        " (Service: {service}, Status Code: {status}, Request ID: {REQUEST_ID}) \
         (SDK Attempt Count: 1)"
    )
}

/// The metadata the EARLIER generation of client closes the same refusal with.
fn earlier_client_metadata(client: &str, code: &str, status: u16) -> String {
    format!(
        " (Service: {client}; Status Code: {status}; Error Code: {code}; \
         Request ID: {REQUEST_ID}; Proxy: null)"
    )
}

/// A definition whose one `Task` state makes `resource`'s call with `parameters`.
fn definition(resource: &str, parameters: &Value) -> String {
    json!({
        "StartAt": "Call",
        "States": {
            "Call": {
                "Type": "Task",
                "Resource": resource,
                "Parameters": parameters,
                "End": true,
            }
        }
    })
    .to_string()
}

/// A refused `Task` reports its cause as the client its integration FLAVOUR is served
/// through renders an exception: the earlier client's form for an optimized `Resource`, and
/// the current SDK's form for the function's own ARN and for the AWS SDK family.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refusal_is_rendered_by_the_client_its_integration_is_served_through() {
    let chain = Chain::new();
    // Neither target has to exist: authorization is settled before the call is made, so
    // these name what a request addresses and nothing is created for them.
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:handler");
    let queue_url = format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT}/intake");
    // The execution role may write its own logs and nothing else, so every row's call is
    // refused for want of an identity-based grant.
    let role_arn = chain
        .role(
            "RenderingRole",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": ["logs:CreateLogDelivery", "logs:PutLogEvents"],
                    "Resource": "*",
                }],
            }),
        )
        .await;

    let rows = [
        Row {
            case: "a function's ARN invoked directly",
            resource: function_arn.clone(),
            parameters: json!({ "event": "ignored" }),
            error: "Lambda.AWSLambdaException",
            metadata: current_sdk_metadata("Lambda", FORBIDDEN),
        },
        Row {
            // The control: the same invocation named through the OPTIMIZED integration is
            // made with the client that integration carries, whose rendering names the
            // answered error code and the proxy and counts no attempts.
            case: "the optimized lambda:invoke integration",
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            error: "Lambda.AWSLambdaException",
            metadata: earlier_client_metadata("AWSLambda", ACCESS_DENIED_CODE, FORBIDDEN),
        },
        Row {
            // The same invocation named through the AWS SDK family, which is made with the
            // current SDK too.
            case: "the same invocation through the AWS SDK integration family",
            resource: "arn:aws:states:::aws-sdk:lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            error: "Lambda.AWSLambdaException",
            metadata: current_sdk_metadata("Lambda", FORBIDDEN),
        },
        Row {
            // Another service's optimized integration, whose own front door names an
            // authorization refusal by a code of its own.
            case: "an optimized send to a queue, served by the earlier client",
            resource: "arn:aws:states:::sqs:sendMessage".to_string(),
            parameters: json!({ "QueueUrl": queue_url, "MessageBody": "hello" }),
            error: "SQS.AmazonSQSException",
            metadata: earlier_client_metadata("AmazonSQS", SQS_REFUSAL_CODE, FORBIDDEN),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine = chain
            .machine(
                &format!("rendered-{index}"),
                &definition(&row.resource, &row.parameters),
                &role_arn,
            )
            .await;
        let (_execution, described) = chain.ran(&machine, "only", case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the refused call to fail the execution, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(row.error),
            "case {case:?}: expected the error {:?}, got {described:?}",
            row.error
        );
        let cause = masked(
            described
                .get("cause")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        assert!(
            cause.ends_with(&row.metadata),
            "case {case:?}: expected the cause to close {:?}, got {cause:?}",
            row.metadata
        );
        // The sentence before the metadata is the service's own refusal, so the whole
        // cause is that sentence and nothing but this one group of fields after it.
        let sentence = cause
            .strip_suffix(&row.metadata)
            .expect("the metadata was just asserted to close the cause");
        assert!(
            !sentence.contains("(Service:"),
            "case {case:?}: expected the metadata to be written once, got {cause:?}"
        );
    }
}

/// One row of the field table: the case, the `Resource` and its parameters, the fields the
/// cause has to carry, and the fields it must not.
struct Written {
    case: &'static str,
    resource: String,
    parameters: Value,
    carried: Vec<String>,
    uncarried: Vec<&'static str>,
}

/// The fields each rendering names are not optional, and the two Lambda flavours carry
/// different ones: the optimized integration's cause names the answered error code and the
/// proxy and counts no attempts, while the legacy function-ARN flavour's names neither the
/// code nor a proxy and closes with the attempt count instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_lambda_flavour_names_the_fields_its_own_client_writes() {
    let chain = Chain::new();
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:handler");
    let role_arn = chain
        .role(
            "AttemptCountRole",
            &json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Action": "lambda:GetFunction",
                    "Resource": "*",
                }],
            }),
        )
        .await;

    let rows = [
        Written {
            case: "a function's ARN invoked directly",
            resource: function_arn.clone(),
            parameters: json!({}),
            carried: vec![
                format!(" (Service: Lambda, Status Code: {FORBIDDEN}, Request ID: "),
                "(SDK Attempt Count: ".to_string(),
            ],
            uncarried: vec!["Error Code: ", "Proxy: null", "Service: AWSLambda"],
        },
        Written {
            case: "the optimized lambda:invoke integration",
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            carried: vec![
                format!(" (Service: AWSLambda; Status Code: {FORBIDDEN}; Error Code: "),
                // The denial's own code is what a client classifying the failure reads.
                format!("Error Code: {ACCESS_DENIED_CODE}"),
                "Proxy: null".to_string(),
            ],
            uncarried: vec!["SDK Attempt Count"],
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        let machine = chain
            .machine(
                &format!("attempts-{index}"),
                &definition(&row.resource, &row.parameters),
                &role_arn,
            )
            .await;
        let (_execution, described) = chain.ran(&machine, "only", case).await;
        let cause = described
            .get("cause")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        for named in row.carried {
            assert!(
                cause.contains(&named),
                "case {case:?}: expected the cause to carry {named:?}, got {cause:?}"
            );
        }
        for absent in row.uncarried {
            assert!(
                !cause.contains(absent),
                "case {case:?}: expected the cause to carry no {absent:?}, got {cause:?}"
            );
        }
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some("Lambda.AWSLambdaException"),
            "case {case:?}: expected Lambda's base exception, got {described:?}"
        );
    }
}
