// SPDX-License-Identifier: CC0-1.0

//! The STATUS a refused `Task` state's cause reports for the invoked service's answer,
//! driven over both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions and IAM mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so the documents the call is judged against
//! are the ones IAM's own handlers wrote.
//!
//! RULE: the refusal a `Task` state reports for a call the execution role may not make is
//! the answer the INVOKED SERVICE would have given, so the metadata an AWS SDK renders
//! beside the sentence is that service's own: its client's name, the code it names the
//! class by, and the status IT answers an unauthorized call with. That status is not one
//! value for every service — it is a Forbidden for the services that answer an
//! authorization failure as one (AWS Lambda, Amazon SQS, Amazon S3, Amazon SNS, AWS STS)
//! and the client-error status where the invoked service's front door answers one as that
//! (Step Functions' own `states:…` calls) — and where the invoked OPERATION models an
//! access-denied error of its own, the status and code are that error's modelled ones
//! (`sns:Publish`'s `AuthorizationError`) rather than the unmodelled
//! `AccessDeniedException` pair.
//!
//! The rows differ along what the rule generalises over: which service the refused call
//! was to and therefore which status and client name the metadata carries, whether the
//! invoked operation MODELS the refusal, and the form the `Task` names its work in (the
//! optimized integration token and the function's own ARN, which is reported in the
//! Lambda event family rather than the task one).
//!
//! The PRINCIPAL the same sentence names is `tests/all/task_refusal_session_principal.rs`,
//! whose `session_principal` spells it and whose `definition` and `unrelated_policy` this
//! suite drives its rows through; what the policies may SAY is
//! `tests/all/task_execution_role.rs`, whose harness (`Chain`) this suite borrows —
//! `tests/all.rs` is one crate root.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    EVENT_LAMBDA_FUNCTION_FAILED, EVENT_TASK_FAILED,
};

use crate::state_machine_lifecycle::{string, ACCOUNT, REGION};
use crate::task_execution_role::Chain;
use crate::task_refusal_session_principal::{definition, session_principal, unrelated_policy};

/// The code an answer names the class of an unauthorized call by, where the invoked
/// operation models no error of its own for one.
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The status a service that answers an authorization failure as a Forbidden carries.
const FORBIDDEN: u16 = 403;

/// The status a service whose front door answers every client error as a Bad Request
/// carries for the same refusal.
const CLIENT_ERROR: u16 = 400;

/// One row: the `Resource` the `Task` names, the request it makes, the IAM action the
/// refusal names, the ARN it is refused ON, and the answer it stands in for — the error
/// name the state reports, the invoked service's client name, the code it names the class
/// by and the status it carries.
struct Row {
    case: &'static str,
    resource: String,
    parameters: Value,
    action: &'static str,
    target: String,
    because: String,
    refusal_error: &'static str,
    /// The name the invoked service's own SDK client carries, which its exception reports
    /// the service under.
    sdk_client: &'static str,
    /// The code the answer names the class by: the invoked operation's modelled one where
    /// it models the refusal, and [`ACCESS_DENIED_CODE`] where it models none.
    code: &'static str,
    /// The status that answer carries.
    status: u16,
    /// The history event the failed work is reported in: a direct function invocation is
    /// reported in the Lambda family, every other integration in the task family.
    failure_event: &'static str,
}

/// The clause the sentence ends with when the documents in force state nothing that
/// allows the call.
fn unstated(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The member the details of `event` sit under, as the history models them.
fn details_member(event: &str) -> &'static str {
    match event {
        EVENT_LAMBDA_FUNCTION_FAILED => "lambdaFunctionFailedEventDetails",
        _ => "taskFailedEventDetails",
    }
}

