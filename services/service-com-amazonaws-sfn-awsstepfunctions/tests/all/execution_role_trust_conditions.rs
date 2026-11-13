// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CONDITIONS a state machine's execution-role trust policy
//! narrows the assume to, driven over each service's own wire protocol through the
//! production `MultiServiceRouter` with Step Functions, IAM and KMS mounted on ONE engine
//! — the shape `core/router/src/lib.rs` mounts them in — so the trust document the assume
//! is decided against is the one IAM's own handlers wrote and the key policy is the one
//! KMS's own handlers hold.
//!
//! Two rules, both about the one assume Step Functions makes to do an execution's work.
//!
//! 1. **the assume is ONE assume.** Step Functions takes the state machine's `roleArn` on
//!    through `sts:AssumeRole` for the work of an execution, presenting the state machine
//!    the work is for under the two confused-deputy keys a service trust policy is
//!    written against (`aws:SourceArn`, the state machine's own ARN, and
//!    `aws:SourceAccount`, the account that owns it). A statement's `Condition` is part of
//!    its grant, so a role whose trust is narrowed to another state machine, to another
//!    account, or to a key the assume carries no value for is a role this service may not
//!    act as for THIS execution. Nothing that role would have done then happens: a state
//!    machine on a CUSTOMER MANAGED key starts nothing at all — the data key its execution's
//!    data goes under is minted as that role, and credentials the assume never minted seal
//!    nothing, so the mint is left to this service's own principal, which the KEY'S OWN
//!    POLICY is the only authority over — and a `Task` state of an execution that IS
//!    started fails rather than having its work done as nobody
//!    (`tests/all/execution_role_assumption.rs` holds that half of the rule).
//! 2. **the refusal names the acting identity.** Where it is the KEY that refuses the
//!    mint, Step Functions relays what KMS answered, and the identity such a refusal
//!    blames is the SESSION of the role this service assumed —
//!    `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, the
//!    spelling `sts:AssumeRole` reports — never the `iam` role ARN, whose path an STS
//!    session ARN does not carry at all.
//!
//! The rows differ along what each rule generalises over: the condition OPERATOR
//! (`StringEquals`, `StringNotEquals`, `ArnEquals`, `ArnLike`), which key it is written
//! over, whether the value the assume carries satisfies it, how many clauses the statement
//! carries, and — for the refusal's wording — where IAM holds the role (at the root path,
//! under a path, and under a nested path). Each rule's own boundary is a test of its own:
//! a `roleArn` IAM holds no record for refuses nothing at all.
//!
//! The harness (`Chain`, the key policies) is `tests/all/execution_data_encryption.rs`'s;
//! `tests/all.rs` is this binary's crate root, so it is reached through
//! `crate::execution_data_encryption`. Whether the role is trusted AT ALL is the weaker
//! question, and `tests/all/execution_role_assumption.rs` holds it.

use serde_json::{json, Value};

use crate::execution_data_encryption::{
    customer_managed, granting_policy, xml_element, Chain, KMS_ACCESS_DENIED,
    KMS_ACCESS_DENIED_STATUS, SFN,
};
use crate::state_machine_lifecycle::{error_code, string, ACCOUNT, HELLO_DEFINITION, REGION};

/// An account that is not the one these resources belong to, for a clause the assume
/// cannot satisfy.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// The KMS calls sealing an execution's data makes, as the role's own policy allows
/// them: a row that gets past the assume must not then be refused on what the role may
/// do.
fn data_key_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["kms:GenerateDataKey", "kms:Decrypt"],
            "Resource": "*",
        }]
    })
}

/// A trust policy trusting this service's global principal to assume the role, narrowed
/// by `condition` where the row states one.
fn trust(condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": { "Service": "states.amazonaws.com" },
        "Action": "sts:AssumeRole",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] })
}

