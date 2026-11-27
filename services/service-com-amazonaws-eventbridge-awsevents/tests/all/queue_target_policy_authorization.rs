// SPDX-License-Identifier: CC0-1.0

//! The QUEUE's own resource policy decides whether a rule's SQS target receives a
//! matched event, end to end over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: AWSEvents.<Operation>`) through the real `MultiServiceRouter`, with
//! Amazon SQS and CloudWatch mounted on the same engine so a delivery is asserted
//! where a client observes one: on the queue, and on the rule's invocation metrics.
//!
//! The RULE asserted here: "for Lambda functions, Amazon SNS topics and Amazon SQS
//! queues, EventBridge relies on resource-based policies" (`PutTargets`), and the
//! call EventBridge makes on a queue target is `sqs:SendMessage` as the service
//! principal `events.amazonaws.com`. A service principal holds no identity-based
//! policies, so the queue's own `Policy` attribute is the ONLY document in force:
//! nothing is enqueued unless that document states an `Allow` about this principal,
//! this action and this queue; an explicit `Deny` about it refuses the delivery
//! whatever is granted beside it, in whichever order the two are written; and a
//! statement conditioned on `aws:SourceArn` or `aws:SourceAccount` authorizes the
//! deliveries of the rule and the account it names and no others. A refused delivery
//! is one that FAILED, so it is counted on the rule's `FailedInvocations` and sent to
//! the target's dead-letter queue — which is the same send by the same principal, so
//! that queue's own policy decides it the same way. What the dead letter REPORTS is
//! the answer Amazon SQS gave the refused `sqs:SendMessage`: that service models no
//! authorization failure on `SendMessage` at all, because it does not disclose a queue
//! it will not let the caller write to — so the answer is its not-found code at HTTP
//! 400 (`AWS.SimpleQueueService.NonExistentQueue`) under the fixed sentence that states
//! the absence and the want of access without saying which holds, and the condition
//! beside it is the class that answer's own code belongs to, which is the resource one
//! (`NO_RESOURCE`).
//!
//! The first test spans a TABLE whose rows differ along what the queue's document
//! states: whether there is one at all, which principal it names, which action and
//! which resource its statements are about, which `Effect` they state and in which
//! order, and which condition they are scoped by. Each row is a FRESH emulator, so
//! one row's document never decides another row's delivery.
//!
//! The condition operators the rows use (`ArnEquals`, `ArnLike`, `StringEquals`,
//! `Null`) and the keys they read are IAM's own, because a queue's `Policy` is an IAM
//! policy document — `smithy_query(query="doc",shape="com.amazonaws.sqs#QueueAttributeName")`
//! for the attribute itself.
//!
//! The harness (the router with EventBridge, Amazon SQS, Step Functions, IAM and
//! CloudWatch on one engine) is `tests/all/target_dispatch.rs`'s, which is also where
//! the grant a target queue needs is written once ([`granting_events`]).

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::{
    rule_arn, DEFAULT_EVENT_BUS_NAME, FAILED_INVOCATIONS_METRIC, INVOCATIONS_METRIC,
    NO_RESOURCE,
    SERVICE_PRINCIPAL,
};

use crate::target_dispatch::{
    answered_request_id, drain, emulator, given_queue, given_queue_with_policy, given_rule,
    granting_events, is_request_id, metric_sum_over, not_disclosed_answer_head, publish,
    rule_name_dimensions, sqs, Emulator, REGION,
};

/// The IAM action a delivery to a queue target performs on that queue.
const SEND_MESSAGE: &str = "sqs:SendMessage";

/// The dead-letter attributes naming what stopped the delivery.
const ERROR_CODE_ATTRIBUTE: &str = "ERROR_CODE";
const ERROR_MESSAGE_ATTRIBUTE: &str = "ERROR_MESSAGE";

/// The account every resource below belongs to.
fn account() -> &'static str {
    engine::emulator_account_id()
}

/// A statement list wrapped as a policy document.
fn document(statements: Value) -> String {
    json!({ "Version": "2012-10-17", "Statement": statements }).to_string()
}

