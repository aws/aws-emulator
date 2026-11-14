// SPDX-License-Identifier: CC0-1.0

//! The role a rule's target names is assumed FOR THE INVOKING RULE, so a trust
//! policy's own `Condition` decides which rules may deliver under it.
//!
//! The RULE asserted here: EventBridge assumes a target's `RoleArn` carrying the
//! condition keys AWS publishes for a service acting on a resource's behalf —
//! `aws:SourceArn`, whose value is the ARN of the RULE whose target is being
//! delivered to, and `aws:SourceAccount`, that rule's account. A statement's
//! `Condition` is part of its grant, so a trust document that grants the assume
//! only where a clause holds grants it for the rules that satisfy the clause and
//! for no others, and a clause the assume carries no value for (`sts:ExternalId`,
//! which EventBridge sends none of) is no grant at all. An assume that is no grant
//! fails the delivery at that step: nothing is handed to the target and the event
//! is written to the target's `DeadLetterConfig` queue under the condition of the
//! step that failed.
//!
//! The table spans what the rule generalises over: the OPERATOR a clause is
//! written under (`ArnEquals`, `ArnLike`, `StringEquals`), the KEY it is written
//! over (the invoking rule, its account, an external id this assume does not
//! carry), whether the clause is satisfied by the rule that is invoking, and a
//! document with no condition at all. Every row fires ONE event matched by TWO
//! rules sharing the role — the rule the condition names and another that does not
//! — so each row states the verdict for both, and no row can pass by refusing or
//! by delivering everything.
//!
//! The harness (the router with EventBridge, SQS, IAM and CloudWatch on one
//! engine, and the queue fixtures) is the one `target_dispatch` owns.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::InvocationOutcome;

use crate::target_dispatch::{
    drain, emulator, events, given_queue, iam, publish, xml_text, Emulator, DETAIL_TYPE, SOURCE,
};

/// The two rules every row installs: the one the trust condition is written about,
/// and the one it is not.
const NAMED_RULE: &str = "scoped-rule";
const OTHER_RULE: &str = "other-rule";

/// The role both rules' targets name.
const ROLE_NAME: &str = "scoped-delivery-role";

/// The service principal a role must trust for EventBridge to act as it, written
/// as the literal AWS publishes rather than through the service's own constant.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The condition EventBridge stamps on an event it could not deliver because it
/// could not assume the target's role, as the literal it writes.
const FAILED_TO_ASSUME_ROLE: &str = "FAILED_TO_ASSUME_ROLE";

/// What a row expects of one rule's handover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The assume was a grant for this rule, so the target received the event.
    Delivered,
    /// The assume was no grant for this rule: the delivery failed at that step and
    /// the event is on the dead-letter queue.
    DeadLettered,
}

/// One row: how the trust document's condition is written, and what becomes of
/// each of the two rules' handovers under it.
struct Row {
    case: &'static str,
    /// The `Condition` element of the trust statement, built from the ARN of the
    /// rule the row names — `None` for a statement carrying no condition.
    condition: fn(&str) -> Option<Value>,
    named: Expect,
    other: Expect,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no condition at all",
            condition: |_| None,
            named: Expect::Delivered,
            other: Expect::Delivered,
        },
        Row {
            case: "ArnEquals on the invoking rule",
            condition: |rule_arn| {
                Some(json!({ "ArnEquals": { "aws:SourceArn": rule_arn } }))
            },
            named: Expect::Delivered,
            other: Expect::DeadLettered,
        },
        Row {
            case: "StringEquals on the invoking rule",
            condition: |rule_arn| {
                Some(json!({ "StringEquals": { "aws:SourceArn": rule_arn } }))
            },
            named: Expect::Delivered,
            other: Expect::DeadLettered,
        },
        Row {
            case: "ArnLike over every rule of the account",
            condition: |rule_arn| {
                // The same ARN with its last segment replaced by a wildcard, so the
                // clause covers both rules rather than one.
                let prefix = rule_arn
                    .rsplit_once('/')
                    .map(|(head, _)| head.to_string())
                    .unwrap_or_else(|| rule_arn.to_string());
                Some(json!({ "ArnLike": { "aws:SourceArn": format!("{prefix}/*") } }))
            },
            named: Expect::Delivered,
            other: Expect::Delivered,
        },
        Row {
            case: "StringEquals on the account the rules belong to",
            condition: |_| {
                Some(json!({
                    "StringEquals": { "aws:SourceAccount": engine::emulator_account_id() },
                }))
            },
            named: Expect::Delivered,
            other: Expect::Delivered,
        },
        Row {
            case: "StringEquals on another account",
            condition: |_| {
                Some(json!({ "StringEquals": { "aws:SourceAccount": "210987654321" } }))
            },
            named: Expect::DeadLettered,
            other: Expect::DeadLettered,
        },
        Row {
            // A key this assume carries no value for: the grant cannot be shown to
            // cover the request, so it covers neither rule.
            case: "StringEquals on an external id this assume does not carry",
            condition: |_| {
                Some(json!({ "StringEquals": { "sts:ExternalId": "shared-secret" } }))
            },
            named: Expect::DeadLettered,
            other: Expect::DeadLettered,
        },
    ]
}

/// A trust document granting [`EVENTS_PRINCIPAL`] the assume, under `condition`
/// when the row states one.
fn trust_document(condition: Option<Value>) -> String {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": { "Service": EVENTS_PRINCIPAL },
        "Action": "sts:AssumeRole",
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string()
}