/// A role of this account at `path`, carrying `trust` as its trust policy and the KMS
/// calls an execution's own cryptographic work makes as its inline policy, by its own ARN.
async fn role_at(chain: &Chain, name: &str, path: &str, trust: &Value) -> String {
    let created = chain
        .iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("Path", path.to_string()),
                ("AssumeRolePolicyDocument", trust.to_string()),
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
                ("PolicyDocument", data_key_policy().to_string()),
            ],
        )
        .await;
    arn
}

/// A key policy naming the account root — the form the default key policy takes, which
/// delegates to that account's own identity-based policies — beside `statements`.
fn key_policy_with(statements: Vec<Value>) -> Value {
    let mut all = granting_policy()
        .get("Statement")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    all.extend(statements);
    json!({ "Version": "2012-10-17", "Statement": all })
}

/// The ARN a state machine of this account named `name` carries, which is the value the
/// assume presents under `aws:SourceArn`.
fn machine_arn(name: &str) -> String {
    format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:{name}")
}

/// A state machine on `key` running as `role_arn`, by its ARN.
async fn encrypted_machine(chain: &Chain, name: &str, role_arn: &str, key: &str) -> String {
    let created = chain
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": name,
                "definition": HELLO_DEFINITION,
                "roleArn": role_arn,
                "encryptionConfiguration": customer_managed(key),
            }),
        )
        .await;
    string(&created, "stateMachineArn")
}

/// How many executions `ListExecutions` reports for `machine`.
async fn started_count(chain: &Chain, machine: &str) -> usize {
    let listed = chain
        .sfn(
            "ListExecutions",
            &json!({ "stateMachineArn": machine, "maxResults": 10 }),
        )
        .await;
    listed
        .get("executions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// One row of the trust-condition table: what the statement is narrowed by, and whether
/// the assume it governs covers this state machine's execution.
struct Row {
    case: &'static str,
    /// The `Condition` element the trust statement carries, built from the ARN of the
    /// state machine the row starts and from the accounts above.
    condition: fn(&str) -> Option<Value>,
    /// Whether the start is served.
    assumable: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a statement carrying no condition at all",
            condition: |_machine| None,
            assumable: true,
        },
        Row {
            case: "the owning account under StringEquals aws:SourceAccount",
            condition: |_machine| Some(json!({ "StringEquals": { "aws:SourceAccount": ACCOUNT } })),
            assumable: true,
        },
        Row {
            case: "this state machine's own ARN under ArnEquals aws:SourceArn",
            condition: |machine| Some(json!({ "ArnEquals": { "aws:SourceArn": machine } })),
            assumable: true,
        },
        Row {
            case: "every state machine of this account and Region under ArnLike aws:SourceArn",
            condition: |_machine| {
                Some(json!({
                    "ArnLike": {
                        "aws:SourceArn":
                            format!("arn:aws:states:{REGION}:{ACCOUNT}:stateMachine:*"),
                    }
                }))
            },
            assumable: true,
        },
        Row {
            case: "both keys together, each satisfied",
            condition: |machine| {
                Some(json!({
                    "StringEquals": { "aws:SourceAccount": ACCOUNT },
                    "ArnEquals": { "aws:SourceArn": machine },
                }))
            },
            assumable: true,
        },
        Row {
            case: "another account under StringEquals aws:SourceAccount",
            condition: |_machine| {
                Some(json!({ "StringEquals": { "aws:SourceAccount": ANOTHER_ACCOUNT } }))
            },
            assumable: false,
        },
        Row {
            case: "another state machine of this account under ArnEquals aws:SourceArn",
            condition: |_machine| {
                Some(json!({ "ArnEquals": { "aws:SourceArn": machine_arn("somewhere-else") } }))
            },
            assumable: false,
        },
        Row {
            case: "another Region's state machines under ArnLike aws:SourceArn",
            condition: |_machine| {
                Some(json!({
                    "ArnLike": {
                        "aws:SourceArn": format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:*"),
                    }
                }))
            },
            assumable: false,
        },
        Row {
            // A negated operator over the value the assume DOES carry excludes exactly
            // this account's state machines.
            case: "the owning account under StringNotEquals aws:SourceAccount",
            condition: |_machine| {
                Some(json!({ "StringNotEquals": { "aws:SourceAccount": ACCOUNT } }))
            },
            assumable: false,
        },
        Row {
            // Nothing is guessed for a key the assume does not carry, so a clause over
            // one cannot be shown to hold and the grant does not cover the assume.
            case: "a clause over a key the assume carries no value for",
            condition: |_machine| {
                Some(json!({ "StringEquals": { "sts:ExternalId": "a-shared-secret" } }))
            },
            assumable: false,
        },
        Row {
            // The boundary of the rule: one clause that cannot be satisfied withholds
            // the grant whatever the clause beside it says.
            case: "a satisfied clause beside one for another account",
            condition: |machine| {
                Some(json!({
                    "ArnEquals": { "aws:SourceArn": machine },
                    "StringEquals": { "aws:SourceAccount": ANOTHER_ACCOUNT },
                }))
            },
            assumable: false,
        },
    ]
}

