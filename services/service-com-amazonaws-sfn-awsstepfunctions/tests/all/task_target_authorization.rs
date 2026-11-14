// SPDX-License-Identifier: CC0-1.0

//! The access request a `Task` state's integration call is subject to, over the
//! integrations whose request addresses its target by something OTHER than that
//! resource's ARN — driven over both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions and IAM mounted on ONE engine.
//!
//! The rule under test: a `Task` state's integration call is made as the state machine's
//! `roleArn`, and the access request it is judged by is the integration's OWN — the IAM
//! action the invoked operation PUBLISHES (`lambda:InvokeFunction` for an invocation,
//! which is not the operation's name) over the ARN of the resource the request addresses,
//! whether the request names it by that ARN, by a queue's url or by a function's name. A
//! call the policies in force on the role do not allow is NOT made: the task fails with
//! the invoked service's own access-denied refusal — reported under the error that
//! operation MODELS for an unauthorized call, and under the service's base SDK exception
//! for one that models none — and the execution ends `FAILED`.
//!
//! The rows differ along what the rule generalises over: which integration the `Resource`
//! names, which family names it (the optimized one and the AWS SDK one, which make the
//! same call on the same resource) and the form the request addresses the target in.
//!
//! Only the DENIED side is here, and that is the point: the call is refused before it is
//! made, so neither target service has to be mounted for the rule to be observed. The
//! GRANTED side needs those services to answer, so it lives in
//! `core/router/tests/all/sfn_task_target_authorization.rs` beside the rest of that
//! cross-service story; `tests/all/task_execution_role.rs` is the suite for what the
//! POLICIES may say, over the integration that addresses its target by an ARN, and this
//! one borrows its harness rather than restating the wiring.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_TASK_FAILED;

use crate::state_machine_lifecycle::{string, REGION};
use crate::task_execution_role::{Chain, REFUSAL_CODE};

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

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

/// One row: the case, the `Resource` the `Task` names, its parameters, the IAM action the
/// refusal must name, the ARN it must be refused ON, the error name a `Task` state
/// reports for it, and the code the answer it stands in for came back under.
struct Row {
    case: &'static str,
    resource: &'static str,
    parameters: Value,
    /// Spelled with the PREFIX the invoked service publishes its own actions under,
    /// which is its ARN namespace for all but Amazon SNS — an action of that service is
    /// `SNS:<ApiName>` ("Actions defined by Amazon Simple Notification Service", IAM
    /// Service Authorization Reference) — because the refusal is that service's own
    /// sentence.
    action: &'static str,
    resource_arn: String,
    refusal_error: &'static str,
    /// The `Error Code:` fragment the cause carries: the code the invoked operation's
    /// OWN answer to an unauthorized call names its class by, which is
    /// [`REFUSAL_CODE`] for an operation that models no such error.
    refusal_code: &'static str,
}

