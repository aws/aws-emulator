// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the key an execution's REMAINING WORK is done under, driven
//! over each service's own wire protocol through the production `MultiServiceRouter`
//! with Step Functions, IAM and KMS mounted on ONE engine — the shape
//! `core/router/src/lib.rs` mounts them in — so the key state and the key policy the
//! decision reads are the ones KMS's own handlers hold.
//!
//! The rule under test: an execution of a state machine on a customer managed key does
//! its remaining work under that key, as the state machine's EXECUTION ROLE, so the
//! outcome the clock makes due is decided against the key as it stands at the instant
//! that outcome is owed — a key in a key state that admits no cryptographic use of it,
//! and a key the authorities in force no longer let that role decrypt with, each make
//! the due outcome a `States.Runtime` FAILURE carrying KMS's own sentence as the cause,
//! in place of the outcome the walk computed. A key that is usable by that role when
//! the outcome comes due leaves the computed outcome alone, and so does a condition put
//! on the key only AFTER the outcome was already materialised.
//!
//! The rows differ along WHAT makes the key unusable for the run — the key's own key
//! state (disabled, deletion scheduled) and the authorization over it (an explicit deny
//! of the decrypt to the role, and a policy that grants the account nothing at all) —
//! beside the control row that touches the key not at all.
//!
//! The harness (`Chain`, the key policies) is `tests/all/execution_data_encryption.rs`'s
//! and the role is `tests/all/role_log_and_key_authorization.rs`'s; `tests/all.rs` is
//! this binary's crate root, so both are reached through `crate::<suite>`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::execution_data_encryption::{customer_managed, granting_policy, Chain, SFN};
use crate::role_log_and_key_authorization::{data_key_policy, role};
use crate::state_machine_lifecycle::{string, ACCOUNT};

/// The ASL error a run that could not be done at all fails with.
const RUNTIME_ERROR: &str = "States.Runtime";

/// How long the `Wait` state of the definition below parks the execution for: long
/// enough that the condition under test is installed while the execution is still
/// RUNNING, and short enough that the outcome comes due inside the poll below.
const WAIT_SECONDS: u64 = 3;

/// A definition that parks the execution in a `Wait` state and then succeeds, so the
/// outcome the walk computed comes due a known span after the start.
fn waiting_definition() -> String {
    json!({
        "StartAt": "Park",
        "States": {
            "Park": { "Type": "Wait", "Seconds": WAIT_SECONDS, "Next": "Done" },
            "Done": { "Type": "Pass", "Result": { "msg": "hi" }, "End": true },
        },
    })
    .to_string()
}

/// How long a poll below waits for a terminal status before giving up: generous, so the
/// outcome is waited for on any host rather than being timed.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll sleeps between looks.
///
/// Deliberately LONGER than the span the `Wait` above parks the execution for, so the
/// instant the outcome comes due passes with no call of this suite's own in flight: the
/// in-process clock is then what materialises it, and the rule is pinned where no
/// caller is there to be served. Nothing here is timed — every look waits for the
/// CONDITION, up to [`TERMINAL_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// The shortest deletion window `ScheduleKeyDeletion` accepts, which is the one the row
/// that schedules a deletion asks for: the window only says when the key material goes,
/// and the key is in `PendingDeletion` for all of it.
const SHORTEST_DELETION_WINDOW_DAYS: u32 = 7;

/// A key policy granting one named principal nothing and the account nothing: the key
/// still carries a document, and no statement in it covers this account's identities.
fn policy_granting_nobody() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AnotherAccountOnly",
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::210987654321:root" },
            "Action": "kms:*",
            "Resource": "*",
        }]
    })
}

/// A key policy that keeps the account's own delegation and DENIES one named principal
/// the decrypt an execution's remaining work makes.
fn policy_denying_decrypt(principal: &str) -> Value {
    let mut statements = granting_policy()
        .get("Statement")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    statements.push(json!({
        "Sid": "DenyTheRoleTheDecrypt",
        "Effect": "Deny",
        "Principal": { "AWS": principal },
        "Action": "kms:Decrypt",
        "Resource": "*",
    }));
    json!({ "Version": "2012-10-17", "Statement": statements })
}