/// An `Allow` of [`SEND_MESSAGE`] to the EventBridge service principal on
/// `queue_arn`, scoped by `condition` when the row states one.
fn grant(queue_arn: &str, condition: Option<Value>) -> Value {
    let mut statement = json!({
        "Effect": "Allow",
        "Principal": {"Service": SERVICE_PRINCIPAL},
        "Action": SEND_MESSAGE,
        "Resource": queue_arn,
    });
    if let Some(condition) = condition {
        statement["Condition"] = condition;
    }
    statement
}

/// A `Deny` of [`SEND_MESSAGE`] to the EventBridge service principal on `queue_arn`.
fn deny(queue_arn: &str) -> Value {
    json!({
        "Effect": "Deny",
        "Principal": {"Service": SERVICE_PRINCIPAL},
        "Action": SEND_MESSAGE,
        "Resource": queue_arn,
    })
}

/// The ARN of a rule of this account and Region that is NOT the one publishing below.
fn another_rule_arn() -> String {
    rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, "somebody-elses-rule")
}

/// How a row writes the document its queue carries: from the queue's own ARN, so a row
/// can state something about the very queue the document is installed on.
type PolicyFor = Box<dyn Fn(&str) -> String>;

/// One row of the table below: the case, the document the queue carries and whether
/// EventBridge delivers to it.
struct Row {
    case: &'static str,
    /// `None` is a queue carrying no `Policy` attribute at all.
    policy: Option<PolicyFor>,
    delivered: bool,
}

/// The queue's own `Policy` decides each delivery, over a table of documents.
#[tokio::test]
async fn a_queue_target_receives_only_what_its_own_policy_allows() {
    let rows: Vec<Row> = vec![
        Row {
            case: "no policy at all",
            policy: None,
            delivered: false,
        },
        Row {
            case: "an unconditional grant to the service principal",
            policy: Some(Box::new(|arn: &str| document(json!([grant(arn, None)])))),
            delivered: true,
        },
        Row {
            case: "a grant whose Action names every action of the service",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": "sqs:*",
                    "Resource": arn,
                }]))
            })),
            delivered: true,
        },
        Row {
            case: "a grant to every principal",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": SEND_MESSAGE,
                    "Resource": arn,
                }]))
            })),
            delivered: true,
        },
        Row {
            case: "a grant naming another service principal",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": "sns.amazonaws.com"},
                    "Action": SEND_MESSAGE,
                    "Resource": arn,
                }]))
            })),
            delivered: false,
        },
        Row {
            case: "a grant naming the account root rather than the service",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("arn:aws:iam::{}:root", account())},
                    "Action": SEND_MESSAGE,
                    "Resource": arn,
                }]))
            })),
            delivered: false,
        },
        Row {
            case: "a grant of another action of the same service",
            policy: Some(Box::new(|arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": "sqs:ReceiveMessage",
                    "Resource": arn,
                }]))
            })),
            delivered: false,
        },
        Row {
            case: "a grant whose Resource names another queue",
            policy: Some(Box::new(|_arn: &str| {
                document(json!([{
                    "Effect": "Allow",
                    "Principal": {"Service": SERVICE_PRINCIPAL},
                    "Action": SEND_MESSAGE,
                    "Resource": format!("arn:aws:sqs:{REGION}:{}:another-queue", account()),
                }]))
            })),
            delivered: false,
        },
        Row {
            case: "an explicit deny written after the grant",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(arn, None), deny(arn)]))
            })),
            delivered: false,
        },
        Row {
            case: "an explicit deny written before the grant",
            policy: Some(Box::new(|arn: &str| {
                document(json!([deny(arn), grant(arn, None)]))
            })),
            delivered: false,
        },
        Row {
            case: "a grant scoped by ArnEquals to the publishing rule",
            policy: Some(Box::new(|arn: &str| {
                let mine = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE);
                document(json!([grant(
                    arn,
                    Some(json!({"ArnEquals": {"aws:SourceArn": mine}}))
                )]))
            })),
            delivered: true,
        },
        Row {
            case: "a grant scoped by ArnLike to every rule of this account",
            policy: Some(Box::new(|arn: &str| {
                let any = format!("arn:aws:events:{REGION}:{}:rule/*", account());
                document(json!([grant(
                    arn,
                    Some(json!({"ArnLike": {"aws:SourceArn": any}}))
                )]))
            })),
            delivered: true,
        },
        Row {
            case: "a grant scoped by ArnEquals to another rule",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    arn,
                    Some(json!({"ArnEquals": {"aws:SourceArn": another_rule_arn()}}))
                )]))
            })),
            delivered: false,
        },
        Row {
            case: "a grant scoped by StringEquals to this account",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    arn,
                    Some(json!({"StringEquals": {"aws:SourceAccount": account()}}))
                )]))
            })),
            delivered: true,
        },
        Row {
            case: "a grant scoped by StringEquals to another account",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    arn,
                    Some(json!({"StringEquals": {"aws:SourceAccount": "210987654321"}}))
                )]))
            })),
            delivered: false,
        },
        Row {
            case: "a grant conditioned on there being no source at all",
            policy: Some(Box::new(|arn: &str| {
                document(json!([grant(
                    arn,
                    Some(json!({"Null": {"aws:SourceArn": "true"}}))
                )]))
            })),
            delivered: false,
        },
        Row {
            case: "an explicit deny scoped by ArnEquals to the publishing rule",
            policy: Some(Box::new(|arn: &str| {
                let mine = rule_arn(REGION, DEFAULT_EVENT_BUS_NAME, RULE);
                let mut denial = deny(arn);
                denial["Condition"] = json!({"ArnEquals": {"aws:SourceArn": mine}});
                document(json!([grant(arn, None), denial]))
            })),
            delivered: false,
        },
        Row {
            case: "an explicit deny scoped by ArnEquals to another rule",
            policy: Some(Box::new(|arn: &str| {
                let mut denial = deny(arn);
                denial["Condition"] = json!({"ArnEquals": {"aws:SourceArn": another_rule_arn()}});
                document(json!([grant(arn, None), denial]))
            })),
            delivered: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let queue = format!("policy-case-{index}");
        let policy = row
            .policy
            .as_ref()
            .map(|policy| policy.as_ref() as &dyn Fn(&str) -> String);
        let (url, arn) = given_queue_with_policy(&emulator, &queue, policy).await;
        given_rule(&emulator, RULE, json!([{ "Id": "queue", "Arn": arn }])).await;
        publish(&emulator).await;
        let received = drain(&emulator, &url).await;
        let expected = usize::from(row.delivered);
        assert_eq!(
            received.len(),
            expected,
            "case {case:?}: expected {expected} message(s) on the queue, got {received:?}"
        );
    }
}

