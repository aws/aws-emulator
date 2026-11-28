// SPDX-License-Identifier: CC0-1.0

//! The metadata a denied LEGACY Lambda-ARN `Task` reports, driven over both services' own
//! wire protocols through the production `MultiServiceRouter` with Step Functions and IAM
//! mounted on ONE engine — the shape `core/router/src/lib.rs` mounts them in — so the
//! documents the call is judged against are the ones IAM's own handlers wrote.
//!
//! RULE: a failed `Task`'s cause renders the invoked service's exception as the client that
//! made the call writes it, and which client that is follows from the integration FLAVOUR
//! the `Resource` named. The LEGACY Lambda flavour — the function's own ARN as the
//! `Resource`, with the state's effective input as the event and no `FunctionName`/`Payload`
//! wrapping — is made with the CURRENT AWS SDK, whose message closes with ONE group of
//! COMMA-separated fields (the service as that SDK's own client names it, the status the
//! answer carried and the refused request) followed by a group of its own naming how many
//! attempts the call took; it names no error code and no proxy. The OPTIMIZED
//! `arn:aws:states:::lambda:invoke` integration is served by the earlier generation of
//! client, whose own rendering `tests/all/lambda_task_refusal_rendering.rs` holds beside
//! this one.
//!
//! The rows differ along what the rule generalises over: the form the function's ARN takes
//! in the `Resource` (unqualified, version-qualified, alias-qualified), what the documents
//! in force say (nothing about the action, a grant of another function, an explicit deny
//! over a grant) and so which clause IAM's sentence closes with, and whether the state
//! writes `Parameters` for the event or is handed the execution's own input.
//!
//! The harness (`Chain`), the masking of the refused request's id (`masked`) and the reading
//! of the blamed session (`session_principal`) are the other suites' — `tests/all/main.rs`
//! is one crate root, so none of them is copied here.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_LAMBDA_FUNCTION_FAILED;

use crate::state_machine_lifecycle::{string, ACCOUNT, REGION};
use crate::task_execution_role::{masked, refusal_metadata, Chain, ACCESS_DENIED_CODE, REQUEST_ID};
use crate::task_refusal_session_principal::session_principal;

/// The IAM action an invocation is subject to: the one AWS Lambda PUBLISHES for `Invoke`,
/// which is not the operation's own name.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The error a `Task` reports for a refusal `Invoke` models no error of its own for:
/// Lambda's base SDK exception, under the name Step Functions writes for that service.
const REFUSED_ERROR: &str = "Lambda.AWSLambdaException";

/// The status AWS Lambda's own front door answers an unauthorized call with.
const FORBIDDEN: u16 = 403;

/// The name the CURRENT AWS SDK's own client gives AWS Lambda, which is the name the client
/// THIS flavour is made with writes in the metadata of a refusal.
const SDK_CLIENT_SERVICE: &str = "Lambda";

/// The workflow type `StartSyncExecution` serves, as `StateMachineType` spells it.
const EXPRESS: &str = "EXPRESS";

/// The terminal status a refused run reaches.
const FAILED: &str = "FAILED";

/// The fields only the EARLIER generation of client writes, none of which belongs to this
/// flavour's cause: that client's own name for the service, the code the answer named the
/// class by, and the proxy the call went through.
const UNWRITTEN_FIELDS: &[&str] = &["AWSLambda", "Error Code:", "Proxy: null", ";"];

/// One row: the case, the function ARN the `Resource` IS, the document the execution role
/// carries (`None` for a role carrying none at all), the clause IAM's sentence closes with,
/// and the `Parameters` the state writes for the event (`None` where the execution's own
/// input is the event).
struct Row {
    case: &'static str,
    function: String,
    documents: Option<Value>,
    because: String,
    parameters: Option<Value>,
}

/// A policy allowing `action` on `resource` and nothing else.
fn allowing(action: &str, resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": resource }],
    })
}

/// The clause IAM's sentence closes with when nothing in force states anything about
/// `action`.
fn unstated(action: &str) -> String {
    format!("because no identity-based policy allows the {action} action")
}