/// A call whose published action the execution role is not granted is refused with the
/// invoked service's own access-denied sentence, over the ARN of what the request
/// addressed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_integration_call_is_judged_by_its_own_action_and_addressed_resource() {
    let chain = Chain::new();
    // Neither target has to exist: authorization is settled before the call, so these
    // name the resources a request addresses and nothing is created for them.
    let queue_url = format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT}/intake");
    let queue_arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:intake");
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:handler");
    let topic_arn = format!("arn:aws:sns:{REGION}:{ACCOUNT}:orders");
    let role_arn = chain
        .role(
            "TargetActionsRole",
            &json!({
                "Version": "2012-10-17",
                // The log writes a state machine makes, which say nothing about any
                // integration call.
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
            case: "a send to a queue, addressed by its url",
            resource: "arn:aws:states:::sqs:sendMessage",
            parameters: json!({ "QueueUrl": queue_url.clone(), "MessageBody": "hello" }),
            action: "sqs:SendMessage",
            resource_arn: queue_arn.clone(),
            refusal_error: "SQS.AmazonSQSException",
            refusal_code: REFUSAL_CODE,
        },
        Row {
            // The other family makes the same call on the same queue, so the refusal
            // must hold through it too.
            case: "the same send through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:sqs:sendMessage",
            parameters: json!({ "queueUrl": queue_url.clone(), "messageBody": "hello" }),
            action: "sqs:SendMessage",
            resource_arn: queue_arn,
            refusal_error: "SQS.AmazonSQSException",
            refusal_code: REFUSAL_CODE,
        },
        // `Publish` MODELS the refusal of an unauthorized call
        // (`AuthorizationErrorException`, HTTP 403 under the query code
        // `AuthorizationError`), so the task reports THAT error rather than the
        // service's base SDK exception, and the cause carries that code.
        Row {
            case: "a publish to a topic, addressed by its ARN",
            resource: "arn:aws:states:::sns:publish",
            parameters: json!({ "TopicArn": topic_arn.clone(), "Message": "hello" }),
            action: "SNS:Publish",
            resource_arn: topic_arn.clone(),
            refusal_error: "SNS.AuthorizationErrorException",
            refusal_code: "Error Code: AuthorizationError",
        },
        Row {
            case: "the same publish through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:sns:publish",
            parameters: json!({ "TopicArn": topic_arn.clone(), "Message": "hello" }),
            action: "SNS:Publish",
            resource_arn: topic_arn,
            refusal_error: "SNS.AuthorizationErrorException",
            refusal_code: "Error Code: AuthorizationError",
        },
        Row {
            case: "an invocation, the function named by its ARN",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            action: "lambda:InvokeFunction",
            resource_arn: function_arn.clone(),
            refusal_error: "Lambda.AWSLambdaException",
            refusal_code: REFUSAL_CODE,
        },
        Row {
            case: "the same invocation through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:lambda:invoke",
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            action: "lambda:InvokeFunction",
            resource_arn: function_arn.clone(),
            refusal_error: "Lambda.AWSLambdaException",
            refusal_code: REFUSAL_CODE,
        },
        Row {
            // A bare function name names the same function, so it is the same access
            // request: the ARN is built in the Region the call is made in.
            case: "the same invocation, the function named by its bare name",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: json!({ "FunctionName": "handler" }),
            action: "lambda:InvokeFunction",
            resource_arn: function_arn.clone(),
            refusal_error: "Lambda.AWSLambdaException",
            refusal_code: REFUSAL_CODE,
        },
        Row {
            // The partial form Lambda documents names it too.
            case: "the same invocation, the function named by its partial ARN",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: json!({ "FunctionName": format!("{ACCOUNT}:function:handler") }),
            action: "lambda:InvokeFunction",
            resource_arn: function_arn,
            refusal_error: "Lambda.AWSLambdaException",
            refusal_code: REFUSAL_CODE,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine_name = format!("target-action-{index}");
        let machine = chain
            .machine(
                &machine_name,
                &definition(row.resource, &row.parameters),
                &role_arn,
            )
            .await;
        let (execution_arn, described) = chain.ran(&machine, "only", case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected FAILED, got {described:?}"
        );
        assert_eq!(
            described.get("error").and_then(Value::as_str),
            Some(row.refusal_error),
            "case {case:?}: expected {:?}, got {described:?}",
            row.refusal_error
        );
        let events = chain.history(&execution_arn).await;
        let failed: Vec<&Value> = events
            .iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some(EVENT_TASK_FAILED))
            .collect();
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: expected exactly one {EVENT_TASK_FAILED}, got {events:?}"
        );
        let details = failed[0]
            .get("taskFailedEventDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected taskFailedEventDetails"));
        let cause = string(details, "cause");
        assert!(
            cause.contains(row.refusal_code),
            "case {case:?}: expected the cause to carry {:?}, got {cause:?}",
            row.refusal_code
        );
        assert!(
            cause.contains(row.action),
            "case {case:?}: expected the cause to name the action {:?}, got {cause:?}",
            row.action
        );
        assert!(
            cause.contains(&format!("on resource: {} ", row.resource_arn)),
            "case {case:?}: expected the cause to name the resource {}, got {cause:?}",
            row.resource_arn
        );
        // The principal is the SESSION of the role the call was made as, under the name
        // the assume gave it.
        let principal =
            crate::task_refusal_session_principal::session_principal(&role_arn, &cause);
        assert!(
            cause.contains(&principal),
            "case {case:?}: expected the cause to name the principal {principal}, got {cause:?}"
        );
    }
}
