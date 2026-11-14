// SPDX-License-Identifier: CC0-1.0

//! A rule whose target is a FIFO (or fair) Amazon SQS queue, end to end over the wire
//! through the real `MultiServiceRouter` with SQS mounted beside EventBridge.
//!
//! The RULE asserted here: `Target$SqsParameters` "contains the message group ID to use
//! when the target is an Amazon SQS fair or FIFO queue", so the `MessageGroupId` the target
//! names travels with the `sqs:SendMessage` each matched event is delivered by — which is
//! what "if you do not provide a MessageGroupId when sending a message to a FIFO queue, the
//! action fails" (Amazon SQS `SendMessage$MessageGroupId`) requires of every delivery to a
//! FIFO queue. EventBridge sends no
//! deduplication id with it: "if you specify a fair or FIFO queue as a target, the queue
//! must have content-based deduplication enabled", so the queue derives one from the body.
//!
//! The rows span the class rather than one case: they differ along the KIND of queue the
//! target names (standard, FIFO), along whether the target carries `SqsParameters` at all
//! (a standard queue takes a delivery with no group),
//! along the group id it names (two different groups, so what arrives is keyed on the
//! target's own value rather than on a constant), and along how many entries one request
//! publishes into one group (one, and three — whose order inside the group is what a FIFO
//! queue adds).
//!
//! A FIFO target naming NO message group is not among them: EventBridge refuses to register
//! one at all, so there is no such target to deliver to — that refusal is
//! `tests/all/fifo_queue_target_parameters.rs`.
//!
//! The harness (the router with EventBridge, SQS, IAM and CloudWatch on one engine) is the
//! one `target_dispatch` owns.

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::InvocationOutcome;

use crate::target_dispatch::{emulator, events, invocations, sqs, Emulator, REGION};

/// The source and detail type the rule below matches on.
const SOURCE: &str = "com.demo.orders";
const DETAIL_TYPE: &str = "Order Created";

/// The system attribute a received message reports its group under, from the
/// `MessageSystemAttributeName` enum of the Amazon SQS model.
const MESSAGE_GROUP_ID: &str = "MessageGroupId";

/// The two groups the rows name, so what a delivery carries is the target's own value.
const GROUP: &str = "orders";
const OTHER_GROUP: &str = "tenant-b";

/// The rule and target id every row writes.
const RULE: &str = "to-the-queue";
const TARGET_ID: &str = "t1";

/// Whether a row's queue is a FIFO queue, and the suffix its name must carry: "the name of
/// a FIFO queue must end with the .fifo suffix" (`CreateQueue$QueueName`).
const FIFO_SUFFIX: &str = ".fifo";

/// Create the queue `name`, as a FIFO queue when `fifo`, and answer its url and ARN.
///
/// A FIFO queue is created with `ContentBasedDeduplication` enabled, which is the
/// prerequisite `Target$SqsParameters` documents for a queue named as a target, and
/// with the policy a rule's target needs: EventBridge "relies on resource-based
/// policies" for a queue (`PutTargets`), so one that has granted the service nothing
/// receives nothing — the rule
/// `tests/all/queue_target_policy_authorization.rs` is the suite for.
async fn given_queue(emulator: &Emulator, case: &str, name: &str, fifo: bool) -> (String, String) {
    let attributes = if fifo {
        json!({ "FifoQueue": "true", "ContentBasedDeduplication": "true" })
    } else {
        json!({})
    };
    let (status, body) = sqs(
        emulator,
        "CreateQueue",
        &json!({ "QueueName": name, "Attributes": attributes }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateQueue {name} answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl in {body}"))
        .to_string();
    let arn = format!("arn:aws:sqs:{REGION}:{}:{name}", engine::emulator_account_id());
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "Policy": crate::target_dispatch::granting_events(&arn) },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes {name} answered {status} {body}"
    );
    (url, arn)
}