/// The ARN of a function of this account in this Region, with `qualifier` after its name
/// where one is given: a version or an alias names a particular function to invoke, and both
/// are a `Resource` a `Task` state may carry.
fn function_arn(name: &str, qualifier: Option<&str>) -> String {
    let arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}");
    match qualifier {
        Some(qualifier) => format!("{arn}:{qualifier}"),
        None => arn,
    }
}

/// The rows. No function is created for any of them: the call is decided before it is made,
/// so what the `Resource` addresses need not exist and Lambda need not be mounted at all.
fn rows() -> Vec<Row> {
    let other_function = function_arn("someone-elses-handler", None);
    vec![
        Row {
            case: "an unqualified function ARN under a role carrying no identity document",
            function: function_arn("legacy-plain", None),
            documents: None,
            because: unstated(INVOKE_ACTION),
            parameters: None,
        },
        Row {
            case: "a version-qualified function ARN, the role granting another function",
            function: function_arn("legacy-versioned", Some("3")),
            documents: Some(allowing(INVOKE_ACTION, &other_function)),
            because: unstated(INVOKE_ACTION),
            parameters: Some(json!({ "event": "written by the state" })),
        },
        Row {
            case: "an alias-qualified function ARN, the grant overturned by an explicit deny",
            function: function_arn("legacy-aliased", Some("live")),
            documents: Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    { "Effect": "Allow", "Action": INVOKE_ACTION, "Resource": "*" },
                    { "Effect": "Deny", "Action": INVOKE_ACTION, "Resource": "*" },
                ],
            })),
            because: "with an explicit deny in an identity-based policy".to_string(),
            parameters: None,
        },
        Row {
            case: "a function ARN whose role speaks only of another service",
            function: function_arn("legacy-unrelated-grant", None),
            documents: Some(allowing("logs:PutLogEvents", "*")),
            because: unstated(INVOKE_ACTION),
            parameters: Some(json!({ "event": {} })),
        },
    ]
}

/// The definition of ONE `Task` state whose `Resource` IS `function`, carrying `parameters`
/// where the row writes them.
fn definition(function: &str, parameters: Option<&Value>) -> String {
    let mut state = json!({ "Type": "Task", "Resource": function, "End": true });
    if let Some(parameters) = parameters {
        state
            .as_object_mut()
            .expect("the state is an object")
            .insert("Parameters".to_string(), parameters.clone());
    }
    json!({ "StartAt": "Invoke", "States": { "Invoke": state } }).to_string()
}

/// An execution role carrying `documents` as its inline policy, and carrying NO
/// identity-based document where a row states none: `Chain::role` always writes one, so the
/// empty document set is made out of IAM's own `CreateRole` alone.
async fn role_with(chain: &Chain, name: &str, documents: Option<&Value>) -> String {
    if let Some(policy) = documents {
        return chain.role(name, policy).await;
    }
    let created = chain
        .iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                (
                    "AssumeRolePolicyDocument",
                    crate::task_execution_role::trust_document().to_string(),
                ),
            ],
        )
        .await;
    crate::execution_data_encryption::xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"))
}

