// SPDX-License-Identifier: CC0-1.0

//! What a client CLASSIFYING a failed execution reads out of its history when the one
//! `Task` state's integration call is one the state machine's execution role may not make
//! — driven over both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions and IAM mounted on ONE engine, the shape
//! `core/router/src/lib.rs` mounts them in, so the documents the call is judged against
//! are the ones IAM's own handlers wrote.
//!
//! RULE: a `Task` state's call the documents in force on its execution role do not allow
//! is reported as the INVOKED service's own answer, rendered by the client that
//! integration is served through — so a refusal served by the earlier generation of
//! client names, beside the IAM sentence, the code that answer gave the class by, and
//! every reading of the failed execution reports that one message: the attempt's own
//! history event, the terminal event that closes the history, and the terminal
//! `DescribeExecution`. The denial is therefore identifiable as an AUTHORIZATION failure
//! from the history alone, where the error name is only the invoked service's base
//! exception.
//!
//! The rows differ along what the rule generalises over: WHAT the role's documents are —
//! no identity-based document at all (the least-privilege role an orchestration carries
//! before anything is granted to it), an inline policy stating nothing about the call, an
//! explicit `Deny` over a grant of it, and a grant of it scoped to another resource — the
//! FORM the `Resource` names the work in (the optimized integration token and the
//! function's own ARN, which is reported in the Lambda event family rather than the task
//! one), and the SERVICE the call was to, whose own front door decides the code. The row
//! named through the AWS SDK integration family is the CONTROL: that family is served by
//! the current AWS SDK, whose rendering counts the call's attempts and names no error
//! code, which is what keeps the rule from being "every cause names a code".
//!
//! What the neighbours pin and this one does not repeat:
//! `tests/all/task_refusal_access_denied_status.rs` holds the STATUS and the client name
//! each service's answer carries (and this suite reuses its `details_member`),
//! `tests/all/lambda_task_refusal_rendering.rs` the two renderings themselves, and
//! `tests/all/denied_invoke_history_verdict.rs` the SETTLING of such an execution (and
//! this suite reuses its `only`). What only this suite reaches is the DOCUMENT SETS that
//! withhold the call — an empty one above all, which is the role an orchestration carries
//! before anything is granted to it — held to the denial's own code being in the history
//! that reports them.
//!
//! Every wait here waits for the CONDITION (`Chain::ran` looks again until the execution
//! has left `RUNNING`), so nothing here depends on how fast the host runs. The harness,
//! the trust policy and the two renderings are `tests/all/task_execution_role.rs`'s and
//! the principal reader is `tests/all/task_refusal_session_principal.rs`'s;
//! `tests/all/main.rs` is this binary's crate root, so both are reached through
//! `crate::<suite>`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{
    EVENT_EXECUTION_FAILED, EVENT_LAMBDA_FUNCTION_FAILED, EVENT_TASK_FAILED,
};

use crate::denied_invoke_history_verdict::only;
use crate::execution_data_encryption::xml_element;
use crate::state_machine_lifecycle::{string, ACCOUNT, REGION};
use crate::task_execution_role::{
    masked, refusal_metadata, trust_document, Chain, ACCESS_DENIED_CODE,
};
use crate::task_refusal_access_denied_status::details_member;
use crate::task_refusal_session_principal::{definition, session_principal};

/// The status a service whose front door answers an unauthorized call as a Forbidden
/// carries, which both services below do.
const FORBIDDEN: u16 = 403;

/// The IAM action an invocation is subject to: the one AWS Lambda PUBLISHES for `Invoke`,
/// which is not the operation's own name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The code Amazon SQS's own front door names an authorization refusal by, which is not
/// the AWS-common one AWS Lambda's answers carry.
const SQS_REFUSAL_CODE: &str = "AccessDenied";

/// The action Amazon SQS's authorization layer names for a send, in the case it reports.
const SQS_SEND_ACTION: &str = "sqs:sendmessage";

/// The field the earlier generation of client names the answered code in, and the group
/// the current AWS SDK closes its own rendering with — the two the classification below
/// reads, so neither is asserted by a spelling of its own.
const CODE_FIELD: &str = "Error Code: ";
/// See [`CODE_FIELD`].
const ATTEMPTS_FIELD: &str = "SDK Attempt Count: ";

/// How a row's role withholds the call, which decides the clause IAM's sentence closes
/// with: a reading that states nothing about the call leaves the default deny to decide
/// it, and an explicit `Deny` refuses it outright.
#[derive(Clone, Copy, Debug)]
enum Withheld {
    /// Nothing in force states anything about the call.
    Unstated,
    /// A grant of the very call, overturned by an explicit `Deny`.
    ExplicitDeny,
}

impl Withheld {
    /// The clause IAM's own sentence closes with for this reading of the documents.
    fn because(self, action: &str) -> String {
        match self {
            Self::Unstated => {
                format!("because no identity-based policy allows the {action} action")
            }
            Self::ExplicitDeny => "with an explicit deny in an identity-based policy".to_string(),
        }
    }
}