/// A rule matching the published events, carrying `target_arn` with `message_group_id` as
/// its `SqsParameters` when the row names one.
async fn given_rule(emulator: &Emulator, case: &str, target_arn: &str, group: Option<&str>) {
    let pattern = json!({ "source": [SOURCE], "detail-type": [DETAIL_TYPE] }).to_string();
    let (status, body) = events(
        emulator,
        "PutRule",
        &json!({ "Name": RULE, "EventPattern": pattern, "State": "ENABLED" }),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: PutRule answered {status} {body}");
    let mut target = json!({ "Id": TARGET_ID, "Arn": target_arn });
    if let Some(group) = group {
        target["SqsParameters"] = json!({ "MessageGroupId": group });
    }
    let (status, body) = events(
        emulator,
        "PutTargets",
        &json!({ "Rule": RULE, "Targets": [target] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutTargets answered {status} {body}"
    );
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "case {case:?}: PutTargets reported a failed entry: {body}"
    );
}

/// Publish one entry per marker in ONE request, and assert each was accepted.
async fn publish(emulator: &Emulator, case: &str, markers: &[&str]) {
    let entries: Vec<Value> = markers
        .iter()
        .map(|marker| {
            json!({
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": json!({ "marker": marker }).to_string(),
            })
        })
        .collect();
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": entries })).await;
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

/// What arrived on the queue at `url`: the marker of each delivered event and the message
/// group the message was sent under, in the order the queue hands them over.
async fn received(emulator: &Emulator, case: &str, url: &str) -> Vec<(String, String)> {
    let (status, body) = sqs(
        emulator,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageSystemAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            let text = message["Body"].as_str().unwrap_or_default();
            let envelope: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivered body is not the event envelope ({error}): {text:?}")
            });
            let marker = envelope["detail"]["marker"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let group = message["Attributes"][MESSAGE_GROUP_ID]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (marker, group)
        })
        .collect()
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// Whether the target queue is a FIFO queue.
    fifo: bool,
    /// The `SqsParameters.MessageGroupId` the target names, or `None` for a target
    /// carrying no `SqsParameters` at all.
    group: Option<&'static str>,
    /// The markers the one request publishes, in order.
    markers: &'static [&'static str],
    /// The group every delivered message must carry — empty for a message sent under none.
    delivered_group: &'static str,
}

#[tokio::test]
async fn a_queue_targets_message_group_travels_with_every_delivery() {
    let rows = [
        Row {
            case: "a standard queue target carrying no SqsParameters",
            fifo: false,
            group: None,
            markers: &["m1"],
            delivered_group: "",
        },
        Row {
            // "In standard queues, using MessageGroupId enables fair queues", where it
            // identifies a tenant and orders nothing — so a standard queue takes one too.
            case: "a standard queue target naming a message group",
            fifo: false,
            group: Some(GROUP),
            markers: &["m1"],
            delivered_group: GROUP,
        },
        Row {
            case: "a FIFO queue target naming a message group",
            fifo: true,
            group: Some(GROUP),
            markers: &["m1"],
            delivered_group: GROUP,
        },
        Row {
            case: "a FIFO queue target naming another message group",
            fifo: true,
            group: Some(OTHER_GROUP),
            markers: &["m1"],
            delivered_group: OTHER_GROUP,
        },
        Row {
            case: "three entries of one request into one group of a FIFO queue",
            fifo: true,
            group: Some(GROUP),
            markers: &["m1", "m2", "m3"],
            delivered_group: GROUP,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let emulator = emulator();
        let name = if row.fifo {
            format!("target-{index}{FIFO_SUFFIX}")
        } else {
            format!("target-{index}")
        };
        let (url, arn) = given_queue(&emulator, case, &name, row.fifo).await;
        given_rule(&emulator, case, &arn, row.group).await;

        publish(&emulator, case, row.markers).await;

        let arrived = received(&emulator, case, &url).await;
        let recorded = invocations(&emulator);
        // One handover per published entry, whichever way each one went: a row whose rule
        // stopped matching would otherwise pass on the ABSENCE of the behaviour under
        // test rather than on the delivery it asserts.
        assert_eq!(
            recorded.len(),
            row.markers.len(),
            "case {case:?}: expected {} recorded handover(s), got {recorded:?}",
            row.markers.len()
        );
        let expected: Vec<(String, String)> = row
            .markers
            .iter()
            .map(|marker| ((*marker).to_string(), row.delivered_group.to_string()))
            .collect();
        assert_eq!(
            arrived, expected,
            "case {case:?}: expected {expected:?} on the queue, got {arrived:?}"
        );
        for (_, _, outcome) in &recorded {
            assert_eq!(
                *outcome,
                InvocationOutcome::Invoked,
                "case {case:?}: the handover must be recorded as invoked, got {recorded:?}"
            );
        }
    }
}
