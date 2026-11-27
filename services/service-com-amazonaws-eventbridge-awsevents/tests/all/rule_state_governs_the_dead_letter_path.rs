// SPDX-License-Identifier: CC0-1.0

//! A rule's `State` governs the WHOLE target pipeline, the dead-letter leg
//! included, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`
//! with Amazon SQS on the same engine.
//!
//! The RULE asserted here: "the state of the rule" (`DescribeRule$State`,
//! `DisableRule`: "disables the specified rule. A disabled rule won't match any
//! events") decides whether an event is matched AT ALL, so a rule that is not
//! `ENABLED` attempts no invocation — and a target that cannot be delivered to
//! parks nothing on its `DeadLetterConfig` queue either, because there was no
//! failed invocation to park. The same rule, enabled, decides every matching
//! event from then on: the next one is handed to the target, and a handover the
//! target's own documents refuse is dead-lettered. The switch is read PER EVENT,
//! so the same rule can be disabled and enabled again and each publication
//! follows the state standing at that instant.
//!
//! The table spans the class rather than one recorded case: the rows differ along
//! how the rule came to be in each state (created `DISABLED`, created `ENABLED`
//! and then disabled, disabled and then enabled again) and along whether the
//! target's own policy admits the delivery — which is what tells "disabled,
//! correctly silent" from "enabled but broken", since the enabled leg of every
//! row is observed as well.
//!
//! The harness (the router with EventBridge and Amazon SQS on one engine) is
//! `tests/all/target_dispatch.rs`'s.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    DEFAULT_RULE_STATE, NO_RESOURCE, RULE_STATE_DISABLED, RULE_STATE_ENABLED,
};

use crate::target_dispatch::{
    answered_request_id, drain, emulator, events, given_queue, given_queue_with_policy,
    not_disclosed_answer_head, Emulator, DETAIL_TYPE, SOURCE,
};

/// The dead-letter attributes naming the delivery that failed and why.
const RULE_ARN_ATTRIBUTE: &str = "RULE_ARN";
const TARGET_ARN_ATTRIBUTE: &str = "TARGET_ARN";
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";

/// The attributes a RETRIED delivery's dead letter carries beside those, which a
/// refusal nothing retried carries neither of — read from the one place this
/// service names them (`crate::state::retry_bounds`).
const RETRY_ATTEMPTS_ATTRIBUTE: &str =
    service_com_amazonaws_eventbridge_awsevents::state::retry_bounds::RETRY_ATTEMPTS;
const EXHAUSTED_RETRY_CONDITION_ATTRIBUTE: &str =
    service_com_amazonaws_eventbridge_awsevents::state::retry_bounds::EXHAUSTED_RETRY_CONDITION;

/// What a dead letter of a send the target queue's own `Policy` withheld opens
/// with: the answer Amazon SQS gives a caller it will not disclose the queue to — its
/// not-found code at HTTP 400 under the fixed sentence that states the absence and
/// the want of access without saying which holds.
fn withheld_send_answer() -> String {
    not_disclosed_answer_head()
}

/// The rule every row below publishes through.
const RULE: &str = "state-governed-rule";

/// The detail every publication below carries.
const DETAIL: &str = r#"{"orderId":"S-9"}"#;

/// Publish the one matching event onto `bus` — the default bus when `None`.
async fn publish_on(emulator: &Emulator, case: &str, bus: Option<&str>) {
    let mut entry = json!({ "Source": SOURCE, "DetailType": DETAIL_TYPE, "Detail": DETAIL });
    if let Some(bus) = bus {
        entry["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": [entry] })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutEvents answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutEvents reported a failed entry: {body}"
    );
}

/// Create the rule `RULE` in `state` (omitting `State` entirely when `None`) with
/// one queue target carrying a dead-letter queue, and answer the rule's ARN.
async fn given_rule_in_state(
    emulator: &Emulator,
    case: &str,
    state: Option<&str>,
    bus: Option<&str>,
    target_arn: &str,
    dead_letter_arn: &str,
) -> String {
    if let Some(bus) = bus {
        let (status, body) = events(emulator, "CreateEventBus", &json!({ "Name": bus })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEventBus answered {status} {body}"
        );
    }
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let mut request = json!({ "Name": RULE, "EventPattern": pattern });
    if let Some(state) = state {
        request["State"] = json!(state);
    }
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "PutRule", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRule answered {status} {body}"
    );
    let rule_arn = body["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no RuleArn in {body}"))
        .to_string();
    let mut put_targets = json!({
        "Rule": RULE,
        "Targets": [{
            "Id": "queue",
            "Arn": target_arn,
            "DeadLetterConfig": { "Arn": dead_letter_arn },
            // A target that DOES allow retries, so a dead letter carrying no
            // retry accounting says the refusal was not retried rather than that
            // the target asked for no retries.
            "RetryPolicy": { "MaximumRetryAttempts": 2, "MaximumEventAgeInSeconds": 300 },
        }],
    });
    if let Some(bus) = bus {
        put_targets["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "PutTargets", &put_targets).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets reported a failed entry: {body}"
    );
    rule_arn
}