/// RULE: the cause of a refused `Task` carries the invoked service's own answer — its
/// client, the code it names the class by and the status IT answers an unauthorized call
/// with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_task_reports_the_invoked_services_own_status() {
    let chain = Chain::new();
    // No target has to exist: the call is refused before it is made, so these only name
    // what a request addresses.
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:handler");
    let queue_url = format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT}/intake");
    let queue_arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:intake");
    let topic_arn = format!("arn:aws:sns:{REGION}:{ACCOUNT}:orders");
    let child_arn = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:child");
    let invoke_action = "lambda:InvokeFunction";
    let role_arn = chain.role("TaskRefusalStatusRole", &unrelated_policy()).await;

    let rows = [
        Row {
            case: "an invocation through the optimized integration",
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            action: invoke_action,
            target: function_arn.clone(),
            because: unstated(invoke_action),
            refusal_error: "Lambda.AWSLambdaException",
            sdk_client: "AWSLambda",
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_TASK_FAILED,
        },
        Row {
            // The direct form — the `Resource` IS the function — is the same call on the
            // same resource, so it stands in for the same answer; the work it names is
            // reported in the Lambda event family.
            case: "the same invocation named by the function's own ARN",
            resource: function_arn.clone(),
            parameters: json!({}),
            action: invoke_action,
            target: function_arn,
            because: unstated(invoke_action),
            refusal_error: "Lambda.AWSLambdaException",
            sdk_client: "AWSLambda",
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_LAMBDA_FUNCTION_FAILED,
        },
        Row {
            case: "a send to a queue",
            resource: "arn:aws:states:::sqs:sendMessage".to_string(),
            parameters: json!({ "QueueUrl": queue_url, "MessageBody": "hello" }),
            action: "sqs:SendMessage",
            target: queue_arn,
            because: unstated("sqs:SendMessage"),
            refusal_error: "SQS.AmazonSQSException",
            sdk_client: "AmazonSQS",
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_TASK_FAILED,
        },
        Row {
            // `Publish` MODELS the refusal of an unauthorized call, so the code and the
            // status are that error's own rather than the unmodelled pair. Amazon SNS
            // publishes its actions under a prefix of its own (`Api::action_prefix`),
            // which is what the sentence names.
            case: "a publish to a topic, whose operation models the refusal",
            resource: "arn:aws:states:::sns:publish".to_string(),
            parameters: json!({ "TopicArn": topic_arn.clone(), "Message": "hello" }),
            action: "SNS:Publish",
            target: topic_arn,
            because: unstated("SNS:Publish"),
            refusal_error: "SNS.AuthorizationErrorException",
            sdk_client: "AmazonSNS",
            code: "AuthorizationError",
            status: FORBIDDEN,
            failure_event: EVENT_TASK_FAILED,
        },
        Row {
            // Step Functions' own front door answers an authorization failure as a client
            // error, so the same refusal reaches the state under a different status.
            case: "a child execution of this service itself",
            resource: "arn:aws:states:::states:startExecution".to_string(),
            parameters: json!({ "StateMachineArn": child_arn.clone() }),
            action: "states:StartExecution",
            target: child_arn,
            because: unstated("states:StartExecution"),
            refusal_error: "StepFunctions.AWSStepFunctionsException",
            sdk_client: "AWSStepFunctions",
            code: ACCESS_DENIED_CODE,
            status: CLIENT_ERROR,
            failure_event: EVENT_TASK_FAILED,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine_name = format!("refusal-status-{index}");
        let machine = chain
            .machine(
                &machine_name,
                &definition(&row.resource, &row.parameters),
                &role_arn,
            )
            .await;
        let (execution_arn, described) = chain.ran(&machine, "only", case).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(row.refusal_error)),
            "case {case:?}: expected FAILED with {:?}, got {described:?}",
            row.refusal_error
        );

        // The message, as the invoked service's own SDK renders its exception: the
        // sentence — whose principal is the session of the execution role — then the
        // metadata that answer carried, up to the identifier of the request. That
        // identifier is one run's own value, so only everything around it is asserted:
        // the service, the status it answers an unauthorized call with, the code it names
        // the class by, and the tail an SDK closes such a message with.
        let principal = session_principal(&role_arn, &machine_name);
        let expected = format!(
            "User: {principal} is not authorized to perform: {} on resource: {} {} (Service: {}; \
             Status Code: {}; Error Code: {}; ",
            row.action, row.target, row.because, row.sdk_client, row.status, row.code
        );
        let cause = string(&described, "cause");
        assert!(
            cause.starts_with(&expected) && cause.ends_with("Proxy: null)"),
            "case {case:?}: expected the cause to open {expected:?} and close \"Proxy: null)\", \
             got {cause:?}"
        );

        // Read back through the history the failed work was reported in: the same message
        // reaches a `Catch` path there.
        let events = chain.history(&execution_arn).await;
        let member = details_member(row.failure_event);
        let reported: Vec<String> = events
            .iter()
            .filter(|event| event.get("type").and_then(Value::as_str) == Some(row.failure_event))
            .map(|event| {
                string(
                    event
                        .get(member)
                        .unwrap_or_else(|| panic!("case {case:?}: expected {member}")),
                    "cause",
                )
            })
            .collect();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly one {}, got {events:?}",
            row.failure_event
        );
        assert_eq!(
            reported[0], cause,
            "case {case:?}: expected the work's own cause to be the one the execution reports \
             {cause:?}, got {:?}",
            reported[0]
        );
    }
}
