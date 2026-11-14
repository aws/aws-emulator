// SPDX-License-Identifier: CC0-1.0

//! Assuming the role a target names is the FIRST step of that target's
//! invocation, so a role EventBridge cannot assume fails the delivery before any
//! target API call is made — and a failed delivery goes to the target's
//! dead-letter queue.
//!
//! The RULE asserted here: EventBridge invokes a target that NAMES a `RoleArn`
//! by acting AS that role, which it assumes before it chooses or makes the
//! target's call — so a `RoleArn` whose trust policy does not admit this service
//! fails the handover whatever service the target belongs to, the event is
//! written to the target's `DeadLetterConfig` queue carrying the condition of
//! that step, and the target itself is never reached. The step is about the role
//! alone, so it decides a handover to a target whose own service answers nothing
//! here exactly as it decides one it does, and it decides one to a target AWS
//! would otherwise govern by the target's own resource-based policy ("for Lambda
//! functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
//! resource-based policies") as well: a role the caller handed EventBridge is one
//! EventBridge has to be able to act as. What the role is ALLOWED to do is a
//! question about the call, so a target this emulator makes no call for earns no
//! verdict about permissions and no dead letter when its role is assumable, and
//! the kinds governed by the target's own policy earn none either.
//!
//! The table spans the class rather than one case: its rows differ along the
//! SERVICE the target belongs to — one whose invocation this emulator makes
//! (Step Functions), one it makes none for (Amazon ECS), one whose own service
//! is not mounted here (Amazon Data Firehose) and one reached under the target's
//! own policy (Amazon SQS) — along whether the role is assumable, and along
//! whether it carries any permissions at all. WHICH rule the assume is made for
//! is `target_role_trust_conditions`' table.
//!
//! The harness (the router with EventBridge, SQS, Step Functions, IAM and
//! CloudWatch on one engine, and the rule/queue/role fixtures) is the one
//! `target_dispatch` owns.

use serde_json::json;
use service_com_amazonaws_eventbridge_awsevents::state::InvocationOutcome;

use crate::target_dispatch::{
    drain, emulator, given_queue, given_role, given_rule, given_state_machine, publish, sfn,
    Emulator, REGION,
};

/// The rule every row's target belongs to, that target's id, and the name of the
/// role it names.
const RULE_NAME: &str = "assume-role";
const TARGET_ID: &str = "t1";
const ROLE_NAME: &str = "delivery-role";

/// The service principal a role must trust for EventBridge to act as it, and the
/// action it must trust that principal for — written as the LITERALS AWS
/// publishes rather than through the service's own constants, so a change to one
/// of those fails these rows.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const ASSUME_ROLE_ACTION: &str = "sts:AssumeRole";

/// A principal that is not this service: a role trusting only it cannot be
/// assumed by EventBridge.
const OTHER_PRINCIPAL: &str = "ec2.amazonaws.com";

/// The condition EventBridge stamps on an event it could not deliver because it
/// could not assume the target's role, as the literal it writes.
const FAILED_TO_ASSUME_ROLE: &str = "FAILED_TO_ASSUME_ROLE";

/// The service a row's target belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// An Amazon Data Firehose delivery stream: a target AWS invokes under the
    /// role the target names, whose own service is not mounted beside
    /// EventBridge here — so the assume-role step is reached and the call that
    /// would have followed it is not.
    DeliveryStream,
    /// An Amazon ECS task definition: another target AWS invokes under that
    /// role, with no invocation here either.
    EcsTask,
    /// A Step Functions state machine: a target AWS invokes under that role and
    /// this emulator does invoke.
    StateMachine,
    /// An Amazon SQS queue: a target whose own CALL is decided by the QUEUE's
    /// policy, and whose role is assumed all the same.
    Queue,
}

/// What a row expects of the handover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The delivery failed at the assume-role step: the event is on the
    /// target's dead-letter queue under that condition, and the target holds
    /// nothing.
    DeadLettered,
    /// Nothing was attempted: this emulator makes no call for the target, and a
    /// role it could have assumed refuses nothing — so there is no dead letter
    /// and no verdict about what that role may do.
    NotAttempted,
    /// The delivery went through: the role is assumable and nothing else refuses
    /// this target's call.
    Delivered,
}