/// The one rule every test here publishes through, named so a row's condition can
/// scope a grant to it.
const RULE: &str = "queue-policy-rule";

/// Each delivery is decided against the document the queue carries AT THAT INSTANT: a
/// grant written after a refused publication lets the next one through, and clearing
/// it stops the one after that.
#[tokio::test]
async fn a_queue_policy_decides_each_delivery_as_it_stands_then() {
    let emulator = emulator();
    let case = "a grant written and then cleared";
    let (url, arn) = given_queue_with_policy(&emulator, "late-grant", None).await;
    given_rule(&emulator, RULE, json!([{ "Id": "queue", "Arn": arn }])).await;

    publish(&emulator).await;
    let before = drain(&emulator, &url).await;
    assert!(
        before.is_empty(),
        "case {case:?}: expected no message before the grant was written, got {before:?}"
    );

    set_policy(&emulator, case, &url, &granting_events(&arn)).await;
    publish(&emulator).await;
    let granted = drain(&emulator, &url).await;
    assert_eq!(
        granted.len(),
        1,
        "case {case:?}: expected one message once the grant was written, got {granted:?}"
    );

    // An empty value is how Amazon SQS clears an attribute, which is what a client
    // withdrawing a queue policy writes.
    set_policy(&emulator, case, &url, "").await;
    publish(&emulator).await;
    let after = drain(&emulator, &url).await;
    assert!(
        after.is_empty(),
        "case {case:?}: expected no message once the grant was cleared, got {after:?}"
    );
}

/// Write `policy` as the `Policy` attribute of the queue at `url`.
async fn set_policy(emulator: &Emulator, case: &str, url: &str, policy: &str) {
    let (status, body) = sqs(
        emulator,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {body}"
    );
}

/// One row of the dead-letter table: the case, whether the dead-letter queue grants
/// the send, and how many dead letters it is to receive.
struct DeadLetterRow {
    case: &'static str,
    granted: bool,
    dead_letters: usize,
}

