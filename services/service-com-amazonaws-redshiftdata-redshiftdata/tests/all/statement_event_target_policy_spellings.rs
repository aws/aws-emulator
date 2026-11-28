// SPDX-License-Identifier: CC0-1.0

//! The QUEUE's own document decides whether a statement's announcement reaches it —
//! in every spelling the policy language gives that grant.
//!
//! The RULE asserted here: a submission that asked for an event ("a value that
//! indicates whether to send an event to the Amazon EventBridge event bus after the
//! SQL statement runs" —
//! `smithy_query(query="doc",shape="ExecuteStatementInput",member="WithEvent")`)
//! has its terminal status change published onto the default bus, and a rule
//! matching that envelope delivers it to its Amazon SQS target when the queue's own
//! resource policy admits the send EventBridge makes — which it makes as the
//! `events.amazonaws.com` service principal, on behalf of the rule. The policy
//! language gives one grant MANY spellings: a principal named alone or in a list, an
//! action named alone or in a list, the queue named by its ARN or by `*`, and a
//! grant scoped to the sending rule with any of the documented string and ARN
//! comparisons. A client's policy is a client's to write, so the announcement must
//! arrive under each of them — and must NOT arrive when the scope names a DIFFERENT
//! rule, which is what tells a real grant apart from a condition nothing evaluates.
//!
//! Each row runs on an emulator of its own, with ONE rule and ONE queue, because
//! every statement event of this service matches the same pattern: a second rule
//! would deliver the same announcement to a second queue and no row could then say
//! which document admitted it.
//!
//! Sources for the spellings: the IAM policy language's own elements (`Principal`,
//! `Action`, `Resource`, `Condition`), whose comparison operators for a scoped grant
//! are `StringEquals`/`StringLike` and `ArnEquals`/`ArnLike`; the condition keys a
//! grant to an EventBridge target is scoped by are `aws:SourceArn` (the rule making
//! the delivery) and `aws:SourceAccount` (the account it belongs to).

use serde_json::{json, Value};

use crate::emulator::Warehouses;
use crate::statement_status_change_events::{
    collect, drain, given_rule, statement_event_pattern, wait_for_terminal, POLL_INTERVAL,
};

/// The warehouse every row runs on, the database it holds and the user a statement
/// authenticates as.
const CLUSTER: &str = "policy-spelling-cluster";
const DATABASE: &str = "devdb";
const DB_USER: &str = "adminuser";

/// A statement the engine runs to completion.
const SQL: &str = "SELECT 1";

/// The service principal EventBridge delivers to an SQS target as.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The rule every row creates, and the queue it targets.
const RULE: &str = "statement-status-changes";
const QUEUE: &str = "statement-events";

/// The ARN of a rule of this account that no row created, for the row whose grant
/// is scoped to someone else's deliveries. Well formed and names nothing, so what
/// refuses the send is the SCOPE and not the shape of the value.
fn another_rules_arn(region: &str) -> String {
    format!(
        "arn:aws:events:{region}:{}:rule/another-rule",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// How many announcements a row watches for before it gives up on an absence.
///
/// A bound on an ABSENCE: the row that must receive NOTHING reads the queue this
/// many times, which is far more than the one read that finds a delivery the bus
/// made. Generous, because what it may not do is decide an absence before the bus
/// has had its chance.
const ABSENCE_READS: usize = 40;

/// One row: the statement of the queue's policy, and whether the announcement
/// reaches the queue under it.
struct Row {
    case: &'static str,
    /// The grant, built against the queue's ARN, the rule's ARN and the Region.
    statement: fn(queue_arn: &str, rule_arn: &str, region: &str) -> Value,
    /// Whether the announcement must be delivered to the queue.
    delivered: bool,
}

/// The table: one spelling of the same grant per row, and one scope that names
/// another rule.
const ROWS: &[Row] = &[
    Row {
        case: "the principal and the action named alone, the queue by its ARN",
        statement: |queue_arn, _rule_arn, _region| {
            json!({
                "Effect": "Allow",
                "Principal": {"Service": EVENTS_PRINCIPAL},
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
            })
        },
        delivered: true,
    },
    Row {
        case: "the grant scoped to the sending rule with ArnEquals",
        statement: |queue_arn, rule_arn, _region| {
            json!({
                "Sid": "AllowEventBridgeDelivery",
                "Effect": "Allow",
                "Principal": {"Service": EVENTS_PRINCIPAL},
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
                "Condition": {"ArnEquals": {"aws:SourceArn": rule_arn}},
            })
        },
        delivered: true,
    },
    Row {
        case: "the same scope spelled with StringEquals",
        statement: |queue_arn, rule_arn, _region| {
            json!({
                "Effect": "Allow",
                "Principal": {"Service": EVENTS_PRINCIPAL},
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
                "Condition": {"StringEquals": {"aws:SourceArn": rule_arn}},
            })
        },
        delivered: true,
    },
    Row {
        case: "the scope spelled with ArnLike over the account's rules",
        statement: |queue_arn, _rule_arn, region| {
            json!({
                "Effect": "Allow",
                "Principal": {"Service": EVENTS_PRINCIPAL},
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
                "Condition": {"ArnLike": {"aws:SourceArn": format!(
                    "arn:aws:events:{region}:{}:rule/*",
                    engine::EMULATOR_ACCOUNT_ID
                )}},
            })
        },
        delivered: true,
    },
    Row {
        case: "the scope spelled over the account the rule belongs to",
        statement: |queue_arn, _rule_arn, _region| {
            json!({
                "Effect": "Allow",
                "Principal": {"Service": EVENTS_PRINCIPAL},
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
                "Condition": {
                    "StringEquals": {"aws:SourceAccount": engine::EMULATOR_ACCOUNT_ID},
                },
            })
        },
        delivered: true,
    },
    Row {
        case: "the principal and the action named in lists, the queue by a wildcard",
        statement: |_queue_arn, _rule_arn, _region| {
            json!({
                "Effect": "Allow",
                "Principal": {"Service": [EVENTS_PRINCIPAL]},
                "Action": ["sqs:SendMessage", "sqs:GetQueueAttributes"],
                "Resource": "*",
            })
        },
        delivered: true,
    },
    Row {
        case: "a grant scoped to another rule's deliveries",
        statement: |queue_arn, _rule_arn, region| {
            json!({
                "Effect": "Allow",
                "Principal": {"Service": EVENTS_PRINCIPAL},
                "Action": "sqs:SendMessage",
                "Resource": queue_arn,
                "Condition": {"ArnEquals": {"aws:SourceArn": another_rules_arn(region)}},
            })
        },
        delivered: false,
    },
];

/// Create the queue `QUEUE` and answer its url and ARN, with no policy on it yet:
/// the policy is installed once the rule it is scoped to exists.
async fn given_queue(emulator: &Warehouses) -> (String, String) {
    let created = emulator
        .sqs_ok("CreateQueue", &json!({"QueueName": QUEUE}))
        .await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("a created queue carries its url: {created}"))
        .to_string();
    let attributes = emulator
        .sqs_ok(
            "GetQueueAttributes",
            &json!({"QueueUrl": &url, "AttributeNames": ["QueueArn"]}),
        )
        .await;
    let arn = attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("a queue carries its ARN: {attributes}"))
        .to_string();
    (url, arn)
}

