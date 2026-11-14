// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH of a Lambda `Task` state's two failure sources a `Retry`
//! and a `Catch` act on, driven over each service's own wire protocol through the
//! production `MultiServiceRouter` with Step Functions, IAM and Lambda mounted on ONE
//! engine — the shape `core/router/src/lib.rs` mounts them in — so the documents the call
//! is judged against are the ones IAM's handlers wrote and an allowed call really runs the
//! function.
//!
//! RULE: a `Task` state's failure carries the error NAME of whatever stopped the work —
//! the `errorType` of the error document the run produced where the call was made and the
//! function errored, and the invoked service's own SDK exception where the policies in
//! force on the state machine's `roleArn` refuse the call, in which case nothing ran — and
//! the state's `Retry` retriers and `Catch` catchers are matched against THAT name. So a
//! retrier or a catcher naming the class of one source does not act on a failure of the
//! other, either documented wildcard acts on both, a catcher that takes the failure over
//! routes its `{Error, Cause}` error output to its own `Next` instead of the execution
//! failing under it, and a retrier that matches makes the state's attempts its
//! `MaxAttempts` plus the first one.
//!
//! The rows differ along what the rule generalises over: which source stopped the work (a
//! handler that raised, and a call the role may not make), what the catcher names (the
//! raised class, the refusal class, each documented wildcard, and a class neither source
//! reports), what a retrier names beside it, and the form the `Task` names its work in
//! (the function's own ARN and the optimized integration token) — plus the rows whose role
//! allows the call and whose handler returns, which are the positive control that the same
//! definitions succeed.
//!
//! The harness (`Chain::with_iam`, `given_function_of`, `role`, `ran_as`, `sfn`) is the one
//! `lambda_task_control_flow` builds for these services, and the history's event types are
//! read through `lambda_function_timeout`'s own reading of them; `tests/all.rs` is this
//! binary's crate root, so both are reached through `crate::<suite>`.

use serde_json::{json, Value};

use crate::lambda_function_timeout::event_types;
use crate::lambda_task_control_flow::Chain;
use crate::state_machine_lifecycle::string;

/// The handler every row invokes: what it ends with is decided by the `kind` the EVENT
/// carries, and the message it raises embeds the `order` the event carries, so the error
/// document is the function's own and its input's rather than anything fixed here.
const HANDLER_SOURCE: &str = r#"
class OrderRejected(Exception):
    pass


def handler(event, context):
    if event.get("kind") == "raise":
        raise OrderRejected("order %s cannot be processed" % event["order"])
    return {"order": event["order"], "handled": True}
"#;

/// The class that raise reports, which is the `errorType` of the document the run produces
/// and so the name the failed task carries when the call WAS made.
const RAISED_ERROR_TYPE: &str = "OrderRejected";

/// The IAM action an invocation is subject to: the one Lambda PUBLISHES for `Invoke`
/// rather than the operation's own name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// An action that has nothing to do with an invocation, which is what the denied role's
/// policy grants instead.
const UNRELATED_ACTION: &str = "logs:PutLogEvents";

/// The error a `Task` state reports when the role may not make the call: Lambda's base
/// SDK exception, which is what a refusal `Invoke` models no error of its own arrives
/// under.
const REFUSED_ERROR: &str = "Lambda.AWSLambdaException";

/// The fixed text of the sentence that refusal carries.
const REFUSED_TEXT: &str = "not authorized to perform: lambda:InvokeFunction";

/// The two wildcards the Amazon States Language documents for a retrier and a catcher:
/// "States.ALL — A wildcard that matches any known error name", and "States.TaskFailed
/// […] matches any known error name except States.Timeout".
const TASK_FAILED_WILDCARD: &str = "States.TaskFailed";
/// See [`TASK_FAILED_WILDCARD`].
const ALL_ERRORS_WILDCARD: &str = "States.ALL";

/// An error name neither source below reports, so a catcher naming it matches nothing.
const UNRELATED_ERROR: &str = "InventoryUnavailable";

