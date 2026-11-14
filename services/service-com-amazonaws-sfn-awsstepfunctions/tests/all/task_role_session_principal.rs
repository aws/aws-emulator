// SPDX-License-Identifier: CC0-1.0

//! ONE spelling of the identity Step Functions acts as, over both of the authorities that
//! can refuse work it does under a state machine's execution role — driven over each
//! service's own wire protocol through the production `MultiServiceRouter` with Step
//! Functions, IAM and KMS mounted on ONE engine, the shape `core/router/src/lib.rs`
//! mounts them in.
//!
//! RULE: this service holds no credentials of a state machine's `roleArn` — it ASSUMES
//! the role for the run's work — so every refusal of that work names a SESSION OF THAT
//! ROLE (`arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`),
//! whichever
//! authority refused: the role's OWN identity-based documents, which decide a `Task`
//! state's integration call, or the resource's CUSTOMER MANAGED KEY, whose policy and the
//! role's documents together decide the use that seals an execution's data. The role's
//! own `arn:<partition>:iam::<account>:role/…` ARN is never what such a refusal names,
//! and the decision is unchanged by how the refusal spells the caller. The session's NAME
//! is the one its assume minted — a value of that run's own, held to what a
//! `RoleSessionName` may be, and no name the request carried — so each assume the work
//! needs (the walk's own calls, and a use of the resource's key) carries its own.
//!
//! The rows differ along what the rule generalises over: which authority refuses (the
//! role's documents withholding an integration call, an explicit deny in the key's policy,
//! and a key policy that delegates to the account while the role's documents allow nothing
//! of KMS), the channel the refusal arrives in (a failed execution's `cause`, and the
//! modelled `KmsAccessDeniedException` of the start itself) and which state machine the
//! work is for.
//!
//! The harness is `tests/all/execution_data_encryption.rs`'s `Chain`, and the role maker,
//! the documents and the key policies are `tests/all/role_log_and_key_authorization.rs`'s —
//! the suite that holds what a state machine's own role has to be allowed — so nothing here
//! restates a document. The principal's own spelling is
//! `tests/all/task_refusal_session_principal.rs`'s `session_principal`. `tests/all.rs` is
//! this binary's crate root, so each is reached through its module.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_data_encryption::{
    customer_managed, Chain, KMS_ACCESS_DENIED, KMS_ACCESS_DENIED_STATUS, SFN,
};
use crate::role_log_and_key_authorization::{
    data_key_policy, key_policy_with, key_statement_for, role, unrelated_policy,
};
use crate::state_machine_lifecycle::{error_code, string, ACCOUNT, REGION};
use crate::task_refusal_session_principal::session_principal;

/// How long a row waits for an execution to leave `RUNNING`, and how long it sleeps
/// between two reads of that status. The deadline is generous: what is under test is the
/// sentence a settled execution carries, not how fast the host settles it.
const PATIENCE: Duration = Duration::from_secs(30);
/// See [`PATIENCE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// A definition whose one `Task` state invokes `function_arn`, which is the work the
/// role's own documents decide.
fn invoking(function_arn: &str) -> String {
    json!({
        "StartAt": "Call",
        "States": {
            "Call": {
                "Type": "Task",
                "Resource": "arn:aws:states:::lambda:invoke",
                "Parameters": { "FunctionName": function_arn, "Payload": {} },
                "End": true,
            }
        }
    })
    .to_string()
}

/// A definition that finishes at once, for a row whose refusal happens before any state
/// runs.
const HELLO: &str = r#"{"StartAt":"Done","States":{"Done":{"Type":"Pass","End":true}}}"#;

/// Which authority refuses the row's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Authority {
    /// The role's own identity-based documents, deciding a `Task` state's call.
    Documents,
    /// The resource's customer managed key, deciding the use that seals the execution's
    /// data.
    Key,
}

/// One row: the authority that refuses, the role and the documents in force on it, what
/// the key policy states about that role, and the state machine the work is for.
struct Row {
    case: &'static str,
    authority: Authority,
    role_name: &'static str,
    policy: Value,
    /// The effect the key policy states for the role, or `None` for a policy that only
    /// delegates to the account.
    key_effect: Option<&'static str>,
    machine_name: &'static str,
}

