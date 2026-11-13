// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a STATE MACHINE'S OWN ROLE has to be allowed, driven over
//! each service's own wire protocol through the production `MultiServiceRouter` with Step
//! Functions, IAM and KMS mounted on ONE engine — the shape `core/router/src/lib.rs`
//! mounts them in — so every document a decision reads is one IAM's or KMS's own handlers
//! wrote.
//!
//! Two rules, both about the role rather than about the caller, because both are work the
//! service does AS that role:
//!
//! 1. **the log destination.** Step Functions sets a vended-log delivery up and writes the
//!    events as the state machine's role, so a control-plane call that states a
//!    `loggingConfiguration` at a level that logs is served only where that role's
//!    policies allow it the CloudWatch Logs delivery actions on the destination;
//!    otherwise it is refused with HTTP 400 `AccessDeniedException` carrying the service's
//!    fixed sentence, and nothing is created or changed — a state machine that could never
//!    deliver a log event is not reported as healthy.
//! 2. **the customer managed key.** Sealing an execution's data is done as the same role,
//!    and KMS weighs TWO authorities over such a use: the key's own resource-based policy
//!    and the role's identity-based policies. Either can grant it — a key policy naming the
//!    role grants it outright, and one naming the account delegates to the role's own
//!    documents — and an explicit deny in either refuses it, with `StartExecution` answering
//!    HTTP 400 `KmsAccessDeniedException` and starting nothing.
//!
//! The rows differ along what each rule generalises over: the level a logging
//! configuration asks for (one that logs, and `OFF`, which needs no grant at all), the
//! operation that states it (`CreateStateMachine` and `UpdateStateMachine`), and — for the
//! key — which document carries the grant (the key policy naming the role, the key policy
//! naming the account with the role's own policy allowing the call, neither, and an
//! explicit deny in the key policy).
//!
//! The harness (`Chain`, the key policies) is the one `execution_data_encryption` builds
//! for the same three services; `tests/all.rs` is this binary's crate root, so it is
//! reached through `crate::execution_data_encryption`.

use serde_json::{json, Value};

use service_com_amazonaws_sfn_awsstepfunctions::state::{observability, LOG_LEVEL_OFF};

use crate::execution_data_encryption::{
    customer_managed, granting_policy, xml_element, Chain, SFN,
};
use crate::state_machine_lifecycle::{error_code, string, ACCOUNT, HELLO_DEFINITION, REGION};

/// The code AWS refuses an authorization failure with, and the status it carries.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// The code and status a start the key does not permit is refused with.
const KMS_ACCESS_DENIED: &str = "KmsAccessDeniedException";

/// The trust policy a role Step Functions acts as carries.
fn trusted_by_step_functions() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "states.amazonaws.com" },
            "Action": "sts:AssumeRole",
        }]
    })
}

/// The CloudWatch Logs delivery policy the service's own logging guidance publishes for a
/// state machine's role: the delivery actions, on every resource.
fn log_delivery_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": [
                "logs:CreateLogDelivery",
                "logs:GetLogDelivery",
                "logs:UpdateLogDelivery",
                "logs:DeleteLogDelivery",
                "logs:ListLogDeliveries",
                "logs:PutResourcePolicy",
                "logs:DescribeResourcePolicies",
                "logs:DescribeLogGroups",
                "logs:CreateLogStream",
                "logs:PutLogEvents",
            ],
            "Resource": "*",
        }]
    })
}

/// A policy that grants ONE of the delivery actions and nothing else of CloudWatch
/// Logs: a role that can put the events but was never given the delivery calls, which
/// is the narrowest grant that is still access to the destination.
fn event_writing_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["logs:CreateLogStream", "logs:PutLogEvents"],
            "Resource": "*",
        }]
    })
}

/// A policy that grants something of ANOTHER service and nothing of CloudWatch Logs,
/// which is the least-privilege role a client's negative path uses.
///
/// `pub(crate)` for the reason [`data_key_policy`] is: a sibling suite needing a role whose
/// documents say nothing about the work under test reads this document rather than carrying
/// a second copy of it.
pub(crate) fn unrelated_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": "s3:ListAllMyBuckets",
            "Resource": "*",
        }]
    })
}

/// A policy allowing the KMS calls sealing an execution's data makes, which is what a key
/// policy that delegates to the account leaves to the role's own documents.
///
/// `pub(crate)`: `tests/all.rs` is this binary's crate root, so a sibling suite that needs
/// a role allowed an execution's own cryptographic work reads this document rather than
/// carrying a second copy of it.
pub(crate) fn data_key_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["kms:GenerateDataKey", "kms:Decrypt"],
            "Resource": "*",
        }]
    })
}