/// The `Resource` of the OPTIMIZED Lambda integration, which names the service and the
/// action rather than the function; the function is named by a `FunctionName` parameter.
const OPTIMIZED_RESOURCE: &str = "arn:aws:states:::lambda:invoke";

/// The role whose inline policy GRANTS the invocation, and the one whose policy says
/// nothing about it.
const GRANTED_ROLE: &str = "TaskRoleWithInvoke";
/// See [`GRANTED_ROLE`].
const DENIED_ROLE: &str = "TaskRoleWithoutInvoke";

/// How many further attempts a row's retrier declares, so a state whose failure that
/// retrier matches is attempted this many times over.
const RETRY_ATTEMPTS: u64 = 2;

/// The states a row's definition carries: the one that does the work, the one its catcher
/// routes to, and the one its success path leads to.
const WORK_STATE: &str = "Work";
/// See [`WORK_STATE`].
const HANDLED_STATE: &str = "Handled";
/// See [`WORK_STATE`].
const ANSWERED_STATE: &str = "Answered";

/// Where the catcher places the error output, which is what the handling state reports.
const FAILURE_MEMBER: &str = "failure";

/// Which form a row's `Task` names its work in.
#[derive(Debug, Clone, Copy)]
enum Resource {
    /// The optimized integration token, with the function named by a parameter.
    Optimized,
    /// The function's own ARN, which names the function itself.
    FunctionArn,
}

impl Resource {
    /// The history event ONE attempt of a state in this form writes when the work is
    /// handed over, which is what counts the attempts the state made: `HistoryEventType`
    /// names a `Task` state's lifecycle twice over, and a `Resource` that is the ARN of a
    /// Lambda function makes the unit of work one invocation of that function
    /// (`LambdaFunction*`), while a `Resource` naming a service integration has one API
    /// call as its unit of work (`Task*`).
    fn scheduled(self) -> &'static str {
        match self {
            Self::Optimized => "TaskScheduled",
            Self::FunctionArn => "LambdaFunctionScheduled",
        }
    }

    /// The function's own answer out of what the state reported as its result: the
    /// optimized integration reports the whole `Invoke` response, whose `Payload` member
    /// is that answer, and a `Resource` that is the function's ARN reports the answer
    /// itself.
    fn payload(self, result: &Value) -> Value {
        match self {
            Self::Optimized => result.get("Payload").cloned().unwrap_or(Value::Null),
            Self::FunctionArn => result.clone(),
        }
    }
}

/// Which role a row's state machine runs as.
#[derive(Debug, Clone, Copy)]
enum Role {
    /// The role whose policy grants the invocation on the function's own ARN.
    Granted,
    /// The role whose policy grants something else entirely.
    Denied,
}

/// What stopped a row's work, and so what name its failure carries.
#[derive(Debug, Clone, Copy)]
enum Source {
    /// The call was made and the handler raised.
    Raised,
    /// The role's policies refused the call, so nothing ran.
    Refused,
}

impl Source {
    /// The error name a failure of this source carries, which is what a retrier and a
    /// catcher are matched against.
    fn name(self) -> &'static str {
        match self {
            Self::Raised => RAISED_ERROR_TYPE,
            Self::Refused => REFUSED_ERROR,
        }
    }
}

/// What a row's execution must end as.
#[derive(Debug, Clone, Copy)]
enum Expected {
    /// The catcher took the failure over, so the execution reaches the handling state and
    /// SUCCEEDS, reporting the error output of a failure of this source.
    Caught(Source),
    /// Nothing took it over, so the execution FAILED under that same name.
    Failed(Source),
    /// The work answered, so the execution SUCCEEDS with what the function returned.
    Answered,
}