/// Install `statement` as the whole of the queue's resource policy.
async fn given_policy(emulator: &Warehouses, url: &str, statement: &Value) {
    let policy = json!({"Version": "2012-10-17", "Statement": [statement]}).to_string();
    emulator
        .sqs_ok(
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": {"Policy": policy}}),
        )
        .await;
}

/// RULE: the queue's own document decides the delivery of a statement's
/// announcement, in every spelling of the grant — and a grant scoped to another
/// rule's deliveries admits none of this rule's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_queue_policy_decides_the_announcement_it_receives() {
    let region = crate::emulator::REGION;
    for row in ROWS {
        let case = row.case;
        let emulator = Warehouses::new();
        emulator
            .create_cluster_with_database(CLUSTER, Some(DATABASE))
            .await;
        let (url, queue_arn) = given_queue(&emulator).await;
        // The rule exists before the policy is written, so a grant scoped to it
        // names the ARN the service really minted for it.
        let rule_arn = {
            let put = emulator
                .events_ok(
                    "PutRule",
                    &json!({
                        "Name": RULE,
                        "EventBusName": "default",
                        "EventPattern": statement_event_pattern().to_string(),
                        "State": "ENABLED",
                    }),
                )
                .await;
            put["RuleArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a RuleArn in {put}"))
                .to_string()
        };
        given_policy(&emulator, &url, &(row.statement)(&queue_arn, &rule_arn, region)).await;
        given_rule(
            &emulator,
            RULE,
            &statement_event_pattern(),
            &[queue_arn.as_str()],
        )
        .await;

        let answered = emulator
            .data_ok(
                "ExecuteStatement",
                &json!({
                    "ClusterIdentifier": CLUSTER,
                    "Database": DATABASE,
                    "DbUser": DB_USER,
                    "Sql": SQL,
                    "WithEvent": true,
                    "StatementName": case,
                }),
            )
            .await;
        let id = answered["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Id in {answered}"))
            .to_string();
        let record = wait_for_terminal(&emulator, case, &id).await;
        assert_eq!(
            record["Status"],
            json!("FINISHED"),
            "case {case:?}: expected the statement to finish, got {record}"
        );

        if row.delivered {
            let delivered = collect(&emulator, &url, 1).await;
            let mine: Vec<&Value> = delivered
                .iter()
                .filter(|envelope| envelope.pointer("/detail/statementId") == Some(&json!(&id)))
                .collect();
            assert_eq!(
                mine.len(),
                1,
                "case {case:?}: expected the announcement of {id} on the queue this policy \
                 admits, got {mine:?} among {delivered:?}"
            );
            continue;
        }

        // The absence half: read the queue repeatedly, so the verdict is not taken
        // before the bus has had its chance to deliver.
        for _read in 0..ABSENCE_READS {
            let leaked = drain(&emulator, &url).await;
            assert!(
                leaked.is_empty(),
                "case {case:?}: a queue whose grant is scoped to another rule's deliveries \
                 receives none of this rule's, got {leaked:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}
