// SPDX-License-Identifier: CC0-1.0

//! The PRINCIPAL a refused `Task` state's cause blames, driven through the production
//! `MultiServiceRouter` with Step Functions and IAM mounted on ONE engine.
//!
//! The rule under test: Step Functions holds no credentials of a state machine's execution
//! role — it ASSUMES the role for the run's work — so the identity one of the execution's
//! calls is made as is a SESSION of that role, and an invoked service's refusal names that
//! session the way `sts:AssumeRole` reports it:
//! `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`. The `sts`
//! vendor and the `assumed-role` resource type are what tell an assumed-role caller apart
//! from an IAM principal, so the role's own `arn:<partition>:iam::<account>:role/…` ARN is
//! never what such a refusal names. The DECISION is still the role's: what the documents in
//! force on it allow is unchanged by how the refusal spells the caller.
//!
//! The rows differ along what the rule generalises over: whether the role sits at the
//! account root or under a PATH (a session ARN carries the role's name alone, never its
//! path), which state machine the work is for (the session is named after it), which
//! service and action the refused call asks for, and whether the documents withhold the
//! call by stating nothing about it or by denying it outright.
//!
//! The wiring, the two wire protocols and the trust policy are
//! `tests/all/task_execution_role.rs`'s: `tests/all.rs` is one crate root, so this suite
//! borrows that harness rather than restating it.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::EVENT_TASK_FAILED;

use crate::execution_data_encryption::xml_element;
use crate::state_machine_lifecycle::{string, REGION};
use crate::task_execution_role::{trust_document, Chain};

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `Error Code:` fragment every row's cause carries.
const REFUSAL_CODE: &str = "Error Code: AccessDeniedException";

/// The ARN of the SESSION a refusal must blame, built the way `sts:AssumeRole` reports one:
/// the `sts` vendor, no Region, the role's own account, and the role's NAME — never its path
/// — followed by the session's name.
///
/// `pub(crate)` because every suite that reads a refused `Task` state's cause is held to the
/// same principal: `tests/all.rs` is one crate root, so the spelling is stated here once.
pub(crate) fn session_principal(role_arn: &str, session_name: &str) -> String {
    let (head, named) = role_arn
        .split_once(":role/")
        .unwrap_or_else(|| panic!("expected an IAM role ARN, got {role_arn:?}"));
    let role_name = named.rsplit('/').next().unwrap_or(named);
    let fields: Vec<&str> = head.split(':').collect();
    let partition = fields.get(1).copied().unwrap_or_default();
    let account = fields.get(4).copied().unwrap_or_default();
    format!("arn:{partition}:sts::{account}:assumed-role/{role_name}/{session_name}")
}

/// A definition whose one `Task` state makes `resource`'s call with `parameters`.
///
/// `pub(crate)` because every suite that reads a refused `Task` state's cause drives it
/// through one such state: `tests/all.rs` is one crate root, so the definition is stated
/// here once.
pub(crate) fn definition(resource: &str, parameters: &Value) -> String {
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

/// An IAM role under `path`, carrying `policy` as its inline policy.
///
/// `Chain::role` makes one at the account root; a role under a path is what the rule's
/// second row needs, because a session ARN carries the role's name without it.
async fn role_under(chain: &Chain, name: &str, path: &str, policy: &Value) -> String {
    let created = chain
        .iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("Path", path.to_string()),
                ("AssumeRolePolicyDocument", trust_document().to_string()),
            ],
        )
        .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("CreateRole answers the role Arn, got {created}"));
    chain
        .iam(
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", "inline".to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    arn
}

/// A document that says nothing about any integration call: the log writes a state machine
/// makes, which withhold every call by stating nothing about it.
///
/// `pub(crate)` for the reason [`definition`] is.
pub(crate) fn unrelated_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["logs:CreateLogDelivery", "logs:PutLogEvents"],
            "Resource": "*",
        }],
    })
}

/// A document that DENIES `action` outright, which is the other way the documents in force
/// refuse a call.
fn denying_policy(action: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Action": [action],
            "Resource": "*",
        }],
    })
}

/// One row: the case, the role the machine runs as, the machine the work is for, the call
/// the `Task` makes, and the sentence the refusal must word.
struct Row {
    case: &'static str,
    /// The IAM role's name, and the path IAM holds it under.
    role_name: &'static str,
    role_path: &'static str,
    /// The documents in force on it.
    policy: Value,
    /// The state machine's name, which is what the session is named after.
    machine_name: &'static str,
    /// The `Resource` the `Task` names and the parameters it carries.
    resource: &'static str,
    parameters: Value,
    /// The IAM action the refusal must name, and the ARN it must be refused ON.
    action: &'static str,
    resource_arn: String,
    /// The clause the sentence ends with: why the documents withheld the call.
    because: &'static str,
}