/// One row: the form the work is named in, the role it is attempted as, what the handler
/// is asked to end with, what the state's catcher and retrier name, how many attempts the
/// state must make, and the outcome.
struct Row {
    case: &'static str,
    resource: Resource,
    role: Role,
    /// The `kind` the input carries, which selects what the handler ends with.
    kind: &'static str,
    /// The `order` the input carries, which the raised message is built from.
    order: u64,
    /// The one `ErrorEquals` name the state's catcher declares.
    catch_on: &'static str,
    /// The one `ErrorEquals` name the state's retrier declares, for a row that has one.
    retry_on: Option<&'static str>,
    /// How many times the state must have handed its work over.
    attempts: usize,
    expected: Expected,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a raise caught by a catcher naming the class the handler raised",
            resource: Resource::FunctionArn,
            role: Role::Granted,
            kind: "raise",
            order: 11,
            catch_on: RAISED_ERROR_TYPE,
            retry_on: None,
            attempts: 1,
            expected: Expected::Caught(Source::Raised),
        },
        Row {
            case: "a raise a catcher naming the refusal class does not take over",
            resource: Resource::FunctionArn,
            role: Role::Granted,
            kind: "raise",
            order: 12,
            catch_on: REFUSED_ERROR,
            retry_on: None,
            attempts: 1,
            expected: Expected::Failed(Source::Raised),
        },
        Row {
            case: "a refused call caught by the task-failure wildcard",
            resource: Resource::FunctionArn,
            role: Role::Denied,
            kind: "raise",
            order: 13,
            catch_on: TASK_FAILED_WILDCARD,
            retry_on: None,
            attempts: 1,
            expected: Expected::Caught(Source::Refused),
        },
        Row {
            case: "a refused call a catcher naming the handler's class does not take over",
            resource: Resource::FunctionArn,
            role: Role::Denied,
            kind: "raise",
            order: 14,
            catch_on: RAISED_ERROR_TYPE,
            retry_on: None,
            attempts: 1,
            expected: Expected::Failed(Source::Refused),
        },
        Row {
            case: "a refused call on the optimized integration, whose handler would have \
                   returned",
            resource: Resource::Optimized,
            role: Role::Denied,
            kind: "return",
            order: 15,
            catch_on: ALL_ERRORS_WILDCARD,
            retry_on: None,
            attempts: 1,
            expected: Expected::Caught(Source::Refused),
        },
        Row {
            case: "a raise on the optimized integration, caught by the task-failure wildcard",
            resource: Resource::Optimized,
            role: Role::Granted,
            kind: "raise",
            order: 16,
            catch_on: TASK_FAILED_WILDCARD,
            retry_on: None,
            attempts: 1,
            expected: Expected::Caught(Source::Raised),
        },
        Row {
            case: "a raise under a catcher naming a class neither source reports",
            resource: Resource::Optimized,
            role: Role::Granted,
            kind: "raise",
            order: 17,
            catch_on: UNRELATED_ERROR,
            retry_on: None,
            attempts: 1,
            expected: Expected::Failed(Source::Raised),
        },
        Row {
            case: "a retrier naming the raised class does not retry a refused call",
            resource: Resource::FunctionArn,
            role: Role::Denied,
            kind: "raise",
            order: 18,
            catch_on: ALL_ERRORS_WILDCARD,
            retry_on: Some(RAISED_ERROR_TYPE),
            attempts: 1,
            expected: Expected::Caught(Source::Refused),
        },
        Row {
            case: "a retrier naming the refusal class spends its attempts on a refused call",
            resource: Resource::FunctionArn,
            role: Role::Denied,
            kind: "raise",
            order: 19,
            catch_on: ALL_ERRORS_WILDCARD,
            retry_on: Some(REFUSED_ERROR),
            attempts: RETRY_ATTEMPTS as usize + 1,
            expected: Expected::Caught(Source::Refused),
        },
        Row {
            case: "a retrier naming the raised class spends its attempts on a raise",
            resource: Resource::FunctionArn,
            role: Role::Granted,
            kind: "raise",
            order: 20,
            catch_on: ALL_ERRORS_WILDCARD,
            retry_on: Some(RAISED_ERROR_TYPE),
            attempts: RETRY_ATTEMPTS as usize + 1,
            expected: Expected::Caught(Source::Raised),
        },
        Row {
            case: "a handler that returned under a role granted the call",
            resource: Resource::FunctionArn,
            role: Role::Granted,
            kind: "return",
            order: 21,
            catch_on: ALL_ERRORS_WILDCARD,
            retry_on: None,
            attempts: 1,
            expected: Expected::Answered,
        },
        Row {
            case: "the same on the optimized integration",
            resource: Resource::Optimized,
            role: Role::Granted,
            kind: "return",
            order: 22,
            catch_on: TASK_FAILED_WILDCARD,
            retry_on: None,
            attempts: 1,
            expected: Expected::Answered,
        },
    ]
}