/// `StartExecution` on a state machine encrypted with a customer managed key follows the
/// trust policy's conditions: a role whose grant covers the assume this start would make
/// runs, and one whose grant does not is refused with the modelled
/// `KmsAccessDeniedException`, having started nothing.
#[tokio::test]
async fn a_start_follows_the_conditions_the_trust_policy_narrows_the_assume_to() {
    let chain = Chain::new();
    let key = chain.create_key(&key_policy_with(Vec::new())).await;

    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            condition,
            assumable,
        } = row;
        let name = format!("trust-gate-{index}");
        let expected_arn = machine_arn(&name);
        let role_arn = role_at(
            &chain,
            &format!("trust-role-{index}"),
            "/",
            &trust(condition(&expected_arn)),
        )
        .await;
        let machine = encrypted_machine(&chain, &name, &role_arn, &key).await;
        assert_eq!(
            machine, expected_arn,
            "case {case:?}: the state machine the condition names is the one created, got {machine}"
        );

        let (status, answered) = chain
            .call(
                &SFN,
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "run" }),
            )
            .await;
        if assumable {
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
        assert_eq!(
            (status, code.as_str()),
            (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
            "case {case:?}: expected the modelled KMS refusal, got {status} {code} with \
             {answered:?}"
        );
        // Credentials the assume never minted seal nothing, so the mint is left to be made
        // as this service's OWN principal, on behalf of the state machine — and the key's
        // policy, the only authority over such a caller, names no grant for it. The
        // refusal is the sentence KMS answered that with, relayed with the metadata that
        // answer carried.
        let message = answered
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected = format!(
            "User: states.amazonaws.com is not authorized to perform: kms:GenerateDataKey on \
             resource: {key} because no resource-based policy allows the kms:GenerateDataKey \
             action (Service: Kms, Status Code: 400, Request ID: "
        );
        assert!(
            message.starts_with(&expected),
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_eq!(
            started_count(&chain, &machine).await,
            0,
            "case {case:?}: expected a refused start to have started nothing"
        );
    }
}

/// A role ARN of this account that IAM holds nothing under, which is how an execution
/// role provisioned out of band looks to this emulator.
const NOT_HELD: &str = "arn:aws:iam::000000000000:role/NotHeldHere";

/// The boundary of the first rule: it refuses only what IAM ANSWERED. A `roleArn` this
/// account's IAM holds no record for says nothing about any trust document, so the start
/// of an encrypted state machine naming it is served — the deploy-time/run-time split this
/// service exposes deliberately.
#[tokio::test]
async fn a_role_iam_holds_no_record_for_refuses_nothing() {
    let chain = Chain::new();
    let key = chain.create_key(&key_policy_with(Vec::new())).await;
    let machine = encrypted_machine(&chain, "unheld-role-machine", NOT_HELD, &key).await;

    let (status, answered) = chain
        .call(
            &SFN,
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "run" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case a role IAM holds nothing under: expected the start to be served, got {status} \
         {answered:?}"
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
        "case a role IAM holds nothing under: expected the execution of that machine, got \
         {described:?}"
    );
}

/// One row of the principal table: where IAM holds the role, which is what an `iam` role
/// ARN carries and an STS session ARN does not.
struct PathRow {
    case: &'static str,
    /// The path IAM holds the role under.
    path: &'static str,
    /// The name IAM holds it by, which is the one segment a session ARN names it with.
    name: &'static str,
}

/// The sentence KMS refuses an explicitly denied use with, for `principal` over `key`.
fn explicit_deny(principal: &str, key: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: kms:GenerateDataKey on resource: {key} \
         with an explicit deny in a resource-based policy"
    )
}