/// RULE: both authorities name the acting identity the same way — a session OF THE
/// EXECUTION ROLE, under the name its own assume minted, never the role's own ARN.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_refusal_of_the_roles_work_names_a_session_of_that_role() {
    let chain = Chain::new();
    // The function a `Task` row addresses: nothing is created for it, because the call is
    // refused before it is made.
    let function_arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:handler");

    let rows = [
        Row {
            case: "the role's own documents withhold the task's integration call",
            authority: Authority::Documents,
            role_name: "session-task-role",
            policy: unrelated_policy(),
            key_effect: None,
            machine_name: "session-one-task",
        },
        Row {
            // The other authority, over the same role: the key's own policy denies it the
            // sealing use outright, so the start is refused rather than a task failing.
            case: "the key policy denies the role the sealing use",
            authority: Authority::Key,
            role_name: "session-key-denied-role",
            policy: data_key_policy(),
            key_effect: Some("Deny"),
            machine_name: "session-one-key-denied",
        },
        Row {
            // And the delegated case, where it is again the role's documents that decide
            // — the refusal is still the key's, and still about the same session.
            case: "the key policy delegates and the role allows nothing of KMS",
            authority: Authority::Key,
            role_name: "session-key-unstated-role",
            policy: unrelated_policy(),
            key_effect: None,
            machine_name: "session-one-key-unstated",
        },
    ];

    for row in &rows {
        let case = row.case;
        let role_arn = role(&chain, row.role_name, &row.policy).await;

        let refusal = match row.authority {
            Authority::Documents => {
                let created = chain
                    .sfn(
                        "CreateStateMachine",
                        &json!({
                            "name": row.machine_name,
                            "definition": invoking(&function_arn),
                            "roleArn": role_arn,
                        }),
                    )
                    .await;
                let machine = string(&created, "stateMachineArn");
                let started = chain
                    .sfn(
                        "StartExecution",
                        &json!({ "stateMachineArn": machine, "name": "only" }),
                    )
                    .await;
                let execution = string(&started, "executionArn");
                // The outcome is owed to the clock, so the row looks again until the
                // execution has left `RUNNING` or the deadline passes.
                let began = Instant::now();
                loop {
                    let described = chain
                        .sfn("DescribeExecution", &json!({ "executionArn": execution }))
                        .await;
                    if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
                        assert_eq!(
                            described.get("status").and_then(Value::as_str),
                            Some("FAILED"),
                            "case {case:?}: expected the refused call to fail the execution, \
                             got {described:?}"
                        );
                        break string(&described, "cause");
                    }
                    assert!(
                        began.elapsed() < PATIENCE,
                        "case {case:?}: expected a terminal status within {PATIENCE:?}, last \
                         saw {described:?}"
                    );
                    ::tokio::time::sleep(POLL_INTERVAL).await;
                }
            }
            Authority::Key => {
                let statements = match row.key_effect {
                    Some(effect) => vec![key_statement_for(&role_arn, effect)],
                    None => Vec::new(),
                };
                let key = chain.create_key(&key_policy_with(statements)).await;
                let created = chain
                    .sfn(
                        "CreateStateMachine",
                        &json!({
                            "name": row.machine_name,
                            "definition": HELLO,
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
                        &json!({ "stateMachineArn": machine, "name": "only" }),
                    )
                    .await;
                // The key's refusal is the start's own modelled error, and nothing was
                // started under it.
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (KMS_ACCESS_DENIED_STATUS, KMS_ACCESS_DENIED),
                    "case {case:?}: expected the modelled KMS refusal, got {status} with \
                     {answered:?}"
                );
                let listed = chain
                    .sfn("ListExecutions", &json!({ "stateMachineArn": machine }))
                    .await;
                assert_eq!(
                    listed
                        .get("executions")
                        .and_then(Value::as_array)
                        .map(Vec::len),
                    Some(0),
                    "case {case:?}: expected a refused start to have started nothing, got \
                     {listed:?}"
                );
                string(&answered, "message")
            }
        };

        // The identity this row's refusal names: a session of the role, under the name
        // the assume gave it.
        let principal = session_principal(&role_arn, &refusal);
        assert!(
            refusal.contains(&principal),
            "case {case:?}: expected the refusal to name the acting session {principal}, got \
             {refusal:?}"
        );
        // The role's own IAM ARN appears nowhere in it: that spelling names a principal
        // rather than the session the work was done as.
        assert!(
            !refusal.contains(&role_arn),
            "case {case:?}: expected the refusal never to name the role ARN {role_arn:?}, got \
             {refusal:?}"
        );
    }
}
