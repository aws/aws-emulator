// SPDX-License-Identifier: CC0-1.0

//! WHAT a refused `Task` state reports about the answer it stands in for — driven across
//! Step Functions, IAM, Lambda and Amazon SQS, mounted on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them.
//!
//! RULE: a `Task` state's integration call the execution role may not make is reported as
//! the answer the INVOKED SERVICE's own front door would have given, so every fact in the
//! cause is that service's: the HTTP STATUS it answers an unauthorized call with — a
//! Forbidden for AWS Lambda and Amazon SQS, the client-error status where the front door
//! answers one as that (Step Functions' own `states:` endpoint) — the code it names the
//! class by (the AWS-common `AccessDeniedException`, or the service's own where its front
//! door words it differently), and the id of that answer's own request, carried in the
//! rendering of the client that serves the integration and under the error name that
//! client's deserialization gives it (the invoked service's base SDK exception, for a
//! class no shape of the invoked operation's model declares). The status is never a
//! constant of this service's: an answer the invoked service really gave carries ITS
//! status (a function Lambda does not hold is a Not Found), and no refusal reports a
//! status the invoked service did not answer with.
//!
//! Three crates have to agree for that to hold — Step Functions decides and makes the
//! call, IAM holds the documents it is judged against, and Lambda and Amazon SQS answer
//! the calls that are made — so the suite lives here rather than in any of them, and
//! borrows the four-service wiring `sfn_task_target_authorization` already owns.
//!
//! The rows differ along what the rule generalises over: the form the `Task` names the
//! invocation in (the integration token, a bare function name, a partial ARN, the
//! function's own ARN), which family names it, WHICH service answered — and so which
//! status and which code the cause carries — and WHY the answer came back (the
//! authorization refusal, and a function the service does not hold, whose own status is a
//! different one).
//!
//! It is the ANSWER that is under test here, which is why this stands beside
//! `all/sfn_task_target_authorization.rs` rather than inside it: that suite asks what the
//! documents in force DECIDE (the access request a call is judged by, and that the target
//! is untouched), while this one holds the three facts a decided refusal reports about the
//! answer it stands in for — the invoked service's own status, the code it names the class
//! by, and an id of that answer's own request, never shared by two calls — over every form
//! the same call is named in. Its harness, its metadata rendering and its reading of a
//! history's causes are that suite's and `all/sfn_task_integration_grant_scope.rs`'s, so
//! nothing here is a second copy of either. (That suite's own reader of a history's causes
//! signs for the Region ITS harness runs in, so the reading below is this harness's.)
//!
//! The metadata a refusal of an OPTIMIZED Lambda call closes with is the EARLIER generation
//! of client's: that client's own name for the service, the status, the code the answer named
//! the class by, the refused request and `Proxy: null`, semicolon-separated and with no
//! attempt count, because that is the client Step Functions' optimized Lambda integration is
//! served through, whichever form its `Parameters` named the function in. The same call named
//! through the AWS SDK family, and the LEGACY form whose `Resource` IS the function's ARN, are
//! made with the current SDK, whose group of comma-separated fields names no error code and
//! counts the attempts instead. The class the
//! task reports is the invoked service's base exception for a code no shape of the invoked
//! operation's model declares: the recorded cause of such a refusal names that base
//! exception beside this rendering, so the two stand together.

use serde_json::{json, Value};

use crate::sfn_task_target_authorization::{
    allow, emulator, logs_only, machine, masked, named_request, queue, ran, refusal_metadata,
    role, session_principal, sfn, string, ACCOUNT, FORBIDDEN, REFUSAL_CODE, REGION,
    SQS_REFUSAL_CODE,
};

/// The IAM action an invocation is subject to: the one Lambda PUBLISHES for `Invoke`,
/// which is not the operation's own name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The status a service whose front door answers every client error as a BAD REQUEST
/// carries for the same refusal.
const CLIENT_ERROR: u16 = 400;

/// The status Lambda answers a call naming a function it does not hold with
/// (`smithy_query(query="operation",operation="Invoke",model_path="services/
/// service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
/// com-amazonaws-lambda-awsgirapiservice.json")`: `ResourceNotFoundException`,
/// `errorKind` client, `httpError` 404).
const NOT_FOUND: u16 = 404;

/// The class that answer names, which the same model declares.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The error a `Task` reports for a class no shape of `Invoke`'s model declares: Lambda's
/// base SDK exception, under the name the client that made the call writes for the
/// service — which is the same name whichever form named it, the optimized integration and
/// the AWS SDK family alike.
const LAMBDA_BASE_EXCEPTION: &str = "Lambda.AWSLambdaException";

/// The name the client the OPTIMIZED Lambda integration is served through gives the service,
/// and the name the current AWS SDK's own client gives it — for the AWS SDK family and for
/// the legacy form whose `Resource` is the function's own ARN.
const LAMBDA_CLIENT: &str = "AWSLambda";
/// See [`LAMBDA_CLIENT`].
const LAMBDA_SDK_CLIENT: &str = "Lambda";

/// The text an SDK exception names the status of the answer it came back from after.
const NAMES_THE_STATUS: &str = "Status Code: ";