/// Call `operation` (`DisableRule`/`EnableRule`) for the rule under test.
async fn state_call(emulator: &Emulator, case: &str, operation: &str, bus: Option<&str>) {
    let mut request = json!({ "Name": RULE });
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} answered {status} {body}"
    );
}

/// The `State` `DescribeRule` reports for the rule under test.
async fn rule_state(emulator: &Emulator, case: &str, bus: Option<&str>) -> String {
    let mut request = json!({ "Name": RULE });
    if let Some(bus) = bus {
        request["EventBusName"] = json!(bus);
    }
    let (status, body) = events(emulator, "DescribeRule", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRule answered {status} {body}"
    );
    body["State"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no State in {body}"))
        .to_string()
}

/// The `StringValue` of one message attribute.
fn attribute(message: &Value, name: &str) -> String {
    message["MessageAttributes"][name]["StringValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// How a row's rule reaches the state its publication is decided under.
#[derive(Clone, Copy, Debug)]
enum Reached {
    /// `PutRule` stated it.
    Created(Option<&'static str>),
    /// `PutRule` stated the first, and then this call was made.
    Switched(Option<&'static str>, &'static str),
}

/// One row: how the rule reached its state, the bus it lives on, the state it
/// then reads back in, and whether a publication under it is matched at all.
struct Row {
    case: &'static str,
    reached: Reached,
    /// `None` is the default bus.
    bus: Option<&'static str>,
    reads_back: &'static str,
    matches: bool,
}

/// A rule that is not ENABLED matches nothing, so neither the target nor its
/// dead-letter queue receives anything; the same rule enabled delivers the next
/// matching event, and dead-letters it when the target's own policy withholds it.
#[tokio::test]
async fn a_rule_that_is_not_enabled_neither_delivers_nor_dead_letters() {
    let rows = [
        Row {
            case: "a rule created DISABLED",
            reached: Reached::Created(Some(RULE_STATE_DISABLED)),
            bus: None,
            reads_back: RULE_STATE_DISABLED,
            matches: false,
        },
        Row {
            case: "a rule created ENABLED and then disabled",
            reached: Reached::Switched(Some(RULE_STATE_ENABLED), "DisableRule"),
            bus: None,
            reads_back: RULE_STATE_DISABLED,
            matches: false,
        },
        Row {
            case: "a rule created with no State at all and then disabled",
            reached: Reached::Switched(None, "DisableRule"),
            bus: None,
            reads_back: RULE_STATE_DISABLED,
            matches: false,
        },
        Row {
            case: "a custom bus, a rule created ENABLED and then disabled",
            reached: Reached::Switched(Some(RULE_STATE_ENABLED), "DisableRule"),
            bus: Some("paused.bus"),
            reads_back: RULE_STATE_DISABLED,
            matches: false,
        },
        Row {
            case: "a rule created DISABLED and then enabled",
            reached: Reached::Switched(Some(RULE_STATE_DISABLED), "EnableRule"),
            bus: None,
            reads_back: RULE_STATE_ENABLED,
            matches: true,
        },
        Row {
            case: "a custom bus, a rule created DISABLED and then enabled",
            reached: Reached::Switched(Some(RULE_STATE_DISABLED), "EnableRule"),
            bus: Some("resumed.bus"),
            reads_back: RULE_STATE_ENABLED,
            matches: true,
        },
        Row {
            case: "a rule created with no State at all",
            reached: Reached::Created(None),
            bus: None,
            reads_back: DEFAULT_RULE_STATE,
            matches: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // Two targets' worth of documents in every row: a queue that grants this
        // service the send (so the enabled leg is observed as a delivery) and one
        // that grants nothing (so the enabled leg is observed as a dead letter).
        for granted in [true, false] {
            let case = row.case;
            let emulator = emulator();
            let target = format!("state-target-{index}-{granted}");
            let (target_url, target_arn) = if granted {
                given_queue(&emulator, &target).await
            } else {
                given_queue_with_policy(&emulator, &target, None).await
            };
            let (dlq_url, dlq_arn) =
                given_queue(&emulator, &format!("state-dlq-{index}-{granted}")).await;
            let state = match row.reached {
                Reached::Created(state) | Reached::Switched(state, _) => state,
            };
            let rule_arn =
                given_rule_in_state(&emulator, case, state, row.bus, &target_arn, &dlq_arn).await;
            if let Reached::Switched(_, operation) = row.reached {
                state_call(&emulator, case, operation, row.bus).await;
            }
            let reads_back = rule_state(&emulator, case, row.bus).await;
            assert_eq!(
                reads_back, row.reads_back,
                "case {case:?} (granted {granted}): expected the rule to read back \
                 {:?}, got {reads_back:?}",
                row.reads_back
            );

            publish_on(&emulator, case, row.bus).await;
            let delivered = drain(&emulator, &target_url).await;
            let dead_lettered = drain(&emulator, &dlq_url).await;
            let expect_delivered = usize::from(row.matches && granted);
            let expect_dead_lettered = usize::from(row.matches && !granted);
            assert_eq!(
                delivered.len(),
                expect_delivered,
                "case {case:?} (granted {granted}): expected {expect_delivered} message(s) on \
                 the target queue, got {delivered:?}"
            );
            assert_eq!(
                dead_lettered.len(),
                expect_dead_lettered,
                "case {case:?} (granted {granted}): expected {expect_dead_lettered} dead \
                 letter(s), got {dead_lettered:?}"
            );
            for message in &dead_lettered {
                assert_eq!(
                    attribute(message, RULE_ARN_ATTRIBUTE),
                    rule_arn,
                    "case {case:?} (granted {granted}): expected the dead letter to name the \
                     rule, got {message:?}"
                );
                assert_eq!(
                    attribute(message, TARGET_ARN_ATTRIBUTE),
                    target_arn,
                    "case {case:?} (granted {granted}): expected the dead letter to name the \
                     target, got {message:?}"
                );
                assert_eq!(
                    attribute(message, ERROR_CODE_ATTRIBUTE),
                    NO_RESOURCE,
                    "case {case:?} (granted {granted}): expected the class of the answer that \
                     service gave a send the queue's own document withheld, got {message:?}"
                );
                let reported = attribute(message, ERROR_MESSAGE_ATTRIBUTE);
                let head = withheld_send_answer();
                assert!(
                    answered_request_id(&reported, &head).is_some(),
                    "case {case:?} (granted {granted}): expected {head:?} then \
                     the id of the refused call, got {reported:?}"
                );
                // Nothing retried this handover — a send the destination's own
                // documents refuse answers the same way however often it is made —
                // so the letter accounts for no retry, whatever the target's
                // `RetryPolicy` allows.
                for retried in [RETRY_ATTEMPTS_ATTRIBUTE, EXHAUSTED_RETRY_CONDITION_ATTRIBUTE] {
                    assert_eq!(
                        message["MessageAttributes"][retried],
                        Value::Null,
                        "case {case:?} (granted {granted}): a refusal nothing retried carries no \
                         {retried}, got {message:?}"
                    );
                }
            }
        }
    }
}

/// The switch is read PER EVENT: the same rule, disabled and enabled again,
/// decides each publication under the state standing at that instant — and the
/// dead-letter queue stays empty for every publication made while it was
/// disabled.
#[tokio::test]
async fn each_publication_follows_the_state_standing_when_it_is_made() {
    let case = "a rule disabled and enabled again, with a target granting nothing";
    let emulator = emulator();
    // The target grants this service nothing, so every handover it IS given fails
    // and lands on the dead-letter queue: the dead letter is then the only
    // observable of "an invocation was attempted".
    let (target_url, target_arn) =
        given_queue_with_policy(&emulator, "switched-target", None).await;
    let (dlq_url, dlq_arn) = given_queue(&emulator, "switched-dlq").await;
    // A custom bus, which is where a client that keeps its own events off the
    // default bus puts such a rule.
    let bus = Some("switched.bus");
    given_rule_in_state(
        &emulator,
        case,
        Some(RULE_STATE_ENABLED),
        bus,
        &target_arn,
        &dlq_arn,
    )
    .await;

    // Enabled: the handover is attempted, refused and parked.
    publish_on(&emulator, case, bus).await;
    let parked = drain(&emulator, &dlq_url).await;
    assert_eq!(
        parked.len(),
        1,
        "case {case:?}: expected the enabled rule to park one dead letter, got {parked:?}"
    );

    // Disabled: nothing is matched, so nothing is attempted and nothing parked.
    state_call(&emulator, case, "DisableRule", bus).await;
    publish_on(&emulator, case, bus).await;
    publish_on(&emulator, case, bus).await;
    let while_disabled = drain(&emulator, &dlq_url).await;
    assert!(
        while_disabled.is_empty(),
        "case {case:?}: a disabled rule attempts no invocation, so it parks no dead letter, \
         got {while_disabled:?}"
    );

    // Enabled again: the next publication is parked once more.
    state_call(&emulator, case, "EnableRule", bus).await;
    publish_on(&emulator, case, bus).await;
    let after = drain(&emulator, &dlq_url).await;
    assert_eq!(
        after.len(),
        1,
        "case {case:?}: expected the re-enabled rule to park the next event, got {after:?}"
    );
    let never_delivered = drain(&emulator, &target_url).await;
    assert!(
        never_delivered.is_empty(),
        "case {case:?}: the target granted nothing, so it receives nothing in any state, got \
         {never_delivered:?}"
    );
}