/// One row: the target, the principal its role trusts, the action that role is
/// allowed (`None` for a role carrying no identity policy at all), and what
/// becomes of the handover.
struct Row {
    case: &'static str,
    target: Target,
    trusts: &'static str,
    allows: Option<&'static str>,
    expected: Expect,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a delivery stream under a role that does not trust this service",
            target: Target::DeliveryStream,
            trusts: OTHER_PRINCIPAL,
            allows: Some("*"),
            expected: Expect::DeadLettered,
        },
        Row {
            case: "an ECS task definition under a role that does not trust this service",
            target: Target::EcsTask,
            trusts: OTHER_PRINCIPAL,
            allows: Some("*"),
            expected: Expect::DeadLettered,
        },
        Row {
            case: "a state machine under a role that does not trust this service",
            target: Target::StateMachine,
            trusts: OTHER_PRINCIPAL,
            allows: Some("*"),
            expected: Expect::DeadLettered,
        },
        Row {
            case: "an ECS task definition under a role this service can assume",
            target: Target::EcsTask,
            trusts: EVENTS_PRINCIPAL,
            allows: Some("*"),
            expected: Expect::NotAttempted,
        },
        Row {
            // The role is assumable and allows nothing: the permission step is
            // about the call, and there is no call here to judge, so nothing is
            // refused and nothing is dead-lettered.
            case: "an ECS task definition under an assumable role carrying no permissions",
            target: Target::EcsTask,
            trusts: EVENTS_PRINCIPAL,
            allows: None,
            expected: Expect::NotAttempted,
        },
        Row {
            case: "a queue under a role that does not trust this service",
            target: Target::Queue,
            trusts: OTHER_PRINCIPAL,
            allows: Some("*"),
            expected: Expect::DeadLettered,
        },
        Row {
            // The queue's own policy grants the send and the role is assumable,
            // so the handover goes through: the role decides nothing further for
            // this kind.
            case: "a queue under a role this service can assume",
            target: Target::Queue,
            trusts: EVENTS_PRINCIPAL,
            allows: None,
            expected: Expect::Delivered,
        },
    ]
}

/// The target a row names: the ARN a rule names it by, and the url what it
/// received is read back from where it is a queue.
///
/// A target whose own API call this harness cannot serve — because no invocation
/// is implemented for its service, or because that service is not mounted beside
/// EventBridge here — is named by the ARN a client would name it by, with
/// nothing created for it: the assume-role step comes BEFORE the target is
/// reached, so what such a delivery fails at does not depend on the target being
/// there. The delivery that DOES reach a delivery stream, and the object its
/// destination then holds, is the cross-service suite
/// `events_rule_firehose_target_delivery`, where that service is mounted.
async fn given_target(emulator: &Emulator, target: Target) -> (String, Option<String>) {
    let account = engine::emulator_account_id();
    match target {
        Target::DeliveryStream => (
            format!("arn:aws:firehose:{REGION}:{account}:deliverystream/orders"),
            None,
        ),
        Target::EcsTask => (
            format!("arn:aws:ecs:{REGION}:{account}:task-definition/orders:1"),
            None,
        ),
        Target::StateMachine => (
            given_state_machine(emulator, "assume-role-flow").await,
            None,
        ),
        Target::Queue => {
            let (url, arn) = given_queue(emulator, "orders").await;
            (arn, Some(url))
        }
    }
}

/// How many executions the state machine `arn` holds — a state-machine target
/// that was never invoked holds none.
async fn executions(emulator: &Emulator, arn: &str) -> usize {
    let (status, body) = sfn(
        emulator,
        "ListExecutions",
        &json!({ "stateMachineArn": arn }),
    )
    .await;
    assert_eq!(status, 200, "ListExecutions answered {status}: {body}");
    body["executions"].as_array().map_or(0, Vec::len)
}