/// A delivery the target queue's policy withholds is DEAD-LETTERED, carrying the
/// condition that stopped it — and the dead-letter queue's own policy decides that
/// send the same way, so one which has granted this service nothing receives nothing
/// either.
#[tokio::test]
async fn a_withheld_delivery_is_dead_lettered_only_where_that_queue_allows_it() {
    let rows = [
        DeadLetterRow {
            case: "a dead-letter queue granting the service the send",
            granted: true,
            dead_letters: 1,
        },
        DeadLetterRow {
            case: "a dead-letter queue granting nothing",
            granted: false,
            dead_letters: 0,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        // The TARGET queue grants nothing in every row, so each row's delivery is
        // withheld and only the dead-letter queue's own document differs.
        let (target_url, target_arn) =
            given_queue_with_policy(&emulator, &format!("dlq-target-{index}"), None).await;
        let grant: Option<&dyn Fn(&str) -> String> = if row.granted {
            Some(&granting_events)
        } else {
            None
        };
        let (dlq_url, dlq_arn) =
            given_queue_with_policy(&emulator, &format!("dlq-sink-{index}"), grant).await;
        given_rule(
            &emulator,
            RULE,
            json!([{
                "Id": "queue",
                "Arn": target_arn,
                "DeadLetterConfig": { "Arn": dlq_arn },
            }]),
        )
        .await;
        publish(&emulator).await;

        let delivered = drain(&emulator, &target_url).await;
        assert!(
            delivered.is_empty(),
            "case {case:?}: the target queue granted nothing, so it must receive nothing; got \
             {delivered:?}"
        );
        let dead_lettered = drain(&emulator, &dlq_url).await;
        assert_eq!(
            dead_lettered.len(),
            row.dead_letters,
            "case {case:?}: expected {} dead letter(s), got {dead_lettered:?}",
            row.dead_letters
        );
        for message in &dead_lettered {
            let code = message["MessageAttributes"][ERROR_CODE_ATTRIBUTE]["StringValue"]
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                code, NO_RESOURCE,
                "case {case:?}: expected the dead letter to name the class of the answer that \
                 service gave, got {message:?}"
            );
            // The sentence is the one Amazon SQS answers a caller whose send the
            // queue's documents did not permit, as the refused delivery read it: that
            // service's own fixed text, where only the id of the refused request
            // varies.
            let expected = not_disclosed_answer_head();
            let reported = message["MessageAttributes"][ERROR_MESSAGE_ATTRIBUTE]["StringValue"]
                .as_str()
                .unwrap_or_default();
            let request_id = answered_request_id(reported, &expected).unwrap_or_else(|| {
                panic!("case {case:?}: expected the answer {expected:?}, got {reported:?}")
            });
            assert!(
                is_request_id(&request_id),
                "case {case:?}: expected a request id in the 8-4-4-4-12 form, got \
                 {request_id:?}"
            );
        }
    }
}

/// One row of the metric table: the case, whether the queue grants the send, and what
/// each of the rule's two handover counters is to total.
struct MetricRow {
    case: &'static str,
    granted: bool,
    invocations: f64,
    failed: f64,
}

/// A withheld delivery is a delivery that FAILED, so the rule counts it on
/// `FailedInvocations` and not on `Invocations` — where one the queue's policy allows
/// is counted the other way round.
#[tokio::test]
async fn a_withheld_delivery_is_counted_as_a_failed_invocation() {
    let rows = [
        MetricRow {
            case: "a queue granting the service the send",
            granted: true,
            invocations: 1.0,
            failed: 0.0,
        },
        MetricRow {
            case: "a queue granting nothing",
            granted: false,
            invocations: 0.0,
            failed: 1.0,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let emulator = emulator();
        let case = row.case;
        let queue = format!("metric-queue-{index}");
        let (_url, arn) = if row.granted {
            given_queue(&emulator, &queue).await
        } else {
            given_queue_with_policy(&emulator, &queue, None).await
        };
        given_rule(&emulator, RULE, json!([{ "Id": "queue", "Arn": arn }])).await;
        publish(&emulator).await;

        for (metric, expected) in [
            (INVOCATIONS_METRIC, row.invocations),
            (FAILED_INVOCATIONS_METRIC, row.failed),
        ] {
            let total = metric_sum_over(&emulator, metric, &rule_name_dimensions(RULE)).await;
            assert_eq!(
                total, expected,
                "case {case:?}: expected {metric} to total {expected}, got {total}"
            );
        }
    }
}