/// A role of this account carrying `policy` as its inline policy, as its ARN.
///
/// `pub(crate)` for the reason [`data_key_policy`] is.
pub(crate) async fn role(chain: &Chain, name: &str, policy: &Value) -> String {
    let created = chain
        .iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                (
                    "AssumeRolePolicyDocument",
                    trusted_by_step_functions().to_string(),
                ),
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

/// A `loggingConfiguration` at `level` delivering to a group of this account.
fn logging(level: &str, group: &str) -> Value {
    json!({
        "level": level,
        "includeExecutionData": true,
        "destinations": [{
            "cloudWatchLogsLogGroup": {
                "logGroupArn": format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:{group}:*"),
            }
        }],
    })
}

/// A `loggingConfiguration` that logs nothing, which needs no destination and no grant.
fn logging_off() -> Value {
    json!({ "level": LOG_LEVEL_OFF, "includeExecutionData": false })
}

/// One row of the log-destination table: what the role may do, what the configuration
/// asks for, and whether the call is served.
struct LogRow {
    case: &'static str,
    /// The policy the role carries.
    policy: fn() -> Value,
    /// The logging configuration the call states.
    configuration: fn(&str) -> Value,
    /// Whether the call is served.
    served: bool,
}

fn log_rows() -> Vec<LogRow> {
    vec![
        LogRow {
            case: "a role allowed nothing of CloudWatch Logs, at a level that logs",
            policy: unrelated_policy,
            configuration: |group| logging(observability::LOG_LEVEL_ALL, group),
            served: false,
        },
        LogRow {
            case: "the same role at the ERROR level, which also logs",
            policy: unrelated_policy,
            configuration: |group| logging(observability::LOG_LEVEL_ERROR, group),
            served: false,
        },
        LogRow {
            case: "a role carrying the published log-delivery policy",
            policy: log_delivery_policy,
            configuration: |group| logging(observability::LOG_LEVEL_ALL, group),
            served: true,
        },
        LogRow {
            // The boundary of the rule: the refusal is the role having NO access to the
            // destination, so a role allowed one of the delivery actions on it is
            // served — this service reads only the documents it can see, and a create
            // AWS serves is not refused on a partial grant.
            case: "a role allowed only the event-writing action on the destination",
            policy: event_writing_policy,
            configuration: |group| logging(observability::LOG_LEVEL_ALL, group),
            served: true,
        },
        LogRow {
            case: "a role allowed nothing of CloudWatch Logs, logging OFF",
            policy: unrelated_policy,
            configuration: |_group| logging_off(),
            served: true,
        },
    ]
}

/// `CreateStateMachine` states a logging configuration only where the role it names may
/// deliver to the destination: the refusal is the service's fixed sentence, and the state
/// machine it would have created exists nowhere.
#[tokio::test]
async fn a_logging_configuration_needs_the_roles_log_delivery_permission() {
    let chain = Chain::new();
    for (index, row) in log_rows().into_iter().enumerate() {
        let LogRow {
            case,
            policy,
            configuration,
            served,
        } = row;
        let role_arn = role(&chain, &format!("log-role-{index}"), &policy()).await;
        let group = format!("/aws/vendedlogs/states/role-{index}");
        let name = format!("log-gate-{index}");
        let (status, answered) = chain
            .call(
                &SFN,
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": HELLO_DEFINITION,
                    "roleArn": role_arn,
                    "loggingConfiguration": configuration(&group),
                }),
            )
            .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be served, got {status} {answered:?}"
            );
            // Round trip: the state machine the call reports is one this service holds,
            // and it is ACTIVE.
            let described = chain
                .sfn(
                    "DescribeStateMachine",
                    &json!({ "stateMachineArn": string(&answered, "stateMachineArn") }),
                )
                .await;
            assert_eq!(
                described.get("status").and_then(Value::as_str),
                Some("ACTIVE"),
                "case {case:?}: expected the created machine to be ACTIVE, got {described:?}"
            );
            continue;
        }
        let code = error_code(&answered);
        let message = answered.get("message").and_then(Value::as_str);
        assert_eq!(
            (status, code.as_str(), message),
            (
                ACCESS_DENIED_STATUS,
                ACCESS_DENIED,
                Some(observability::LOG_DESTINATION_ACCESS_DENIED)
            ),
            "case {case:?}: expected the fixed log-destination refusal, got {status} {code} with \
             {answered:?}"
        );
        // Nothing was created: the name the refused call asked for is in no listing.
        let listed = chain.sfn("ListStateMachines", &json!({})).await;
        let names: Vec<String> = listed
            .get("stateMachines")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|machine| string(machine, "name"))
            .collect();
        assert!(
            !names.contains(&name),
            "case {case:?}: expected the refused machine to exist nowhere, got {names:?}"
        );
    }
}