#[tokio::test]
async fn a_role_this_service_cannot_assume_dead_letters_the_event_whatever_the_target_is() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        let (target_arn, received) = given_target(&emulator, row.target).await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;
        let role_arn = given_role(&emulator, ROLE_NAME, row.trusts, row.allows).await;
        // The assume is made FOR the invoking rule, which the refusal names: the
        // ARN EventBridge mints for a rule of the default bus.
        let rule_arn = format!(
            "arn:aws:events:{REGION}:{}:rule/{RULE_NAME}",
            engine::emulator_account_id()
        );
        given_rule(
            &emulator,
            RULE_NAME,
            json!([{
                "Id": TARGET_ID,
                "Arn": target_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }]),
        )
        .await;

        publish(&emulator).await;

        let recorded = invocation(&emulator, case);
        match row.expected {
            Expect::DeadLettered => {
                let InvocationOutcome::Failed { code, message } = &recorded else {
                    panic!("case {case:?}: expected a failed invocation, got {recorded:?}");
                };
                assert_eq!(
                    code, FAILED_TO_ASSUME_ROLE,
                    "case {case:?}: expected the condition {FAILED_TO_ASSUME_ROLE}, got {code:?}"
                );
                let expected_message = format!(
                    "Event bus failed to assume role {role_arn}: its trust policy does not let \
                     {EVENTS_PRINCIPAL} perform {ASSUME_ROLE_ACTION} for {rule_arn}."
                );
                assert_eq!(
                    *message, expected_message,
                    "case {case:?}: expected the sentence {expected_message:?}, got {message:?}"
                );
            }
            Expect::NotAttempted => assert!(
                matches!(recorded, InvocationOutcome::NotAttempted(_)),
                "case {case:?}: expected a handover nobody attempted, got {recorded:?}"
            ),
            Expect::Delivered => assert!(
                matches!(recorded, InvocationOutcome::Invoked),
                "case {case:?}: expected the delivery to go through, got {recorded:?}"
            ),
        }

        // The dead letter: one message for a delivery that failed, none for a
        // handover nobody attempted and none for one that went through.
        let dead_letters = drain(&emulator, &dead_letter_url).await;
        let expected_letters = usize::from(row.expected == Expect::DeadLettered);
        assert_eq!(
            dead_letters.len(),
            expected_letters,
            "case {case:?}: expected {expected_letters} dead letter(s), got {dead_letters:?}"
        );
        if let Some(letter) = dead_letters.first() {
            let attributes = &letter["MessageAttributes"];
            assert_eq!(
                attributes["ERROR_CODE"]["StringValue"],
                json!(FAILED_TO_ASSUME_ROLE),
                "case {case:?}: expected the dead-letter ERROR_CODE \
                 {FAILED_TO_ASSUME_ROLE}, got {letter:?}"
            );
            assert_eq!(
                attributes["TARGET_ARN"]["StringValue"],
                json!(target_arn),
                "case {case:?}: expected the dead-letter TARGET_ARN {target_arn:?}, \
                 got {letter:?}"
            );
        }

        // What the TARGET holds: the delivery the role governs nothing for
        // arrived, and the one that failed at the assume-role step reached no
        // target at all.
        if let Some(url) = received {
            let delivered = drain(&emulator, &url).await;
            let expected_messages = usize::from(row.expected == Expect::Delivered);
            assert_eq!(
                delivered.len(),
                expected_messages,
                "case {case:?}: expected {expected_messages} message(s) on the target queue, \
                 got {delivered:?}"
            );
        }
        if row.target == Target::StateMachine {
            let started = executions(&emulator, &target_arn).await;
            let expected_executions = usize::from(row.expected == Expect::Delivered);
            assert_eq!(
                started, expected_executions,
                "case {case:?}: expected {expected_executions} execution(s) of the target, \
                 got {started}"
            );
        }
    }
}

/// The ONE handover EventBridge recorded for the row's publication.
fn invocation(emulator: &Emulator, case: &str) -> InvocationOutcome {
    let recorded = crate::target_dispatch::invocations(emulator);
    assert_eq!(
        recorded.len(),
        1,
        "case {case:?}: expected one recorded handover, got {recorded:?}"
    );
    recorded
        .into_iter()
        .next()
        .map(|(_, _, outcome)| outcome)
        .unwrap_or_else(|| panic!("case {case:?}: nothing was recorded"))
}