/// One row: the case, the documents the role carries (`None` for a role carrying no
/// identity-based document at all), how they withhold the call, the `Resource` the `Task`
/// names and the request it makes, the action and resource the refusal names, and the
/// answer it stands in for — the error name the state reports, the invoked service's own
/// client name, the code it names the class by, the status it carries, the history event
/// family the failed work is reported in, and whether the client serving this row's
/// integration RENDERS that code at all.
struct Row {
    case: &'static str,
    documents: Option<Value>,
    withheld: Withheld,
    resource: String,
    parameters: Value,
    action: &'static str,
    target: String,
    error: &'static str,
    sdk_client: &'static str,
    code: &'static str,
    status: u16,
    failure_event: &'static str,
    /// Whether the client this row's integration is served through names the answered
    /// code: the earlier generation of client does, and the current AWS SDK counts the
    /// call's attempts instead. Stated per row rather than derived from the helper that
    /// builds the expectation, so the rendering family is a property this suite asserts.
    renders_code: bool,
}

/// A role whose identity-based documents are `documents`: an inline policy where a row
/// states one, and NO document at all where it states none.
///
/// `Chain::role` always writes an inline policy, so the empty document set — a role
/// created and nothing granted to it yet — is made here out of IAM's own `CreateRole`
/// alone.
async fn role_with(chain: &Chain, name: &str, documents: Option<&Value>) -> String {
    if let Some(policy) = documents {
        return chain.role(name, policy).await;
    }
    let created = chain
        .iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust_document().to_string()),
            ],
        )
        .await;
    xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"))
}

/// A policy granting `action` on `resource` and nothing else.
fn allowing(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
    })
}

/// A policy granting `action` on `resource` and denying the very same call.
fn denying(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            { "Effect": "Allow", "Action": action, "Resource": resource },
            { "Effect": "Deny", "Action": action, "Resource": resource },
        ],
    })
}

/// The rows: every document set that withholds the call, over both forms of the Lambda
/// invocation — each served by its own client, so each carrying its own rendering — a
/// second service's optimized integration, and the AWS SDK family as the control.
fn rows(function_arn: &str) -> Vec<Row> {
    let other_function = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:not-the-one");
    let queue_url = format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT}/intake");
    let queue_arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:intake");
    vec![
        Row {
            // The least-privilege role an orchestration carries before anything is granted
            // to it: no identity-based document in force at all.
            case: "a role carrying no identity document, through the optimized integration",
            documents: None,
            withheld: Withheld::Unstated,
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": function_arn, "Payload": {} }),
            action: INVOKE_ACTION,
            target: function_arn.to_string(),
            error: "Lambda.AWSLambdaException",
            sdk_client: "AWSLambda",
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_TASK_FAILED,
            renders_code: true,
        },
        Row {
            // The same empty document set over the OTHER form of the same call, whose work
            // is reported in the Lambda event family.
            case: "a role carrying no identity document, through the function's own ARN",
            documents: None,
            withheld: Withheld::Unstated,
            resource: function_arn.to_string(),
            parameters: json!({}),
            action: INVOKE_ACTION,
            target: function_arn.to_string(),
            error: "Lambda.AWSLambdaException",
            // The LEGACY flavour is made with the current AWS SDK, which names the
            // service as its own client does and counts the attempts instead of
            // naming the answered code.
            sdk_client: "Lambda",
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_LAMBDA_FUNCTION_FAILED,
            renders_code: false,
        },
        Row {
            case: "a role whose only grant is of another service's action",
            documents: Some(allowing("logs:PutLogEvents", "*")),
            withheld: Withheld::Unstated,
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": function_arn, "Payload": {} }),
            action: INVOKE_ACTION,
            target: function_arn.to_string(),
            error: "Lambda.AWSLambdaException",
            sdk_client: "AWSLambda",
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_TASK_FAILED,
            renders_code: true,
        },
        Row {
            case: "a role whose grant of the invocation an explicit deny overturns",
            documents: Some(denying(INVOKE_ACTION, function_arn)),
            withheld: Withheld::ExplicitDeny,
            resource: "arn:aws:states:::lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": function_arn, "Payload": {} }),
            action: INVOKE_ACTION,
            target: function_arn.to_string(),
            error: "Lambda.AWSLambdaException",
            sdk_client: "AWSLambda",
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_TASK_FAILED,
            renders_code: true,
        },
        Row {
            case: "a role whose invocation grant names another function",
            documents: Some(allowing(INVOKE_ACTION, &other_function)),
            withheld: Withheld::Unstated,
            resource: function_arn.to_string(),
            parameters: json!({}),
            action: INVOKE_ACTION,
            target: function_arn.to_string(),
            error: "Lambda.AWSLambdaException",
            sdk_client: "Lambda",
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_LAMBDA_FUNCTION_FAILED,
            renders_code: false,
        },
        Row {
            // Another service's optimized integration: the code the history carries is the
            // one THAT service's front door answers, not AWS Lambda's.
            case: "a send to a queue, under a role carrying no identity document",
            documents: None,
            withheld: Withheld::Unstated,
            resource: "arn:aws:states:::sqs:sendMessage".to_string(),
            parameters: json!({ "QueueUrl": queue_url, "MessageBody": "hello" }),
            action: SQS_SEND_ACTION,
            target: queue_arn,
            error: "SQS.AmazonSQSException",
            sdk_client: "AmazonSQS",
            code: SQS_REFUSAL_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_TASK_FAILED,
            renders_code: true,
        },
        Row {
            // The control: the same invocation named through the AWS SDK family is made
            // with the current AWS SDK, whose rendering counts the attempts and names no
            // code — so the history of this one carries no code to classify it by.
            case: "the same invocation through the AWS SDK integration family",
            documents: None,
            withheld: Withheld::Unstated,
            resource: "arn:aws:states:::aws-sdk:lambda:invoke".to_string(),
            parameters: json!({ "FunctionName": function_arn, "Payload": {} }),
            action: INVOKE_ACTION,
            target: function_arn.to_string(),
            error: "Lambda.AWSLambdaException",
            sdk_client: "Lambda",
            // The code this answer named the class by is the same one; what differs is
            // that the client serving this family writes no field for it.
            code: ACCESS_DENIED_CODE,
            status: FORBIDDEN,
            failure_event: EVENT_TASK_FAILED,
            renders_code: false,
        },
    ]
}