/// One row: how the `Task` names its work, and the answer the cause has to report.
struct Row {
    case: &'static str,
    /// The `Resource` the `Task` names and the parameters it carries.
    resource: String,
    parameters: Value,
    /// The error name the failed execution reports.
    error: &'static str,
    /// The name the client that serves this call gives the invoked service.
    client: &'static str,
    /// The code that answer named the class by.
    code: &'static str,
    /// The status that answer carried, which is the invoked service's own.
    status: u16,
    /// The `(action, resource)` the refusal's sentence names, for a row the documents in
    /// force refused; `None` for a row whose call was MADE and answered.
    refused: Option<(&'static str, String)>,
}

/// Every cause the events of an execution's history carry, whichever event details carry
/// it: the failure of a `Task` state is reported under the details member of the event type
/// that state's work names — a task failure for an integration call, the Lambda family's
/// own event for a `Resource` that is a function's ARN — so what is read is the cause
/// itself rather than one member's spelling.
async fn history_causes(
    router: &::smithy_server_runtime::MultiServiceRouter,
    execution_arn: &str,
) -> Vec<String> {
    let answered = sfn(
        router,
        "GetExecutionHistory",
        &json!({ "executionArn": execution_arn, "maxResults": 100 }),
    )
    .await;
    let events = answered
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    events
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|members| members.values())
        .filter_map(|member| member.get("cause").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The status an SDK exception's message names, read back out of `cause`: the ONE status
/// such a message states, so a cause naming another is a cause naming an answer that was
/// never given.
fn answered_status(case: &str, cause: &str) -> u16 {
    let named: Vec<&str> = cause.split(NAMES_THE_STATUS).skip(1).collect();
    assert_eq!(
        named.len(),
        1,
        "case {case:?}: expected the cause to name exactly one status, got {cause:?}"
    );
    let digits: String = named[0].chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: expected a status, got {digits:?} in {cause:?}"))
}

/// A refused `Task` reports the status, the code and the request of the answer the invoked
/// service would have given, whichever form the work was named in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_task_reports_the_invoked_services_own_answer() {
    let router = emulator();
    let (queue_url, queue_arn) = queue(&router, "answered-target").await;
    // Neither function exists: the refused rows are settled before any call is made, and
    // the row whose call IS made is answered by Lambda itself for a function it does not
    // hold.
    let denied_function = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:answered-denied");
    let granted_function = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:answered-granted");
    let child_machine = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:answered-child");
    // The role may invoke exactly one function, so the same definition is refused for one
    // and served for the other, and nothing else it could call is granted at all.
    let role_arn = role(
        &router,
        "AnsweredRefusalRole",
        &json!({
            "Version": "2012-10-17",
            "Statement": [logs_only(), allow(INVOKE_ACTION, &granted_function)],
        }),
    )
    .await;

    let rows = [
        Row {
            case: "an invocation, the function named by its ARN",
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": denied_function.clone(), "Payload": {} }),
            error: LAMBDA_BASE_EXCEPTION,
            client: LAMBDA_CLIENT,
            code: REFUSAL_CODE,
            status: FORBIDDEN,
            refused: Some((INVOKE_ACTION, denied_function.clone())),
        },
        Row {
            // A bare name names the same function, so it is the same access request and
            // the same answer.
            case: "the same invocation, the function named by its bare name",
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": "answered-denied" }),
            error: LAMBDA_BASE_EXCEPTION,
            client: LAMBDA_CLIENT,
            code: REFUSAL_CODE,
            status: FORBIDDEN,
            refused: Some((INVOKE_ACTION, denied_function.clone())),
        },
        Row {
            // And so does the partial form Lambda documents for a `FunctionName`.
            case: "the same invocation, the function named by its partial ARN",
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": format!("{ACCOUNT}:function:answered-denied") }),
            error: LAMBDA_BASE_EXCEPTION,
            client: LAMBDA_CLIENT,
            code: REFUSAL_CODE,
            status: FORBIDDEN,
            refused: Some((INVOKE_ACTION, denied_function.clone())),
        },
        Row {
            // The direct form — the `Resource` IS the function — is the same call on the
            // same resource, so it stands in for the same answer; the client THAT form is
            // made with is the current SDK, which names the service its own way.
            case: "the function's own ARN as the Resource",
            resource: denied_function.clone(),
            parameters: json!({}),
            error: LAMBDA_BASE_EXCEPTION,
            client: LAMBDA_SDK_CLIENT,
            code: REFUSAL_CODE,
            status: FORBIDDEN,
            refused: Some((INVOKE_ACTION, denied_function.clone())),
        },
        Row {
            // The AWS SDK family makes the same call on the same resource, so the answer
            // it stands in for carries the same status and the same code — rendered by the
            // current SDK, which is the client THAT family is served through.
            case: "the same invocation through the AWS SDK integration",
            resource: "arn:aws:states:::aws-sdk:lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": denied_function.clone(), "Payload": {} }),
            error: LAMBDA_BASE_EXCEPTION,
            client: LAMBDA_SDK_CLIENT,
            code: REFUSAL_CODE,
            status: FORBIDDEN,
            refused: Some((INVOKE_ACTION, denied_function.clone())),
        },
        Row {
            // Another service, whose front door answers an unauthorized call with a code
            // of its own and spells the action it refused in lower case.
            case: "a send to a queue",
            resource: "arn:aws:states:::sqs:sendMessage".to_string(),
            parameters: json!({ "QueueUrl": queue_url.clone(), "MessageBody": "hello" }),
            error: "SQS.AmazonSQSException",
            client: "AmazonSQS",
            code: SQS_REFUSAL_CODE,
            status: FORBIDDEN,
            refused: Some(("sqs:sendmessage", queue_arn.clone())),
        },
        Row {
            // And a service whose front door answers an authorization failure as a client
            // error, which is what shows the status is the INVOKED service's rather than
            // one value for every refusal.
            case: "a child execution of this service itself",
            resource: "arn:aws:states:::states:startExecution".to_string(),
            parameters: json!({ "StateMachineArn": child_machine.clone() }),
            error: "StepFunctions.AWSStepFunctionsException",
            client: "AWSStepFunctions",
            code: REFUSAL_CODE,
            status: CLIENT_ERROR,
            refused: Some(("states:StartExecution", child_machine.clone())),
        },
        Row {
            // A call the role MAY make, answered by Lambda itself: the status the cause
            // names is the one THAT answer carried, not the one an authorization refusal
            // carries.
            case: "an invocation the role may make, of a function Lambda does not hold",
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": granted_function.clone(), "Payload": {} }),
            error: "Lambda.ResourceNotFoundException",
            client: LAMBDA_CLIENT,
            code: NOT_FOUND_CODE,
            status: NOT_FOUND,
            refused: None,
        },
        Row {
            // The same answer to the LEGACY form of that call, which is made with the
            // current SDK: one answer, rendered by whichever client asked for it.
            case: "the same answer to the function's own ARN as the Resource",
            resource: granted_function.clone(),
            parameters: json!({}),
            error: "Lambda.ResourceNotFoundException",
            client: LAMBDA_SDK_CLIENT,
            code: NOT_FOUND_CODE,
            status: NOT_FOUND,
            refused: None,
        },
    ];

    // Every answer names the call it was given for, so no two rows may report one id.
    let mut named_requests: Vec<String> = Vec::new();

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let machine_arn = machine(
            &router,
            &format!("answered-{index}"),
            &row.resource,
            &row.parameters,
            &role_arn,
        )
        .await;
        let (execution_arn, described) = ran(&router, &machine_arn, case).await;
        assert_eq!(
            (
                described.get("status").and_then(Value::as_str),
                described.get("error").and_then(Value::as_str),
            ),
            (Some("FAILED"), Some(row.error)),
            "case {case:?}: expected FAILED with {:?}, got {described:?}",
            row.error
        );
        let cause = string(&described, "cause");
        // The metadata the serving client closes the message with, holding the id of the
        // answered request to its PLACE rather than to one run's value.
        let expected = refusal_metadata(&row.resource, row.client, row.code, row.status);
        let reported = masked(&cause);
        assert!(
            reported.ends_with(&expected),
            "case {case:?}: expected the cause to close {expected:?}, got {reported:?}"
        );
        // The one status it names is the invoked service's own — so a refusal of a Lambda
        // call reports that service's Forbidden and never another service's or this
        // service's own client-error status.
        assert_eq!(
            answered_status(case, &cause),
            row.status,
            "case {case:?}: expected the status the invoked service answers with, got \
             {cause:?}"
        );
        // That id is the answer's own, so each row reports one of its own.
        let request_id = named_request(&cause)
            .unwrap_or_else(|| panic!("case {case:?}: expected a named request in {cause:?}"));
        assert!(
            !request_id.is_empty(),
            "case {case:?}: expected the answered request to be named, got {cause:?}"
        );
        assert!(
            !named_requests.contains(&request_id),
            "case {case:?}: expected this call's own request id, got {request_id:?} again \
             after {named_requests:?}"
        );
        named_requests.push(request_id);
        // A row the documents in force refused carries the invoked service's own sentence
        // before that metadata: the session of the execution role the call would have been
        // made as, the action the call is subject to and the resource it was over.
        if let Some((action, resource)) = &row.refused {
            let principal = session_principal(&role_arn, &cause);
            let sentence = format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {resource} because no identity-based policy allows the {action} action"
            );
            assert_eq!(
                reported,
                format!("{sentence}{expected}"),
                "case {case:?}: expected the sentence and the answer's own metadata, got \
                 {reported:?}"
            );
        }
        // Every event of the history that reports a cause reports THIS one, whichever
        // family the failed work was recorded in: an integration call writes a task
        // failure, a `Resource` that is a function's own ARN the Lambda family's event,
        // and the terminal execution failure carries it beside them.
        let causes = history_causes(&router, &execution_arn).await;
        assert!(
            !causes.is_empty() && causes.iter().all(|carried| carried == &cause),
            "case {case:?}: expected every cause the history reports to be the execution's \
             own {cause:?}, got {causes:?}"
        );
    }
}