/// `UpdateStateMachine` is held to the same rule, over the role the machine will RUN as:
/// a configuration a role may not deliver is refused, and the machine is left as it was.
#[tokio::test]
async fn an_update_that_turns_logging_on_needs_the_same_permission() {
    let chain = Chain::new();
    let unallowed = role(&chain, "update-role-unallowed", &unrelated_policy()).await;
    let created = chain
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": "log-update-gate",
                "definition": HELLO_DEFINITION,
                "roleArn": unallowed,
            }),
        )
        .await;
    let machine = string(&created, "stateMachineArn");

    // The role the machine already runs as may not deliver, so turning logging on is
    // refused even though the request names no role of its own.
    let (status, answered) = chain
        .call(
            &SFN,
            "UpdateStateMachine",
            &json!({
                "stateMachineArn": machine,
                "loggingConfiguration": logging(
                    observability::LOG_LEVEL_ALL,
                    "/aws/vendedlogs/states/log-update-gate",
                ),
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (ACCESS_DENIED_STATUS, ACCESS_DENIED),
        "expected the update to be refused, got {status} {answered:?}"
    );
    let described = chain
        .sfn("DescribeStateMachine", &json!({ "stateMachineArn": machine }))
        .await;
    assert!(
        described.get("loggingConfiguration").is_none()
            || described
                .get("loggingConfiguration")
                .and_then(|configuration| configuration.get("level"))
                .and_then(Value::as_str)
                == Some(LOG_LEVEL_OFF),
        "expected the refused update to leave the machine's logging as it was, got {described:?}"
    );

    // ...and the same update naming a role that MAY deliver is served.
    let allowed = role(&chain, "update-role-allowed", &log_delivery_policy()).await;
    let (status, answered) = chain
        .call(
            &SFN,
            "UpdateStateMachine",
            &json!({
                "stateMachineArn": machine,
                "roleArn": allowed,
                "loggingConfiguration": logging(
                    observability::LOG_LEVEL_ALL,
                    "/aws/vendedlogs/states/log-update-gate",
                ),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected the update naming an allowed role to be served, got {status} {answered:?}"
    );
}

/// A statement granting one named principal the two calls sealing an execution's data
/// makes.
///
/// `pub(crate)` for the reason [`data_key_policy`] is.
pub(crate) fn key_statement_for(principal: &str, effect: &str) -> Value {
    json!({
        "Sid": "StatesRole",
        "Effect": effect,
        "Principal": { "AWS": principal },
        "Action": ["kms:GenerateDataKey", "kms:Decrypt"],
        "Resource": "*",
    })
}

/// A key policy naming the account root, which DELEGATES the decision to that account's
/// own identity-based policies, beside `statements`.
///
/// `pub(crate)` for the reason [`data_key_policy`] is.
pub(crate) fn key_policy_with(statements: Vec<Value>) -> Value {
    let mut all = granting_policy()
        .get("Statement")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    all.extend(statements);
    json!({ "Version": "2012-10-17", "Statement": all })
}

/// `StartExecution` on a state machine encrypted with a customer managed key follows the
/// authorities KMS weighs over the role's use of that key: the key's own policy, and — for
/// a policy that delegates to the account — the role's identity-based documents.
#[tokio::test]
async fn a_start_follows_the_key_policy_and_the_roles_own_policies() {
    let chain = Chain::new();
    // (case, whether the role's own policy allows the data-key calls, whether the key
    // policy names the role, the effect that statement carries, whether the start is served)
    let rows: Vec<(&str, bool, Option<&str>, bool)> = vec![
        (
            "the key policy grants the role, which has no KMS policy of its own",
            false,
            Some("Allow"),
            true,
        ),
        (
            "the key policy delegates to the account and the role's own policy allows it",
            true,
            None,
            true,
        ),
        (
            "the key policy delegates to the account and the role allows nothing of KMS",
            false,
            None,
            false,
        ),
        (
            "the key policy denies the role explicitly",
            true,
            Some("Deny"),
            false,
        ),
    ];

    for (index, (case, role_allows, key_states, served)) in rows.into_iter().enumerate() {
        let policy = if role_allows {
            data_key_policy()
        } else {
            unrelated_policy()
        };
        let role_arn = role(&chain, &format!("key-role-{index}"), &policy).await;
        let statements = match key_states {
            Some(effect) => vec![key_statement_for(&role_arn, effect)],
            None => Vec::new(),
        };
        let key = chain.create_key(&key_policy_with(statements)).await;
        let created = chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("key-gate-{index}"),
                    "definition": HELLO_DEFINITION,
                    "roleArn": role_arn,
                    "encryptionConfiguration": customer_managed(&key),
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");

        let (status, answered) = chain
            .call(
                &SFN,
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "run" }),
            )
            .await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the start to be served, got {status} {answered:?}"
            );
            // Round trip: the execution the start reports is one this service holds.
            let described = chain
                .sfn(
                    "DescribeExecution",
                    &json!({ "executionArn": string(&answered, "executionArn") }),
                )
                .await;
            assert_eq!(
                described.get("stateMachineArn").and_then(Value::as_str),
                Some(machine.as_str()),
                "case {case:?}: expected the execution of that machine, got {described:?}"
            );
            continue;
        }
        let code = error_code(&answered);
        let message = answered
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            (status, code.as_str()),
            (ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
            "case {case:?}: expected the modelled KMS refusal, got {status} {code} with \
             {answered:?}"
        );
        assert!(
            message.contains(&format!("assumed-role/key-role-{index}/")),
            "case {case:?}: expected the refusal to name the session of the role the use was \
             decided for, got {message:?}"
        );
        // Nothing was started: the machine has no executions.
        let listed = chain
            .sfn("ListExecutions", &json!({ "stateMachineArn": machine }))
            .await;
        assert_eq!(
            listed
                .get("executions")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0),
            "case {case:?}: expected a refused start to have started nothing, got {listed:?}"
        );
    }
}