/// A statement denying one named principal the data-key mint sealing an execution's data
/// makes.
fn deny_statement(principal: &str) -> Value {
    json!({
        "Sid": "DenyTheRole",
        "Effect": "Deny",
        "Principal": { "AWS": principal },
        "Action": "kms:GenerateDataKey",
        "Resource": "*",
    })
}

/// A start the key refuses the execution role relays KMS's own sentence, and the identity
/// it blames is the SESSION this service assumed that role under — whatever path IAM holds
/// the role under, which a session ARN does not carry.
#[tokio::test]
async fn a_relayed_key_refusal_names_the_session_the_role_is_assumed_under() {
    let chain = Chain::new();
    let rows = [
        PathRow {
            case: "a role at the root path",
            path: "/",
            name: "session-role-root",
        },
        PathRow {
            case: "a role under a path",
            path: "/service-role/",
            name: "session-role-pathed",
        },
        PathRow {
            case: "a role under a nested path",
            path: "/service-role/states/",
            name: "session-role-nested",
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let PathRow { case, path, name } = row;
        let role_arn = role_at(&chain, name, path, &trust(None)).await;
        let key = chain
            .create_key(&key_policy_with(vec![deny_statement(&role_arn)]))
            .await;
        let machine_name = format!("session-machine-{index}");
        let machine = encrypted_machine(&chain, &machine_name, &role_arn, &key).await;

        let (status, answered) = chain
            .call(
                &SFN,
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "run" }),
            )
            .await;
        let code = error_code(&answered);
        assert_eq!(
            (status, code.as_str()),
            (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
            "case {case:?}: expected the modelled KMS refusal, got {status} {code} with \
             {answered:?}"
        );
        let message = answered
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // The principal is a session of THIS role: the `sts` vendor, the `assumed-role`
        // resource type, the name IAM holds the role by and a session name after it.
        let session_prefix = format!("arn:aws:sts::{ACCOUNT}:assumed-role/{name}/");
        let expected = explicit_deny(&format!("{session_prefix}<session>"), &key);
        let session = message
            .strip_prefix(&format!("User: {session_prefix}"))
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or_default()
            .to_string();
        assert!(
            !session.is_empty(),
            "case {case:?}: expected the principal to be {expected:?}, got {message:?}"
        );
        let sentence = message.replacen(&session, "<session>", 1);
        assert!(
            sentence.starts_with(&expected),
            "case {case:?}: expected {expected:?}, got {sentence:?}"
        );
        // ...and never the `iam` role ARN, whose path a session ARN does not carry.
        assert!(
            !message.contains(&role_arn),
            "case {case:?}: expected no {role_arn:?} in the relayed refusal, got {message:?}"
        );
        // The metadata tail is the KMS answer's own: the service, the status, the id of
        // the refused request and how many attempts it took.
        let tail = sentence
            .strip_prefix(&expected)
            .unwrap_or_default()
            .to_string();
        let reported_id = tail
            .strip_prefix(" (Service: Kms, Status Code: 400, Request ID: ")
            .and_then(|rest| rest.strip_suffix(") (SDK Attempt Count: 1)"))
            .unwrap_or_default();
        assert!(
            ::service_common::query_error_envelope::is_request_id(reported_id),
            "case {case:?}: expected KMS's own metadata tail naming the refused request, got \
             {tail:?}"
        );
        assert_eq!(
            started_count(&chain, &machine).await,
            0,
            "case {case:?}: expected a refused start to have started nothing"
        );
    }
}