/// The role every row's targets name, trusting this service under `condition`;
/// answered with the ARN IAM persisted.
async fn given_conditioned_role(emulator: &Emulator, case: &str, condition: Option<Value>) -> String {
    let document = trust_document(condition);
    let (status, created) = iam(
        emulator,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("AssumeRolePolicyDocument", &document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status}: {created}"
    );
    xml_text(&created, "Arn").unwrap_or_else(|| panic!("case {case:?}: no Arn in {created}"))
}

/// A rule called `name` matching the published event, answered with the ARN
/// `PutRule` reports for it — which is the value a trust condition is written
/// about.
async fn given_rule_arn(emulator: &Emulator, case: &str, name: &str) -> String {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule({name}) answered {status} {body}"
    );
    body["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RuleArn for {name} in {body}"))
        .to_string()
}

/// Install `name`'s target: the queue `target_arn`, under `role_arn`, with
/// `dead_letter_arn` as its dead-letter queue.
async fn given_target(
    emulator: &Emulator,
    case: &str,
    name: &str,
    target_arn: &str,
    role_arn: &str,
    dead_letter_arn: &str,
) {
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({
            "Rule": name,
            "Targets": [{
                "Id": "t1",
                "Arn": target_arn,
                "RoleArn": role_arn,
                "DeadLetterConfig": { "Arn": dead_letter_arn },
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets({name}) answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets({name}) reported a failed entry: {body}"
    );
}

/// What EventBridge recorded for `rule`'s one handover.
fn outcome_of(emulator: &Emulator, case: &str, rule: &str) -> InvocationOutcome {
    let recorded = crate::target_dispatch::invocations(emulator);
    recorded
        .into_iter()
        .find(|(recorded_rule, _, _)| recorded_rule == rule)
        .map(|(_, _, outcome)| outcome)
        .unwrap_or_else(|| panic!("case {case:?}: nothing was recorded for {rule}"))
}

/// Assert `rule`'s handover went the way `expected` says, from what EventBridge
/// recorded and from what the target queue holds.
async fn assert_handover(
    emulator: &Emulator,
    case: &str,
    rule: &str,
    queue_url: &str,
    expected: Expect,
) {
    let recorded = outcome_of(emulator, case, rule);
    match expected {
        Expect::Delivered => assert!(
            matches!(recorded, InvocationOutcome::Invoked),
            "case {case:?}: expected {rule}'s delivery to go through, got {recorded:?}"
        ),
        Expect::DeadLettered => {
            let InvocationOutcome::Failed { code, .. } = &recorded else {
                panic!("case {case:?}: expected {rule}'s delivery to fail, got {recorded:?}");
            };
            assert_eq!(
                code, FAILED_TO_ASSUME_ROLE,
                "case {case:?}: expected {rule} to fail at the assume-role step with \
                 {FAILED_TO_ASSUME_ROLE}, got {code:?}"
            );
        }
    }
    let held = drain(emulator, queue_url).await;
    let expected_messages = usize::from(expected == Expect::Delivered);
    assert_eq!(
        held.len(),
        expected_messages,
        "case {case:?}: expected {expected_messages} message(s) on {rule}'s target, got {held:?}"
    );
}

#[tokio::test]
async fn a_trust_condition_decides_which_rule_may_deliver_under_the_role() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        // The rule the condition is written about has to exist before the role,
        // because its ARN is what the clause names.
        let named_arn = given_rule_arn(&emulator, case, NAMED_RULE).await;
        let other_arn = given_rule_arn(&emulator, case, OTHER_RULE).await;
        assert_ne!(
            named_arn, other_arn,
            "case {case:?}: the two rules must be addressed by different ARNs"
        );
        let role_arn = given_conditioned_role(&emulator, case, (row.condition)(&named_arn)).await;
        let (named_url, named_target) = given_queue(&emulator, "named-target").await;
        let (other_url, other_target) = given_queue(&emulator, "other-target").await;
        let (dead_letter_url, dead_letter_arn) = given_queue(&emulator, "dead-letters").await;
        given_target(
            &emulator,
            case,
            NAMED_RULE,
            &named_target,
            &role_arn,
            &dead_letter_arn,
        )
        .await;
        given_target(
            &emulator,
            case,
            OTHER_RULE,
            &other_target,
            &role_arn,
            &dead_letter_arn,
        )
        .await;

        publish(&emulator).await;

        assert_handover(&emulator, case, NAMED_RULE, &named_url, row.named).await;
        assert_handover(&emulator, case, OTHER_RULE, &other_url, row.other).await;

        // One dead letter per handover the assume refused, carrying the condition of
        // the step that failed.
        let dead_letters = drain(&emulator, &dead_letter_url).await;
        let expected_letters = usize::from(row.named == Expect::DeadLettered)
            + usize::from(row.other == Expect::DeadLettered);
        assert_eq!(
            dead_letters.len(),
            expected_letters,
            "case {case:?}: expected {expected_letters} dead letter(s), got {dead_letters:?}"
        );
        for letter in &dead_letters {
            assert_eq!(
                letter["MessageAttributes"]["ERROR_CODE"]["StringValue"],
                json!(FAILED_TO_ASSUME_ROLE),
                "case {case:?}: expected the dead-letter ERROR_CODE {FAILED_TO_ASSUME_ROLE}, \
                 got {letter:?}"
            );
        }
    }
}