/// RULE: a denied `Task` state's refusal reaches the execution's history as the invoked
/// service's own answer, so the code that answer named the class by is in the history
/// wherever the serving client renders one — and the attempt's event, the terminal event
/// and the terminal read all report that one message.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_task_reports_the_denials_code_in_the_history() {
    let chain = Chain::new();
    // No target has to exist: authorization is settled before the call is made, so these
    // only name what a request addresses.
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:handler");

    for (index, row) in rows(&function_arn).into_iter().enumerate() {
        let case = row.case;
        let role_arn = role_with(
            &chain,
            &format!("HistoryDenialRole{index}"),
            row.documents.as_ref(),
        )
        .await;
        let machine = chain
            .machine(
                &format!("history-denial-{index}"),
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
            (Some("FAILED"), Some(row.error)),
            "case {case:?}: expected FAILED with {:?}, got {described:?}",
            row.error
        );

        // The whole message, up to the id of the refused request — one run's own value, so
        // it is masked and everything around it is held: the sentence IAM words, naming the
        // session of the execution role, the action and the resource, closed by the clause
        // the row's own reading of the documents gives, and then the metadata the serving
        // client renders.
        let cause = string(&described, "cause");
        let principal = session_principal(&role_arn, &cause);
        let metadata = refusal_metadata(&row.resource, row.sdk_client, row.code, row.status);
        let expected = format!(
            "User: {principal} is not authorized to perform: {} on resource: {} {}{metadata}",
            row.action,
            row.target,
            row.withheld.because(row.action),
        );
        assert_eq!(
            masked(&cause),
            expected,
            "case {case:?}: expected the cause {expected:?}, got {:?}",
            masked(&cause)
        );

        // Every reading of the failed execution reports that one message: the attempt's own
        // event, under the family the `Resource` named the work in, and the terminal event
        // that closes the history.
        let events = chain.history(&execution_arn).await;
        let member = details_member(row.failure_event);
        let attempt = only(case, &events, row.failure_event);
        let attempt_details = attempt
            .get(member)
            .unwrap_or_else(|| panic!("case {case:?}: expected {member} in {attempt}"));
        assert_eq!(
            (
                string(attempt_details, "error"),
                string(attempt_details, "cause")
            ),
            (row.error.to_string(), cause.clone()),
            "case {case:?}: expected the attempt's own event to report the refusal, got \
             {attempt}"
        );
        let terminal = only(case, &events, EVENT_EXECUTION_FAILED);
        let terminal_details = terminal.get("executionFailedEventDetails").unwrap_or_else(|| {
            panic!("case {case:?}: expected the terminal details in {terminal}")
        });
        assert_eq!(
            (
                string(terminal_details, "error"),
                string(terminal_details, "cause")
            ),
            (row.error.to_string(), cause.clone()),
            "case {case:?}: expected the closing event to report what the terminal read \
             reports, got {terminal}"
        );

        // And so the denial is classifiable from the history alone: a client that reads it
        // for the denial's own code finds it wherever the serving client renders one, and
        // finds that client's attempt count instead where it renders no code.
        let blob = serde_json::to_string(&events).expect("the history serializes");
        let named = format!("{CODE_FIELD}{}", row.code);
        let (present, absent) = match row.renders_code {
            true => (named.as_str(), ATTEMPTS_FIELD),
            false => (ATTEMPTS_FIELD, CODE_FIELD),
        };
        assert!(
            blob.contains(present),
            "case {case:?}: expected the history to carry {present:?}, got {blob}"
        );
        assert!(
            !blob.contains(absent),
            "case {case:?}: expected the history to carry no {absent:?}, got {blob}"
        );
    }
}