/// A statement allowing `action` on `resource`.
fn allow(action: &str, resource: &str) -> Value {
    json!({ "Effect": "Allow", "Action": action, "Resource": resource })
}

/// The message the handler raises for `order`, built the way the handler builds it.
fn raised_message(order: u64) -> String {
    format!("order {order} cannot be processed")
}

/// The definition a row runs: ONE `Task` state naming its work in `resource`'s form, with
/// one catcher naming `catch_on` routing to a state that reports the error output, one
/// retrier naming `retry_on` where the row declares one, and a `Succeed` on its success
/// path.
fn definition(
    resource: Resource,
    function_arn: &str,
    catch_on: &str,
    retry_on: Option<&str>,
) -> Value {
    let mut work = match resource {
        Resource::Optimized => json!({
            "Type": "Task",
            "Resource": OPTIMIZED_RESOURCE,
            "Parameters": { "FunctionName": function_arn, "Payload.$": "$" },
        }),
        Resource::FunctionArn => json!({ "Type": "Task", "Resource": function_arn }),
    };
    let members = work.as_object_mut().expect("a state is an object");
    members.insert(
        "Catch".to_string(),
        json!([{
            "ErrorEquals": [catch_on],
            "ResultPath": format!("$.{FAILURE_MEMBER}"),
            "Next": HANDLED_STATE,
        }]),
    );
    members.insert("Next".to_string(), json!(ANSWERED_STATE));
    if let Some(error) = retry_on {
        members.insert(
            "Retry".to_string(),
            json!([{
                "ErrorEquals": [error],
                "IntervalSeconds": 1,
                "MaxAttempts": RETRY_ATTEMPTS,
                "BackoffRate": 1.0,
            }]),
        );
    }
    json!({
        "StartAt": WORK_STATE,
        "States": {
            WORK_STATE: work,
            HANDLED_STATE: {
                "Type": "Pass",
                "OutputPath": format!("$.{FAILURE_MEMBER}"),
                "End": true,
            },
            ANSWERED_STATE: { "Type": "Succeed" },
        },
    })
}

/// The `output` an execution reported, as the document it carries.
fn output(described: &Value, case: &str) -> Value {
    let text = described
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected an output on {described}"));
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("case {case:?}: expected the output to be JSON, got {error} for {text:?}")
    })
}

/// The text of `member` on `document`.
fn text(document: &Value, member: &str, case: &str) -> String {
    document
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} on {document}"))
        .to_string()
}

/// How many of `events` are of `kind`.
fn count(events: &[Value], kind: &str) -> usize {
    event_types(events)
        .iter()
        .filter(|written| *written == kind)
        .count()
}