/// What a row does to the key while the execution is parked in its `Wait` state.
enum Condition {
    /// Nothing at all: the control row.
    None,
    /// One KMS call, by key id.
    Kms(&'static str),
    /// Replace the key policy with the document this builds for the execution role.
    Policy(fn(&str) -> Value),
}

/// One row: what is done to the key mid-execution, and what the due outcome becomes.
struct Row {
    case: &'static str,
    condition: Condition,
    /// The status the execution is expected to leave `RUNNING` with.
    status: &'static str,
    /// The sentence the relayed cause must state, built from the key ARN — `None` for a
    /// row that expects no failure at all.
    cause: Option<fn(&str, &str) -> String>,
}

/// The sentence KMS refuses a use its key state admits nothing of.
fn disabled_sentence(key: &str, _role: &str) -> String {
    format!("{key} is disabled.")
}

/// The same for a key whose deletion the account scheduled.
fn pending_deletion_sentence(key: &str, _role: &str) -> String {
    format!("{key} is pending deletion.")
}

/// The sentence KMS refuses an explicitly denied use with, for the SESSION the role is
/// assumed under: a refusal about work Step Functions does as the role blames a session
/// of it, not the `iam` role ARN.
fn explicit_deny_sentence(key: &str, role_name: &str) -> String {
    format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/<session> is not authorized to \
         perform: kms:Decrypt on resource: {key} with an explicit deny in a resource-based policy"
    )
}

/// The sentence KMS refuses a use no document allows.
fn no_grant_sentence(key: &str, role_name: &str) -> String {
    format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{role_name}/<session> is not authorized to \
         perform: kms:Decrypt on resource: {key} because no resource-based policy allows the \
         kms:Decrypt action"
    )
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the key is left usable",
            condition: Condition::None,
            status: "SUCCEEDED",
            cause: None,
        },
        Row {
            case: "the key is disabled",
            condition: Condition::Kms("DisableKey"),
            status: "FAILED",
            cause: Some(disabled_sentence),
        },
        Row {
            case: "the key's deletion is scheduled",
            condition: Condition::Kms("ScheduleKeyDeletion"),
            status: "FAILED",
            cause: Some(pending_deletion_sentence),
        },
        Row {
            case: "the execution role is explicitly denied the decrypt",
            condition: Condition::Policy(policy_denying_decrypt),
            status: "FAILED",
            cause: Some(explicit_deny_sentence),
        },
        Row {
            case: "the key policy grants this account nothing",
            condition: Condition::Policy(|_role| policy_granting_nobody()),
            status: "FAILED",
            cause: Some(no_grant_sentence),
        },
    ]
}

/// The execution's metadata, which is the read that reports a status while the key is
/// unusable: `METADATA_ONLY` opens nothing the key sealed.
async fn metadata(chain: &Chain, execution: &str) -> Value {
    chain
        .sfn(
            "DescribeExecution",
            &json!({ "executionArn": execution, "includedData": "METADATA_ONLY" }),
        )
        .await
}