/// Hold one answered refusal to the whole cause the rule states, and report the id of the
/// refused request it named.
fn assert_cause(
    case: &str,
    role_arn: &str,
    reported: &Value,
    function: &str,
    because: &str,
) -> String {
    assert_eq!(
        (
            reported.get("status").and_then(Value::as_str),
            reported.get("error").and_then(Value::as_str),
        ),
        (Some(FAILED), Some(REFUSED_ERROR)),
        "case {case:?}: expected {FAILED} with {REFUSED_ERROR}, got {reported:?}"
    );
    let cause = string(reported, "cause");
    let principal = session_principal(role_arn, &cause);
    // The metadata the client this flavour is made with closes the message with, rendered by
    // the one renderer every suite of this crate root holds a refusal's metadata to
    // ([`refusal_metadata`]): for a `Resource` that IS a function's ARN that is the current
    // AWS SDK's, which names the service as that SDK's own client does.
    let expected = format!(
        "User: {principal} is not authorized to perform: {INVOKE_ACTION} on resource: \
         {function} {because}{}",
        refusal_metadata(function, SDK_CLIENT_SERVICE, ACCESS_DENIED_CODE, FORBIDDEN)
    );
    assert_eq!(
        masked(&cause),
        expected,
        "case {case:?}: expected the cause {expected:?}, got {:?}",
        masked(&cause)
    );
    for unwritten in UNWRITTEN_FIELDS {
        assert!(
            !cause.contains(unwritten),
            "case {case:?}: expected the cause to write no {unwritten:?}, got {cause:?}"
        );
    }
    // The id the mask stood in for is this call's own, so the field carries one and the
    // mask itself is nowhere in the text.
    let named = cause
        .split_once("Request ID: ")
        .map(|(_, tail)| {
            tail.chars()
                .take_while(|character| *character != ')')
                .collect::<String>()
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected a named request in {cause:?}"));
    assert!(
        !named.is_empty() && named != REQUEST_ID,
        "case {case:?}: expected the refused request to be named, got {cause:?}"
    );
    named
}

/// RULE, through the operation that answers an EXPRESS run inside the call: the denied
/// legacy-ARN `Task` fails the run, and the cause the answer carries is the sentence closed
/// by the CURRENT AWS SDK's own metadata — with no `output` member beside it, because no
/// work produced one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_legacy_arn_task_closes_its_cause_with_the_current_sdks_metadata() {
    let chain = Chain::new();
    // Every answer names the call it was given for, so no two runs may report one id.
    let mut named_requests: Vec<String> = Vec::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let role_arn = role_with(
            &chain,
            &format!("LegacyArnSyncRole{index}"),
            row.documents.as_ref(),
        )
        .await;
        let created = chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("legacy-arn-sync-{index}"),
                    "definition": definition(&row.function, row.parameters.as_ref()),
                    "roleArn": role_arn,
                    "type": EXPRESS,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let answered = chain
            .sfn(
                "StartSyncExecution",
                &json!({
                    "stateMachineArn": machine,
                    "name": "only",
                    "input": json!({ "marker": index }).to_string(),
                }),
            )
            .await;
        let named = assert_cause(case, &role_arn, &answered, &row.function, &row.because);
        assert!(
            !named_requests.contains(&named),
            "case {case:?}: expected this call's own request id, got {named:?} again after \
             {named_requests:?}"
        );
        named_requests.push(named);
        // A refused call produced nothing, so the answer carries no result at all.
        assert!(
            answered.get("output").is_none(),
            "case {case:?}: expected no output on a failed run, got {answered:?}"
        );
    }
}

/// RULE, continued: the same text is what the asynchronous run reports — the terminal read
/// and the event the Lambda work family records the failure under both carry that one cause,
/// so a `Catch` path and a history reader see the client's own rendering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_rendering_reaches_the_read_and_the_history_of_a_standard_run() {
    let chain = Chain::new();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let role_arn = role_with(
            &chain,
            &format!("LegacyArnStandardRole{index}"),
            row.documents.as_ref(),
        )
        .await;
        let machine = chain
            .machine(
                &format!("legacy-arn-standard-{index}"),
                &definition(&row.function, row.parameters.as_ref()),
                &role_arn,
            )
            .await;
        let (execution_arn, described) = chain.ran(&machine, "only", case).await;
        assert_cause(case, &role_arn, &described, &row.function, &row.because);
        let cause = string(&described, "cause");
        // The work the `Resource` named is Lambda's own, so the attempt's failure is
        // recorded in that family's event — carrying the very text the read reports.
        let events = chain.history(&execution_arn).await;
        let recorded: Vec<String> = events
            .iter()
            .filter(|event| {
                event.get("type").and_then(Value::as_str) == Some(EVENT_LAMBDA_FUNCTION_FAILED)
            })
            .filter_map(|event| event.get("lambdaFunctionFailedEventDetails"))
            .map(|details| string(details, "cause"))
            .collect();
        assert_eq!(
            recorded,
            vec![cause.clone()],
            "case {case:?}: expected the Lambda work family's event to carry the one cause \
             {cause:?}, got {events:?}"
        );
    }
}