/// What the `cause` of a failure of `source` must carry: the run's own error document for
/// a handler that raised — the class it raised and the message it built from this
/// execution's input — and the refusal the invoked service answers an unauthorized call
/// with, naming the SESSION of the role `role_arn` it was attempted as and the function it
/// was over, for a call the policies refused.
fn assert_cause(
    source: Source,
    cause: &str,
    order: u64,
    function: &str,
    role_arn: &str,
    case: &str,
) {
    match source {
        Source::Raised => {
            let document: Value = serde_json::from_str(cause).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: expected the cause to be the run's error document, got \
                     {error} for {cause:?}"
                )
            });
            assert_eq!(
                (
                    document.get("errorType").and_then(Value::as_str),
                    document.get("errorMessage").and_then(Value::as_str),
                ),
                (
                    Some(RAISED_ERROR_TYPE),
                    Some(raised_message(order).as_str())
                ),
                "case {case:?}: expected the run's own error document, got {document}"
            );
        }
        Source::Refused => {
            // The principal a refusal blames is a SESSION of that role, under the name
            // the assume gave it.
            let principal =
                crate::task_refusal_session_principal::session_principal(role_arn, cause);
            for fragment in [REFUSED_TEXT, function, principal.as_str()] {
                assert!(
                    cause.contains(fragment),
                    "case {case:?}: expected the refusal to carry {fragment:?}, got {cause:?}"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_failures_error_name_decides_which_retrier_and_catcher_act_on_it() {
    let chain = Chain::with_iam();
    let function = chain
        .given_function_of("order-handler", HANDLER_SOURCE, None)
        .await;
    let granted = chain
        .role(
            "the role granted the invocation",
            GRANTED_ROLE,
            &json!([allow(INVOKE_ACTION, &function)]),
        )
        .await;
    let denied = chain
        .role(
            "the role granted something else",
            DENIED_ROLE,
            &json!([allow(UNRELATED_ACTION, "*")]),
        )
        .await;

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            resource,
            role,
            kind,
            order,
            catch_on,
            retry_on,
            attempts,
            expected,
        } = row;
        let role_arn = match role {
            Role::Granted => granted.as_str(),
            Role::Denied => denied.as_str(),
        };
        let machine_name = format!("machine-{index}");
        let described = chain
            .ran_as(
                case,
                &machine_name,
                &definition(resource, &function, catch_on, retry_on),
                &json!({ "kind": kind, "order": order }),
                role_arn,
            )
            .await;
        let answered = chain
            .sfn(
                case,
                "GetExecutionHistory",
                &json!({
                    "executionArn": string(&described, "executionArn"),
                    "maxResults": 200,
                }),
            )
            .await;
        let events = answered
            .get("events")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        match expected {
            Expected::Caught(source) => {
                // The catcher took the failure over, so the execution reached the
                // handling state and succeeded there, carrying the error output of the
                // source that stopped the work.
                assert_eq!(
                    described.get("status").and_then(Value::as_str),
                    Some("SUCCEEDED"),
                    "case {case:?}: expected the catcher to recover the execution, got \
                     {described}"
                );
                let caught = output(&described, case);
                assert_eq!(
                    caught.get("Error").and_then(Value::as_str),
                    Some(source.name()),
                    "case {case:?}: expected the error output to name {:?}, got {caught}",
                    source.name()
                );
                assert_cause(
                    source,
                    &text(&caught, "Cause", case),
                    order,
                    &function,
                    role_arn,
                    case,
                );
            }
            Expected::Failed(source) => {
                // Nothing names this failure, so the execution fails under it.
                assert_eq!(
                    (
                        described.get("status").and_then(Value::as_str),
                        described.get("error").and_then(Value::as_str),
                    ),
                    (Some("FAILED"), Some(source.name())),
                    "case {case:?}: expected the execution to fail under {:?}, got {described}",
                    source.name()
                );
                assert_cause(
                    source,
                    &string(&described, "cause"),
                    order,
                    &function,
                    role_arn,
                    case,
                );
                assert_eq!(
                    count(&events, "ExecutionSucceeded"),
                    0,
                    "case {case:?}: expected no success for the execution, got {:?}",
                    event_types(&events)
                );
            }
            Expected::Answered => {
                assert_eq!(
                    described.get("status").and_then(Value::as_str),
                    Some("SUCCEEDED"),
                    "case {case:?}: expected the execution to succeed, got {described}"
                );
                assert_eq!(
                    resource.payload(&output(&described, case)),
                    json!({ "order": order, "handled": true }),
                    "case {case:?}: expected the function's own answer, got {described}"
                );
            }
        }

        // However the failure was routed, the state handed its work over once per attempt
        // — one attempt, or the retrier's own `MaxAttempts` beside the first, for a
        // retrier that matched the failure's name.
        assert_eq!(
            count(&events, resource.scheduled()),
            attempts,
            "case {case:?}: expected {attempts} attempt(s) of {}, got {:?}",
            resource.scheduled(),
            event_types(&events)
        );
    }
}