/// Poll the execution's metadata until it reports a status other than `RUNNING`, and
/// answer that record. Panics with `case` when the deadline passes first.
async fn poll_until_terminal(chain: &Chain, case: &str, execution: &str) -> Value {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let described = metadata(chain, execution).await;
        if string(&described, "status") != "RUNNING" {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the execution to leave RUNNING, got {described:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Assert the execution is still parked in its `Wait` state, which is where the row's
/// condition is installed: the walk plans the wait at the start, so the execution is
/// RUNNING from its first report until the wait the definition asked for has passed.
async fn assert_parked(chain: &Chain, case: &str, execution: &str) {
    let described = metadata(chain, execution).await;
    assert_eq!(
        string(&described, "status"),
        "RUNNING",
        "case {case:?}: expected the execution to be parked in its Wait state, got {described:?}"
    );
}

/// The relayed cause with the per-call values masked: the session name Step Functions
/// assumed the role under, and the id and attempt count of the refused KMS call.
fn without_per_call_values(cause: &str, role_name: &str) -> String {
    let masked = match cause.split_once(&format!("assumed-role/{role_name}/")) {
        Some((head, tail)) => match tail.split_once(' ') {
            Some((_session, rest)) => {
                format!("{head}assumed-role/{role_name}/<session> {rest}")
            }
            None => cause.to_string(),
        },
        None => cause.to_string(),
    };
    match masked.split_once(" (Service: Kms, Status Code: 400, Request ID: ") {
        Some((sentence, _metadata)) => sentence.to_string(),
        None => masked,
    }
}

/// The whole rule, row by row: what the key admits when the outcome comes due is what
/// the execution ends as.
///
/// Multi-threaded, so the in-process clock that materialises a due outcome runs while
/// this test waits: the rule is about the instant the outcome was owed, not about which
/// of the two reached it first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_outcome_coming_due_follows_the_key_the_run_works_under() {
    for (index, row) in rows().into_iter().enumerate() {
        let Row {
            case,
            condition,
            status: expected,
            cause,
        } = row;
        let chain = Chain::new();
        let role_name = format!("run-key-role-{index}");
        let role_arn = role(&chain, &role_name, &data_key_policy()).await;
        let key = chain.create_key(&granting_policy()).await;
        let created = chain
            .sfn(
                "CreateStateMachine",
                &json!({
                    "name": format!("run-key-{index}"),
                    "definition": waiting_definition(),
                    "roleArn": role_arn,
                    "encryptionConfiguration": customer_managed(&key),
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = chain
            .sfn(
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "run" }),
            )
            .await;
        let execution = string(&started, "executionArn");
        assert_parked(&chain, case, &execution).await;

        install(&chain, &condition, &key, &role_arn).await;

        let described = poll_until_terminal(&chain, case, &execution).await;
        assert_eq!(
            string(&described, "status"),
            expected,
            "case {case:?}: expected the execution to end {expected}, got {described:?}"
        );
        // The execution's `error` and `cause` are data of its own, sealed under the key,
        // so they are reported only to a read that opens them — which is served again
        // once the key is back to what it was.
        withdraw(&chain, &condition, &key).await;
        let described = chain
            .sfn(
                "DescribeExecution",
                &json!({ "executionArn": execution }),
            )
            .await;
        assert_eq!(
            string(&described, "status"),
            expected,
            "case {case:?}: expected the whole record to report the same outcome, got \
             {described:?}"
        );
        match cause {
            None => {
                assert_eq!(
                    (
                        described.get("error").and_then(Value::as_str),
                        described.get("cause").and_then(Value::as_str)
                    ),
                    (None, None),
                    "case {case:?}: expected no failure at all, got {described:?}"
                );
            }
            Some(sentence) => {
                assert_eq!(
                    described.get("error").and_then(Value::as_str),
                    Some(RUNTIME_ERROR),
                    "case {case:?}: expected the run's own {RUNTIME_ERROR}, got {described:?}"
                );
                let reported = described
                    .get("cause")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let masked = without_per_call_values(reported, &role_name);
                assert_eq!(
                    masked,
                    sentence(&key, &role_name),
                    "case {case:?}: expected KMS's own sentence relayed as the cause, got \
                     {reported:?}"
                );
            }
        }
    }
}

/// Put the row's condition on the key.
async fn install(chain: &Chain, condition: &Condition, key: &str, role_arn: &str) {
    match condition {
        Condition::None => {}
        Condition::Kms("ScheduleKeyDeletion") => {
            chain
                .kms(
                    "ScheduleKeyDeletion",
                    &json!({ "KeyId": key, "PendingWindowInDays": SHORTEST_DELETION_WINDOW_DAYS }),
                )
                .await;
        }
        Condition::Kms(operation) => {
            chain.kms(operation, &json!({ "KeyId": key })).await;
        }
        Condition::Policy(document) => {
            chain.put_policy(key, &document(role_arn)).await;
        }
    }
}

/// Take the row's condition back off the key, so the reads that report the execution's
/// own data are served again.
async fn withdraw(chain: &Chain, condition: &Condition, key: &str) {
    match condition {
        Condition::None => {}
        // Cancelling a scheduled deletion leaves the key DISABLED, so it takes both
        // calls to put it back into use.
        Condition::Kms("ScheduleKeyDeletion") => {
            chain.kms("CancelKeyDeletion", &json!({ "KeyId": key })).await;
            chain.kms("EnableKey", &json!({ "KeyId": key })).await;
        }
        Condition::Kms(_) => {
            chain.kms("EnableKey", &json!({ "KeyId": key })).await;
        }
        Condition::Policy(_) => {
            chain.put_policy(key, &granting_policy()).await;
        }
    }
}

/// The boundary of the rule: it decides an outcome that is still OWED. A condition put
/// on the key after the outcome was already materialised revisits nothing, so the
/// execution keeps the outcome the walk computed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_made_unusable_after_the_outcome_settled_changes_nothing() {
    let chain = Chain::new();
    let role_arn = role(&chain, "settled-run-key-role", &data_key_policy()).await;
    let key = chain.create_key(&granting_policy()).await;
    let created = chain
        .sfn(
            "CreateStateMachine",
            &json!({
                "name": "settled-run-key",
                "definition": waiting_definition(),
                "roleArn": role_arn,
                "encryptionConfiguration": customer_managed(&key),
            }),
        )
        .await;
    let machine = string(&created, "stateMachineArn");
    let started = chain
        .sfn(
            "StartExecution",
            &json!({ "stateMachineArn": machine, "name": "run" }),
        )
        .await;
    let execution = string(&started, "executionArn");
    let settled = poll_until_terminal(&chain, "the outcome had already settled", &execution).await;
    assert_eq!(
        string(&settled, "status"),
        "SUCCEEDED",
        "case the outcome had already settled: expected the computed outcome, got {settled:?}"
    );

    chain.kms("DisableKey", &json!({ "KeyId": key })).await;
    let (status, answer) = chain
        .call(
            &SFN,
            "DescribeExecution",
            &json!({ "executionArn": execution, "includedData": "METADATA_ONLY" }),
        )
        .await;
    assert_eq!(
        (status, string(&answer, "status").as_str()),
        (200, "SUCCEEDED"),
        "case the outcome had already settled: expected the outcome to stand, got {status} with \
         {answer:?}"
    );
}