/// RULE: a refused `Task` state's cause names the assumed-role SESSION of the state
/// machine's execution role, never the role's own IAM ARN.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_tasks_cause_names_the_assumed_role_session_of_the_execution_role() {
    let chain = Chain::new();
    // No target has to exist: authorization is settled before the call, so these name the
    // resources a request addresses and nothing is created for them.
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:handler");
    let queue_url = format!("https://sqs.{REGION}.amazonaws.com/{ACCOUNT}/intake");
    let queue_arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:intake");
    let child_arn = format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:child");

    let rows = [
        Row {
            case: "an invocation no statement is about, under a role at the account root",
            role_name: "SessionPrincipalRole",
            role_path: "/",
            policy: unrelated_policy(),
            machine_name: "session-principal-invoke",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            action: "lambda:InvokeFunction",
            resource_arn: function_arn.clone(),
            because: "because no identity-based policy allows the lambda:InvokeFunction action",
        },
        Row {
            // The same call, refused by an explicit `Deny`, under a role IAM holds beneath
            // a path: the session ARN carries the role's NAME and not the path.
            case: "an invocation denied outright, under a role beneath a path",
            role_name: "SessionDeniedRole",
            role_path: "/service-role/",
            policy: denying_policy("lambda:InvokeFunction"),
            machine_name: "session-denied-invoke",
            resource: "arn:aws:states:::lambda:invoke",
            parameters: json!({ "FunctionName": function_arn.clone(), "Payload": {} }),
            action: "lambda:InvokeFunction",
            resource_arn: function_arn.clone(),
            because: "with an explicit deny in an identity-based policy",
        },
        Row {
            // Another service's call, addressed by a queue url, for another state machine:
            // the session is named after THAT machine.
            case: "a send to a queue no statement is about",
            role_name: "SessionQueueRole",
            role_path: "/",
            policy: unrelated_policy(),
            machine_name: "session-principal-send",
            resource: "arn:aws:states:::sqs:sendMessage",
            parameters: json!({ "QueueUrl": queue_url.clone(), "MessageBody": "hello" }),
            action: "sqs:SendMessage",
            resource_arn: queue_arn.clone(),
            because: "because no identity-based policy allows the sqs:SendMessage action",
        },
        Row {
            // And a call of THIS service, made through the AWS SDK integration family.
            case: "a child execution no statement is about, through the AWS SDK family",
            role_name: "SessionChildRole",
            role_path: "/",
            policy: unrelated_policy(),
            machine_name: "session-principal-child",
            resource: "arn:aws:states:::aws-sdk:sfn:startExecution",
            parameters: json!({ "StateMachineArn": child_arn.clone() }),
            action: "states:StartExecution",
            resource_arn: child_arn.clone(),
            because: "because no identity-based policy allows the states:StartExecution action",
        },
    ];

    for row in &rows {
        let case = row.case;
        let role_arn = role_under(&chain, row.role_name, row.role_path, &row.policy).await;
        let machine = chain
            .machine(
                row.machine_name,
                &definition(row.resource, &row.parameters),
                &role_arn,
            )
            .await;
        let (execution_arn, described) = chain.ran(&machine, "only", case).await;
        assert_eq!(
            described.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: expected the refused call to fail the execution, got {described:?}"
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

        // The whole sentence, principal first: the session of the role, the action, the
        // resource the request addressed and why the documents withheld the call.
        let principal = session_principal(&role_arn, row.machine_name);
        let expected = format!(
            "User: {principal} is not authorized to perform: {} on resource: {} {}",
            row.action, row.resource_arn, row.because
        );
        assert!(
            cause.starts_with(&expected),
            "case {case:?}: expected the cause to open {expected:?}, got {cause:?}"
        );
        // The principal is an assumed-role session of the token service, which is what
        // tells it apart from an IAM principal.
        for fragment in ["arn:aws:sts::", ":assumed-role/"] {
            assert!(
                principal.contains(fragment),
                "case {case:?}: expected the blamed principal to carry {fragment:?}, got \
                 {principal:?}"
            );
        }
        // And the role's own IAM ARN appears nowhere in it: that spelling names a principal
        // rather than the session the call was made as.
        assert!(
            !cause.contains(&role_arn),
            "case {case:?}: expected the cause never to name the role ARN {role_arn:?}, got \
             {cause:?}"
        );
        // The refusal is still the invoked service's own answer to an unauthorized call.
        assert!(
            cause.contains(REFUSAL_CODE),
            "case {case:?}: expected the cause to carry {REFUSAL_CODE:?}, got {cause:?}"
        );
    }
}
